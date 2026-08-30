//! Wire protocol for semantic activation events.
//!
//! Each activation is one newline-terminated text line sent from the Kindle
//! to the companion listener:
//!
//! ```text
//! event action=<semantic-id>;
//! ```
//!
//! The wire carries only the stable dotted [`crate::ui::action`] ids; no
//! device, button, or coordinate state leaves the Kindle.

/// Build the one-line wire representation of a semantic activation.
pub fn format_action_line(semantic_id: &str) -> String {
    format!("event action={semantic_id};\n")
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
}
