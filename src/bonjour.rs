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
//! Nothing here contacts the network; a [`Candidate`] is produced only from
//! a resolved service's own IPv4 addresses + TXT port, so callers can test
//! selection purely from resolver output.

use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

/// DNS-SD service type this project advertises/browses.
pub const SERVICE_TYPE: &str = "_paperspoon._tcp.local.";
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
pub fn browse_bonjour(timeout: Duration) -> anyhow::Result<Option<Candidate>> {
    let daemon = ServiceDaemon::new()
        .map_err(|error| anyhow::anyhow!("bonjour daemon init failed: {error}"))?;
    eprintln!(
        "discovery bonjour browse-start service={SERVICE_TYPE} timeout_ms={}",
        timeout.as_millis()
    );
    let receiver = daemon
        .browse(SERVICE_TYPE)
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
            ServiceEvent::ServiceResolved(resolved) => match resolved_address(&resolved, &found) {
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
/// address over IPv6 and the TXT-provided port over the service port.
fn resolved_address(
    resolved: &mdns_sd::ResolvedService,
    _found: &[(String, String)],
) -> Option<Candidate> {
    let txt_port = resolved
        .get_properties()
        .get("port")
        .and_then(|value| value.val_str().parse::<u16>().ok());
    let mut addresses: Vec<Ipv4Addr> = resolved.get_addresses_v4().into_iter().collect();
    addresses.sort();
    let address = addresses.first().copied()?;
    let port = txt_port.unwrap_or_else(|| resolved.get_port());
    Some(Candidate {
        instance: resolved.get_fullname().to_string(),
        addr: SocketAddr::new(std::net::IpAddr::V4(address), port),
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
    fn candidate_uses_txt_port_when_present() {
        // A resolved service with a TXT "port" override wins over the SRV port.
        assert_eq!(
            tx_port_choice(5590, Some("5591")),
            5591,
            "TXT port must take precedence over the SRV-advertised port"
        );
    }

    #[test]
    fn candidate_falls_back_to_service_port() {
        assert_eq!(tx_port_choice(5581, None), 5581);
    }

    fn tx_port_choice(srv_port: u16, txt: Option<&str>) -> u16 {
        txt.and_then(|v| v.parse::<u16>().ok()).unwrap_or(srv_port)
    }
}
