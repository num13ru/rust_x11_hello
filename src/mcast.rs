//! Raw multicast reachability probe for the Kindle Wi-Fi path.
//!
//! Phase 5B discriminator: while the Mac floods a multicast group with a
//! known marker, this joins `224.0.0.251:5353` and reports whether ANY
//! packet arrives. This separates "multicast is blocked on the WLAN path"
//! from "mDNS/Bonjour specifically fails".
//!
//! Uses only `std::net` — no crate needed. Joining with `0.0.0.0` selects
//! the system's default-route interface (on the Kindle: `wlan0`, the exact
//! interface under test). The marker is also sent to the group so a joined
//! interface that receives its own send proves RX.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

/// mDNS multicast group and port (RFC 6762).
pub const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub const MDNS_PORT: u16 = 5353;
/// Marker sent to the group and matched in received data.
pub const PROBE_MARKER: &[u8] = b"rust-x11-hello-multicast-probe";
/// Bound on how long the probe listens.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Run one bounded multicast RX probe on the mDNS group.
///
/// Returns `true` when at least one packet was received, `false` otherwise.
pub fn probe_multicast(timeout: Duration) -> anyhow::Result<bool> {
    let socket = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0)))
        .map_err(|error| anyhow::anyhow!("multicast probe bind failed: {error}"))?;
    socket
        .set_read_timeout(Some(timeout))
        .map_err(|error| anyhow::anyhow!("multicast probe read timeout failed: {error}"))?;
    eprintln!(
        "multicast probe start group={MDNS_GROUP} port={MDNS_PORT} timeout_ms={}",
        timeout.as_millis()
    );

    // Join on the default-route interface (0.0.0.0). On the Kindle this is
    // wlan0 — the exact link under test.
    match socket.join_multicast_v4(&MDNS_GROUP, &Ipv4Addr::UNSPECIFIED) {
        Ok(()) => eprintln!("multicast probe join default-interface ok"),
        Err(error) => eprintln!("multicast probe join failed: {error}"),
    }

    // Send the marker so a joined interface receiving its own send proves RX.
    match socket.send_to(PROBE_MARKER, (MDNS_GROUP, MDNS_PORT)) {
        Ok(n) => eprintln!("multicast probe sent-marker bytes={n}"),
        Err(error) => eprintln!("multicast probe send-marker failed: {error}"),
    }

    let mut buffer = [0u8; 512];
    let mut received = false;
    loop {
        match socket.recv_from(&mut buffer) {
            Ok((count, source)) => {
                received = true;
                eprintln!(
                    "multicast probe received from={source} bytes={count} prefix={}",
                    String::from_utf8_lossy(&buffer[..count.min(32)])
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                eprintln!("multicast probe timeout");
                break;
            }
            Err(error) => {
                eprintln!("multicast probe error: {error}");
                break;
            }
        }
    }
    Ok(received)
}
