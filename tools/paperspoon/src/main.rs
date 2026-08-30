//! Minimal TCP PaperSpoon listener for Kindle semantic activations.
//!
//! Listens on `0.0.0.0:<port>` (default 5581). For each accepted connection:
//!
//! - lines from the Kindle (`event action=<semantic-id>;`) are printed to
//!   stdout and appended with a unix timestamp and peer address to a log
//!   file (default `paperspoon.log`);
//! - lines typed on stdin are forwarded to the Kindle as control commands
//!   (`display <text>`).
//!
//! Usage: `paperspoon [port] [log-file]`
//!
//! Only the most recently accepted connection receives stdin control lines.
//! The Kindle reconnects across runs, and each accepted socket would get its
//! own stdin reader racing on the process-global stdin lock; a stale reader
//! for an earlier (dead) connection would swallow operator lines forever.
//! One forwarder thread therefore writes every stdin line to the current
//! connection, replaced on each accept.

use std::env;
use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default TCP port. Must match `rust_x11_hello`'s `COMPANION_PORT`.
const DEFAULT_PORT: u16 = 5581;
/// Default log file name for received activation lines.
const DEFAULT_LOG_FILE: &str = "paperspoon.log";
fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let port: u16 = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let log_path = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| DEFAULT_LOG_FILE.to_string());

    let listener = TcpListener::bind(("0.0.0.0", port))?;
    println!("listening on 0.0.0.0:{port}, logging to {log_path}");
    println!("type 'display <text>' to send a control command");
    io::stdout().flush()?;

    // The connection operator control lines go to. Set to the most recent
    // accept; the single forwarder below writes each stdin line to it.
    let current: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));

    {
        let current = Arc::clone(&current);
        std::thread::spawn(move || {
            let stdin = io::stdin();
            for line in stdin.lock().lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => break,
                };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                // Clone a fresh socket handle out of the lock: TcpStream is
                // not Clone, and the lock must not be held across the write
                // (the accept loop sets `current` under the same lock).
                if let Some(stream) = current.lock().expect("current lock").as_ref() {
                    if let Ok(mut stream) = stream.try_clone() {
                        let _ = writeln!(stream, "{line}");
                    }
                }
            }
        });
    }

    for conn in listener.incoming() {
        let mut stream = match conn {
            Ok(s) => s,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };
        let peer = stream
            .peer_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        println!("connected: {peer}");
        io::stdout().flush()?;

        // This is now the active Kindle connection for control lines.
        let write_stream = stream.try_clone()?;
        *current.lock().expect("current lock") = Some(write_stream);

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;

        let reader = BufReader::new(&mut stream);
        for line in reader.lines() {
            let line = line.unwrap_or_default();
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            println!("received from {peer}: {line}");
            io::stdout().flush()?;

            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            writeln!(file, "{ts} {peer} {line}")?;
            file.flush()?;
        }

        // Drop the current slot only if it still belongs to this connection,
        // so a newer accept is not clobbered by an older one finishing late.
        let stale = current
            .lock()
            .expect("current lock")
            .as_ref()
            .and_then(|s| s.peer_addr().ok())
            == Some(peer);
        if stale {
            *current.lock().expect("current lock") = None;
        }

        println!("disconnected: {peer}");
        io::stdout().flush()?;
    }
    Ok(())
}
