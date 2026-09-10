//! Logical button hit testing and contact tracking.
//!
//! The UI layer consumes a generic [`PointerEvent`] and never sees X11 event
//! structures. Only primary contacts (`detail == PRIMARY_BUTTON_DETAIL`)
//! participate in activation; auxiliary details observed on the Kindle (6, 9)
//! are ignored by the tracker.

use super::geometry::{LogicalButton, button_grid};
use super::screen::{PhysicalPoint, RemotePoint};

/// The core-X11 button detail that represents a primary touch contact.
pub const PRIMARY_BUTTON_DETAIL: u8 = 1;

/// A wire-independent pointer event in window-relative coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PointerEvent {
    pub kind: PointerEventKind,
    pub detail: u8,
    pub point: PhysicalPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerEventKind {
    Press,
    Release,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ContactState {
    #[default]
    Idle,
    Armed(u8),
    Cancelled,
}

/// Tracks one primary contact through press/release, using logical buttons.
///
/// A primary press arms the button under the initial coordinate. A matching
/// primary release activates only while still inside that same button.
/// Repeated primary presses, presses outside the grid, releases elsewhere, and
/// explicit cancellation discard the contact. Unmatched releases do nothing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ContactTracker {
    state: ContactState,
}

pub fn hit_button(buttons: &[LogicalButton], point: RemotePoint) -> Option<&LogicalButton> {
    buttons
        .iter()
        .find(|button| button.bounds.contains_remote(point))
}

impl ContactTracker {
    #[cfg(test)]
    pub fn press(&mut self, detail: u8, point: RemotePoint, buttons: &[LogicalButton]) {
        self.press_optional(detail, Some(point), buttons);
    }

    fn press_optional(
        &mut self,
        detail: u8,
        point: Option<RemotePoint>,
        buttons: &[LogicalButton],
    ) {
        if detail != PRIMARY_BUTTON_DETAIL {
            return;
        }

        self.state = match self.state {
            ContactState::Idle => point
                .and_then(|point| hit_button(buttons, point))
                .map(|button| ContactState::Armed(button.id))
                .unwrap_or(ContactState::Cancelled),
            ContactState::Armed(_) | ContactState::Cancelled => ContactState::Cancelled,
        };
    }

    #[cfg(test)]
    pub fn release(
        &mut self,
        detail: u8,
        point: RemotePoint,
        buttons: &[LogicalButton],
    ) -> Option<u8> {
        self.release_optional(detail, Some(point), buttons)
    }

    fn release_optional(
        &mut self,
        detail: u8,
        point: Option<RemotePoint>,
        buttons: &[LogicalButton],
    ) -> Option<u8> {
        if detail != PRIMARY_BUTTON_DETAIL {
            return None;
        }

        match std::mem::take(&mut self.state) {
            ContactState::Armed(armed_id)
                if point
                    .and_then(|point| hit_button(buttons, point))
                    .map(|button| button.id)
                    == Some(armed_id) =>
            {
                Some(armed_id)
            }
            ContactState::Idle | ContactState::Armed(_) | ContactState::Cancelled => None,
        }
    }

    pub fn cancel(&mut self) {
        self.state = ContactState::Idle;
    }
}

/// Feed a wire-independent pointer event into the tracker. Returns the
/// activated button id on a matched primary release.
pub fn handle_pointer_event(
    tracker: &mut ContactTracker,
    kind: PointerEventKind,
    detail: u8,
    point: Option<RemotePoint>,
    width: u16,
    height: u16,
) -> Option<u8> {
    let buttons = button_grid(width, height);
    match kind {
        PointerEventKind::Press => {
            tracker.press_optional(detail, point, &buttons);
            None
        }
        PointerEventKind::Release => tracker.release_optional(detail, point, &buttons),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::geometry::{
        CELL_MARGIN, GRID_RECT_LEFT, GRID_RECT_TOP, GRID_ROWS_CELL_HEIGHT, GRID_ROWS_CELL_WIDTH,
        WINDOW_HEIGHT as PHYSICAL_HEIGHT, WINDOW_WIDTH,
    };
    use crate::ui::screen::SYSTEM_UI_HEIGHT;

    const WINDOW_HEIGHT: u16 = PHYSICAL_HEIGHT - SYSTEM_UI_HEIGHT;

    /// Center point of grid cell `(row, column)` at the reference size.
    fn cell_center(row: u16, column: u16) -> RemotePoint {
        RemotePoint {
            x: (GRID_RECT_LEFT
                + column * (GRID_ROWS_CELL_WIDTH + CELL_MARGIN * 2)
                + GRID_ROWS_CELL_WIDTH / 2),
            y: (GRID_RECT_TOP
                + row * (GRID_ROWS_CELL_HEIGHT + CELL_MARGIN * 2)
                + GRID_ROWS_CELL_HEIGHT / 2),
        }
    }

    /// A point inside the boundary of cell `(row, column)` at the reference size.
    fn inside_cell(row: u16, column: u16) -> RemotePoint {
        RemotePoint {
            x: GRID_RECT_LEFT + column * (GRID_ROWS_CELL_WIDTH + CELL_MARGIN * 2),
            y: GRID_RECT_TOP + row * (GRID_ROWS_CELL_HEIGHT + CELL_MARGIN * 2),
        }
    }

    #[test]
    fn grid_hit_testing_uses_half_open_edges() {
        let buttons = button_grid(WINDOW_WIDTH, WINDOW_HEIGHT);

        // Interior points of each cell hit their cell.
        assert_eq!(hit_button(&buttons, cell_center(0, 0)).unwrap().id, 1);
        assert_eq!(hit_button(&buttons, cell_center(1, 2)).unwrap().id, 6);
        // Points just inside the leading/trailing edge of a cell.
        assert_eq!(hit_button(&buttons, inside_cell(0, 1)).unwrap().id, 2);
        // The next column begins after the non-interactive gap.
        assert_eq!(hit_button(&buttons, inside_cell(0, 1)).unwrap().id, 2);
        // The next row begins after the non-interactive gap.
        assert_eq!(
            hit_button(
                &buttons,
                RemotePoint {
                    x: GRID_RECT_LEFT + GRID_ROWS_CELL_WIDTH / 2,
                    y: GRID_RECT_TOP + GRID_ROWS_CELL_HEIGHT + CELL_MARGIN * 2,
                }
            )
            .unwrap()
            .id,
            4
        );
        // Just outside the grid: no hit.
        assert!(
            hit_button(
                &buttons,
                RemotePoint {
                    x: GRID_RECT_LEFT - 1,
                    y: GRID_RECT_TOP,
                }
            )
            .is_none()
        );
        // The remote viewport's trailing edge is outside its half-open bounds.
        assert!(
            hit_button(
                &buttons,
                RemotePoint {
                    x: WINDOW_WIDTH,
                    y: cell_center(0, 0).y
                }
            )
            .is_none()
        );
    }

    #[test]
    fn gaps_and_outer_margins_do_not_activate_buttons() {
        let buttons = button_grid(WINDOW_WIDTH, WINDOW_HEIGHT);
        let mut contact = ContactTracker::default();
        for point in [
            RemotePoint {
                x: GRID_RECT_LEFT + GRID_ROWS_CELL_WIDTH,
                y: cell_center(0, 0).y,
            },
            RemotePoint {
                x: cell_center(0, 0).x,
                y: GRID_RECT_TOP + GRID_ROWS_CELL_HEIGHT,
            },
            RemotePoint {
                x: 0,
                y: cell_center(0, 0).y,
            },
            RemotePoint {
                x: WINDOW_WIDTH - CELL_MARGIN,
                y: cell_center(0, 0).y,
            },
        ] {
            assert!(hit_button(&buttons, point).is_none());
            contact.press(PRIMARY_BUTTON_DETAIL, cell_center(0, 0), &buttons);
            assert_eq!(
                contact.release(PRIMARY_BUTTON_DETAIL, point, &buttons),
                None
            );
            contact.press(PRIMARY_BUTTON_DETAIL, point, &buttons);
            assert_eq!(
                contact.release(PRIMARY_BUTTON_DETAIL, cell_center(0, 0), &buttons),
                None
            );
        }
    }

    #[test]
    fn third_row_buttons_are_distinct_activation_targets() {
        let buttons = button_grid(WINDOW_WIDTH, WINDOW_HEIGHT);
        let mut contact = ContactTracker::default();

        for column in 0..3 {
            let point = cell_center(2, column);
            let expected_id = 7 + column as u8;
            assert_eq!(hit_button(&buttons, point).unwrap().id, expected_id);
            contact.press(PRIMARY_BUTTON_DETAIL, point, &buttons);
            assert_eq!(
                contact.release(PRIMARY_BUTTON_DETAIL, point, &buttons),
                Some(expected_id)
            );

            contact.press(PRIMARY_BUTTON_DETAIL, point, &buttons);
            assert_eq!(
                contact.release(PRIMARY_BUTTON_DETAIL, RemotePoint { x: 5, y: 5 }, &buttons),
                None
            );
        }
    }

    #[test]
    fn primary_press_and_same_button_release_activate_once() {
        let buttons = button_grid(WINDOW_WIDTH, WINDOW_HEIGHT);
        let mut contact = ContactTracker::default();
        let inside_button_4 = cell_center(1, 0);
        let nearby_in_4 = inside_button_4;

        contact.press(PRIMARY_BUTTON_DETAIL, inside_button_4, &buttons);
        assert_eq!(contact.state, ContactState::Armed(4));

        contact.press(9, nearby_in_4, &buttons);
        assert_eq!(contact.state, ContactState::Armed(4));
        assert_eq!(contact.release(9, inside_button_4, &buttons), None);
        assert_eq!(contact.state, ContactState::Armed(4));

        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, inside_button_4, &buttons),
            Some(4)
        );
        assert_eq!(contact.state, ContactState::Idle);
        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, inside_button_4, &buttons),
            None
        );
    }
    #[test]
    fn primary_contact_cancels_on_mismatch_repeat_outside_or_geometry_change() {
        let buttons = button_grid(WINDOW_WIDTH, WINDOW_HEIGHT);
        let button_1 = cell_center(0, 0);
        let button_2 = cell_center(0, 1);
        let outside = RemotePoint { x: 5, y: 5 };
        let mut contact = ContactTracker::default();

        contact.press(PRIMARY_BUTTON_DETAIL, button_1, &buttons);
        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, button_2, &buttons),
            None
        );

        contact.press(PRIMARY_BUTTON_DETAIL, outside, &buttons);
        assert_eq!(contact.state, ContactState::Cancelled);
        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, button_1, &buttons),
            None
        );

        contact.press(PRIMARY_BUTTON_DETAIL, button_1, &buttons);
        contact.press(PRIMARY_BUTTON_DETAIL, button_1, &buttons);
        assert_eq!(contact.state, ContactState::Cancelled);
        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, button_1, &buttons),
            None
        );

        contact.press(PRIMARY_BUTTON_DETAIL, button_1, &buttons);
        contact.cancel();
        assert_eq!(contact.state, ContactState::Idle);
        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, button_1, &buttons),
            None
        );
    }

    #[test]
    fn pointer_events_wire_auxiliary_details_do_not_touch_primary_state() {
        let mut contact = ContactTracker::default();
        let inside = cell_center(1, 0);
        let outside = RemotePoint { x: 5, y: 5 };

        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEventKind::Press,
                6,
                Some(inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
            ),
            None
        );
        assert_eq!(contact.state, ContactState::Idle);

        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEventKind::Press,
                PRIMARY_BUTTON_DETAIL,
                Some(inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
            ),
            None
        );
        assert_eq!(contact.state, ContactState::Armed(4));

        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEventKind::Release,
                6,
                Some(outside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
            ),
            None
        );
        assert_eq!(contact.state, ContactState::Armed(4));

        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEventKind::Release,
                PRIMARY_BUTTON_DETAIL,
                Some(inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
            ),
            Some(4)
        );
        assert_eq!(contact.state, ContactState::Idle);
    }

    #[test]
    fn release_outside_remote_viewport_cancels_application_contact() {
        let mut contact = ContactTracker::default();
        let inside = cell_center(0, 0);

        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEventKind::Press,
                PRIMARY_BUTTON_DETAIL,
                Some(inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
            ),
            None
        );
        assert_eq!(contact.state, ContactState::Armed(1));
        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEventKind::Release,
                PRIMARY_BUTTON_DETAIL,
                None,
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
            ),
            None
        );
        assert_eq!(contact.state, ContactState::Idle);
    }
}
