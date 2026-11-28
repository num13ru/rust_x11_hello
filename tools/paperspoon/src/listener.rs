//! The accept loop. One foreground owner for each PaperPad connection:
//! hello negotiation, retained-frame resend, pointer/action dispatch, and
//! per-connection log records.

use std::fs::OpenOptions;
use std::io::{self, BufReader, Write};
use std::net::SocketAddr;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc;

use paper_protocol::{V2PointerPhase, v3};

use paperspoon::application_input::ApplicationInput;
use paperspoon::application_ui::ApplicationUi;
use paperspoon::operator_log;
use paperspoon::operator_log::write_log_record;

use crate::dispatch::forward_url;
use crate::frame_sender::{
    ApplicationSend, AuthoritativeFrame, FrameSender, matching_application_viewport,
    replace_application_frame, replace_hello_frame,
};
use crate::inbound::{
    SessionMessage, read_negotiated_hello, read_session_message, read_v3_session_message,
};
use crate::latency;
use crate::live_now_playing::LiveNowPlaying;
use crate::options::Options;
use crate::server::{CurrentConnection, SharedStatus};

pub(crate) fn run_listener(
    listener: TcpListener,
    options: &Options,
    current: CurrentConnection,
    frame_sender: FrameSender,
    live: Arc<LiveNowPlaying>,
    application_viewport_rx: mpsc::Receiver<Option<(u16, u16)>>,
    status: SharedStatus,
) {
    let log_path = &options.log_path;
    let forward_url_actions = options.forward_url;
    let compression = options.compression;
    while !status.stopping() {
        let conn = listener.accept().map(|(stream, _)| stream);
        let mut stream = match conn {
            Ok(s) => s,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(25));
                continue;
            }
            Err(e) => {
                operator_log::error!("accept error: {e}");
                continue;
            }
        };
        // Some Unix platforms inherit the listener's nonblocking mode.
        if let Err(error) = stream.set_nonblocking(false) {
            operator_log::error!("cannot set blocking mode on accepted connection: {error}");
            continue;
        }
        let tracked = match status.track(&stream) {
            Ok(tracked) => tracked,
            Err(error) => {
                if !status.stopping() {
                    operator_log::error!("cannot track connection for shutdown: {error}");
                }
                continue;
            }
        };
        let peer = stream
            .peer_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        operator_log::info!("connected: {peer}");

        // A new connection has not received any application frame yet.
        while application_viewport_rx.try_recv().is_ok() {}

        let mut file = match OpenOptions::new().create(true).append(true).open(log_path) {
            Ok(f) => f,
            Err(e) => {
                operator_log::error!("log open error for {peer}: {e}");
                continue;
            }
        };
        let mut reader = BufReader::new(&mut stream);
        let (hello, offered) = match read_negotiated_hello(&mut reader) {
            Ok(Some(hello)) => hello,
            Ok(None) => continue,
            Err(error) => {
                operator_log::error!("TCP read error from {peer}: {error}");
                continue;
            }
        };
        let encodings = match compression.negotiate(offered) {
            Ok(encodings) => encodings,
            Err(error) => {
                operator_log::error!("PPFB negotiation with {peer}: {error}");
                continue;
            }
        };
        if let Some(encodings) = encodings {
            let mut socket = &**reader.get_ref();
            if let Err(error) = socket.write_all(&v3::encode_ack(encodings)) {
                operator_log::error!("PPFB v3 HelloAck to {peer}: {error}");
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
            compression,
        );
        operator_log::info!("received from {peer}: {hello_record}");
        if let Err(error) = write_log_record(&mut file, &peer.to_string(), &hello_record) {
            operator_log::error!("log write error: {error}");
        }

        // A valid Hello makes this the active Kindle connection for frames.
        let connection_token = match current.install_negotiated(
            reader.get_ref(),
            hello.pixel_formats(),
            (hello.viewport_width(), hello.viewport_height()),
            encodings,
        ) {
            Ok(token) => {
                tracked.negotiated(
                    peer,
                    token.at_viewport((hello.viewport_width(), hello.viewport_height())),
                    encodings,
                    hello.pixel_formats(),
                );
                token
            }
            Err(e) => {
                operator_log::error!("clone error for {peer}: {e}");
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
                    operator_log::info!("{record}");
                    if let Err(error) = write_log_record(&mut file, &peer.to_string(), &record) {
                        operator_log::error!("frame log error: {error}");
                    }
                }
                Ok(None) => {
                    operator_log::error!("automatic hello frame not sent: no PaperPad connected");
                }
                Err(error) => {
                    operator_log::error!("automatic hello frame error: {error}");
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
                    operator_log::error!("TCP read error from {peer}: {error}");
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
                    if current.update_viewport_if_current(&connection_token, reported_viewport) {
                        tracked.resized(connection_token.at_viewport(reported_viewport));
                    }
                    let record = format!(
                        "viewport changed width={} height={}",
                        viewport.width(),
                        viewport.height()
                    );
                    operator_log::info!("received from {peer}: {record}");
                    if let Err(error) = write_log_record(&mut file, &peer.to_string(), &record) {
                        operator_log::error!("log write error: {error}");
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
                            operator_log::info!("{record}");
                            if let Err(error) =
                                write_log_record(&mut file, &peer.to_string(), &record)
                            {
                                operator_log::error!("frame log error: {error}");
                            }
                        }
                        Ok(None) => {
                            operator_log::error!(
                                "viewport replacement frame not sent: no PaperPad connected"
                            );
                        }
                        Err(error) => {
                            operator_log::error!("viewport replacement frame error: {error}");
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
                    operator_log::error!("log write error: {error}");
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
                        operator_log::info!("{feedback_record}");
                        if let Err(error) =
                            write_log_record(&mut file, &peer.to_string(), &feedback_record)
                        {
                            operator_log::error!("frame log error: {error}");
                        }
                    }
                    Ok(ApplicationSend::Unchanged) => {
                        operator_log::error!(
                            "pointer feedback frame unchanged after {visual_state}"
                        );
                    }
                    Ok(ApplicationSend::NoConnection) => {
                        operator_log::error!(
                            "pointer feedback frame not sent: no PaperPad connected"
                        );
                    }
                    Err(error) => operator_log::error!("pointer feedback frame error: {error}"),
                }
            }
            operator_log::info!("received from {peer}: {record}");

            // Never let a log write or flush failure tear down the connection.
            if let Err(error) = write_log_record(&mut file, &peer.to_string(), &record) {
                operator_log::error!("log write error: {error}");
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
                let dispatch = if forward_url_actions {
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
                            operator_log::error!("forward error to Hammerspoon: {error}");
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
                operator_log::info!("resolved for {peer}: {host_record}");
                if let Err(error) = write_log_record(&mut file, &peer.to_string(), &host_record) {
                    operator_log::error!("log write error: {error}");
                }
            }
        }
        current.clear_if_current(&connection_token);
        live.connection_changed();

        operator_log::info!("disconnected: {peer}");
    }
}
