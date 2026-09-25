//! Geometry primitives used by PaperPad-owned system UI.

use super::screen::PhysicalPoint;

pub const CELL_MARGIN: u16 = 20;
pub const LABEL_TEXT_Y_OFFSET: u16 = 5;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayoutRect {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextPlacement {
    pub x: i16,
    pub y: i16,
    pub text: &'static [u8],
}

pub(crate) fn logical_coordinate(value: u32) -> i16 {
    value.min(i16::MAX as u32) as i16
}
