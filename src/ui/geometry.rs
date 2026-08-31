//! Pure geometry and layout math for the logical button grid.
//!
//! This module has no X11 dependency: it converts a window extent into logical
//! button rectangles, scaled text positions, and text label placements. The X11
//! layer maps these onto its own wire types.

/// Reference window width the fixed Kindle layout was verified at.
pub const WINDOW_WIDTH: u16 = 760;
/// Reference window height the fixed Kindle layout was verified at.
///
/// 400 = 60 (top grid inset) + 2 rows of 128 (grid) + 8 (gap)
///       + 36 (exit bar) + 40 (status strip). The status strip is where
///       PaperSpoon `display <text>` lines render, below the exit bar.
pub const WINDOW_HEIGHT: u16 = 400;
pub const GRID_COLUMNS: u16 = 3;
pub const GRID_ROWS: u16 = 2;
pub const GRID_LEFT_INSET: u16 = 20;
pub const GRID_RIGHT_INSET: u16 = 20;
/// Vertical space above the grid: title baseline plus top margin.
pub const GRID_TOP_INSET: u16 = 60;
/// Vertical gap between the grid's bottom row and the top of the exit bar.
pub const EXIT_BAR_GAP: u16 = 8;
/// Height of the full-width exit bar at the very bottom of the window.
pub const EXIT_BAR_HEIGHT: u16 = 36;
/// Height of the status strip below the exit bar where PaperSpoon
/// `display <text>` commands render.
pub const STATUS_BAR_HEIGHT: u16 = 40;
pub const TITLE_X: u16 = 20;
pub const TITLE_BASELINE: u16 = 40;
pub const TITLE_TEXT: &[u8] = b"Core X11 button grid: tap 1-6";
pub const BUTTON_LABELS: [&[u8]; 6] = [b"1", b"2", b"3", b"4", b"5", b"6"];
/// Grid cell width at the reference window size: `(760 - 20 - 20) / 3`.
pub const GRID_ROWS_CELL_WIDTH: u16 = 240;

/// Grid top edge at the reference window size.
pub const GRID_RECT_TOP: u16 = GRID_TOP_INSET;
/// Grid left edge at the reference window size.
pub const GRID_RECT_LEFT: u16 = GRID_LEFT_INSET;
/// Grid width at the reference window size: `WINDOW_WIDTH - 2 * inset`.
pub const GRID_RECT_WIDTH: u16 = GRID_COLUMNS * GRID_ROWS_CELL_WIDTH;
/// Grid height at the reference window size: `GRID_ROWS * cell height`.
pub const GRID_RECT_HEIGHT: u16 = GRID_ROWS * GRID_ROWS_CELL_HEIGHT;
/// Exit-bar left edge: the bar spans the full window width.
pub const EXIT_BAR_RECT_LEFT: u16 = 0;
/// Exit-bar width: the bar spans the full window width.
pub const EXIT_BAR_RECT_WIDTH: u16 = WINDOW_WIDTH;
/// Exit-bar top edge at the reference window size, below the grid and gap.
pub const EXIT_BAR_RECT_TOP: u16 = GRID_RECT_TOP + GRID_RECT_HEIGHT + EXIT_BAR_GAP;
/// Scale factor applied to the default X11 font's glyph advance when centering
/// a one-character button label (`i16` cast clamps).
pub const LABEL_TEXT_X_OFFSET: u16 = 3;
/// Scale factor naming "five pixels below center" when centering a label.
pub const LABEL_TEXT_Y_OFFSET: u16 = 5;
/// Left inset of the EXIT label relative to the bar's center.
pub const EXIT_TEXT_X_OFFSET: u16 = 18;

/// Logical button id of the full-width exit bar.
pub const EXIT_BUTTON_ID: u8 = 7;
/// Label drawn centered in the exit bar.
pub const EXIT_TEXT: &[u8] = b"EXIT";

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

/// Reference-size vertical budget: top inset + grid + gap + exit bar +
/// status strip must exactly equal [`WINDOW_HEIGHT`]. A change to any
/// constant that breaks the sum is caught by [`layout_fits_reference`]
/// before the build ships.
pub const fn reference_vertical_budget() -> u32 {
    GRID_TOP_INSET as u32
        + EXIT_BAR_GAP as u32
        + EXIT_BAR_HEIGHT as u32
        + GRID_ROWS as u32 * GRID_ROWS_CELL_HEIGHT as u32
        + STATUS_BAR_HEIGHT as u32
}
/// True when the reference geometry consumes the full window height exactly.
///
/// Evaluated at compile time so a constant that breaks the vertical budget
/// fails the build immediately, not just in tests.
pub const fn layout_fits_reference() -> bool {
    reference_vertical_budget() == WINDOW_HEIGHT as u32
}

/// Height of one grid row at the reference window size.
pub const GRID_ROWS_CELL_HEIGHT: u16 = 128;

const _: () = {
    assert!(layout_fits_reference());
    // Reference grid tiles the full width: insets + 3 cells.
    assert!(GRID_RECT_LEFT + GRID_RECT_WIDTH + GRID_RIGHT_INSET == WINDOW_WIDTH);
    // Exit bar spans the full reference width; grid + gap + bar + status
    // strip close the vertical budget exactly.
    assert!(EXIT_BAR_RECT_LEFT + EXIT_BAR_RECT_WIDTH == WINDOW_WIDTH);
    assert!(EXIT_BAR_RECT_TOP + EXIT_BAR_HEIGHT + STATUS_BAR_HEIGHT == WINDOW_HEIGHT);
    // Status strip is non-empty and above the window's bottom edge.
    assert!(STATUS_BAR_HEIGHT > 0);
    assert!(STATUS_BAR_HEIGHT < WINDOW_HEIGHT);
};
/// Build the 2×3 logical button grid for a window extent.
///
/// Layout is a closed vertical stack: top inset, grid, gap, exit bar, status
/// strip. The grid occupies `top..grid_bottom` and the bar `bar_top..bar_end`
/// (where `bar_end = height - status_height`); the status strip is reserved
/// below the bar and never overlaps the interactive buttons.
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
    let top = scale_u16(GRID_TOP_INSET, height, WINDOW_HEIGHT).min(height);
    let status_height = scale_u16(STATUS_BAR_HEIGHT, height, WINDOW_HEIGHT).min(height);
    let bar_height = scale_u16(EXIT_BAR_HEIGHT, height, WINDOW_HEIGHT)
        .min(height.saturating_sub(top).saturating_sub(status_height));
    let gap = scale_u16(EXIT_BAR_GAP, height, WINDOW_HEIGHT).min(
        height
            .saturating_sub(top)
            .saturating_sub(status_height + bar_height),
    );
    let grid_bottom = height.saturating_sub(status_height + bar_height + gap);
    let bar_top = grid_bottom.saturating_add(gap);

    let grid_height = grid_bottom.saturating_sub(top);
    if grid_height < GRID_ROWS {
        return Vec::new();
    }

    let grid_width = width.saturating_sub(left + right);
    if grid_width < GRID_COLUMNS {
        return Vec::new();
    }

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
    if status_height + bar_height <= height && bar_height > 0 && height >= bar_top + bar_height {
        buttons.push(LogicalButton {
            id: EXIT_BUTTON_ID,
            bounds: LogicalRect {
                x: EXIT_BAR_RECT_LEFT,
                y: bar_top,
                width,
                height: bar_height,
            },
        });
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
    let baseline_offset = u32::from(scale_u16(LABEL_TEXT_Y_OFFSET, height, WINDOW_HEIGHT).max(1));
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

    /// Reference-size rect of grid cell `(row, column)`.
    fn reference_cell_rect(row: u16, column: u16) -> LayoutRect {
        LayoutRect {
            x: (GRID_RECT_LEFT + column * GRID_ROWS_CELL_WIDTH) as i16,
            y: (GRID_RECT_TOP + row * GRID_ROWS_CELL_HEIGHT) as i16,
            width: GRID_ROWS_CELL_WIDTH,
            height: GRID_ROWS_CELL_HEIGHT,
        }
    }

    /// Reference-size rect of the exit bar.
    fn reference_exit_rect() -> LayoutRect {
        LayoutRect {
            x: EXIT_BAR_RECT_LEFT as i16,
            y: EXIT_BAR_RECT_TOP as i16,
            width: EXIT_BAR_RECT_WIDTH,
            height: EXIT_BAR_HEIGHT,
        }
    }

    /// Label placement centered on a rect with the named text offsets.
    fn label_placement(
        rect: LayoutRect,
        text_x_offset: u16,
        baseline_offset: u16,
        text: &'static [u8],
    ) -> TextPlacement {
        TextPlacement {
            x: rect.x + rect.width as i16 / 2 - text_x_offset as i16,
            y: rect.y + rect.height as i16 / 2 + baseline_offset as i16,
            text,
        }
    }

    #[test]
    fn default_layout_draws_the_six_button_grid() {
        let layout = draw_layout(WINDOW_WIDTH, WINDOW_HEIGHT).expect("nonzero geometry");

        let expected_rectangles: Vec<LayoutRect> = (0..GRID_ROWS)
            .flat_map(|row| (0..GRID_COLUMNS).map(move |column| reference_cell_rect(row, column)))
            .chain(std::iter::once(reference_exit_rect()))
            .collect();
        assert_eq!(layout.rectangles, expected_rectangles);

        let expected_text: Vec<TextPlacement> = std::iter::once(TextPlacement {
            x: TITLE_X as i16,
            y: TITLE_BASELINE as i16,
            text: TITLE_TEXT,
        })
        .chain(
            expected_rectangles[..6]
                .iter()
                .zip(BUTTON_LABELS)
                .map(|(rect, label)| {
                    label_placement(*rect, LABEL_TEXT_X_OFFSET, LABEL_TEXT_Y_OFFSET, label)
                }),
        )
        .chain(std::iter::once(label_placement(
            expected_rectangles[6],
            EXIT_TEXT_X_OFFSET,
            LABEL_TEXT_Y_OFFSET,
            EXIT_TEXT,
        )))
        .collect();
        assert_eq!(layout.text, expected_text);
    }

    #[test]
    fn half_size_layout_scales_every_rect_and_label_by_two() {
        let layout = draw_layout(WINDOW_WIDTH / 2, WINDOW_HEIGHT / 2).expect("nonzero geometry");
        let reference = draw_layout(WINDOW_WIDTH, WINDOW_HEIGHT).expect("nonzero geometry");

        assert_eq!(layout.rectangles.len(), reference.rectangles.len());
        for (actual, expected) in layout.rectangles.iter().zip(&reference.rectangles) {
            assert_eq!(actual.x, expected.x / 2);
            assert_eq!(actual.y, expected.y / 2);
            assert_eq!(actual.width, expected.width / 2);
            assert_eq!(actual.height, expected.height / 2);
        }

        let half_title_baseline = scale_i16(TITLE_BASELINE, WINDOW_HEIGHT / 2, WINDOW_HEIGHT);
        assert_text(
            &layout.text[0],
            scale_i16(TITLE_X, WINDOW_WIDTH / 2, WINDOW_WIDTH),
            half_title_baseline,
            TITLE_TEXT,
        );
        let half_baseline_offset =
            scale_u16(LABEL_TEXT_Y_OFFSET, WINDOW_HEIGHT / 2, WINDOW_HEIGHT).max(1);
        // Six grid buttons: label centered on each half-size cell.
        for (placement, rect) in layout.text[1..7].iter().zip(&layout.rectangles[..6]) {
            assert_eq!(
                placement.x,
                rect.x + rect.width as i16 / 2 - LABEL_TEXT_X_OFFSET as i16
            );
            assert_eq!(
                placement.y,
                rect.y + rect.height as i16 / 2 + half_baseline_offset as i16
            );
        }
        // The exit label is centered on the bar with its own x offset.
        let exit_rect = layout.rectangles[6];
        assert_eq!(
            layout.text[7].x,
            exit_rect.x + exit_rect.width as i16 / 2 - EXIT_TEXT_X_OFFSET as i16
        );
        assert_eq!(
            layout.text[7].y,
            exit_rect.y + exit_rect.height as i16 / 2 + half_baseline_offset as i16
        );
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
        assert_eq!(minimum.len(), usize::from(GRID_COLUMNS * GRID_ROWS));
        for button in &minimum {
            assert_eq!(button.bounds.width, 1);
            assert_eq!(button.bounds.height, 1);
        }

        let maximum = draw_layout(u16::MAX, u16::MAX).expect("maximum geometry");
        assert_eq!(
            maximum.rectangles.len(),
            usize::from(GRID_COLUMNS * GRID_ROWS) + 1
        );
        assert_rectangles_within(&maximum.rectangles, u16::MAX, u16::MAX);
        for placement in maximum.text {
            assert!(placement.x >= 0);
            assert!(placement.y >= 0);
        }
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    #[test]
    fn reference_layout_consumes_full_height() {
        assert!(
            layout_fits_reference(),
            "vertical budget {} != WINDOW_HEIGHT {}",
            reference_vertical_budget(),
            WINDOW_HEIGHT
        );
    }

    #[test]
    fn grid_and_exit_bar_never_overlap_at_reference_size() {
        let buttons = button_grid(WINDOW_WIDTH, WINDOW_HEIGHT);
        let grid_bottom = buttons
            .iter()
            .filter(|b| b.id != EXIT_BUTTON_ID)
            .map(|b| u32::from(b.bounds.y) + u32::from(b.bounds.height))
            .max()
            .expect("grid has buttons");
        let exit = buttons
            .iter()
            .find(|b| b.id == EXIT_BUTTON_ID)
            .expect("exit bar present");
        assert!(
            u32::from(exit.bounds.y) >= grid_bottom,
            "exit bar y {} overlaps grid bottom {}",
            exit.bounds.y,
            grid_bottom
        );
        // The bar ends above the status strip, which occupies the bottom
        // STATUS_BAR_HEIGHT reference pixels.
        assert_eq!(
            u32::from(exit.bounds.y) + u32::from(exit.bounds.height),
            u32::from(WINDOW_HEIGHT - STATUS_BAR_HEIGHT)
        );
    }
}
