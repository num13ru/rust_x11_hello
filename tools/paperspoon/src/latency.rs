//! Asynchronous JSONL latency tracing for PaperSpoon's pointer/action path.

use paper_protocol::V2PointerTrace;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Stamp {
    pub(crate) wall_us: u64,
    pub(crate) mono_us: u64,
}

static STARTED: OnceLock<Instant> = OnceLock::new();
static LOGGER: OnceLock<Option<Sender<String>>> = OnceLock::new();

pub(crate) fn init(path: &str) {
    let _ = STARTED.set(Instant::now());
    let _ = LOGGER.set(start_logger(path));
}

fn start_logger(path: &str) -> Option<Sender<String>> {
    let file = match File::create(path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("latency log disabled: failed to create {path}: {error}");
            return None;
        }
    };
    let (tx, rx) = mpsc::channel::<String>();
    let path_owned = path.to_string();
    match std::thread::Builder::new()
        .name("paperspoon-latency-log".to_string())
        .spawn(move || {
            let mut writer = BufWriter::new(file);
            while let Ok(line) = rx.recv() {
                if writeln!(writer, "{line}")
                    .and_then(|()| writer.flush())
                    .is_err()
                {
                    eprintln!("latency log write failed: {path_owned}");
                    break;
                }
            }
        }) {
        Ok(_) => {
            eprintln!("latency log path={path}");
            Some(tx)
        }
        Err(error) => {
            eprintln!("latency log disabled: failed to start writer: {error}");
            None
        }
    }
}

pub(crate) fn stamp() -> Stamp {
    let wall_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    let mono_us = STARTED
        .get_or_init(Instant::now)
        .elapsed()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64;
    Stamp { wall_us, mono_us }
}

pub(crate) fn record_at(stamp: Stamp, stage: &str, trace: Option<V2PointerTrace>, fields: &str) {
    let Some(Some(logger)) = LOGGER.get() else {
        return;
    };
    let correlation = trace
        .map(|trace| {
            format!(
                ",\"session\":{},\"trace\":{},\"origin_mono_us\":{}",
                trace.session_id(),
                trace.trace_id(),
                trace.origin_mono_us()
            )
        })
        .unwrap_or_default();
    let line = format!(
        "{{\"wall_us\":{},\"mono_us\":{},\"component\":\"paperspoon\",\"stage\":\"{stage}\"{correlation}{fields}}}",
        stamp.wall_us, stamp.mono_us
    );
    let _ = logger.send(line);
}

pub(crate) fn record(stage: &str, trace: Option<V2PointerTrace>, fields: &str) {
    record_at(stamp(), stage, trace, fields);
}
