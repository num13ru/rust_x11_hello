//! PaperPad-owned system UI.
//!
//! The local Exit control is deliberately independent of PaperSpoon actions
//! and application-button contact tracking.

use super::button::{PRIMARY_BUTTON_DETAIL, PointerEvent, PointerEventKind};
use super::geometry::{
    CELL_MARGIN, LABEL_TEXT_Y_OFFSET, LayoutRect, LogicalRect, TextPlacement, logical_coordinate,
};
use super::screen::{SYSTEM_UI_HEIGHT, ScreenLayout};

const EXIT_TEXT_X_OFFSET: u16 = 18;
const EXIT_TEXT: &[u8] = b"EXIT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SystemAction {
    Exit,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ExitContact {
    #[default]
    Idle,
    Armed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SystemDrawLayout {
    pub(crate) rectangle: LayoutRect,
    pub(crate) text: TextPlacement,
}

#[derive(Debug, Default)]
pub(crate) struct SystemUi {
    exit_contact: ExitContact,
}

impl SystemUi {
    pub(crate) fn handle_pointer(
        &mut self,
        event: PointerEvent,
        width: u16,
        height: u16,
    ) -> Option<SystemAction> {
        if event.detail != PRIMARY_BUTTON_DETAIL {
            return None;
        }

        match event.kind {
            PointerEventKind::Press => {
                self.exit_contact = match self.exit_contact {
                    ExitContact::Idle
                        if exit_bounds(width, height)
                            .is_some_and(|bounds| bounds.contains(event.point)) =>
                    {
                        ExitContact::Armed
                    }
                    ExitContact::Idle | ExitContact::Armed | ExitContact::Cancelled => {
                        ExitContact::Cancelled
                    }
                };
                None
            }
            PointerEventKind::Release => {
                let activated = self.exit_contact == ExitContact::Armed
                    && exit_bounds(width, height)
                        .is_some_and(|bounds| bounds.contains(event.point));
                self.exit_contact = ExitContact::Idle;
                activated.then_some(SystemAction::Exit)
            }
        }
    }

    pub(crate) fn cancel_contact(&mut self) {
        self.exit_contact = ExitContact::Idle;
    }
}

pub(crate) fn exit_bounds(width: u16, height: u16) -> Option<LogicalRect> {
    let screen = ScreenLayout::new(width, height)?;
    let region = screen.system_ui_region;
    let (physical_width, _) = screen.physical_size();
    if region.height < SYSTEM_UI_HEIGHT || region.width <= CELL_MARGIN * 2 {
        return None;
    }

    Some(LogicalRect {
        x: CELL_MARGIN,
        y: region.y,
        width: physical_width - CELL_MARGIN * 2,
        height: region.height,
    })
}

pub(crate) fn draw_layout(width: u16, height: u16) -> Option<SystemDrawLayout> {
    let bounds = exit_bounds(width, height)?;
    let center_x = u32::from(bounds.x) + u32::from(bounds.width) / 2;
    let center_y = u32::from(bounds.y) + u32::from(bounds.height) / 2;

    Some(SystemDrawLayout {
        rectangle: LayoutRect {
            x: logical_coordinate(u32::from(bounds.x)),
            y: logical_coordinate(u32::from(bounds.y)),
            width: bounds.width,
            height: bounds.height,
        },
        text: TextPlacement {
            x: logical_coordinate(center_x.saturating_sub(u32::from(EXIT_TEXT_X_OFFSET))),
            y: logical_coordinate(center_y + u32::from(LABEL_TEXT_Y_OFFSET)),
            text: EXIT_TEXT,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::geometry::{Point, WINDOW_HEIGHT, WINDOW_WIDTH, button_grid};

    fn pointer(kind: PointerEventKind, point: Point) -> PointerEvent {
        PointerEvent {
            kind,
            detail: PRIMARY_BUTTON_DETAIL,
            point,
        }
    }

    fn exit_center() -> Point {
        let bounds = exit_bounds(WINDOW_WIDTH, WINDOW_HEIGHT).expect("Exit bounds");
        Point {
            x: (bounds.x + bounds.width / 2) as i16,
            y: (bounds.y + bounds.height / 2) as i16,
        }
    }

    #[test]
    fn portrait_exit_occupies_bottom_system_strip() {
        assert_eq!(
            exit_bounds(WINDOW_WIDTH, WINDOW_HEIGHT),
            Some(LogicalRect {
                x: 20,
                y: 1624,
                width: 1232,
                height: 72,
            })
        );
        assert_eq!(
            draw_layout(WINDOW_WIDTH, WINDOW_HEIGHT),
            Some(SystemDrawLayout {
                rectangle: LayoutRect {
                    x: 20,
                    y: 1624,
                    width: 1232,
                    height: 72,
                },
                text: TextPlacement {
                    x: 618,
                    y: 1665,
                    text: b"EXIT",
                },
            })
        );
    }

    #[test]
    fn exit_stays_inside_system_region_below_remote_viewport() {
        for (width, height) in [
            (636, 848),
            (1273, 1696),
            (1274, 1696),
            (1696, 1272),
            (760, 528),
            (123, 263),
            (u16::MAX, u16::MAX),
        ] {
            let buttons = button_grid(width, height);
            let exit = exit_bounds(width, height).expect("Exit bounds");
            let screen = ScreenLayout::new(width, height).expect("screen layout");

            assert_eq!(buttons.len(), 9, "{width}x{height}");
            assert_eq!(exit.x, CELL_MARGIN);
            assert_eq!(exit.y, screen.system_ui_region.y);
            assert_eq!(exit.height, SYSTEM_UI_HEIGHT);
            assert_eq!(exit.x + exit.width + CELL_MARGIN, screen.physical_size().0);
            assert_eq!(exit.y, screen.remote_viewport.height);
            assert_eq!(exit.y + exit.height, screen.physical_size().1);
            assert!(buttons.iter().all(|button| {
                button.bounds.y + button.bounds.height <= screen.remote_viewport.height
            }));
        }
    }

    #[test]
    fn exit_requires_nonzero_margined_width_and_full_system_height() {
        for (width, height) in [(0, 1696), (1272, 0), (40, 1696), (1272, 71)] {
            assert_eq!(exit_bounds(width, height), None);
            assert_eq!(draw_layout(width, height), None);
        }

        assert!(exit_bounds(41, 72).is_some());
        assert!(exit_bounds(122, 1696).is_some());
        assert!(exit_bounds(1272, 262).is_some());
    }

    #[test]
    fn exit_requires_primary_down_and_up_inside() {
        let inside = exit_center();
        let outside = Point { x: 5, y: 5 };
        let mut ui = SystemUi::default();

        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Release, inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            None
        );
        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Press, outside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            None
        );
        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Release, inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            None
        );
        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Press, inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            None
        );
        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Release, outside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            None
        );
        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Press, inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            None
        );
        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Release, inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            Some(SystemAction::Exit)
        );
    }

    #[test]
    fn repeated_primary_press_and_cancellation_disarm_exit() {
        let inside = exit_center();
        let mut ui = SystemUi::default();

        ui.handle_pointer(
            pointer(PointerEventKind::Press, inside),
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
        );
        ui.handle_pointer(
            pointer(PointerEventKind::Press, inside),
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
        );
        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Release, inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            None
        );

        ui.handle_pointer(
            pointer(PointerEventKind::Press, inside),
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
        );
        ui.cancel_contact();
        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Release, inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            None
        );
    }

    #[test]
    fn auxiliary_details_do_not_change_exit_contact() {
        let inside = exit_center();
        let mut ui = SystemUi::default();

        ui.handle_pointer(
            pointer(PointerEventKind::Press, inside),
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
        );
        assert_eq!(
            ui.handle_pointer(
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
        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Release, inside),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            Some(SystemAction::Exit)
        );
    }
}
