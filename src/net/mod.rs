//! Persistent TCP transport for semantic activation events and PaperSpoon
//! control.
//!
//! The Kindle connects out to PaperSpoon once at launch and keeps the
//! connection for the run. Each activation writes one newline-terminated
//! `event action=<semantic-id>;` line over the connection. A reader thread
//! consumes inbound PaperSpoon lines (control commands such as
//! `display <text>`) and pushes them into a channel the X11 event loop
//! drains between events, so the loop is never blocked on the network.
//!
//! If the connection drops, the state goes disconnected, the next activation
//! retries the connect, and a disconnected PaperSpoon never breaks the X11
//! event loop or the on-device activation log.

use paper_protocol::{format_action_line, parse_display_command};

pub mod discover;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// PaperSpoon address: legacy USBNetwork static host. This device (a
/// Paperwhite 6) cannot run USBNetwork — no maintained package accepts it
/// (see `docs/usbnetwork-pw2-report.md`) — so runs set
/// `RUST_X11_HELLO_COMPANION` to the Mac's LAN address over Wi-Fi.
pub const PAPERSPOON_PORT: u16 = paper_protocol::DEFAULT_TCP_PORT;

/// Environment override for the PaperSpoon host, required for
/// non-USBNetwork transports (this Kindle runs the Wi-Fi peer's address,
/// because it cannot run USBNetwork). The `COMPANION` name is retained for
/// compatibility with existing KUAL launch environments.
const COMPANION_HOST_ENV: &str = "RUST_X11_HELLO_COMPANION";
/// Environment override for the PaperSpoon port (defaults to
/// [`PAPERSPOON_PORT`]); used by tests to avoid clashing with other
/// listeners.
const COMPANION_PORT_ENV: &str = "RUST_X11_HELLO_COMPANION_PORT";

const TCP_CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
fn paperspoon_port(value: Option<&str>) -> u16 {
    value
        .and_then(|value| value.parse().ok())
        .unwrap_or(PAPERSPOON_PORT)
}

/// Messages a PaperSpoon reader thread can deliver to the event loop.
#[derive(Debug)]
pub enum PaperspoonMsg {
    Display(String),
    Disconnected,
}

/// A persistent outbound connection to PaperSpoon with an inbound message
/// queue drained by the X11 event loop.
pub struct Paperspoon {
    /// Shared with the reconnector thread; `None` = disconnected.
    stream: Arc<Mutex<Option<TcpStream>>>,
    reader: Option<Receiver<PaperspoonMsg>>,
    /// Writer half cloned into the reader thread; kept alive here so writes
    /// from the main thread and reads share one socket.
    _reader_tx: Option<Sender<PaperspoonMsg>>,
    wake_tx: Option<Sender<()>>,
    stopping: Arc<AtomicBool>,
    reconnector: Option<JoinHandle<()>>,
}

fn spawn_reader(
    stream: TcpStream,
    message_tx: Sender<PaperspoonMsg>,
    wake_tx: Sender<()>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(_) => break,
            };
            if let Some(text) = parse_display_command(&line) {
                let _ = message_tx.send(PaperspoonMsg::Display(text));
            }
        }
        let _ = message_tx.send(PaperspoonMsg::Disconnected);
        let _ = wake_tx.send(());
    })
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
    /// Create a disconnected PaperSpoon that will attempt to connect on the
    /// next activation. Startup failures are not fatal to the X11 loop.
    pub fn disconnected() -> Self {
        Self {
            stream: Arc::new(Mutex::new(None)),
            reader: None,
            _reader_tx: None,
            wake_tx: None,
            stopping: Arc::new(AtomicBool::new(false)),
            reconnector: None,
        }
    }

    /// Connect to PaperSpoon and start the reader thread.
    pub fn connect() -> Result<Self> {
        let host = std::env::var(COMPANION_HOST_ENV).ok();
        let port = std::env::var(COMPANION_PORT_ENV).ok();
        let addr = paperspoon_addr(host.as_deref(), paperspoon_port(port.as_deref()))?;
        Self::connect_to(addr)
    }

    /// Connect to a specific PaperSpoon address and start the reader thread.
    ///
    /// Internal and `pub(crate)`: tests inject an ephemeral listener this
    /// way so they never depend on process-global env vars.
    pub(crate) fn connect_to(addr: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT)
            .context("failed to connect to PaperSpoon")?;
        let _ = stream.set_nodelay(true);

        // The socket is shared with a reconnector thread. When the reader
        // hits EOF, it clears the shared slot and wakes the reconnector,
        // which retries until PaperSpoon is reachable again and swaps in a
        // fresh socket — proactive auto-reconnect without user input.
        let shared = Arc::new(Mutex::new(Some(stream)));
        let (tx, rx) = mpsc::channel::<PaperspoonMsg>();
        let (wake_tx, wake_rx) = mpsc::channel::<()>();

        let reader_stream = shared
            .lock()
            .expect("shared stream lock")
            .as_ref()
            .expect("connected stream")
            .try_clone()
            .context("failed to clone PaperSpoon stream")?;
        let reader_handle = spawn_reader(reader_stream, tx.clone(), wake_tx.clone());

        // Reconnector: wake on EOF, clear slot, retry connect until success,
        // install fresh socket + reader, then wait for the next EOF.
        let worker_shared = Arc::clone(&shared);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let worker_tx = tx.clone();
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
                {
                    let mut guard = worker_shared.lock().expect("shared stream lock");
                    if let Some(stream) = guard.take() {
                        let _ = stream.shutdown(Shutdown::Both);
                    }
                }
                if worker_stopping.load(Ordering::Acquire) {
                    break;
                }
                loop {
                    if worker_stopping.load(Ordering::Acquire) {
                        break 'reconnector;
                    }
                    match TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT) {
                        Ok(new_stream) => {
                            let _ = new_stream.set_nodelay(true);
                            let fresh_reader = new_stream.try_clone().expect("fresh clone");
                            {
                                let mut guard = worker_shared.lock().expect("shared stream lock");
                                if worker_stopping.load(Ordering::Acquire) {
                                    let _ = new_stream.shutdown(Shutdown::Both);
                                    break 'reconnector;
                                }
                                *guard = Some(new_stream);
                            }
                            active_reader = Some(spawn_reader(
                                fresh_reader,
                                worker_tx.clone(),
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
            stream: shared,
            reader: Some(rx),
            _reader_tx: Some(tx),
            wake_tx: Some(wake_tx),
            stopping,
            reconnector: Some(reconnector),
        })
    }

    /// Send one semantic activation over the persistent connection.
    ///
    /// Writes to the shared stream; if the reconnector has not yet finished
    /// restoring it, the write fails fast and the caller logs it (the
    /// reconnector keeps retrying in the background).
    pub fn send_action(&mut self, semantic_id: &str) -> Result<()> {
        let line = format_action_line(semantic_id);
        let mut write_stream = {
            let guard = self.stream.lock().expect("shared stream lock");
            guard
                .as_ref()
                .context("PaperSpoon not connected")?
                .try_clone()
                .context("failed to clone PaperSpoon stream")?
        };
        write_stream
            .write_all(line.as_bytes())
            .context("failed to write action to PaperSpoon")?;
        Ok(())
    }

    /// Drain any display commands received since the last call.
    ///
    /// Returns the first pending display text, if any. The event loop calls
    /// this between X11 events; it never blocks. `Disconnected` messages are
    /// consumed here so the reconnector's channel stays empty between cycles.
    pub fn poll_display(&mut self) -> Option<String> {
        let rx = self.reader.as_ref()?;
        loop {
            match rx.try_recv() {
                Ok(PaperspoonMsg::Display(text)) => return Some(text),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }
}

impl Drop for Paperspoon {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);

        if let Some(stream) = self.stream.lock().expect("shared stream lock").take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(wake_tx) = self.wake_tx.take() {
            let _ = wake_tx.send(());
        }
        if let Some(reconnector) = self.reconnector.take() {
            let _ = reconnector.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accept_before(listener: &std::net::TcpListener, timeout: Duration) -> TcpStream {
        listener.set_nonblocking(true).expect("set nonblocking");
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline, "accept timed out");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        }
    }

    #[test]
    fn paperspoon_port_has_a_default() {
        assert_eq!(paperspoon_port(None), PAPERSPOON_PORT);
        assert_eq!(paperspoon_port(Some("not-a-port")), PAPERSPOON_PORT);
        assert_eq!(paperspoon_port(Some("6000")), 6000);
    }

    #[test]
    fn explicit_host_resolves_directly() {
        let addr = paperspoon_addr(Some("127.0.0.1"), 6000).expect("explicit host must resolve");
        assert_eq!(addr, SocketAddr::from(([127, 0, 0, 1], 6000)));
    }

    #[test]
    fn auto_reconnects_when_paperspoon_returns() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::Duration;

        // Start a listener (PaperSpoon), connect, then stop it to force EOF.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        let mut paperspoon = Paperspoon::connect_to(addr).expect("connect");
        let mut first = accept_before(&listener, Duration::from_secs(2));
        writeln!(first, "display first").expect("write first");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if paperspoon.poll_display() == Some("first".to_string()) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "no first display");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(first);
        drop(listener); // Force EOF: the reconnector must notice.

        // A new PaperSpoon appears; the reconnector should restore the stream
        // without any send_action call from us. The reconnector loops forever
        // retrying `addr` (now free), so rebinding the SAME port and
        // accepting proves the auto-restore.
        let listener3 = TcpListener::bind(addr).expect("rebind same port");
        let mut reconnected_peer = accept_before(&listener3, Duration::from_secs(5));
        writeln!(reconnected_peer, "display again").expect("write again");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if paperspoon.poll_display() == Some("again".to_string()) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no reconnected display"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        // Drop must stop and join the reader even while its peer remains idle.
        reconnected_peer
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set peer deadline");
        drop(paperspoon);
        let mut byte = [0];
        assert_eq!(reconnected_peer.read(&mut byte).expect("read EOF"), 0);
    }
}
