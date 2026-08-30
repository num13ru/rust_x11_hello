//! TCP companion for Kindle semantic activations.
//!
//! The companion strictly parses versioned messages, accepts the legacy event
//! line during migration, durably logs valid events, and serializes all writes
//! back to the active Kindle connection.

use std::collections::{HashSet, VecDeque};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use transport_protocol::{
    AckStatus, DiscoveryMessage, MAX_DISCOVERY_DATAGRAM_BYTES, MAX_STREAM_FRAME_BYTES,
    StreamMessage, format_discovery_message, format_stream_message, generate_hex_id,
    parse_discovery_message, parse_stream_message, read_bounded_line,
};

pub const DEFAULT_LOG_FILE: &str = "companion.log";
pub const DEFAULT_IDENTITY_FILE: &str = "companion.id";
pub const DEDUP_CAPACITY: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub port: u16,
    pub discovery_port: u16,
    pub log_path: PathBuf,
    pub identity_path: PathBuf,
}

impl Config {
    /// Parse the backward-compatible positional CLI:
    /// `companion [port] [log-file] [identity-file] [discovery-port]`.
    pub fn from_args<I, S>(args: I) -> io::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let args: Vec<String> = args.into_iter().map(Into::into).collect();
        if args.len() > 5 {
            return Err(invalid_input(
                "usage: companion [port] [log-file] [identity-file] [discovery-port]",
            ));
        }
        let port = match args.get(1) {
            Some(value) => value
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| invalid_input(format!("invalid TCP port '{value}'")))?,
            None => transport_protocol::DEFAULT_TCP_PORT,
        };
        let discovery_port = match args.get(4) {
            Some(value) => value
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| invalid_input(format!("invalid discovery port '{value}'")))?,
            None => transport_protocol::DEFAULT_DISCOVERY_PORT,
        };
        Ok(Self {
            port,
            discovery_port,
            log_path: args
                .get(2)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_LOG_FILE)),
            identity_path: args
                .get(3)
                .map(PathBuf::from)
                .or_else(|| env::var_os("RUST_X11_HELLO_COMPANION_ID_FILE").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from(DEFAULT_IDENTITY_FILE)),
        })
    }
}

#[derive(Debug)]
struct ActiveWriter {
    connection_id: u64,
    stream: TcpStream,
}

type SharedWriter = Arc<Mutex<Option<ActiveWriter>>>;

#[derive(Debug)]
struct DedupCache {
    capacity: usize,
    order: VecDeque<(String, u64)>,
    seen: HashSet<(String, u64)>,
}

impl DedupCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            seen: HashSet::with_capacity(capacity),
        }
    }

    fn contains(&self, session: &str, seq: u64) -> bool {
        self.seen.contains(&(session.to_string(), seq))
    }

    fn insert(&mut self, session: String, seq: u64) {
        let key = (session, seq);
        if self.capacity == 0 || !self.seen.insert(key.clone()) {
            return;
        }
        self.order.push_back(key);
        while self.order.len() > self.capacity {
            if let Some(expired) = self.order.pop_front() {
                self.seen.remove(&expired);
            }
        }
    }
}

pub fn run(config: Config) -> io::Result<()> {
    let companion_id = load_or_create_identity(&config.identity_path)?;
    let listener = TcpListener::bind(("0.0.0.0", config.port))?;
    let discovery_socket = UdpSocket::bind(("0.0.0.0", config.discovery_port))?;
    discovery_socket.set_broadcast(true)?;
    spawn_discovery_responder(discovery_socket, companion_id.clone(), config.port);
    println!(
        "listening tcp=0.0.0.0:{} discovery=0.0.0.0:{}, logging to {}, companion={companion_id}",
        config.port,
        config.discovery_port,
        config.log_path.display()
    );
    println!("type 'display <text>' to send a control command");
    io::stdout().flush()?;

    let active: SharedWriter = Arc::new(Mutex::new(None));
    spawn_stdin_forwarder(Arc::clone(&active));
    let dedup = Arc::new(Mutex::new(DedupCache::new(DEDUP_CAPACITY)));
    let next_connection_id = AtomicU64::new(1);

    for connection in listener.incoming() {
        let stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("accept error: {error}");
                continue;
            }
        };
        let peer = stream
            .peer_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        let connection_id = next_connection_id.fetch_add(1, Ordering::Relaxed);
        let writer = stream.try_clone()?;
        {
            let mut active_guard = lock(&active, "active connection")?;
            if active_guard.is_some() {
                eprintln!("rejecting additional connection from {peer}: companion is busy");
                continue;
            }
            *active_guard = Some(ActiveWriter {
                connection_id,
                stream: writer,
            });
        }

        println!("connected: {peer}");
        io::stdout().flush()?;
        let active_for_client = Arc::clone(&active);
        let dedup_for_client = Arc::clone(&dedup);
        let log_path = config.log_path.clone();
        let companion_id = companion_id.clone();
        std::thread::spawn(move || {
            if let Err(error) = handle_connection(
                stream,
                peer,
                connection_id,
                &log_path,
                &companion_id,
                &active_for_client,
                &dedup_for_client,
            ) {
                eprintln!("connection {peer} error: {error}");
            }
            if let Err(error) = clear_active(&active_for_client, connection_id) {
                eprintln!("connection {peer} cleanup error: {error}");
            }
            println!("disconnected: {peer}");
            let _ = io::stdout().flush();
        });
    }
    Ok(())
}

fn spawn_discovery_responder(socket: UdpSocket, companion_id: String, tcp_port: u16) {
    std::thread::spawn(move || discovery_responder(socket, &companion_id, tcp_port));
}

fn discovery_responder(socket: UdpSocket, companion_id: &str, tcp_port: u16) {
    let mut buffer = [0_u8; MAX_DISCOVERY_DATAGRAM_BYTES + 1];
    loop {
        let (length, peer) = match socket.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(error) => {
                eprintln!("discovery receive error: {error}");
                continue;
            }
        };
        let nonce = match parse_discovery_message(&buffer[..length]) {
            Ok(DiscoveryMessage::Discover { nonce }) => nonce,
            Ok(DiscoveryMessage::Offer { .. }) => {
                eprintln!("ignored discovery offer from {peer}");
                continue;
            }
            Err(error) => {
                eprintln!("rejected discovery datagram from {peer}: {error}");
                continue;
            }
        };
        let offer = match format_discovery_message(&DiscoveryMessage::Offer {
            nonce,
            companion: companion_id.to_string(),
            port: tcp_port,
        }) {
            Ok(offer) => offer,
            Err(error) => {
                eprintln!("failed to format discovery offer: {error}");
                continue;
            }
        };
        for (kind, destination) in discovery_offer_destinations(peer) {
            match socket.send_to(offer.as_bytes(), destination) {
                Ok(length) => {
                    println!(
                        "discovery offer {kind} sent to {destination} bytes={length} companion={companion_id} port={tcp_port}"
                    );
                }
                Err(error) => {
                    eprintln!("failed to send {kind} discovery offer to {destination}: {error}");
                }
            }
        }
    }
}

fn discovery_offer_destinations(peer: SocketAddr) -> [(&'static str, SocketAddr); 2] {
    [
        ("unicast", peer),
        (
            "limited-broadcast",
            SocketAddr::from((Ipv4Addr::BROADCAST, peer.port())),
        ),
    ]
}

fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    connection_id: u64,
    log_path: &Path,
    companion_id: &str,
    active: &SharedWriter,
    dedup: &Arc<Mutex<DedupCache>>,
) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let mut negotiated_session: Option<String> = None;

    loop {
        let line = match read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES) {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(()),
            Err(transport_protocol::ReadFrameError::TooLong { limit }) => {
                eprintln!("rejected oversized frame from {peer}; limit={limit}");
                continue;
            }
            Err(transport_protocol::ReadFrameError::InvalidUtf8) => {
                eprintln!("rejected non-UTF-8 frame from {peer}");
                continue;
            }
            Err(transport_protocol::ReadFrameError::Io(error)) => return Err(error),
        };
        if line.is_empty() {
            continue;
        }

        let message = match parse_stream_message(&line) {
            Ok(message) => message,
            Err(error) => {
                eprintln!("rejected protocol frame from {peer}: {error}");
                continue;
            }
        };
        match message {
            StreamMessage::Hello { session } => {
                if negotiated_session.is_some() {
                    eprintln!("rejected repeated hello from {peer}");
                    continue;
                }
                negotiated_session = Some(session);
                send_to_connection(
                    active,
                    connection_id,
                    &StreamMessage::Welcome {
                        companion: companion_id.to_string(),
                    },
                )?;
            }
            StreamMessage::Event {
                session,
                seq,
                action,
            } => {
                if negotiated_session.as_deref() != Some(session.as_str()) {
                    eprintln!(
                        "rejected event from {peer}: session was not negotiated or mismatched"
                    );
                    continue;
                }
                let duplicate = lock(dedup, "dedup cache")?.contains(&session, seq);
                if !duplicate {
                    let canonical = format_stream_message(&StreamMessage::Event {
                        session: session.clone(),
                        seq,
                        action,
                    })
                    .map_err(|error| invalid_data(error.to_string()))?;
                    append_and_sync(&mut file, peer, canonical.trim_end())?;
                    lock(dedup, "dedup cache")?.insert(session.clone(), seq);
                } else {
                    println!("duplicate from {peer}: session={session} seq={seq}");
                    io::stdout().flush()?;
                }
                send_to_connection(
                    active,
                    connection_id,
                    &StreamMessage::Ack {
                        session,
                        seq,
                        status: AckStatus::Logged,
                    },
                )?;
            }
            StreamMessage::LegacyEvent { action } => {
                let canonical = format_stream_message(&StreamMessage::LegacyEvent { action })
                    .map_err(|error| invalid_data(error.to_string()))?;
                append_and_sync(&mut file, peer, canonical.trim_end())?;
            }
            StreamMessage::Welcome { .. }
            | StreamMessage::Ack { .. }
            | StreamMessage::Display { .. } => {
                eprintln!("rejected wrong-direction message from {peer}");
            }
        }
    }
}

fn append_and_sync(file: &mut File, peer: SocketAddr, line: &str) -> io::Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    writeln!(file, "{timestamp} {peer} {line}")?;
    file.flush()?;
    file.sync_data()?;
    println!("received from {peer}: {line}");
    io::stdout().flush()
}

fn spawn_stdin_forwarder(active: SharedWriter) {
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        loop {
            let line = match read_bounded_line(&mut stdin, MAX_STREAM_FRAME_BYTES) {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(transport_protocol::ReadFrameError::TooLong { limit }) => {
                    eprintln!("operator input rejected: line exceeds {limit} bytes");
                    continue;
                }
                Err(transport_protocol::ReadFrameError::InvalidUtf8) => {
                    eprintln!("operator input rejected: line is not valid UTF-8");
                    continue;
                }
                Err(transport_protocol::ReadFrameError::Io(error)) => {
                    eprintln!("stdin error: {error}");
                    break;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let message = match parse_stream_message(&line) {
                Ok(message @ StreamMessage::Display { .. }) => message,
                Ok(_) => {
                    eprintln!("operator input rejected: only 'display <text>' is allowed");
                    continue;
                }
                Err(error) => {
                    eprintln!("operator input rejected: {error}");
                    continue;
                }
            };
            if let Err(error) = send_to_active(&active, &message) {
                eprintln!("display not sent: {error}");
            }
        }
    });
}

fn send_to_connection(
    active: &SharedWriter,
    connection_id: u64,
    message: &StreamMessage,
) -> io::Result<()> {
    let encoded =
        format_stream_message(message).map_err(|error| invalid_data(error.to_string()))?;
    let mut active_guard = lock(active, "active connection")?;
    let writer = active_guard
        .as_mut()
        .filter(|writer| writer.connection_id == connection_id)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "connection is no longer active",
            )
        })?;
    writer.stream.write_all(encoded.as_bytes())?;
    writer.stream.flush()
}

fn send_to_active(active: &SharedWriter, message: &StreamMessage) -> io::Result<()> {
    let connection_id = lock(active, "active connection")?
        .as_ref()
        .map(|writer| writer.connection_id)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "no active Kindle connection")
        })?;
    send_to_connection(active, connection_id, message)
}

fn clear_active(active: &SharedWriter, connection_id: u64) -> io::Result<()> {
    let mut active_guard = lock(active, "active connection")?;
    if active_guard
        .as_ref()
        .is_some_and(|writer| writer.connection_id == connection_id)
    {
        *active_guard = None;
    }
    Ok(())
}

pub fn load_or_create_identity(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(identity) => validate_identity(identity.trim()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_identity(path),
        Err(error) => Err(error),
    }
}

fn create_identity(path: &Path) -> io::Result<String> {
    let identity = generate_hex_id()?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid_input("identity path must have a UTF-8 file name"))?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()));
    let create_result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        writeln!(file, "{identity}")?;
        file.sync_all()?;
        fs::hard_link(&temporary, path)
    })();
    let _ = fs::remove_file(&temporary);

    match create_result {
        Ok(()) => Ok(identity),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing = fs::read_to_string(path)?;
            validate_identity(existing.trim())
        }
        Err(error) => Err(error),
    }
}

fn validate_identity(identity: &str) -> io::Result<String> {
    format_stream_message(&StreamMessage::Welcome {
        companion: identity.to_string(),
    })
    .map_err(|error| invalid_data(format!("invalid companion identity: {error}")))?;
    Ok(identity.to_string())
}

fn lock<'a, T>(mutex: &'a Mutex<T>, label: &str) -> io::Result<std::sync::MutexGuard<'a, T>> {
    mutex
        .lock()
        .map_err(|_| io::Error::other(format!("{label} lock poisoned")))
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    const SESSION: &str = "0123456789abcdef0123456789abcdef";
    const COMPANION: &str = "fedcba9876543210fedcba9876543210";
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    static NETWORK_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn temporary_path(name: &str) -> PathBuf {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "rust-x11-hello-companion-{}-{sequence}-{name}",
            std::process::id()
        ))
    }

    #[test]
    fn cli_is_backward_compatible_but_rejects_invalid_configuration() {
        assert_eq!(
            Config::from_args(["companion"]).expect("default config"),
            Config {
                port: transport_protocol::DEFAULT_TCP_PORT,
                discovery_port: transport_protocol::DEFAULT_DISCOVERY_PORT,
                log_path: PathBuf::from(DEFAULT_LOG_FILE),
                identity_path: PathBuf::from(DEFAULT_IDENTITY_FILE),
            }
        );
        let configured = Config::from_args(["companion", "6000", "/tmp/log", "/tmp/id", "6001"])
            .expect("configured CLI");
        assert_eq!(configured.port, 6000);
        assert_eq!(configured.discovery_port, 6001);
        assert_eq!(configured.log_path, PathBuf::from("/tmp/log"));
        assert_eq!(configured.identity_path, PathBuf::from("/tmp/id"));
        assert!(Config::from_args(["companion", "0"]).is_err());
        assert!(Config::from_args(["companion", "not-a-port"]).is_err());
        assert!(Config::from_args(["companion", "5581", "log", "id", "0"]).is_err());
        assert!(Config::from_args(["companion", "5581", "log", "id", "5580", "extra"]).is_err());
    }

    #[test]
    fn identity_is_created_once_and_invalid_identity_is_rejected() {
        let path = temporary_path("identity");
        let first = load_or_create_identity(&path).expect("create identity");
        let second = load_or_create_identity(&path).expect("reload identity");
        assert_eq!(first, second);
        assert_eq!(first.len(), transport_protocol::HEX_ID_BYTES);
        fs::write(&path, "not-an-id\n").expect("replace identity for negative test");
        assert!(load_or_create_identity(&path).is_err());
        fs::remove_file(path).expect("remove identity fixture");
    }

    #[test]
    fn dedup_cache_is_bounded() {
        let mut cache = DedupCache::new(2);
        cache.insert(SESSION.to_string(), 1);
        cache.insert(SESSION.to_string(), 2);
        cache.insert(SESSION.to_string(), 3);
        assert!(!cache.contains(SESSION, 1));
        assert!(cache.contains(SESSION, 2));
        assert!(cache.contains(SESSION, 3));
    }

    #[test]
    fn handshake_ack_duplicate_and_legacy_behavior() {
        let _network_guard = NETWORK_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind test listener");
        let address = listener.local_addr().expect("listener address");
        let log_path = temporary_path("events.log");
        let server_log_path = log_path.clone();
        let server = std::thread::spawn(move || {
            let (stream, peer) = listener.accept().expect("accept test client");
            let active: SharedWriter = Arc::new(Mutex::new(Some(ActiveWriter {
                connection_id: 1,
                stream: stream.try_clone().expect("clone server writer"),
            })));
            let dedup = Arc::new(Mutex::new(DedupCache::new(DEDUP_CAPACITY)));
            handle_connection(
                stream,
                peer,
                1,
                &server_log_path,
                COMPANION,
                &active,
                &dedup,
            )
            .expect("handle test connection");
        });

        let mut client = TcpStream::connect(address).expect("connect test client");
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set timeout");
        let mut reader = BufReader::new(client.try_clone().expect("clone test reader"));

        client
            .write_all(format!("{}\n", "x".repeat(MAX_STREAM_FRAME_BYTES + 1)).as_bytes())
            .expect("write oversized frame");
        client
            .write_all(&[0xff, b'\n'])
            .expect("write invalid UTF-8 frame");
        client
            .write_all(format!("hello v=2 session={SESSION};\n").as_bytes())
            .expect("write unsupported hello");
        let hello = format_stream_message(&StreamMessage::Hello {
            session: SESSION.to_string(),
        })
        .expect("format hello");
        client.write_all(hello.as_bytes()).expect("write hello");
        let welcome = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
            .expect("read welcome")
            .expect("welcome frame");
        assert_eq!(
            parse_stream_message(&welcome).expect("parse welcome"),
            StreamMessage::Welcome {
                companion: COMPANION.to_string()
            }
        );

        let wrong_session = format_stream_message(&StreamMessage::Event {
            session: COMPANION.to_string(),
            seq: 99,
            action: "media.next".to_string(),
        })
        .expect("format wrong-session event");
        client
            .write_all(wrong_session.as_bytes())
            .expect("write wrong-session event");
        let event = format_stream_message(&StreamMessage::Event {
            session: SESSION.to_string(),
            seq: 7,
            action: "media.next".to_string(),
        })
        .expect("format event");
        client.write_all(event.as_bytes()).expect("write event");
        client
            .write_all(event.as_bytes())
            .expect("write duplicate event");
        for _ in 0..2 {
            let ack = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
                .expect("read ack")
                .expect("ack frame");
            assert_eq!(
                parse_stream_message(&ack).expect("parse ack"),
                StreamMessage::Ack {
                    session: SESSION.to_string(),
                    seq: 7,
                    status: AckStatus::Logged,
                }
            );
        }

        let legacy = format_stream_message(&StreamMessage::LegacyEvent {
            action: "tmux.work".to_string(),
        })
        .expect("format legacy");
        client.write_all(legacy.as_bytes()).expect("write legacy");
        client
            .shutdown(std::net::Shutdown::Both)
            .expect("shutdown client");
        server.join().expect("join test server");

        let mut log = String::new();
        File::open(&log_path)
            .expect("open event log")
            .read_to_string(&mut log)
            .expect("read event log");
        assert_eq!(log.matches("seq=7").count(), 1);
        assert_eq!(log.matches("seq=99").count(), 0);
        assert_eq!(log.matches("event action=tmux.work;").count(), 1);
        fs::remove_file(log_path).expect("remove event log fixture");
    }

    #[test]
    fn discovery_echoes_nonce_and_advertises_tcp_source_address() {
        let _network_guard = NETWORK_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let server = UdpSocket::bind(("127.0.0.1", 0)).expect("bind discovery server");
        let server_address = server.local_addr().expect("server address");
        let responder = std::thread::spawn(move || {
            let mut buffer = [0_u8; MAX_DISCOVERY_DATAGRAM_BYTES + 1];
            let (length, peer) = server.recv_from(&mut buffer).expect("receive discover");
            let nonce = match parse_discovery_message(&buffer[..length]).expect("parse discover") {
                DiscoveryMessage::Discover { nonce } => nonce,
                DiscoveryMessage::Offer { .. } => panic!("unexpected offer"),
            };
            let offer = format_discovery_message(&DiscoveryMessage::Offer {
                nonce,
                companion: COMPANION.to_string(),
                port: 6000,
            })
            .expect("format offer");
            server
                .send_to(offer.as_bytes(), peer)
                .expect("send discovery offer");
        });

        let client = UdpSocket::bind(("127.0.0.1", 0)).expect("bind discovery client");
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set discovery timeout");
        let discover = format_discovery_message(&DiscoveryMessage::Discover {
            nonce: SESSION.to_string(),
        })
        .expect("format discover");
        client
            .send_to(discover.as_bytes(), server_address)
            .expect("send discover");
        let mut buffer = [0_u8; MAX_DISCOVERY_DATAGRAM_BYTES + 1];
        let (length, source) = client.recv_from(&mut buffer).expect("receive offer");
        assert_eq!(source.ip(), server_address.ip());
        assert_eq!(
            parse_discovery_message(&buffer[..length]).expect("parse offer"),
            DiscoveryMessage::Offer {
                nonce: SESSION.to_string(),
                companion: COMPANION.to_string(),
                port: 6000,
            }
        );
        responder.join().expect("join discovery responder");
    }

    #[test]
    fn display_and_ack_writes_remain_separate_frames() {
        let _network_guard = NETWORK_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind writer fixture");
        let address = listener.local_addr().expect("writer fixture address");
        let client = TcpStream::connect(address).expect("connect writer fixture");
        let (server, _) = listener.accept().expect("accept writer fixture");
        let active: SharedWriter = Arc::new(Mutex::new(Some(ActiveWriter {
            connection_id: 7,
            stream: server,
        })));

        let ack_writer = Arc::clone(&active);
        let ack = std::thread::spawn(move || {
            send_to_connection(
                &ack_writer,
                7,
                &StreamMessage::Ack {
                    session: SESSION.to_string(),
                    seq: 3,
                    status: AckStatus::Logged,
                },
            )
            .expect("send ack frame");
        });
        let display_writer = Arc::clone(&active);
        let display = std::thread::spawn(move || {
            send_to_connection(
                &display_writer,
                7,
                &StreamMessage::Display {
                    text: "hello world".to_string(),
                },
            )
            .expect("send display frame");
        });
        ack.join().expect("join ack writer");
        display.join().expect("join display writer");

        let mut reader = BufReader::new(client);
        let first = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
            .expect("read first serialized frame")
            .expect("first serialized frame");
        let second = read_bounded_line(&mut reader, MAX_STREAM_FRAME_BYTES)
            .expect("read second serialized frame")
            .expect("second serialized frame");
        let messages = [
            parse_stream_message(&first).expect("parse first serialized frame"),
            parse_stream_message(&second).expect("parse second serialized frame"),
        ];
        assert!(messages.iter().any(|message| matches!(
            message,
            StreamMessage::Ack {
                seq: 3,
                status: AckStatus::Logged,
                ..
            }
        )));
        assert!(messages.iter().any(|message| matches!(
            message,
            StreamMessage::Display { text } if text == "hello world"
        )));
    }
}
