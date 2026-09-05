//! Semantic action IDs for the nine-button grid.
//!
//! This is the wire-independent unit of the small semantic protocol: a button
//! activation maps to a stable dotted action id that a transport (TCP over
//! Wi-Fi; USBNetwork is unavailable on this PW6) will carry verbatim to a
//! macOS PaperSpoon. No transport exists yet; this module only defines the
//! mapping and the IDs.

/// A semantic action assignable to a grid button.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticAction {
    MediaPlayPause,
    MediaNext,
    MediaPrevious,
    TerminalNewWindow,
    TmuxWork,
    ZoomToggleMute,
    StubButton7,
    StubButton8,
    StubButton9,
    Exit,
}

impl SemanticAction {
    /// Stable dotted wire identifier for the action.
    pub const fn id(self) -> &'static str {
        match self {
            Self::MediaPlayPause => "media.play_pause",
            Self::MediaNext => "media.next",
            Self::MediaPrevious => "media.previous",
            Self::TerminalNewWindow => "terminal.new_window",
            Self::TmuxWork => "tmux.work",
            Self::ZoomToggleMute => "zoom.toggle_mute",
            Self::StubButton7 => "stub.button_7",
            Self::StubButton8 => "stub.button_8",
            Self::StubButton9 => "stub.button_9",
            Self::Exit => "app.exit",
        }
    }
}

/// Map a grid button id (1..=9, plus the exit bar id) to its semantic action.
///
/// Returns `None` for any other id; the grid never produces one.
pub fn action_for_button(button_id: u8) -> Option<SemanticAction> {
    match button_id {
        crate::ui::geometry::EXIT_BUTTON_ID => Some(SemanticAction::Exit),
        1 => Some(SemanticAction::MediaPlayPause),
        2 => Some(SemanticAction::MediaNext),
        3 => Some(SemanticAction::MediaPrevious),
        4 => Some(SemanticAction::TerminalNewWindow),
        5 => Some(SemanticAction::TmuxWork),
        6 => Some(SemanticAction::ZoomToggleMute),
        7 => Some(SemanticAction::StubButton7),
        8 => Some(SemanticAction::StubButton8),
        9 => Some(SemanticAction::StubButton9),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grid_button_maps_to_a_documented_semantic_action() {
        let expected = [
            (1, SemanticAction::MediaPlayPause, "media.play_pause"),
            (2, SemanticAction::MediaNext, "media.next"),
            (3, SemanticAction::MediaPrevious, "media.previous"),
            (4, SemanticAction::TerminalNewWindow, "terminal.new_window"),
            (5, SemanticAction::TmuxWork, "tmux.work"),
            (6, SemanticAction::ZoomToggleMute, "zoom.toggle_mute"),
            (7, SemanticAction::StubButton7, "stub.button_7"),
            (8, SemanticAction::StubButton8, "stub.button_8"),
            (9, SemanticAction::StubButton9, "stub.button_9"),
            (
                crate::ui::geometry::EXIT_BUTTON_ID,
                SemanticAction::Exit,
                "app.exit",
            ),
        ];

        for (button_id, action, id) in expected {
            assert_eq!(action_for_button(button_id), Some(action));
            assert_eq!(action.id(), id);
        }
    }

    #[test]
    fn out_of_range_button_ids_have_no_semantic_action() {
        assert_eq!(action_for_button(0), None);
        assert_eq!(action_for_button(u8::MAX), None);
    }

    #[test]
    fn action_ids_are_stable_and_unique() {
        let mut ids = vec![
            SemanticAction::MediaPlayPause.id(),
            SemanticAction::MediaNext.id(),
            SemanticAction::MediaPrevious.id(),
            SemanticAction::TerminalNewWindow.id(),
            SemanticAction::TmuxWork.id(),
            SemanticAction::ZoomToggleMute.id(),
            SemanticAction::StubButton7.id(),
            SemanticAction::StubButton8.id(),
            SemanticAction::StubButton9.id(),
            SemanticAction::Exit.id(),
        ];
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 10);
    }
}
