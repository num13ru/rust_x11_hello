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
use std::sync::mpsc::{self, Sender};
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
fn paperspoon_port() -> u16 {
    std::env::var(COMPANION_PORT_ENV)
        .ok()
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
    stream: Option<TcpStream>,
    reader: Option<mpsc::Receiver<PaperspoonMsg>>,
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
fn paperspoon_addr() -> Result<SocketAddr> {
    match std::env::var(COMPANION_HOST_ENV) {
        // An explicit, non-empty override resolves directly; empty or
        // missing falls through to discovery.
        Ok(host) if !host.trim().is_empty() => (host, paperspoon_port())
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
            stream: None,
            reader: None,
            _reader_tx: None,
        }
    }

    /// Connect to PaperSpoon and start the reader thread.
    pub fn connect() -> Result<Self> {
        let addr = paperspoon_addr()?;
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
        let (tx, rx) = mpsc::channel::<PaperspoonMsg>();
        let reader_stream = stream
            .try_clone()
            .context("failed to clone PaperSpoon stream")?;
        let reader_tx = tx.clone();
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
        });
        Ok(Self {
            stream: Some(stream),
            reader: Some(rx),
            _reader_tx: Some(tx),
        })
    }

    /// Send one semantic activation over the persistent connection.
    ///
    /// If the connection is gone, tries to reconnect once; a PaperSpoon that
    /// is down costs a bounded connect timeout and the error is logged by the
    /// caller.
    pub fn send_action(&mut self, semantic_id: &str) -> Result<()> {
        let line = format_action_line(semantic_id);
        if self.stream.is_none() {
            self.reconnect()?;
        }
        let stream = self.stream.as_mut().context("PaperSpoon not connected")?;
        stream
            .write_all(line.as_bytes())
            .context("failed to write action to PaperSpoon")
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
                PaperspoonMsg::Display(text) => Some(text),
                PaperspoonMsg::Disconnected => {
                    // First poll after disconnect reports it once; the stream
                    // itself is cleared by reconnect on the next send.
                    None
                }
            })
    }

    fn reconnect(&mut self) -> Result<()> {
        let addr = paperspoon_addr()?;
        // Full connect: a fresh socket also gets a fresh reader thread, so
        // inbound display commands work after a reconnect too (the original
        // reconnect created a bare socket with no reader, silently dropping
        // every PaperSpoon->Kindle control line).
        *self = Self::connect_to(addr)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paperspoon_port_has_a_default() {
        assert_eq!(paperspoon_port(), PAPERSPOON_PORT);
    }

    #[test]
    fn explicit_host_env_resolves_directly() {
        // SAFETY: these tests run serially by default; the env var is
        // restored before the test returns.
        unsafe { std::env::set_var(COMPANION_HOST_ENV, "127.0.0.1") };
        let addr = paperspoon_addr().expect("explicit host must resolve");
        unsafe { std::env::remove_var(COMPANION_HOST_ENV) };
        assert_eq!(addr, SocketAddr::from(([127, 0, 0, 1], PAPERSPOON_PORT)));
    }

    #[test]
    fn reconnect_restores_reader_thread() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::time::Duration;

        // PaperSpoon that is down at launch: the first activation reconnects.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral");
        let addr = listener.local_addr().expect("local addr");
        // SAFETY: tests run serially; overrides are restored before returning.
        unsafe {
            std::env::set_var(COMPANION_HOST_ENV, addr.ip().to_string());
            std::env::set_var(COMPANION_PORT_ENV, addr.port().to_string());
        }
        let mut paperspoon = Paperspoon::disconnected();
        let reconnect_result = paperspoon.reconnect();
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
            if let Some(text) = paperspoon.poll_display() {
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
