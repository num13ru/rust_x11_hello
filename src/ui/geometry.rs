//! Pure geometry and layout math for the logical button grid.
//!
//! This module has no X11 dependency: it converts a window extent into logical
//! button rectangles, scaled text positions, and text label placements. The X11
//! layer maps these onto its own wire types.

pub const GRID_COLUMNS: u16 = 3;
pub const GRID_ROWS: u16 = 3;
/// Each cell has this much space on each side of its column slot.
pub const CELL_MARGIN: u16 = 20;
pub const GRID_TOP_INSET: u16 = 60;
pub const EXIT_BAR_GAP: u16 = 8;
/// Twice the original 36-pixel exit bar height.
pub const EXIT_BAR_HEIGHT: u16 = 72;
/// Reserved at the bottom for PaperSpoon display commands.
pub const STATUS_BAR_HEIGHT: u16 = 40;
pub const TITLE_X: u16 = CELL_MARGIN;
pub const TITLE_BASELINE: u16 = 40;
pub const TITLE_TEXT: &[u8] = b"Core X11 button grid: tap 1-9";
pub const BUTTON_LABELS: [&[u8]; 9] = [b"1", b"2", b"3", b"4", b"5", b"6", b"7", b"8", b"9"];
pub const LABEL_TEXT_X_OFFSET: u16 = 3;
pub const LABEL_TEXT_Y_OFFSET: u16 = 5;
pub const EXIT_TEXT_X_OFFSET: u16 = 18;
pub const EXIT_BUTTON_ID: u8 = 10;
pub const EXIT_TEXT: &[u8] = b"EXIT";

// Portrait Kindle fixture for input tests; runtime uses the selected X11 screen.
#[cfg(test)]
pub const WINDOW_WIDTH: u16 = 1272;
#[cfg(test)]
pub const WINDOW_HEIGHT: u16 = 1696;
#[cfg(test)]
pub const GRID_RECT_LEFT: u16 = CELL_MARGIN;
#[cfg(test)]
pub const GRID_RECT_TOP: u16 = GRID_TOP_INSET;
#[cfg(test)]
pub const GRID_ROWS_CELL_WIDTH: u16 = WINDOW_WIDTH / GRID_COLUMNS - CELL_MARGIN * 2;
#[cfg(test)]
pub const GRID_ROWS_CELL_HEIGHT: u16 = GRID_ROWS_CELL_WIDTH;
#[cfg(test)]
pub const EXIT_BAR_RECT_TOP: u16 = GRID_TOP_INSET
    + GRID_ROWS * GRID_ROWS_CELL_HEIGHT
    + (GRID_ROWS - 1) * CELL_MARGIN * 2
    + EXIT_BAR_GAP;

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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

/// Build square cells using `width / 3 - 2 * CELL_MARGIN` when height permits.
/// Column gaps absorb integer-division remainders so Exit aligns with both
/// outer grid edges. Short windows reduce the cell side, never stretch it.
/// Extents unable to fit the margins, nine cells, Exit, and status strip have
/// no interactive controls. Layout is capped at X11's signed coordinate limit.
pub fn button_grid(width: u16, height: u16) -> Vec<LogicalButton> {
    let width = width.min(i16::MAX as u16);
    let height = height.min(i16::MAX as u16);
    let row_gap = CELL_MARGIN * 2;
    let chrome_height = GRID_TOP_INSET
        + (GRID_ROWS - 1) * row_gap
        + EXIT_BAR_GAP
        + EXIT_BAR_HEIGHT
        + STATUS_BAR_HEIGHT;
    let side = (width / GRID_COLUMNS)
        .saturating_sub(CELL_MARGIN * 2)
        .min(height.saturating_sub(chrome_height) / GRID_ROWS);
    if side == 0 {
        return Vec::new();
    }

    let inner_width = width - CELL_MARGIN * 2;
    let mut buttons = Vec::with_capacity(usize::from(GRID_COLUMNS * GRID_ROWS) + 1);
    for row in 0..GRID_ROWS {
        for column in 0..GRID_COLUMNS {
            let x = u32::from(CELL_MARGIN)
                + u32::from(inner_width - side) * u32::from(column) / u32::from(GRID_COLUMNS - 1);
            buttons.push(LogicalButton {
                id: (row * GRID_COLUMNS + column + 1) as u8,
                bounds: LogicalRect {
                    x: x as u16,
                    y: GRID_TOP_INSET + row * (side + row_gap),
                    width: side,
                    height: side,
                },
            });
        }
    }
    buttons.push(LogicalButton {
        id: EXIT_BUTTON_ID,
        bounds: LogicalRect {
            x: CELL_MARGIN,
            y: GRID_TOP_INSET + GRID_ROWS * side + (GRID_ROWS - 1) * row_gap + EXIT_BAR_GAP,
            width: inner_width,
            height: EXIT_BAR_HEIGHT,
        },
    });
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
        x: logical_coordinate(u32::from(TITLE_X.min(width - 1))),
        y: logical_coordinate(u32::from(TITLE_BASELINE.min(height - 1))),
        text: TITLE_TEXT,
    });
    let baseline_offset = u32::from(LABEL_TEXT_Y_OFFSET);
    for (button, label) in buttons.iter().zip(BUTTON_LABELS) {
        let center_x = u32::from(button.bounds.x) + u32::from(button.bounds.width) / 2;
        let center_y = u32::from(button.bounds.y) + u32::from(button.bounds.height) / 2;
        text.push(TextPlacement {
            x: logical_coordinate(center_x.saturating_sub(u32::from(LABEL_TEXT_X_OFFSET))),
            y: logical_coordinate(center_y + baseline_offset),
            text: label,
        });
    }

    // Label the exit bar, which is not in BUTTON_LABELS.

    if let Some(exit) = buttons.iter().find(|button| button.id == EXIT_BUTTON_ID) {
        let center_x = u32::from(exit.bounds.x) + u32::from(exit.bounds.width) / 2;
        let center_y = u32::from(exit.bounds.y) + u32::from(exit.bounds.height) / 2;
        text.push(TextPlacement {
            x: logical_coordinate(center_x.saturating_sub(u32::from(EXIT_TEXT_X_OFFSET))),
            y: logical_coordinate(center_y + baseline_offset),
            text: EXIT_TEXT,
        });
    }

    Some(DrawLayout { rectangles, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portrait_screen_has_square_cells_and_aligned_double_height_exit() {
        let buttons = button_grid(1272, 1696);
        assert_eq!(buttons.len(), 10);
        for (index, button) in buttons[..9].iter().enumerate() {
            assert_eq!(button.id, index as u8 + 1);
            assert_eq!(
                button.bounds,
                LogicalRect {
                    x: [20, 444, 868][index % 3],
                    y: [60, 484, 908][index / 3],
                    width: 384,
                    height: 384,
                }
            );
        }
        assert_eq!(buttons[9].id, EXIT_BUTTON_ID);
        assert_eq!(
            buttons[9].bounds,
            LogicalRect {
                x: 20,
                y: 1300,
                width: 1232,
                height: 72,
            }
        );
        let layout = draw_layout(1272, 1696).unwrap();
        assert_eq!(layout.rectangles.len(), 10);
        assert_eq!(layout.text.len(), 11);
        assert_eq!(layout.text[0].text, TITLE_TEXT);
        for (index, label) in layout.text[1..10].iter().enumerate() {
            assert_eq!(label.text, BUTTON_LABELS[index]);
            assert_eq!(label.x, buttons[index].bounds.x as i16 + 192 - 3);
            assert_eq!(label.y, buttons[index].bounds.y as i16 + 192 + 5);
        }
        assert_eq!(
            layout.text[10],
            TextPlacement {
                x: 618,
                y: 1341,
                text: EXIT_TEXT
            }
        );
    }

    #[test]
    fn resized_and_odd_extents_keep_squares_aligned_and_controls_separate() {
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
            assert_eq!(buttons.len(), 10, "{width}x{height}");
            for (index, button) in buttons.iter().enumerate() {
                let rect = button.bounds;
                assert!(rect.width > 0 && rect.height > 0);
                assert!(u32::from(rect.x) + u32::from(rect.width) <= u32::from(width));
                assert!(
                    u32::from(rect.y) + u32::from(rect.height)
                        <= u32::from(height - STATUS_BAR_HEIGHT)
                );
                if index < 9 {
                    assert_eq!(rect.width, rect.height);
                }
                for other in &buttons[index + 1..] {
                    let other = other.bounds;
                    assert!(
                        rect.x + rect.width <= other.x
                            || other.x + other.width <= rect.x
                            || rect.y + rect.height <= other.y
                            || other.y + other.height <= rect.y
                    );
                }
            }
            let exit = buttons[9].bounds;
            assert_eq!(exit.x, buttons[0].bounds.x);
            assert_eq!(
                exit.x + exit.width,
                buttons[2].bounds.x + buttons[2].bounds.width
            );
            assert_eq!(exit.height, 72);
            assert_eq!(
                exit.y,
                buttons[8].bounds.y + buttons[8].bounds.height + EXIT_BAR_GAP
            );
            let layout = draw_layout(width, height).unwrap();
            for (rect, button) in layout.rectangles.iter().zip(&buttons) {
                assert_eq!(rect.x as u16, button.bounds.x);
                assert_eq!(rect.y as u16, button.bounds.y);
                assert_eq!(rect.width, button.bounds.width);
                assert_eq!(rect.height, button.bounds.height);
            }
        }
        // Landscape fallback is constrained by height, not stretched to width.
        assert!(button_grid(1696, 1272)[0].bounds.width < 1696 / 3 - 40);
    }

    #[test]
    fn zero_or_insufficient_space_has_no_interactive_controls() {
        assert!(draw_layout(0, 1696).is_none());
        assert!(draw_layout(1272, 0).is_none());
        for (width, height) in [(1, 1), (3, 3), (122, 1696), (1272, 262)] {
            assert!(button_grid(width, height).is_empty());
            let layout = draw_layout(width, height).unwrap();
            assert!(layout.rectangles.is_empty());
            assert_eq!(layout.text.len(), 1);
            assert!(layout.text[0].x >= 0 && layout.text[0].x < width as i16);
            assert!(layout.text[0].y >= 0 && layout.text[0].y < height as i16);
        }
    }
}
