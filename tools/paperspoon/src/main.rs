//! Minimal TCP PaperSpoon listener for Kindle semantic activations.
//!
//! Listens on `0.0.0.0:<port>` (default 5581). For each accepted connection:
//!
//! - lines from the Kindle (`event action=<semantic-id>;`) are printed to
//!   stdout and appended with a unix timestamp and peer address to a log
//!   file (default `paperspoon.log`);
//! - lines typed on stdin are forwarded to the Kindle as control commands
//!   (`display <text>`);
//! - received action lines are also sent as UDP datagrams to a local
//!   consumer (default `127.0.0.1:5584`) so Hammerspoon can dispatch them
//!   without opening another TCP port. One datagram per action line: the
//!   consumer fires exactly once per datagram, so actions never replay.
//!
//! Usage: `paperspoon [<port> <log-file>] [--forward-udp <port> |
//!         --no-forward-udp]`
//!
//! Only the most recently accepted connection receives stdin control lines.
//! The Kindle reconnects across runs, and each accepted socket would get its
//! own stdin reader racing on the process-global stdin lock; a stale reader
//! for an earlier (dead) connection would swallow operator lines forever.
//! One forwarder thread therefore writes every stdin line to the current
//! connection, replaced on each accept.
//!
//! UDP forwarding is best-effort and stateless: if the consumer is not
//! listening when an action arrives, the datagram is dropped (UDP semantics)
//! and the next action is delivered normally. The Kindle-facing accept loop
//! never blocks on the forward path.

use std::env;
use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::time::{SystemTime, UNIX_EPOCH};

use paper_protocol::{DISCOVERY_PORT, parse_action_line};

mod discovery;
mod server;

use server::CurrentConnection;

/// Default TCP port. Must match `rust_x11_hello`'s `COMPANION_PORT`.
const DEFAULT_PORT: u16 = paper_protocol::DEFAULT_TCP_PORT;
/// Default log file name for received activation lines.
const DEFAULT_LOG_FILE: &str = "paperspoon.log";
/// Whether to forward actions to Hammerspoon via `open -g hammerspoon://`.
const FORWARD_TO_HAMMERSPOON: bool = true;

/// Parsed command line.
struct Options {
    port: u16,
    log_path: String,
    /// None means URL forwarding is disabled.
    forward_url: bool,
}

fn parse_options(args: &[String]) -> Options {
    let mut port = DEFAULT_PORT;
    let mut log_path = DEFAULT_LOG_FILE.to_string();
    let mut forward_url = FORWARD_TO_HAMMERSPOON;
    let positional = args.iter().skip(1);
    for arg in positional {
        match arg.as_str() {
            "--no-forward-url" => {
                forward_url = false;
            }
            _ => {
                // Positional: try port first, then log file.
                if port == DEFAULT_PORT
                    && let Ok(parsed) = arg.parse()
                {
                    port = parsed;
                    continue;
                }
                if log_path == DEFAULT_LOG_FILE {
                    log_path = arg.clone();
                    continue;
                }
            }
        }
    }
    Options {
        port,
        log_path,
        forward_url,
    }
}

/// Forward one action line to Hammerspoon via `open -g hammerspoon://`.
/// The action id is passed as a query parameter (`?action=<id>`); Hammerspoon's
/// urlevent handler receives it in the params table. No sockets, no ports,
/// no file polling.
fn forward_url(action_id: &str) -> io::Result<()> {
    std::process::Command::new("open")
        .arg("-g")
        .arg(format!("hammerspoon://paperpad?action={}", action_id))
        .status()?;
    Ok(())
}

fn flush_stdout(context: &str) {
    if let Err(error) = io::stdout().flush() {
        eprintln!("stdout flush error {context}: {error}");
    }
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let opts = parse_options(&args);

    let listener = TcpListener::bind(("0.0.0.0", opts.port))?;
    let tcp_port = listener.local_addr()?.port();
    let discovery_socket = discovery::bind_discovery_socket()?;
    let _discovery_worker = std::thread::Builder::new()
        .name("paperspoon-discovery".to_string())
        .spawn(move || {
            if let Err(error) = discovery::run_discovery_listener(discovery_socket, tcp_port) {
                eprintln!("discovery listener error: {error}");
            }
        })?;

    println!(
        "listening on 0.0.0.0:{}, logging to {}",
        tcp_port, opts.log_path
    );
    println!("discovery listening address=0.0.0.0:{DISCOVERY_PORT}");

    println!("type 'display <text>' to send a control command");
    if opts.forward_url {
        println!("forwarding actions to Hammerspoon via open -g hammerspoon://paperpad/...");
    } else {
        println!("action forwarding to Hammerspoon disabled");
    }
    io::stdout().flush()?;

    let current = CurrentConnection::default();

    let _stdin_worker = {
        let current = current.clone();
        std::thread::Builder::new()
            .name("paperspoon-stdin".to_string())
            .spawn(move || {
                let stdin = io::stdin();
                for line in stdin.lock().lines() {
                    let line = match line {
                        Ok(line) => line,
                        Err(error) => {
                            eprintln!("stdin read error: {error}");
                            break;
                        }
                    };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if let Err(error) = current.forward_line(line) {
                        eprintln!("control write error: {error}");
                    }
                }
            })?
    };

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
        flush_stdout("after connected banner");

        // This is now the active Kindle connection for control lines.
        let connection_token = match current.install(&stream) {
            Ok(token) => token,
            Err(e) => {
                eprintln!("clone error for {peer}: {e}");
                continue;
            }
        };

        let mut file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&opts.log_path)
        {
            Ok(f) => f,
            Err(e) => {
                eprintln!("log open error for {peer}: {e}");
                current.clear_if_current(&connection_token);
                continue;
            }
        };
        let reader = BufReader::new(&mut stream);
        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    eprintln!("TCP read error from {peer}: {error}");
                    break;
                }
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            println!("received from {peer}: {line}");
            flush_stdout("after received line");

            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            // Never let a log write or flush failure tear down the connection.
            if let Err(error) = writeln!(file, "{ts} {peer} {line}") {
                eprintln!("log write error: {error}");
            } else if let Err(error) = file.flush() {
                eprintln!("log flush error: {error}");
            }

            if opts.forward_url
                && let Some(action_id) = parse_action_line(line)
                && let Err(error) = forward_url(action_id)
            {
                eprintln!("forward error to Hammerspoon: {error}");
            }
        }
        current.clear_if_current(&connection_token);

        println!("disconnected: {peer}");
        flush_stdout("after disconnected banner");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_options_keeps_backward_compatible_positional_args() {
        let opts = parse_options(&[
            "paperspoon".to_string(),
            "5581".to_string(),
            "/tmp/paperspoon.log".to_string(),
        ]);
        assert_eq!(opts.port, 5581);
        assert_eq!(opts.log_path, "/tmp/paperspoon.log");
        assert!(opts.forward_url);
    }

    #[test]
    fn parse_options_forward_url_default_on() {
        let opts = parse_options(&["paperspoon".to_string()]);
        assert!(opts.forward_url);
    }

    #[test]
    fn parse_options_without_forward_url_disables() {
        let opts = parse_options(&["paperspoon".to_string(), "--no-forward-url".to_string()]);
        assert!(!opts.forward_url);
    }

    #[test]
    fn parse_options_positional_and_forward_url_coexist() {
        let opts = parse_options(&[
            "paperspoon".to_string(),
            "5582".to_string(),
            "/tmp/x.log".to_string(),
            "--no-forward-url".to_string(),
        ]);
        assert_eq!(opts.port, 5582);
        assert_eq!(opts.log_path, "/tmp/x.log");
        assert!(!opts.forward_url);
    }
}
