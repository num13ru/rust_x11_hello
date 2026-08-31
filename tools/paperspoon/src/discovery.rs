//! PaperSpoon UDP discovery responder.
//!
//! Binds UDP `0.0.0.0:5580`; on a valid DISCOVER datagram, replies with a
//! single unicast HERE to the exact request source. No broadcast responses,
//! no multicast, no ACKs.
//!
//! Wire format (newline-terminated ASCII):
//!
//! ```text
//! PAPERPAD DISCOVER <nonce>
//! PAPERSPOON HERE <nonce> <tcp-port>
//! ```

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

/// UDP discovery listener port.
pub const DISCOVERY_PORT: u16 = 5580;
/// The TCP port advertised to discoverers.
pub const TCP_PORT: u16 = 5581;
/// Prefix of the request datagram.
const DISCOVER_PREFIX: &str = "PAPERPAD DISCOVER ";
/// Prefix of the response datagram.
const HERE_PREFIX: &str = "PAPERSPOON HERE ";

/// Parse a DISCOVER request datagram into its nonce.
fn parse_discover(datagram: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(datagram).ok()?.trim_end();
    let nonce = text.strip_prefix(DISCOVER_PREFIX)?;
    if nonce.is_empty() {
        return None;
    }
    Some(nonce.to_string())
}

/// Parse a HERE response datagram into nonce and TCP port.
#[cfg(test)]
fn parse_here(datagram: &[u8]) -> Option<(String, u16)> {
    let text = std::str::from_utf8(datagram).ok()?.trim_end();
    let rest = text.strip_prefix(HERE_PREFIX)?;
    let mut parts = rest.split_whitespace();
    let nonce = parts.next()?.to_string();
    let port = parts.next()?.parse::<u16>().ok()?;
    if port == 0 || parts.next().is_some() {
        return None;
    }
    Some((nonce, port))
}
/// Build a HERE response datagram.
fn format_here(nonce: &str, tcp_port: u16) -> String {
    format!("{HERE_PREFIX}{nonce} {tcp_port}\n")
}

/// Bind and run the discovery responder forever.
///
/// Returns only on bind failure; the loop skips malformed requests.
pub fn run_discovery_listener() -> std::io::Result<()> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)))?;
    println!("discovery listening address=0.0.0.0:{DISCOVERY_PORT}");
    let mut buffer = [0u8; 256];
    loop {
        let (count, source) = socket.recv_from(&mut buffer)?;
        let Some(nonce) = parse_discover(&buffer[..count]) else {
            eprintln!("discovery request invalid from={source}");
            continue;
        };
        eprintln!("discovery request from={source} nonce={nonce}");
        let response = format_here(&nonce, TCP_PORT);
        socket.send_to(response.as_bytes(), source)?;
        eprintln!("discovery response sent to={source}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responder_parses_and_formats_roundtrip() {
        let nonce = "cafe1234";
        let request = format!("{DISCOVER_PREFIX}{nonce}\n");
        assert_eq!(parse_discover(request.as_bytes()), Some(nonce.to_string()));
        let response = format_here(nonce, TCP_PORT);
        assert_eq!(
            parse_here(response.as_bytes()),
            Some((nonce.to_string(), TCP_PORT))
        );
    }

    #[test]
    fn responder_rejects_malformed_requests() {
        assert_eq!(parse_discover(b""), None);
        assert_eq!(parse_discover(b"BOGUS xyz"), None);
        assert_eq!(parse_discover(b"PAPERPAD DISCOVER"), None);
    }
}
