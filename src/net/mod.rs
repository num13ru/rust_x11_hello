//! Bounded TCP transport for semantic activation events.
//!
//! The Kindle acts as a one-shot listener: each activation connects to the
//! companion port with a very short connect timeout, writes one
//! newline-terminated line (`event action=<semantic-id>`), and disconnects.
//! A client that is down costs a bounded `TCP_CONNECT_TIMEOUT`, so the X11
//! event loop drain still shows on-device presses.
//! This keeps the send path allocation-free apart from the fixed connection
//! attempt, and it needs no transport success to keep the UI working: write
//! failures are logged and return `Ok`.
use anyhow::{Context, Result};
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::proto::format_action_line;

/// Companion address: standard USBNetwork static host.
pub const COMPANION_HOST: &str = "192.168.15.201";
pub const COMPANION_PORT: u16 = 5581;

const TCP_CONNECT_TIMEOUT: Duration = Duration::from_millis(150);

/// Send one semicolon-terminated protocol line to the companion.
pub fn send_semantic_action(semantic_id: &str) -> Result<()> {
    let addr = (COMPANION_HOST, COMPANION_PORT)
        .to_socket_addrs()
        .context("failed to resolve companion address")?
        .next()
        .context("companion address resolved to nothing")?;

    let mut stream = TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT)
        .context("failed to connect to companion")?;
    let _ = stream.set_nodelay(true);
    let line = format_action_line(semantic_id);
    stream
        .write_all(line.as_bytes())
        .context("failed to write action to companion")?;
    Ok(())
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
}
