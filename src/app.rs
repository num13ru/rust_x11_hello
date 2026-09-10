//! Wire- and X11-independent application state and decisions.

use crate::ui::action::{SemanticAction, action_for_button};
use crate::ui::button::{ContactTracker, PointerEvent, handle_pointer_event};
use crate::ui::screen::ScreenLayout;
use crate::ui::system::{SystemAction, SystemUi};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Redraw<'a> {
    size: (u16, u16),
    status_text: Option<&'a str>,
}

impl<'a> Redraw<'a> {
    pub(crate) fn size(self) -> (u16, u16) {
        self.size
    }

    pub(crate) fn status_text(self) -> Option<&'a str> {
        self.status_text
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GeometryUpdate<'a> {
    IgnoredZero,
    Unchanged,
    Redraw(Redraw<'a>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Activation {
    Send {
        button_id: u8,
        action: SemanticAction,
    },
    Exit,
    Unknown {
        button_id: u8,
    },
}

pub(crate) struct AppState {
    width: u16,
    height: u16,
    application_contact: ContactTracker,
    system_ui: SystemUi,
    status_text: Option<String>,
}

impl AppState {
    pub(crate) fn new((width, height): (u16, u16)) -> Self {
        Self {
            width,
            height,
            application_contact: ContactTracker::default(),
            system_ui: SystemUi::default(),
            status_text: None,
        }
    }

    pub(crate) fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub(crate) fn redraw(&self) -> Redraw<'_> {
        Redraw {
            size: self.size(),
            status_text: self.status_text.as_deref(),
        }
    }

    pub(crate) fn set_status_text(&mut self, text: String) -> Redraw<'_> {
        self.status_text = Some(text);
        self.redraw()
    }

    pub(crate) fn update_geometry(&mut self, reported: (u16, u16)) -> GeometryUpdate<'_> {
        if reported.0 == 0 || reported.1 == 0 {
            GeometryUpdate::IgnoredZero
        } else if reported == self.size() {
            GeometryUpdate::Unchanged
        } else {
            self.application_contact.cancel();
            self.system_ui.cancel_contact();
            self.width = reported.0;
            self.height = reported.1;
            GeometryUpdate::Redraw(self.redraw())
        }
    }

    pub(crate) fn handle_pointer(&mut self, event: PointerEvent) -> Option<Activation> {
        let system_action = self
            .system_ui
            .handle_pointer(event, self.width, self.height);
        let (remote_point, remote_size) = match ScreenLayout::new(self.width, self.height) {
            Some(layout) => (
                layout.physical_to_remote(event.point),
                (layout.remote_viewport.width, layout.remote_viewport.height),
            ),
            None => (None, (0, 0)),
        };
        let application_button = handle_pointer_event(
            &mut self.application_contact,
            event.kind,
            event.detail,
            remote_point,
            remote_size.0,
            remote_size.1,
        );

        match system_action {
            Some(SystemAction::Exit) => Some(Activation::Exit),
            None => application_button.map(activation_for_button),
        }
    }

    pub(crate) fn cancel_contact(&mut self) {
        self.application_contact.cancel();
        self.system_ui.cancel_contact();
    }
}

fn activation_for_button(button_id: u8) -> Activation {
    match action_for_button(button_id) {
        Some(action) => Activation::Send { button_id, action },
        None => Activation::Unknown { button_id },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::button::{PRIMARY_BUTTON_DETAIL, PointerEventKind};
    use crate::ui::geometry::button_grid;
    use crate::ui::screen::PhysicalPoint;
    use crate::ui::system::exit_bounds;

    const SIZE: (u16, u16) = (1272, 1696);

    fn center_of(button_id: u8, size: (u16, u16)) -> PhysicalPoint {
        let screen = ScreenLayout::new(size.0, size.1).expect("screen layout");
        let button = button_grid(screen.remote_viewport.width, screen.remote_viewport.height)
            .into_iter()
            .find(|button| button.id == button_id)
            .expect("button exists");
        PhysicalPoint {
            x: (screen.remote_viewport.x + button.bounds.x + button.bounds.width / 2) as i16,
            y: (screen.remote_viewport.y + button.bounds.y + button.bounds.height / 2) as i16,
        }
    }

    fn pointer(kind: PointerEventKind, point: PhysicalPoint) -> PointerEvent {
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
        let exit = exit_bounds(SIZE.0, SIZE.1).expect("Exit bounds");
        let exit_point = PhysicalPoint {
            x: (exit.x + exit.width / 2) as i16,
            y: (exit.y + exit.height / 2) as i16,
        };
        assert_eq!(
            state.handle_pointer(pointer(PointerEventKind::Press, exit_point)),
            None
        );
        assert_eq!(
            state.handle_pointer(pointer(PointerEventKind::Release, exit_point)),
            Some(Activation::Exit)
        );
        assert_eq!(
            activation_for_button(u8::MAX),
            Activation::Unknown { button_id: u8::MAX }
        );
    }

    #[test]
    fn application_and_exit_contacts_cannot_cross_activate() {
        let application_point = center_of(1, SIZE);
        let exit = exit_bounds(SIZE.0, SIZE.1).expect("Exit bounds");
        let exit_point = PhysicalPoint {
            x: (exit.x + exit.width / 2) as i16,
            y: (exit.y + exit.height / 2) as i16,
        };
        let mut state = AppState::new(SIZE);

        assert_eq!(
            state.handle_pointer(pointer(PointerEventKind::Press, application_point)),
            None
        );
        assert_eq!(
            state.handle_pointer(pointer(PointerEventKind::Release, exit_point)),
            None
        );
        assert_eq!(
            state.handle_pointer(pointer(PointerEventKind::Press, exit_point)),
            None
        );
        assert_eq!(
            state.handle_pointer(pointer(PointerEventKind::Release, application_point)),
            None
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
        let _ = state.set_status_text("connected".into());

        let resized = (SIZE.0 / 2, SIZE.1 / 2);
        assert_eq!(
            state.update_geometry(resized),
            GeometryUpdate::Redraw(Redraw {
                size: resized,
                status_text: Some("connected"),
            })
        );
        assert_eq!(state.size(), resized);
        assert_eq!(
            state.handle_pointer(pointer(PointerEventKind::Release, point)),
            None
        );
    }

    #[test]
    fn status_update_requests_redraw_and_replaces_text_without_transport_knowledge() {
        let mut state = AppState::new(SIZE);
        assert_eq!(
            state.redraw(),
            Redraw {
                size: SIZE,
                status_text: None,
            }
        );
        assert_eq!(
            state.set_status_text("first".into()),
            Redraw {
                size: SIZE,
                status_text: Some("first"),
            }
        );
        assert_eq!(
            state.set_status_text("second".into()),
            Redraw {
                size: SIZE,
                status_text: Some("second"),
            }
        );
    }
}
