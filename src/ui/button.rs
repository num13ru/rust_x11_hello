//! Wire-independent pointer records shared by remote input and local system UI.

use super::screen::PhysicalPoint;

/// Core-X11 button detail representing a primary touch contact.
pub const PRIMARY_BUTTON_DETAIL: u8 = 1;

/// Normalized pointer event in window-relative physical coordinates.
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
