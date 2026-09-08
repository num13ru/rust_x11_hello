//! PaperSpoon endpoint selection from validated discovery responses.
//!
//! Datagram formats, parsing, and fixed ports live in `paper_protocol`.
//! This module retains the Kindle client's policy for selecting a unique
//! endpoint from UDP response source addresses.

use std::net::SocketAddr;

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
