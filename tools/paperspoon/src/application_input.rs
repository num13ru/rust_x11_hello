//! Host-owned application contact tracking and hit testing.

use crate::application_ui::ApplicationUi;
use paper_protocol::{V2Pointer, V2PointerPhase};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApplicationActivation {
    pub button_id: u8,
    pub action_id: &'static str,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApplicationInputResult {
    pub activation: Option<ApplicationActivation>,
    pub pressed_button: Option<u8>,
    pub visual_changed: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ContactState {
    #[default]
    Idle,
    Armed(u8),
    Cancelled,
}

/// Tracks one remote primary contact against the last host-rendered UI frame.
#[derive(Debug, Default)]
pub struct ApplicationInput {
    viewport: Option<(u16, u16)>,
    contact: ContactState,
}

impl ApplicationInput {
    pub fn is_active(&self) -> bool {
        self.viewport.is_some()
    }

    /// Replace the active application viewport and cancel any partial contact.
    /// `None` means the displayed frame is not application UI.
    pub fn set_viewport(&mut self, ui: &mut ApplicationUi, viewport: Option<(u16, u16)>) -> bool {
        self.viewport = viewport.filter(|(width, height)| *width > 0 && *height > 0);
        self.contact = ContactState::Idle;
        ui.set_pressed_button(None)
    }

    pub fn handle_pointer(
        &mut self,
        ui: &mut ApplicationUi,
        pointer: V2Pointer,
    ) -> ApplicationInputResult {
        let Some((width, height)) = self.viewport else {
            self.contact = ContactState::Idle;
            return ApplicationInputResult {
                visual_changed: ui.set_pressed_button(None),
                ..ApplicationInputResult::default()
            };
        };
        let hit = ui
            .layout(width, height)
            .and_then(|layout| {
                layout
                    .buttons
                    .into_iter()
                    .find(|button| button.bounds.contains(pointer.x(), pointer.y()))
            })
            .map(|button| button.id);

        let activation = match pointer.phase() {
            V2PointerPhase::Down => {
                self.contact = match self.contact {
                    ContactState::Idle => hit
                        .map(ContactState::Armed)
                        .unwrap_or(ContactState::Cancelled),
                    ContactState::Armed(_) | ContactState::Cancelled => ContactState::Cancelled,
                };
                None
            }
            V2PointerPhase::Up => {
                let activated = match (self.contact, hit) {
                    (ContactState::Armed(armed_id), Some(released_id))
                        if armed_id == released_id =>
                    {
                        action_for_button(armed_id).map(|action_id| ApplicationActivation {
                            button_id: armed_id,
                            action_id,
                        })
                    }
                    _ => None,
                };
                self.contact = ContactState::Idle;
                activated
            }
        };
        let pressed_button = match self.contact {
            ContactState::Armed(button_id) => Some(button_id),
            ContactState::Idle | ContactState::Cancelled => None,
        };
        ApplicationInputResult {
            activation,
            pressed_button,
            visual_changed: ui.set_pressed_button(pressed_button),
        }
    }
}

fn action_for_button(button_id: u8) -> Option<&'static str> {
    match button_id {
        1 => Some("media.play_pause"),
        2 => Some("media.next"),
        3 => Some("media.previous"),
        4 => Some("terminal.new_window"),
        5 => Some("tmux.work"),
        6 => Some("zoom.toggle_mute"),
        7 => Some("stub.button_7"),
        8 => Some("stub.button_8"),
        9 => Some("stub.button_9"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: (u16, u16) = (1272, 1624);

    fn pointer(phase: V2PointerPhase, x: u16, y: u16) -> V2Pointer {
        V2Pointer::new(phase, x, y)
    }

    fn tracker(ui: &mut ApplicationUi) -> ApplicationInput {
        let mut input = ApplicationInput::default();
        assert!(!input.set_viewport(ui, Some(VIEWPORT)));
        input
    }

    #[test]
    fn every_button_resolves_to_its_host_owned_action() {
        let mut ui = ApplicationUi::default();
        let layout = ui
            .layout(VIEWPORT.0, VIEWPORT.1)
            .expect("application layout");
        let expected = [
            "media.play_pause",
            "media.next",
            "media.previous",
            "terminal.new_window",
            "tmux.work",
            "zoom.toggle_mute",
            "stub.button_7",
            "stub.button_8",
            "stub.button_9",
        ];

        for (button, action_id) in layout.buttons.into_iter().zip(expected) {
            let x = button.bounds.x + button.bounds.width / 2;
            let y = button.bounds.y + button.bounds.height / 2;
            let mut input = tracker(&mut ui);
            assert_eq!(
                input.handle_pointer(&mut ui, pointer(V2PointerPhase::Down, x, y)),
                ApplicationInputResult {
                    activation: None,
                    pressed_button: Some(button.id),
                    visual_changed: true,
                }
            );
            assert_eq!(
                input.handle_pointer(&mut ui, pointer(V2PointerPhase::Up, x, y)),
                ApplicationInputResult {
                    activation: Some(ApplicationActivation {
                        button_id: button.id,
                        action_id,
                    }),
                    pressed_button: None,
                    visual_changed: true,
                }
            );
        }
    }

    #[test]
    fn gaps_cross_button_releases_and_out_of_bounds_cancel_without_activation() {
        let mut ui = ApplicationUi::default();
        let mut input = tracker(&mut ui);

        assert_eq!(
            input.handle_pointer(&mut ui, pointer(V2PointerPhase::Down, 10, 10)),
            ApplicationInputResult::default()
        );
        assert_eq!(
            input.handle_pointer(&mut ui, pointer(V2PointerPhase::Up, 10, 10)),
            ApplicationInputResult::default()
        );

        let pressed = input.handle_pointer(&mut ui, pointer(V2PointerPhase::Down, 100, 100));
        assert_eq!(pressed.pressed_button, Some(1));
        assert!(pressed.visual_changed);
        assert_eq!(
            input.handle_pointer(&mut ui, pointer(V2PointerPhase::Up, 500, 500)),
            ApplicationInputResult {
                visual_changed: true,
                ..ApplicationInputResult::default()
            }
        );

        assert!(
            input
                .handle_pointer(&mut ui, pointer(V2PointerPhase::Down, 100, 100))
                .visual_changed
        );
        assert_eq!(
            input.handle_pointer(&mut ui, pointer(V2PointerPhase::Up, VIEWPORT.0, VIEWPORT.1),),
            ApplicationInputResult {
                visual_changed: true,
                ..ApplicationInputResult::default()
            }
        );
    }

    #[test]
    fn unmatched_repeated_and_viewport_changes_reset_contact_and_visual_state() {
        let mut ui = ApplicationUi::default();
        let mut input = ApplicationInput::default();
        assert!(!input.is_active());
        assert_eq!(
            input.handle_pointer(&mut ui, pointer(V2PointerPhase::Up, 100, 100)),
            ApplicationInputResult::default()
        );

        assert!(!input.set_viewport(&mut ui, Some(VIEWPORT)));
        assert!(input.is_active());
        assert!(
            input
                .handle_pointer(&mut ui, pointer(V2PointerPhase::Down, 100, 100))
                .visual_changed
        );
        assert_eq!(
            input.handle_pointer(&mut ui, pointer(V2PointerPhase::Down, 100, 100)),
            ApplicationInputResult {
                visual_changed: true,
                ..ApplicationInputResult::default()
            }
        );
        assert_eq!(
            input.handle_pointer(&mut ui, pointer(V2PointerPhase::Up, 100, 100)),
            ApplicationInputResult::default()
        );

        assert!(
            input
                .handle_pointer(&mut ui, pointer(V2PointerPhase::Down, 100, 100))
                .visual_changed
        );
        assert!(input.set_viewport(&mut ui, Some((636, 776))));
        assert_eq!(
            input.handle_pointer(&mut ui, pointer(V2PointerPhase::Up, 100, 100)),
            ApplicationInputResult::default()
        );
        assert!(!input.set_viewport(&mut ui, None));
        assert!(!input.is_active());
        assert_eq!(
            input.handle_pointer(&mut ui, pointer(V2PointerPhase::Down, 100, 100)),
            ApplicationInputResult::default()
        );
    }
}
