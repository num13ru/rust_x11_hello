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
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use paper_protocol::{DISCOVERY_PORT, V2PointerPhase};
use paperspoon::{
    application_input::ApplicationInput, application_renderer::encode_application,
    application_ui::ApplicationUi,
};

mod diagnostic;
mod discovery;
mod inbound;
mod server;

use diagnostic::{DiagnosticFrame, StdinCommand, parse_stdin_command};
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

fn write_log_record(file: &mut impl Write, origin: &str, record: &str) -> io::Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let line = format!("{timestamp} {origin} {record}\n");
    file.write_all(line.as_bytes())?;
    file.flush()
}

fn append_host_log_record(path: &str, record: &str) {
    let result = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| write_log_record(&mut file, "host", record));
    if let Err(error) = result {
        eprintln!("frame log error: {error}");
    }
}

fn matching_application_viewport(
    reported_viewport: (u16, u16),
    rendered_application_viewport: Option<(u16, u16)>,
) -> Option<(u16, u16)> {
    rendered_application_viewport.filter(|viewport| *viewport == reported_viewport)
}

#[derive(Clone)]
struct FrameSender {
    current: CurrentConnection,
    state: Arc<Mutex<FrameSenderState>>,
}

struct FrameSenderState {
    next_frame_id: u64,
    authoritative: Option<AuthoritativeFrame>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthoritativeFrame {
    Application { viewport: (u16, u16) },
    Diagnostic(DiagnosticFrame),
}

impl AuthoritativeFrame {
    fn dimensions(self) -> (u16, u16) {
        match self {
            Self::Application { viewport } => viewport,
            Self::Diagnostic(frame) => frame.dimensions(),
        }
    }

    fn application_viewport(self) -> Option<(u16, u16)> {
        match self {
            Self::Application { viewport } => Some(viewport),
            Self::Diagnostic(_) => None,
        }
    }

    fn encode(self, ui: &ApplicationUi, frame_id: u64) -> Result<Vec<u8>, String> {
        match self {
            Self::Application { viewport } => {
                encode_application(frame_id, ui, viewport.0, viewport.1)
            }
            Self::Diagnostic(frame) => frame.encode(frame_id),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SentFrame {
    frame_id: u64,
    encoded_len: usize,
    application_viewport: Option<(u16, u16)>,
    encode_elapsed: Duration,
    socket_write_elapsed: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HelloFrame {
    sent: SentFrame,
    retained: bool,
    frame: AuthoritativeFrame,
}

impl FrameSender {
    fn new(current: CurrentConnection) -> Self {
        Self {
            current,
            state: Arc::new(Mutex::new(FrameSenderState {
                next_frame_id: 1,
                authoritative: None,
            })),
        }
    }

    #[cfg(test)]
    fn send_with(
        &self,
        encode: impl FnOnce(u64) -> Result<Vec<u8>, String>,
    ) -> Result<Option<SentFrame>, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        self.send_locked(&mut state, encode, None)
    }

    fn send_locked(
        &self,
        state: &mut FrameSenderState,
        encode: impl FnOnce(u64) -> Result<Vec<u8>, String>,
        authoritative: Option<AuthoritativeFrame>,
    ) -> Result<Option<SentFrame>, String> {
        let frame_id = state.next_frame_id;
        let next_frame_id = frame_id
            .checked_add(1)
            .ok_or_else(|| "frame ID exhausted".to_string())?;
        let encode_started = Instant::now();
        let encoded = encode(frame_id)?;
        let encode_elapsed = encode_started.elapsed();
        let encoded_len = encoded.len();
        // write_all measures local socket acceptance, not remote receipt or display.
        let socket_write_started = Instant::now();
        let sent = self
            .current
            .forward_bytes(&encoded)
            .map_err(|error| format!("failed write frame: {error}"))?;
        let socket_write_elapsed = socket_write_started.elapsed();
        if !sent {
            return Ok(None);
        }
        state.next_frame_id = next_frame_id;
        if let Some(frame) = authoritative {
            state.authoritative = Some(frame);
        }
        Ok(Some(SentFrame {
            frame_id,
            encoded_len,
            application_viewport: authoritative.and_then(AuthoritativeFrame::application_viewport),
            encode_elapsed,
            socket_write_elapsed,
        }))
    }

    fn send_authoritative(
        &self,
        ui: &ApplicationUi,
        frame: AuthoritativeFrame,
    ) -> Result<Option<SentFrame>, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        self.send_locked(
            &mut state,
            |frame_id| frame.encode(ui, frame_id),
            Some(frame),
        )
    }

    fn send_diagnostic(
        &self,
        ui: &ApplicationUi,
        frame: DiagnosticFrame,
    ) -> Result<Option<SentFrame>, String> {
        self.send_authoritative(ui, AuthoritativeFrame::Diagnostic(frame))
    }

    fn send_application(
        &self,
        ui: &ApplicationUi,
        viewport: (u16, u16),
    ) -> Result<Option<SentFrame>, String> {
        self.send_authoritative(ui, AuthoritativeFrame::Application { viewport })
    }

    fn send_for_hello(
        &self,
        ui: &ApplicationUi,
        viewport: (u16, u16),
    ) -> Result<Option<HelloFrame>, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        let retained = state
            .authoritative
            .filter(|frame| frame.dimensions() == viewport);
        let frame = retained.unwrap_or(AuthoritativeFrame::Application { viewport });
        self.send_locked(
            &mut state,
            |frame_id| frame.encode(ui, frame_id),
            Some(frame),
        )
        .map(|sent| {
            sent.map(|sent| HelloFrame {
                sent,
                retained: retained.is_some(),
                frame,
            })
        })
    }
}

fn replace_application_frame(
    frame_sender: &FrameSender,
    ui: &ApplicationUi,
    input: &mut ApplicationInput,
    rendered_viewport: &mut Option<(u16, u16)>,
    viewport: (u16, u16),
) -> Result<Option<SentFrame>, String> {
    *rendered_viewport = None;
    input.set_viewport(None);
    let sent = frame_sender.send_application(ui, viewport)?;
    if sent.is_some() {
        *rendered_viewport = Some(viewport);
        input.set_viewport(Some(viewport));
    }
    Ok(sent)
}

fn replace_hello_frame(
    frame_sender: &FrameSender,
    ui: &ApplicationUi,
    input: &mut ApplicationInput,
    rendered_viewport: &mut Option<(u16, u16)>,
    viewport: (u16, u16),
) -> Result<Option<HelloFrame>, String> {
    *rendered_viewport = None;
    input.set_viewport(None);
    let sent = frame_sender.send_for_hello(ui, viewport)?;
    if let Some(hello_frame) = sent {
        *rendered_viewport = hello_frame.sent.application_viewport;
        input.set_viewport(hello_frame.sent.application_viewport);
    }
    Ok(sent)
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
    let frame_sender = FrameSender::new(current.clone());
    let (application_viewport_tx, application_viewport_rx) = mpsc::channel();

    let _stdin_worker = {
        let frame_sender = frame_sender.clone();
        let log_path = opts.log_path.clone();
        std::thread::Builder::new()
            .name("paperspoon-stdin".to_string())
            .spawn(move || {
                let stdin = io::stdin();
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
                        match frame_sender.send_diagnostic(&application_ui, frame) {
                            Ok(Some(sent)) => {
                                if application_viewport_tx.send(None).is_err() {
                                    eprintln!("application viewport tracker stopped");
                                }
                                let (width, height) = frame.dimensions();
                                let record = format!(
                                    "sent diagnostic frame id={} pattern={} width={width} height={height} bytes={} encode_us={} socket_write_us={} source=stdin",
                                    sent.frame_id,
                                    frame.pattern_name(),
                                    sent.encoded_len,
                                    sent.encode_elapsed.as_micros(),
                                    sent.socket_write_elapsed.as_micros()
                                );
                                println!("{record}");
                                flush_stdout("after sent frame");
                                append_host_log_record(&log_path, &record);
                            }
                            Ok(None) => {
                                eprintln!("frame not sent: no PaperPad connected");
                            }
                            Err(error) => eprintln!("frame command error: {error}"),
                        }
                    }
                    Ok(StdinCommand::ApplicationFrame { width, height }) => {
                        match frame_sender.send_application(&application_ui, (width, height)) {
                            Ok(Some(sent)) => {
                                if application_viewport_tx.send(Some((width, height))).is_err() {
                                    eprintln!("application viewport tracker stopped");
                                }
                                let record = format!(
                                    "sent application frame id={} width={width} height={height} bytes={} encode_us={} socket_write_us={} source=stdin",
                                    sent.frame_id,
                                    sent.encoded_len,
                                    sent.encode_elapsed.as_micros(),
                                    sent.socket_write_elapsed.as_micros()
                                );
                                println!("{record}");
                                flush_stdout("after sent application frame");
                                append_host_log_record(&log_path, &record);
                            }
                            Ok(None) => {
                                eprintln!("application frame not sent: no PaperPad connected");
                            }
                            Err(error) => eprintln!("ui command error: {error}"),
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
        if let Err(error) = write_log_record(&mut file, &peer.to_string(), &hello_record) {
            eprintln!("log write error: {error}");
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
        let mut reported_viewport = (hello.viewport_width(), hello.viewport_height());
        let mut application_input = ApplicationInput::default();
        let mut rendered_application_viewport = None;
        match replace_hello_frame(
            &frame_sender,
            &application_ui,
            &mut application_input,
            &mut rendered_application_viewport,
            reported_viewport,
        ) {
            Ok(Some(hello_frame)) => {
                let sent = hello_frame.sent;
                let (kind, pattern) = match hello_frame.frame {
                    AuthoritativeFrame::Application { .. } => ("application", String::new()),
                    AuthoritativeFrame::Diagnostic(frame) => {
                        ("diagnostic", format!(" pattern={}", frame.pattern_name()))
                    }
                };
                let record = format!(
                    "sent {kind} frame id={}{pattern} width={} height={} bytes={} encode_us={} socket_write_us={} source=hello retained={}",
                    sent.frame_id,
                    reported_viewport.0,
                    reported_viewport.1,
                    sent.encoded_len,
                    sent.encode_elapsed.as_micros(),
                    sent.socket_write_elapsed.as_micros(),
                    hello_frame.retained
                );
                println!("{record}");
                flush_stdout("after automatic hello frame");
                if let Err(error) = write_log_record(&mut file, &peer.to_string(), &record) {
                    eprintln!("frame log error: {error}");
                }
            }
            Ok(None) => {
                eprintln!("automatic hello frame not sent: no PaperPad connected");
            }
            Err(error) => {
                eprintln!("automatic hello frame error: {error}");
            }
        }
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
                    let record = format!(
                        "viewport changed width={} height={}",
                        viewport.width(),
                        viewport.height()
                    );
                    println!("received from {peer}: {record}");
                    flush_stdout("after received viewport change");
                    if let Err(error) = write_log_record(&mut file, &peer.to_string(), &record) {
                        eprintln!("log write error: {error}");
                    }
                    match replace_application_frame(
                        &frame_sender,
                        &application_ui,
                        &mut application_input,
                        &mut rendered_application_viewport,
                        reported_viewport,
                    ) {
                        Ok(Some(sent)) => {
                            let record = format!(
                                "sent application frame id={} width={} height={} bytes={} encode_us={} socket_write_us={} source=viewport_changed",
                                sent.frame_id,
                                reported_viewport.0,
                                reported_viewport.1,
                                sent.encoded_len,
                                sent.encode_elapsed.as_micros(),
                                sent.socket_write_elapsed.as_micros()
                            );
                            println!("{record}");
                            flush_stdout("after viewport replacement frame");
                            if let Err(error) =
                                write_log_record(&mut file, &peer.to_string(), &record)
                            {
                                eprintln!("frame log error: {error}");
                            }
                        }
                        Ok(None) => {
                            eprintln!("viewport replacement frame not sent: no PaperPad connected");
                        }
                        Err(error) => {
                            eprintln!("viewport replacement frame error: {error}");
                        }
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

            // Never let a log write or flush failure tear down the connection.
            if let Err(error) = write_log_record(&mut file, &peer.to_string(), &record) {
                eprintln!("log write error: {error}");
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
                if let Err(error) = write_log_record(&mut file, &peer.to_string(), &host_record) {
                    eprintln!("log write error: {error}");
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
    use paper_protocol::{V2DecodeResult, V2Payload, decode_v2_message, decode_v2_payload};
    use std::io::Read;
    use std::net::{Shutdown, TcpStream};

    fn install_connection(
        listener: &TcpListener,
        current: &CurrentConnection,
    ) -> (server::ConnectionToken, TcpStream) {
        let client = TcpStream::connect(listener.local_addr().expect("listener address"))
            .expect("connect client");
        let (peer, _) = listener.accept().expect("accept client");
        let token = current.install(&client).expect("install connection");
        (token, peer)
    }

    fn read_sent_frame(peer: &mut TcpStream, sent: SentFrame) -> (u64, (u16, u16), Vec<u8>) {
        let mut encoded = vec![0; sent.encoded_len];
        peer.read_exact(&mut encoded).expect("read encoded frame");
        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(&encoded).expect("decode frame")
        else {
            panic!("complete frame expected");
        };
        assert_eq!(consumed, sent.encoded_len);
        let V2Payload::Frame(frame) = decode_v2_payload(message).expect("typed frame") else {
            panic!("frame payload expected");
        };
        (
            frame.frame_id(),
            (frame.width(), frame.height()),
            frame.pixels().to_vec(),
        )
    }

    fn diagnostic(command: &str) -> DiagnosticFrame {
        let StdinCommand::Frame(frame) = parse_stdin_command(command).expect("diagnostic command")
        else {
            panic!("diagnostic frame expected");
        };
        frame
    }

    #[test]
    fn exhausted_frame_id_is_rejected_before_encoding() {
        let frame_sender = FrameSender::new(CurrentConnection::default());
        frame_sender
            .state
            .lock()
            .expect("frame sender lock")
            .next_frame_id = u64::MAX;
        assert_eq!(
            frame_sender.send_with(|_| panic!("must not encode")),
            Err("frame ID exhausted".to_string())
        );
    }

    #[test]
    fn matching_hello_resends_retained_diagnostic_with_fresh_id() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let ui = ApplicationUi::default();
        let (token, mut first_peer) = install_connection(&listener, &current);
        let border = diagnostic("frame border 9x9");

        let first_sent = frame_sender
            .send_diagnostic(&ui, border)
            .expect("send border")
            .expect("active connection");
        let first = read_sent_frame(&mut first_peer, first_sent);
        assert_eq!(first.0, 1);
        assert_eq!(first.1, (9, 9));
        assert_eq!(first_sent.application_viewport, None);

        assert!(current.clear_if_current(&token));
        assert_eq!(
            frame_sender
                .send_diagnostic(&ui, diagnostic("frame checkerboard 9x9"))
                .expect("no-connection send"),
            None
        );
        frame_sender
            .send_application(&ui, (0, 9))
            .expect_err("failed encoding must not replace retained frame");

        let broken_client = TcpStream::connect(listener.local_addr().expect("listener address"))
            .expect("connect broken client");
        let (broken_peer, _) = listener.accept().expect("accept broken client");
        current
            .install(&broken_client)
            .expect("install broken connection");
        broken_client
            .shutdown(Shutdown::Both)
            .expect("shut down broken connection");
        drop(broken_peer);
        frame_sender
            .send_diagnostic(&ui, diagnostic("frame checkerboard 9x9"))
            .expect_err("failed write must not replace retained frame");

        let (_token, mut second_peer) = install_connection(&listener, &current);
        let mut input = ApplicationInput::default();
        let mut rendered_viewport = Some((9, 9));
        let resent = replace_hello_frame(
            &frame_sender,
            &ui,
            &mut input,
            &mut rendered_viewport,
            (9, 9),
        )
        .expect("resend retained frame")
        .expect("active replacement connection");
        let second = read_sent_frame(&mut second_peer, resent.sent);

        assert!(resent.retained);
        assert_eq!(resent.sent.application_viewport, None);
        assert!(!input.is_active());
        assert_eq!(rendered_viewport, None);
        assert_eq!(second.0, 2);
        assert_eq!(second.1, first.1);
        assert_eq!(second.2, first.2);
    }

    #[test]
    fn matching_hello_resends_application_and_enables_input() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let ui = ApplicationUi::default();
        let (token, mut first_peer) = install_connection(&listener, &current);

        let first_sent = frame_sender
            .send_application(&ui, (128, 128))
            .expect("send application")
            .expect("active connection");
        let first = read_sent_frame(&mut first_peer, first_sent);
        assert!(current.clear_if_current(&token));

        let (_token, mut second_peer) = install_connection(&listener, &current);
        let mut input = ApplicationInput::default();
        let mut rendered_viewport = None;
        let resent = replace_hello_frame(
            &frame_sender,
            &ui,
            &mut input,
            &mut rendered_viewport,
            (128, 128),
        )
        .expect("resend retained application")
        .expect("active replacement connection");
        let second = read_sent_frame(&mut second_peer, resent.sent);

        assert!(resent.retained);
        assert_eq!(resent.sent.application_viewport, Some((128, 128)));
        assert!(input.is_active());
        assert_eq!(rendered_viewport, Some((128, 128)));
        assert_eq!(second.0, 2);
        assert_eq!(second.1, first.1);
        assert_eq!(second.2, first.2);
    }

    #[test]
    fn mismatched_hello_replaces_retained_frame_with_default_application() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let ui = ApplicationUi::default();
        let (token, mut first_peer) = install_connection(&listener, &current);

        let first_sent = frame_sender
            .send_diagnostic(&ui, diagnostic("frame border 9x9"))
            .expect("send diagnostic")
            .expect("active connection");
        let _ = read_sent_frame(&mut first_peer, first_sent);
        assert!(current.clear_if_current(&token));

        let (_token, mut second_peer) = install_connection(&listener, &current);
        let mut input = ApplicationInput::default();
        let mut rendered_viewport = None;
        let replacement = replace_hello_frame(
            &frame_sender,
            &ui,
            &mut input,
            &mut rendered_viewport,
            (128, 128),
        )
        .expect("render fallback application")
        .expect("active replacement connection");
        let decoded = read_sent_frame(&mut second_peer, replacement.sent);

        assert!(!replacement.retained);
        assert_eq!(replacement.sent.application_viewport, Some((128, 128)));
        assert!(input.is_active());
        assert_eq!(rendered_viewport, Some((128, 128)));
        assert_eq!(decoded.0, 2);
        assert_eq!(decoded.1, (128, 128));
    }

    #[test]
    fn application_frame_replacement_tracks_only_successful_send() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let client = TcpStream::connect(listener.local_addr().expect("listener address"))
            .expect("connect client");
        let (mut peer, _) = listener.accept().expect("accept client");
        let current = CurrentConnection::default();
        current.install(&client).expect("install connection");
        let frame_sender = FrameSender::new(current);
        let mut input = ApplicationInput::default();
        let mut rendered_viewport = None;

        let sent = replace_application_frame(
            &frame_sender,
            &ApplicationUi::default(),
            &mut input,
            &mut rendered_viewport,
            (128, 128),
        )
        .expect("send application frame")
        .expect("active connection");
        assert!(input.is_active());
        assert_eq!(rendered_viewport, Some((128, 128)));
        let mut encoded = vec![0; sent.encoded_len];
        peer.read_exact(&mut encoded).expect("read encoded frame");
        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(&encoded).expect("decode frame")
        else {
            panic!("complete frame expected");
        };
        assert_eq!(consumed, sent.encoded_len);
        let V2Payload::Frame(frame) = decode_v2_payload(message).expect("typed frame") else {
            panic!("frame payload expected");
        };
        assert_eq!(frame.frame_id(), 1);
        assert_eq!((frame.width(), frame.height()), (128, 128));

        replace_application_frame(
            &frame_sender,
            &ApplicationUi::default(),
            &mut input,
            &mut rendered_viewport,
            (0, 128),
        )
        .expect_err("zero-width replacement must fail");
        assert!(!input.is_active());
        assert_eq!(rendered_viewport, None);
    }

    #[test]
    fn concurrent_frame_producers_keep_ids_in_wire_order() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let client = TcpStream::connect(listener.local_addr().expect("listener address"))
            .expect("connect client");
        let (mut peer, _) = listener.accept().expect("accept client");
        let current = CurrentConnection::default();
        current.install(&client).expect("install connection");
        let frame_sender = FrameSender::new(current);
        let (encoding_tx, encoding_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let first = {
            let frame_sender = frame_sender.clone();
            std::thread::spawn(move || {
                frame_sender.send_with(|frame_id| {
                    encoding_tx.send(()).expect("announce first encoder");
                    release_rx.recv().expect("release first encoder");
                    Ok(vec![frame_id as u8; 4])
                })
            })
        };
        encoding_rx.recv().expect("first encoder started");
        let second = {
            let frame_sender = frame_sender.clone();
            std::thread::spawn(move || {
                frame_sender.send_with(|frame_id| Ok(vec![frame_id as u8; 4]))
            })
        };
        release_tx.send(()).expect("release first encoder");

        let mut received = [0; 8];
        peer.read_exact(&mut received).expect("read both frames");
        assert_eq!(received, [1, 1, 1, 1, 2, 2, 2, 2]);
        let first_sent = first
            .join()
            .expect("join first")
            .expect("send first")
            .expect("connected first sender");
        let second_sent = second
            .join()
            .expect("join second")
            .expect("send second")
            .expect("connected second sender");
        assert_eq!(
            (
                first_sent.frame_id,
                first_sent.encoded_len,
                first_sent.application_viewport
            ),
            (1, 4, None)
        );
        assert_eq!(
            (
                second_sent.frame_id,
                second_sent.encoded_len,
                second_sent.application_viewport
            ),
            (2, 4, None)
        );
    }

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
