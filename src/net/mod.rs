//! Persistent TCP transport for outbound PaperPad input and inbound PaperSpoon
//! control/framebuffer messages.
//!
//! The Kindle connects out to PaperSpoon once at launch and keeps the
//! connection for the run. Protocol-v2 pointer phases use one bounded writer
//! queue so their byte ordering is deterministic. A reader thread
//! consumes inbound protocol-v2 Frame messages,
//! publishing the latest of each into mailboxes the X11 event loop drains
//! between events and on a bounded idle poll interval. Frames are validated
//! against the current remote viewport but are not rendered yet.
//!
//! If startup fails, the background worker retries resolution and connection.
//! If an established connection drops, the reconnector retries the same
//! endpoint. Neither path blocks or breaks the X11 event loop.

use paper_protocol::{
    V2_HEADER_LEN, V2DecodeResult, V2MessageType, V2Payload, V2Pointer, V2PointerPhase,
    decode_v2_message, decode_v2_payload,
};

mod connection;
pub mod discover;

use crate::config::PaperpadConfig;
use anyhow::{Context, Result};
use connection::{ConnectionState, ConnectionToken};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const TCP_CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
const TCP_WRITE_TIMEOUT: Duration = Duration::from_millis(150);
const STARTUP_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const STARTUP_STOP_POLL_INTERVAL: Duration = Duration::from_millis(50);
const OUTBOUND_QUEUE_CAPACITY: usize = 16;
type StartupResult = Result<(SocketAddr, TcpStream)>;

enum StartupUpdate {
    Connected {
        addr: SocketAddr,
        stream: TcpStream,
    },
    Failed {
        error: anyhow::Error,
        retry_after: Option<Duration>,
    },
}

fn configure_stream(stream: &TcpStream) -> Result<()> {
    let _ = stream.set_nodelay(true);
    stream
        .set_write_timeout(Some(TCP_WRITE_TIMEOUT))
        .context("failed to set PaperSpoon write timeout")
}

fn connect_stream(addr: SocketAddr) -> Result<TcpStream> {
    let stream = TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT)
        .context("failed to connect to PaperSpoon")?;
    configure_stream(&stream)?;
    Ok(stream)
}

fn wait_for_startup_retry(stopping: &AtomicBool, delay: Duration) -> bool {
    let deadline = Instant::now() + delay;
    loop {
        if stopping.load(Ordering::Acquire) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        std::thread::sleep(remaining.min(STARTUP_STOP_POLL_INTERVAL));
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ReceivedFrame {
    frame_id: u64,
    width: u16,
    height: u16,
    stride: usize,
    encoded: Vec<u8>,
}

impl ReceivedFrame {
    pub(crate) fn frame_id(&self) -> u64 {
        self.frame_id
    }

    pub(crate) fn width(&self) -> u16 {
        self.width
    }

    pub(crate) fn height(&self) -> u16 {
        self.height
    }

    pub(crate) fn stride(&self) -> usize {
        self.stride
    }

    pub(crate) fn pixels(&self) -> &[u8] {
        &self.encoded[V2_HEADER_LEN + paper_protocol::V2_FRAME_PREFIX_LEN..]
    }
}

#[derive(Default)]
struct FrameMailboxState {
    viewport: (u16, u16),
    pending: Option<ReceivedFrame>,
}

#[derive(Clone, Default)]
struct FrameMailbox {
    state: Arc<Mutex<FrameMailboxState>>,
}

#[derive(Debug, Eq, PartialEq)]
struct FrameDimensionMismatch {
    expected: (u16, u16),
    actual: (u16, u16),
}

impl std::fmt::Display for FrameDimensionMismatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "frame dimensions {}x{} do not match remote viewport {}x{}",
            self.actual.0, self.actual.1, self.expected.0, self.expected.1
        )
    }
}

impl FrameMailbox {
    fn set_viewport(&self, viewport: (u16, u16)) {
        let mut state = self.state.lock().expect("frame mailbox lock");
        state.viewport = viewport;
        if state
            .pending
            .as_ref()
            .is_some_and(|frame| (frame.width(), frame.height()) != viewport)
        {
            state.pending = None;
        }
    }

    fn publish(&self, frame: ReceivedFrame) -> Result<(), FrameDimensionMismatch> {
        let mut state = self.state.lock().expect("frame mailbox lock");
        let actual = (frame.width(), frame.height());
        if actual != state.viewport {
            return Err(FrameDimensionMismatch {
                expected: state.viewport,
                actual,
            });
        }
        state.pending = Some(frame);
        Ok(())
    }

    fn take(&self) -> Option<ReceivedFrame> {
        self.state
            .lock()
            .expect("frame mailbox lock")
            .pending
            .take()
    }
}

/// A persistent outbound connection to PaperSpoon with an inbound message
/// queue drained by the X11 event loop.
pub struct Paperspoon {
    /// Shared with the reconnector thread; owns the active socket.
    connection: Arc<Mutex<ConnectionState>>,
    frames: FrameMailbox,
    outbound_tx: Option<SyncSender<QueuedOutbound>>,
    wake_tx: Option<Sender<()>>,
    stopping: Arc<AtomicBool>,
    writer: Option<JoinHandle<()>>,
    reconnector: Option<JoinHandle<()>>,
    startup_rx: Option<Receiver<StartupUpdate>>,
}

struct QueuedOutbound {
    connection: ConnectionToken,
    bytes: Vec<u8>,
    description: &'static str,
}

fn write_outbound(
    connection: &Arc<Mutex<ConnectionState>>,
    outbound: &QueuedOutbound,
) -> Result<()> {
    let mut write_stream = {
        let guard = connection.lock().expect("shared stream lock");
        guard.clone_stream_for(&outbound.connection)?
    };
    if let Err(error) = write_stream.stream_mut().write_all(&outbound.bytes) {
        connection
            .lock()
            .expect("shared stream lock")
            .disconnect_if_current(&write_stream);
        return Err(error)
            .with_context(|| format!("failed to write {} to PaperSpoon", outbound.description));
    }
    Ok(())
}

fn spawn_writer(
    connection: Arc<Mutex<ConnectionState>>,
    outbound_rx: Receiver<QueuedOutbound>,
    stopping: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(outbound) = outbound_rx.recv() {
            if stopping.load(Ordering::Acquire) {
                break;
            }
            match write_outbound(&connection, &outbound) {
                Err(error) if !stopping.load(Ordering::Acquire) => {
                    eprintln!("transport error: {error:#}");
                }
                _ => {}
            }
        }
    })
}

fn enqueue_outbound<T>(outbound_tx: &SyncSender<T>, outbound: T) -> Result<()> {
    match outbound_tx.try_send(outbound) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(anyhow::anyhow!("PaperSpoon outbound queue full")),
        Err(TrySendError::Disconnected(_)) => {
            Err(anyhow::anyhow!("PaperSpoon outbound worker stopped"))
        }
    }
}

fn spawn_reader(stream: TcpStream, frames: FrameMailbox, wake_tx: Sender<()>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        loop {
            match read_inbound_frame(&mut reader) {
                Ok(Some(frame)) => match frames.publish(frame) {
                    Ok(()) => {}
                    Err(error) => eprintln!("frame rejected: {error}"),
                },
                Ok(None) => break,
                Err(error) => {
                    eprintln!("transport error reading PaperSpoon: {error}");
                    break;
                }
            }
        }
        let _ = wake_tx.send(());
    })
}

fn read_inbound_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<ReceivedFrame>> {
    if reader.fill_buf()?.is_empty() {
        return Ok(None);
    }
    read_v2_frame(reader).map(Some)
}

fn read_v2_frame<R: Read>(reader: &mut R) -> io::Result<ReceivedFrame> {
    let mut encoded = vec![0; V2_HEADER_LEN];
    reader.read_exact(&mut encoded)?;

    let additional = match decode_v2_message(&encoded).map_err(invalid_v2_data)? {
        V2DecodeResult::Incomplete { additional } => additional,
        V2DecodeResult::Complete { .. } => 0,
    };
    if additional > 0 {
        let header_len = encoded.len();
        encoded.resize(header_len + additional, 0);
        reader.read_exact(&mut encoded[header_len..])?;
    }

    let V2DecodeResult::Complete { message, consumed } =
        decode_v2_message(&encoded).map_err(invalid_v2_data)?
    else {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "incomplete protocol-v2 message",
        ));
    };
    if consumed != encoded.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "protocol-v2 reader consumed an unexpected byte count",
        ));
    }
    if message.message_type() != V2MessageType::Frame {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "PaperSpoon sent unexpected {:?} message",
                message.message_type()
            ),
        ));
    }

    let V2Payload::Frame(frame) = decode_v2_payload(message).map_err(invalid_v2_data)? else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PaperSpoon message type and typed payload disagreed",
        ));
    };
    Ok(ReceivedFrame {
        frame_id: frame.frame_id(),
        width: frame.width(),
        height: frame.height(),
        stride: frame.stride(),
        encoded,
    })
}

fn invalid_v2_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

/// Resolve the PaperSpoon address.
///
/// - If `RUST_X11_HELLO_COMPANION` is set, resolve it directly (explicit
///   host control path).
/// - Otherwise run zero-config UDP discovery; do not fall back to a
///   hard-coded IP when discovery fails (that would hide the experiment's
///   result).
fn paperspoon_addr(host: Option<&str>, port: u16) -> Result<SocketAddr> {
    match host {
        // An explicit, non-empty override resolves directly; empty or
        // missing falls through to discovery.
        Some(host) if !host.trim().is_empty() => (host, port)
            .to_socket_addrs()
            .context("failed to resolve PaperSpoon address")?
            .next()
            .context("PaperSpoon address resolved to nothing"),
        _ => crate::net::discover::discover_paperspoon(),
    }
}

impl Paperspoon {
    /// Create a disconnected PaperSpoon without a reconnect worker.
    /// Startup failures and later sends remain non-fatal to the X11 loop.
    pub fn disconnected() -> Self {
        Self {
            connection: Arc::new(Mutex::new(ConnectionState::disconnected())),
            frames: FrameMailbox::default(),
            outbound_tx: None,
            wake_tx: None,
            stopping: Arc::new(AtomicBool::new(false)),
            writer: None,
            reconnector: None,
            startup_rx: None,
        }
    }

    /// Start retrying PaperSpoon connection attempts without blocking X11.
    pub fn start(config: PaperpadConfig) -> Self {
        Self::start_with(
            move || {
                let addr = paperspoon_addr(config.host(), config.port())?;
                connect_stream(addr).map(|stream| (addr, stream))
            },
            Some(STARTUP_RETRY_INTERVAL),
        )
    }

    fn start_with<F>(mut connect: F, retry_interval: Option<Duration>) -> Self
    where
        F: FnMut() -> StartupResult + Send + 'static,
    {
        let mut paperspoon = Self::disconnected();
        let worker_stopping = Arc::clone(&paperspoon.stopping);
        let (startup_tx, startup_rx) = mpsc::sync_channel(1);
        // OS hostname resolution cannot be cancelled through std. Detaching
        // keeps local Exit independent of a resolver that has not returned;
        // a late result is dropped when this receiver no longer exists.
        drop(std::thread::spawn(move || {
            loop {
                if worker_stopping.load(Ordering::Acquire) {
                    break;
                }
                match connect() {
                    Ok((addr, stream)) => {
                        let _ = startup_tx.send(StartupUpdate::Connected { addr, stream });
                        break;
                    }
                    Err(error) => {
                        if startup_tx
                            .send(StartupUpdate::Failed {
                                error,
                                retry_after: retry_interval,
                            })
                            .is_err()
                        {
                            break;
                        }
                        let Some(delay) = retry_interval else {
                            break;
                        };
                        if !wait_for_startup_retry(&worker_stopping, delay) {
                            break;
                        }
                    }
                }
            }
        }));

        paperspoon.startup_rx = Some(startup_rx);
        paperspoon
    }

    /// Connect to a specific PaperSpoon address and start the reader thread.
    ///
    /// Internal and `pub(crate)`: tests inject an ephemeral listener this
    /// way so they never depend on process-global env vars.
    #[cfg(test)]
    pub(crate) fn connect_to(addr: SocketAddr) -> Result<Self> {
        let stream = connect_stream(addr)?;
        Self::from_stream(addr, stream, FrameMailbox::default())
    }

    fn from_stream(addr: SocketAddr, stream: TcpStream, frames: FrameMailbox) -> Result<Self> {
        // The socket is shared with a reconnector thread. When the reader
        // hits EOF, it clears the shared slot and wakes the reconnector,
        // which retries until PaperSpoon is reachable again and swaps in a
        // fresh socket — proactive auto-reconnect without user input.
        let connection = Arc::new(Mutex::new(ConnectionState::connected(stream)));
        let (outbound_tx, outbound_rx) = mpsc::sync_channel(OUTBOUND_QUEUE_CAPACITY);
        let (wake_tx, wake_rx) = mpsc::channel::<()>();

        let reader_stream = connection
            .lock()
            .expect("shared stream lock")
            .clone_stream()?
            .into_stream();
        let reader_handle = spawn_reader(reader_stream, frames.clone(), wake_tx.clone());

        // Reconnector: wake on EOF, clear slot, retry connect until success,
        // install fresh socket + reader, then wait for the next EOF.
        let worker_connection = Arc::clone(&connection);
        let stopping = Arc::new(AtomicBool::new(false));
        let writer = spawn_writer(Arc::clone(&connection), outbound_rx, Arc::clone(&stopping));
        let worker_stopping = Arc::clone(&stopping);
        let worker_frames = frames.clone();
        let worker_wake_tx = wake_tx.clone();
        let reconnector = std::thread::spawn(move || {
            let mut active_reader = Some(reader_handle);
            'reconnector: loop {
                if wake_rx.recv().is_err() {
                    break;
                }
                if let Some(reader) = active_reader.take() {
                    let _ = reader.join();
                }
                worker_connection
                    .lock()
                    .expect("shared stream lock")
                    .disconnect();
                if worker_stopping.load(Ordering::Acquire) {
                    break;
                }
                loop {
                    if worker_stopping.load(Ordering::Acquire) {
                        break 'reconnector;
                    }
                    match TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT) {
                        Ok(new_stream) => {
                            if configure_stream(&new_stream).is_err() {
                                let _ = new_stream.shutdown(Shutdown::Both);
                                std::thread::sleep(TCP_CONNECT_TIMEOUT);
                                continue;
                            }
                            let fresh_reader = new_stream.try_clone().expect("fresh clone");
                            {
                                let mut guard =
                                    worker_connection.lock().expect("shared stream lock");
                                if worker_stopping.load(Ordering::Acquire) {
                                    let _ = new_stream.shutdown(Shutdown::Both);
                                    break 'reconnector;
                                }
                                guard.reconnect(new_stream);
                            }
                            active_reader = Some(spawn_reader(
                                fresh_reader,
                                worker_frames.clone(),
                                worker_wake_tx.clone(),
                            ));
                            break;
                        }
                        Err(_) => std::thread::sleep(TCP_CONNECT_TIMEOUT),
                    }
                }
            }
        });

        Ok(Self {
            connection,
            frames,
            outbound_tx: Some(outbound_tx),
            wake_tx: Some(wake_tx),
            stopping,
            writer: Some(writer),
            reconnector: Some(reconnector),
            startup_rx: None,
        })
    }

    fn promote_startup(&mut self) {
        let update = match self.startup_rx.as_ref() {
            None => return,
            Some(startup_rx) => match startup_rx.try_recv() {
                Ok(update) => update,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.startup_rx.take();
                    eprintln!("transport error at startup: PaperSpoon startup worker stopped");
                    return;
                }
            },
        };

        match update {
            StartupUpdate::Connected { addr, stream } => {
                match Self::from_stream(addr, stream, self.frames.clone()) {
                    Ok(paperspoon) => {
                        self.startup_rx.take();
                        *self = paperspoon;
                        eprintln!("transport: connected to PaperSpoon");
                    }
                    Err(error) => {
                        self.startup_rx.take();
                        eprintln!("transport error at startup: {error:#}");
                    }
                }
            }
            StartupUpdate::Failed { error, retry_after } => {
                if let Some(delay) = retry_after {
                    eprintln!(
                        "transport error at startup: {error:#}; retrying in {} ms",
                        delay.as_millis()
                    );
                } else {
                    self.startup_rx.take();
                    eprintln!("transport error at startup: {error:#}");
                }
            }
        }
    }

    /// Queue one viewport-relative pointer phase for ordered background delivery.
    ///
    /// Disconnected, full-queue, and stopped-worker states fail immediately.
    /// Accepted pointer events are not retried after reconnect.
    pub fn send_pointer(&mut self, phase: V2PointerPhase, x: u16, y: u16) -> Result<()> {
        let encoded = V2Pointer::new(phase, x, y).encode_message()?;
        self.enqueue_bytes(encoded, "pointer event")
    }

    fn enqueue_bytes(&mut self, bytes: Vec<u8>, description: &'static str) -> Result<()> {
        self.promote_startup();
        let connection = {
            let guard = self.connection.lock().expect("shared stream lock");
            guard.current_token()?
        };
        let outbound_tx = self
            .outbound_tx
            .as_ref()
            .context("PaperSpoon outbound worker not running")?;
        enqueue_outbound(
            outbound_tx,
            QueuedOutbound {
                connection,
                bytes,
                description,
            },
        )
    }

    /// Update dimensions that inbound remote frames must match.
    pub(crate) fn set_remote_viewport(&self, viewport: (u16, u16)) {
        self.frames.set_viewport(viewport);
    }

    /// Drain the latest validated frame without blocking on network I/O.
    pub(crate) fn poll_frame(&mut self) -> Option<ReceivedFrame> {
        self.promote_startup();
        self.frames.take()
    }
}

impl Drop for Paperspoon {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.startup_rx.take();

        self.connection
            .lock()
            .expect("shared stream lock")
            .disconnect();
        if let Some(wake_tx) = self.wake_tx.take() {
            let _ = wake_tx.send(());
        }
        self.outbound_tx.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reconnector) = self.reconnector.take() {
            let _ = reconnector.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn received_frame(frame_id: u64, width: u16, height: u16, pixels: Vec<u8>) -> ReceivedFrame {
        let frame = paper_protocol::Mono1Frame::new(width, height, pixels).expect("valid Mono1");
        let encoded = paper_protocol::encode_v2_frame(frame_id, &frame).expect("encode Frame");
        read_v2_frame(&mut Cursor::new(encoded)).expect("decode Frame")
    }

    fn accept_before(listener: &std::net::TcpListener, timeout: Duration) -> TcpStream {
        listener.set_nonblocking(true).expect("set nonblocking");
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).expect("set peer blocking");
                    return stream;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline, "accept timed out");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        }
    }

    fn read_expected_pointer(peer: &mut TcpStream, expected: V2Pointer) {
        let message_len = V2_HEADER_LEN + paper_protocol::V2_POINTER_PAYLOAD_LEN;
        let mut encoded = vec![0; message_len];
        peer.read_exact(&mut encoded).expect("read pointer message");
        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(&encoded).expect("decode pointer message")
        else {
            panic!("complete pointer message expected");
        };
        assert_eq!(consumed, message_len);
        assert_eq!(decode_v2_payload(message), Ok(V2Payload::Pointer(expected)));
    }

    #[test]
    fn background_start_rejects_pointer_and_drop_is_nonblocking() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let mut paperspoon = Paperspoon::start_with(
            move || {
                started_tx.send(()).expect("announce resolver start");
                let _ = release_rx.recv();
                finished_tx.send(()).expect("announce resolver finish");
                Err(anyhow::anyhow!("injected delayed startup"))
            },
            None,
        );
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("startup worker did not run");

        assert!(
            paperspoon
                .send_pointer(V2PointerPhase::Down, 1, 2)
                .expect_err("pending startup must not accept pointer input")
                .to_string()
                .contains("PaperSpoon not connected")
        );

        let (dropped_tx, dropped_rx) = mpsc::channel();
        let dropper = std::thread::spawn(move || {
            drop(paperspoon);
            dropped_tx.send(()).expect("announce drop");
        });
        dropped_rx
            .recv_timeout(Duration::from_millis(500))
            .expect("drop waited for unresolved startup");

        release_tx.send(()).expect("release startup worker");
        finished_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("startup worker did not finish");
        dropper.join().expect("dropper join");
    }

    #[test]
    fn background_start_drop_cancels_scheduled_retry() {
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let paperspoon = Paperspoon::start_with(
            move || {
                attempt_tx.send(()).expect("announce startup attempt");
                Err(anyhow::anyhow!("injected retryable failure"))
            },
            Some(Duration::from_millis(100)),
        );
        attempt_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first startup attempt did not run");
        let update = paperspoon
            .startup_rx
            .as_ref()
            .expect("startup receiver")
            .recv_timeout(Duration::from_secs(2))
            .expect("first failure update");
        assert!(matches!(update, StartupUpdate::Failed { .. }));

        drop(paperspoon);
        assert!(
            attempt_rx.recv_timeout(Duration::from_millis(250)).is_err(),
            "startup retried after drop"
        );
    }

    #[test]
    fn background_start_promotes_connected_transport() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        let mut paperspoon = Paperspoon::start_with(
            move || connect_stream(addr).map(|stream| (addr, stream)),
            None,
        );
        let mut peer = accept_before(&listener, Duration::from_secs(2));
        peer.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set peer timeout");

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match paperspoon.send_pointer(V2PointerPhase::Down, 123, 456) {
                Ok(()) => break,
                Err(error) if error.to_string().contains("PaperSpoon not connected") => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "startup result was not promoted"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("unexpected pointer error: {error:#}"),
            }
        }
        read_expected_pointer(&mut peer, V2Pointer::new(V2PointerPhase::Down, 123, 456));
    }

    #[test]
    fn background_start_retries_then_promotes_connection() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_attempts = Arc::clone(&attempts);
        let mut paperspoon = Paperspoon::start_with(
            move || {
                if worker_attempts.fetch_add(1, Ordering::AcqRel) == 0 {
                    return Err(anyhow::anyhow!("injected first-attempt failure"));
                }
                connect_stream(addr).map(|stream| (addr, stream))
            },
            Some(Duration::from_millis(20)),
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while attempts.load(Ordering::Acquire) < 2 {
            paperspoon.promote_startup();
            assert!(
                std::time::Instant::now() < deadline,
                "startup was not retried"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut peer = accept_before(&listener, Duration::from_secs(2));
        peer.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set peer timeout");
        loop {
            match paperspoon.send_pointer(V2PointerPhase::Up, 321, 654) {
                Ok(()) => break,
                Err(error) if error.to_string().contains("PaperSpoon not connected") => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "retried startup result was not promoted"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("unexpected pointer error: {error:#}"),
            }
        }
        read_expected_pointer(&mut peer, V2Pointer::new(V2PointerPhase::Up, 321, 654));
        assert_eq!(attempts.load(Ordering::Acquire), 2);
    }

    #[test]
    fn background_start_failure_remains_disconnected() {
        let mut paperspoon =
            Paperspoon::start_with(|| Err(anyhow::anyhow!("injected startup failure")), None);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while paperspoon.startup_rx.is_some() {
            paperspoon.promote_startup();
            assert!(
                std::time::Instant::now() < deadline,
                "startup failure was not collected"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(
            paperspoon
                .send_pointer(V2PointerPhase::Down, 1, 2)
                .expect_err("failed startup must remain disconnected")
                .to_string()
                .contains("PaperSpoon not connected")
        );
    }

    fn transport_write_timeout(paperspoon: &Paperspoon) -> Option<Duration> {
        paperspoon
            .connection
            .lock()
            .expect("shared stream lock")
            .clone_stream()
            .expect("clone configured stream")
            .into_stream()
            .write_timeout()
            .expect("read write timeout")
    }

    #[test]
    fn explicit_host_resolves_directly() {
        let addr = paperspoon_addr(Some("127.0.0.1"), 6000).expect("explicit host must resolve");
        assert_eq!(addr, SocketAddr::from(([127, 0, 0, 1], 6000)));
    }

    #[test]
    fn connected_transport_has_bounded_write_timeout() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let paperspoon =
            Paperspoon::connect_to(listener.local_addr().expect("addr")).expect("connect");
        let _peer = accept_before(&listener, Duration::from_secs(2));
        assert_eq!(
            transport_write_timeout(&paperspoon),
            Some(TCP_WRITE_TIMEOUT)
        );
    }

    #[test]
    fn writer_delivers_ordered_binary_pointer_phases() {
        use std::io::Read as _;

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let mut paperspoon =
            Paperspoon::connect_to(listener.local_addr().expect("addr")).expect("connect");
        let mut peer = accept_before(&listener, Duration::from_secs(2));
        peer.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set peer timeout");

        let expected = [
            V2Pointer::new(V2PointerPhase::Down, 0x1234, 0xabcd),
            V2Pointer::new(V2PointerPhase::Up, u16::MAX, 0),
        ];
        for pointer in expected {
            paperspoon
                .send_pointer(pointer.phase(), pointer.x(), pointer.y())
                .expect("queue pointer");
        }

        let message_len = V2_HEADER_LEN + paper_protocol::V2_POINTER_PAYLOAD_LEN;
        let mut encoded = vec![0; message_len * expected.len()];
        peer.read_exact(&mut encoded)
            .expect("read pointer messages");
        for (bytes, pointer) in encoded.chunks_exact(message_len).zip(expected) {
            let V2DecodeResult::Complete { message, consumed } =
                decode_v2_message(bytes).expect("decode pointer message")
            else {
                panic!("complete pointer message expected");
            };
            assert_eq!(consumed, message_len);
            assert_eq!(decode_v2_payload(message), Ok(V2Payload::Pointer(pointer)));
        }
    }

    #[test]
    fn reader_publishes_valid_frame_from_tcp_stream() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let mut paperspoon =
            Paperspoon::connect_to(listener.local_addr().expect("addr")).expect("connect");
        paperspoon.set_remote_viewport((9, 2));
        let mut peer = accept_before(&listener, Duration::from_secs(2));
        let frame = paper_protocol::Mono1Frame::new(9, 2, vec![0xaa, 0x80, 0x55, 0x00])
            .expect("valid Mono1");
        let encoded = paper_protocol::encode_v2_frame(17, &frame).expect("encode Frame");
        peer.write_all(&encoded).expect("write Frame");

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let received = loop {
            if let Some(frame) = paperspoon.poll_frame() {
                break frame;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "reader did not publish Frame"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(received.frame_id(), 17);
        assert_eq!((received.width(), received.height()), (9, 2));
        assert_eq!(received.pixels(), &[0xaa, 0x80, 0x55, 0x00]);
    }

    #[test]
    fn outbound_enqueue_rejects_full_and_stopped_queue() {
        let (tx, rx) = mpsc::sync_channel(1);
        enqueue_outbound(&tx, "first".to_string()).expect("fill queue");
        assert!(
            enqueue_outbound(&tx, "second".to_string())
                .expect_err("full queue must reject")
                .to_string()
                .contains("outbound queue full")
        );

        drop(rx);
        assert!(
            enqueue_outbound(&tx, "third".to_string())
                .expect_err("stopped queue must reject")
                .to_string()
                .contains("outbound worker stopped")
        );
    }

    #[test]
    fn send_pointer_rejects_disconnected_transport() {
        let mut paperspoon = Paperspoon::disconnected();
        assert!(
            paperspoon
                .send_pointer(V2PointerPhase::Down, 1, 2)
                .expect_err("disconnected pointer send must fail")
                .to_string()
                .contains("PaperSpoon not connected")
        );
    }

    #[test]
    fn frame_mailbox_accepts_matching_frame_and_coalesces_latest() {
        let mailbox = FrameMailbox::default();
        mailbox.set_viewport((9, 2));
        mailbox
            .publish(received_frame(1, 9, 2, vec![0xaa, 0x80, 0x55, 0x00]))
            .expect("matching frame");
        mailbox
            .publish(received_frame(2, 9, 2, vec![0x00; 4]))
            .expect("replacement frame");

        let frame = mailbox.take().expect("pending frame");
        assert_eq!(frame.frame_id(), 2);
        assert_eq!((frame.width(), frame.height()), (9, 2));
        assert_eq!(frame.stride(), 2);
        assert_eq!(frame.pixels(), &[0x00; 4]);
        assert_eq!(mailbox.take(), None);
    }

    #[test]
    fn mismatched_frame_does_not_replace_valid_pending_frame() {
        let mailbox = FrameMailbox::default();
        mailbox.set_viewport((8, 2));
        mailbox
            .publish(received_frame(3, 8, 2, vec![0xff; 2]))
            .expect("matching frame");

        assert_eq!(
            mailbox
                .publish(received_frame(4, 8, 1, vec![0x00]))
                .expect_err("mismatched frame"),
            FrameDimensionMismatch {
                expected: (8, 2),
                actual: (8, 1),
            }
        );
        assert_eq!(mailbox.take().expect("original frame").frame_id(), 3);
    }

    #[test]
    fn viewport_change_discards_only_incompatible_pending_frame() {
        let mailbox = FrameMailbox::default();
        mailbox.set_viewport((8, 2));
        mailbox
            .publish(received_frame(5, 8, 2, vec![0x00; 2]))
            .expect("matching frame");
        mailbox.set_viewport((8, 2));
        assert_eq!(mailbox.take().expect("compatible frame").frame_id(), 5);

        mailbox
            .publish(received_frame(6, 8, 2, vec![0xff; 2]))
            .expect("matching frame");
        mailbox.set_viewport((9, 2));
        assert_eq!(mailbox.take(), None);
    }

    #[test]
    fn inbound_reader_accepts_consecutive_binary_frames() {
        let first = paper_protocol::Mono1Frame::new(9, 2, vec![0xaa, 0x80, 0x55, 0x00])
            .expect("valid first Mono1");
        let second = paper_protocol::Mono1Frame::new(8, 1, vec![0xff]).expect("valid second Mono1");
        let mut stream = paper_protocol::encode_v2_frame(7, &first).expect("encode first Frame");
        stream.extend_from_slice(
            &paper_protocol::encode_v2_frame(8, &second).expect("encode second Frame"),
        );
        let mut reader = BufReader::new(Cursor::new(stream));

        let frame = read_inbound_frame(&mut reader)
            .expect("read first frame")
            .expect("first frame");
        assert_eq!(frame.frame_id(), 7);
        assert_eq!((frame.width(), frame.height(), frame.stride()), (9, 2, 2));
        assert_eq!(frame.pixels(), &[0xaa, 0x80, 0x55, 0x00]);

        let frame = read_inbound_frame(&mut reader)
            .expect("read second frame")
            .expect("second frame");
        assert_eq!(frame.frame_id(), 8);
        assert_eq!((frame.width(), frame.height(), frame.stride()), (8, 1, 1));
        assert_eq!(frame.pixels(), &[0xff]);
        assert!(read_inbound_frame(&mut reader).expect("read EOF").is_none());
    }

    #[test]
    fn inbound_reader_rejects_malformed_or_unexpected_v2_messages() {
        let frame = paper_protocol::Mono1Frame::new(8, 1, vec![0x00]).expect("valid Mono1");
        let mut malformed = paper_protocol::encode_v2_frame(8, &frame).expect("encode Frame");
        malformed[V2_HEADER_LEN + 13] = 1;
        assert_eq!(
            read_inbound_frame(&mut BufReader::new(Cursor::new(malformed)))
                .expect_err("nonzero Frame reserved byte")
                .kind(),
            io::ErrorKind::InvalidData
        );

        let hello = paper_protocol::V2Hello::new(8, 1)
            .encode_message()
            .expect("encode Hello");
        assert_eq!(
            read_inbound_frame(&mut BufReader::new(Cursor::new(hello)))
                .expect_err("unexpected inbound Hello")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn inbound_reader_rejects_oversized_v2_header_before_payload() {
        let mut header = [0_u8; V2_HEADER_LEN];
        header[0..4].copy_from_slice(&paper_protocol::V2_MAGIC);
        header[4] = paper_protocol::V2_VERSION;
        header[5] = V2MessageType::Frame as u8;
        header[8..12]
            .copy_from_slice(&((paper_protocol::V2_MAX_PAYLOAD_LEN + 1) as u32).to_be_bytes());

        assert_eq!(
            read_inbound_frame(&mut BufReader::new(Cursor::new(header)))
                .expect_err("oversized payload header")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn legacy_inbound_line_reconnects_without_peer_eof() {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        let mut paperspoon = Paperspoon::connect_to(addr).expect("connect");
        let mut first_peer = accept_before(&listener, Duration::from_secs(2));

        first_peer
            .write_all(b"legacy line protocol\n")
            .expect("write legacy line");

        // Keep the original peer open: a second accept proves the reader
        // rejected the frame and woke the reconnector instead of seeing EOF.
        let mut replacement_peer = accept_before(&listener, Duration::from_secs(5));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match paperspoon.send_pointer(V2PointerPhase::Down, 7, 8) {
                Ok(()) => break,
                Err(error) if error.to_string().contains("PaperSpoon not connected") => {}
                Err(error) => panic!("unexpected pointer error after reconnect: {error:#}"),
            }
            assert!(
                std::time::Instant::now() < deadline,
                "legacy-line reconnect was not promoted"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        read_expected_pointer(
            &mut replacement_peer,
            V2Pointer::new(V2PointerPhase::Down, 7, 8),
        );
    }

    #[test]
    fn auto_reconnects_when_paperspoon_returns() {
        use std::io::Read;
        use std::net::TcpListener;
        use std::time::Duration;

        // Start a listener (PaperSpoon), connect, then stop it to force EOF.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        let mut paperspoon = Paperspoon::connect_to(addr).expect("connect");
        let first = accept_before(&listener, Duration::from_secs(2));
        drop(first);
        drop(listener); // Force EOF: the reconnector must notice.

        // A new PaperSpoon appears; the reconnector should restore the stream
        // without any outbound pointer call from us. The reconnector loops forever
        // retrying `addr` (now free), so rebinding the SAME port and
        // accepting proves the auto-restore.
        let listener3 = TcpListener::bind(addr).expect("rebind same port");
        let mut reconnected_peer = accept_before(&listener3, Duration::from_secs(5));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match paperspoon.send_pointer(V2PointerPhase::Up, 9, 10) {
                Ok(()) => break,
                Err(error) if error.to_string().contains("PaperSpoon not connected") => {}
                Err(error) => panic!("unexpected pointer error after reconnect: {error:#}"),
            }
            assert!(
                std::time::Instant::now() < deadline,
                "reconnected transport was not promoted"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        read_expected_pointer(
            &mut reconnected_peer,
            V2Pointer::new(V2PointerPhase::Up, 9, 10),
        );

        // Drop must stop and join the reader even while its peer remains idle.
        assert_eq!(
            transport_write_timeout(&paperspoon),
            Some(TCP_WRITE_TIMEOUT)
        );
        reconnected_peer
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set peer deadline");
        drop(paperspoon);
        let mut byte = [0];
        assert_eq!(reconnected_peer.read(&mut byte).expect("read EOF"), 0);
    }
}
