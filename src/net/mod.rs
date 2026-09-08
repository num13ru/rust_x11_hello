//! Persistent TCP transport for semantic activation events and PaperSpoon
//! control.
//!
//! The Kindle connects out to PaperSpoon once at launch and keeps the
//! connection for the run. Each activation writes one newline-terminated
//! `event action=<semantic-id>;` line over the connection. A reader thread
//! consumes inbound PaperSpoon lines (control commands such as
//! `display <text>`) and publishes the latest into a mailbox the X11 event loop
//! drains between events and on a bounded idle poll interval.
//!
//! If an established connection drops, the background reconnector retries the
//! same endpoint. A startup connection failure remains disconnected, but never
//! breaks the X11 event loop or the on-device activation log.

use paper_protocol::{format_action_line, parse_display_command};

mod connection;
pub mod discover;

use crate::config::PaperpadConfig;
use anyhow::{Context, Result};
use connection::ConnectionState;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const TCP_CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
/// Maximum complete inbound TCP line, including its newline when present.
/// Status rendering policy remains separate; this limit only bounds framing.
const MAX_INBOUND_LINE_BYTES: usize = 8 * 1024;

#[derive(Clone, Default)]
struct DisplayMailbox {
    pending: Arc<Mutex<Option<String>>>,
}

impl DisplayMailbox {
    fn publish(&self, text: String) {
        *self.pending.lock().expect("display mailbox lock") = Some(text);
    }

    fn take(&self) -> Option<String> {
        self.pending.lock().expect("display mailbox lock").take()
    }
}

/// A persistent outbound connection to PaperSpoon with an inbound message
/// queue drained by the X11 event loop.
pub struct Paperspoon {
    /// Shared with the reconnector thread; owns the active socket.
    connection: Arc<Mutex<ConnectionState>>,
    display: DisplayMailbox,
    wake_tx: Option<Sender<()>>,
    stopping: Arc<AtomicBool>,
    reconnector: Option<JoinHandle<()>>,
}

fn spawn_reader(stream: TcpStream, display: DisplayMailbox, wake_tx: Sender<()>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        while let Ok(Some(line)) = read_inbound_line(&mut reader) {
            if let Some(text) = parse_display_command(&line) {
                display.publish(text);
            }
        }
        let _ = wake_tx.send(());
    })
}

fn read_inbound_line<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_INBOUND_LINE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > MAX_INBOUND_LINE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PaperSpoon inbound line exceeds 8192 bytes",
        ));
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Resolve the PaperSpoon address.
///
/// - If `RUST_X11_HELLO_COMPANION` is set, resolve it directly (explicit
///   host control path).
/// - Otherwise run zero-config UDP discovery; do not fall back to a
///   hard-coded IP when discovery fails (that would hide the experiment's
///   result).
fn paperspoon_addr(host: Option<&str>, port: u16) -> Result<SocketAddr> {
    match host {
        // An explicit, non-empty override resolves directly; empty or
        // missing falls through to discovery.
        Some(host) if !host.trim().is_empty() => (host, port)
            .to_socket_addrs()
            .context("failed to resolve PaperSpoon address")?
            .next()
            .context("PaperSpoon address resolved to nothing"),
        _ => crate::net::discover::discover_paperspoon(),
    }
}

impl Paperspoon {
    /// Create a disconnected PaperSpoon without a reconnect worker.
    /// Startup failures and later sends remain non-fatal to the X11 loop.
    pub fn disconnected() -> Self {
        Self {
            connection: Arc::new(Mutex::new(ConnectionState::disconnected())),
            display: DisplayMailbox::default(),
            wake_tx: None,
            stopping: Arc::new(AtomicBool::new(false)),
            reconnector: None,
        }
    }

    /// Connect to PaperSpoon and start the reader thread.
    pub fn connect(config: &PaperpadConfig) -> Result<Self> {
        let addr = paperspoon_addr(config.host(), config.port())?;
        Self::connect_to(addr)
    }

    /// Connect to a specific PaperSpoon address and start the reader thread.
    ///
    /// Internal and `pub(crate)`: tests inject an ephemeral listener this
    /// way so they never depend on process-global env vars.
    pub(crate) fn connect_to(addr: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT)
            .context("failed to connect to PaperSpoon")?;
        let _ = stream.set_nodelay(true);

        // The socket is shared with a reconnector thread. When the reader
        // hits EOF, it clears the shared slot and wakes the reconnector,
        // which retries until PaperSpoon is reachable again and swaps in a
        // fresh socket — proactive auto-reconnect without user input.
        let connection = Arc::new(Mutex::new(ConnectionState::connected(stream)));
        let display = DisplayMailbox::default();
        let (wake_tx, wake_rx) = mpsc::channel::<()>();

        let reader_stream = connection
            .lock()
            .expect("shared stream lock")
            .clone_stream()?
            .into_stream();
        let reader_handle = spawn_reader(reader_stream, display.clone(), wake_tx.clone());

        // Reconnector: wake on EOF, clear slot, retry connect until success,
        // install fresh socket + reader, then wait for the next EOF.
        let worker_connection = Arc::clone(&connection);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let worker_display = display.clone();
        let worker_wake_tx = wake_tx.clone();
        let reconnector = std::thread::spawn(move || {
            let mut active_reader = Some(reader_handle);
            'reconnector: loop {
                if wake_rx.recv().is_err() {
                    break;
                }
                if let Some(reader) = active_reader.take() {
                    let _ = reader.join();
                }
                worker_connection
                    .lock()
                    .expect("shared stream lock")
                    .disconnect();
                if worker_stopping.load(Ordering::Acquire) {
                    break;
                }
                loop {
                    if worker_stopping.load(Ordering::Acquire) {
                        break 'reconnector;
                    }
                    match TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT) {
                        Ok(new_stream) => {
                            let _ = new_stream.set_nodelay(true);
                            let fresh_reader = new_stream.try_clone().expect("fresh clone");
                            {
                                let mut guard =
                                    worker_connection.lock().expect("shared stream lock");
                                if worker_stopping.load(Ordering::Acquire) {
                                    let _ = new_stream.shutdown(Shutdown::Both);
                                    break 'reconnector;
                                }
                                guard.reconnect(new_stream);
                            }
                            active_reader = Some(spawn_reader(
                                fresh_reader,
                                worker_display.clone(),
                                worker_wake_tx.clone(),
                            ));
                            break;
                        }
                        Err(_) => std::thread::sleep(TCP_CONNECT_TIMEOUT),
                    }
                }
            }
        });

        Ok(Self {
            connection,
            display,
            wake_tx: Some(wake_tx),
            stopping,
            reconnector: Some(reconnector),
        })
    }

    /// Send one semantic activation over the persistent connection.
    ///
    /// Writes to the shared stream; if the reconnector has not yet finished
    /// restoring it, the write fails fast and the caller logs it (the
    /// reconnector keeps retrying in the background).
    pub fn send_action(&mut self, semantic_id: &str) -> Result<()> {
        let line = format_action_line(semantic_id);
        let mut write_stream = {
            let guard = self.connection.lock().expect("shared stream lock");
            guard.clone_stream()?
        };
        if let Err(error) = write_stream.stream_mut().write_all(line.as_bytes()) {
            self.connection
                .lock()
                .expect("shared stream lock")
                .disconnect_if_current(&write_stream);
            return Err(error).context("failed to write action to PaperSpoon");
        }
        Ok(())
    }

    /// Drain any display commands received since the last call.
    ///
    /// Returns the latest pending display text, if any, and empties the
    /// single-slot mailbox. It never blocks on network I/O.
    pub fn poll_display(&mut self) -> Option<String> {
        self.display.take()
    }
}

impl Drop for Paperspoon {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);

        self.connection
            .lock()
            .expect("shared stream lock")
            .disconnect();
        if let Some(wake_tx) = self.wake_tx.take() {
            let _ = wake_tx.send(());
        }
        if let Some(reconnector) = self.reconnector.take() {
            let _ = reconnector.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn accept_before(listener: &std::net::TcpListener, timeout: Duration) -> TcpStream {
        listener.set_nonblocking(true).expect("set nonblocking");
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline, "accept timed out");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        }
    }

    #[test]
    fn explicit_host_resolves_directly() {
        let addr = paperspoon_addr(Some("127.0.0.1"), 6000).expect("explicit host must resolve");
        assert_eq!(addr, SocketAddr::from(([127, 0, 0, 1], 6000)));
    }

    #[test]
    fn display_mailbox_coalesces_to_latest_unread_text() {
        let mailbox = DisplayMailbox::default();
        assert_eq!(mailbox.take(), None);
        mailbox.publish("first".to_string());
        mailbox.publish("second".to_string());
        assert_eq!(mailbox.take(), Some("second".to_string()));
        assert_eq!(mailbox.take(), None);
    }

    #[test]
    fn inbound_line_reader_preserves_lines_semantics() {
        let mut input = Cursor::new(b"display first\r\ndisplay second\nfinal".to_vec());
        assert_eq!(
            read_inbound_line(&mut input).expect("read CRLF line"),
            Some("display first".to_string())
        );
        assert_eq!(
            read_inbound_line(&mut input).expect("read LF line"),
            Some("display second".to_string())
        );
        assert_eq!(
            read_inbound_line(&mut input).expect("read final line"),
            Some("final".to_string())
        );
        assert_eq!(read_inbound_line(&mut input).expect("read EOF"), None);
    }

    #[test]
    fn inbound_line_reader_accepts_exact_limit() {
        let mut input = Cursor::new(vec![b'x'; MAX_INBOUND_LINE_BYTES]);
        let line = read_inbound_line(&mut input)
            .expect("read bounded line")
            .expect("line");
        assert_eq!(line.len(), MAX_INBOUND_LINE_BYTES);
    }

    #[test]
    fn inbound_line_reader_rejects_oversize_and_invalid_utf8() {
        let mut oversized = Cursor::new(vec![b'x'; MAX_INBOUND_LINE_BYTES + 1]);
        assert_eq!(
            read_inbound_line(&mut oversized)
                .expect_err("oversized line must fail")
                .kind(),
            io::ErrorKind::InvalidData
        );

        let mut invalid_utf8 = Cursor::new(vec![0xff, b'\n']);
        assert_eq!(
            read_inbound_line(&mut invalid_utf8)
                .expect_err("invalid UTF-8 must fail")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn oversized_inbound_line_reconnects_without_peer_eof() {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        let mut paperspoon = Paperspoon::connect_to(addr).expect("connect");
        let mut first_peer = accept_before(&listener, Duration::from_secs(2));

        let mut oversized = vec![b'x'; MAX_INBOUND_LINE_BYTES + 1];
        oversized.push(b'\n');
        first_peer
            .write_all(&oversized)
            .expect("write oversized line");

        // Keep the original peer open: a second accept proves the reader
        // rejected the frame and woke the reconnector instead of seeing EOF.
        let mut replacement_peer = accept_before(&listener, Duration::from_secs(5));
        writeln!(replacement_peer, "display recovered").expect("write recovered display");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if paperspoon.poll_display() == Some("recovered".to_string()) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no display after oversized-line reconnect"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn auto_reconnects_when_paperspoon_returns() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::Duration;

        // Start a listener (PaperSpoon), connect, then stop it to force EOF.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        let mut paperspoon = Paperspoon::connect_to(addr).expect("connect");
        let mut first = accept_before(&listener, Duration::from_secs(2));
        writeln!(first, "display first").expect("write first");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if paperspoon.poll_display() == Some("first".to_string()) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "no first display");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(first);
        drop(listener); // Force EOF: the reconnector must notice.

        // A new PaperSpoon appears; the reconnector should restore the stream
        // without any send_action call from us. The reconnector loops forever
        // retrying `addr` (now free), so rebinding the SAME port and
        // accepting proves the auto-restore.
        let listener3 = TcpListener::bind(addr).expect("rebind same port");
        let mut reconnected_peer = accept_before(&listener3, Duration::from_secs(5));
        writeln!(reconnected_peer, "display again").expect("write again");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if paperspoon.poll_display() == Some("again".to_string()) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no reconnected display"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        // Drop must stop and join the reader even while its peer remains idle.
        reconnected_peer
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set peer deadline");
        drop(paperspoon);
        let mut byte = [0];
        assert_eq!(reconnected_peer.read(&mut byte).expect("read EOF"), 0);
    }
}
