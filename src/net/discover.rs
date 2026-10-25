//! PaperPad UDP discovery client.
//!
//! Binds UDP `0.0.0.0:5582`, sends a bounded number of broadcast DISCOVER
//! probes to `255.255.255.255:5580`, and collects valid HERE endpoints
//! (source IP + advertised TCP port). Bounded: never loops indefinitely and
//! finishes in a few seconds on failure or ambiguity.

use crate::discovery::select_endpoint;
use anyhow::{Context, Result};
use paper_protocol::{DISCOVERY_CLIENT_PORT, DISCOVERY_PORT, format_discover, parse_here};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::ptr;
use std::time::{Duration, Instant};

/// Number of discovery probes.
pub const PROBE_COUNT: u32 = 3;
/// Response window per probe.
pub const PROBE_WINDOW: Duration = Duration::from_millis(500);

struct IfAddrs(*mut libc::ifaddrs);

impl Drop for IfAddrs {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `getifaddrs` initialized this list and ownership remains
            // with this guard until it is dropped exactly once.
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
}

fn directed_broadcast(address: Ipv4Addr, netmask: Ipv4Addr) -> Option<Ipv4Addr> {
    let broadcast = Ipv4Addr::from(u32::from(address) | !u32::from(netmask));
    (broadcast != address && broadcast != Ipv4Addr::BROADCAST).then_some(broadcast)
}

fn interface_directed_broadcasts() -> io::Result<Vec<Ipv4Addr>> {
    let mut head = ptr::null_mut();
    // SAFETY: `head` is a valid out-pointer. A successful allocation is
    // immediately owned by `IfAddrs`, which releases it with `freeifaddrs`.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let _guard = IfAddrs(head);
    let mut current = head;
    let mut broadcasts = Vec::new();

    while !current.is_null() {
        // SAFETY: nodes belong to the live `IfAddrs` list. The kernel-provided
        // sockaddr pointers are checked before casting an AF_INET entry.
        let interface = unsafe { &*current };
        let flags = interface.ifa_flags as libc::c_int;
        let usable = flags & libc::IFF_UP != 0
            && flags & libc::IFF_BROADCAST != 0
            && flags & libc::IFF_LOOPBACK == 0;
        if usable && !interface.ifa_addr.is_null() && !interface.ifa_netmask.is_null() {
            // SAFETY: both pointers are non-null and the family is read from
            // the common sockaddr prefix before either sockaddr_in cast.
            let family = unsafe { (*interface.ifa_addr).sa_family as libc::c_int };
            if family == libc::AF_INET {
                // SAFETY: AF_INET guarantees sockaddr_in layout for the
                // address; the paired netmask has the same family/layout.
                let address = unsafe { &*(interface.ifa_addr.cast::<libc::sockaddr_in>()) };
                let netmask = unsafe { &*(interface.ifa_netmask.cast::<libc::sockaddr_in>()) };
                let address = Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr));
                let netmask = Ipv4Addr::from(u32::from_be(netmask.sin_addr.s_addr));
                if let Some(broadcast) = directed_broadcast(address, netmask) {
                    broadcasts.push(broadcast);
                }
            }
        }
        // SAFETY: `ifa_next` is either null or the next node in the same live
        // list, retained by `_guard` for the duration of this traversal.
        current = unsafe { (*current).ifa_next };
    }

    broadcasts.sort_unstable();
    broadcasts.dedup();
    Ok(broadcasts)
}

fn discovery_targets() -> Vec<SocketAddr> {
    let mut targets = vec![SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT))];
    match interface_directed_broadcasts() {
        Ok(broadcasts) => targets.extend(
            broadcasts
                .into_iter()
                .map(|address| SocketAddr::from((address, DISCOVERY_PORT))),
        ),
        Err(error) => eprintln!("discovery interface enumeration failed: {error}"),
    }
    targets.sort_unstable();
    targets.dedup();
    targets
}

/// Run zero-config discovery for PaperSpoon.
///
/// Returns the one distinct discovered `SocketAddr` (PaperSpoon IP + TCP
/// port), or an error describing absence, ambiguity, or an I/O failure.
pub fn discover_paperspoon() -> Result<SocketAddr> {
    let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_CLIENT_PORT));
    let targets = discovery_targets();
    eprintln!("discovery targets={targets:?}");
    discover_paperspoon_from_targets(bind_addr, &targets)
}

#[cfg(test)]
fn discover_paperspoon_with_schedule(
    bind_addr: SocketAddr,
    target: SocketAddr,
    probe_count: u32,
    probe_window: Duration,
) -> Result<SocketAddr> {
    discover_paperspoon_with_targets_schedule(bind_addr, &[target], probe_count, probe_window)
}

fn discover_paperspoon_from_targets(
    bind_addr: SocketAddr,
    targets: &[SocketAddr],
) -> Result<SocketAddr> {
    discover_paperspoon_with_targets_schedule(bind_addr, targets, PROBE_COUNT, PROBE_WINDOW)
}

fn discover_paperspoon_with_targets_schedule(
    bind_addr: SocketAddr,
    targets: &[SocketAddr],
    probe_count: u32,
    probe_window: Duration,
) -> Result<SocketAddr> {
    if targets.is_empty() {
        return Err(anyhow::anyhow!("discovery has no probe targets"));
    }
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
        for target in targets {
            eprintln!("discovery probe attempt={attempt} destination={target} nonce={nonce}");
            socket
                .send_to(request.as_bytes(), target)
                .with_context(|| format!("discovery probe send destination={target}"))?;
        }

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

    #[test]
    fn directed_broadcast_is_derived_without_hard_coding_the_subnet() {
        assert_eq!(
            directed_broadcast(
                Ipv4Addr::new(192, 168, 0, 3),
                Ipv4Addr::new(255, 255, 255, 0)
            ),
            Some(Ipv4Addr::new(192, 168, 0, 255))
        );
        assert_eq!(
            directed_broadcast(Ipv4Addr::new(10, 71, 4, 8), Ipv4Addr::new(255, 255, 0, 0)),
            Some(Ipv4Addr::new(10, 71, 255, 255))
        );
    }

    #[test]
    fn directed_broadcast_skips_host_only_and_limited_broadcast_results() {
        assert_eq!(
            directed_broadcast(
                Ipv4Addr::new(192, 168, 0, 3),
                Ipv4Addr::new(255, 255, 255, 255)
            ),
            None
        );
        assert_eq!(
            directed_broadcast(Ipv4Addr::new(192, 168, 0, 3), Ipv4Addr::UNSPECIFIED),
            None
        );
    }

    #[test]
    fn responses_from_all_probe_targets_share_ambiguity_detection() {
        let responders = [
            UdpSocket::bind("127.0.0.1:0").expect("bind first responder"),
            UdpSocket::bind("127.0.0.1:0").expect("bind second responder"),
        ];
        let targets: Vec<_> = responders
            .iter()
            .map(|socket| socket.local_addr().expect("responder address"))
            .collect();
        let threads: Vec<_> = responders
            .into_iter()
            .enumerate()
            .map(|(index, responder)| {
                std::thread::spawn(move || {
                    let mut buffer = [0u8; 256];
                    let (count, source) = responder.recv_from(&mut buffer).expect("receive probe");
                    let nonce = parse_discover(&buffer[..count]).expect("parse probe");
                    let response = format_here(&nonce, DEFAULT_TCP_PORT + index as u16);
                    responder
                        .send_to(response.as_bytes(), source)
                        .expect("send offer");
                })
            })
            .collect();

        let error = discover_paperspoon_with_targets_schedule(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            &targets,
            1,
            TEST_PROBE_WINDOW,
        )
        .expect_err("distinct offers across targets should be ambiguous");
        for thread in threads {
            thread.join().expect("responder thread");
        }

        assert!(error.to_string().contains("multiple PaperSpoon responders"));
    }
}
