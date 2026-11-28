//! Persistent TCP transport for outbound PaperPad input and inbound PaperSpoon
//! control/framebuffer messages.
//!
//! The Kindle connects out to PaperSpoon once at launch and keeps the
//! connection for the run. Protocol-v2 pointer phases use one bounded writer
//! queue so their byte ordering is deterministic. A reader thread
//! consumes inbound protocol-v2 Frame messages,
//! publishing the latest of each into mailboxes the PaperPad runtime drains
//! between events and on a bounded idle poll interval. Frames are validated
//! against the current remote viewport but are not rendered yet.
//!
//! If startup fails, the background worker retries resolution and connection.
//! If an established connection drops, the reconnector retries the same
//! endpoint. Neither path blocks or breaks the PaperPad runtime loop.

use paper_protocol::v3;
use paper_protocol::{
    V2_HEADER_LEN, V2DecodeResult, V2Hello, V2MessageType, V2Payload, V2PixelFormat,
    V2PixelFormats, V2Pointer, V2PointerPhase, V2PointerTrace, decode_v2_message,
    decode_v2_payload,
};

mod connection;
pub mod discover;

use crate::config::PaperpadConfig;
use crate::latency;
#[cfg(target_os = "linux")]
use crate::runtime_wait::RuntimeNotifier;
use anyhow::{Context, Result};
use connection::{ConnectionState, ConnectionStream, ConnectionToken};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const TCP_CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
const TCP_WRITE_TIMEOUT: Duration = Duration::from_millis(150);
const STARTUP_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const OUTBOUND_QUEUE_CAPACITY: usize = 16;
type StartupResult = Result<(SocketAddr, TcpStream)>;

/// Main-runtime wakeup, separate from the reader-to-reconnector channel.
#[derive(Clone, Debug, Default)]
struct RuntimeWake {
    #[cfg(target_os = "linux")]
    notifier: Option<RuntimeNotifier>,
}

impl RuntimeWake {
    #[cfg(target_os = "linux")]
    fn new(notifier: RuntimeNotifier) -> Self {
        Self {
            notifier: Some(notifier),
        }
    }

    fn notify(&self) {
        #[cfg(target_os = "linux")]
        match self.notifier.as_ref().map(RuntimeNotifier::notify) {
            Some(Err(error)) => {
                eprintln!("transport failed to notify native runtime: {error}");
            }
            Some(Ok(())) | None => {}
        }

        #[cfg(not(target_os = "linux"))]
        let _ = self;
    }
}

#[derive(Debug, Default)]
struct StopSignal {
    requested: AtomicBool,
    wait_lock: Mutex<()>,
    wait_condvar: Condvar,
}

impl StopSignal {
    fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    fn request(&self) {
        let _guard = self
            .wait_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.requested.store(true, Ordering::Release);
        self.wait_condvar.notify_all();
    }

    /// Wait for the retry deadline. Returns `false` when shutdown interrupts it.
    fn wait_retry(&self, delay: Duration) -> bool {
        if self.is_requested() {
            return false;
        }
        let guard = self
            .wait_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (_guard, _timeout) = self
            .wait_condvar
            .wait_timeout_while(guard, delay, |_| !self.is_requested())
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !self.is_requested()
    }
}

enum StartupUpdate {
    Connected {
        addr: SocketAddr,
        stream: TcpStream,
        encodings: Option<v3::Encodings>,
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

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ReceivedFrame {
    frame_id: u64,
    width: u16,
    height: u16,
    pixel_format: V2PixelFormat,
    stride: usize,
    data: Vec<u8>,
    pixel_offset: usize,
    receive_decode_elapsed: Duration,
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

    pub(crate) fn pixel_format(&self) -> V2PixelFormat {
        self.pixel_format
    }

    pub(crate) fn stride(&self) -> usize {
        self.stride
    }

    pub(crate) fn pixels(&self) -> &[u8] {
        &self.data[self.pixel_offset..]
    }

    pub(crate) fn receive_decode_us(&self) -> u128 {
        self.receive_decode_elapsed.as_micros()
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
    runtime_wake: RuntimeWake,
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
    fn with_runtime_wake(runtime_wake: RuntimeWake) -> Self {
        Self {
            state: Arc::default(),
            runtime_wake,
        }
    }

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
        {
            let mut state = self.state.lock().expect("frame mailbox lock");
            let actual = (frame.width(), frame.height());
            if actual != state.viewport {
                return Err(FrameDimensionMismatch {
                    expected: state.viewport,
                    actual,
                });
            }
            state.pending = Some(frame);
        }
        self.runtime_wake.notify();
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
/// queue drained by the PaperPad runtime.
pub struct Paperspoon {
    /// Shared with the reconnector thread; owns the active socket.
    connection: Arc<Mutex<ConnectionState>>,
    remote_viewport: Arc<Mutex<(u16, u16)>>,
    frames: FrameMailbox,
    outbound_tx: Option<SyncSender<QueuedOutbound>>,
    wake_tx: Option<Sender<()>>,
    stopping: Arc<StopSignal>,
    writer: Option<JoinHandle<()>>,
    reconnector: Option<JoinHandle<()>>,
    startup_rx: Option<Receiver<StartupUpdate>>,
    pixel_formats: V2PixelFormats,
    transport: v3::ClientMode,
}

fn write_hello(
    stream: &mut TcpStream,
    viewport: (u16, u16),
    pixel_formats: V2PixelFormats,
) -> Result<()> {
    let encoded =
        V2Hello::with_pixel_formats(viewport.0, viewport.1, pixel_formats).encode_message()?;
    stream
        .write_all(&encoded)
        .context("failed to write Hello to PaperSpoon")
}

fn negotiate(
    stream: TcpStream,
    addr: SocketAddr,
    viewport: (u16, u16),
    formats: V2PixelFormats,
    mode: v3::ClientMode,
) -> Result<(TcpStream, Option<v3::Encodings>)> {
    if mode != v3::ClientMode::Legacy {
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    }
    let (stream, encodings) = mode
        .negotiate(
            stream,
            v3::Viewport {
                width: viewport.0,
                height: viewport.1,
                formats,
            },
            || connect_stream(addr),
        )
        .context("PaperSpoon transport negotiation failed")?;
    stream.set_read_timeout(None)?;
    match encodings {
        Some(encodings) => eprintln!("PaperSpoon: negotiated PPFB v3 encodings={encodings:?}"),
        None => eprintln!("PaperSpoon: using PPFB v2 Raw"),
    }
    Ok((stream, encodings))
}

struct QueuedOutbound {
    connection: ConnectionToken,
    bytes: Vec<u8>,
    description: &'static str,
    trace: Option<V2PointerTrace>,
}

fn disconnect_if_current_and_notify(
    connection: &Arc<Mutex<ConnectionState>>,
    candidate: &ConnectionStream,
    runtime_wake: &RuntimeWake,
) -> bool {
    let disconnected = connection
        .lock()
        .expect("shared stream lock")
        .disconnect_if_current(candidate);
    if disconnected {
        runtime_wake.notify();
    }
    disconnected
}

fn write_outbound(
    connection: &Arc<Mutex<ConnectionState>>,
    outbound: &QueuedOutbound,
    runtime_wake: &RuntimeWake,
) -> Result<()> {
    let mut write_stream = {
        let guard = connection.lock().expect("shared stream lock");
        guard.clone_stream_for(&outbound.connection)?
    };
    let write_started = latency::stamp();
    if let Some(trace) = outbound.trace {
        latency::record_at(write_started, "socket_write_start", Some(trace), "");
    }
    if let Err(error) = write_stream.stream_mut().write_all(&outbound.bytes) {
        if let Some(trace) = outbound.trace {
            latency::record("socket_write_error", Some(trace), "");
        }
        disconnect_if_current_and_notify(connection, &write_stream, runtime_wake);
        return Err(error)
            .with_context(|| format!("failed to write {} to PaperSpoon", outbound.description));
    }
    if let Some(trace) = outbound.trace {
        latency::record("socket_write_done", Some(trace), "");
    }
    Ok(())
}

fn spawn_writer(
    connection: Arc<Mutex<ConnectionState>>,
    outbound_rx: Receiver<QueuedOutbound>,
    stopping: Arc<StopSignal>,
    runtime_wake: RuntimeWake,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(outbound) = outbound_rx.recv() {
            if stopping.is_requested() {
                break;
            }
            match write_outbound(&connection, &outbound, &runtime_wake) {
                Err(error) if !stopping.is_requested() => {
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

fn spawn_reader(
    stream: TcpStream,
    frames: FrameMailbox,
    wake_tx: Sender<()>,
    formats: V2PixelFormats,
    encodings: Option<v3::Encodings>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut decoder = v3::FrameDecoder::default();
        loop {
            let result = match encodings {
                Some(encodings) => {
                    read_v3_frame(&mut reader, &frames, formats, encodings, &mut decoder)
                }
                None => read_inbound_frame(&mut reader),
            };
            match result {
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
    // The production reader calls this after fill_buf sees the first frame bytes.
    // Idle time before a frame begins is excluded from this measurement.
    let receive_decode_started = Instant::now();
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
    let receive_decode_elapsed = receive_decode_started.elapsed();
    latency::record(
        "frame_received",
        None,
        &format!(
            ",\"frame_id\":{},\"receive_decode_us\":{}",
            frame.frame_id(),
            receive_decode_elapsed.as_micros()
        ),
    );
    Ok(ReceivedFrame {
        frame_id: frame.frame_id(),
        width: frame.width(),
        height: frame.height(),
        pixel_format: frame.pixel_format(),
        stride: frame.stride(),
        data: encoded,
        pixel_offset: V2_HEADER_LEN + paper_protocol::V2_FRAME_PREFIX_LEN,
        receive_decode_elapsed,
    })
}

fn read_v3_frame(
    reader: &mut impl BufRead,
    frames: &FrameMailbox,
    formats: V2PixelFormats,
    encodings: v3::Encodings,
    decoder: &mut v3::FrameDecoder,
) -> io::Result<Option<ReceivedFrame>> {
    if reader.fill_buf()?.is_empty() {
        return Ok(None);
    }
    let started = Instant::now();
    let mut bytes = [0; V2_HEADER_LEN];
    reader.read_exact(&mut bytes)?;
    let header = v3::Header::decode(bytes)?;
    let (width, height) = frames.state.lock().expect("frame mailbox lock").viewport;
    let received = decoder.read_frame(
        reader,
        header,
        v3::Viewport {
            width,
            height,
            formats,
        },
        encodings,
    )?;
    let receive_decode_elapsed = started.elapsed();
    let frame = received.frame;
    latency::record(
        "frame_received",
        None,
        &format!(
            ",\"frame_id\":{},\"receive_decode_us\":{},\"encoding\":\"{:?}\",\"wire_bytes\":{},\"raw_bytes\":{},\"decode_us\":{},\"validation_us\":{}",
            received.id,
            receive_decode_elapsed.as_micros(),
            received.encoding,
            received.encoded_len,
            frame.pixels().len(),
            received.decode_elapsed.as_micros(),
            received.validation_elapsed.as_micros()
        ),
    );
    Ok(Some(ReceivedFrame {
        frame_id: received.id,
        width,
        height,
        pixel_format: frame.pixel_format(),
        stride: frame.stride(),
        data: frame.into_pixels(),
        pixel_offset: 0,
        receive_decode_elapsed,
    }))
}

fn invalid_v2_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

/// Resolve the PaperSpoon address.
///
/// - If `PAPERPAD_COMPANION` is set, resolve it directly (explicit
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
    /// Startup failures and later sends remain non-fatal to the runtime loop.
    #[cfg(test)]
    pub fn disconnected() -> Self {
        Self::disconnected_with_runtime_wake(RuntimeWake::default())
    }

    fn disconnected_with_runtime_wake(runtime_wake: RuntimeWake) -> Self {
        Self {
            connection: Arc::new(Mutex::new(ConnectionState::disconnected())),
            remote_viewport: Arc::new(Mutex::new((0, 0))),
            frames: FrameMailbox::with_runtime_wake(runtime_wake),
            outbound_tx: None,
            wake_tx: None,
            stopping: Arc::new(StopSignal::default()),
            writer: None,
            reconnector: None,
            startup_rx: None,
            pixel_formats: V2PixelFormats::MONO1,
            transport: v3::ClientMode::Legacy,
        }
    }

    /// Start retrying PaperSpoon connection attempts without blocking PaperPad.
    #[cfg(target_os = "linux")]
    pub fn start(
        config: PaperpadConfig,
        remote_viewport: (u16, u16),
        pixel_formats: V2PixelFormats,
        runtime_notifier: RuntimeNotifier,
    ) -> Self {
        let mode = config.transport();
        Self::start_with_transport(
            move || {
                let addr = paperspoon_addr(config.host(), config.port())?;
                connect_stream(addr).map(|stream| (addr, stream))
            },
            Some(STARTUP_RETRY_INTERVAL),
            remote_viewport,
            pixel_formats,
            RuntimeWake::new(runtime_notifier),
            mode,
        )
    }

    #[cfg(test)]
    fn start_with<F>(
        connect: F,
        retry_interval: Option<Duration>,
        remote_viewport: (u16, u16),
    ) -> Self
    where
        F: FnMut() -> StartupResult + Send + 'static,
    {
        Self::start_with_pixel_formats(
            connect,
            retry_interval,
            remote_viewport,
            V2PixelFormats::MONO1,
            RuntimeWake::default(),
        )
    }

    #[cfg(test)]
    fn start_with_pixel_formats<F>(
        connect: F,
        retry_interval: Option<Duration>,
        remote_viewport: (u16, u16),
        pixel_formats: V2PixelFormats,
        runtime_wake: RuntimeWake,
    ) -> Self
    where
        F: FnMut() -> StartupResult + Send + 'static,
    {
        Self::start_with_transport(
            connect,
            retry_interval,
            remote_viewport,
            pixel_formats,
            runtime_wake,
            v3::ClientMode::Legacy,
        )
    }

    fn start_with_transport<F>(
        mut connect: F,
        retry_interval: Option<Duration>,
        remote_viewport: (u16, u16),
        pixel_formats: V2PixelFormats,
        runtime_wake: RuntimeWake,
        transport: v3::ClientMode,
    ) -> Self
    where
        F: FnMut() -> StartupResult + Send + 'static,
    {
        let mut paperspoon = Self::disconnected_with_runtime_wake(runtime_wake.clone());
        paperspoon.pixel_formats = pixel_formats;
        paperspoon.transport = transport;
        let worker_viewport = Arc::clone(&paperspoon.remote_viewport);
        paperspoon.set_remote_viewport(remote_viewport);
        let worker_stopping = Arc::clone(&paperspoon.stopping);
        let (startup_tx, startup_rx) = mpsc::sync_channel(1);
        // OS hostname resolution cannot be cancelled through std. Detaching
        // keeps local Exit independent of a resolver that has not returned;
        // a late result is dropped when this receiver no longer exists.
        drop(std::thread::spawn(move || {
            loop {
                if worker_stopping.is_requested() {
                    break;
                }
                let connected = connect().and_then(|(addr, stream)| {
                    if transport == v3::ClientMode::Legacy {
                        return Ok((addr, stream, None));
                    }
                    let viewport = *worker_viewport.lock().expect("remote viewport lock");
                    let (stream, encodings) =
                        negotiate(stream, addr, viewport, pixel_formats, transport)?;
                    Ok((addr, stream, encodings))
                });
                match connected {
                    Ok((addr, stream, encodings)) => {
                        if startup_tx
                            .send(StartupUpdate::Connected {
                                addr,
                                stream,
                                encodings,
                            })
                            .is_ok()
                        {
                            runtime_wake.notify();
                        }
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
                        runtime_wake.notify();
                        let Some(delay) = retry_interval else {
                            break;
                        };
                        if !worker_stopping.wait_retry(delay) {
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
    pub(crate) fn connect_to(addr: SocketAddr, remote_viewport: (u16, u16)) -> Result<Self> {
        Self::connect_to_with_pixel_formats(addr, remote_viewport, V2PixelFormats::MONO1)
    }

    #[cfg(test)]
    fn connect_to_with_pixel_formats(
        addr: SocketAddr,
        remote_viewport: (u16, u16),
        pixel_formats: V2PixelFormats,
    ) -> Result<Self> {
        let stream = connect_stream(addr)?;
        Self::from_stream(
            addr,
            stream,
            FrameMailbox::default(),
            remote_viewport,
            pixel_formats,
        )
    }

    #[cfg(test)]
    fn from_stream(
        addr: SocketAddr,
        mut stream: TcpStream,
        frames: FrameMailbox,
        remote_viewport: (u16, u16),
        pixel_formats: V2PixelFormats,
    ) -> Result<Self> {
        write_hello(&mut stream, remote_viewport, pixel_formats)?;
        Self::from_negotiated_stream(
            addr,
            stream,
            frames,
            remote_viewport,
            pixel_formats,
            v3::ClientMode::Legacy,
            None,
        )
    }

    fn from_negotiated_stream(
        addr: SocketAddr,
        stream: TcpStream,
        frames: FrameMailbox,
        remote_viewport: (u16, u16),
        pixel_formats: V2PixelFormats,
        transport: v3::ClientMode,
        encodings: Option<v3::Encodings>,
    ) -> Result<Self> {
        // The socket is shared with a reconnector thread. When the reader
        // hits EOF, it clears the shared slot and wakes the reconnector,
        // which retries until PaperSpoon is reachable again and swaps in a
        // fresh socket — proactive auto-reconnect without user input.
        frames.set_viewport(remote_viewport);
        let runtime_wake = frames.runtime_wake.clone();
        let connection = Arc::new(Mutex::new(ConnectionState::connected(
            stream,
            encodings.is_some(),
        )));
        let remote_viewport = Arc::new(Mutex::new(remote_viewport));
        let (outbound_tx, outbound_rx) = mpsc::sync_channel(OUTBOUND_QUEUE_CAPACITY);
        let (wake_tx, wake_rx) = mpsc::channel::<()>();

        let reader_stream = connection
            .lock()
            .expect("shared stream lock")
            .clone_stream()?
            .into_stream();
        let reader_handle = spawn_reader(
            reader_stream,
            frames.clone(),
            wake_tx.clone(),
            pixel_formats,
            encodings,
        );

        // Reconnector: wake on EOF, clear slot, retry connect until success,
        // install fresh socket + reader, then wait for the next EOF.
        let worker_connection = Arc::clone(&connection);
        let stopping = Arc::new(StopSignal::default());
        let writer = spawn_writer(
            Arc::clone(&connection),
            outbound_rx,
            Arc::clone(&stopping),
            runtime_wake.clone(),
        );
        let worker_stopping = Arc::clone(&stopping);
        let worker_frames = frames.clone();
        let worker_wake_tx = wake_tx.clone();
        let worker_remote_viewport = Arc::clone(&remote_viewport);
        let worker_runtime_wake = runtime_wake.clone();
        let reconnector = std::thread::spawn(move || {
            let mut active_reader = Some(reader_handle);
            'reconnector: loop {
                if wake_rx.recv().is_err() {
                    break;
                }
                if let Some(reader) = active_reader.take() {
                    let _ = reader.join();
                }
                let disconnected = worker_connection
                    .lock()
                    .expect("shared stream lock")
                    .disconnect();
                if disconnected {
                    worker_runtime_wake.notify();
                }
                if worker_stopping.is_requested() {
                    break;
                }
                loop {
                    if worker_stopping.is_requested() {
                        break 'reconnector;
                    }
                    let viewport = *worker_remote_viewport.lock().expect("remote viewport lock");
                    let session = connect_stream(addr).and_then(|stream| {
                        negotiate(stream, addr, viewport, pixel_formats, transport)
                    });
                    match session {
                        Ok((new_stream, encodings)) => {
                            let fresh_reader = new_stream.try_clone().expect("fresh clone");
                            {
                                let mut guard =
                                    worker_connection.lock().expect("shared stream lock");
                                if worker_stopping.is_requested() {
                                    let _ = new_stream.shutdown(Shutdown::Both);
                                    break 'reconnector;
                                }
                                guard.reconnect(new_stream, encodings.is_some());
                            }
                            worker_runtime_wake.notify();
                            active_reader = Some(spawn_reader(
                                fresh_reader,
                                worker_frames.clone(),
                                worker_wake_tx.clone(),
                                pixel_formats,
                                encodings,
                            ));
                            break;
                        }
                        Err(error) => {
                            eprintln!("PaperSpoon reconnect: {error:#}");
                            if !worker_stopping.wait_retry(STARTUP_RETRY_INTERVAL) {
                                break 'reconnector;
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            connection,
            remote_viewport,
            frames,
            outbound_tx: Some(outbound_tx),
            wake_tx: Some(wake_tx),
            stopping,
            writer: Some(writer),
            reconnector: Some(reconnector),
            startup_rx: None,
            pixel_formats,
            transport,
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
            StartupUpdate::Connected {
                addr,
                mut stream,
                encodings,
            } => {
                let remote_viewport = *self.remote_viewport.lock().expect("remote viewport lock");
                if self.transport == v3::ClientMode::Legacy
                    && let Err(error) =
                        write_hello(&mut stream, remote_viewport, self.pixel_formats)
                {
                    self.startup_rx.take();
                    eprintln!("transport error at startup: {error:#}");
                    return;
                }
                match Self::from_negotiated_stream(
                    addr,
                    stream,
                    self.frames.clone(),
                    remote_viewport,
                    self.pixel_formats,
                    self.transport,
                    encodings,
                ) {
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
    #[cfg(test)]
    pub fn send_pointer(&mut self, phase: V2PointerPhase, x: u16, y: u16) -> Result<()> {
        let encoded = V2Pointer::new(phase, x, y).encode_message()?;
        self.enqueue_bytes(encoded, "pointer event", None)
    }

    /// Queue one traced viewport-relative pointer for latency correlation.
    pub fn send_traced_pointer(
        &mut self,
        phase: V2PointerPhase,
        x: u16,
        y: u16,
        trace: V2PointerTrace,
    ) -> Result<()> {
        let encoded = V2Pointer::new_traced(phase, x, y, trace).encode_message()?;
        self.enqueue_bytes(encoded, "traced pointer event", Some(trace))
    }

    fn enqueue_bytes(
        &mut self,
        mut bytes: Vec<u8>,
        description: &'static str,
        trace: Option<V2PointerTrace>,
    ) -> Result<()> {
        self.promote_startup();
        let (connection, uses_v3) = self
            .connection
            .lock()
            .expect("shared stream lock")
            .current_session()?;
        if uses_v3 {
            v3::encode_control(&mut bytes)?;
        }
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
                trace,
            },
        )
    }

    /// Update dimensions that inbound remote frames must match.
    pub(crate) fn set_remote_viewport(&self, viewport: (u16, u16)) {
        *self.remote_viewport.lock().expect("remote viewport lock") = viewport;
        self.frames.set_viewport(viewport);
    }

    /// Drain the latest validated frame without blocking on network I/O.
    pub(crate) fn poll_frame(&mut self) -> Option<ReceivedFrame> {
        self.promote_startup();
        self.frames.take()
    }

    pub(crate) fn is_connected(&mut self) -> bool {
        self.promote_startup();
        self.connection
            .lock()
            .expect("shared stream lock")
            .is_connected()
    }
}

impl Drop for Paperspoon {
    fn drop(&mut self) {
        self.stopping.request();
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
    #[cfg(target_os = "linux")]
    use crate::runtime_wait::RuntimeWait;
    use std::io::Cursor;
    #[cfg(target_os = "linux")]
    use std::os::fd::AsRawFd;
    #[cfg(target_os = "linux")]
    use std::os::unix::net::UnixStream;

    const TEST_VIEWPORT: (u16, u16) = (1272, 1624);

    #[test]
    fn v3_reader_reconstructs_and_transfers_owned_pixels_to_the_mailbox() {
        let viewport = (129, 17);
        let formats = V2PixelFormats::MONO1.with(V2PixelFormat::Gray8);
        let mailbox = FrameMailbox::default();
        mailbox.set_viewport(viewport);
        let first = paper_protocol::Gray8Frame::new(129, 17, vec![93; 129 * 17]).unwrap();
        let second = paper_protocol::Mono1Frame::new(129, 17, vec![0; 17 * 17]).unwrap();
        let mut encoder = paper_protocol::compression::ZstdEncoder::new(1).unwrap();
        let mut compressed = Vec::new();
        assert!(
            v3::compressed_frame(1, &first, &mut encoder, &mut Vec::new(), &mut compressed)
                .unwrap()
        );
        let mut raw = Vec::new();
        v3::raw_frame(2, &second, &mut raw).unwrap();
        compressed.extend_from_slice(&raw);
        let mut reader = Cursor::new(compressed);
        let mut decoder = v3::FrameDecoder::default();
        for (index, pixels) in [(1, first.pixels()), (2, second.pixels())] {
            let frame = read_v3_frame(
                &mut reader,
                &mailbox,
                formats,
                v3::Encodings::RAW_ZSTD,
                &mut decoder,
            )
            .unwrap()
            .unwrap();
            assert_eq!(frame.frame_id(), index);
            assert_eq!(frame.pixels(), pixels);
            mailbox.publish(frame).unwrap();
        }
        assert_eq!(mailbox.take().unwrap().pixels(), second.pixels());
        assert!(
            read_v3_frame(
                &mut reader,
                &mailbox,
                formats,
                v3::Encodings::RAW_ZSTD,
                &mut decoder
            )
            .unwrap()
            .is_none()
        );
    }

    fn received_frame(frame_id: u64, width: u16, height: u16, pixels: Vec<u8>) -> ReceivedFrame {
        let frame = paper_protocol::Mono1Frame::new(width, height, pixels).expect("valid Mono1");
        let encoded = paper_protocol::encode_v2_frame(frame_id, &frame).expect("encode Frame");
        read_v2_frame(&mut Cursor::new(encoded)).expect("decode Frame")
    }

    #[test]
    fn startup_retry_wait_is_interruptible_without_polling() {
        let stopping = Arc::new(StopSignal::default());
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let (result_tx, result_rx) = mpsc::channel();
        let worker_stopping = Arc::clone(&stopping);
        let worker_barrier = Arc::clone(&barrier);
        let worker = std::thread::spawn(move || {
            worker_barrier.wait();
            result_tx
                .send(worker_stopping.wait_retry(Duration::from_secs(10)))
                .expect("send retry result");
        });

        barrier.wait();
        stopping.request();
        assert!(
            !result_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("startup retry did not wake")
        );
        worker.join().expect("startup retry worker panicked");
    }

    #[test]
    fn startup_retry_wait_reaches_real_deadline() {
        let stopping = StopSignal::default();
        let started = Instant::now();
        assert!(stopping.wait_retry(Duration::from_millis(10)));
        assert!(started.elapsed() >= Duration::from_millis(10));
    }

    #[cfg(target_os = "linux")]
    fn runtime_wake_pair() -> (UnixStream, UnixStream, RuntimeWait, RuntimeWake) {
        let (input, input_guard) = UnixStream::pair().expect("create inert input pair");
        let (waiter, notifier) =
            RuntimeWait::new(input.as_raw_fd()).expect("create runtime waiter");
        (input, input_guard, waiter, RuntimeWake::new(notifier))
    }

    #[cfg(target_os = "linux")]
    fn wait_for_runtime_notification(waiter: &RuntimeWait) {
        let ready = waiter
            .wait(Some(Instant::now() + Duration::from_secs(2)))
            .expect("wait for runtime notification");
        assert!(ready.notified, "runtime notification did not arrive");
        assert!(!ready.input.readable, "inert input unexpectedly readable");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn validated_frame_publication_wakes_and_preserves_latest_only_mailbox() {
        let (_input, _input_guard, waiter, runtime_wake) = runtime_wake_pair();
        let mailbox = FrameMailbox::with_runtime_wake(runtime_wake);
        mailbox.set_viewport((9, 2));

        mailbox
            .publish(received_frame(1, 9, 2, vec![0x11, 0x80, 0x22, 0x00]))
            .expect("publish first frame");
        mailbox
            .publish(received_frame(2, 9, 2, vec![0x44, 0x80, 0x55, 0x00]))
            .expect("publish replacement frame");

        wait_for_runtime_notification(&waiter);
        assert_eq!(mailbox.take().expect("latest frame").frame_id(), 2);
        let after_drain = waiter
            .wait(Some(Instant::now() + Duration::from_millis(10)))
            .expect("wait after coalesced frame wake");
        assert!(after_drain.deadline_elapsed);
        assert!(!after_drain.notified);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejected_frame_does_not_wake_runtime() {
        let (_input, _input_guard, waiter, runtime_wake) = runtime_wake_pair();
        let mailbox = FrameMailbox::with_runtime_wake(runtime_wake);
        mailbox.set_viewport((9, 2));

        mailbox
            .publish(received_frame(3, 8, 2, vec![0x33; 2]))
            .expect_err("mismatched frame must be rejected");
        let ready = waiter
            .wait(Some(Instant::now() + Duration::from_millis(10)))
            .expect("wait after rejected frame");
        assert!(ready.deadline_elapsed);
        assert!(!ready.notified);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn terminal_startup_failure_wakes_before_main_thread_promotion() {
        let (_input, _input_guard, waiter, runtime_wake) = runtime_wake_pair();
        let mut paperspoon = Paperspoon::start_with_pixel_formats(
            || Err(anyhow::anyhow!("injected terminal startup failure")),
            None,
            TEST_VIEWPORT,
            V2PixelFormats::MONO1,
            runtime_wake,
        );

        wait_for_runtime_notification(&waiter);
        assert!(paperspoon.startup_rx.is_some());
        paperspoon.promote_startup();
        assert!(paperspoon.startup_rx.is_none());
        assert!(!paperspoon.is_connected());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn startup_success_wakes_before_connection_state_is_promoted() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let addr = listener.local_addr().expect("listener address");
        let (_input, _input_guard, waiter, runtime_wake) = runtime_wake_pair();
        let mut paperspoon = Paperspoon::start_with_pixel_formats(
            move || connect_stream(addr).map(|stream| (addr, stream)),
            None,
            TEST_VIEWPORT,
            V2PixelFormats::MONO1,
            runtime_wake,
        );

        wait_for_runtime_notification(&waiter);
        assert!(paperspoon.startup_rx.is_some());
        let mut peer = accept_before(&listener, Duration::from_secs(2));
        assert!(paperspoon.is_connected());
        read_expected_hello(&mut peer, TEST_VIEWPORT);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reader_disconnect_and_reconnect_each_wake_runtime() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let addr = listener.local_addr().expect("listener address");
        let (_input, _input_guard, waiter, runtime_wake) = runtime_wake_pair();
        let stream = connect_stream(addr).expect("connect initial stream");
        let mut paperspoon = Paperspoon::from_stream(
            addr,
            stream,
            FrameMailbox::with_runtime_wake(runtime_wake),
            TEST_VIEWPORT,
            V2PixelFormats::MONO1,
        )
        .expect("start notified transport");
        let mut first_peer = accept_before(&listener, Duration::from_secs(2));
        read_expected_hello(&mut first_peer, TEST_VIEWPORT);

        drop(listener);
        drop(first_peer);
        wait_for_runtime_notification(&waiter);
        assert!(!paperspoon.is_connected());

        let replacement_listener =
            std::net::TcpListener::bind(addr).expect("rebind listener for reconnect");
        let mut replacement_peer = accept_before(&replacement_listener, Duration::from_secs(5));
        read_expected_hello(&mut replacement_peer, TEST_VIEWPORT);
        wait_for_runtime_notification(&waiter);
        assert!(paperspoon.is_connected());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn writer_failure_wakes_runtime_and_disconnects_current_token() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let addr = listener.local_addr().expect("listener address");
        let stream = connect_stream(addr).expect("connect stream");
        let _peer = accept_before(&listener, Duration::from_secs(2));
        let connection = Arc::new(Mutex::new(ConnectionState::connected(stream, false)));
        let (connection_token, shutdown_stream) = {
            let guard = connection.lock().expect("connection lock");
            (
                guard.current_session().expect("current token").0,
                guard
                    .clone_stream()
                    .expect("clone stream for shutdown")
                    .into_stream(),
            )
        };
        shutdown_stream
            .shutdown(Shutdown::Both)
            .expect("shut down current socket");
        let (_input, _input_guard, waiter, runtime_wake) = runtime_wake_pair();
        let outbound = QueuedOutbound {
            connection: connection_token,
            bytes: vec![1],
            description: "injected writer failure",
            trace: None,
        };

        write_outbound(&connection, &outbound, &runtime_wake)
            .expect_err("write on shut-down socket must fail");
        wait_for_runtime_notification(&waiter);
        assert!(!connection.lock().expect("connection lock").is_connected());
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

    fn read_expected_hello(peer: &mut TcpStream, expected: (u16, u16)) {
        read_expected_hello_with_formats(peer, expected, V2PixelFormats::MONO1);
    }

    fn read_expected_hello_with_formats(
        peer: &mut TcpStream,
        expected: (u16, u16),
        pixel_formats: V2PixelFormats,
    ) {
        let message_len = V2_HEADER_LEN + paper_protocol::V2_HELLO_PAYLOAD_LEN;
        let mut encoded = vec![0; message_len];
        peer.read_exact(&mut encoded).expect("read Hello message");
        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(&encoded).expect("decode Hello message")
        else {
            panic!("complete Hello message expected");
        };
        assert_eq!(consumed, message_len);
        assert_eq!(
            decode_v2_payload(message),
            Ok(V2Payload::Hello(V2Hello::with_pixel_formats(
                expected.0,
                expected.1,
                pixel_formats,
            )))
        );
    }

    #[test]
    fn hello_advertises_configured_pixel_formats() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let mut client = TcpStream::connect(listener.local_addr().expect("listener address"))
            .expect("connect client");
        let mut peer = accept_before(&listener, Duration::from_secs(2));
        let pixel_formats = V2PixelFormats::MONO1.with(V2PixelFormat::Gray8);

        write_hello(&mut client, TEST_VIEWPORT, pixel_formats).expect("write Hello");
        read_expected_hello_with_formats(&mut peer, TEST_VIEWPORT, pixel_formats);
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
            TEST_VIEWPORT,
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
            TEST_VIEWPORT,
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
            TEST_VIEWPORT,
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
        read_expected_hello(&mut peer, TEST_VIEWPORT);
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
            TEST_VIEWPORT,
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
        read_expected_hello(&mut peer, TEST_VIEWPORT);
        read_expected_pointer(&mut peer, V2Pointer::new(V2PointerPhase::Up, 321, 654));
        assert_eq!(attempts.load(Ordering::Acquire), 2);
    }

    #[test]
    fn background_start_failure_remains_disconnected() {
        let mut paperspoon = Paperspoon::start_with(
            || Err(anyhow::anyhow!("injected startup failure")),
            None,
            TEST_VIEWPORT,
        );
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

    fn assert_transport_write_timeout_is_bounded(paperspoon: &Paperspoon) {
        let actual = transport_write_timeout(paperspoon).expect("configured write timeout");
        let rounded_upper_bound = TCP_WRITE_TIMEOUT + Duration::from_millis(20);
        assert!(
            (TCP_WRITE_TIMEOUT..=rounded_upper_bound).contains(&actual),
            "configured write timeout {actual:?} is outside {TCP_WRITE_TIMEOUT:?}..={rounded_upper_bound:?}"
        );
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
            Paperspoon::connect_to(listener.local_addr().expect("addr"), TEST_VIEWPORT)
                .expect("connect");
        let _peer = accept_before(&listener, Duration::from_secs(2));
        assert_transport_write_timeout_is_bounded(&paperspoon);
    }

    #[test]
    fn writer_delivers_ordered_binary_pointer_phases() {
        use std::io::Read as _;

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let mut paperspoon =
            Paperspoon::connect_to(listener.local_addr().expect("addr"), TEST_VIEWPORT)
                .expect("connect");
        let mut peer = accept_before(&listener, Duration::from_secs(2));
        peer.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set peer timeout");
        read_expected_hello(&mut peer, TEST_VIEWPORT);

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
    fn writer_delivers_traced_pointer_correlation() {
        use std::io::Read as _;

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let mut paperspoon =
            Paperspoon::connect_to(listener.local_addr().expect("addr"), TEST_VIEWPORT)
                .expect("connect");
        let mut peer = accept_before(&listener, Duration::from_secs(2));
        peer.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set peer timeout");
        read_expected_hello(&mut peer, TEST_VIEWPORT);

        let trace = V2PointerTrace::new(17, 23, 42);
        let expected = V2Pointer::new_traced(V2PointerPhase::Down, 123, 456, trace);
        paperspoon
            .send_traced_pointer(expected.phase(), expected.x(), expected.y(), trace)
            .expect("queue traced pointer");

        let message_len = V2_HEADER_LEN + paper_protocol::V2_TRACED_POINTER_PAYLOAD_LEN;
        let mut encoded = vec![0; message_len];
        peer.read_exact(&mut encoded).expect("read traced pointer");
        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(&encoded).expect("decode traced pointer")
        else {
            panic!("complete traced pointer expected");
        };
        assert_eq!(consumed, message_len);
        assert_eq!(decode_v2_payload(message), Ok(V2Payload::Pointer(expected)));
    }

    #[test]
    fn reader_publishes_valid_frame_from_tcp_stream() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let mut paperspoon =
            Paperspoon::connect_to(listener.local_addr().expect("addr"), TEST_VIEWPORT)
                .expect("connect");
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
        assert_eq!(received.pixel_format(), V2PixelFormat::Mono1);
        assert_eq!(received.stride(), 2);
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
        let second = paper_protocol::Gray8Frame::new(8, 1, vec![0, 32, 64, 96, 128, 160, 192, 255])
            .expect("valid second Gray8");
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
        assert_eq!(frame.pixel_format(), V2PixelFormat::Gray8);
        assert_eq!((frame.width(), frame.height(), frame.stride()), (8, 1, 8));
        assert_eq!(frame.pixels(), &[0, 32, 64, 96, 128, 160, 192, 255]);
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
        let mut paperspoon = Paperspoon::connect_to(addr, TEST_VIEWPORT).expect("connect");
        let mut first_peer = accept_before(&listener, Duration::from_secs(2));

        first_peer
            .write_all(b"legacy line protocol\n")
            .expect("write legacy line");

        // Keep the original peer open: a second accept proves the reader
        // rejected the frame and woke the reconnector instead of seeing EOF.
        let mut replacement_peer = accept_before(&listener, Duration::from_secs(5));
        read_expected_hello(&mut replacement_peer, TEST_VIEWPORT);
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
        let pixel_formats = V2PixelFormats::MONO1.with(V2PixelFormat::Gray8);
        let mut paperspoon =
            Paperspoon::connect_to_with_pixel_formats(addr, TEST_VIEWPORT, pixel_formats)
                .expect("connect");
        let mut first = accept_before(&listener, Duration::from_secs(2));
        read_expected_hello_with_formats(&mut first, TEST_VIEWPORT, pixel_formats);
        drop(first);
        drop(listener); // Force EOF: the reconnector must notice.

        let disconnect_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while paperspoon.is_connected() {
            assert!(
                std::time::Instant::now() < disconnect_deadline,
                "transport did not publish disconnected state"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        // A new PaperSpoon appears; the reconnector should restore the stream
        // without any outbound pointer call from us. The reconnector loops forever
        // retrying `addr` (now free), so rebinding the SAME port and
        // accepting proves the auto-restore.
        let listener3 = TcpListener::bind(addr).expect("rebind same port");
        let mut reconnected_peer = accept_before(&listener3, Duration::from_secs(5));
        read_expected_hello_with_formats(&mut reconnected_peer, TEST_VIEWPORT, pixel_formats);
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
        assert_transport_write_timeout_is_bounded(&paperspoon);
        reconnected_peer
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set peer deadline");
        drop(paperspoon);
        let mut byte = [0];
        assert_eq!(reconnected_peer.read(&mut byte).expect("read EOF"), 0);
    }

    #[test]
    fn v3_paced_stream_keeps_only_the_latest_frame_for_a_stalled_display() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let formats = V2PixelFormats::GRAY8;
        let mut paperspoon = Paperspoon::start_with_transport(
            move || connect_stream(addr).map(|stream| (addr, stream)),
            None,
            (64, 64),
            formats,
            RuntimeWake::default(),
            v3::ClientMode::Auto,
        );
        let mut peer = accept_before(&listener, Duration::from_secs(3));
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        peer.set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut hello = [0; v3::HEADER_LEN + 8];
        peer.read_exact(&mut hello).unwrap();
        let header = v3::Header::decode(hello[..v3::HEADER_LEN].try_into().unwrap()).unwrap();
        assert_eq!(header.kind, v3::Kind::Hello);
        let hello = v3::Hello::decode(&hello[v3::HEADER_LEN..]).unwrap();
        assert_eq!(hello.encodings, v3::Encodings::RAW_ZSTD);
        peer.write_all(&v3::encode_ack(v3::Encodings::RAW_ZSTD))
            .unwrap();

        let first = paper_protocol::Gray8Frame::new(64, 64, vec![42; 4096]).unwrap();
        let latest = paper_protocol::Gray8Frame::new(64, 64, vec![176; 4096]).unwrap();
        let mut encoder = paper_protocol::compression::ZstdEncoder::new(1).unwrap();
        let mut message = Vec::new();
        let mut scratch = Vec::new();
        assert!(v3::compressed_frame(1, &first, &mut encoder, &mut scratch, &mut message).unwrap());

        // Keep the first frame incomplete while the network reader runs.
        for byte in &message[..message.len() - 1] {
            peer.write_all(std::slice::from_ref(byte)).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(paperspoon.poll_frame().is_none());
        peer.write_all(&message[message.len() - 1..]).unwrap();

        // Leave display consumption paused throughout the burst.
        const FINAL_ID: u64 = 1024;
        for id in 2..=FINAL_ID {
            assert!(
                v3::compressed_frame(id, &latest, &mut encoder, &mut scratch, &mut message)
                    .unwrap()
            );
            peer.write_all(&message).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if paperspoon
                .frames
                .state
                .lock()
                .unwrap()
                .pending
                .as_ref()
                .map(ReceivedFrame::frame_id)
                == Some(FINAL_ID)
            {
                break;
            }
            assert!(Instant::now() < deadline, "the latest frame did not arrive");
            std::thread::sleep(Duration::from_millis(5));
        }
        let frame = paperspoon.poll_frame().unwrap();
        assert_eq!(frame.frame_id(), FINAL_ID);
        assert_eq!(frame.pixels(), latest.pixels());
        assert!(paperspoon.poll_frame().is_none());
        assert!(paperspoon.is_connected());
    }

    #[test]
    fn default_transport_retries_legacy_and_upgrades_after_reconnect() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let formats = V2PixelFormats::MONO1;
        let mut paperspoon = Paperspoon::start_with_transport(
            move || connect_stream(addr).map(|stream| (addr, stream)),
            None,
            TEST_VIEWPORT,
            formats,
            RuntimeWake::default(),
            v3::ClientMode::default(),
        );
        let accept = || {
            let peer = accept_before(&listener, Duration::from_secs(3));
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            peer
        };
        let read_v3_hello = |peer: &mut TcpStream| {
            let mut message = [0; v3::HEADER_LEN + 8];
            peer.read_exact(&mut message).unwrap();
            let header = v3::Header::decode(message[..v3::HEADER_LEN].try_into().unwrap()).unwrap();
            assert_eq!(header.kind, v3::Kind::Hello);
            assert_eq!(
                v3::Hello::decode(&message[v3::HEADER_LEN..])
                    .unwrap()
                    .encodings,
                v3::Encodings::RAW_ZSTD
            );
        };
        let send_pointer = |paperspoon: &mut Paperspoon| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if paperspoon.send_pointer(V2PointerPhase::Down, 7, 8).is_ok() {
                    break;
                }
                assert!(Instant::now() < deadline, "session did not accept input");
                std::thread::sleep(Duration::from_millis(5));
            }
        };

        let mut probe = accept();
        read_v3_hello(&mut probe);
        drop(probe);
        let mut legacy = accept();
        read_expected_hello(&mut legacy, TEST_VIEWPORT);
        send_pointer(&mut paperspoon);
        let pointer = V2Pointer::new(V2PointerPhase::Down, 7, 8);
        read_expected_pointer(&mut legacy, pointer);
        drop(legacy);

        let mut upgraded = accept();
        read_v3_hello(&mut upgraded);
        upgraded
            .write_all(&v3::encode_ack(v3::Encodings::RAW_ZSTD))
            .unwrap();
        send_pointer(&mut paperspoon);
        let mut control = [0; v3::HEADER_LEN + paper_protocol::V2_POINTER_PAYLOAD_LEN];
        upgraded.read_exact(&mut control).unwrap();
        assert_eq!(
            v3::decode_control(&mut control).unwrap(),
            V2Payload::Pointer(pointer)
        );
    }

    #[test]
    fn v3_reconnects_after_a_partial_frame_and_renegotiates_raw() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let formats = V2PixelFormats::GRAY8;
        let dimensions = (64, 64);
        let mut paperspoon = Paperspoon::start_with_transport(
            move || connect_stream(addr).map(|stream| (addr, stream)),
            None,
            dimensions,
            formats,
            RuntimeWake::default(),
            v3::ClientMode::Auto,
        );
        let accept = |encodings| {
            let mut peer = accept_before(&listener, Duration::from_secs(3));
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut hello = [0; v3::HEADER_LEN + 8];
            peer.read_exact(&mut hello).unwrap();
            let header = v3::Header::decode(hello[..v3::HEADER_LEN].try_into().unwrap()).unwrap();
            assert_eq!(header.kind, v3::Kind::Hello);
            let hello = v3::Hello::decode(&hello[v3::HEADER_LEN..]).unwrap();
            assert_eq!(
                (hello.width, hello.height, hello.formats),
                (64, 64, formats)
            );
            assert_eq!(hello.encodings, v3::Encodings::RAW_ZSTD);
            peer.write_all(&v3::encode_ack(encodings)).unwrap();
            peer
        };
        let take_frame = |paperspoon: &mut Paperspoon| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(frame) = paperspoon.poll_frame() {
                    return frame;
                }
                assert!(Instant::now() < deadline, "frame did not arrive");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let frame = paper_protocol::Gray8Frame::new(64, 64, vec![98; 4096]).unwrap();
        let mut encoder = paper_protocol::compression::ZstdEncoder::new(1).unwrap();
        let mut message = Vec::new();
        let mut first = accept(v3::Encodings::RAW_ZSTD);
        assert!(
            v3::compressed_frame(1, &frame, &mut encoder, &mut Vec::new(), &mut message).unwrap()
        );
        first.write_all(&message).unwrap();
        let valid = take_frame(&mut paperspoon);
        assert_eq!((valid.frame_id(), valid.pixels()), (1, frame.pixels()));

        assert!(
            v3::compressed_frame(2, &frame, &mut encoder, &mut Vec::new(), &mut message).unwrap()
        );
        first.write_all(&message[..message.len() - 1]).unwrap();
        first.shutdown(Shutdown::Write).unwrap();

        let mut second = accept(v3::Encodings::RAW);
        let replacement = paper_protocol::Gray8Frame::new(64, 64, vec![128; 4096]).unwrap();
        v3::raw_frame(3, &replacement, &mut message).unwrap();
        second.write_all(&message).unwrap();
        let received = take_frame(&mut paperspoon);
        assert_eq!(
            (received.frame_id(), received.pixels()),
            (3, replacement.pixels())
        );
        assert_eq!(valid.pixels(), frame.pixels());
        paperspoon.send_pointer(V2PointerPhase::Down, 7, 8).unwrap();
        let mut control = [0; v3::HEADER_LEN + paper_protocol::V2_POINTER_PAYLOAD_LEN];
        second.read_exact(&mut control).unwrap();
        assert_eq!(
            v3::decode_control(&mut control).unwrap(),
            V2Payload::Pointer(V2Pointer::new(V2PointerPhase::Down, 7, 8))
        );
    }
}
