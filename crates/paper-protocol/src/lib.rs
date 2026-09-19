//! Shared, std-only protocol-v2 primitives for PaperPad and PaperSpoon.

mod framebuffer;
mod v2;
mod v2_payload;

pub use framebuffer::{
    Mono1Frame, Mono1FrameError, Mono1Pixel, mono1_payload_len, mono1_stride, validate_mono1_pixels,
};
pub use v2::{
    V2_HEADER_LEN, V2_MAGIC, V2_MAX_PAYLOAD_LEN, V2_VERSION, V2DecodeError, V2DecodeResult,
    V2EncodeError, V2Header, V2Message, V2MessageType, decode_v2_message, encode_v2_message,
};
pub use v2_payload::{
    V2_FRAME_PREFIX_LEN, V2_HELLO_PAYLOAD_LEN, V2_POINTER_PAYLOAD_LEN, V2_VIEWPORT_PAYLOAD_LEN,
    V2FramePayload, V2Hello, V2Payload, V2PayloadError, V2PixelFormat, V2Pointer, V2PointerPhase,
    V2Viewport, decode_v2_payload, encode_v2_frame,
};

/// Default PaperSpoon TCP listener port.
pub const DEFAULT_TCP_PORT: u16 = 5581;
/// PaperSpoon UDP discovery listener port.
pub const DISCOVERY_PORT: u16 = 5580;
/// PaperPad UDP discovery client port.
pub const DISCOVERY_CLIENT_PORT: u16 = 5582;
/// Prefix for a discovery request datagram.
pub const DISCOVER_PREFIX: &str = "PAPERPAD DISCOVER ";
/// Prefix for a discovery response datagram.
pub const HERE_PREFIX: &str = "PAPERSPOON HERE ";

/// Build a newline-terminated discovery request datagram.
pub fn format_discover(nonce: &str) -> String {
    format!("{DISCOVER_PREFIX}{nonce}\n")
}

/// Parse a discovery request datagram into its nonce.
///
/// The nonce remains otherwise unvalidated to preserve the existing wire
/// behavior.
pub fn parse_discover(datagram: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(datagram).ok()?.trim_end();
    let nonce = text.strip_prefix(DISCOVER_PREFIX)?;
    if nonce.is_empty() {
        return None;
    }
    Some(nonce.to_string())
}

/// Build a newline-terminated discovery response datagram.
pub fn format_here(nonce: &str, tcp_port: u16) -> String {
    format!("{HERE_PREFIX}{nonce} {tcp_port}\n")
}

/// Parse a discovery response into its nonce and advertised TCP port.
pub fn parse_here(datagram: &[u8]) -> Option<(String, u16)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_request_roundtrip_preserves_nonce() {
        let request = format_discover("deadbeef");
        assert_eq!(request, "PAPERPAD DISCOVER deadbeef\n");
        assert_eq!(parse_discover(request.as_bytes()), Some("deadbeef".into()));
    }

    #[test]
    fn discovery_request_parser_preserves_permissive_nonce_behavior() {
        assert_eq!(parse_discover(b"PAPERPAD DISCOVER "), None);
        assert_eq!(
            parse_discover(b"PAPERPAD DISCOVER nonce with spaces\n"),
            Some("nonce with spaces".into())
        );
        assert_eq!(parse_discover(b"BOGUS deadbeef"), None);
        assert_eq!(parse_discover(&[0xff]), None);
    }

    #[test]
    fn discovery_response_roundtrip_preserves_nonce_and_port() {
        let response = format_here("deadbeef", DEFAULT_TCP_PORT);
        assert_eq!(response, "PAPERSPOON HERE deadbeef 5581\n");
        assert_eq!(
            parse_here(response.as_bytes()),
            Some(("deadbeef".into(), DEFAULT_TCP_PORT))
        );
    }

    #[test]
    fn malformed_discovery_responses_are_rejected() {
        assert_eq!(parse_here(b""), None);
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef"), None);
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef 0"), None);
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef 5581 extra"), None);
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef notaport"), None);
        assert_eq!(parse_here(b"PAPERSPOON HERE deadbeef 65536"), None);
    }
}
