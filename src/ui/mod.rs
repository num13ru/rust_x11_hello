//! Logical UI concepts: geometry, hit testing, and contact tracking.
//!
//! This layer is wire-independent. `x11` maps raw screen events onto
//! [`crate::ui::button::PointerEvent`] and keeps `x11rb` types out of here.

pub mod action;
pub mod button;
pub mod geometry;
