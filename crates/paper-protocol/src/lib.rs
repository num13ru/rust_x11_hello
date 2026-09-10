//! Shared, std-only wire protocol for PaperPad and PaperSpoon.
//!
//! Kindle activations are newline-terminated action lines:
//!
//! ```text
//! event action=<semantic-id>;
//! ```
//!
//! PaperSpoon sends display commands in the other direction:
//!
//! ```text
//! display <text>
//! ```

/// Prefix of a Kindle-to-PaperSpoon activation line.
pub const EVENT_PREFIX: &str = "event action=";
/// Terminator of an activation line, before its trailing newline.
pub const ACTION_TERMINATOR: &str = ";";
/// Prefix of a PaperSpoon-to-Kindle display command.
pub const DISPLAY_PREFIX: &str = "display";

/// `:` suffix form of a display command, tolerated for manual terminal use.
pub const DISPLAY_COLON_SPECIFIER: &str = ":";
/// Space suffix form of a display command.
pub const DISPLAY_SPACE_SPECIFIER: &str = " ";

/// Default PaperSpoon TCP listener port.
pub const DEFAULT_TCP_PORT: u16 = 5581;
/// PaperSpoon UDP discovery listener port.
pub const DISCOVERY_PORT: u16 = 5580;
/// PaperPad UDP discovery client port.
pub const DISCOVERY_CLIENT_PORT: u16 = 5582;
/// Prefix of a discovery request datagram.
pub const DISCOVER_PREFIX: &str = "PAPERPAD DISCOVER ";
/// Prefix of a discovery response datagram.
pub const HERE_PREFIX: &str = "PAPERSPOON HERE ";

/// Build the one-line wire representation of a semantic activation.
pub fn format_action_line(semantic_id: &str) -> String {
    format!("{EVENT_PREFIX}{semantic_id}{ACTION_TERMINATOR}\n")
}

/// Parse an activation line after the transport has removed surrounding
/// whitespace.
///
/// This intentionally preserves the existing permissive behavior: an empty
/// action id is accepted, and a trailing newline is not removed here.
pub fn parse_action_line(line: &str) -> Option<&str> {
    line.strip_prefix(EVENT_PREFIX)?
        .strip_suffix(ACTION_TERMINATOR)
}

/// Parse a PaperSpoon display command into its text payload.
///
/// Canonical `display <text>` and the manual-terminal alias
/// `display:<text>` are accepted. The prefix is case-sensitive. Empty
/// payloads, concatenated prefixes, and unrelated lines return `None`.
pub fn parse_display_command(line: &str) -> Option<String> {
    let line = line.trim_end_matches('\n');
    let rest = line.strip_prefix(DISPLAY_PREFIX)?;
    let rest = rest
        .strip_prefix(DISPLAY_SPACE_SPECIFIER)
        .or_else(|| rest.strip_prefix(DISPLAY_COLON_SPECIFIER))?;
    let text = rest.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

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
    fn action_line_roundtrip_matches_existing_wire_format() {
        let line = format_action_line("media.play_pause");
        assert_eq!(line, "event action=media.play_pause;\n");
        assert_eq!(parse_action_line(line.trim()), Some("media.play_pause"));
    }

    #[test]
    fn action_parser_preserves_permissive_payload_behavior() {
        assert_eq!(parse_action_line("event action=;"), Some(""));
        assert_eq!(parse_action_line("event action=x;;"), Some("x;"));
        assert_eq!(parse_action_line("event action=x;\n"), None);
        assert_eq!(parse_action_line("bogus action=x;"), None);
    }

    #[test]
    fn display_command_parses_canonical_and_colon_forms() {
        assert_eq!(
            parse_display_command("display hello\n"),
            Some("hello".to_string())
        );
        assert_eq!(
            parse_display_command("display: world"),
            Some("world".to_string())
        );
        assert_eq!(
            parse_display_command("display:compact"),
            Some("compact".to_string())
        );
        assert_eq!(
            parse_display_command("display spaced "),
            Some("spaced".to_string())
        );
    }

    #[test]
    fn display_parser_rejects_non_command_prefixes_and_empty_payloads() {
        assert_eq!(parse_display_command("displayed"), None);
        assert_eq!(parse_display_command("display-text"), None);
        assert_eq!(parse_display_command("Display hello"), None);
        assert_eq!(parse_display_command("display\n"), None);
        assert_eq!(parse_display_command("display:\n"), None);
        assert_eq!(parse_display_command("display \n"), None);
        assert_eq!(parse_display_command("event action=x;\n"), None);
    }

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
