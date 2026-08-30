//! Persistent TCP transport for semantic activation events and companion
//! control.
//!
//! The Kindle connects out to the companion once at launch and keeps the
//! connection for the run. Each activation writes one newline-terminated
//! `event action=<semantic-id>;` line over the connection. A reader thread
//! consumes inbound companion lines (control commands such as
//! `display <text>`) and pushes them into a channel the X11 event loop
//! drains between events, so the loop is never blocked on the network.
//!
//! If the connection drops, the state goes disconnected, the next activation
//! retries the connect, and a disconnected companion never breaks the X11
//! event loop or the on-device activation log.

use crate::proto::{format_action_line, parse_display_command};
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

/// Companion address: standard USBNetwork static host. This device (a
/// Paperwhite 6) cannot run USBNetwork — no maintained package accepts it
/// (see `docs/usbnetwork-pw2-report.md`) — so runs set
/// `RUST_X11_HELLO_COMPANION` to the Mac's LAN address over Wi-Fi.
pub const COMPANION_HOST: &str = "192.168.15.201";
pub const COMPANION_PORT: u16 = 5581;

/// Environment override for the companion host, required for non-USBNetwork
/// transports (this Kindle runs the Wi-Fi peer's address, because it cannot
/// run USBNetwork).
const COMPANION_HOST_ENV: &str = "RUST_X11_HELLO_COMPANION";
/// Environment override for the companion port (defaults to
/// [`COMPANION_PORT`]); used by tests to avoid clashing with other listeners.
const COMPANION_PORT_ENV: &str = "RUST_X11_HELLO_COMPANION_PORT";

const TCP_CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
fn companion_port() -> u16 {
    std::env::var(COMPANION_PORT_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(COMPANION_PORT)
}

/// Messages a companion reader thread can deliver to the event loop.
#[derive(Debug)]
pub enum CompanionMsg {
    Display(String),
    Disconnected,
}

/// A persistent outbound connection to the companion with an inbound message
/// queue drained by the X11 event loop.
pub struct Companion {
    stream: Option<TcpStream>,
    reader: Option<mpsc::Receiver<CompanionMsg>>,
    /// Writer half cloned into the reader thread; kept alive here so writes
    /// from the main thread and reads share one socket.
    _reader_tx: Option<Sender<CompanionMsg>>,
}

/// Resolve the companion host, honoring the `RUST_X11_HELLO_COMPANION`
/// override (e.g., a Wi-Fi peer's address) and falling back to the
/// USBNetwork static host when unset. The fallback exists only for
/// compatibility; on this PW6 the override is the working transport.
fn companion_host() -> String {
    std::env::var(COMPANION_HOST_ENV).unwrap_or_else(|_| COMPANION_HOST.to_string())
}

/// Resolve the companion address tuple from the configured host and port.
fn companion_addr() -> Result<SocketAddr> {
    (companion_host(), companion_port())
        .to_socket_addrs()
        .context("failed to resolve companion address")?
        .next()
        .context("companion address resolved to nothing")
}

impl Companion {
    /// Create a disconnected companion that will attempt to connect on the
    /// next activation. Startup failures are not fatal to the X11 loop.
    pub fn disconnected() -> Self {
        Self {
            stream: None,
            reader: None,
            _reader_tx: None,
        }
    }

    /// Connect to the companion and start the reader thread.
    pub fn connect() -> Result<Self> {
        let addr = companion_addr()?;
        Self::connect_to(addr)
    }

    /// Connect to a specific companion address and start the reader thread.
    ///
    /// Internal and `pub(crate)`: tests inject an ephemeral listener this
    /// way so they never depend on process-global env vars.
    pub(crate) fn connect_to(addr: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT)
            .context("failed to connect to companion")?;
        let _ = stream.set_nodelay(true);
        let (tx, rx) = mpsc::channel::<CompanionMsg>();
        let reader_stream = stream
            .try_clone()
            .context("failed to clone companion stream")?;
        let reader_tx = tx.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(reader_stream);
            for line in reader.lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => break,
                };
                if let Some(text) = parse_display_command(&line) {
                    let _ = reader_tx.send(CompanionMsg::Display(text));
                }
            }
            let _ = reader_tx.send(CompanionMsg::Disconnected);
        });
        Ok(Self {
            stream: Some(stream),
            reader: Some(rx),
            _reader_tx: Some(tx),
        })
    }

    /// Send one semantic activation over the persistent connection.
    ///
    /// If the connection is gone, tries to reconnect once; a companion that
    /// is down costs a bounded connect timeout and the error is logged by the
    /// caller.
    pub fn send_action(&mut self, semantic_id: &str) -> Result<()> {
        let line = format_action_line(semantic_id);
        if self.stream.is_none() {
            self.reconnect()?;
        }
        let stream = self.stream.as_mut().context("companion not connected")?;
        stream
            .write_all(line.as_bytes())
            .context("failed to write action to companion")
    }

    /// Drain any display commands received since the last call.
    ///
    /// Returns the first pending display text, if any. The event loop calls
    /// this between X11 events; it never blocks.
    pub fn poll_display(&self) -> Option<String> {
        self.reader
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
            .and_then(|msg| match msg {
                CompanionMsg::Display(text) => Some(text),
                CompanionMsg::Disconnected => {
                    // First poll after disconnect reports it once; the stream
                    // itself is cleared by reconnect on the next send.
                    None
                }
            })
    }

    fn reconnect(&mut self) -> Result<()> {
        let addr = companion_addr()?;
        // Full connect: a fresh socket also gets a fresh reader thread, so
        // inbound display commands work after a reconnect too (the original
        // reconnect created a bare socket with no reader, silently dropping
        // every companion->Kindle control line).
        *self = Self::connect_to(addr)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companion_address_resolves() {
        let addrs: Vec<std::net::SocketAddr> = (COMPANION_HOST, COMPANION_PORT)
            .to_socket_addrs()
            .expect("static host must resolve")
            .collect();
        assert!(!addrs.is_empty());
    }

    #[test]
    fn companion_host_env_override_is_isolated() {
        // SAFETY: these tests run serially by default; the env var is
        // restored before the test returns.
        unsafe { std::env::set_var(COMPANION_HOST_ENV, "10.0.0.99") };
        let overridden = companion_host();
        unsafe { std::env::remove_var(COMPANION_HOST_ENV) };
        let default = companion_host();
        assert_eq!(overridden, "10.0.0.99");
        assert_eq!(default, COMPANION_HOST);
    }

    #[test]
    fn reconnect_restores_reader_thread() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::time::Duration;

        // Companion that is down at launch: the first activation reconnects.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral");
        let addr = listener.local_addr().expect("local addr");
        // SAFETY: tests run serially; overrides are restored before returning.
        unsafe {
            std::env::set_var(COMPANION_HOST_ENV, addr.ip().to_string());
            std::env::set_var(COMPANION_PORT_ENV, addr.port().to_string());
        }
        let mut companion = Companion::disconnected();
        let reconnect_result = companion.reconnect();
        unsafe {
            std::env::remove_var(COMPANION_HOST_ENV);
            std::env::remove_var(COMPANION_PORT_ENV);
        }
        reconnect_result.expect("reconnect");

        let (mut conn, _) = listener.accept().expect("accept");
        // Sending over the persistent connection must reach a live reader
        // thread even though the socket was created by reconnect(), not
        // connect_to(). Without the fix the display text is silently dropped
        // and poll_display stays None.
        writeln!(conn, "display hello").expect("write display");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(text) = companion.poll_display() {
                assert_eq!(text, "hello");
                return;
            }
            if std::time::Instant::now() >= deadline {
                panic!("display command not delivered over reconnected socket");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
