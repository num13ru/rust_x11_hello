//! Logical button hit testing and contact tracking.
//!
//! The UI layer consumes a generic [`PointerEvent`] and never sees X11 event
//! structures. Only primary contacts (`detail == PRIMARY_BUTTON_DETAIL`)
//! participate in activation; auxiliary details observed on the Kindle (6, 9)
//! are ignored by the tracker.

use super::geometry::{LogicalButton, Point, button_grid};

/// The core-X11 button detail that represents a primary touch contact.
pub const PRIMARY_BUTTON_DETAIL: u8 = 1;

/// A wire-independent pointer event in window-relative coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PointerEvent {
    pub kind: PointerEventKind,
    pub detail: u8,
    pub point: Point,
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

pub fn hit_button(buttons: &[LogicalButton], point: Point) -> Option<&LogicalButton> {
    buttons.iter().find(|button| button.bounds.contains(point))
}

impl ContactTracker {
    pub fn press(&mut self, detail: u8, point: Point, buttons: &[LogicalButton]) {
        if detail != PRIMARY_BUTTON_DETAIL {
            return;
        }

        self.state = match self.state {
            ContactState::Idle => hit_button(buttons, point)
                .map(|button| ContactState::Armed(button.id))
                .unwrap_or(ContactState::Cancelled),
            ContactState::Armed(_) | ContactState::Cancelled => ContactState::Cancelled,
        };
    }

    pub fn release(&mut self, detail: u8, point: Point, buttons: &[LogicalButton]) -> Option<u8> {
        if detail != PRIMARY_BUTTON_DETAIL {
            return None;
        }

        match std::mem::take(&mut self.state) {
            ContactState::Armed(armed_id)
                if hit_button(buttons, point).map(|button| button.id) == Some(armed_id) =>
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
    event: PointerEvent,
    width: u16,
    height: u16,
) -> Option<u8> {
    let buttons = button_grid(width, height);
    match event.kind {
        PointerEventKind::Press => {
            tracker.press(event.detail, event.point, &buttons);
            None
        }
        PointerEventKind::Release => tracker.release(event.detail, event.point, &buttons),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::geometry::{
        EXIT_BAR_HEIGHT, EXIT_BAR_RECT_TOP, EXIT_BUTTON_ID, GRID_RECT_LEFT, GRID_RECT_TOP,
        GRID_ROWS_CELL_HEIGHT, GRID_ROWS_CELL_WIDTH, WINDOW_HEIGHT, WINDOW_WIDTH,
    };

    /// Center point of grid cell `(row, column)` at the reference size.
    fn cell_center(row: u16, column: u16) -> Point {
        Point {
            x: (GRID_RECT_LEFT + column * GRID_ROWS_CELL_WIDTH + GRID_ROWS_CELL_WIDTH / 2) as i16,
            y: (GRID_RECT_TOP + row * GRID_ROWS_CELL_HEIGHT + GRID_ROWS_CELL_HEIGHT / 2) as i16,
        }
    }

    /// A point inside the boundary of cell `(row, column)` at the reference size.
    fn inside_cell(row: u16, column: u16) -> Point {
        Point {
            x: (GRID_RECT_LEFT + column * GRID_ROWS_CELL_WIDTH) as i16,
            y: (GRID_RECT_TOP + row * GRID_ROWS_CELL_HEIGHT) as i16,
        }
    }

    /// center of the exit bar at the reference size.
    fn exit_bar_center() -> Point {
        Point {
            x: (WINDOW_WIDTH / 2) as i16,
            y: (EXIT_BAR_RECT_TOP + EXIT_BAR_HEIGHT / 2) as i16,
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
        // The shared vertical edge belongs to the right-hand cell (half-open).
        assert_eq!(hit_button(&buttons, inside_cell(0, 1)).unwrap().id, 2);
        // The shared horizontal edge belongs to the lower cell.
        assert_eq!(
            hit_button(
                &buttons,
                Point {
                    x: (GRID_RECT_LEFT + GRID_ROWS_CELL_WIDTH / 2) as i16,
                    y: (GRID_RECT_TOP + GRID_ROWS_CELL_HEIGHT) as i16,
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
                Point {
                    x: GRID_RECT_LEFT as i16 - 1,
                    y: GRID_RECT_TOP as i16,
                }
            )
            .is_none()
        );
        // Exit bar: full width, below the grid.
        assert_eq!(
            hit_button(&buttons, exit_bar_center()).unwrap().id,
            EXIT_BUTTON_ID
        );
        assert_eq!(
            hit_button(
                &buttons,
                Point {
                    x: WINDOW_WIDTH as i16 - 1,
                    y: (EXIT_BAR_RECT_TOP + 1) as i16,
                }
            )
            .unwrap()
            .id,
            EXIT_BUTTON_ID
        );
        // Outside the window: no hit. (Negative x is a boundary sentinel.)
        assert!(
            hit_button(
                &buttons,
                Point {
                    x: WINDOW_WIDTH as i16,
                    y: exit_bar_center().y
                }
            )
            .is_none()
        );
        assert!(
            hit_button(
                &buttons,
                Point {
                    x: -1,
                    y: GRID_RECT_TOP as i16
                }
            )
            .is_none()
        );
    }

    #[test]
    fn third_row_and_exit_are_distinct_activation_targets() {
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
                contact.release(PRIMARY_BUTTON_DETAIL, exit_bar_center(), &buttons),
                None
            );
        }

        contact.press(PRIMARY_BUTTON_DETAIL, exit_bar_center(), &buttons);
        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, exit_bar_center(), &buttons),
            Some(EXIT_BUTTON_ID)
        );
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
        let outside = Point { x: 5, y: 5 };
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
        let outside = Point { x: 5, y: 5 };

        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEvent {
                    kind: PointerEventKind::Press,
                    detail: 6,
                    point: inside,
                },
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
            ),
            None
        );
        assert_eq!(contact.state, ContactState::Idle);

        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEvent {
                    kind: PointerEventKind::Press,
                    detail: PRIMARY_BUTTON_DETAIL,
                    point: inside,
                },
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
            ),
            None
        );
        assert_eq!(contact.state, ContactState::Armed(4));

        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEvent {
                    kind: PointerEventKind::Release,
                    detail: 6,
                    point: outside,
                },
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
            ),
            None
        );
        assert_eq!(contact.state, ContactState::Armed(4));

        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEvent {
                    kind: PointerEventKind::Release,
                    detail: PRIMARY_BUTTON_DETAIL,
                    point: inside,
                },
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
            ),
            Some(4)
        );
        assert_eq!(contact.state, ContactState::Idle);
    }
}
