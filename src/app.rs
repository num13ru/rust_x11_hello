//! Wire- and X11-independent application state and decisions.

use crate::ui::action::{SemanticAction, action_for_button};
use crate::ui::button::{ContactTracker, PointerEvent, handle_pointer_event};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GeometryUpdate {
    IgnoredZero,
    Unchanged,
    Changed { width: u16, height: u16 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Activation {
    Send {
        button_id: u8,
        action: SemanticAction,
    },
    Exit {
        button_id: u8,
    },
    Unknown {
        button_id: u8,
    },
}

pub(crate) struct AppState {
    width: u16,
    height: u16,
    contact: ContactTracker,
    status_text: Option<String>,
}

impl AppState {
    pub(crate) fn new((width, height): (u16, u16)) -> Self {
        Self {
            width,
            height,
            contact: ContactTracker::default(),
            status_text: None,
        }
    }

    pub(crate) fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub(crate) fn status_text(&self) -> Option<&str> {
        self.status_text.as_deref()
    }

    pub(crate) fn set_status_text(&mut self, text: String) {
        self.status_text = Some(text);
    }

    pub(crate) fn update_geometry(&mut self, reported: (u16, u16)) -> GeometryUpdate {
        if reported.0 == 0 || reported.1 == 0 {
            GeometryUpdate::IgnoredZero
        } else if reported == self.size() {
            GeometryUpdate::Unchanged
        } else {
            self.contact.cancel();
            self.width = reported.0;
            self.height = reported.1;
            GeometryUpdate::Changed {
                width: reported.0,
                height: reported.1,
            }
        }
    }

    pub(crate) fn handle_pointer(&mut self, event: PointerEvent) -> Option<Activation> {
        let button_id = handle_pointer_event(&mut self.contact, event, self.width, self.height)?;
        Some(activation_for_button(button_id))
    }

    pub(crate) fn cancel_contact(&mut self) {
        self.contact.cancel();
    }
}

fn activation_for_button(button_id: u8) -> Activation {
    match action_for_button(button_id) {
        Some(SemanticAction::Exit) => Activation::Exit { button_id },
        Some(action) => Activation::Send { button_id, action },
        None => Activation::Unknown { button_id },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::button::{PRIMARY_BUTTON_DETAIL, PointerEventKind};
    use crate::ui::geometry::{EXIT_BUTTON_ID, Point, button_grid};

    const SIZE: (u16, u16) = (1272, 1696);

    fn center_of(button_id: u8, size: (u16, u16)) -> Point {
        let button = button_grid(size.0, size.1)
            .into_iter()
            .find(|button| button.id == button_id)
            .expect("button exists");
        Point {
            x: (button.bounds.x + button.bounds.width / 2) as i16,
            y: (button.bounds.y + button.bounds.height / 2) as i16,
        }
    }

    fn pointer(kind: PointerEventKind, point: Point) -> PointerEvent {
        PointerEvent {
            kind,
            detail: PRIMARY_BUTTON_DETAIL,
            point,
        }
    }

    fn activate(state: &mut AppState, button_id: u8) -> Option<Activation> {
        let point = center_of(button_id, state.size());
        assert_eq!(
            state.handle_pointer(pointer(PointerEventKind::Press, point)),
            None
        );
        state.handle_pointer(pointer(PointerEventKind::Release, point))
    }

    #[test]
    fn pointer_activation_distinguishes_send_exit_and_unknown() {
        let mut state = AppState::new(SIZE);
        assert_eq!(
            activate(&mut state, 1),
            Some(Activation::Send {
                button_id: 1,
                action: SemanticAction::MediaPlayPause,
            })
        );
        assert_eq!(
            activate(&mut state, EXIT_BUTTON_ID),
            Some(Activation::Exit {
                button_id: EXIT_BUTTON_ID,
            })
        );
        assert_eq!(
            activation_for_button(u8::MAX),
            Activation::Unknown { button_id: u8::MAX }
        );
    }

    #[test]
    fn geometry_updates_ignore_zero_and_cancel_contact_on_change() {
        let mut state = AppState::new(SIZE);
        let point = center_of(1, SIZE);
        assert_eq!(
            state.handle_pointer(pointer(PointerEventKind::Press, point)),
            None
        );
        assert_eq!(
            state.update_geometry((0, SIZE.1)),
            GeometryUpdate::IgnoredZero
        );
        assert_eq!(state.size(), SIZE);
        assert_eq!(state.update_geometry(SIZE), GeometryUpdate::Unchanged);

        let resized = (SIZE.0 / 2, SIZE.1 / 2);
        assert_eq!(
            state.update_geometry(resized),
            GeometryUpdate::Changed {
                width: resized.0,
                height: resized.1,
            }
        );
        assert_eq!(state.size(), resized);
        assert_eq!(
            state.handle_pointer(pointer(PointerEventKind::Release, point)),
            None
        );
    }

    #[test]
    fn status_text_is_replaced_without_transport_knowledge() {
        let mut state = AppState::new(SIZE);
        assert_eq!(state.status_text(), None);
        state.set_status_text("first".into());
        state.set_status_text("second".into());
        assert_eq!(state.status_text(), Some("second"));
    }
}
