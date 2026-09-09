//! PaperPad UDP discovery client.
//!
//! Binds UDP `0.0.0.0:5582`, sends a bounded number of broadcast DISCOVER
//! probes to `255.255.255.255:5580`, and collects valid HERE endpoints
//! (source IP + advertised TCP port). Bounded: never loops indefinitely and
//! finishes in a few seconds on failure or ambiguity.

use crate::discovery::select_endpoint;
use anyhow::{Context, Result};
use paper_protocol::{DISCOVERY_CLIENT_PORT, DISCOVERY_PORT, format_discover, parse_here};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// Number of discovery probes.
pub const PROBE_COUNT: u32 = 3;
/// Response window per probe.
pub const PROBE_WINDOW: Duration = Duration::from_millis(500);

/// Run zero-config discovery for PaperSpoon.
///
/// Returns the one distinct discovered `SocketAddr` (PaperSpoon IP + TCP
/// port), or an error describing absence, ambiguity, or an I/O failure.
pub fn discover_paperspoon() -> Result<SocketAddr> {
    discover_paperspoon_from_to(
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_CLIENT_PORT)),
        SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT)),
    )
}

/// Core discovery with explicit bind and probe addresses (fixed broadcast
/// addresses for the real path, ephemeral loopback addresses for tests).
fn discover_paperspoon_from_to(bind_addr: SocketAddr, target: SocketAddr) -> Result<SocketAddr> {
    discover_paperspoon_with_schedule(bind_addr, target, PROBE_COUNT, PROBE_WINDOW)
}

fn discover_paperspoon_with_schedule(
    bind_addr: SocketAddr,
    target: SocketAddr,
    probe_count: u32,
    probe_window: Duration,
) -> Result<SocketAddr> {
    let socket = UdpSocket::bind(bind_addr)
        .with_context(|| format!("discovery bind address={bind_addr}"))?;
    socket
        .set_broadcast(true)
        .context("discovery enable broadcast")?;

    let nonce = fresh_nonce();
    eprintln!(
        "discovery bind address={}",
        socket
            .local_addr()
            .context("discovery read local address")?
    );

    let mut responses: Vec<(SocketAddr, u16)> = Vec::new();
    let deadline = Instant::now() + probe_count * probe_window;

    'probe: for attempt in 1..=probe_count {
        if Instant::now() >= deadline {
            break;
        }

        let request = format_discover(&nonce);
        eprintln!("discovery probe attempt={attempt} destination={target} nonce={nonce}");
        socket
            .send_to(request.as_bytes(), target)
            .context("discovery probe send")?;

        let probe_deadline = std::cmp::min(Instant::now() + probe_window, deadline);
        loop {
            let remaining = probe_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                eprintln!("discovery probe window elapsed attempt={attempt}");
                continue 'probe;
            }
            socket
                .set_read_timeout(Some(remaining))
                .context("discovery set read timeout")?;

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
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
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
    use paper_protocol::{DEFAULT_TCP_PORT, format_here, parse_discover};

    const TEST_PROBE_WINDOW: Duration = Duration::from_millis(100);

    fn discover_with_offers(ports: &[u16]) -> Result<SocketAddr> {
        let responder = UdpSocket::bind("127.0.0.1:0").expect("bind responder");
        responder
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set responder deadline");
        let responder_port = responder.local_addr().unwrap().port();
        let ports = ports.to_vec();
        let responder_thread = std::thread::spawn(move || -> std::io::Result<()> {
            let mut buffer = [0u8; 256];
            let (count, source) = responder.recv_from(&mut buffer)?;
            let nonce = parse_discover(&buffer[..count]).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid discovery request")
            })?;
            for port in ports {
                let here = format_here(&nonce, port);
                responder.send_to(here.as_bytes(), source)?;
            }
            Ok(())
        });

        // Exercise the real client core with an ephemeral client port so
        // parallel test processes cannot collide on the production port.
        let bind_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, responder_port));
        let endpoint = discover_paperspoon_with_schedule(bind_addr, target, 1, TEST_PROBE_WINDOW);
        let responder_result = responder_thread.join().expect("responder thread panicked");
        responder_result.expect("responder failed");
        endpoint
    }

    #[test]
    fn client_discovers_responder_over_udp() {
        let endpoint = discover_with_offers(&[DEFAULT_TCP_PORT]).expect("discover");

        assert_eq!(endpoint.ip(), Ipv4Addr::LOCALHOST);
        assert_eq!(endpoint.port(), DEFAULT_TCP_PORT);
    }

    #[test]
    fn duplicate_endpoint_responses_are_accepted() {
        let endpoint =
            discover_with_offers(&[DEFAULT_TCP_PORT, DEFAULT_TCP_PORT]).expect("discover");

        assert_eq!(
            endpoint,
            SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_TCP_PORT))
        );
    }

    #[test]
    fn distinct_endpoint_responses_are_rejected() {
        let error = discover_with_offers(&[DEFAULT_TCP_PORT, DEFAULT_TCP_PORT + 1])
            .expect_err("distinct endpoint offers should be ambiguous");

        assert!(error.to_string().contains("multiple PaperSpoon responders"));
    }
}
