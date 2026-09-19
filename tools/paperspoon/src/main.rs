//! TCP PaperSpoon listener for host-rendered PaperPad sessions.
//!
//! Listens on `0.0.0.0:<port>` (default 5581). For each accepted connection:
//!
//! - protocol-v2 pointer records from the Kindle are logged and resolved by
//!   the host-owned application UI;
//! - `frame <pattern> <width>x<height>` generates and sends a binary v2
//!   diagnostic framebuffer;
//! - resolved host action IDs are optionally forwarded to Hammerspoon with
//!   `open -g hammerspoon://paperpad?action=<id>`.
//!
//! Usage: `paperspoon [<port> <log-file>] [--no-forward-url]`
//!
//! Only the most recently accepted connection receives stdin control lines.
//! The Kindle reconnects across runs, and each accepted socket would get its
//! own stdin reader racing on the process-global stdin lock; a stale reader
//! for an earlier (dead) connection would swallow operator lines forever.
//! One forwarder thread therefore writes every stdin line to the current
//! connection, replaced on each accept.
//!
//! URL-forwarding failures are reported without discarding the already logged
//! action. Passing `--no-forward-url` disables the Hammerspoon launch.

use std::env;
use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use paper_protocol::{DISCOVERY_PORT, V2PointerPhase};
use paperspoon::{
    application_input::ApplicationInput, application_renderer::encode_application,
    application_ui::ApplicationUi,
};

mod diagnostic;
mod discovery;
mod inbound;
mod server;

use diagnostic::{StdinCommand, parse_stdin_command};
use inbound::{SessionMessage, read_session_hello, read_session_message};
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

fn matching_application_viewport(
    reported_viewport: (u16, u16),
    rendered_application_viewport: Option<(u16, u16)>,
) -> Option<(u16, u16)> {
    rendered_application_viewport.filter(|viewport| *viewport == reported_viewport)
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

    println!(
        "type 'frame <white|black|horizontal|checkerboard|border|corners> <width>x<height>' to send a diagnostic framebuffer"
    );
    println!("type 'ui <width>x<height>' to send the host-rendered application UI");
    if opts.forward_url {
        println!("forwarding actions to Hammerspoon via open -g hammerspoon://paperpad/...");
    } else {
        println!("action forwarding to Hammerspoon disabled");
    }
    io::stdout().flush()?;

    let current = CurrentConnection::default();
    let (application_viewport_tx, application_viewport_rx) = mpsc::channel();

    let _stdin_worker = {
        let current = current.clone();
        std::thread::Builder::new()
            .name("paperspoon-stdin".to_string())
            .spawn(move || {
                let stdin = io::stdin();
                let mut next_frame_id = 1_u64;
                let application_ui = ApplicationUi::default();
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
                    match parse_stdin_command(line) {
                Ok(StdinCommand::Frame(frame)) => {
                            let encoded = match frame.encode(next_frame_id) {
                                Ok(encoded) => encoded,
                                Err(error) => {
                                    eprintln!("frame command error: {error}");
                                    continue;
                                }
                            };
                            match current.forward_bytes(&encoded) {
                                Ok(true) => {
                                    if application_viewport_tx.send(None).is_err() {
                                        eprintln!("application viewport tracker stopped");
                                    }
                                    let (width, height) = frame.dimensions();
                                    println!(
                                        "sent frame id={next_frame_id} pattern={} width={width} height={height} bytes={}",
                                        frame.pattern_name(),
                                        encoded.len()
                                    );
                                    flush_stdout("after sent frame");
                                    next_frame_id = next_frame_id.wrapping_add(1);
                                }
                                Ok(false) => {
                                    eprintln!("frame not sent: no PaperPad connected");
                                }
                                Err(error) => eprintln!("frame write error: {error}"),
                            }
                        }
                        Ok(StdinCommand::ApplicationFrame { width, height }) => {
                            let encoded = match encode_application(
                                next_frame_id,
                                &application_ui,
                                width,
                                height,
                            ) {
                                Ok(encoded) => encoded,
                                Err(error) => {
                                    eprintln!("ui command error: {error}");
                                    continue;
                                }
                            };
                            match current.forward_bytes(&encoded) {
                                Ok(true) => {
                                    if application_viewport_tx.send(Some((width, height))).is_err() {
                                        eprintln!("application viewport tracker stopped");
                                    }
                                    println!(
                                        "sent application frame id={next_frame_id} width={width} height={height} bytes={}",
                                        encoded.len()
                                    );
                                    flush_stdout("after sent application frame");
                                    next_frame_id = next_frame_id.wrapping_add(1);
                                }
                                Ok(false) => {
                                    eprintln!("application frame not sent: no PaperPad connected");
                                }
                                Err(error) => eprintln!("application frame write error: {error}"),
                            }
                        }
                        Err(error) => eprintln!("stdin command error: {error}"),
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

        // A new connection has not received any application frame yet.
        while application_viewport_rx.try_recv().is_ok() {}

        let mut file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&opts.log_path)
        {
            Ok(f) => f,
            Err(e) => {
                eprintln!("log open error for {peer}: {e}");
                continue;
            }
        };
        let mut reader = BufReader::new(&mut stream);
        let hello = match read_session_hello(&mut reader) {
            Ok(Some(hello)) => hello,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("TCP read error from {peer}: {error}");
                continue;
            }
        };
        let hello_record = format!(
            "hello protocol={} viewport={}x{}",
            hello.protocol_version(),
            hello.viewport_width(),
            hello.viewport_height()
        );
        println!("received from {peer}: {hello_record}");
        flush_stdout("after received Hello");
        let hello_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        if let Err(error) = writeln!(file, "{hello_ts} {peer} {hello_record}") {
            eprintln!("log write error: {error}");
        } else if let Err(error) = file.flush() {
            eprintln!("log flush error: {error}");
        }

        // A valid Hello makes this the active Kindle connection for frames.
        let connection_token = match current.install(reader.get_ref()) {
            Ok(token) => token,
            Err(e) => {
                eprintln!("clone error for {peer}: {e}");
                continue;
            }
        };
        let application_ui = ApplicationUi::default();
        let mut application_input = ApplicationInput::default();
        let mut reported_viewport = (hello.viewport_width(), hello.viewport_height());
        let mut rendered_application_viewport = None;
        loop {
            let message = match read_session_message(&mut reader) {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(error) => {
                    eprintln!("TCP read error from {peer}: {error}");
                    break;
                }
            };
            let mut rendered_viewport_changed = false;
            for viewport in application_viewport_rx.try_iter() {
                rendered_application_viewport = viewport;
                rendered_viewport_changed = true;
            }
            let pointer = match message {
                SessionMessage::ViewportChanged(viewport) => {
                    reported_viewport = (viewport.width(), viewport.height());
                    application_input.set_viewport(matching_application_viewport(
                        reported_viewport,
                        rendered_application_viewport,
                    ));
                    let record = format!(
                        "viewport changed width={} height={}",
                        viewport.width(),
                        viewport.height()
                    );
                    println!("received from {peer}: {record}");
                    flush_stdout("after received viewport change");
                    let ts = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0);
                    if let Err(error) = writeln!(file, "{ts} {peer} {record}") {
                        eprintln!("log write error: {error}");
                    } else if let Err(error) = file.flush() {
                        eprintln!("log flush error: {error}");
                    }
                    continue;
                }
                SessionMessage::Pointer(pointer) => pointer,
            };
            if rendered_viewport_changed {
                application_input.set_viewport(matching_application_viewport(
                    reported_viewport,
                    rendered_application_viewport,
                ));
            }
            let phase = match pointer.phase() {
                V2PointerPhase::Down => "down",
                V2PointerPhase::Up => "up",
            };
            let record = format!("pointer phase={phase} x={} y={}", pointer.x(), pointer.y());
            let host_activation = application_input.handle_pointer(&application_ui, pointer);
            println!("received from {peer}: {record}");
            flush_stdout("after received line");

            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            // Never let a log write or flush failure tear down the connection.
            if let Err(error) = writeln!(file, "{ts} {peer} {record}") {
                eprintln!("log write error: {error}");
            } else if let Err(error) = file.flush() {
                eprintln!("log flush error: {error}");
            }

            if let Some(activation) = host_activation {
                let dispatch = if opts.forward_url {
                    match forward_url(activation.action_id) {
                        Ok(()) => "forwarded",
                        Err(error) => {
                            eprintln!("forward error to Hammerspoon: {error}");
                            "failed"
                        }
                    }
                } else {
                    "disabled"
                };
                let host_record = format!(
                    "host action button={} semantic={} dispatch={dispatch}",
                    activation.button_id, activation.action_id
                );
                println!("resolved for {peer}: {host_record}");
                flush_stdout("after host action");
                if let Err(error) = writeln!(file, "{ts} {peer} {host_record}") {
                    eprintln!("log write error: {error}");
                } else if let Err(error) = file.flush() {
                    eprintln!("log flush error: {error}");
                }
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
    fn application_input_activates_only_for_frame_matching_reported_viewport() {
        assert_eq!(
            matching_application_viewport((1272, 1624), Some((1272, 1624))),
            Some((1272, 1624))
        );
        assert_eq!(
            matching_application_viewport((800, 600), Some((1272, 1624))),
            None
        );
        assert_eq!(matching_application_viewport((800, 600), None), None);
    }

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
