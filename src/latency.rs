//! Low-overhead JSONL latency tracing kept off the input and transport paths.

use paper_protocol::V2PointerTrace;
use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const LATENCY_LOG_ENV: &str = "PAPERPAD_LATENCY_LOG";

#[derive(Clone, Copy, Debug)]
pub(crate) struct Stamp {
    pub(crate) wall_us: u64,
    pub(crate) mono_us: u64,
}

static STARTED: OnceLock<Instant> = OnceLock::new();
static SESSION_ID: OnceLock<u64> = OnceLock::new();
static NEXT_TRACE_ID: AtomicU64 = AtomicU64::new(1);
static LOGGER: OnceLock<Option<Sender<String>>> = OnceLock::new();

pub(crate) fn init_from_env() {
    let _ = STARTED.set(Instant::now());
    let _ = session_id();
    let _ = LOGGER.set(match env::var(LATENCY_LOG_ENV) {
        Ok(path) if !path.trim().is_empty() => start_logger(&path),
        _ => None,
    });
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
        .name("paperpad-latency-log".to_string())
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

fn session_id() -> u64 {
    *SESSION_ID.get_or_init(|| {
        let wall = stamp().wall_us;
        let pid = u64::from(std::process::id());
        let candidate = wall.rotate_left(17) ^ pid;
        candidate.max(1)
    })
}

pub(crate) fn new_trace(origin: Stamp) -> V2PointerTrace {
    let trace_id = NEXT_TRACE_ID.fetch_add(1, Ordering::Relaxed);
    V2PointerTrace::new(session_id(), trace_id, origin.mono_us)
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
        .unwrap_or_else(|| format!(",\"session\":{}", session_id()));
    let line = format!(
        "{{\"wall_us\":{},\"mono_us\":{},\"component\":\"paperpad\",\"stage\":\"{stage}\"{correlation}{fields}}}",
        stamp.wall_us, stamp.mono_us
    );
    let _ = logger.send(line);
}

pub(crate) fn record(stage: &str, trace: Option<V2PointerTrace>, fields: &str) {
    record_at(stamp(), stage, trace, fields);
}
