//! Shared operator command dispatch.

use std::io;
use std::sync::{Arc, mpsc};

use paper_protocol::PixelFormat;
use paperspoon::application_ui::ApplicationUi;
use paperspoon::operator_log;

use crate::frame_sender::{ApplicationSend, FrameSender};
use crate::live_now_playing::LiveNowPlaying;
use crate::server::FrameTarget;
use paperspoon::operator::{LiveCommand, StdinCommand};
use paperspoon::operator_log::{append_host_log_record, flush_stdout};

/// Forward one action line to Hammerspoon via `open -g hammerspoon://`.
/// The action id is passed as a query parameter (`?action=<id>`); Hammerspoon's
/// urlevent handler receives it in the params table. No sockets, no ports,
/// no file polling.
pub(crate) fn forward_url(
    action_id: &str,
    trace: Option<paper_protocol::V2PointerTrace>,
) -> io::Result<()> {
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
    let mut launch = std::process::Command::new("open");
    launch.arg("-g").arg(format!(
        "hammerspoon://paperpad?action={action_id}{trace_query}"
    ));
    if operator_log::is_capturing() {
        let output = launch.output()?;
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            operator_log::info!("{line}");
        }
        for line in String::from_utf8_lossy(&output.stderr).lines() {
            operator_log::error!("{line}");
        }
    } else {
        launch.status()?;
    }
    Ok(())
}

pub(crate) struct Operator {
    pub(crate) frame_sender: FrameSender,
    pub(crate) live: Arc<LiveNowPlaying>,
    pub(crate) application_ui: ApplicationUi,
    pub(crate) application_viewport_tx: mpsc::Sender<Option<(u16, u16)>>,
    pub(crate) log_path: String,
}

impl Operator {
    pub(crate) fn dispatch(
        &self,
        command: StdinCommand,
        source: &str,
        target: Option<&FrameTarget>,
    ) -> Result<String, String> {
        let live_command = matches!(command, StdinCommand::LiveNowPlaying(_));
        let result = self.execute(command, source, target);
        if let Err(error) = &result {
            operator_log::error!("{error}");
        }
        if live_command {
            flush_stdout("after live Now Playing command");
        }
        result
    }

    fn execute(
        &self,
        command: StdinCommand,
        source: &str,
        expected: Option<&FrameTarget>,
    ) -> Result<String, String> {
        let Self {
            frame_sender,
            live,
            application_ui,
            application_viewport_tx,
            log_path,
        } = self;
        let format = match &command {
            StdinCommand::Frame(frame) => Some(frame.pixel_format()),
            StdinCommand::Jpeg(_) | StdinCommand::NowPlaying => Some(PixelFormat::Gray8),
            StdinCommand::ApplicationFrame { .. } => Some(PixelFormat::Mono1),
            StdinCommand::LiveNowPlaying(_) => None,
        };
        if let Some(format) = format {
            if let Some(expected) = expected
                && !frame_sender
                    .frame_target(format)
                    .is_ok_and(|target| expected.is_same(&target))
            {
                return Err(
                    "PaperPad connection or viewport changed before the command could run".into(),
                );
            }
            live.stop();
        }
        match command {
            StdinCommand::LiveNowPlaying(command) => {
                let command_error = |error| format!("Now Playing command: {error}");
                match command {
                    LiveCommand::Start => live
                        .start()
                        .map(|()| {
                            if live.connection_changed() {
                                let _ = application_viewport_tx.send(None);
                            }
                            let status = live.status();
                            operator_log::info!("{status}");
                            status
                        })
                        .map_err(command_error),
                    LiveCommand::Stop => {
                        let mut restore_error = None;
                        if live.stop()
                            && let Ok(target) = frame_sender.frame_target(PixelFormat::Mono1)
                            && (source == "stdin"
                                || expected.is_some_and(|expected| expected.is_same(&target)))
                        {
                            let viewport = target.viewport();
                            match frame_sender.send_application_to(
                                application_ui,
                                viewport,
                                expected,
                            ) {
                                Ok(ApplicationSend::Sent(_) | ApplicationSend::Unchanged) => {
                                    let _ = application_viewport_tx.send(Some(viewport));
                                }
                                Ok(ApplicationSend::NoConnection) => {}
                                Err(error) => {
                                    restore_error = Some(format!(
                                        "applying grid after Now Playing stop: {error}"
                                    ));
                                }
                            }
                        }
                        let status = live.status();
                        operator_log::info!("{status}");
                        restore_error.map_or(Ok(status), Err)
                    }
                    LiveCommand::Status => {
                        let status = live.status();
                        operator_log::info!("{status}");
                        Ok(status)
                    }
                    LiveCommand::Refresh => live
                        .refresh()
                        .map(|()| "Now Playing refresh queued".into())
                        .map_err(command_error),
                    LiveCommand::Fake(change) => live
                        .fake(change)
                        .map(|()| "Test provider change queued".into())
                        .map_err(command_error),
                }
            }
            StdinCommand::Frame(frame) => match frame_sender.send_diagnostic_to(frame, expected) {
                Ok(Some(sent)) => {
                    if application_viewport_tx.send(None).is_err() {
                        operator_log::error!("application viewport tracker stopped");
                    }
                    let (width, height) = frame.dimensions();
                    let record = format!(
                        "sent diagnostic frame id={} pattern={} width={width} height={height} bytes={} encode_us={} socket_write_us={} source={source}",
                        sent.frame_id,
                        frame.pattern_name(),
                        sent.encoded_len,
                        sent.encode_elapsed.as_micros(),
                        sent.socket_write_elapsed.as_micros()
                    );
                    operator_log::info!("{record}");
                    flush_stdout("after sent frame");
                    append_host_log_record(log_path, &record);
                    Ok(format!("Sent diagnostic frame {}", sent.frame_id))
                }
                Ok(None) => Err("frame not sent: no PaperPad connected".into()),
                Err(error) => Err(format!("frame command error: {error}")),
            },
            StdinCommand::Jpeg(path) => match frame_sender.send_jpeg_to(&path, expected) {
                Ok(Some(jpeg)) => {
                    if application_viewport_tx.send(None).is_err() {
                        operator_log::error!("application viewport tracker stopped");
                    }
                    let sent = jpeg.sent;
                    let record = format!(
                        "sent JPEG frame id={} path={} width={} height={} bytes={} encode_us={} socket_write_us={} source={source}",
                        sent.frame_id,
                        path.display(),
                        jpeg.viewport.0,
                        jpeg.viewport.1,
                        sent.encoded_len,
                        sent.encode_elapsed.as_micros(),
                        sent.socket_write_elapsed.as_micros()
                    );
                    operator_log::info!("{record}");
                    flush_stdout("after sent JPEG frame");
                    append_host_log_record(log_path, &record);
                    Ok(format!("Sent JPEG frame {}", sent.frame_id))
                }
                Ok(None) => Err("JPEG frame not sent: no PaperPad connected".into()),
                Err(error) => Err(format!("JPEG frame command error: {error}")),
            },
            StdinCommand::NowPlaying => match frame_sender.send_now_playing(expected) {
                Ok(Some(now_playing)) => {
                    if application_viewport_tx.send(None).is_err() {
                        operator_log::error!("application viewport tracker stopped");
                    }
                    let sent = now_playing.sent;
                    let record = format!(
                        "sent Now Playing frame id={} width={} height={} bytes={} encode_us={} socket_write_us={} source={source}",
                        sent.frame_id,
                        now_playing.viewport.0,
                        now_playing.viewport.1,
                        sent.encoded_len,
                        sent.encode_elapsed.as_micros(),
                        sent.socket_write_elapsed.as_micros()
                    );
                    operator_log::info!("{record}");
                    flush_stdout("after Now Playing frame");
                    append_host_log_record(log_path, &record);
                    Ok(format!("Sent Now Playing frame {}", sent.frame_id))
                }
                Ok(None) => Err("Now Playing frame not sent: no PaperPad connected".into()),
                Err(error) => Err(format!("Now Playing frame error: {error}")),
            },
            StdinCommand::ApplicationFrame { width, height } => {
                match frame_sender.send_application_to(application_ui, (width, height), expected) {
                    Ok(ApplicationSend::Sent(sent)) => {
                        if application_viewport_tx.send(Some((width, height))).is_err() {
                            operator_log::error!("application viewport tracker stopped");
                        }
                        let record = format!(
                            "sent application frame id={} width={width} height={height} bytes={} encode_us={} socket_write_us={} source={source}",
                            sent.frame_id,
                            sent.encoded_len,
                            sent.encode_elapsed.as_micros(),
                            sent.socket_write_elapsed.as_micros()
                        );
                        operator_log::info!("{record}");
                        flush_stdout("after sent application frame");
                        append_host_log_record(log_path, &record);
                        Ok(format!("Sent application frame {}", sent.frame_id))
                    }
                    Ok(ApplicationSend::Unchanged) => {
                        let record = format!(
                            "application frame skipped unchanged width={width} height={height} source={source}"
                        );
                        operator_log::info!("{record}");
                        flush_stdout("after skipped application frame");
                        append_host_log_record(log_path, &record);
                        Ok("Application frame unchanged".into())
                    }
                    Ok(ApplicationSend::NoConnection) => {
                        Err("application frame not sent: no PaperPad connected".into())
                    }
                    Err(error) => Err(format!("ui command error: {error}")),
                }
            }
        }
    }
}
