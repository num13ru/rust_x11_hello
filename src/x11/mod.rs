//! Dormant X11 implementation retained only until native benchmark validation.
//!
//! Raw `x11rb::protocol::Event` values stop here. Downstream UI code works
//! with [`crate::ui::button::PointerEvent`] instead, so the backend could be
//! replaced or supplemented later without touching UI logic.

pub mod display;
pub mod events;
