//! Geometry primitives used by PaperPad-owned system UI.

use super::screen::PhysicalPoint;

pub const CELL_MARGIN: u16 = 20;

/// Half-open axis-aligned rectangle in physical window coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}
impl LogicalRect {
    pub(crate) fn contains_physical(self, point: PhysicalPoint) -> bool {
        let point_x = i32::from(point.x);
        let point_y = i32::from(point.y);
        let left = i32::from(self.x);
        let top = i32::from(self.y);
        let right = left + i32::from(self.width);
        let bottom = top + i32::from(self.height);

        point_x >= left && point_x < right && point_y >= top && point_y < bottom
    }
}
