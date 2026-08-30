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

/// Environment override for the companion host, for non-USBNetwork transports
/// (e.g., the Kindle already on Wi-Fi, where the Mac's local IP differs).
const COMPANION_HOST_ENV: &str = "RUST_X11_HELLO_COMPANION";

/// Resolve the companion host, honoring the `RUST_X11_HELLO_COMPANION`
/// override (e.g., a Wi-Fi peer's address) and falling back to the
/// USBNetwork static host when unset.
fn companion_host() -> String {
    std::env::var(COMPANION_HOST_ENV).unwrap_or_else(|_| COMPANION_HOST.to_string())
}

/// Resolve the companion address tuple from the configured host and port.
fn companion_addr() -> Result<std::net::SocketAddr> {
    (companion_host(), COMPANION_PORT)
        .to_socket_addrs()
        .context("failed to resolve companion address")?
        .next()
        .context("companion address resolved to nothing")
}

const TCP_CONNECT_TIMEOUT: Duration = Duration::from_millis(150);

/// Send one semicolon-terminated protocol line to the companion.
pub fn send_semantic_action(semantic_id: &str) -> Result<()> {
    let addr = companion_addr()?;

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
}
