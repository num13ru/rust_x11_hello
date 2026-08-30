//! Minimal TCP companion for Kindle semantic activations.
//!
//! Listens on `0.0.0.0:<port>` (default 5581), reads newline-terminated
//! protocol lines (`event action=<semantic-id>;`), prints each to stdout
//! and appends it with a unix timestamp and peer address to a log file
//! (default `companion.log`).
//!
//! Usage: `companion [port] [log-file]`

use std::env;
use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let port: u16 = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5581);
    let log_path = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "companion.log".to_string());

    let listener = TcpListener::bind(("0.0.0.0", port))?;
    println!("listening on 0.0.0.0:{port}, logging to {log_path}");
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

        let mut file = OpenOptions::new().create(true).append(true).open(&log_path)?;

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
    }
    Ok(())
}