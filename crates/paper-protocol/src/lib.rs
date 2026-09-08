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
/// This intentionally preserves the existing permissive prefix behavior:
/// every line beginning with `display` is treated as a display command.
/// Canonical `display <text>` and tolerated `display: <text>` forms are both
/// accepted. Empty payloads and unrelated lines return `None`.
pub fn parse_display_command(line: &str) -> Option<String> {
    let line = line.trim_end_matches('\n');
    let rest = line
        .strip_prefix(DISPLAY_PREFIX)
        .or_else(|| line.strip_prefix(&format!("{DISPLAY_PREFIX}{DISPLAY_COLON_SPECIFIER}")))
        .or_else(|| line.strip_prefix(&format!("{DISPLAY_PREFIX}{DISPLAY_SPACE_SPECIFIER}")))?;
    let rest = rest.strip_prefix(DISPLAY_COLON_SPECIFIER).unwrap_or(rest);
    let text = rest.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
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
            parse_display_command("display spaced "),
            Some("spaced".to_string())
        );
    }

    #[test]
    fn display_parser_preserves_permissive_prefix_behavior() {
        assert_eq!(parse_display_command("displayed"), Some("ed".to_string()));
        assert_eq!(parse_display_command("display\n"), None);
        assert_eq!(parse_display_command("display:\n"), None);
        assert_eq!(parse_display_command("event action=x;\n"), None);
    }
}
