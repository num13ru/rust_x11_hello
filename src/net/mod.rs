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

use crate::proto::{format_action_line, parse_display_command};

pub mod discover;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// PaperSpoon address: legacy USBNetwork static host. This device (a
/// Paperwhite 6) cannot run USBNetwork — no maintained package accepts it
/// (see `docs/usbnetwork-pw2-report.md`) — so runs set
/// `RUST_X11_HELLO_COMPANION` to the Mac's LAN address over Wi-Fi.
pub const PAPERSPOON_PORT: u16 = 5581;

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
        let reader_tx = tx.clone();
        let reader_wake = wake_tx.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(reader_stream);
            for line in reader.lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => break,
                };
                if let Some(text) = parse_display_command(&line) {
                    let _ = reader_tx.send(PaperspoonMsg::Display(text));
                }
            }
            let _ = reader_tx.send(PaperspoonMsg::Disconnected);
            let _ = reader_wake.send(());
        });

        // Reconnector: wake on EOF, clear slot, retry connect until success,
        // install fresh socket + reader, then wait for the next EOF.
        {
            let shared = Arc::clone(&shared);
            let reader_tx = tx.clone();
            std::thread::spawn(move || {
                loop {
                    let _ = wake_rx.recv();
                    {
                        *shared.lock().expect("shared stream lock") = None;
                    }
                    loop {
                        match TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT) {
                            Ok(new_stream) => {
                                let _ = new_stream.set_nodelay(true);
                                {
                                    let mut guard = shared.lock().expect("shared stream lock");
                                    *guard = Some(new_stream);
                                }
                                let fresh_reader = shared
                                    .lock()
                                    .expect("shared stream lock")
                                    .as_ref()
                                    .expect("connected stream")
                                    .try_clone()
                                    .expect("fresh clone");
                                let fresh_reader_tx = reader_tx.clone();
                                let fresh_reader_wake = wake_tx.clone();
                                std::thread::spawn(move || {
                                    let reader = BufReader::new(fresh_reader);
                                    for line in reader.lines() {
                                        let line = match line {
                                            Ok(line) => line,
                                            Err(_) => break,
                                        };
                                        if let Some(text) = parse_display_command(&line) {
                                            let _ =
                                                fresh_reader_tx.send(PaperspoonMsg::Display(text));
                                        }
                                    }
                                    let _ = fresh_reader_tx.send(PaperspoonMsg::Disconnected);
                                    let _ = fresh_reader_wake.send(());
                                });
                                break;
                            }
                            Err(_) => std::thread::sleep(TCP_CONNECT_TIMEOUT),
                        }
                    }
                }
            });
        }

        Ok(Self {
            stream: shared,
            reader: Some(rx),
            _reader_tx: Some(tx),
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

#[cfg(test)]
mod tests {
    use super::*;

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
        use std::io::Write;
        use std::net::TcpListener;
        use std::time::Duration;

        // Start a listener (PaperSpoon), connect, then stop it to force EOF.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        let mut paperspoon = Paperspoon::connect_to(addr).expect("connect");
        let (mut first, _) = listener.accept().expect("accept first");
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
        // Wait for the reconnector to connect to `addr` again.
        listener3.set_nonblocking(true).expect("nonblocking");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut reconnected = false;
        while std::time::Instant::now() < deadline {
            if let Ok((mut conn, _)) = listener3.accept() {
                writeln!(conn, "display again").expect("write again");
                // The reconnector installs a fresh reader; the display text
                // must be delivered to poll_display.
                let dl = std::time::Instant::now() + Duration::from_secs(2);
                loop {
                    if paperspoon.poll_display() == Some("again".to_string()) {
                        reconnected = true;
                        break;
                    }
                    if std::time::Instant::now() >= dl {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                if reconnected {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(reconnected, "reconnector did not restore the connection");
    }
}
