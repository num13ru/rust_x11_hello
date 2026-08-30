//! Minimal TCP companion for Kindle semantic activations.
//!
//! Listens on `0.0.0.0:<port>` (default 5581). For each accepted connection:
//!
//! - lines from the Kindle (`event action=<semantic-id>;`) are printed to
//!   stdout and appended with a unix timestamp and peer address to a log
//!   file (default `companion.log`);
//! - lines typed on stdin are forwarded to the Kindle as control commands
//!   (`display <text>`).
//!
//! Usage: `companion [port] [log-file]`

use std::env;
use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let port: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5581);
    let log_path = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "companion.log".to_string());

    let listener = TcpListener::bind(("0.0.0.0", port))?;
    println!("listening on 0.0.0.0:{port}, logging to {log_path}");
    println!("type 'display <text>' to send a control command");
    io::stdout().flush()?;

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

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;

        // Forward stdin (operator control commands) to the Kindle.
        let mut write_stream = stream.try_clone()?;
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
                let _ = writeln!(write_stream, "{line}");
            }
        });

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
        println!("disconnected: {peer}");
        io::stdout().flush()?;
    }
    Ok(())
}