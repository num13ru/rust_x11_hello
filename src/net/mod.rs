//! Non-blocking Kindle transport worker.
//!
//! The X11 thread only enqueues semantic actions and polls received display
//! commands. A dedicated worker performs explicit-host resolution or bounded
//! UDP discovery, TCP handshake, writes, ACK tracking, retry, and reconnect.

use anyhow::{Context, Result, anyhow};
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::time::{Duration, Instant};
use transport_protocol::{
    AckStatus, DiscoveryMessage, MAX_DISCOVERY_DATAGRAM_BYTES, MAX_STREAM_FRAME_BYTES,
    StreamMessage, format_discovery_message, format_stream_message, generate_hex_id,
    parse_discovery_message, parse_stream_message, read_bounded_line,
};

const COMPANION_HOST_ENV: &str = "RUST_X11_HELLO_COMPANION";
const COMPANION_PORT_ENV: &str = "RUST_X11_HELLO_COMPANION_PORT";
const DISCOVERY_PORT_ENV: &str = "RUST_X11_HELLO_DISCOVERY_PORT";
const COMPANION_ID_ENV: &str = "RUST_X11_HELLO_COMPANION_ID";
const LEGACY_MODE_ENV: &str = "RUST_X11_HELLO_LEGACY";

const EVENT_CHANNEL_CAPACITY: usize = 64;
const MESSAGE_CHANNEL_CAPACITY: usize = 16;
const PENDING_CAPACITY: usize = 32;
const DISCOVERY_PROBES: usize = 3;
const MAX_DISCOVERY_RESPONSES: usize = 64;
const MAX_SEND_ATTEMPTS: u8 = 3;
const MAX_CONNECT_ATTEMPTS: u8 = 3;
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(750);
const DISCOVERY_WINDOW: Duration = Duration::from_millis(250);
const ACK_TIMEOUT: Duration = Duration::from_secs(1);
const RECONNECT_BACKOFF: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
struct ClientConfig {
    explicit_host: Option<String>,
    tcp_port: u16,
    discovery_destination: SocketAddr,
    expected_companion: Option<String>,
    legacy_mode: bool,
    connect_timeout: Duration,
    handshake_timeout: Duration,
    discovery_window: Duration,
    discovery_probes: usize,
    ack_timeout: Duration,
    reconnect_backoff: Duration,
    max_send_attempts: u8,
    max_connect_attempts: u8,
}

impl ClientConfig {
    fn from_env() -> Result<Self> {
        let explicit_host = nonempty_env(COMPANION_HOST_ENV);
        let tcp_port = parse_env_port(COMPANION_PORT_ENV, transport_protocol::DEFAULT_TCP_PORT)?;
        let discovery_port = parse_env_port(
            DISCOVERY_PORT_ENV,
            transport_protocol::DEFAULT_DISCOVERY_PORT,
        )?;
        let expected_companion = nonempty_env(COMPANION_ID_ENV);
        if let Some(identity) = &expected_companion {
            format_stream_message(&StreamMessage::Welcome {
                companion: identity.clone(),
            })
            .context("invalid configured companion identity")?;
        }
        let legacy_mode = match nonempty_env(LEGACY_MODE_ENV).as_deref() {
            None | Some("0") | Some("false") => false,
            Some("1") | Some("true") => true,
            Some(value) => {
                return Err(anyhow!(
                    "{LEGACY_MODE_ENV} must be one of 0, 1, false, or true; got '{value}'"
                ));
            }
        };
        if legacy_mode && explicit_host.is_none() {
            return Err(anyhow!(
                "{LEGACY_MODE_ENV}=1 requires an explicit {COMPANION_HOST_ENV}"
            ));
        }
        Ok(Self {
            explicit_host,
            tcp_port,
            discovery_destination: SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), discovery_port),
            expected_companion,
            legacy_mode,
            connect_timeout: TCP_CONNECT_TIMEOUT,
            handshake_timeout: HANDSHAKE_TIMEOUT,
            discovery_window: DISCOVERY_WINDOW,
            discovery_probes: DISCOVERY_PROBES,
            ack_timeout: ACK_TIMEOUT,
            reconnect_backoff: RECONNECT_BACKOFF,
            max_send_attempts: MAX_SEND_ATTEMPTS,
            max_connect_attempts: MAX_CONNECT_ATTEMPTS,
        })
    }
}

#[derive(Debug)]
enum CompanionMsg {
    Display(String),
}

#[derive(Debug)]
enum WorkerEvent {
    Action(String),
    Inbound {
        generation: u64,
        message: StreamMessage,
    },
    Disconnected {
        generation: u64,
    },
    Shutdown,
}

#[derive(Debug)]
struct PendingEvent {
    seq: u64,
    action: String,
    attempts: u8,
    last_sent: Option<Instant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ConnectionMode {
    Versioned { companion: String },
    Legacy,
}

#[derive(Debug)]
struct ActiveConnection {
    generation: u64,
    peer: SocketAddr,
    stream: TcpStream,
    mode: ConnectionMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Offer {
    companion: String,
    address: SocketAddr,
}

/// Handle owned by the X11 event loop.
pub struct Companion {
    worker_tx: SyncSender<WorkerEvent>,
    message_rx: Receiver<CompanionMsg>,
}

impl Companion {
    /// Validate configuration and start the network worker. Successful return
    /// means the worker started, not that a companion has already connected.
    pub fn connect() -> Result<Self> {
        Self::start(ClientConfig::from_env()?)
    }

    fn start(config: ClientConfig) -> Result<Self> {
        let session = generate_hex_id().context("failed to generate transport session id")?;
        let (worker_tx, worker_rx) = mpsc::sync_channel(EVENT_CHANNEL_CAPACITY);
        let (message_tx, message_rx) = mpsc::sync_channel(MESSAGE_CHANNEL_CAPACITY);
        let worker_sender = worker_tx.clone();
        std::thread::Builder::new()
            .name("companion-network".to_string())
            .spawn(move || {
                run_worker(config, session, worker_rx, worker_sender, message_tx);
            })
            .context("failed to start companion network worker")?;
        Ok(Self {
            worker_tx,
            message_rx,
        })
    }

    /// Disabled transport used only after configuration/worker setup failure.
    pub fn disconnected() -> Self {
        let (worker_tx, worker_rx) = mpsc::sync_channel(1);
        drop(worker_rx);
        let (_message_tx, message_rx) = mpsc::sync_channel(1);
        Self {
            worker_tx,
            message_rx,
        }
    }

    /// Enqueue one semantic activation without blocking the X11 event loop.
    pub fn send_action(&mut self, semantic_id: &str) -> Result<()> {
        format_stream_message(&StreamMessage::LegacyEvent {
            action: semantic_id.to_string(),
        })
        .context("invalid semantic action id")?;
        match self
            .worker_tx
            .try_send(WorkerEvent::Action(semantic_id.to_string()))
        {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(anyhow!(
                "transport action queue full; action was not enqueued"
            )),
            Err(TrySendError::Disconnected(_)) => {
                Err(anyhow!("transport network worker is not running"))
            }
        }
    }

    /// Return the first display control received since the last poll.
    pub fn poll_display(&self) -> Option<String> {
        match self.message_rx.try_recv() {
            Ok(CompanionMsg::Display(text)) => Some(text),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }
}

impl Drop for Companion {
    fn drop(&mut self) {
        let _ = self.worker_tx.try_send(WorkerEvent::Shutdown);
    }
}

fn run_worker(
    config: ClientConfig,
    session: String,
    worker_rx: Receiver<WorkerEvent>,
    worker_tx: SyncSender<WorkerEvent>,
    message_tx: SyncSender<CompanionMsg>,
) {
    let mut pending = VecDeque::<PendingEvent>::new();
    let mut next_sequence = 1_u64;
    let mut connection: Option<ActiveConnection> = None;
    let mut generation = 0_u64;
    let mut connect_requested = true;
    let mut connect_attempts = 0_u8;
    let mut next_connect_at = Instant::now();

    loop {
        if connection.is_none() && connect_requested && Instant::now() >= next_connect_at {
            generation = generation.wrapping_add(1);
            match establish_connection(&config, &session, generation, &worker_tx) {
                Ok(established) => {
                    eprintln!(
                        "transport: connected peer={} mode={:?}",
                        established.peer, established.mode
                    );
                    connection = Some(established);
                    connect_requested = false;
                    connect_attempts = 0;
                    prepare_pending_for_reconnect(&mut pending, &config);
                }
                Err(error) => {
                    connect_attempts = connect_attempts.saturating_add(1);
                    eprintln!(
                        "transport connect attempt {connect_attempts}/{} failed: {error:#}",
                        config.max_connect_attempts
                    );
                    if pending.is_empty() || connect_attempts >= config.max_connect_attempts {
                        if !pending.is_empty() {
                            for event in pending.drain(..) {
                                eprintln!(
                                    "transport failed seq={} action={} reason=connect_attempts_exhausted",
                                    event.seq, event.action
                                );
                            }
                        }
                        connect_requested = false;
                        connect_attempts = 0;
                    } else {
                        next_connect_at = Instant::now() + config.reconnect_backoff;
                    }
                }
            }
        }

        let write_failure = connection.as_mut().and_then(|active| {
            send_due_events(active, &session, &mut pending, &config)
                .err()
                .map(|error| (active.peer, error))
        });
        if let Some((peer, error)) = write_failure {
            eprintln!("transport write failed peer={peer}: {error:#}");
            connection = None;
            prepare_pending_for_reconnect(&mut pending, &config);
            if !pending.is_empty() {
                connect_requested = true;
                connect_attempts = 0;
                next_connect_at = Instant::now() + config.reconnect_backoff;
            }
        }

        remove_timed_out_events(&mut pending, &config);
        let wait = next_worker_wait(
            connection.as_ref(),
            &pending,
            connect_requested,
            next_connect_at,
            &config,
        );
        let received = match wait {
            Some(duration) => worker_rx.recv_timeout(duration),
            None => match worker_rx.recv() {
                Ok(event) => Ok(event),
                Err(_) => break,
            },
        };
        let event = match received {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        match event {
            WorkerEvent::Action(action) => {
                if pending.len() >= PENDING_CAPACITY {
                    eprintln!("transport queue full; dropped action={action}");
                    continue;
                }
                let seq = next_sequence;
                let Some(incremented) = next_sequence.checked_add(1) else {
                    eprintln!("transport sequence exhausted; dropped action={action}");
                    continue;
                };
                next_sequence = incremented;
                pending.push_back(PendingEvent {
                    seq,
                    action,
                    attempts: 0,
                    last_sent: None,
                });
                if connection.is_none() {
                    connect_requested = true;
                    connect_attempts = 0;
                    next_connect_at = Instant::now();
                }
            }
            WorkerEvent::Inbound {
                generation: inbound_generation,
                message,
            } => {
                if connection.as_ref().map(|active| active.generation) != Some(inbound_generation) {
                    continue;
                }
                handle_inbound(message, &session, &mut pending, &message_tx);
            }
            WorkerEvent::Disconnected {
                generation: disconnected_generation,
            } => {
                if connection.as_ref().map(|active| active.generation)
                    == Some(disconnected_generation)
                {
                    if let Some(active) = connection.take() {
                        eprintln!("transport: disconnected peer={}", active.peer);
                    }
                    prepare_pending_for_reconnect(&mut pending, &config);
                    if !pending.is_empty() {
                        connect_requested = true;
                        connect_attempts = 0;
                        next_connect_at = Instant::now() + config.reconnect_backoff;
                    }
                }
            }
            WorkerEvent::Shutdown => break,
        }
    }
}

fn establish_connection(
    config: &ClientConfig,
    session: &str,
    generation: u64,
    worker_tx: &SyncSender<WorkerEvent>,
) -> Result<ActiveConnection> {
    let offered = if config.explicit_host.is_some() {
        None
    } else {
        Some(discover_companion(config)?)
    };
    let address = match (&config.explicit_host, &offered) {
        (Some(host), _) => resolve_explicit(host, config.tcp_port)?,
        (None, Some(offer)) => offer.address,
        (None, None) => unreachable!("target source is exhaustive"),
    };
    let mut stream = TcpStream::connect_timeout(&address, config.connect_timeout)
        .with_context(|| format!("failed to connect companion at {address}"))?;
    let _ = stream.set_nodelay(true);

    let mode = if config.legacy_mode {
        ConnectionMode::Legacy
    } else {
        stream
            .set_read_timeout(Some(config.handshake_timeout))
            .context("failed to set handshake timeout")?;
        let hello = format_stream_message(&StreamMessage::Hello {
            session: session.to_string(),
        })
        .context("failed to format hello")?;
        stream
            .write_all(hello.as_bytes())
            .context("failed to write hello")?;
        stream.flush().context("failed to flush hello")?;
        let reader_stream = stream.try_clone().context("failed to clone TCP reader")?;
        let mut reader = BufReader::new(reader_stream);
        let welcome = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
            .context("failed to read welcome")?
            .context("companion closed before welcome")?;
        let companion = match parse_stream_message(&welcome).context("invalid welcome frame")? {
            StreamMessage::Welcome { companion } => companion,
            other => return Err(anyhow!("expected welcome, received {other:?}")),
        };
        match &config.expected_companion {
            Some(expected) if companion != *expected => {
                return Err(anyhow!(
                    "companion identity mismatch: expected {expected}, received {companion}"
                ));
            }
            _ => {}
        }
        match &offered {
            Some(offer) if companion != offer.companion => {
                return Err(anyhow!(
                    "discovery/TCP identity mismatch: offered {}, welcomed {companion}",
                    offer.companion
                ));
            }
            _ => {}
        }
        stream
            .set_read_timeout(None)
            .context("failed to clear handshake timeout")?;
        spawn_reader(reader, generation, worker_tx.clone());
        return Ok(ActiveConnection {
            generation,
            peer: address,
            stream,
            mode: ConnectionMode::Versioned { companion },
        });
    };

    let reader_stream = stream.try_clone().context("failed to clone TCP reader")?;
    spawn_reader(BufReader::new(reader_stream), generation, worker_tx.clone());
    Ok(ActiveConnection {
        generation,
        peer: address,
        stream,
        mode,
    })
}

fn spawn_reader(
    mut reader: BufReader<TcpStream>,
    generation: u64,
    worker_tx: SyncSender<WorkerEvent>,
) {
    std::thread::spawn(move || {
        loop {
            let line = match read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES) {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(transport_protocol::ReadFrameError::TooLong { limit }) => {
                    eprintln!("transport rejected oversized inbound frame; limit={limit}");
                    continue;
                }
                Err(transport_protocol::ReadFrameError::InvalidUtf8) => {
                    eprintln!("transport rejected non-UTF-8 inbound frame");
                    continue;
                }
                Err(transport_protocol::ReadFrameError::Io(error)) => {
                    eprintln!("transport read error: {error}");
                    break;
                }
            };
            if line.is_empty() {
                continue;
            }
            let message = match parse_stream_message(&line) {
                Ok(message) => message,
                Err(error) => {
                    eprintln!("transport rejected inbound frame: {error}");
                    continue;
                }
            };
            if worker_tx
                .send(WorkerEvent::Inbound {
                    generation,
                    message,
                })
                .is_err()
            {
                return;
            }
        }
        let _ = worker_tx.send(WorkerEvent::Disconnected { generation });
    });
}

fn send_due_events(
    connection: &mut ActiveConnection,
    session: &str,
    pending: &mut VecDeque<PendingEvent>,
    config: &ClientConfig,
) -> Result<()> {
    match connection.mode {
        ConnectionMode::Legacy => {
            while let Some(event) = pending.pop_front() {
                let line = format_stream_message(&StreamMessage::LegacyEvent {
                    action: event.action.clone(),
                })
                .context("failed to format legacy event")?;
                connection
                    .stream
                    .write_all(line.as_bytes())
                    .with_context(|| format!("failed to write legacy event seq={}", event.seq))?;
                connection.stream.flush().context("failed to flush event")?;
                eprintln!(
                    "transport legacy sent seq={} action={} acknowledgement=unavailable",
                    event.seq, event.action
                );
            }
        }
        ConnectionMode::Versioned { .. } => {
            let now = Instant::now();
            for event in pending.iter_mut() {
                let due = event
                    .last_sent
                    .is_none_or(|sent| now.saturating_duration_since(sent) >= config.ack_timeout);
                if !due || event.attempts >= config.max_send_attempts {
                    continue;
                }
                let line = format_stream_message(&StreamMessage::Event {
                    session: session.to_string(),
                    seq: event.seq,
                    action: event.action.clone(),
                })
                .context("failed to format event")?;
                connection
                    .stream
                    .write_all(line.as_bytes())
                    .with_context(|| format!("failed to write event seq={}", event.seq))?;
                connection.stream.flush().context("failed to flush event")?;
                event.attempts = event.attempts.saturating_add(1);
                event.last_sent = Some(now);
                eprintln!(
                    "transport sent seq={} action={} attempt={}/{}",
                    event.seq, event.action, event.attempts, config.max_send_attempts
                );
            }
        }
    }
    Ok(())
}

fn remove_timed_out_events(pending: &mut VecDeque<PendingEvent>, config: &ClientConfig) {
    let now = Instant::now();
    pending.retain(|event| {
        let exhausted = event.attempts >= config.max_send_attempts
            && event
                .last_sent
                .is_some_and(|sent| now.saturating_duration_since(sent) >= config.ack_timeout);
        if exhausted {
            eprintln!(
                "transport failed seq={} action={} reason=ack_timeout attempts={}",
                event.seq, event.action, event.attempts
            );
        }
        !exhausted
    });
}

fn prepare_pending_for_reconnect(pending: &mut VecDeque<PendingEvent>, config: &ClientConfig) {
    pending.retain_mut(|event| {
        if event.attempts >= config.max_send_attempts {
            eprintln!(
                "transport failed seq={} action={} reason=connection_lost attempts={}",
                event.seq, event.action, event.attempts
            );
            false
        } else {
            event.last_sent = None;
            true
        }
    });
}

fn handle_inbound(
    message: StreamMessage,
    session: &str,
    pending: &mut VecDeque<PendingEvent>,
    message_tx: &SyncSender<CompanionMsg>,
) {
    match message {
        StreamMessage::Ack {
            session: ack_session,
            seq,
            status: AckStatus::Logged,
        } if ack_session == session => {
            if let Some(position) = pending.iter().position(|event| event.seq == seq) {
                if let Some(event) = pending.remove(position) {
                    eprintln!(
                        "transport acknowledged seq={} action={} status=logged attempts={}",
                        event.seq, event.action, event.attempts
                    );
                }
            } else {
                eprintln!("transport ignored late or duplicate ack seq={seq}");
            }
        }
        StreamMessage::Ack {
            session: ack_session,
            seq,
            ..
        } => {
            eprintln!("transport ignored mismatched ack session={ack_session} seq={seq}");
        }
        StreamMessage::Display { text } => {
            if let Err(error) = message_tx.try_send(CompanionMsg::Display(text)) {
                eprintln!("transport dropped display command: {error}");
            }
        }
        other => eprintln!("transport ignored wrong-direction inbound message: {other:?}"),
    }
}

fn next_worker_wait(
    connection: Option<&ActiveConnection>,
    pending: &VecDeque<PendingEvent>,
    connect_requested: bool,
    next_connect_at: Instant,
    config: &ClientConfig,
) -> Option<Duration> {
    let now = Instant::now();
    let mut deadline = if connection.is_none() && connect_requested {
        Some(next_connect_at)
    } else {
        None
    };
    if connection.is_some_and(|active| matches!(active.mode, ConnectionMode::Versioned { .. })) {
        for event in pending {
            if event.attempts >= config.max_send_attempts {
                if let Some(sent) = event.last_sent {
                    deadline = earliest(deadline, sent + config.ack_timeout);
                }
            } else {
                let due = event
                    .last_sent
                    .map_or(now, |sent| sent + config.ack_timeout);
                deadline = earliest(deadline, due);
            }
        }
    }
    deadline.map(|deadline| deadline.saturating_duration_since(now))
}

fn earliest(current: Option<Instant>, candidate: Instant) -> Option<Instant> {
    Some(current.map_or(candidate, |current| current.min(candidate)))
}

fn discover_companion(config: &ClientConfig) -> Result<Offer> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .context("failed to bind UDP discovery socket")?;
    socket
        .set_broadcast(true)
        .context("failed to enable UDP broadcast")?;
    let nonce = generate_hex_id().context("failed to generate discovery nonce")?;
    let request = format_discovery_message(&DiscoveryMessage::Discover {
        nonce: nonce.clone(),
    })
    .context("failed to format discovery request")?;
    let mut offers = BTreeMap::<String, SocketAddr>::new();
    let mut responses = 0_usize;

    for _ in 0..config.discovery_probes {
        socket
            .send_to(request.as_bytes(), config.discovery_destination)
            .with_context(|| {
                format!(
                    "failed to send discovery request to {}",
                    config.discovery_destination
                )
            })?;
        let deadline = Instant::now() + config.discovery_window;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || responses >= MAX_DISCOVERY_RESPONSES {
                break;
            }
            socket
                .set_read_timeout(Some(remaining))
                .context("failed to set discovery timeout")?;
            let mut buffer = [0_u8; MAX_DISCOVERY_DATAGRAM_BYTES + 1];
            let (length, source) = match socket.recv_from(&mut buffer) {
                Ok(received) => received,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => return Err(error).context("failed to receive discovery offer"),
            };
            responses += 1;
            let (offer_nonce, companion, port) = match parse_discovery_message(&buffer[..length]) {
                Ok(DiscoveryMessage::Offer {
                    nonce,
                    companion,
                    port,
                }) => (nonce, companion, port),
                Ok(DiscoveryMessage::Discover { .. }) => continue,
                Err(error) => {
                    eprintln!("transport ignored malformed discovery response: {error}");
                    continue;
                }
            };
            if offer_nonce != nonce {
                eprintln!("transport ignored stale discovery nonce from {source}");
                continue;
            }
            match &config.expected_companion {
                Some(expected) if companion != *expected => continue,
                _ => {}
            }
            let address = SocketAddr::new(source.ip(), port);
            match offers.insert(companion.clone(), address) {
                Some(previous) if previous != address => {
                    return Err(anyhow!(
                        "companion identity {companion} answered from multiple addresses: {previous}, {address}"
                    ));
                }
                _ => {}
            }
        }
    }
    select_offer(offers, config.expected_companion.as_deref())
}

fn select_offer(
    offers: BTreeMap<String, SocketAddr>,
    expected_companion: Option<&str>,
) -> Result<Offer> {
    if let Some(expected) = expected_companion {
        let address = offers
            .get(expected)
            .copied()
            .ok_or_else(|| anyhow!("no discovery offer matched companion {expected}"))?;
        return Ok(Offer {
            companion: expected.to_string(),
            address,
        });
    }
    match offers.len() {
        0 => Err(anyhow!("no valid companion discovery offer received")),
        1 => {
            let (companion, address) = offers.into_iter().next().expect("length checked");
            Ok(Offer { companion, address })
        }
        count => Err(anyhow!(
            "ambiguous discovery: {count} companions answered; configure {COMPANION_ID_ENV}"
        )),
    }
}

fn resolve_explicit(host: &str, port: u16) -> Result<SocketAddr> {
    (host, port)
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve explicit companion host '{host}'"))?
        .next()
        .ok_or_else(|| anyhow!("explicit companion host '{host}' resolved to no addresses"))
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_env_port(name: &str, default: u16) -> Result<u16> {
    match nonempty_env(name) {
        Some(value) => value
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| anyhow!("{name} must be a nonzero u16 port; got '{value}'")),
        None => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::Mutex;

    const SESSION: &str = "0123456789abcdef0123456789abcdef";
    const COMPANION_ID: &str = "fedcba9876543210fedcba9876543210";
    static NETWORK_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_config(address: SocketAddr) -> ClientConfig {
        ClientConfig {
            explicit_host: Some(address.ip().to_string()),
            tcp_port: address.port(),
            discovery_destination: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9),
            expected_companion: Some(COMPANION_ID.to_string()),
            legacy_mode: false,
            connect_timeout: Duration::from_millis(250),
            handshake_timeout: Duration::from_secs(1),
            discovery_window: Duration::from_millis(50),
            discovery_probes: 1,
            ack_timeout: Duration::from_millis(100),
            reconnect_backoff: Duration::from_millis(25),
            max_send_attempts: 3,
            max_connect_attempts: 2,
        }
    }

    #[test]
    fn offer_selection_handles_none_one_pairing_and_ambiguity() {
        assert!(select_offer(BTreeMap::new(), None).is_err());
        let first = SocketAddr::from(([127, 0, 0, 1], 5581));
        let second = SocketAddr::from(([127, 0, 0, 1], 5582));
        let mut offers = BTreeMap::new();
        offers.insert(COMPANION_ID.to_string(), first);
        assert_eq!(
            select_offer(offers.clone(), None).expect("single offer"),
            Offer {
                companion: COMPANION_ID.to_string(),
                address: first,
            }
        );
        offers.insert(SESSION.to_string(), second);
        assert!(select_offer(offers.clone(), None).is_err());
        assert_eq!(
            select_offer(offers, Some(SESSION)).expect("paired offer"),
            Offer {
                companion: SESSION.to_string(),
                address: second,
            }
        );
    }

    #[test]
    fn pending_ack_removes_only_matching_session_and_sequence() {
        let mut pending = VecDeque::from([
            PendingEvent {
                seq: 1,
                action: "media.next".to_string(),
                attempts: 1,
                last_sent: Some(Instant::now()),
            },
            PendingEvent {
                seq: 2,
                action: "tmux.work".to_string(),
                attempts: 1,
                last_sent: Some(Instant::now()),
            },
        ]);
        let (message_tx, _message_rx) = mpsc::sync_channel(4);
        handle_inbound(
            StreamMessage::Ack {
                session: COMPANION_ID.to_string(),
                seq: 1,
                status: AckStatus::Logged,
            },
            SESSION,
            &mut pending,
            &message_tx,
        );
        assert_eq!(pending.len(), 2);
        handle_inbound(
            StreamMessage::Ack {
                session: SESSION.to_string(),
                seq: 1,
                status: AckStatus::Logged,
            },
            SESSION,
            &mut pending,
            &message_tx,
        );
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].seq, 2);
    }

    #[test]
    fn reconnect_drops_exhausted_event_and_rearms_retryable_event() {
        let config = test_config(SocketAddr::from(([127, 0, 0, 1], 9)));
        let mut pending = VecDeque::from([
            PendingEvent {
                seq: 1,
                action: "media.next".to_string(),
                attempts: config.max_send_attempts,
                last_sent: Some(Instant::now()),
            },
            PendingEvent {
                seq: 2,
                action: "tmux.work".to_string(),
                attempts: 1,
                last_sent: Some(Instant::now()),
            },
        ]);
        prepare_pending_for_reconnect(&mut pending, &config);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].seq, 2);
        assert_eq!(pending[0].attempts, 1);
        assert_eq!(pending[0].last_sent, None);
    }

    #[test]
    fn explicit_connection_negotiates_and_acknowledges_without_discovery() {
        let _network_guard = NETWORK_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind fake companion");
        let address = listener.local_addr().expect("listener address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept Kindle worker");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set server timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone server reader"));
            let hello = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
                .expect("read hello")
                .expect("hello frame");
            let session = match parse_stream_message(&hello).expect("parse hello") {
                StreamMessage::Hello { session } => session,
                other => panic!("unexpected message: {other:?}"),
            };
            let welcome = format_stream_message(&StreamMessage::Welcome {
                companion: COMPANION_ID.to_string(),
            })
            .expect("format welcome");
            stream.write_all(welcome.as_bytes()).expect("write welcome");
            let event = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
                .expect("read event")
                .expect("event frame");
            let (seq, action) = match parse_stream_message(&event).expect("parse event") {
                StreamMessage::Event {
                    session: event_session,
                    seq,
                    action,
                } => {
                    assert_eq!(event_session, session);
                    (seq, action)
                }
                other => panic!("unexpected message: {other:?}"),
            };
            let ack = format_stream_message(&StreamMessage::Ack {
                session,
                seq,
                status: AckStatus::Logged,
            })
            .expect("format ack");
            stream.write_all(ack.as_bytes()).expect("write ack");
            (seq, action)
        });

        let companion = Companion::start(test_config(address)).expect("start worker");
        let started = Instant::now();
        let mut companion = companion;
        companion
            .send_action("media.next")
            .expect("enqueue action without blocking");
        assert!(started.elapsed() < Duration::from_millis(100));
        let (seq, action) = server.join().expect("join fake companion");
        assert_eq!(seq, 1);
        assert_eq!(action, "media.next");
        drop(companion);
    }

    #[test]
    fn unavailable_peer_does_not_block_action_enqueue() {
        let _network_guard = NETWORK_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve port");
        let address = listener.local_addr().expect("reserved address");
        drop(listener);
        let mut config = test_config(address);
        config.connect_timeout = Duration::from_millis(25);
        config.max_connect_attempts = 1;
        let mut companion = Companion::start(config).expect("start worker");
        let started = Instant::now();
        companion.send_action("tmux.work").expect("enqueue action");
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn discovery_ignores_stale_offer_and_connects_to_advertised_tcp_port() {
        let _network_guard = NETWORK_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let discovery = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind discovery fixture");
        let discovery_address = discovery.local_addr().expect("discovery address");
        let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind TCP fixture");
        let tcp_address = tcp.local_addr().expect("TCP address");
        let server = std::thread::spawn(move || {
            let mut datagram = [0_u8; MAX_DISCOVERY_DATAGRAM_BYTES + 1];
            let (length, source) = discovery
                .recv_from(&mut datagram)
                .expect("receive discovery request");
            let nonce = match parse_discovery_message(&datagram[..length]).expect("parse discover")
            {
                DiscoveryMessage::Discover { nonce } => nonce,
                other => panic!("unexpected discovery message: {other:?}"),
            };
            let stale = format_discovery_message(&DiscoveryMessage::Offer {
                nonce: SESSION.to_string(),
                companion: COMPANION_ID.to_string(),
                port: tcp_address.port(),
            })
            .expect("format stale offer");
            discovery
                .send_to(stale.as_bytes(), source)
                .expect("send stale offer");
            let valid = format_discovery_message(&DiscoveryMessage::Offer {
                nonce,
                companion: COMPANION_ID.to_string(),
                port: tcp_address.port(),
            })
            .expect("format valid offer");
            discovery
                .send_to(valid.as_bytes(), source)
                .expect("send valid offer");

            let (mut stream, _) = tcp.accept().expect("accept discovered connection");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set TCP timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone TCP reader"));
            let hello = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
                .expect("read discovered hello")
                .expect("discovered hello frame");
            let session = match parse_stream_message(&hello).expect("parse discovered hello") {
                StreamMessage::Hello { session } => session,
                other => panic!("unexpected discovered message: {other:?}"),
            };
            let welcome = format_stream_message(&StreamMessage::Welcome {
                companion: COMPANION_ID.to_string(),
            })
            .expect("format discovered welcome");
            stream
                .write_all(welcome.as_bytes())
                .expect("write discovered welcome");
            let event = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
                .expect("read discovered event")
                .expect("discovered event frame");
            let (seq, action) = match parse_stream_message(&event).expect("parse discovered event")
            {
                StreamMessage::Event {
                    session: event_session,
                    seq,
                    action,
                } => {
                    assert_eq!(event_session, session);
                    (seq, action)
                }
                other => panic!("unexpected discovered event: {other:?}"),
            };
            let ack = format_stream_message(&StreamMessage::Ack {
                session,
                seq,
                status: AckStatus::Logged,
            })
            .expect("format discovered ack");
            stream
                .write_all(ack.as_bytes())
                .expect("write discovered ack");
            action
        });

        let mut config = test_config(tcp_address);
        config.explicit_host = None;
        config.discovery_destination = discovery_address;
        config.discovery_window = Duration::from_millis(250);
        let mut companion = Companion::start(config).expect("start discovery worker");
        companion
            .send_action("zoom.toggle_mute")
            .expect("enqueue discovered action");
        assert_eq!(
            server.join().expect("join discovery fixture"),
            "zoom.toggle_mute"
        );
    }

    #[test]
    fn ack_timeout_resends_same_event_identity() {
        let _network_guard = NETWORK_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind retry fixture");
        let address = listener.local_addr().expect("retry address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept retry client");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set retry timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone retry reader"));
            let hello = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
                .expect("read retry hello")
                .expect("retry hello frame");
            let session = match parse_stream_message(&hello).expect("parse retry hello") {
                StreamMessage::Hello { session } => session,
                other => panic!("unexpected retry message: {other:?}"),
            };
            stream
                .write_all(
                    format_stream_message(&StreamMessage::Welcome {
                        companion: COMPANION_ID.to_string(),
                    })
                    .expect("format retry welcome")
                    .as_bytes(),
                )
                .expect("write retry welcome");
            let first = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
                .expect("read first retry event")
                .expect("first retry frame");
            let second = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
                .expect("read second retry event")
                .expect("second retry frame");
            assert_eq!(first, second);
            let seq = match parse_stream_message(&second).expect("parse retry event") {
                StreamMessage::Event {
                    session: event_session,
                    seq,
                    ..
                } => {
                    assert_eq!(event_session, session);
                    seq
                }
                other => panic!("unexpected retry event: {other:?}"),
            };
            stream
                .write_all(
                    format_stream_message(&StreamMessage::Ack {
                        session,
                        seq,
                        status: AckStatus::Logged,
                    })
                    .expect("format retry ack")
                    .as_bytes(),
                )
                .expect("write retry ack");
        });

        let mut config = test_config(address);
        config.ack_timeout = Duration::from_millis(50);
        let mut companion = Companion::start(config).expect("start retry worker");
        companion
            .send_action("media.play_pause")
            .expect("enqueue retry action");
        server.join().expect("join retry fixture");
    }

    #[test]
    fn explicit_legacy_mode_sends_no_hello_and_claims_no_ack() {
        let _network_guard = NETWORK_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind legacy fixture");
        let address = listener.local_addr().expect("legacy address");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept legacy client");
            let mut reader = BufReader::new(stream);
            let line = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
                .expect("read legacy event")
                .expect("legacy event frame");
            parse_stream_message(&line).expect("parse legacy event")
        });
        let mut config = test_config(address);
        config.legacy_mode = true;
        let mut companion = Companion::start(config).expect("start legacy worker");
        companion
            .send_action("media.previous")
            .expect("enqueue legacy action");
        assert_eq!(
            server.join().expect("join legacy fixture"),
            StreamMessage::LegacyEvent {
                action: "media.previous".to_string()
            }
        );
    }

    #[test]
    fn formatted_event_never_contains_unbounded_action_data() {
        let invalid = "x".repeat(MAX_STREAM_FRAME_BYTES);
        let result = format_stream_message(&StreamMessage::Event {
            session: SESSION.to_string(),
            seq: 1,
            action: invalid,
        });
        assert!(result.is_err());
    }

    #[test]
    fn connection_reader_rejects_wrong_direction_without_affecting_display_channel() {
        let (message_tx, message_rx) = mpsc::sync_channel(4);
        let mut pending = VecDeque::new();
        handle_inbound(
            StreamMessage::Hello {
                session: SESSION.to_string(),
            },
            SESSION,
            &mut pending,
            &message_tx,
        );
        assert!(message_rx.try_recv().is_err());
        handle_inbound(
            StreamMessage::Display {
                text: "hello".to_string(),
            },
            SESSION,
            &mut pending,
            &message_tx,
        );
        assert!(matches!(
            message_rx.try_recv(),
            Ok(CompanionMsg::Display(text)) if text == "hello"
        ));
    }

    #[test]
    fn legacy_write_has_no_ack_claim() {
        let _ = std::mem::size_of::<ConnectionMode>();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            format_stream_message(&StreamMessage::LegacyEvent {
                action: "media.previous".to_string(),
            })
            .expect("format legacy")
            .as_bytes(),
        );
        let mut text = String::new();
        text.push_str(std::str::from_utf8(&bytes).expect("legacy UTF-8"));
        assert_eq!(text, "event action=media.previous;\n");
    }
}
