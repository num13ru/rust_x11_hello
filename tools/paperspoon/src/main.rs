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
use paper_protocol::{compression::ZstdEncoder, v3};
use paperspoon::{
    application_input::ApplicationInput, application_renderer::ApplicationRenderer,
    application_ui::ApplicationUi, now_playing::load_now_playing_frame,
};

mod diagnostic;
mod discovery;
mod image_frame;
mod inbound;
mod latency;
mod live_now_playing;
mod server;

use diagnostic::{DiagnosticFrame, LiveCommand, StdinCommand, parse_stdin_command};
use image_frame::decode_jpeg_path_fit_contain;
use inbound::{
    SessionMessage, read_negotiated_hello, read_session_message, read_v3_session_message,
};
use live_now_playing::{LiveNowPlaying, ProviderKind};
use server::{ConnectionToken, CurrentConnection, FrameForward, FrameTarget, FrameTargetError};

/// Default TCP port. Must match `rust_x11_hello`'s `COMPANION_PORT`.
const DEFAULT_PORT: u16 = paper_protocol::DEFAULT_TCP_PORT;
/// Default log file name for received activation lines.
const DEFAULT_LOG_FILE: &str = "paperspoon.log";
const DEFAULT_LATENCY_LOG_FILE: &str = "paperspoon.latency.jsonl";
/// Whether to forward actions to Hammerspoon via `open -g hammerspoon://`.
const FORWARD_TO_HAMMERSPOON: bool = true;

/// Parsed command line.
struct Options {
    port: u16,
    log_path: String,
    latency_log_path: String,
    /// None means URL forwarding is disabled.
    forward_url: bool,
    live_provider: Option<ProviderKind>,
    progress_interval: Duration,
    compression: HostCompression,
    compression_level: i32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum HostCompression {
    Raw,
    #[default]
    Auto,
    Zstd,
}

impl HostCompression {
    fn negotiate(
        self,
        offered: Option<v3::Encodings>,
    ) -> Result<Option<v3::Encodings>, &'static str> {
        let supported = if self == Self::Raw {
            v3::Encodings::RAW
        } else {
            v3::Encodings::RAW_ZSTD
        };
        let agreed = offered.map(|offered| offered.intersection(supported));
        if self == Self::Zstd && !agreed.is_some_and(|agreed| agreed.supports(v3::Encoding::Zstd)) {
            return Err(
                "Zstandard is required. Enable PPFB v3 Zstandard on the client or select --compression=auto",
            );
        }
        Ok(agreed)
    }
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut port = DEFAULT_PORT;
    let mut log_path = DEFAULT_LOG_FILE.to_string();
    let mut latency_log_path = DEFAULT_LATENCY_LOG_FILE.to_string();
    let mut forward_url = FORWARD_TO_HAMMERSPOON;
    let mut live_provider = None;
    let mut progress_interval = Duration::from_secs(1);
    let mut compression = HostCompression::default();
    let mut compression_level = 1;
    let positional = args.iter().skip(1);
    for arg in positional {
        match arg.as_str() {
            "--compression=raw" => compression = HostCompression::Raw,
            "--compression=auto" => compression = HostCompression::Auto,
            "--compression=zstd" => compression = HostCompression::Zstd,
            value if value.starts_with("--compression=") => {
                return Err("--compression requires raw, auto, or zstd".into());
            }
            value if value.starts_with("--compression-level=") => {
                compression_level = value["--compression-level=".len()..]
                    .parse()
                    .ok()
                    .filter(|level| matches!(level, 1 | 3 | 6))
                    .ok_or("--compression-level requires 1, 3, or 6")?;
            }
            "--no-forward-url" => {
                forward_url = false;
            }
            "--now-playing-fake" | "--now-playing-apple-music" => {
                let provider = if arg == "--now-playing-fake" {
                    ProviderKind::Fake
                } else {
                    ProviderKind::AppleMusic
                };
                if live_provider.is_some_and(|selected| selected != provider) {
                    return Err("choose one Now Playing provider".into());
                }
                live_provider = Some(provider);
            }
            value if value.starts_with("--now-playing-progress-ms=") => {
                let interval = value["--now-playing-progress-ms=".len()..]
                    .parse::<u64>()
                    .ok()
                    .filter(|interval| matches!(interval, 100 | 250 | 500 | 1000 | 2000))
                    .ok_or("--now-playing-progress-ms expects 100, 250, 500, 1000, or 2000")?;
                progress_interval = Duration::from_millis(interval);
            }
            value if value.starts_with("--latency-log=") => {
                if let Some(path) = value.strip_prefix("--latency-log=")
                    && !path.is_empty()
                {
                    latency_log_path = path.to_string();
                }
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
    Ok(Options {
        port,
        log_path,
        latency_log_path,
        forward_url,
        live_provider,
        progress_interval,
        compression,
        compression_level,
    })
}

/// Forward one action line to Hammerspoon via `open -g hammerspoon://`.
/// The action id is passed as a query parameter (`?action=<id>`); Hammerspoon's
/// urlevent handler receives it in the params table. No sockets, no ports,
/// no file polling.
fn forward_url(action_id: &str, trace: Option<paper_protocol::V2PointerTrace>) -> io::Result<()> {
    let trace_query = trace
        .map(|trace| {
            format!(
                "&session={}&trace={}&origin_mono_us={}",
                trace.session_id(),
                trace.trace_id(),
                trace.origin_mono_us()
            )
        })
        .unwrap_or_default();
    std::process::Command::new("open")
        .arg("-g")
        .arg(format!(
            "hammerspoon://paperpad?action={action_id}{trace_query}"
        ))
        .status()?;
    Ok(())
}

fn flush_stdout(context: &str) {
    if let Err(error) = io::stdout().flush() {
        eprintln!("stdout flush error {context}: {error}");
    }
}

fn write_log_record(file: &mut impl Write, origin: &str, record: &str) -> io::Result<()> {
    let timestamp_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros())
        .unwrap_or(0);
    let line = format!(
        "{}.{:06} {origin} {record}\n",
        timestamp_us / 1_000_000,
        timestamp_us % 1_000_000
    );
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
    live_active: bool,
    authoritative: Option<AuthoritativeFrame>,
    last_application_frame: Option<Mono1Frame>,
    application_renderer: Option<ApplicationRenderer>,
    encoding_session: Option<ConnectionToken>,
    encoder: Option<ZstdEncoder>,
    compression_level: i32,
    compression_scratch: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AuthoritativeFrame {
    Application { viewport: (u16, u16) },
    Diagnostic(DiagnosticFrame),
    Jpeg(Arc<Gray8Frame>),
    NowPlaying(Arc<Gray8Frame>),
}

impl AuthoritativeFrame {
    fn dimensions(&self) -> (u16, u16) {
        match self {
            Self::Application { viewport } => *viewport,
            Self::Diagnostic(frame) => frame.dimensions(),
            Self::Jpeg(frame) | Self::NowPlaying(frame) => (frame.width(), frame.height()),
        }
    }

    fn application_viewport(&self) -> Option<(u16, u16)> {
        match self {
            Self::Application { viewport } => Some(*viewport),
            Self::Diagnostic(_) | Self::Jpeg(_) | Self::NowPlaying(_) => None,
        }
    }

    fn pixel_format(&self) -> PixelFormat {
        match self {
            Self::Application { .. } => PixelFormat::Mono1,
            Self::Diagnostic(frame) => frame.pixel_format(),
            Self::Jpeg(_) | Self::NowPlaying(_) => PixelFormat::Gray8,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SentNowPlaying {
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
    #[cfg(test)]
    fn new(current: CurrentConnection) -> Self {
        Self::with_compression_level(current, 1)
    }

    fn with_compression_level(current: CurrentConnection, compression_level: i32) -> Self {
        Self {
            current,
            state: Arc::new(Mutex::new(FrameSenderState {
                next_frame_id: 1,
                live_active: false,
                authoritative: None,
                last_application_frame: None,
                application_renderer: None,
                encoding_session: None,
                encoder: None,
                compression_level,
                compression_scratch: Vec::new(),
            })),
        }
    }

    fn set_live_active(&self, active: bool) {
        self.state.lock().expect("frame sender lock").live_active = active;
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
        let transport = self.current.transport();
        let encode_started = Instant::now();
        let mut encoded = encode(frame_id)?;
        let Some((connection, encodings)) = transport else {
            return Ok(None);
        };
        if !state
            .encoding_session
            .as_ref()
            .is_some_and(|current| current.matches(&connection))
        {
            state.encoding_session = Some(connection.clone());
            state.encoder = None;
            state.compression_scratch.clear();
            if encodings.is_some_and(|encodings| encodings.supports(v3::Encoding::Zstd)) {
                match ZstdEncoder::new(state.compression_level) {
                    Ok(encoder) => state.encoder = Some(encoder),
                    Err(error) => eprintln!(
                        "PPFB v3 Zstandard unavailable for this connection, using Raw: {error}"
                    ),
                }
            }
        }
        if encodings.is_some() {
            encoded[4] = v3::VERSION;
            if let Some(encoder) = &mut state.encoder {
                let pixels_start = paper_protocol::V2_HEADER_LEN + v3::RAW_PREFIX_LEN;
                let raw_len = encoded.len() - pixels_start;
                match encoder.encode_if_smaller(
                    &encoded[pixels_start..],
                    v3::RAW_PREFIX_LEN,
                    v3::ZSTD_PREFIX_LEN,
                    &mut state.compression_scratch,
                ) {
                    Ok(Some(compressed)) => {
                        let payload_len = v3::ZSTD_PREFIX_LEN + compressed.len();
                        encoded[8..12].copy_from_slice(&(payload_len as u32).to_be_bytes());
                        encoded[paper_protocol::V2_HEADER_LEN + 13] = v3::Encoding::Zstd as u8;
                        encoded.truncate(pixels_start);
                        encoded.extend_from_slice(&(raw_len as u32).to_be_bytes());
                        encoded.extend_from_slice(compressed);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        eprintln!(
                            "PPFB v3 Zstandard compression failed, using Raw for this connection: {error}"
                        );
                        state.encoder = None;
                    }
                }
            }
        }
        let encode_elapsed = encode_started.elapsed();
        let encoded_len = encoded.len();
        // write_all measures local socket acceptance, not remote receipt or display.
        let socket_write_started = Instant::now();
        let forwarded = self
            .current
            .forward_negotiated(
                &encoded,
                pixel_format,
                target,
                &connection,
                authoritative.as_ref().map(AuthoritativeFrame::dimensions),
            )
            .map_err(|error| format!("failed to write frame: {error}"))?;
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

    fn send_now_playing(&self) -> Result<Option<SentNowPlaying>, String> {
        self.send_now_playing_with(load_now_playing_frame)
    }

    fn send_now_playing_with(
        &self,
        load: impl FnOnce(u16, u16) -> Result<Gray8Frame, String>,
    ) -> Result<Option<SentNowPlaying>, String> {
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
        let frame = Arc::new(load(viewport.0, viewport.1)?);
        self.send_now_playing_frame(frame, target)
    }

    fn send_now_playing_frame(
        &self,
        frame: Arc<Gray8Frame>,
        target: FrameTarget,
    ) -> Result<Option<SentNowPlaying>, String> {
        let viewport = target.viewport();
        let encoded_frame = Arc::clone(&frame);
        let mut state = self.state.lock().expect("frame sender lock");
        let sent = self.send_locked(
            &mut state,
            PixelFormat::Gray8,
            Some(&target),
            move |frame_id| {
                encode_v2_frame(frame_id, encoded_frame.as_ref())
                    .map_err(|error| format!("failed to encode Now Playing frame: {error}"))
            },
            Some(AuthoritativeFrame::NowPlaying(frame)),
        )?;
        Ok(sent.map(|sent| SentNowPlaying { sent, viewport }))
    }

    fn send_live_frame(
        &self,
        frame: &Gray8Frame,
        target: &FrameTarget,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<SentFrame>, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        if !state.live_active || cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(None);
        }
        let sent = self.send_locked(
            &mut state,
            PixelFormat::Gray8,
            Some(target),
            |id| encode_v2_frame(id, frame).map_err(|error| error.to_string()),
            None,
        )?;
        if sent.is_some() {
            state.authoritative = None;
            state.last_application_frame = None;
        }
        Ok(sent)
    }

    fn send_application(
        &self,
        ui: &ApplicationUi,
        viewport: (u16, u16),
    ) -> Result<ApplicationSend, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        if state.live_active && self.current.frame_target(PixelFormat::Gray8).is_ok() {
            return Ok(ApplicationSend::Unchanged);
        }
        self.send_application_locked(&mut state, ui, viewport, true)
    }

    fn send_application_locked(
        &self,
        state: &mut FrameSenderState,
        ui: &ApplicationUi,
        viewport: (u16, u16),
        suppress_unchanged: bool,
    ) -> Result<ApplicationSend, String> {
        if state.application_renderer.is_none() {
            state.application_renderer = Some(ApplicationRenderer::new()?);
        }
        let candidate = state
            .application_renderer
            .as_mut()
            .ok_or_else(|| "application renderer initialization was lost".to_string())?
            .render_bounded(ui, viewport.0, viewport.1)?;
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
        if state.live_active && self.current.frame_target(PixelFormat::Gray8).is_ok() {
            return Ok(None);
        }
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
        if state.live_active && pixel_formats.supports(PixelFormat::Gray8) {
            return Ok(None);
        }
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
            AuthoritativeFrame::Jpeg(gray8) | AuthoritativeFrame::NowPlaying(gray8) => {
                let encoded_frame = Arc::clone(gray8);
                self.send_locked(
                    &mut state,
                    PixelFormat::Gray8,
                    None,
                    move |frame_id| {
                        encode_v2_frame(frame_id, encoded_frame.as_ref()).map_err(|error| {
                            format!("failed to encode retained Gray8 frame: {error}")
                        })
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
    ui: &mut ApplicationUi,
    input: &mut ApplicationInput,
    rendered_viewport: &mut Option<(u16, u16)>,
    viewport: (u16, u16),
) -> Result<Option<SentFrame>, String> {
    *rendered_viewport = None;
    input.set_viewport(ui, None);
    let sent = frame_sender.send_application_for_viewport_change(ui, viewport)?;
    if sent.is_some() {
        *rendered_viewport = Some(viewport);
        input.set_viewport(ui, Some(viewport));
    }
    Ok(sent)
}

fn replace_hello_frame(
    frame_sender: &FrameSender,
    ui: &mut ApplicationUi,
    input: &mut ApplicationInput,
    rendered_viewport: &mut Option<(u16, u16)>,
    viewport: (u16, u16),
    pixel_formats: V2PixelFormats,
) -> Result<Option<HelloFrame>, String> {
    *rendered_viewport = None;
    input.set_viewport(ui, None);
    let sent = frame_sender.send_for_hello(ui, viewport, pixel_formats)?;
    if let Some(ref hello_frame) = sent {
        *rendered_viewport = hello_frame.sent.application_viewport;
        input.set_viewport(ui, hello_frame.sent.application_viewport);
    }
    Ok(sent)
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.get(1).map(String::as_str) == Some(paperspoon::now_playing::HELPER_ARGUMENT) {
        if args.len() != 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "internal Now Playing helper requires width and height",
            ));
        }
        let parse_dimension = |value: &str, name: &str| {
            value
                .parse::<u16>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("invalid internal Now Playing helper {name}"),
                    )
                })
        };
        let width = parse_dimension(&args[2], "width")?;
        let height = parse_dimension(&args[3], "height")?;
        paperspoon::now_playing::run_helper(width, height);
    }
    let opts =
        parse_options(&args).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    latency::init(&opts.latency_log_path);

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
        "type 'frame <white|black|horizontal|checkerboard|border|corners|gray-gradient|gray-bars> <width>x<height>', 'frame gray <0..255> <width>x<height>', 'frame jpeg <path>', or 'frame nowplaying' to send a framebuffer"
    );
    println!("type 'ui <width>x<height>' to send the host-rendered application UI");
    if opts.forward_url {
        println!("forwarding actions to Hammerspoon via open -g hammerspoon://paperpad/...");
    } else {
        println!("action forwarding to Hammerspoon disabled");
    }
    io::stdout().flush()?;

    let current = CurrentConnection::default();
    let frame_sender = FrameSender::with_compression_level(current.clone(), opts.compression_level);
    let live = Arc::new(LiveNowPlaying::new(
        frame_sender.clone(),
        opts.live_provider,
        opts.progress_interval,
        opts.log_path.clone(),
    ));
    #[cfg(target_os = "macos")]
    if opts.live_provider.is_some() {
        let mut signals = signal_hook::iterator::Signals::new([
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGTERM,
        ])?;
        let live = Arc::clone(&live);
        std::thread::Builder::new()
            .name("paperspoon-shutdown".to_string())
            .spawn(move || {
                if let Some(signal) = signals.forever().next() {
                    live.shutdown();
                    std::process::exit(128 + signal);
                }
            })?;
    }
    println!("type 'nowplaying <start|stop|status|refresh>' for live Now Playing");
    let (application_viewport_tx, application_viewport_rx) = mpsc::channel();

    let _stdin_worker = {
        let frame_sender = frame_sender.clone();
        let live = Arc::clone(&live);
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
                let command = parse_stdin_command(line);
                if matches!(command, Ok(StdinCommand::Frame(_) | StdinCommand::Jpeg(_) | StdinCommand::NowPlaying | StdinCommand::ApplicationFrame { .. })) {
                    live.stop();
                }
                match command {
                    Ok(StdinCommand::LiveNowPlaying(command)) => {
                        let result = match command {
                            LiveCommand::Start => live.start().map(|()| {
                                if live.connection_changed() {
                                    let _ = application_viewport_tx.send(None);
                                }
                                println!("{}", live.status());
                            }),
                            LiveCommand::Stop => {
                                if live.stop() && let Ok(target) = frame_sender.current.frame_target(PixelFormat::Mono1) {
                                    let viewport = target.viewport();
                                    match frame_sender.send_application(&application_ui, viewport) {
                                        Ok(ApplicationSend::Sent(_) | ApplicationSend::Unchanged) => {
                                            let _ = application_viewport_tx.send(Some(viewport));
                                        }
                                        Ok(ApplicationSend::NoConnection) => {}
                                        Err(error) => eprintln!("applying grid after Now Playing stop: {error}"),
                                    }
                                }
                                println!("{}", live.status());
                                Ok(())
                            }
                            LiveCommand::Status => {
                                println!("{}", live.status());
                                Ok(())
                            }
                            LiveCommand::Refresh => live.refresh(),
                            LiveCommand::Fake(change) => live.fake(change),
                        };
                        if let Err(error) = result {
                            eprintln!("Now Playing command: {error}");
                        }
                        flush_stdout("after live Now Playing command");
                    }
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
                    Ok(StdinCommand::NowPlaying) => {
                        match frame_sender.send_now_playing() {
                            Ok(Some(now_playing)) => {
                                if application_viewport_tx.send(None).is_err() {
                                    eprintln!("application viewport tracker stopped");
                                }
                                let sent = now_playing.sent;
                                let record = format!(
                                    "sent Now Playing frame id={} width={} height={} bytes={} encode_us={} socket_write_us={} source=stdin",
                                    sent.frame_id,
                                    now_playing.viewport.0,
                                    now_playing.viewport.1,
                                    sent.encoded_len,
                                    sent.encode_elapsed.as_micros(),
                                    sent.socket_write_elapsed.as_micros()
                                );
                                println!("{record}");
                                flush_stdout("after Now Playing frame");
                                append_host_log_record(&log_path, &record);
                            }
                            Ok(None) => {
                                eprintln!("Now Playing frame not sent: no PaperPad connected");
                            }
                            Err(error) => eprintln!("Now Playing frame error: {error}"),
                        }
                    }
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
                live.stop();
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
        let (hello, offered) = match read_negotiated_hello(&mut reader) {
            Ok(Some(hello)) => hello,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("TCP read error from {peer}: {error}");
                continue;
            }
        };
        let encodings = match opts.compression.negotiate(offered) {
            Ok(encodings) => encodings,
            Err(error) => {
                eprintln!("PPFB negotiation with {peer}: {error}");
                continue;
            }
        };
        if let Some(encodings) = encodings {
            let mut socket = &**reader.get_ref();
            if let Err(error) = socket.write_all(&v3::encode_ack(encodings)) {
                eprintln!("PPFB v3 HelloAck to {peer}: {error}");
                continue;
            }
        }
        let hello_record = format!(
            "hello protocol={} viewport={}x{} encodings={encodings:?} compression={:?}",
            if encodings.is_some() {
                v3::VERSION
            } else {
                hello.protocol_version()
            },
            hello.viewport_width(),
            hello.viewport_height(),
            opts.compression,
        );
        println!("received from {peer}: {hello_record}");
        flush_stdout("after received Hello");
        if let Err(error) = write_log_record(&mut file, &peer.to_string(), &hello_record) {
            eprintln!("log write error: {error}");
        }

        // A valid Hello makes this the active Kindle connection for frames.
        let connection_token = match current.install_negotiated(
            reader.get_ref(),
            hello.pixel_formats(),
            (hello.viewport_width(), hello.viewport_height()),
            encodings,
        ) {
            Ok(token) => token,
            Err(e) => {
                eprintln!("clone error for {peer}: {e}");
                continue;
            }
        };
        let mut application_ui = ApplicationUi::default();
        let mut reported_viewport = (hello.viewport_width(), hello.viewport_height());
        let mut application_input = ApplicationInput::default();
        let mut rendered_application_viewport = None;
        if !live.connection_changed() {
            match replace_hello_frame(
                &frame_sender,
                &mut application_ui,
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
                        AuthoritativeFrame::NowPlaying(_) => ("nowplaying", String::new()),
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
        }
        loop {
            let read = if encodings.is_some() {
                read_v3_session_message(&mut reader)
            } else {
                read_session_message(&mut reader)
            };
            let message = match read {
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
                    if live.connection_changed() {
                        rendered_application_viewport = None;
                        application_input.set_viewport(&mut application_ui, None);
                        continue;
                    }
                    match replace_application_frame(
                        &frame_sender,
                        &mut application_ui,
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
                application_input.set_viewport(
                    &mut application_ui,
                    matching_application_viewport(reported_viewport, rendered_application_viewport),
                );
            }
            let phase = match pointer.phase() {
                V2PointerPhase::Down => "down",
                V2PointerPhase::Up => "up",
            };
            let trace = pointer.trace();
            let correlation = trace
                .map(|trace| format!("session={} trace={}", trace.session_id(), trace.trace_id()))
                .unwrap_or_else(|| "session=legacy trace=legacy".to_string());
            latency::record(
                "pointer_received",
                trace,
                &format!(
                    ",\"phase\":\"{phase}\",\"x\":{},\"y\":{}",
                    pointer.x(),
                    pointer.y()
                ),
            );
            let record = format!(
                "pointer {correlation} phase={phase} x={} y={}",
                pointer.x(),
                pointer.y()
            );
            if live.pointer(pointer) {
                if let Err(error) = write_log_record(&mut file, &peer.to_string(), &record) {
                    eprintln!("log write error: {error}");
                }
                continue;
            }
            let input_result = application_input.handle_pointer(&mut application_ui, pointer);
            latency::record(
                "input_processed",
                trace,
                &format!(
                    ",\"phase\":\"{phase}\",\"visual_changed\":{},\"activation\":{}",
                    input_result.visual_changed,
                    input_result.activation.is_some()
                ),
            );
            if input_result.visual_changed && application_input.is_active() {
                let visual_state = input_result
                    .pressed_button
                    .map(|button_id| format!("pressed button={button_id}"))
                    .unwrap_or_else(|| "released".to_string());
                match frame_sender.send_application(&application_ui, reported_viewport) {
                    Ok(ApplicationSend::Sent(sent)) => {
                        latency::record(
                            "feedback_frame_sent",
                            trace,
                            &format!(
                                ",\"frame_id\":{},\"encode_us\":{},\"socket_write_us\":{}",
                                sent.frame_id,
                                sent.encode_elapsed.as_micros(),
                                sent.socket_write_elapsed.as_micros()
                            ),
                        );
                        let feedback_record = format!(
                            "sent application frame id={} width={} height={} bytes={} encode_us={} socket_write_us={} source=pointer {correlation} state={visual_state}",
                            sent.frame_id,
                            reported_viewport.0,
                            reported_viewport.1,
                            sent.encoded_len,
                            sent.encode_elapsed.as_micros(),
                            sent.socket_write_elapsed.as_micros()
                        );
                        println!("{feedback_record}");
                        flush_stdout("after pointer feedback frame");
                        if let Err(error) =
                            write_log_record(&mut file, &peer.to_string(), &feedback_record)
                        {
                            eprintln!("frame log error: {error}");
                        }
                    }
                    Ok(ApplicationSend::Unchanged) => {
                        eprintln!("pointer feedback frame unchanged after {visual_state}");
                    }
                    Ok(ApplicationSend::NoConnection) => {
                        eprintln!("pointer feedback frame not sent: no PaperPad connected");
                    }
                    Err(error) => eprintln!("pointer feedback frame error: {error}"),
                }
            }
            println!("received from {peer}: {record}");
            flush_stdout("after received line");

            // Never let a log write or flush failure tear down the connection.
            if let Err(error) = write_log_record(&mut file, &peer.to_string(), &record) {
                eprintln!("log write error: {error}");
            }

            if let Some(activation) = input_result.activation {
                latency::record(
                    "action_resolved",
                    trace,
                    &format!(
                        ",\"button\":{},\"action\":\"{}\"",
                        activation.button_id, activation.action_id
                    ),
                );
                let dispatch = if opts.forward_url {
                    let invoke_started = latency::stamp();
                    latency::record_at(
                        invoke_started,
                        "action_invoke_start",
                        trace,
                        &format!(",\"action\":\"{}\"", activation.action_id),
                    );
                    match forward_url(activation.action_id, trace) {
                        Ok(()) => {
                            let invoke_finished = latency::stamp();
                            latency::record_at(
                                invoke_finished,
                                "action_invoke_return",
                                trace,
                                &format!(
                                    ",\"action\":\"{}\",\"ok\":true,\"duration_us\":{}",
                                    activation.action_id,
                                    invoke_finished
                                        .mono_us
                                        .saturating_sub(invoke_started.mono_us)
                                ),
                            );
                            "forwarded"
                        }
                        Err(error) => {
                            let invoke_finished = latency::stamp();
                            latency::record_at(
                                invoke_finished,
                                "action_invoke_return",
                                trace,
                                &format!(
                                    ",\"action\":\"{}\",\"ok\":false,\"duration_us\":{}",
                                    activation.action_id,
                                    invoke_finished
                                        .mono_us
                                        .saturating_sub(invoke_started.mono_us)
                                ),
                            );
                            eprintln!("forward error to Hammerspoon: {error}");
                            "failed"
                        }
                    }
                } else {
                    "disabled"
                };
                let host_record = format!(
                    "host action {correlation} button={} semantic={} dispatch={dispatch}",
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
        live.connection_changed();

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

    #[test]
    fn v3_sender_falls_back_to_raw_without_losing_compression_for_next_frame() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let current = CurrentConnection::default();
        let viewport = v3::Viewport {
            width: 64,
            height: 64,
            formats: V2PixelFormats::GRAY8,
        };
        let encodings = v3::Encodings::RAW_ZSTD;
        current
            .install_negotiated(&client, viewport.formats, (64, 64), Some(encodings))
            .unwrap();
        let sender = FrameSender::new(current.clone());
        let target = current.frame_target(PixelFormat::Gray8).unwrap();
        let wrong_size = diagnostic("frame gray 42 4x2");
        assert_eq!(
            sender.send_diagnostic(wrong_size).unwrap_err(),
            "PaperPad viewport changed while preparing frame"
        );

        let mut state = 0x5eed_u32;
        let noise = (0..4096)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let mut decoder = v3::FrameDecoder::default();
        for (pixels, encoding) in [
            (vec![42; 4096], v3::Encoding::Zstd),
            (noise, v3::Encoding::Raw),
            (vec![176; 4096], v3::Encoding::Zstd),
        ] {
            let frame = Arc::new(Gray8Frame::new(64, 64, pixels).unwrap());
            let sent = sender
                .send_now_playing_frame(Arc::clone(&frame), target.clone())
                .unwrap()
                .unwrap()
                .sent;
            let mut header = [0; v3::HEADER_LEN];
            peer.read_exact(&mut header).unwrap();
            let received = decoder
                .read_frame(
                    &mut peer,
                    v3::Header::decode(header).unwrap(),
                    viewport,
                    encodings,
                )
                .unwrap();
            assert_eq!(received.id, sent.frame_id);
            assert_eq!(received.encoded_len, sent.encoded_len);
            assert_eq!(received.encoding, encoding);
            assert_eq!(received.frame.pixels(), frame.pixels());
        }
    }

    #[test]
    fn v3_connection_change_during_encoding_discards_the_pending_frame() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let current = CurrentConnection::default();
        let original = TcpStream::connect(addr).unwrap();
        let (_original_peer, _) = listener.accept().unwrap();
        let replacement = TcpStream::connect(addr).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let dimensions = (64, 64);
        let formats = V2PixelFormats::GRAY8;
        let encodings = v3::Encodings::RAW_ZSTD;
        current
            .install_negotiated(&original, formats, dimensions, Some(encodings))
            .unwrap();
        let sender = FrameSender::new(current.clone());
        let original_target = current.frame_target(PixelFormat::Gray8).unwrap();
        let stale = Arc::new(Gray8Frame::new(64, 64, vec![42; 4096]).unwrap());
        let mut state = sender.state.lock().unwrap();
        let result = sender.send_locked(
            &mut state,
            PixelFormat::Gray8,
            Some(&original_target),
            |id| {
                original.shutdown(Shutdown::Both).unwrap();
                current
                    .install_negotiated(&replacement, formats, dimensions, Some(encodings))
                    .unwrap();
                encode_v2_frame(id, stale.as_ref()).map_err(|error| error.to_string())
            },
            Some(AuthoritativeFrame::NowPlaying(Arc::clone(&stale))),
        );
        assert_eq!(
            result.unwrap_err(),
            "PaperPad connection changed while preparing frame"
        );
        assert!(state.authoritative.is_none());
        assert_eq!(state.next_frame_id, 1);
        assert!(!state.compression_scratch.is_empty());
        drop(state);

        let fresh = Arc::new(Gray8Frame::new(64, 64, vec![176; 4096]).unwrap());
        let target = current.frame_target(PixelFormat::Gray8).unwrap();
        let sent = sender
            .send_now_playing_frame(Arc::clone(&fresh), target)
            .unwrap()
            .unwrap();
        let mut header = [0; v3::HEADER_LEN];
        peer.read_exact(&mut header).unwrap();
        let received = v3::FrameDecoder::default()
            .read_frame(
                &mut peer,
                v3::Header::decode(header).unwrap(),
                v3::Viewport {
                    width: 64,
                    height: 64,
                    formats,
                },
                encodings,
            )
            .unwrap();
        assert_eq!(received.id, sent.sent.frame_id);
        assert_eq!(received.id, 1);
        assert_eq!(received.encoding, v3::Encoding::Zstd);
        assert_eq!(received.frame.pixels(), fresh.pixels());
    }

    #[cfg(unix)]
    #[test]
    fn v3_disconnect_during_a_partial_write_retires_the_connection() {
        use std::os::fd::AsRawFd;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let buffer_size: libc::c_int = 4096;
        for (socket, option) in [(&client, libc::SO_SNDBUF), (&peer, libc::SO_RCVBUF)] {
            // The pointer refers to a socket-option integer for the duration of the call.
            let status = unsafe {
                libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::SOL_SOCKET,
                    option,
                    (&raw const buffer_size).cast(),
                    size_of_val(&buffer_size) as libc::socklen_t,
                )
            };
            assert_eq!(status, 0, "{}", io::Error::last_os_error());
        }

        const WIDTH: u16 = 2048;
        const HEIGHT: u16 = 1024;
        let pixel_count = usize::from(WIDTH) * usize::from(HEIGHT);
        let current = CurrentConnection::default();
        current
            .install_negotiated(
                &client,
                V2PixelFormats::GRAY8,
                (WIDTH, HEIGHT),
                Some(v3::Encodings::RAW),
            )
            .unwrap();
        let sender = FrameSender::new(current.clone());
        let target = current.frame_target(PixelFormat::Gray8).unwrap();
        let frame = Arc::new(Gray8Frame::new(WIDTH, HEIGHT, vec![42; pixel_count]).unwrap());

        let result = std::thread::scope(|scope| {
            let sending = scope.spawn(|| sender.send_now_playing_frame(frame, target));
            let mut header = [0; v3::HEADER_LEN];
            peer.read_exact(&mut header).unwrap();
            let header = v3::Header::decode(header).unwrap();
            assert_eq!(header.kind, v3::Kind::Frame);
            assert_eq!(header.payload_len, v3::RAW_PREFIX_LEN + pixel_count);
            let mut prefix_and_pixel = [0; v3::RAW_PREFIX_LEN + 1];
            peer.read_exact(&mut prefix_and_pixel).unwrap();
            assert_eq!(prefix_and_pixel[v3::RAW_PREFIX_LEN], 42);
            assert!(
                !sending.is_finished(),
                "payload fit in the test socket buffers"
            );
            peer.shutdown(Shutdown::Both).unwrap();
            sending.join().unwrap()
        });

        assert!(result.unwrap_err().contains("failed to write frame"));
        assert!(!current.is_active());
        let state = sender.state.lock().unwrap();
        assert_eq!(state.next_frame_id, 1);
        assert!(state.authoritative.is_none());
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
        let mut ui = ApplicationUi::default();
        let mut input = ApplicationInput::default();
        let mut rendered_viewport = Some((12, 16));
        let resent = replace_hello_frame(
            &frame_sender,
            &mut ui,
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
    fn now_playing_requires_gray8_before_acquisition_and_preserves_on_failure() {
        let disconnected = FrameSender::new(CurrentConnection::default());
        assert_eq!(
            disconnected.send_now_playing_with(|_, _| panic!("must not acquire")),
            Ok(None)
        );

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let (_token, _peer) =
            install_hello_connection(&listener, &current, V2PixelFormats::MONO1, (12, 16));
        let error = frame_sender
            .send_now_playing_with(|_, _| panic!("must not acquire"))
            .expect_err("Mono1-only peer rejects Now Playing");
        assert!(error.contains("does not support Gray8"));

        let formats = V2PixelFormats::MONO1.with(PixelFormat::Gray8);
        let (_token, mut peer) = install_hello_connection(&listener, &current, formats, (12, 16));
        let baseline = frame_sender
            .send_diagnostic(diagnostic("frame gray 96 12x16"))
            .expect("send baseline")
            .expect("active connection");
        read_sent_frame(&mut peer, baseline);
        let baseline_state = {
            let state = frame_sender.state.lock().expect("frame sender lock");
            (state.next_frame_id, state.authoritative.clone())
        };

        let error = frame_sender
            .send_now_playing_with(|_, _| Err("no usable Now Playing state".to_string()))
            .expect_err("acquisition failure");
        assert!(error.contains("no usable"));
        let state = frame_sender.state.lock().expect("frame sender lock");
        assert_eq!(state.next_frame_id, baseline_state.0);
        assert_eq!(state.authoritative, baseline_state.1);
    }

    #[test]
    fn now_playing_uses_viewport_and_resends_rendered_pixels() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let formats = V2PixelFormats::MONO1.with(PixelFormat::Gray8);
        let (first_token, mut first_peer) =
            install_hello_connection(&listener, &current, formats, (12, 16));

        let first = frame_sender
            .send_now_playing_with(|width, height| {
                assert_eq!((width, height), (12, 16));
                Gray8Frame::new(
                    width,
                    height,
                    vec![77; usize::from(width) * usize::from(height)],
                )
                .map_err(|error| error.to_string())
            })
            .expect("send Now Playing")
            .expect("active connection");
        let (first_id, first_frame) = read_sent_owned_frame(&mut first_peer, first.sent);
        assert_eq!(first_id, 1);
        assert_eq!(first_frame.pixels(), &[77; 12 * 16]);

        assert!(current.clear_if_current(&first_token));
        let (_second_token, mut second_peer) =
            install_hello_connection(&listener, &current, formats, (12, 16));
        let resent = frame_sender
            .send_for_hello(&ApplicationUi::default(), (12, 16), formats)
            .expect("resend retained Now Playing")
            .expect("active connection");
        let (resent_id, resent_frame) = read_sent_owned_frame(&mut second_peer, resent.sent);
        assert_eq!(resent_id, 2);
        assert_eq!(resent_frame.pixels(), first_frame.pixels());
        assert!(resent.retained);
        assert!(matches!(resent.frame, AuthoritativeFrame::NowPlaying(_)));
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
        let mut ui = ApplicationUi::default();
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
            &mut ui,
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
        let mut ui = ApplicationUi::default();
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
            &mut ui,
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
        let mut ui = ApplicationUi::default();
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
            &mut ui,
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
        let mut ui = ApplicationUi::default();
        let mut input = ApplicationInput::default();
        let mut rendered_viewport = None;

        let sent = replace_application_frame(
            &frame_sender,
            &mut ui,
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
            &mut ui,
            &mut input,
            &mut rendered_viewport,
            (0, 128),
        )
        .expect_err("zero-width replacement must fail");
        assert!(!input.is_active());
        assert_eq!(rendered_viewport, None);
    }

    #[test]
    fn pointer_visual_transitions_send_pressed_then_restored_frames() {
        const VIEWPORT: (u16, u16) = (300, 500);
        const POINTER: (u16, u16) = (30, 70);

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let current = CurrentConnection::default();
        let frame_sender = FrameSender::new(current.clone());
        let (_token, mut peer) = install_connection(&listener, &current);
        let mut ui = ApplicationUi::default();
        let mut input = ApplicationInput::default();
        assert!(!input.set_viewport(&mut ui, Some(VIEWPORT)));

        let ApplicationSend::Sent(initial) = frame_sender
            .send_application(&ui, VIEWPORT)
            .expect("initial application frame")
        else {
            panic!("initial application frame must be sent");
        };
        let (initial_id, initial_frame) = read_sent_owned_frame(&mut peer, initial);
        let Frame::Mono1(initial_frame) = initial_frame else {
            panic!("Mono1 application frame expected");
        };
        assert_eq!(initial_id, 1);
        assert_eq!(
            initial_frame.pixel(POINTER.0, POINTER.1),
            Some(paper_protocol::Mono1Pixel::White)
        );

        let pressed = input.handle_pointer(
            &mut ui,
            paper_protocol::V2Pointer::new(V2PointerPhase::Down, POINTER.0, POINTER.1),
        );
        assert_eq!(pressed.activation, None);
        assert_eq!(pressed.pressed_button, Some(1));
        assert!(pressed.visual_changed);
        let ApplicationSend::Sent(pressed_send) = frame_sender
            .send_application(&ui, VIEWPORT)
            .expect("pressed application frame")
        else {
            panic!("pressed application frame must be sent");
        };
        let (pressed_id, pressed_frame) = read_sent_owned_frame(&mut peer, pressed_send);
        let Frame::Mono1(pressed_frame) = pressed_frame else {
            panic!("Mono1 pressed frame expected");
        };
        assert_eq!(pressed_id, 2);
        assert_eq!(
            pressed_frame.pixel(POINTER.0, POINTER.1),
            Some(paper_protocol::Mono1Pixel::Black)
        );

        let released = input.handle_pointer(
            &mut ui,
            paper_protocol::V2Pointer::new(V2PointerPhase::Up, POINTER.0, POINTER.1),
        );
        assert_eq!(
            released.activation,
            Some(paperspoon::application_input::ApplicationActivation {
                button_id: 1,
                action_id: "media.play_pause",
            })
        );
        assert_eq!(released.pressed_button, None);
        assert!(released.visual_changed);
        let ApplicationSend::Sent(restored_send) = frame_sender
            .send_application(&ui, VIEWPORT)
            .expect("restored application frame")
        else {
            panic!("restored application frame must be sent");
        };
        let (restored_id, restored_frame) = read_sent_owned_frame(&mut peer, restored_send);
        let Frame::Mono1(restored_frame) = restored_frame else {
            panic!("Mono1 restored frame expected");
        };
        assert_eq!(restored_id, 3);
        assert_eq!(restored_frame.pixels(), initial_frame.pixels());
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
    fn application_render_failure_preserves_authoritative_frame_and_id() {
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
        read_sent_frame(&mut peer, first);
        let baseline = {
            let state = frame_sender.state.lock().expect("frame sender lock");
            (
                state.next_frame_id,
                state.authoritative.clone(),
                state.last_application_frame.clone(),
            )
        };

        peer.set_nonblocking(true).expect("nonblocking peer");
        ui.set_status("unsupported 😀");
        let error = frame_sender
            .send_application(&ui, (128, 128))
            .expect_err("unsupported glyph must reject replacement frame");
        assert!(error.contains("Status"));
        assert!(error.contains("no bundled glyph"));
        assert_eq!(
            peer.read(&mut [0; 1])
                .expect_err("render failure must not write a frame")
                .kind(),
            io::ErrorKind::WouldBlock
        );

        let state = frame_sender.state.lock().expect("frame sender lock");
        assert_eq!(state.next_frame_id, baseline.0);
        assert_eq!(state.authoritative, baseline.1);
        assert_eq!(state.last_application_frame, baseline.2);
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
            &mut ui,
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
        ])
        .unwrap();
        assert_eq!(opts.port, 5581);
        assert_eq!(opts.log_path, "/tmp/paperspoon.log");
        assert_eq!(opts.latency_log_path, DEFAULT_LATENCY_LOG_FILE);
        assert_eq!(
            opts.compression.negotiate(Some(v3::Encodings::RAW_ZSTD)),
            Ok(Some(v3::Encodings::RAW_ZSTD))
        );
        assert_eq!(opts.compression.negotiate(None), Ok(None));
        assert_eq!(opts.compression_level, 1);
        assert!(opts.forward_url);
    }

    #[test]
    fn parse_options_forward_url_default_on() {
        let opts = parse_options(&["paperspoon".to_string()]).unwrap();
        assert!(opts.forward_url);
    }

    #[test]
    fn parse_options_without_forward_url_disables() {
        let opts =
            parse_options(&["paperspoon".to_string(), "--no-forward-url".to_string()]).unwrap();
        assert!(!opts.forward_url);
    }

    #[test]
    fn parse_options_positional_and_forward_url_coexist() {
        let opts = parse_options(&[
            "paperspoon".to_string(),
            "5582".to_string(),
            "/tmp/x.log".to_string(),
            "--no-forward-url".to_string(),
        ])
        .unwrap();
        assert_eq!(opts.port, 5582);
        assert_eq!(opts.log_path, "/tmp/x.log");
        assert!(!opts.forward_url);
    }

    #[test]
    fn parse_options_accepts_explicit_latency_log() {
        let opts = parse_options(&[
            "paperspoon".to_string(),
            "--latency-log=/tmp/paperspoon-trace.jsonl".to_string(),
        ])
        .unwrap();
        assert_eq!(opts.latency_log_path, "/tmp/paperspoon-trace.jsonl");
    }
}
