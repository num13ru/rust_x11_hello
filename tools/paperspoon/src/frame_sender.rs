//! Shared frame encoding and send-side state.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use paper_protocol::compression::ZstdEncoder;
use paper_protocol::v3;
use paper_protocol::{Gray8Frame, Mono1Frame, PixelFormat, V2PixelFormats, encode_v2_frame};

use paperspoon::application_input::ApplicationInput;
use paperspoon::application_renderer::ApplicationRenderer;
use paperspoon::application_ui::ApplicationUi;
use paperspoon::now_playing::load_now_playing_frame;
use paperspoon::operator::DiagnosticFrame;
use paperspoon::operator_log;

use crate::image_frame::decode_jpeg_path_fit_contain;
use crate::server::{
    ConnectionToken, CurrentConnection, FrameForward, FrameTarget, FrameTargetError,
};

pub(crate) fn matching_application_viewport(
    reported_viewport: (u16, u16),
    rendered_application_viewport: Option<(u16, u16)>,
) -> Option<(u16, u16)> {
    rendered_application_viewport.filter(|viewport| *viewport == reported_viewport)
}

#[derive(Clone)]
pub(crate) struct FrameSender {
    pub(crate) current: CurrentConnection,
    pub(crate) state: Arc<Mutex<FrameSenderState>>,
}

pub(crate) struct FrameSenderState {
    pub(crate) next_frame_id: u64,
    pub(crate) live_active: bool,
    pub(crate) authoritative: Option<AuthoritativeFrame>,
    pub(crate) last_application_frame: Option<Mono1Frame>,
    pub(crate) application_renderer: Option<ApplicationRenderer>,
    pub(crate) encoding_session: Option<ConnectionToken>,
    pub(crate) encoder: Option<ZstdEncoder>,
    pub(crate) compression_scratch: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AuthoritativeFrame {
    Application { viewport: (u16, u16) },
    Diagnostic(DiagnosticFrame),
    Jpeg(Arc<Gray8Frame>),
    NowPlaying(Arc<Gray8Frame>),
}

impl AuthoritativeFrame {
    pub(crate) fn dimensions(&self) -> (u16, u16) {
        match self {
            Self::Application { viewport } => *viewport,
            Self::Diagnostic(frame) => frame.dimensions(),
            Self::Jpeg(frame) | Self::NowPlaying(frame) => (frame.width(), frame.height()),
        }
    }

    pub(crate) fn application_viewport(&self) -> Option<(u16, u16)> {
        match self {
            Self::Application { viewport } => Some(*viewport),
            Self::Diagnostic(_) | Self::Jpeg(_) | Self::NowPlaying(_) => None,
        }
    }

    pub(crate) fn pixel_format(&self) -> PixelFormat {
        match self {
            Self::Application { .. } => PixelFormat::Mono1,
            Self::Diagnostic(frame) => frame.pixel_format(),
            Self::Jpeg(_) | Self::NowPlaying(_) => PixelFormat::Gray8,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SentFrame {
    pub(crate) frame_id: u64,
    pub(crate) encoded_len: usize,
    pub(crate) application_viewport: Option<(u16, u16)>,
    pub(crate) encode_elapsed: Duration,
    pub(crate) socket_write_elapsed: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SentJpeg {
    pub(crate) sent: SentFrame,
    pub(crate) viewport: (u16, u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SentNowPlaying {
    pub(crate) sent: SentFrame,
    pub(crate) viewport: (u16, u16),
}

#[derive(Debug)]
pub(crate) enum ApplicationSend {
    Sent(SentFrame),
    Unchanged,
    NoConnection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HelloFrame {
    pub(crate) sent: SentFrame,
    pub(crate) retained: bool,
    pub(crate) frame: AuthoritativeFrame,
}

impl FrameSender {
    pub(crate) fn new(current: CurrentConnection) -> Self {
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
                compression_scratch: Vec::new(),
            })),
        }
    }

    pub(crate) fn set_live_active(&self, active: bool) {
        self.state.lock().expect("frame sender lock").live_active = active;
    }

    pub(crate) fn frame_target(
        &self,
        pixel_format: PixelFormat,
    ) -> Result<FrameTarget, FrameTargetError> {
        self.current.frame_target(pixel_format)
    }

    #[cfg(test)]
    pub(crate) fn send_with(
        &self,
        encode: impl FnOnce(u64) -> Result<Vec<u8>, String>,
    ) -> Result<Option<SentFrame>, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        self.send_locked(&mut state, PixelFormat::Mono1, None, encode, None)
    }

    pub(crate) fn send_locked(
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
                match ZstdEncoder::new(1) {
                    Ok(encoder) => state.encoder = Some(encoder),
                    Err(error) => operator_log::error!(
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
                        operator_log::error!(
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

    #[cfg(test)]
    pub(crate) fn send_diagnostic(
        &self,
        frame: DiagnosticFrame,
    ) -> Result<Option<SentFrame>, String> {
        self.send_diagnostic_to(frame, None)
    }

    pub(crate) fn send_diagnostic_to(
        &self,
        frame: DiagnosticFrame,
        target: Option<&FrameTarget>,
    ) -> Result<Option<SentFrame>, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        self.send_locked(
            &mut state,
            frame.pixel_format(),
            target,
            |frame_id| frame.encode(frame_id),
            Some(AuthoritativeFrame::Diagnostic(frame)),
        )
    }

    #[cfg(test)]
    pub(crate) fn send_jpeg(&self, path: &std::path::Path) -> Result<Option<SentJpeg>, String> {
        self.send_jpeg_to(path, None)
    }

    pub(crate) fn send_jpeg_to(
        &self,
        path: &std::path::Path,
        expected: Option<&FrameTarget>,
    ) -> Result<Option<SentJpeg>, String> {
        let Some(target) = self.gray_target(expected)? else {
            return Ok(None);
        };
        let viewport = target.viewport();
        let frame = Arc::new(decode_jpeg_path_fit_contain(path, viewport.0, viewport.1)?);
        self.send_jpeg_frame(frame, target)
    }

    pub(crate) fn send_jpeg_frame(
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

    pub(crate) fn send_now_playing(
        &self,
        expected: Option<&FrameTarget>,
    ) -> Result<Option<SentNowPlaying>, String> {
        self.send_now_playing_to(load_now_playing_frame, expected)
    }

    #[cfg(test)]
    pub(crate) fn send_now_playing_with(
        &self,
        load: impl FnOnce(u16, u16) -> Result<Gray8Frame, String>,
    ) -> Result<Option<SentNowPlaying>, String> {
        self.send_now_playing_to(load, None)
    }

    fn gray_target(&self, expected: Option<&FrameTarget>) -> Result<Option<FrameTarget>, String> {
        match self.frame_target(PixelFormat::Gray8) {
            Ok(target) if expected.is_some_and(|expected| !expected.is_same(&target)) => {
                Err("PaperPad connection or viewport changed before the command could run".into())
            }
            Ok(target) => Ok(Some(target)),
            Err(FrameTargetError::NoConnection) => Ok(None),
            Err(FrameTargetError::UnsupportedPixelFormat) => {
                Err("connected PaperPad does not support Gray8 frames".into())
            }
            Err(FrameTargetError::ViewportUnavailable) => {
                Err("connected PaperPad has no active viewport".into())
            }
        }
    }

    fn send_now_playing_to(
        &self,
        load: impl FnOnce(u16, u16) -> Result<Gray8Frame, String>,
        expected: Option<&FrameTarget>,
    ) -> Result<Option<SentNowPlaying>, String> {
        let Some(target) = self.gray_target(expected)? else {
            return Ok(None);
        };
        let viewport = target.viewport();
        let frame = Arc::new(load(viewport.0, viewport.1)?);
        self.send_now_playing_frame(frame, target)
    }

    pub(crate) fn send_now_playing_frame(
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

    pub(crate) fn send_live_frame(
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

    pub(crate) fn send_application(
        &self,
        ui: &ApplicationUi,
        viewport: (u16, u16),
    ) -> Result<ApplicationSend, String> {
        self.send_application_to(ui, viewport, None)
    }

    pub(crate) fn send_application_to(
        &self,
        ui: &ApplicationUi,
        viewport: (u16, u16),
        target: Option<&FrameTarget>,
    ) -> Result<ApplicationSend, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        if state.live_active && self.current.frame_target(PixelFormat::Gray8).is_ok() {
            return Ok(ApplicationSend::Unchanged);
        }
        self.send_application_locked(&mut state, ui, viewport, true, target)
    }

    pub(crate) fn send_application_locked(
        &self,
        state: &mut FrameSenderState,
        ui: &ApplicationUi,
        viewport: (u16, u16),
        suppress_unchanged: bool,
        target: Option<&FrameTarget>,
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
            && target.is_none_or(|expected| {
                self.frame_target(PixelFormat::Mono1)
                    .is_ok_and(|target| expected.is_same(&target))
            })
            && matches!(state.authoritative.as_ref(), Some(AuthoritativeFrame::Application { viewport: current }) if *current == viewport)
            && state.last_application_frame.as_ref() == Some(&candidate)
        {
            return Ok(ApplicationSend::Unchanged);
        }
        let sent = self.send_locked(
            state,
            PixelFormat::Mono1,
            target,
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

    pub(crate) fn send_application_for_viewport_change(
        &self,
        ui: &ApplicationUi,
        viewport: (u16, u16),
    ) -> Result<Option<SentFrame>, String> {
        let mut state = self.state.lock().expect("frame sender lock");
        if state.live_active && self.current.frame_target(PixelFormat::Gray8).is_ok() {
            return Ok(None);
        }
        state.last_application_frame = None;
        match self.send_application_locked(&mut state, ui, viewport, false, None)? {
            ApplicationSend::Sent(sent) => Ok(Some(sent)),
            ApplicationSend::NoConnection => Ok(None),
            ApplicationSend::Unchanged => {
                Err("viewport replacement was unexpectedly suppressed".to_string())
            }
        }
    }

    pub(crate) fn send_for_hello(
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
                match self.send_application_locked(&mut state, ui, *viewport, false, None)? {
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

pub(crate) fn replace_application_frame(
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

pub(crate) fn replace_hello_frame(
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
