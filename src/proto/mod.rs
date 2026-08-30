//! Wire protocol for semantic activation events and PaperSpoon control.
//!
//! Each activation is one newline-terminated text line sent from the Kindle
//! to the PaperSpoon listener:
//!
//! ```text
//! event action=<semantic-id>;
//! ```
//!
//! PaperSpoon can send one control command back:
//!
//! ```text
//! display <text>
//! ```
//!
//! which renders `<text>` in the window's status area. The wire carries only
//! the stable dotted [`crate::ui::action`] ids and the display text; no
//! device, button, or coordinate state leaves the Kindle.

/// Prefix of every Kindle→PaperSpoon activation line.
pub const EVENT_PREFIX: &str = "event action=";
/// Terminator of every activation line, before the trailing newline.
pub const ACTION_TERMINATOR: &str = ";";
/// Prefix of the PaperSpoon→Kindle display control command.
pub const DISPLAY_PREFIX: &str = "display";

/// `/` suffix form of the display command, tolerated for manual terminal use.
pub const DISPLAY_COLON_SPECIFIER: &str = ":";
/// Space suffix form of the display command.
pub const DISPLAY_SPACE_SPECIFIER: &str = " ";

/// Build the one-line wire representation of a semantic activation.
pub fn format_action_line(semantic_id: &str) -> String {
    format!("{EVENT_PREFIX}{semantic_id}{ACTION_TERMINATOR}\n")
}

/// Parse a PaperSpoon `display <text>` command into the text payload.
///
/// Accepts both `display <text>` and `display: <text>` (the `:` variant is
/// tolerated for manual terminal use). Returns `None` for any other line,
/// which the caller treats as unrecognized and ignores.
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
    fn action_line_matches_documented_wire_format() {
        assert_eq!(
            format_action_line("media.play_pause"),
            format!("{EVENT_PREFIX}media.play_pause{ACTION_TERMINATOR}\n")
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
