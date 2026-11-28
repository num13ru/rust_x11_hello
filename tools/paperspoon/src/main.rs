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
//! Only the most recently accepted connection receives operator input.

use std::env;
use std::io::{self, BufRead, IsTerminal};
use std::net::TcpListener;
use std::sync::{Arc, mpsc};

use paper_protocol::DISCOVERY_PORT;

use paperspoon::operator::parse_stdin_command;
use paperspoon::operator_log;

mod discovery;
mod dispatch;
mod frame_sender;
mod image_frame;
mod inbound;
mod latency;
mod listener;
mod live_now_playing;
mod options;
mod server;
mod terminal;

use dispatch::Operator;
use frame_sender::FrameSender;
use listener::run_listener;
use live_now_playing::LiveNowPlaying;
use options::{TuiMode, parse_options, resolve_tui_mode};
use server::CurrentConnection;
use terminal::run_tui_mode;

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
    let resolved_tui_mode = resolve_tui_mode(
        opts.tui_mode,
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    if resolved_tui_mode != TuiMode::Disabled {
        return run_tui_mode(opts);
    }

    // Legacy console path.
    latency::init(&opts.latency_log_path);

    let listener = TcpListener::bind(("0.0.0.0", opts.port))?;
    let tcp_port = listener.local_addr()?.port();
    let discovery_socket = discovery::bind_discovery_socket()?;
    let _discovery_worker = std::thread::Builder::new()
        .name("paperspoon-discovery".to_string())
        .spawn(move || {
            if let Err(error) =
                discovery::run_discovery_listener(discovery_socket, tcp_port, || false)
            {
                operator_log::error!("discovery listener error: {error}");
            }
        })?;

    operator_log::info!(
        "listening on 0.0.0.0:{}, logging to {}",
        tcp_port,
        opts.log_path
    );
    operator_log::info!("discovery listening address=0.0.0.0:{DISCOVERY_PORT}");

    operator_log::info!(
        "type 'frame <white|black|horizontal|checkerboard|border|corners|gray-gradient|gray-bars> <width>x<height>', 'frame gray <0..255> <width>x<height>', 'frame jpeg <path>', or 'frame nowplaying' to send a framebuffer"
    );
    operator_log::info!("type 'ui <width>x<height>' to send the host-rendered application UI");
    if opts.forward_url {
        operator_log::info!(
            "forwarding actions to Hammerspoon via open -g hammerspoon://paperpad/..."
        );
    } else {
        operator_log::info!("action forwarding to Hammerspoon disabled");
    }
    operator_log::flush()?;

    let current = CurrentConnection::default();
    let frame_sender = FrameSender::new(current.clone());
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
    operator_log::info!("type 'nowplaying <start|stop|status|refresh>' for live Now Playing");
    let (application_viewport_tx, application_viewport_rx) = mpsc::channel();

    let operator = Operator {
        frame_sender: frame_sender.clone(),
        live: Arc::clone(&live),
        application_ui: paperspoon::application_ui::ApplicationUi::default(),
        application_viewport_tx,
        log_path: opts.log_path.clone(),
    };
    let _stdin_worker = std::thread::Builder::new()
        .name("paperspoon-stdin".to_string())
        .spawn(move || {
            let stdin = io::stdin();
            for line in stdin.lock().lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        operator_log::error!("stdin read error: {error}");
                        break;
                    }
                };
                let line = line.trim();
                if !line.is_empty() {
                    match parse_stdin_command(line) {
                        Ok(command) => {
                            let _ = operator.dispatch(command, "stdin", None);
                        }
                        Err(error) => operator_log::error!("stdin command error: {error}"),
                    }
                }
            }
            operator.live.stop();
        })?;

    run_listener(
        listener,
        &opts,
        current,
        frame_sender,
        live,
        application_viewport_rx,
        None,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_sender::{
        ApplicationSend, AuthoritativeFrame, SentFrame, matching_application_viewport,
        replace_application_frame, replace_hello_frame,
    };
    use crate::live_now_playing::ProviderKind;
    use crate::options::DEFAULT_LATENCY_LOG_FILE;
    use paper_protocol::{Frame, V2DecodeResult, V2Payload, decode_v2_message, decode_v2_payload};
    use paper_protocol::{
        Gray8Frame, PixelFormat, V2PixelFormats, V2PointerPhase, encode_v2_frame, v3,
    };
    use paperspoon::application_input::ApplicationInput;
    use paperspoon::application_ui::ApplicationUi;
    use paperspoon::operator::{DiagnosticFrame, StdinCommand};
    use std::io::Read;
    use std::net::{Shutdown, TcpStream};
    use std::time::Duration;

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

    #[cfg(target_os = "macos")]
    #[test]
    fn operator_restores_application_and_reports_only_successful_frame_changes() {
        let directory = tempfile::tempdir().unwrap();
        let log_path = directory.path().join("paperspoon.log");
        let log_path_string = log_path.to_str().unwrap().to_owned();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let current = CurrentConnection::default();
        let (_token, _peer) =
            install_hello_connection(&listener, &current, V2PixelFormats::MONO1, (128, 128));
        let frame_sender = FrameSender::new(current);
        let live = Arc::new(LiveNowPlaying::new(
            frame_sender.clone(),
            Some(ProviderKind::Fake),
            Duration::from_secs(1),
            log_path_string.clone(),
        ));
        let (application_viewport_tx, viewport_rx) = mpsc::channel();
        let operator = Operator {
            frame_sender,
            live,
            application_ui: ApplicationUi::default(),
            application_viewport_tx,
            log_path: log_path_string,
        };
        let dispatch = |line| {
            let _ = operator.dispatch(parse_stdin_command(line).unwrap(), "stdin", None);
        };

        dispatch("nowplaying start");
        assert!(operator.live.status().starts_with("Now Playing active"));
        assert_eq!(viewport_rx.try_recv(), Err(mpsc::TryRecvError::Empty));

        dispatch("nowplaying stop");
        assert_eq!(operator.live.status(), "Now Playing stopped");
        assert_eq!(viewport_rx.try_recv(), Ok(Some((128, 128))));

        dispatch("ui 128x128");
        dispatch("frame gray 17 128x128");
        assert_eq!(viewport_rx.try_recv(), Err(mpsc::TryRecvError::Empty));

        dispatch("nowplaying start");
        dispatch("frame white 129x128");
        assert_eq!(operator.live.status(), "Now Playing stopped");
        assert_eq!(viewport_rx.try_recv(), Ok(None));
        assert_eq!(viewport_rx.try_recv(), Err(mpsc::TryRecvError::Empty));

        let log = std::fs::read_to_string(log_path).unwrap();
        assert!(
            log.contains("application frame skipped unchanged width=128 height=128 source=stdin")
        );
        let mut sent = log
            .lines()
            .filter(|line| line.contains("sent diagnostic frame"));
        let record = sent.next().expect("diagnostic log record");
        assert!(record.contains("id=2 pattern=white width=129 height=128"));
        assert!(record.ends_with("source=stdin"));
        assert!(sent.next().is_none());
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
    fn queued_diagnostic_does_not_send_to_a_replacement_connection() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let current = CurrentConnection::default();
        let viewport = (128, 128);
        let (first, _peer) =
            install_hello_connection(&listener, &current, V2PixelFormats::MONO1, viewport);
        let (second, mut peer) =
            install_hello_connection(&listener, &current, V2PixelFormats::MONO1, viewport);
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let sender = FrameSender::new(current);
        let frame = diagnostic("frame white 128x128");

        assert!(
            sender
                .send_diagnostic_to(frame, Some(&first.at_viewport(viewport)))
                .is_err()
        );
        let sent = sender
            .send_diagnostic_to(frame, Some(&second.at_viewport(viewport)))
            .unwrap()
            .unwrap();
        assert_eq!(read_sent_frame(&mut peer, sent).0, 1);
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
    fn parse_options_accepts_tui_flags() {
        let opts = parse_options(&["paperspoon".to_string(), "--tui".to_string()]).unwrap();
        assert_eq!(opts.tui_mode, TuiMode::Force);

        let opts = parse_options(&["paperspoon".to_string(), "--no-tui".to_string()]).unwrap();
        assert_eq!(opts.tui_mode, TuiMode::Disabled);

        let opts = parse_options(&["paperspoon".to_string()]).unwrap();
        assert_eq!(opts.tui_mode, TuiMode::Auto);
    }

    #[test]
    fn parse_options_rejects_contradictory_tui_flags() {
        let error = parse_options(&[
            "paperspoon".to_string(),
            "--tui".to_string(),
            "--no-tui".to_string(),
        ])
        .unwrap_err();
        assert!(error.contains("mutually exclusive"));

        let error = parse_options(&[
            "paperspoon".to_string(),
            "--no-tui".to_string(),
            "--tui".to_string(),
        ])
        .unwrap_err();
        assert!(error.contains("mutually exclusive"));
    }

    #[test]
    fn resolve_tui_mode_requires_ttys_for_force() {
        assert!(resolve_tui_mode(TuiMode::Force, false, true).is_err());
        assert!(resolve_tui_mode(TuiMode::Force, true, false).is_err());
        assert_eq!(
            resolve_tui_mode(TuiMode::Force, true, true).unwrap(),
            TuiMode::Force
        );
        assert_eq!(
            resolve_tui_mode(TuiMode::Auto, true, true).unwrap(),
            TuiMode::Auto
        );
        assert_eq!(
            resolve_tui_mode(TuiMode::Auto, false, true).unwrap(),
            TuiMode::Disabled
        );
        assert_eq!(
            resolve_tui_mode(TuiMode::Disabled, true, true).unwrap(),
            TuiMode::Disabled
        );
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
