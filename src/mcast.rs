//! Raw multicast reachability probe for the Kindle Wi-Fi path.
//!
//! Phase 5B discriminator: while the Mac floods a multicast group, this binds
//! and joins `224.0.0.251:5353` on the named interface and reports whether any
//! externally generated packet arrives. This separates "multicast is blocked
//! on the WLAN path" from "mDNS/Bonjour specifically fails".
//!
//! A distinct local marker checks socket loopback but never counts as external
//! reception because multicast loopback does not traverse the access point.

use if_addrs::IfAddr;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// mDNS multicast group and port (RFC 6762).
pub const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub const MDNS_PORT: u16 = 5353;
/// Default Kindle Wi-Fi interface.
pub const DEFAULT_INTERFACE: &str = "wlan0";
/// Marker the external sender can use; any non-local-marker packet also counts.
pub const PROBE_MARKER: &[u8] = b"rust-x11-hello-multicast-probe";
const LOCAL_LOOPBACK_MARKER: &[u8] = b"rust-x11-hello-local-loopback";
/// Bound on how long the probe listens.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Run one bounded multicast RX probe on the mDNS group.
///
/// Returns `true` when at least one externally generated packet was received,
/// `false` when only local loopback (or nothing) was observed.
pub fn probe_multicast(interface_name: &str, timeout: Duration) -> anyhow::Result<bool> {
    if timeout.is_zero() {
        return Err(anyhow::anyhow!("multicast probe timeout must be non-zero"));
    }

    let interface_ip = interface_ipv4(interface_name)?;
    let bind_addr = probe_bind_addr();
    let socket = bind_probe_socket(bind_addr, interface_ip, interface_name)?;
    socket
        .set_multicast_loop_v4(true)
        .map_err(|error| anyhow::anyhow!("multicast probe enable loopback failed: {error}"))?;
    eprintln!(
        "multicast probe start interface={interface_name} address={interface_ip} bind={bind_addr} group={MDNS_GROUP} port={MDNS_PORT} timeout_ms={}",
        timeout.as_millis()
    );

    socket
        .join_multicast_v4(&MDNS_GROUP, &interface_ip)
        .map_err(|error| {
            anyhow::anyhow!("multicast probe join {MDNS_GROUP} on {interface_name}: {error}")
        })?;
    eprintln!("multicast probe join interface={interface_name} address={interface_ip} ok");

    let sent = socket
        .send_to(LOCAL_LOOPBACK_MARKER, (MDNS_GROUP, MDNS_PORT))
        .map_err(|error| anyhow::anyhow!("multicast probe send local marker failed: {error}"))?;
    eprintln!("multicast probe sent-local-marker bytes={sent}");

    let mut buffer = [0u8; 512];
    let mut local_loopback_received = false;
    let mut external_received = false;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| anyhow::anyhow!("multicast probe timeout is too large"))?;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        socket
            .set_read_timeout(Some(remaining))
            .map_err(|error| anyhow::anyhow!("multicast probe read timeout failed: {error}"))?;

        match socket.recv_from(&mut buffer) {
            Ok((count, source)) => {
                let packet = &buffer[..count];
                let kind = if packet == LOCAL_LOOPBACK_MARKER {
                    local_loopback_received = true;
                    "local-loopback"
                } else if packet == PROBE_MARKER {
                    external_received = true;
                    "external-marker"
                } else {
                    external_received = true;
                    "external-other"
                };
                let prefix = escaped_prefix(packet, 32);
                eprintln!(
                    "multicast probe received kind={kind} from={source} bytes={count} prefix={prefix}"
                );
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => {
                return Err(anyhow::anyhow!("multicast probe receive failed: {error}"));
            }
        }
    }

    eprintln!(
        "multicast probe complete local_loopback={local_loopback_received} external={external_received}"
    );
    Ok(external_received)
}

fn probe_bind_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], MDNS_PORT))
}

fn escaped_prefix(packet: &[u8], limit: usize) -> String {
    packet
        .iter()
        .take(limit)
        .flat_map(|byte| std::ascii::escape_default(*byte))
        .map(char::from)
        .collect()
}

fn bind_probe_socket(
    bind_addr: SocketAddr,
    interface_ip: Ipv4Addr,
    interface_name: &str,
) -> anyhow::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|error| anyhow::anyhow!("multicast probe socket failed: {error}"))?;
    socket
        .set_reuse_address(true)
        .map_err(|error| anyhow::anyhow!("multicast probe SO_REUSEADDR failed: {error}"))?;
    #[cfg(target_vendor = "apple")]
    socket
        .set_reuse_port(true)
        .map_err(|error| anyhow::anyhow!("multicast probe SO_REUSEPORT failed: {error}"))?;
    socket
        .set_multicast_if_v4(&interface_ip)
        .map_err(|error| anyhow::anyhow!("multicast probe select {interface_name}: {error}"))?;
    socket
        .bind(&bind_addr.into())
        .map_err(|error| anyhow::anyhow!("multicast probe bind {bind_addr} failed: {error}"))?;
    Ok(socket.into())
}

fn interface_ipv4(interface_name: &str) -> anyhow::Result<Ipv4Addr> {
    let mut addresses: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .map_err(|error| anyhow::anyhow!("multicast probe list interfaces failed: {error}"))?
        .into_iter()
        .filter_map(|interface| {
            if interface.name != interface_name || !interface.is_oper_up() {
                return None;
            }
            match interface.addr {
                IfAddr::V4(address) if !address.ip.is_loopback() => Some(address.ip),
                _ => None,
            }
        })
        .collect();
    addresses.sort_by_key(|address| (address.is_link_local(), *address));
    addresses.first().copied().ok_or_else(|| {
        anyhow::anyhow!("multicast probe interface {interface_name} has no active IPv4 address")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receiver_binds_the_multicast_destination_port() {
        assert_eq!(probe_bind_addr(), SocketAddr::from(([0, 0, 0, 0], 5353)));
    }

    #[test]
    fn local_and_external_markers_are_distinct() {
        assert_ne!(LOCAL_LOOPBACK_MARKER, PROBE_MARKER);
    }

    #[test]
    fn binary_packet_prefix_is_log_safe() {
        assert_eq!(escaped_prefix(&[0, b'A', b'\n', 0xff], 4), "\\x00A\\n\\xff");
    }
}
