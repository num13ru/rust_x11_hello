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

use paper_protocol::{
    DISCOVERY_PORT, Gray8Frame, Mono1Frame, PixelFormat, V2PixelFormats, V2PointerPhase,
    encode_v2_frame,
};
use paperspoon::{
    application_input::ApplicationInput, application_renderer::render_application_bounded,
    application_ui::ApplicationUi,
};

mod diagnostic;
mod discovery;
mod image_frame;
mod inbound;
mod server;

use diagnostic::{DiagnosticFrame, StdinCommand, parse_stdin_command};
use image_frame::decode_jpeg_path_fit_contain;
use inbound::{SessionMessage, read_session_hello, read_session_message};
use server::{CurrentConnection, FrameForward, FrameTarget, FrameTargetError};

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
    last_application_frame: Option<Mono1Frame>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AuthoritativeFrame {
    Application { viewport: (u16, u16) },
    Diagnostic(DiagnosticFrame),
    Jpeg(Arc<Gray8Frame>),
}

impl AuthoritativeFrame {
    fn dimensions(&self) -> (u16, u16) {
        match self {
            Self::Application { viewport } => *viewport,
            Self::Diagnostic(frame) => frame.dimensions(),
            Self::Jpeg(frame) => (frame.width(), frame.height()),
        }
    }

    fn application_viewport(&self) -> Option<(u16, u16)> {
        match self {
            Self::Application { viewport } => Some(*viewport),
            Self::Diagnostic(_) | Self::Jpeg(_) => None,
        }
    }

    fn pixel_format(&self) -> PixelFormat {
        match self {
            Self::Application { .. } => PixelFormat::Mono1,
            Self::Diagnostic(frame) => frame.pixel_format(),
            Self::Jpeg(_) => PixelFormat::Gray8,
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
struct SentJpeg {
    sent: SentFrame,
    viewport: (u16, u16),
}

#[derive(Debug)]
enum ApplicationSend {
    Sent(SentFrame),
    Unchanged,
    NoConnection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
                last_application_frame: None,
            })),
        }
    }

    #[cfg(test)]
    fn send_with(
        &self,
        encode: impl FnOnce(u64) -> Result<Vec<u8>, String>,
    ) -> Result<Option<SentFrame>, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        self.send_locked(&mut state, PixelFormat::Mono1, None, encode, None)
    }

    fn send_locked(
        &self,
        state: &mut FrameSenderState,
        pixel_format: PixelFormat,
        target: Option<&FrameTarget>,
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
        let forwarded = match target {
            Some(target) => self
                .current
                .forward_frame_to(&encoded, pixel_format, target),
            None => self.current.forward_frame(&encoded, pixel_format),
        }
        .map_err(|error| format!("failed write frame: {error}"))?;
        let socket_write_elapsed = socket_write_started.elapsed();
        match forwarded {
            FrameForward::Sent => {}
            FrameForward::NoConnection => return Ok(None),
            FrameForward::UnsupportedPixelFormat => {
                return Err(format!(
                    "connected PaperPad does not support {pixel_format:?} frames"
                ));
            }
            FrameForward::ConnectionChanged => {
                return Err("PaperPad connection changed while preparing frame".to_string());
            }
            FrameForward::ViewportChanged => {
                return Err("PaperPad viewport changed while preparing frame".to_string());
            }
        }
        let application_viewport = authoritative
            .as_ref()
            .and_then(AuthoritativeFrame::application_viewport);
        state.next_frame_id = next_frame_id;
        if let Some(frame) = authoritative {
            let clears_application = !matches!(&frame, AuthoritativeFrame::Application { .. });
            state.authoritative = Some(frame);
            if clears_application {
                state.last_application_frame = None;
            }
        }
        Ok(Some(SentFrame {
            frame_id,
            encoded_len,
            application_viewport,
            encode_elapsed,
            socket_write_elapsed,
        }))
    }

    fn send_diagnostic(&self, frame: DiagnosticFrame) -> Result<Option<SentFrame>, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        self.send_locked(
            &mut state,
            frame.pixel_format(),
            None,
            |frame_id| frame.encode(frame_id),
            Some(AuthoritativeFrame::Diagnostic(frame)),
        )
    }

    fn send_jpeg(&self, path: &std::path::Path) -> Result<Option<SentJpeg>, String> {
        let target = match self.current.frame_target(PixelFormat::Gray8) {
            Ok(target) => target,
            Err(FrameTargetError::NoConnection) => return Ok(None),
            Err(FrameTargetError::UnsupportedPixelFormat) => {
                return Err("connected PaperPad does not support Gray8 frames".to_string());
            }
            Err(FrameTargetError::ViewportUnavailable) => {
                return Err("connected PaperPad has no active viewport".to_string());
            }
        };
        let viewport = target.viewport();
        let frame = Arc::new(decode_jpeg_path_fit_contain(path, viewport.0, viewport.1)?);
        self.send_jpeg_frame(frame, target)
    }

    fn send_jpeg_frame(
        &self,
        frame: Arc<Gray8Frame>,
        target: FrameTarget,
    ) -> Result<Option<SentJpeg>, String> {
        let viewport = target.viewport();
        let encoded_frame = Arc::clone(&frame);
        let mut state = self.state.lock().expect("frame sender lock");
        let sent = self.send_locked(
            &mut state,
            PixelFormat::Gray8,
            Some(&target),
            move |frame_id| {
                encode_v2_frame(frame_id, encoded_frame.as_ref())
                    .map_err(|error| format!("failed to encode JPEG frame: {error}"))
            },
            Some(AuthoritativeFrame::Jpeg(frame)),
        )?;
        Ok(sent.map(|sent| SentJpeg { sent, viewport }))
    }

    fn send_application(
        &self,
        ui: &ApplicationUi,
        viewport: (u16, u16),
    ) -> Result<ApplicationSend, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        self.send_application_locked(&mut state, ui, viewport, true)
    }

    fn send_application_locked(
        &self,
        state: &mut FrameSenderState,
        ui: &ApplicationUi,
        viewport: (u16, u16),
        suppress_unchanged: bool,
    ) -> Result<ApplicationSend, String> {
        let candidate = render_application_bounded(ui, viewport.0, viewport.1)?;
        if !self.current.is_active() {
            return Ok(ApplicationSend::NoConnection);
        }
        if suppress_unchanged
            && matches!(state.authoritative.as_ref(), Some(AuthoritativeFrame::Application { viewport: current }) if *current == viewport)
            && state.last_application_frame.as_ref() == Some(&candidate)
        {
            return Ok(ApplicationSend::Unchanged);
        }
        let sent = self.send_locked(
            state,
            PixelFormat::Mono1,
            None,
            |frame_id| {
                encode_v2_frame(frame_id, &candidate)
                    .map_err(|error| format!("failed to encode application frame: {error}"))
            },
            Some(AuthoritativeFrame::Application { viewport }),
        )?;
        match sent {
            Some(sent) => {
                state.last_application_frame = Some(candidate);
                Ok(ApplicationSend::Sent(sent))
            }
            None => Ok(ApplicationSend::NoConnection),
        }
    }

    fn send_application_for_viewport_change(
        &self,
        ui: &ApplicationUi,
        viewport: (u16, u16),
    ) -> Result<Option<SentFrame>, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        state.last_application_frame = None;
        match self.send_application_locked(&mut state, ui, viewport, false)? {
            ApplicationSend::Sent(sent) => Ok(Some(sent)),
            ApplicationSend::NoConnection => Ok(None),
            ApplicationSend::Unchanged => {
                Err("viewport replacement was unexpectedly suppressed".to_string())
            }
        }
    }

    fn send_for_hello(
        &self,
        ui: &ApplicationUi,
        viewport: (u16, u16),
        pixel_formats: V2PixelFormats,
    ) -> Result<Option<HelloFrame>, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        let retained = state
            .authoritative
            .as_ref()
            .filter(|frame| {
                frame.dimensions() == viewport && pixel_formats.supports(frame.pixel_format())
            })
            .cloned();
        let was_retained = retained.is_some();
        let frame = retained.unwrap_or(AuthoritativeFrame::Application { viewport });
        let sent = match &frame {
            AuthoritativeFrame::Application { viewport } => {
                match self.send_application_locked(&mut state, ui, *viewport, false)? {
                    ApplicationSend::Sent(sent) => Some(sent),
                    ApplicationSend::NoConnection => None,
                    ApplicationSend::Unchanged => {
                        return Err("Hello replacement was unexpectedly suppressed".to_string());
                    }
                }
            }
            AuthoritativeFrame::Diagnostic(diagnostic) => self.send_locked(
                &mut state,
                diagnostic.pixel_format(),
                None,
                |frame_id| diagnostic.encode(frame_id),
                Some(frame.clone()),
            )?,
            AuthoritativeFrame::Jpeg(jpeg) => {
                let encoded_frame = Arc::clone(jpeg);
                self.send_locked(
                    &mut state,
                    PixelFormat::Gray8,
                    None,
                    move |frame_id| {
                        encode_v2_frame(frame_id, encoded_frame.as_ref())
                            .map_err(|error| format!("failed to encode JPEG frame: {error}"))
                    },
                    Some(frame.clone()),
                )?
            }
        };
        Ok(sent.map(|sent| HelloFrame {
            sent,
            retained: was_retained,
            frame,
        }))
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
    let sent = frame_sender.send_application_for_viewport_change(ui, viewport)?;
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
    pixel_formats: V2PixelFormats,
) -> Result<Option<HelloFrame>, String> {
    *rendered_viewport = None;
    input.set_viewport(None);
    let sent = frame_sender.send_for_hello(ui, viewport, pixel_formats)?;
    if let Some(ref hello_frame) = sent {
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
        "type 'frame <white|black|horizontal|checkerboard|border|corners|gray-gradient|gray-bars> <width>x<height>', 'frame gray <0..255> <width>x<height>', or 'frame jpeg <path>' to send a diagnostic framebuffer"
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
                        match frame_sender.send_diagnostic(frame) {
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
                    Ok(StdinCommand::Jpeg(path)) => match frame_sender.send_jpeg(&path) {
                        Ok(Some(jpeg)) => {
                            if application_viewport_tx.send(None).is_err() {
                                eprintln!("application viewport tracker stopped");
                            }
                            let sent = jpeg.sent;
                            let record = format!(
                                "sent JPEG frame id={} path={} width={} height={} bytes={} encode_us={} socket_write_us={} source=stdin",
                                sent.frame_id,
                                path.display(),
                                jpeg.viewport.0,
                                jpeg.viewport.1,
                                sent.encoded_len,
                                sent.encode_elapsed.as_micros(),
                                sent.socket_write_elapsed.as_micros()
                            );
                            println!("{record}");
                            flush_stdout("after sent JPEG frame");
                            append_host_log_record(&log_path, &record);
                        }
                        Ok(None) => eprintln!("JPEG frame not sent: no PaperPad connected"),
                        Err(error) => eprintln!("JPEG frame command error: {error}"),
                    },
                    Ok(StdinCommand::ApplicationFrame { width, height }) => {
                        match frame_sender.send_application(&application_ui, (width, height)) {
                            Ok(ApplicationSend::Sent(sent)) => {
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
                            Ok(ApplicationSend::Unchanged) => {
                                let record = format!(
                                    "application frame skipped unchanged width={width} height={height} source=stdin"
                                );
                                println!("{record}");
                                flush_stdout("after skipped application frame");
                                append_host_log_record(&log_path, &record);
                            }
                            Ok(ApplicationSend::NoConnection) => {
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
        let connection_token = match current.install_for_hello(
            reader.get_ref(),
            hello.pixel_formats(),
            (hello.viewport_width(), hello.viewport_height()),
        ) {
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
            hello.pixel_formats(),
        ) {
            Ok(Some(hello_frame)) => {
                let sent = hello_frame.sent;
                let (kind, pattern) = match hello_frame.frame {
                    AuthoritativeFrame::Application { .. } => ("application", String::new()),
                    AuthoritativeFrame::Diagnostic(frame) => {
                        ("diagnostic", format!(" pattern={}", frame.pattern_name()))
                    }
                    AuthoritativeFrame::Jpeg(_) => ("jpeg", String::new()),
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
                    current.update_viewport_if_current(&connection_token, reported_viewport);
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
    use paper_protocol::{Frame, V2DecodeResult, V2Payload, decode_v2_message, decode_v2_payload};
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

    fn install_hello_connection(
        listener: &TcpListener,
        current: &CurrentConnection,
        pixel_formats: V2PixelFormats,
        viewport: (u16, u16),
    ) -> (server::ConnectionToken, TcpStream) {
        let client = TcpStream::connect(listener.local_addr().expect("listener address"))
            .expect("connect client");
        let (peer, _) = listener.accept().expect("accept client");
        let token = current
            .install_for_hello(&client, pixel_formats, viewport)
            .expect("install Hello connection");
        (token, peer)
    }

    fn read_sent_frame(peer: &mut TcpStream, sent: SentFrame) -> (u64, (u16, u16), Vec<u8>) {
        let (frame_id, frame) = read_sent_owned_frame(peer, sent);
        (
            frame_id,
            (frame.width(), frame.height()),
            frame.pixels().to_vec(),
        )
    }

    fn read_sent_owned_frame(peer: &mut TcpStream, sent: SentFrame) -> (u64, Frame) {
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
            frame.to_owned_frame().expect("own sent frame"),
        )
    }

    fn diagnostic(command: &str) -> DiagnosticFrame {
        let StdinCommand::Frame(frame) = parse_stdin_command(command).expect("diagnostic command")
        else {
            panic!("diagnostic frame expected");
        };
        frame
    }

    fn jpeg_fixture_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/grayscale-example.jpg")
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
    fn gray8_diagnostic_requires_capability_without_consuming_frame_state() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let client = TcpStream::connect(listener.local_addr().expect("listener address"))
            .expect("connect client");
        let (mut peer, _) = listener.accept().expect("accept client");
        let current = CurrentConnection::default();
        current.install(&client).expect("install Mono1 connection");
        let frame_sender = FrameSender::new(current.clone());
        let gray8 = diagnostic("frame gray 128 4x2");

        let error = frame_sender
            .send_diagnostic(gray8)
            .expect_err("Mono1-only peer rejects Gray8");
        assert!(error.contains("does not support Gray8"));
        let state = frame_sender.state.lock().expect("frame sender lock");
        assert_eq!(state.next_frame_id, 1);
        assert_eq!(state.authoritative, None);
        drop(state);

        current
            .install_with_pixel_formats(&client, V2PixelFormats::MONO1.with(PixelFormat::Gray8))
            .expect("install Gray8-capable connection");
        let sent = frame_sender
            .send_diagnostic(gray8)
            .expect("send Gray8")
            .expect("active connection");
        let mut encoded = vec![0; sent.encoded_len];
        peer.read_exact(&mut encoded).expect("read Gray8 frame");
        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(&encoded).expect("decode Gray8 frame")
        else {
            panic!("complete Gray8 frame expected");
        };
        assert_eq!(consumed, encoded.len());
        let V2Payload::Frame(frame) = decode_v2_payload(message).expect("typed Gray8 frame") else {
            panic!("Gray8 frame payload expected");
        };
        assert_eq!(frame.frame_id(), 1);
        assert_eq!(frame.pixel_format(), PixelFormat::Gray8);
        assert_eq!(frame.pixels(), &[128; 8]);
    }

    #[test]
    fn jpeg_uses_active_viewport_and_resends_retained_pixels() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let formats = V2PixelFormats::MONO1.with(PixelFormat::Gray8);
        let (first_token, mut first_peer) =
            install_hello_connection(&listener, &current, formats, (12, 16));

        let first_sent = frame_sender
            .send_jpeg(&jpeg_fixture_path())
            .expect("send fixture JPEG")
            .expect("active connection");
        assert_eq!(first_sent.viewport, (12, 16));
        let (first_id, first_frame) = read_sent_owned_frame(&mut first_peer, first_sent.sent);
        let Frame::Gray8(first_frame) = first_frame else {
            panic!("Gray8 JPEG frame expected");
        };
        assert_eq!(first_id, 1);
        assert_eq!((first_frame.width(), first_frame.height()), (12, 16));
        assert!(
            first_frame.pixels()[..4 * 12]
                .iter()
                .all(|&pixel| pixel == u8::MAX)
        );
        assert!(
            first_frame.pixels()[4 * 12..12 * 12]
                .iter()
                .any(|&pixel| pixel < 64)
        );
        assert!(
            first_frame.pixels()[12 * 12..]
                .iter()
                .all(|&pixel| pixel == u8::MAX)
        );

        assert!(current.clear_if_current(&first_token));
        let (_second_token, mut second_peer) =
            install_hello_connection(&listener, &current, formats, (12, 16));
        let mut input = ApplicationInput::default();
        let mut rendered_viewport = Some((12, 16));
        let resent = replace_hello_frame(
            &frame_sender,
            &ApplicationUi::default(),
            &mut input,
            &mut rendered_viewport,
            (12, 16),
            formats,
        )
        .expect("resend retained JPEG")
        .expect("replacement connection");
        let (second_id, second_frame) = read_sent_owned_frame(&mut second_peer, resent.sent);
        let Frame::Gray8(second_frame) = second_frame else {
            panic!("retained Gray8 JPEG frame expected");
        };

        assert!(resent.retained);
        assert!(matches!(resent.frame, AuthoritativeFrame::Jpeg(_)));
        assert_eq!(second_id, 2);
        assert_eq!(second_frame, first_frame);
        assert!(!input.is_active());
        assert_eq!(rendered_viewport, None);
    }

    #[test]
    fn jpeg_input_failures_leave_frame_state_unchanged() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let formats = V2PixelFormats::MONO1.with(PixelFormat::Gray8);
        let (_token, mut peer) = install_hello_connection(&listener, &current, formats, (12, 16));
        let sent = frame_sender
            .send_diagnostic(diagnostic("frame gray 96 12x16"))
            .expect("send baseline")
            .expect("active connection");
        read_sent_frame(&mut peer, sent);
        let baseline = {
            let state = frame_sender.state.lock().expect("frame sender lock");
            (
                state.next_frame_id,
                state.authoritative.clone(),
                state.last_application_frame.clone(),
            )
        };

        let missing = jpeg_fixture_path().with_file_name("missing-image.jpg");
        assert!(frame_sender.send_jpeg(&missing).is_err());
        let malformed = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(frame_sender.send_jpeg(&malformed).is_err());

        let state = frame_sender.state.lock().expect("frame sender lock");
        assert_eq!(state.next_frame_id, baseline.0);
        assert_eq!(state.authoritative, baseline.1);
        assert_eq!(state.last_application_frame, baseline.2);
    }

    #[test]
    fn jpeg_checks_connection_and_capability_before_reading_path() {
        let missing = jpeg_fixture_path().with_file_name("missing-image.jpg");
        let disconnected = FrameSender::new(CurrentConnection::default());
        assert_eq!(disconnected.send_jpeg(&missing), Ok(None));
        let disconnected_state = disconnected.state.lock().expect("frame sender lock");
        assert_eq!(disconnected_state.next_frame_id, 1);
        assert_eq!(disconnected_state.authoritative, None);
        drop(disconnected_state);

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let (_token, _peer) =
            install_hello_connection(&listener, &current, V2PixelFormats::MONO1, (12, 16));
        let error = frame_sender
            .send_jpeg(&missing)
            .expect_err("Mono1-only peer must reject before opening path");
        assert!(error.contains("does not support Gray8"));
        let state = frame_sender.state.lock().expect("frame sender lock");
        assert_eq!(state.next_frame_id, 1);
        assert_eq!(state.authoritative, None);
    }

    #[test]
    fn jpeg_target_races_leave_frame_state_unchanged() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let formats = V2PixelFormats::MONO1.with(PixelFormat::Gray8);
        let (token, _first_peer) = install_hello_connection(&listener, &current, formats, (5, 7));
        let stale_viewport = current
            .frame_target(PixelFormat::Gray8)
            .expect("initial frame target");
        assert!(current.update_viewport_if_current(&token, (6, 8)));
        let viewport_error = frame_sender
            .send_jpeg_frame(
                Arc::new(Gray8Frame::new(5, 7, vec![0; 35]).expect("Gray8 frame")),
                stale_viewport,
            )
            .expect_err("stale viewport must reject frame");
        assert!(viewport_error.contains("viewport changed"));

        let stale_connection = current
            .frame_target(PixelFormat::Gray8)
            .expect("updated frame target");
        let (_replacement_token, _replacement_peer) =
            install_hello_connection(&listener, &current, formats, (6, 8));
        let connection_error = frame_sender
            .send_jpeg_frame(
                Arc::new(Gray8Frame::new(6, 8, vec![0; 48]).expect("Gray8 frame")),
                stale_connection,
            )
            .expect_err("replaced connection must reject frame");
        assert!(connection_error.contains("connection changed"));

        let state = frame_sender.state.lock().expect("frame sender lock");
        assert_eq!(state.next_frame_id, 1);
        assert_eq!(state.authoritative, None);
        assert_eq!(state.last_application_frame, None);
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
            .send_diagnostic(border)
            .expect("send border")
            .expect("active connection");
        let first = read_sent_frame(&mut first_peer, first_sent);
        assert_eq!(first.0, 1);
        assert_eq!(first.1, (9, 9));
        assert_eq!(first_sent.application_viewport, None);

        assert!(current.clear_if_current(&token));
        assert_eq!(
            frame_sender
                .send_diagnostic(diagnostic("frame checkerboard 9x9"))
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
            .send_diagnostic(diagnostic("frame checkerboard 9x9"))
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
            V2PixelFormats::MONO1,
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
            .expect("send application");
        let ApplicationSend::Sent(first_sent) = first_sent else {
            panic!("expected application frame on active connection");
        };
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
            V2PixelFormats::MONO1,
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
            .send_diagnostic(diagnostic("frame border 9x9"))
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
            V2PixelFormats::MONO1,
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
    fn unchanged_application_pixels_skip_without_consuming_an_id() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let (_token, mut peer) = install_connection(&listener, &current);
        let mut ui = ApplicationUi::default();

        let ApplicationSend::Sent(first) = frame_sender
            .send_application(&ui, (128, 128))
            .expect("first application render")
        else {
            panic!("first application frame must be sent");
        };
        let first_pixels = read_sent_frame(&mut peer, first).2;
        peer.set_nonblocking(true).expect("nonblocking peer");
        assert!(matches!(
            frame_sender.send_application(&ui, (128, 128)),
            Ok(ApplicationSend::Unchanged)
        ));
        assert_eq!(
            peer.read(&mut [0; 1])
                .expect_err("no duplicate frame")
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            frame_sender
                .state
                .lock()
                .expect("frame sender lock")
                .next_frame_id,
            2
        );
        peer.set_nonblocking(false).expect("blocking peer");

        ui.set_status("ready");
        let ApplicationSend::Sent(changed) = frame_sender
            .send_application(&ui, (128, 128))
            .expect("changed application render")
        else {
            panic!("changed application frame must be sent");
        };
        assert_eq!(changed.frame_id, 2);
        assert_ne!(read_sent_frame(&mut peer, changed).2, first_pixels);
    }

    #[test]
    fn viewport_change_and_diagnostic_switch_force_application_frames() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let (_token, mut peer) = install_connection(&listener, &current);
        let ui = ApplicationUi::default();

        let ApplicationSend::Sent(first) = frame_sender
            .send_application(&ui, (128, 128))
            .expect("first application frame")
        else {
            panic!("first application frame must be sent");
        };
        read_sent_frame(&mut peer, first);

        let resized = frame_sender
            .send_application_for_viewport_change(&ui, (129, 128))
            .expect("viewport replacement")
            .expect("active connection");
        assert_eq!(read_sent_frame(&mut peer, resized).1, (129, 128));

        let restored = frame_sender
            .send_application_for_viewport_change(&ui, (128, 128))
            .expect("restored viewport")
            .expect("active connection");
        assert_eq!(read_sent_frame(&mut peer, restored).1, (128, 128));

        let diagnostic = frame_sender
            .send_diagnostic(diagnostic("frame white 128x128"))
            .expect("diagnostic frame")
            .expect("active connection");
        read_sent_frame(&mut peer, diagnostic);
        let ApplicationSend::Sent(application) = frame_sender
            .send_application(&ui, (128, 128))
            .expect("return to application")
        else {
            panic!("application after diagnostic must be sent");
        };
        assert_eq!(application.frame_id, 5);
        read_sent_frame(&mut peer, application);
    }

    #[test]
    fn failed_application_send_does_not_advance_baseline_and_hello_resends() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let (token, mut first_peer) = install_connection(&listener, &current);
        let mut ui = ApplicationUi::default();
        let ApplicationSend::Sent(first) = frame_sender
            .send_application(&ui, (128, 128))
            .expect("first application frame")
        else {
            panic!("first application frame must be sent");
        };
        let first_pixels = read_sent_frame(&mut first_peer, first).2;

        ui.set_status("changed");
        assert!(current.clear_if_current(&token));
        drop(first_peer);
        let broken_client = TcpStream::connect(listener.local_addr().expect("listener address"))
            .expect("connect broken client");
        let (broken_peer, _) = listener.accept().expect("accept broken client");
        let broken_token = current
            .install(&broken_client)
            .expect("install broken client");
        broken_client
            .shutdown(Shutdown::Both)
            .expect("shut down broken client");
        drop(broken_peer);
        frame_sender
            .send_application(&ui, (128, 128))
            .expect_err("failed socket write");
        let state = frame_sender.state.lock().expect("frame sender lock");
        assert_eq!(state.next_frame_id, 2);
        assert_eq!(
            state
                .last_application_frame
                .as_ref()
                .expect("last successfully sent application frame")
                .pixels(),
            first_pixels
        );
        drop(state);
        assert!(!current.clear_if_current(&broken_token));
        assert!(matches!(
            frame_sender.send_application(&ui, (128, 128)),
            Ok(ApplicationSend::NoConnection)
        ));

        let (_token, mut second_peer) = install_connection(&listener, &current);
        let mut input = ApplicationInput::default();
        let mut rendered_viewport = None;
        let resent = replace_hello_frame(
            &frame_sender,
            &ui,
            &mut input,
            &mut rendered_viewport,
            (128, 128),
            V2PixelFormats::MONO1,
        )
        .expect("Hello replacement")
        .expect("active connection");
        assert_eq!(resent.sent.frame_id, 2);
        assert!(input.is_active());
        read_sent_frame(&mut second_peer, resent.sent);
        assert!(matches!(
            frame_sender.send_application(&ui, (128, 128)),
            Ok(ApplicationSend::Unchanged)
        ));
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
