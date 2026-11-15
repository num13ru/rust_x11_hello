//! PaperPad-owned system UI.
//!
//! The local Exit control is deliberately independent of PaperSpoon actions
//! and application-button contact tracking.

use super::button::{PRIMARY_BUTTON_DETAIL, PointerEvent, PointerEventKind};
use super::geometry::{CELL_MARGIN, LogicalRect};
use super::screen::{SYSTEM_UI_HEIGHT, ScreenLayout};

pub(crate) const CONNECTION_INDICATOR_DIAMETER: u16 = 24;

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
                            .is_some_and(|bounds| bounds.contains_physical(event.point)) =>
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
                        .is_some_and(|bounds| bounds.contains_physical(event.point));
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

    let left = connection_indicator_bounds(width, height)
        .map(|indicator| indicator.x + indicator.width + CELL_MARGIN)
        .unwrap_or(CELL_MARGIN);
    if physical_width <= left + CELL_MARGIN {
        return None;
    }

    Some(LogicalRect {
        x: left,
        y: region.y,
        width: physical_width - left - CELL_MARGIN,
        height: region.height,
    })
}

pub(crate) fn connection_indicator_bounds(width: u16, height: u16) -> Option<LogicalRect> {
    let screen = ScreenLayout::new(width, height)?;
    let region = screen.system_ui_region;
    let required_width = CELL_MARGIN
        .checked_mul(3)?
        .checked_add(CONNECTION_INDICATOR_DIAMETER)?
        .checked_add(1)?;
    if region.height < SYSTEM_UI_HEIGHT
        || region.width < required_width
        || region.height < CONNECTION_INDICATOR_DIAMETER
    {
        return None;
    }

    Some(LogicalRect {
        x: CELL_MARGIN,
        y: region.y + (region.height - CONNECTION_INDICATOR_DIAMETER) / 2,
        width: CONNECTION_INDICATOR_DIAMETER,
        height: CONNECTION_INDICATOR_DIAMETER,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::screen::PhysicalPoint;

    const WINDOW_WIDTH: u16 = 1272;
    const WINDOW_HEIGHT: u16 = 1696;

    fn pointer(kind: PointerEventKind, point: PhysicalPoint) -> PointerEvent {
        PointerEvent {
            kind,
            detail: PRIMARY_BUTTON_DETAIL,
            point,
        }
    }

    fn exit_center() -> PhysicalPoint {
        let bounds = exit_bounds(WINDOW_WIDTH, WINDOW_HEIGHT).expect("Exit bounds");
        PhysicalPoint {
            x: (bounds.x + bounds.width / 2) as i16,
            y: (bounds.y + bounds.height / 2) as i16,
        }
    }

    #[test]
    fn portrait_exit_occupies_bottom_system_strip() {
        assert_eq!(
            exit_bounds(WINDOW_WIDTH, WINDOW_HEIGHT),
            Some(LogicalRect {
                x: 64,
                y: 1624,
                width: 1188,
                height: 72,
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
            let exit = exit_bounds(width, height).expect("Exit bounds");
            let screen = ScreenLayout::new(width, height).expect("screen layout");
            let indicator = connection_indicator_bounds(width, height).expect("indicator bounds");
            assert_eq!(indicator.x, CELL_MARGIN);
            assert_eq!(indicator.y, screen.system_ui_region.y + 24);
            assert_eq!(exit.x, indicator.x + indicator.width + CELL_MARGIN);
            assert_eq!(exit.y, screen.system_ui_region.y);
            assert_eq!(exit.height, SYSTEM_UI_HEIGHT);
            assert_eq!(exit.x + exit.width + CELL_MARGIN, screen.physical_size().0);
            assert_eq!(exit.y, screen.remote_viewport.height);
            assert_eq!(exit.y + exit.height, screen.physical_size().1);
        }
    }

    #[test]
    fn exit_requires_nonzero_margined_width_and_full_system_height() {
        for (width, height) in [(0, 1696), (1272, 0), (40, 1696), (1272, 71)] {
            assert_eq!(exit_bounds(width, height), None);
        }

        assert!(exit_bounds(41, 72).is_some());
        assert!(connection_indicator_bounds(41, 72).is_none());
        assert!(exit_bounds(122, 1696).is_some());
        assert!(connection_indicator_bounds(122, 1696).is_some());
        assert!(exit_bounds(1272, 262).is_some());
    }

    #[test]
    fn connection_indicator_is_not_an_exit_hit_target() {
        let indicator =
            connection_indicator_bounds(WINDOW_WIDTH, WINDOW_HEIGHT).expect("indicator bounds");
        let point = PhysicalPoint {
            x: (indicator.x + indicator.width / 2) as i16,
            y: (indicator.y + indicator.height / 2) as i16,
        };
        let mut ui = SystemUi::default();

        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Press, point),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            None
        );
        assert_eq!(
            ui.handle_pointer(
                pointer(PointerEventKind::Release, point),
                WINDOW_WIDTH,
                WINDOW_HEIGHT
            ),
            None
        );
    }

    #[test]
    fn exit_requires_primary_down_and_up_inside() {
        let inside = exit_center();
        let outside = PhysicalPoint { x: 5, y: 5 };
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
