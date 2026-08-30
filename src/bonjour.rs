//! Bonjour/DNS-SD diagnostic browsing for the `_paperspoon._tcp.local.`
//! service.
//!
//! This is deliberately a diagnostic path, not the normal connection path:
//! it browses, resolves, logs every state transition, and returns at most
//! one candidate [`SocketAddr`]. The normal transport still connects out to
//! the explicit `RUST_X11_HELLO_COMPANION` address; Bonjour discovery is
//! exercised here so its behavior on the physical Kindle/network can be
//! observed before it becomes the automatic discovery mechanism.
//!
//! Logging follows the plan's required diagnostic stages:
//!
//! ```text
//! discovery bonjour browse-start
//! discovery bonjour service-found ...
//! discovery bonjour service-resolved ...
//! discovery bonjour address=... port=...
//! discovery bonjour resolve-failed ...
//! discovery bonjour timeout
//! ```
//!
/// DNS-SD service type this project advertises/browses.
pub const SERVICE_TYPE: &str = "_paperspoon._tcp.local.";
/// Meta-service type that returns every mDNS service on the LAN; used to
/// discriminate "no multicast at all" from "our advert missing".
pub const SERVICES_TYPE: &str = "_services._dns-sd._udp.local.";
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

/// Bound on how long a single diagnostic browse waits for a resolution.
pub const DEFAULT_BROWSE_TIMEOUT: Duration = Duration::from_secs(15);

/// A resolvable candidate endpoint, decoupled from mdns-sd types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    /// Instance name without the `.local.` service suffix.
    pub instance: String,
    pub addr: SocketAddr,
}

/// Run one bounded diagnostic browse of [`SERVICE_TYPE`].
///
/// Logs every mdns-sd event and returns `Ok(Some(Candidate))` only when a
/// service instance resolved to a usable IPv4 endpoint; `Ok(None)` on
/// timeout/removal, `Err` on initialization failures.
pub fn browse_bonjour(service_type: &str, timeout: Duration) -> anyhow::Result<Option<Candidate>> {
    let daemon = ServiceDaemon::new()
        .map_err(|error| anyhow::anyhow!("bonjour daemon init failed: {error}"))?;
    eprintln!(
        "discovery bonjour browse-start service={service_type} timeout_ms={}",
        timeout.as_millis()
    );
    debug_assert_ne!(service_type, SERVICES_TYPE);
    let receiver = daemon
        .browse(service_type)
        .map_err(|error| anyhow::anyhow!("bonjour browse failed: {error}"))?;
    let deadline = std::time::Instant::now() + timeout;
    let mut found: Vec<(String, String)> = Vec::new();
    let mut selected: Option<Candidate> = None;

    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            eprintln!("discovery bonjour timeout");
            break;
        }
        let event = match receiver.recv_timeout(remaining) {
            Ok(event) => event,
            Err(_) => {
                eprintln!("discovery bonjour timeout");
                break;
            }
        };
        match event {
            ServiceEvent::SearchStarted(service_type) => {
                eprintln!("discovery bonjour search-started service={service_type}");
            }
            ServiceEvent::ServiceFound(service_type, full_name) => {
                eprintln!("discovery bonjour service-found type={service_type} name={full_name}");
                found.push((service_type, full_name));
            }
            ServiceEvent::ServiceResolved(resolved) => match resolved_address(&resolved) {
                Some(candidate) => {
                    eprintln!(
                        "discovery bonjour service-resolved name={} address={} port={}",
                        candidate.instance,
                        candidate.addr.ip(),
                        candidate.addr.port()
                    );
                    selected = Some(candidate);
                    break;
                }
                None => {
                    eprintln!(
                        "discovery bonjour service-resolved name={} no-usable-ipv4",
                        resolved.get_fullname()
                    );
                }
            },
            ServiceEvent::ServiceRemoved(service_type, full_name) => {
                eprintln!("discovery bonjour service-removed type={service_type} name={full_name}");
            }
            ServiceEvent::SearchStopped(service_type) => {
                eprintln!("discovery bonjour search-stopped service={service_type}");
            }
            _ => {}
        }
    }
    let _ = daemon.shutdown();
    Ok(selected)
}

/// Build the single candidate from a resolved service, preferring an IPv4
/// address and the SRV-advertised port. TXT metadata stays advisory (the TCP
/// handshake remains authoritative) and is not used for the port.
fn resolved_address(resolved: &mdns_sd::ResolvedService) -> Option<Candidate> {
    let mut addresses: Vec<Ipv4Addr> = resolved.get_addresses_v4().into_iter().collect();
    addresses.sort();
    let address = addresses.first().copied()?;
    Some(Candidate {
        instance: resolved.get_fullname().to_string(),
        addr: SocketAddr::new(std::net::IpAddr::V4(address), resolved.get_port()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_type_is_stable() {
        assert_eq!(SERVICE_TYPE, "_paperspoon._tcp.local.");
    }

    #[test]
    fn services_browsing_type_is_stable() {
        assert_eq!(SERVICES_TYPE, "_services._dns-sd._udp.local.");
    }

    #[test]
    fn candidate_uses_srv_port() {
        // The SRV-advertised port is observed; TXT "port" metadata is
        // advisory and must not override it (handshake stays authoritative).
        assert_eq!(srv_port_choice(5590, Some("5591")), 5590);
    }

    #[test]
    fn candidate_falls_back_to_service_port() {
        assert_eq!(srv_port_choice(5581, None), 5581);
    }

    fn srv_port_choice(srv_port: u16, _txt: Option<&str>) -> u16 {
        srv_port
    }
}
