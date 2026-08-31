//! Minimal zero-config UDP discovery for the PaperPad/PaperSpoon pair.
//!
//! Wire format (newline-terminated ASCII, deliberately tiny):
//!
//! ```text
//! PAPERPAD DISCOVER <nonce>
//! PAPERSPOON HERE <nonce> <tcp-port>
//! ```
//!
//! - `<nonce>` is a hex string, unique per discovery attempt, validated
//!   against the echoed value in the HERE response.
//! - The UDP **source address** of the HERE datagram is the discovered
//!   PaperSpoon address; the response payload only advertises the TCP port.
//!
//! Fixed ports:
//!
//! - UDP 5580 — PaperSpoon discovery listener
//! - UDP 5582 — PaperPad discovery client (bound, so PaperSpoon can reply)
//! - TCP 5581 — existing PaperSpoon TCP listener
//!
//! No session IDs, ACKs, sequence numbers, or protocol negotiation.

use std::net::SocketAddr;

/// PaperSpoon UDP discovery listener port.
pub const DISCOVERY_PORT: u16 = 5580;
/// PaperPad UDP discovery client port.
pub const CLIENT_PORT: u16 = 5582;
/// Marker prefix of the request datagram.
pub const DISCOVER_PREFIX: &str = "PAPERPAD DISCOVER ";
/// Marker prefix of the response datagram.
pub const HERE_PREFIX: &str = "PAPERSPOON HERE ";

/// Parse a HERE response datagram into its nonce and advertised TCP port.
///
/// Returns `None` for malformed responses, empty payloads, or a non-zero
/// port that fails to parse.
pub fn parse_here(datagram: &[u8]) -> Option<(String, u16)> {
    let text = std::str::from_utf8(datagram).ok()?.trim_end();
    let rest = text.strip_prefix(HERE_PREFIX)?;
    let mut parts = rest.split_whitespace();
    let nonce = parts.next()?.to_string();
    let port = parts.next()?.parse::<u16>().ok()?;
    if port == 0 {
        return None;
    }
    // Reject anything after the two expected fields.
    if parts.next().is_some() {
        return None;
    }
    Some((nonce, port))
}

/// Build a DISCOVER request datagram for a nonce.
pub fn format_discover(nonce: &str) -> String {
    format!("{DISCOVER_PREFIX}{nonce}\n")
}

/// Select the discovered PaperSpoon endpoint from validated responses.
///
/// Returns `SocketAddr { ip: response source IP, port: advertised TCP port }`
/// for exactly one distinct responder; `Err` for zero or multiple.
pub fn select_endpoint(
    responses: impl Iterator<Item = (SocketAddr, u16)>,
) -> Result<SocketAddr, DiscoveryError> {
    let mut endpoints: Vec<SocketAddr> = responses
        .map(|(source, port)| SocketAddr::new(source.ip(), port))
        .collect();
    endpoints.sort_unstable();
    endpoints.dedup();
    match endpoints.len() {
        0 => Err(DiscoveryError::NoResponder),
        1 => Ok(endpoints[0]),
        _ => Err(DiscoveryError::Ambiguous(endpoints)),
    }
}

/// Discovery outcome when selecting among validated responders.
#[derive(Debug)]
pub enum DiscoveryError {
    /// No valid responder produced an endpoint.
    NoResponder,
    /// More than one distinct endpoint was offered.
    Ambiguous(Vec<SocketAddr>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_request_uses_marker_and_nonce() {
        let request = format_discover("deadbeef");
        assert!(request.starts_with(DISCOVER_PREFIX));
        assert!(request.contains("deadbeef"));
        assert!(request.ends_with('\n'));
    }

    #[test]
    fn here_roundtrip_preserves_nonce_and_port() {
        let response = format!("{HERE_PREFIX}deadbeef 5581\n");
        assert_eq!(
            parse_here(response.as_bytes()),
            Some(("deadbeef".to_string(), 5581))
        );
    }

    #[test]
    fn malformed_datagrams_are_rejected() {
        assert_eq!(parse_here(b""), None);
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef"), None);
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef 0"), None);
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef 5581 extra"), None);
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef notaport"), None);
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef 70000"), None);
    }

    #[test]
    fn invalid_port_rejected_zero_or_overflow() {
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef 0"), None);
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef 65536"), None);
    }

    #[test]
    fn endpoint_selected_from_udp_source() {
        let source: SocketAddr = "192.168.0.12:5580".parse().unwrap();
        let endpoint = select_endpoint(std::iter::once((source, 5581))).unwrap();
        assert_eq!(endpoint, "192.168.0.12:5581".parse().unwrap());
    }

    #[test]
    fn multiple_responders_are_ambiguous() {
        let a: SocketAddr = "192.168.0.12:5580".parse().unwrap();
        let b: SocketAddr = "192.168.0.13:5580".parse().unwrap();
        let result = select_endpoint([(a, 5581), (b, 5581)].into_iter());
        assert!(matches!(result, Err(DiscoveryError::Ambiguous(_))));
    }

    #[test]
    fn no_responder_is_an_error() {
        assert!(matches!(
            select_endpoint(std::iter::empty()),
            Err(DiscoveryError::NoResponder)
        ));
    }
}
