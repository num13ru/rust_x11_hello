//! PaperSpoon's operator-facing output, separate from persistent records.

use std::collections::VecDeque;
use std::fmt::{self, Write as _};
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, TryLockError};

const MAX_RECORDS: usize = 5_000;
const MAX_MESSAGE_BYTES: usize = 4_096;
const TRUNCATED: char = '…';

static CAPTURE: OnceLock<LogBuffer> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[macro_export]
macro_rules! operator_info {
    ($($arg:tt)*) => {
        $crate::operator_log::write($crate::operator_log::Stream::Stdout, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! operator_error {
    ($($arg:tt)*) => {
        $crate::operator_log::write($crate::operator_log::Stream::Stderr, format_args!($($arg)*))
    };
}

pub use crate::{operator_error as error, operator_info as info};

/// Capture subsequent operator output for the terminal UI. Call before starting workers.
pub fn start_capture() -> &'static LogBuffer {
    CAPTURE.get_or_init(LogBuffer::new)
}

pub fn write(stream: Stream, message: fmt::Arguments<'_>) {
    if let Some(log) = CAPTURE.get() {
        log.push(stream, message);
        return;
    }
    match stream {
        Stream::Stdout => println!("{message}"),
        Stream::Stderr => eprintln!("{message}"),
    }
}

pub fn flush() -> io::Result<()> {
    if CAPTURE.get().is_some() {
        return Ok(());
    }
    io::stdout().flush()
}

pub fn flush_stdout(context: &str) {
    if let Err(cause) = flush() {
        error!("stdout flush error {context}: {cause}");
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogRecord {
    pub stream: Stream,
    pub message: String,
}

pub enum LogPosition {
    Follow,
    From(u64),
}

pub struct LogWindow {
    /// ID of the oldest retained record, equal to the number of evictions.
    pub first_id: u64,
    /// ID at the start of this window after clamping it to retained history.
    pub start_id: u64,
    /// ID the next retained record will receive.
    pub next_id: u64,
    /// Records discarded when the history was busy.
    pub dropped: u64,
    pub records: Vec<LogRecord>,
}

pub struct LogBuffer {
    history: Mutex<History>,
    dropped: AtomicU64,
}

struct History {
    records: VecDeque<LogRecord>,
    next_id: u64,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl LogBuffer {
    /// Creates an empty, bounded log store.
    pub fn new() -> Self {
        Self {
            history: Mutex::new(History {
                records: VecDeque::with_capacity(MAX_RECORDS),
                next_id: 0,
            }),
            dropped: AtomicU64::new(0),
        }
    }

    /// Pushes one record built from a string. Used by tests and simple callers.
    pub fn push_str(&self, stream: Stream, message: &str) {
        self.push(stream, format_args!("{message}"));
    }

    fn push(&self, stream: Stream, message: fmt::Arguments<'_>) {
        let record = LogRecord {
            stream,
            message: BoundedMessage::format(message),
        };
        let mut history = match self.history.try_lock() {
            Ok(history) => history,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        if history.records.len() == MAX_RECORDS {
            history.records.pop_front();
        }
        history.records.push_back(record);
        history.next_id += 1;
    }

    /// Copy at most `limit` records for a single display window.
    pub fn window(&self, position: LogPosition, limit: usize) -> LogWindow {
        let history = self
            .history
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let next_id = history.next_id;
        let first_id = next_id - history.records.len() as u64;
        let start_id = match position {
            LogPosition::Follow => next_id.saturating_sub(limit as u64).max(first_id),
            LogPosition::From(id) => id.clamp(first_id, next_id),
        };
        let start = (start_id - first_id) as usize;
        let end = start + limit.min(history.records.len() - start);
        LogWindow {
            first_id,
            start_id,
            next_id,
            dropped: self.dropped.load(Ordering::Relaxed),
            records: history.records.range(start..end).cloned().collect(),
        }
    }
}

struct BoundedMessage(String);

impl BoundedMessage {
    fn format(message: fmt::Arguments<'_>) -> String {
        let mut result = Self(String::new());
        if result.write_fmt(message).is_err() {
            while result.0.len() > MAX_MESSAGE_BYTES - TRUNCATED.len_utf8() {
                result.0.pop();
            }
            result.0.push(TRUNCATED);
        }
        result.0
    }

    fn push(&mut self, character: char) -> fmt::Result {
        if self.0.len() + character.len_utf8() > MAX_MESSAGE_BYTES {
            return Err(fmt::Error);
        }
        self.0.push(character);
        Ok(())
    }
}

impl fmt::Write for BoundedMessage {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for character in text.chars() {
            if character.is_control() {
                for escaped in character.escape_debug() {
                    self.push(escaped)?;
                }
            } else {
                self.push(character)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn window_follows_tail_or_stays_anchored_until_evicted() {
        let log = LogBuffer::new();
        for id in 0..MAX_RECORDS + 5 {
            log.push(Stream::Stdout, format_args!("record {id}"));
        }

        let evicted = log.window(LogPosition::From(0), 2);
        assert_eq!(evicted.first_id, 5);
        assert_eq!(evicted.start_id, 5);
        assert_eq!(evicted.records.len(), 2);
        assert_eq!(evicted.records[0].message, "record 5");

        let anchored = log.window(LogPosition::From(8), 2);
        let tail = log.window(LogPosition::Follow, 2);
        assert_eq!(tail.next_id, MAX_RECORDS as u64 + 5);
        assert_eq!(tail.start_id, tail.next_id - 2);
        assert_eq!(
            tail.records[1].message,
            format!("record {}", MAX_RECORDS + 4)
        );

        log.push(Stream::Stderr, format_args!("new message"));
        let still_anchored = log.window(LogPosition::From(8), 2);
        assert_eq!(still_anchored.start_id, 8);
        assert_eq!(still_anchored.records, anchored.records);
        assert_eq!(still_anchored.dropped, 0);
        assert_eq!(
            log.window(LogPosition::Follow, 1).records[0].message,
            "new message"
        );
    }

    #[test]
    fn producer_drops_instead_of_waiting_for_history() {
        let log = LogBuffer::new();
        let held = log.history.lock().unwrap();
        std::thread::scope(|scope| {
            let (finished, done) = mpsc::channel();
            let log = &log;
            scope.spawn(move || {
                log.push(Stream::Stderr, format_args!("while history is busy"));
                finished.send(()).unwrap();
            });
            let result = done.recv_timeout(Duration::from_secs(1));
            drop(held);
            result.expect("producer must finish while history is held");
        });

        let window = log.window(LogPosition::Follow, 2);
        assert_eq!(window.dropped, 1);
        assert_eq!(window.next_id, 0);
        assert!(window.records.is_empty());
    }

    #[test]
    fn capture_routes_each_stream_with_bounded_terminal_safe_text() {
        let log = start_capture();
        info!("path café 🦀\n\r\t\x1b[2J\u{009b}");
        error!("");
        let fits = "a".repeat(MAX_MESSAGE_BYTES);
        info!("{fits}");
        let oversized = "🦀".repeat(MAX_MESSAGE_BYTES);
        info!("{oversized} should be outside the message");
        flush().unwrap();

        let window = log.window(LogPosition::Follow, 4);
        assert_eq!(window.records.len(), 4);
        assert_eq!(window.records[0].stream, Stream::Stdout);
        assert_eq!(
            window.records[0].message,
            "path café 🦀\\n\\r\\t\\u{1b}[2J\\u{9b}"
        );
        assert_eq!(window.records[1].stream, Stream::Stderr);
        assert!(window.records[1].message.is_empty());
        assert_eq!(window.records[2].message, fits);
        let truncated = &window.records[3].message;
        assert!(truncated.len() <= MAX_MESSAGE_BYTES);
        assert!(truncated.ends_with(TRUNCATED));
        assert!(truncated.chars().all(|ch| ch == '🦀' || ch == TRUNCATED));
    }
}
