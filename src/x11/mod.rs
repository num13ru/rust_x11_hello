//! X11 window/input integration and reference display backend.
//!
//! Raw `x11rb::protocol::Event` values stop here. Downstream UI code works
//! with [`crate::ui::button::PointerEvent`] instead, so the backend could be
//! replaced or supplemented later without touching UI logic.

mod backend;
pub mod display;
pub mod events;
mod framebuffer;
mod render;
