use std::fmt;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use paper_protocol::{V2EncodeError, V2Pointer, V2PointerPhase};

use crate::connection::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_WRITE_TIMEOUT, connect_and_send_hello, validate_inputs,
};
use crate::receiver::{FrameReadError, ReadPhase, ReceivedFrame, read_next_frame};

pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_millis(250);
pub const DEFAULT_RECONNECT_DELAY: Duration = Duration::from_millis(500);
const POINTER_QUEUE_CAPACITY: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerConfig {
    pub host: String,
    pub port: u16,
    pub viewport_width: u16,
    pub viewport_height: u16,
    pub connect_timeout: Duration,
    pub write_timeout: Duration,
    pub read_timeout: Duration,
    pub reconnect_delay: Duration,
}

impl WorkerConfig {
    pub fn new(host: String, port: u16, viewport_width: u16, viewport_height: u16) -> Self {
        Self {
            host,
            port,
            viewport_width,
            viewport_height,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            write_timeout: DEFAULT_WRITE_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
            reconnect_delay: DEFAULT_RECONNECT_DELAY,
        }
    }

    fn validate(&self) -> Result<(), WorkerStartError> {
        validate_inputs(
            &self.host,
            self.port,
            self.viewport_width,
            self.viewport_height,
            self.connect_timeout,
            self.write_timeout,
        )
        .map_err(|error| WorkerStartError::InvalidConfig(error.to_string()))?;
        if self.read_timeout.is_zero() {
            return Err(WorkerStartError::InvalidConfig(
                "read timeout must be nonzero".to_string(),
            ));
        }
        if self.reconnect_delay.is_zero() {
            return Err(WorkerStartError::InvalidConfig(
                "reconnect delay must be nonzero".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionStatus {
    Connecting,
    Connected,
    Disconnected,
    Error(String),
    Stopped,
}

#[derive(Debug)]
pub enum WorkerStartError {
    InvalidConfig(String),
    Spawn(io::Error),
}

impl fmt::Display for WorkerStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) => write!(formatter, "invalid worker config: {message}"),
            Self::Spawn(error) => write!(formatter, "could not spawn network worker: {error}"),
        }
    }
}

impl std::error::Error for WorkerStartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfig(_) => None,
            Self::Spawn(error) => Some(error),
        }
    }
}

#[derive(Debug)]
pub enum PointerSendError {
    NotConnected,
    QueueFull,
    WorkerStopped,
    Encode(V2EncodeError),
}

impl fmt::Display for PointerSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConnected => formatter.write_str("PaperSpoon is not connected"),
            Self::QueueFull => formatter.write_str("pointer queue is full"),
            Self::WorkerStopped => formatter.write_str("network worker has stopped"),
            Self::Encode(error) => write!(formatter, "could not encode PPFB pointer: {error}"),
        }
    }
}

impl std::error::Error for PointerSendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode(error) => Some(error),
            Self::NotConnected | Self::QueueFull | Self::WorkerStopped => None,
        }
    }
}

#[derive(Debug)]
struct PointerCommand {
    epoch: u64,
    encoded: Vec<u8>,
}

#[derive(Debug)]
pub struct WorkerUpdates {
    pub status: Option<ConnectionStatus>,
    pub frame: Option<ReceivedFrame>,
}

#[derive(Clone)]
struct SharedUpdates {
    inner: Arc<Mutex<SharedInner>>,
}

struct SharedInner {
    wake_pending: bool,
    status: ConnectionStatus,
    status_changed: bool,
    frame: Option<ReceivedFrame>,
}

impl SharedUpdates {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SharedInner {
                wake_pending: false,
                status: ConnectionStatus::Stopped,
                status_changed: false,
                frame: None,
            })),
        }
    }

    fn take(&self) -> WorkerUpdates {
        let mut inner = self.inner.lock().expect("worker updates lock");
        let status = inner.status_changed.then(|| inner.status.clone());
        inner.status_changed = false;
        let frame = inner.frame.take();
        inner.wake_pending = false;
        WorkerUpdates { status, frame }
    }
}

#[derive(Clone)]
struct StopSignal {
    inner: Arc<StopInner>,
}

struct StopInner {
    requested: Mutex<bool>,
    changed: Condvar,
}

impl StopSignal {
    fn new() -> Self {
        Self {
            inner: Arc::new(StopInner {
                requested: Mutex::new(false),
                changed: Condvar::new(),
            }),
        }
    }

    fn request(&self) {
        *self.inner.requested.lock().expect("worker stop lock") = true;
        self.inner.changed.notify_all();
    }

    fn is_requested(&self) -> bool {
        *self.inner.requested.lock().expect("worker stop lock")
    }

    fn wait(&self, duration: Duration) -> bool {
        let requested = self.inner.requested.lock().expect("worker stop lock");
        if *requested {
            return true;
        }
        let (requested, _) = self
            .inner
            .changed
            .wait_timeout(requested, duration)
            .expect("worker stop wait");
        *requested
    }
}

struct Publisher {
    shared: SharedUpdates,
    stop: StopSignal,
    notify: Box<dyn Fn() -> bool + Send>,
}

impl Publisher {
    fn publish_status(&self, status: ConnectionStatus) -> bool {
        self.publish(|inner| {
            if inner.status == status {
                return false;
            }
            inner.status = status;
            inner.status_changed = true;
            true
        })
    }

    fn publish_frame(&self, frame: ReceivedFrame) -> bool {
        self.publish(|inner| {
            inner.frame = Some(frame);
            true
        })
    }

    fn publish(&self, update: impl FnOnce(&mut SharedInner) -> bool) -> bool {
        let should_notify = {
            let mut inner = self.shared.inner.lock().expect("worker updates lock");
            if !update(&mut inner) || inner.wake_pending {
                false
            } else {
                inner.wake_pending = true;
                true
            }
        };
        if should_notify && !(self.notify)() {
            self.stop.request();
            return false;
        }
        !self.stop.is_requested()
    }
}

pub struct WorkerHandle {
    shared: SharedUpdates,
    stop: StopSignal,
    pointer_sender: SyncSender<PointerCommand>,
    active_epoch: Arc<AtomicU64>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    pub fn take_updates(&self) -> WorkerUpdates {
        self.shared.take()
    }

    pub fn send_pointer(
        &self,
        phase: V2PointerPhase,
        x: u16,
        y: u16,
    ) -> Result<(), PointerSendError> {
        let epoch = self.active_epoch.load(Ordering::Acquire);
        if epoch == 0 {
            return Err(PointerSendError::NotConnected);
        }
        let encoded = V2Pointer::new(phase, x, y)
            .encode_message()
            .map_err(PointerSendError::Encode)?;
        self.pointer_sender
            .try_send(PointerCommand { epoch, encoded })
            .map_err(|error| match error {
                TrySendError::Full(_) => PointerSendError::QueueFull,
                TrySendError::Disconnected(_) => PointerSendError::WorkerStopped,
            })
    }

    pub fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        self.stop.request();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

pub fn start_worker(
    config: WorkerConfig,
    notify: impl Fn() -> bool + Send + 'static,
) -> Result<WorkerHandle, WorkerStartError> {
    config.validate()?;
    let shared = SharedUpdates::new();
    let stop = StopSignal::new();
    let active_epoch = Arc::new(AtomicU64::new(0));
    let (pointer_sender, pointer_receiver) = mpsc::sync_channel(POINTER_QUEUE_CAPACITY);
    let publisher = Publisher {
        shared: shared.clone(),
        stop: stop.clone(),
        notify: Box::new(notify),
    };
    let worker_stop = stop.clone();
    let worker_epoch = Arc::clone(&active_epoch);
    let join = thread::Builder::new()
        .name("paperview-network".to_string())
        .spawn(move || {
            run_worker(
                config,
                worker_stop,
                publisher,
                pointer_receiver,
                worker_epoch,
            )
        })
        .map_err(WorkerStartError::Spawn)?;
    Ok(WorkerHandle {
        shared,
        stop,
        pointer_sender,
        active_epoch,
        join: Some(join),
    })
}

fn run_worker(
    config: WorkerConfig,
    stop: StopSignal,
    publisher: Publisher,
    pointer_receiver: Receiver<PointerCommand>,
    active_epoch: Arc<AtomicU64>,
) {
    let mut next_epoch = 1_u64;
    'worker: loop {
        active_epoch.store(0, Ordering::Release);
        if stop.is_requested() || !publisher.publish_status(ConnectionStatus::Connecting) {
            break;
        }
        let mut stream = match connect_and_send_hello(
            &config.host,
            config.port,
            config.viewport_width,
            config.viewport_height,
            config.connect_timeout,
            config.write_timeout,
        ) {
            Ok(stream) => stream,
            Err(error) => {
                if !publisher.publish_status(ConnectionStatus::Error(error.to_string()))
                    || stop.wait(config.reconnect_delay)
                {
                    break;
                }
                continue;
            }
        };
        if let Err(error) = stream.set_read_timeout(Some(config.read_timeout)) {
            if !publisher.publish_status(ConnectionStatus::Error(format!(
                "could not set PaperSpoon read timeout: {error}"
            ))) || stop.wait(config.reconnect_delay)
            {
                break;
            }
            continue;
        }
        let epoch = next_epoch;
        next_epoch = next_epoch.checked_add(1).unwrap_or(1);
        active_epoch.store(epoch, Ordering::Release);
        if !publisher.publish_status(ConnectionStatus::Connected) {
            break;
        }

        let mut reader = StopAwareReader {
            stream: &mut stream,
            stop: &stop,
        };
        loop {
            if let Err(error) = send_pending_pointers(reader.stream, &pointer_receiver, epoch) {
                if !publisher.publish_status(ConnectionStatus::Error(format!(
                    "could not send PPFB pointer: {error}"
                ))) {
                    break 'worker;
                }
                break;
            }
            match read_next_frame(&mut reader, config.viewport_width, config.viewport_height) {
                Ok(Some(frame)) => {
                    if !publisher.publish_frame(frame) {
                        break 'worker;
                    }
                }
                Ok(None) => {
                    if !publisher.publish_status(ConnectionStatus::Disconnected) {
                        break 'worker;
                    }
                    break;
                }
                Err(FrameReadError::Io {
                    phase: ReadPhase::Header,
                    received: 0,
                    source,
                }) if matches!(
                    source.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
                {
                    if stop.is_requested() {
                        break 'worker;
                    }
                }
                Err(FrameReadError::Io { source, .. })
                    if stop.is_requested() && source.kind() == io::ErrorKind::ConnectionAborted =>
                {
                    break 'worker;
                }
                Err(error) => {
                    if !publisher.publish_status(ConnectionStatus::Error(error.to_string())) {
                        break 'worker;
                    }
                    break;
                }
            }
        }
        active_epoch.store(0, Ordering::Release);
        if stop.is_requested() || stop.wait(config.reconnect_delay) {
            break;
        }
    }
    active_epoch.store(0, Ordering::Release);
    let _ = publisher.publish_status(ConnectionStatus::Stopped);
}

fn send_pending_pointers(
    stream: &mut TcpStream,
    pointer_receiver: &Receiver<PointerCommand>,
    epoch: u64,
) -> io::Result<()> {
    loop {
        match pointer_receiver.try_recv() {
            Ok(command) if command.epoch == epoch => stream.write_all(&command.encoded)?,
            Ok(_) => {}
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(()),
        }
    }
}

struct StopAwareReader<'a> {
    stream: &'a mut TcpStream,
    stop: &'a StopSignal,
}

impl Read for StopAwareReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.stop.is_requested() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "PaperView worker is stopping",
            ));
        }
        self.stream.read(buffer)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
    use std::time::Instant;

    use paper_protocol::{
        Gray8Frame, V2_HEADER_LEN, V2DecodeResult, V2Payload, V2PixelFormat, V2PixelFormats,
        decode_v2_message, decode_v2_payload, encode_v2_frame,
    };

    use super::*;

    fn test_frame(frame_id: u64, value: u8) -> ReceivedFrame {
        let frame = Gray8Frame::new(2, 2, vec![value; 4]).expect("valid test frame");
        let encoded = encode_v2_frame(frame_id, &frame).expect("encode test frame");
        read_next_frame(&mut io::Cursor::new(encoded), 2, 2)
            .expect("read test frame")
            .expect("test frame present")
    }

    fn channel_notifier(sender: SyncSender<()>) -> impl Fn() -> bool + Send {
        move || match sender.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => true,
            Err(TrySendError::Disconnected(())) => false,
        }
    }

    fn wait_for_frame(handle: &WorkerHandle, wake: &Receiver<()>, wanted_id: u64) -> ReceivedFrame {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for frame");
            wake.recv_timeout(remaining).expect("worker wake");
            let updates = handle.take_updates();
            if let Some(frame) = updates.frame
                && frame.frame_id() == wanted_id
            {
                return frame;
            }
        }
    }

    fn wait_for_status(handle: &WorkerHandle, wake: &Receiver<()>, wanted: &ConnectionStatus) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for status");
            wake.recv_timeout(remaining).expect("worker wake");
            let updates = handle.take_updates();
            if updates.status.as_ref() == Some(wanted) {
                return;
            }
        }
    }

    fn read_hello(peer: &mut TcpStream) -> paper_protocol::V2Hello {
        let mut encoded = vec![0; V2_HEADER_LEN];
        peer.read_exact(&mut encoded).expect("read Hello header");
        let additional = match decode_v2_message(&encoded).expect("decode Hello header") {
            V2DecodeResult::Incomplete { additional } => additional,
            V2DecodeResult::Complete { .. } => panic!("header-only Hello must be incomplete"),
        };
        encoded.resize(V2_HEADER_LEN + additional, 0);
        peer.read_exact(&mut encoded[V2_HEADER_LEN..])
            .expect("read Hello payload");
        let message = match decode_v2_message(&encoded).expect("decode Hello") {
            V2DecodeResult::Complete { message, .. } => message,
            V2DecodeResult::Incomplete { .. } => panic!("complete Hello expected"),
        };
        let V2Payload::Hello(hello) = decode_v2_payload(message).expect("typed Hello") else {
            panic!("Hello expected");
        };
        hello
    }

    fn read_pointer(peer: &mut TcpStream) -> V2Pointer {
        let mut encoded = vec![0; V2_HEADER_LEN];
        peer.read_exact(&mut encoded).expect("read message header");
        let additional = match decode_v2_message(&encoded).expect("decode message header") {
            V2DecodeResult::Incomplete { additional } => additional,
            V2DecodeResult::Complete { .. } => panic!("header-only message must be incomplete"),
        };
        encoded.resize(V2_HEADER_LEN + additional, 0);
        peer.read_exact(&mut encoded[V2_HEADER_LEN..])
            .expect("read message payload");
        let message = match decode_v2_message(&encoded).expect("decode message") {
            V2DecodeResult::Complete { message, .. } => message,
            V2DecodeResult::Incomplete { .. } => panic!("complete message expected"),
        };
        let payload = decode_v2_payload(message).expect("decode typed payload");
        let V2Payload::Pointer(pointer) = payload else {
            panic!("pointer payload expected");
        };
        pointer
    }

    fn test_config(port: u16, width: u16, height: u16) -> WorkerConfig {
        WorkerConfig {
            host: "127.0.0.1".to_string(),
            port,
            viewport_width: width,
            viewport_height: height,
            connect_timeout: Duration::from_secs(1),
            write_timeout: Duration::from_secs(1),
            read_timeout: Duration::from_millis(25),
            reconnect_delay: Duration::from_millis(10),
        }
    }

    #[test]
    fn pointer_sender_rejects_disconnected_full_and_stopped_states() {
        let shared = SharedUpdates::new();
        let stop = StopSignal::new();
        let active_epoch = Arc::new(AtomicU64::new(0));
        let (pointer_sender, pointer_receiver) = mpsc::sync_channel(1);
        let handle = WorkerHandle {
            shared,
            stop,
            pointer_sender,
            active_epoch: Arc::clone(&active_epoch),
            join: None,
        };

        assert!(matches!(
            handle.send_pointer(V2PointerPhase::Down, 1, 2),
            Err(PointerSendError::NotConnected)
        ));

        active_epoch.store(7, Ordering::Release);
        handle
            .send_pointer(V2PointerPhase::Down, 1, 2)
            .expect("first pointer fits queue");
        assert!(matches!(
            handle.send_pointer(V2PointerPhase::Up, 1, 2),
            Err(PointerSendError::QueueFull)
        ));

        drop(pointer_receiver);
        assert!(matches!(
            handle.send_pointer(V2PointerPhase::Up, 1, 2),
            Err(PointerSendError::WorkerStopped)
        ));
    }

    #[test]
    fn active_session_forwards_pointer_messages_in_order() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let port = listener.local_addr().expect("listener address").port();
        let server = thread::spawn(move || {
            let (mut peer, _) = listener.accept().expect("accept PaperView");
            let hello = read_hello(&mut peer);
            let down = read_pointer(&mut peer);
            let up = read_pointer(&mut peer);
            (hello, down, up)
        });

        let (wake_sender, wake_receiver) = mpsc::sync_channel(1);
        let handle = start_worker(test_config(port, 2, 2), channel_notifier(wake_sender))
            .expect("start worker");
        wait_for_status(&handle, &wake_receiver, &ConnectionStatus::Connected);
        handle
            .send_pointer(V2PointerPhase::Down, 1, 0)
            .expect("queue down");
        handle
            .send_pointer(V2PointerPhase::Up, 1, 0)
            .expect("queue up");

        let (hello, down, up) = server.join().expect("join server");
        handle.shutdown();
        assert_eq!((hello.viewport_width(), hello.viewport_height()), (2, 2));
        assert_eq!(down, V2Pointer::new(V2PointerPhase::Down, 1, 0));
        assert_eq!(up, V2Pointer::new(V2PointerPhase::Up, 1, 0));
    }

    #[test]
    fn pointer_drain_discards_commands_from_stale_sessions() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let address = listener.local_addr().expect("listener address");
        let mut writer = TcpStream::connect(address).expect("connect writer");
        let (mut reader, _) = listener.accept().expect("accept reader");
        let (pointer_sender, pointer_receiver) = mpsc::sync_channel(2);
        pointer_sender
            .send(PointerCommand {
                epoch: 1,
                encoded: V2Pointer::new(V2PointerPhase::Down, 0, 0)
                    .encode_message()
                    .expect("encode stale pointer"),
            })
            .expect("queue stale pointer");
        pointer_sender
            .send(PointerCommand {
                epoch: 2,
                encoded: V2Pointer::new(V2PointerPhase::Up, 1, 1)
                    .encode_message()
                    .expect("encode current pointer"),
            })
            .expect("queue current pointer");

        send_pending_pointers(&mut writer, &pointer_receiver, 2).expect("drain pointers");
        assert_eq!(
            read_pointer(&mut reader),
            V2Pointer::new(V2PointerPhase::Up, 1, 1)
        );
        reader
            .set_read_timeout(Some(Duration::from_millis(25)))
            .expect("set read timeout");
        let mut byte = [0_u8; 1];
        let error = reader
            .read_exact(&mut byte)
            .expect_err("stale pointer must not be written");
        assert!(
            matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ),
            "unexpected read error: {error}"
        );
    }

    #[test]
    fn pending_wake_coalesces_status_and_keeps_only_latest_frame() {
        let shared = SharedUpdates::new();
        let stop = StopSignal::new();
        let notification_count = Arc::new(AtomicUsize::new(0));
        let count_for_notifier = Arc::clone(&notification_count);
        let publisher = Publisher {
            shared: shared.clone(),
            stop,
            notify: Box::new(move || {
                count_for_notifier.fetch_add(1, Ordering::SeqCst);
                true
            }),
        };

        assert!(publisher.publish_status(ConnectionStatus::Connecting));
        assert!(publisher.publish_frame(test_frame(1, 10)));
        assert!(publisher.publish_frame(test_frame(2, 20)));
        assert!(publisher.publish_status(ConnectionStatus::Connected));
        assert_eq!(notification_count.load(Ordering::SeqCst), 1);

        let updates = shared.take();
        assert_eq!(updates.status, Some(ConnectionStatus::Connected));
        assert_eq!(updates.frame.expect("latest frame").frame_id(), 2);
        assert!(publisher.publish_status(ConnectionStatus::Disconnected));
        assert_eq!(notification_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn worker_reconnects_after_invalid_session_and_sends_fresh_hello() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback server");
        let port = listener.local_addr().expect("listener address").port();
        let (release_first, wait_first) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().expect("accept first session");
            let first_hello = read_hello(&mut first);
            let first_frame = Gray8Frame::new(2, 2, vec![1; 4]).expect("first frame");
            first
                .write_all(&encode_v2_frame(1, &first_frame).expect("encode first frame"))
                .expect("send first frame");
            wait_first
                .recv_timeout(Duration::from_secs(3))
                .expect("release first session");
            first
                .write_all(
                    &paper_protocol::V2Hello::new(2, 2)
                        .encode_message()
                        .expect("encode unexpected Hello"),
                )
                .expect("send unexpected Hello");
            drop(first);

            let (mut second, _) = listener.accept().expect("accept second session");
            let second_hello = read_hello(&mut second);
            let second_frame = Gray8Frame::new(2, 2, vec![2; 4]).expect("second frame");
            second
                .write_all(&encode_v2_frame(2, &second_frame).expect("encode second frame"))
                .expect("send second frame");
            (first_hello, second_hello)
        });

        let (wake_sender, wake_receiver) = mpsc::sync_channel(1);
        let handle = start_worker(test_config(port, 2, 2), channel_notifier(wake_sender))
            .expect("start worker");
        assert_eq!(wait_for_frame(&handle, &wake_receiver, 1).frame_id(), 1);
        release_first.send(()).expect("continue first session");
        assert_eq!(wait_for_frame(&handle, &wake_receiver, 2).frame_id(), 2);
        handle.shutdown();

        let (first_hello, second_hello) = server.join().expect("join server");
        for hello in [first_hello, second_hello] {
            assert_eq!(hello.viewport_width(), 2);
            assert_eq!(hello.viewport_height(), 2);
            assert_eq!(
                hello.pixel_formats(),
                V2PixelFormats::MONO1.with(V2PixelFormat::Gray8)
            );
        }
    }

    #[test]
    fn shutdown_interrupts_idle_socket_read_within_read_timeout() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback server");
        let port = listener.local_addr().expect("listener address").port();
        let (accepted_sender, accepted_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut peer, _) = listener.accept().expect("accept worker");
            let _hello = read_hello(&mut peer);
            accepted_sender.send(()).expect("report accepted session");
            peer.set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set EOF timeout");
            let mut byte = [0; 1];
            assert_eq!(peer.read(&mut byte).expect("worker closes socket"), 0);
        });

        let (wake_sender, _wake_receiver) = mpsc::sync_channel(1);
        let handle = start_worker(test_config(port, 2, 2), channel_notifier(wake_sender))
            .expect("start worker");
        accepted_receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("worker connected");
        let started = Instant::now();
        handle.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "shutdown took {:?}",
            started.elapsed()
        );
        server.join().expect("join server");
    }

    #[test]
    fn invalid_config_is_rejected_before_thread_spawn() {
        let mut config = test_config(5581, 2, 2);
        config.read_timeout = Duration::ZERO;
        let error = match start_worker(config, || true) {
            Err(error) => error,
            Ok(handle) => {
                handle.shutdown();
                panic!("zero read timeout must fail");
            }
        };
        assert!(matches!(error, WorkerStartError::InvalidConfig(_)));

        let mut config = test_config(5581, 2, 2);
        config.reconnect_delay = Duration::ZERO;
        let error = match start_worker(config, || true) {
            Err(error) => error,
            Ok(handle) => {
                handle.shutdown();
                panic!("zero retry delay must fail");
            }
        };
        assert!(matches!(error, WorkerStartError::InvalidConfig(_)));
    }

    #[test]
    fn disconnected_notifier_stops_worker() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback server");
        let port = listener.local_addr().expect("listener address").port();
        let handle = start_worker(test_config(port, 2, 2), || false).expect("start worker");
        let started = Instant::now();
        handle.shutdown();
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
