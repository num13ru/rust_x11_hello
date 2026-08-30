//! Wire protocol for semantic activation events and companion control.
//!
//! Each activation is one newline-terminated text line sent from the Kindle
//! to the companion listener:
//!
//! ```text
//! event action=<semantic-id>;
//! ```
//!
//! The companion can send one control command back:
//!
//! ```text
//! display <text>
//! ```
//!
//! which renders `<text>` in the window's status area. The wire carries only
//! the stable dotted [`crate::ui::action`] ids and the display text; no
//! device, button, or coordinate state leaves the Kindle.

/// Build the one-line wire representation of a semantic activation.
pub fn format_action_line(semantic_id: &str) -> String {
    format!("event action={semantic_id};\n")
}

/// Parse a companion `display <text>` command into the text payload.
///
/// Accepts both `display <text>` and `display: <text>` (the `:` variant is
/// tolerated for manual terminal use). Returns `None` for any other line,
/// which the caller treats as unrecognized and ignores.
pub fn parse_display_command(line: &str) -> Option<String> {
    let line = line.trim_end_matches('\n');
    let rest = line
        .strip_prefix("display")
        .or_else(|| line.strip_prefix("display:"))
        .or_else(|| line.strip_prefix("display "))?;
    let rest = rest.strip_prefix(':').unwrap_or(rest);
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
    fn action_line_matches_documented_wire_format() {
        assert_eq!(
            format_action_line("media.play_pause"),
            "event action=media.play_pause;\n"
        );
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
            parse_display_command("display   spaced  "),
            Some("spaced".to_string())
        );
    }

    #[test]
    fn display_command_rejects_empty_and_unknown() {
        assert_eq!(parse_display_command("display\n"), None);
        assert_eq!(parse_display_command("display:\n"), None);
        assert_eq!(parse_display_command("event action=x;\n"), None);
    }
}
