//! PaperPad UDP discovery client.
//!
//! Binds UDP `0.0.0.0:5582`, sends a bounded number of broadcast DISCOVER
//! probes to `255.255.255.255:5580`, and returns the first valid HERE
//! endpoint (source IP + advertised TCP port). Bounded: never loops
//! indefinitely, finishes in a few seconds on failure.

use crate::discovery::{CLIENT_PORT, DISCOVERY_PORT, parse_here, select_endpoint};
use anyhow::{Context, Result};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// Number of discovery probes.
pub const PROBE_COUNT: u32 = 3;
/// Response window per probe.
pub const PROBE_WINDOW: Duration = Duration::from_millis(500);

/// Run zero-config discovery for PaperSpoon.
///
/// Returns the discovered `SocketAddr` (PaperSpoon IP + TCP port) or an
/// error describing the failure.
pub fn discover_paperspoon() -> Result<SocketAddr> {
    discover_paperspoon_to(SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT)))
}

/// Core discovery against a specific probe target (broadcast for the real
/// path, loopback for tests).
fn discover_paperspoon_to(target: SocketAddr) -> Result<SocketAddr> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, CLIENT_PORT)))
        .context("discovery bind address=0.0.0.0:5582")?;
    socket
        .set_broadcast(true)
        .context("discovery enable broadcast")?;
    socket
        .set_read_timeout(Some(PROBE_WINDOW))
        .context("discovery set read timeout")?;

    let nonce = fresh_nonce();
    eprintln!("discovery bind address=0.0.0.0:{CLIENT_PORT}");

    let mut responses: Vec<(SocketAddr, u16)> = Vec::new();
    let deadline = Instant::now() + PROBE_COUNT * PROBE_WINDOW;

    'probe: for attempt in 1..=PROBE_COUNT {
        let request = crate::discovery::format_discover(&nonce);
        eprintln!("discovery probe attempt={attempt} destination={target} nonce={nonce}");
        socket
            .send_to(request.as_bytes(), target)
            .context("discovery probe send")?;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let mut buffer = [0u8; 256];
            match socket.recv_from(&mut buffer) {
                Ok((count, source)) => {
                    eprintln!("discovery response from={source} bytes={count}");
                    let Some((response_nonce, port)) = parse_here(&buffer[..count]) else {
                        eprintln!("discovery response invalid from={source}");
                        continue;
                    };
                    if response_nonce != nonce {
                        eprintln!("discovery response nonce-mismatch from={source}");
                        continue;
                    }
                    eprintln!("discovery response valid endpoint={}:{port}", source.ip());
                    responses.push((source, port));
                    break 'probe;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    eprintln!("discovery probe window elapsed attempt={attempt}");
                    continue 'probe;
                }
                Err(error) => {
                    return Err(error).context("discovery receive");
                }
            }
        }
    }

    drop(socket);
    match select_endpoint(responses.into_iter()) {
        Ok(endpoint) => {
            eprintln!("discovery result endpoint={endpoint}");
            Ok(endpoint)
        }
        Err(crate::discovery::DiscoveryError::NoResponder) => Err(anyhow::anyhow!(
            "discovery failed: no valid response received"
        )),
        Err(crate::discovery::DiscoveryError::Ambiguous(endpoints)) => Err(anyhow::anyhow!(
            "discovery failed: multiple PaperSpoon responders: {endpoints:?}"
        )),
    }
}

/// Generate a fresh per-attempt nonce (hex-encoded pseudo-random bytes).
///
/// Not cryptographic; only needs to distinguish this attempt from stale or
/// unrelated packets.
fn fresh_nonce() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{nanos:x}{pid:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;

    #[test]
    fn client_parses_valid_here() {
        let response = crate::discovery::format_discover("deadbeef");
        assert!(response.starts_with(crate::discovery::DISCOVER_PREFIX));
    }

    #[test]
    fn client_discovers_responder_over_udp() {
        // Real responder socket on an ephemeral loopback port: reads the
        // DISCOVER nonce, replies HERE with TCP port 5581.
        let responder = UdpSocket::bind("127.0.0.1:0").expect("bind responder");
        let responder_port = responder.local_addr().unwrap().port();
        let responder_thread = std::thread::spawn(move || {
            let mut buffer = [0u8; 256];
            let (count, source) = responder.recv_from(&mut buffer).expect("recv discover");
            let datagram = std::str::from_utf8(&buffer[..count]).unwrap();
            let nonce = datagram
                .strip_prefix(crate::discovery::DISCOVER_PREFIX)
                .unwrap()
                .trim_end()
                .to_string();
            let here = format!("{} {nonce} 5581\n", crate::discovery::HERE_PREFIX);
            responder
                .send_to(here.as_bytes(), source)
                .expect("send here");
        });

        // Exercise the real client core against the loopback responder.
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, responder_port));
        let endpoint = discover_paperspoon_to(target).expect("discover");
        responder_thread.join().expect("responder thread");

        assert_eq!(endpoint.ip(), Ipv4Addr::LOCALHOST);
        assert_eq!(endpoint.port(), 5581);
    }
}
