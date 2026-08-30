//! Pure geometry and layout math for the logical button grid.
//!
//! This module has no X11 dependency: it converts a window extent into logical
//! button rectangles, scaled text positions, and text label placements. The X11
//! layer maps these onto its own wire types.

/// Reference window width the fixed Kindle layout was verified at.
pub const WINDOW_WIDTH: u16 = 760;
/// Reference window height the fixed Kindle layout was verified at.
pub const WINDOW_HEIGHT: u16 = 360;
pub const GRID_COLUMNS: u16 = 3;
pub const GRID_ROWS: u16 = 2;
pub const GRID_LEFT_INSET: u16 = 20;
pub const GRID_RIGHT_INSET: u16 = 20;
pub const GRID_TOP_INSET: u16 = 60;
pub const GRID_BOTTOM_INSET: u16 = 20;
/// Height of the full-width exit bar below the button grid.
pub const EXIT_BAR_HEIGHT: u16 = 36;
pub const TITLE_X: u16 = 20;
pub const TITLE_BASELINE: u16 = 40;
pub const TITLE_TEXT: &[u8] = b"Core X11 button grid: tap 1-6";
pub const BUTTON_LABELS: [&[u8]; 6] = [b"1", b"2", b"3", b"4", b"5", b"6"];
/// Integer 2-D point in window-relative coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Point {
    pub x: i16,
    pub y: i16,
}

/// Half-open axis-aligned rectangle in logical coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl LogicalRect {
    pub fn contains(self, point: Point) -> bool {
        let point_x = i32::from(point.x);
        let point_y = i32::from(point.y);
        let left = i32::from(self.x);
        let top = i32::from(self.y);

        point_x >= left
            && point_y >= top
            && point_x < left + i32::from(self.width)
            && point_y < top + i32::from(self.height)
    }
}

/// One logical button of the grid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalButton {
    pub id: u8,
    pub bounds: LogicalRect,
}

/// A wire-independent rectangle, consumed by the drawing layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayoutRect {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

/// A text run to draw at a position.
#[derive(Clone, Copy, Debug)]
pub struct TextPlacement {
    pub x: i16,
    pub y: i16,
    pub text: &'static [u8],
}

/// A complete layout ready for the drawing layer.
#[derive(Debug)]
pub struct DrawLayout {
    pub rectangles: Vec<LayoutRect>,
    pub text: Vec<TextPlacement>,
}

fn scale_u16(reference_value: u16, actual_extent: u16, reference_extent: u16) -> u16 {
    debug_assert_ne!(reference_extent, 0);
    ((u32::from(reference_value) * u32::from(actual_extent)) / u32::from(reference_extent))
        .min(u32::from(u16::MAX)) as u16
}

fn scale_i16(reference_value: u16, actual_extent: u16, reference_extent: u16) -> i16 {
    scale_u16(reference_value, actual_extent, reference_extent).min(i16::MAX as u16) as i16
}

fn bounded_insets(
    actual_extent: u16,
    minimum_content: u16,
    reference_leading: u16,
    reference_trailing: u16,
    reference_extent: u16,
) -> (u16, u16) {
    let margin_budget = actual_extent.saturating_sub(minimum_content);
    let leading = scale_u16(reference_leading, actual_extent, reference_extent).min(margin_budget);
    let trailing = scale_u16(reference_trailing, actual_extent, reference_extent)
        .min(margin_budget.saturating_sub(leading));
    (leading, trailing)
}

fn partition_offset(extent: u16, index: u16, parts: u16) -> u16 {
    ((u32::from(extent) * u32::from(index)) / u32::from(parts)) as u16
}

/// Build the 2×3 logical button grid for a window extent.
///
/// Returns an empty grid when the extent cannot host `GRID_ROWS` full-height
/// and `GRID_COLUMNS` full-width cells, so hit testing then finds nothing.
pub fn button_grid(width: u16, height: u16) -> Vec<LogicalButton> {
    if width < GRID_COLUMNS || height < GRID_ROWS {
        return Vec::new();
    }

    let (left, right) = bounded_insets(
        width,
        GRID_COLUMNS,
        GRID_LEFT_INSET,
        GRID_RIGHT_INSET,
        WINDOW_WIDTH,
    );
    let (top, bottom) = bounded_insets(
        height,
        GRID_ROWS,
        GRID_TOP_INSET,
        GRID_BOTTOM_INSET,
        WINDOW_HEIGHT,
    );
    let grid_width = width - left - right;
    let grid_height = height - top - bottom;
    let mut buttons = Vec::with_capacity(usize::from(GRID_COLUMNS * GRID_ROWS) + 1);

    for row in 0..GRID_ROWS {
        let row_start = partition_offset(grid_height, row, GRID_ROWS);
        let row_end = partition_offset(grid_height, row + 1, GRID_ROWS);
        for column in 0..GRID_COLUMNS {
            let column_start = partition_offset(grid_width, column, GRID_COLUMNS);
            let column_end = partition_offset(grid_width, column + 1, GRID_COLUMNS);
            buttons.push(LogicalButton {
                id: (row * GRID_COLUMNS + column + 1) as u8,
                bounds: LogicalRect {
                    x: left + column_start,
                    y: top + row_start,
                    width: column_end - column_start,
                    height: row_end - row_start,
                },
            });
        }
    }

    // Full-width exit bar at the bottom, above the bottom inset.
    let exit_bar = if height >= EXIT_BAR_HEIGHT {
        let y = height - EXIT_BAR_HEIGHT;
        Some(LogicalButton {
            id: 7,
            bounds: LogicalRect {
                x: 0,
                y,
                width,
                height: EXIT_BAR_HEIGHT,
            },
        })
    } else {
        None
    };
    if let Some(button) = exit_bar {
        buttons.push(button);
    }

    buttons
}
fn logical_coordinate(value: u32) -> i16 {
    value.min(i16::MAX as u32) as i16
}

/// Compute the layout to draw for a window extent, or `None` for zero extents.
pub fn draw_layout(width: u16, height: u16) -> Option<DrawLayout> {
    if width == 0 || height == 0 {
        return None;
    }

    let buttons = button_grid(width, height);
    let rectangles = buttons
        .iter()
        .map(|button| LayoutRect {
            x: logical_coordinate(u32::from(button.bounds.x)),
            y: logical_coordinate(u32::from(button.bounds.y)),
            width: button.bounds.width,
            height: button.bounds.height,
        })
        .collect();
    let mut text = Vec::with_capacity(BUTTON_LABELS.len() + 1);
    text.push(TextPlacement {
        x: scale_i16(TITLE_X, width, WINDOW_WIDTH),
        y: scale_i16(TITLE_BASELINE, height, WINDOW_HEIGHT),
        text: TITLE_TEXT,
    });
    let baseline_offset = u32::from(scale_u16(5, height, WINDOW_HEIGHT).max(1));
    for (button, label) in buttons.iter().zip(BUTTON_LABELS) {
        let center_x = u32::from(button.bounds.x) + u32::from(button.bounds.width) / 2;
        let center_y = u32::from(button.bounds.y) + u32::from(button.bounds.height) / 2;
        text.push(TextPlacement {
            x: logical_coordinate(center_x.saturating_sub(3)),
            y: logical_coordinate(center_y + baseline_offset),
            text: label,
        });
    }

    // Label the exit bar (button 7), which is not in BUTTON_LABELS.
    if let Some(exit) = buttons.iter().find(|button| button.id == 7) {
        let center_x = u32::from(exit.bounds.x) + u32::from(exit.bounds.width) / 2;
        let center_y = u32::from(exit.bounds.y) + u32::from(exit.bounds.height) / 2;
        text.push(TextPlacement {
            x: logical_coordinate(center_x.saturating_sub(18)),
            y: logical_coordinate(center_y + baseline_offset),
            text: b"EXIT",
        });
    }

    Some(DrawLayout { rectangles, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rectangle(rectangle: &LayoutRect, x: i16, y: i16, width: u16, height: u16) {
        assert_eq!(rectangle.x, x);
        assert_eq!(rectangle.y, y);
        assert_eq!(rectangle.width, width);
        assert_eq!(rectangle.height, height);
    }

    fn assert_rectangles_within(rectangles: &[LayoutRect], width: u16, height: u16) {
        for rectangle in rectangles {
            assert!(rectangle.x >= 0);
            assert!(rectangle.y >= 0);
            assert!(rectangle.width > 0);
            assert!(rectangle.height > 0);
            assert!(u32::from(rectangle.x as u16) + u32::from(rectangle.width) <= u32::from(width));
            assert!(
                u32::from(rectangle.y as u16) + u32::from(rectangle.height) <= u32::from(height)
            );
        }
    }

    fn assert_text(placement: &TextPlacement, x: i16, y: i16, text: &[u8]) {
        assert_eq!(placement.x, x);
        assert_eq!(placement.y, y);
        assert_eq!(placement.text, text);
    }

    #[test]
    fn default_layout_draws_the_six_button_grid() {
        let layout = draw_layout(WINDOW_WIDTH, WINDOW_HEIGHT).expect("nonzero geometry");

        assert_eq!(layout.rectangles.len(), 7);
        assert_rectangle(&layout.rectangles[0], 20, 60, 240, 140);
        assert_rectangle(&layout.rectangles[1], 260, 60, 240, 140);
        assert_rectangle(&layout.rectangles[2], 500, 60, 240, 140);
        assert_rectangle(&layout.rectangles[3], 20, 200, 240, 140);
        assert_rectangle(&layout.rectangles[4], 260, 200, 240, 140);
        assert_rectangle(&layout.rectangles[5], 500, 200, 240, 140);
        assert_rectangle(&layout.rectangles[6], 0, 324, 760, 36);
        assert_eq!(layout.text.len(), 8);
        assert_text(&layout.text[0], 20, 40, TITLE_TEXT);
        assert_text(&layout.text[1], 137, 135, b"1");
        assert_text(&layout.text[6], 617, 275, b"6");
        assert_text(&layout.text[7], 362, 347, b"EXIT");
    }

    #[test]
    fn half_size_layout_scales_the_grid_and_labels() {
        let layout = draw_layout(380, 180).expect("nonzero geometry");

        assert_eq!(layout.rectangles.len(), 7);
        assert_rectangle(&layout.rectangles[0], 10, 30, 120, 70);
        assert_rectangle(&layout.rectangles[1], 130, 30, 120, 70);
        assert_rectangle(&layout.rectangles[5], 250, 100, 120, 70);
        assert_rectangle(&layout.rectangles[6], 0, 144, 380, 36);
        assert_text(&layout.text[0], 10, 20, TITLE_TEXT);
        assert_text(&layout.text[1], 67, 67, b"1");
        assert_text(&layout.text[6], 307, 137, b"6");
        assert_text(&layout.text[7], 172, 164, b"EXIT");
    }

    #[test]
    fn zero_tiny_and_maximum_geometry_are_safe() {
        assert!(draw_layout(0, WINDOW_HEIGHT).is_none());
        assert!(draw_layout(WINDOW_WIDTH, 0).is_none());

        let tiny = draw_layout(1, 1).expect("one-pixel geometry");
        assert!(tiny.rectangles.is_empty());
        assert_eq!(tiny.text.len(), 1);
        assert_text(&tiny.text[0], 0, 0, TITLE_TEXT);
        assert!(button_grid(1, 1).is_empty());

        let minimum = button_grid(GRID_COLUMNS, GRID_ROWS);
        assert_eq!(minimum.len(), 6);
        for button in &minimum {
            assert_eq!(button.bounds.width, 1);
            assert_eq!(button.bounds.height, 1);
        }

        let maximum = draw_layout(u16::MAX, u16::MAX).expect("maximum geometry");
        assert_eq!(maximum.rectangles.len(), 7);
        assert_rectangles_within(&maximum.rectangles, u16::MAX, u16::MAX);
        for placement in maximum.text {
            assert!(placement.x >= 0);
            assert!(placement.y >= 0);
        }
    }
}
