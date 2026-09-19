//! Network- and renderer-independent model of the host-owned application UI.

const GRID_COLUMNS: u16 = 3;
const GRID_ROWS: u16 = 3;
const CELL_MARGIN: u16 = 20;
const GRID_TOP_INSET: u16 = 60;
const STATUS_BAR_HEIGHT: u16 = 40;
const STATUS_TEXT_BOTTOM_MARGIN: u16 = 10;
const TITLE_X: u16 = CELL_MARGIN;
const TITLE_BASELINE: u16 = 40;
const LABEL_TEXT_X_OFFSET: u16 = 3;
const LABEL_TEXT_Y_OFFSET: u16 = 5;

/// Existing PaperPad application title retained for visual equivalence.
pub const TITLE_TEXT: &str = "Core X11 button grid: tap 1-9";

const BUTTON_LABELS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// Half-open rectangle in remote-viewport coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub(crate) fn contains(self, x: u16, y: u16) -> bool {
        let x = u32::from(x);
        let y = u32::from(y);
        x >= u32::from(self.x)
            && y >= u32::from(self.y)
            && x < u32::from(self.x) + u32::from(self.width)
            && y < u32::from(self.y) + u32::from(self.height)
    }
}

/// Text and its baseline origin in remote-viewport coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextRun<'a> {
    pub x: u16,
    pub baseline_y: u16,
    pub text: &'a str,
}

/// One host-owned application button. Exit is intentionally not represented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ButtonLayout {
    pub id: u8,
    pub bounds: Rect,
    pub label: TextRun<'static>,
}

/// Complete host application layout for one remote viewport.
#[derive(Debug, Eq, PartialEq)]
pub struct ApplicationLayout<'a> {
    pub width: u16,
    pub height: u16,
    pub title: TextRun<'static>,
    pub buttons: Vec<ButtonLayout>,
    pub status_region: Rect,
    pub status: Option<TextRun<'a>>,
}

/// Host-owned application state, independent from transport and rendering.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct ApplicationUi {
    status: Option<String>,
}

impl ApplicationUi {
    /// Replace the remote status text shown in subsequent layouts.
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = Some(status.into());
    }

    /// Remove remote status text from subsequent layouts.
    pub fn clear_status(&mut self) {
        self.status = None;
    }

    /// Compute a deterministic layout for a nonzero remote viewport.
    pub fn layout(&self, width: u16, height: u16) -> Option<ApplicationLayout<'_>> {
        if width == 0 || height == 0 {
            return None;
        }

        let status_height = STATUS_BAR_HEIGHT.min(height);
        let status_region = Rect {
            x: 0,
            y: height - status_height,
            width,
            height: status_height,
        };
        let status = self.status.as_deref().map(|text| TextRun {
            x: CELL_MARGIN.min(width - 1),
            baseline_y: height
                .saturating_sub(STATUS_TEXT_BOTTOM_MARGIN)
                .min(height - 1),
            text,
        });

        Some(ApplicationLayout {
            width,
            height,
            title: TextRun {
                x: TITLE_X.min(width - 1),
                baseline_y: TITLE_BASELINE.min(height - 1),
                text: TITLE_TEXT,
            },
            buttons: button_grid(width, height),
            status_region,
            status,
        })
    }
}

fn button_grid(width: u16, height: u16) -> Vec<ButtonLayout> {
    let Some(side) = grid_cell_side(width, height) else {
        return Vec::new();
    };
    let row_gap = CELL_MARGIN * 2;
    let inner_width = width - CELL_MARGIN * 2;
    let mut buttons = Vec::with_capacity(usize::from(GRID_COLUMNS * GRID_ROWS));

    for row in 0..GRID_ROWS {
        for column in 0..GRID_COLUMNS {
            let x = u32::from(CELL_MARGIN)
                + u32::from(inner_width - side) * u32::from(column) / u32::from(GRID_COLUMNS - 1);
            let y = GRID_TOP_INSET + row * (side + row_gap);
            let center_x = x + u32::from(side) / 2;
            let center_y = u32::from(y) + u32::from(side) / 2;
            let index = usize::from(row * GRID_COLUMNS + column);
            buttons.push(ButtonLayout {
                id: index as u8 + 1,
                bounds: Rect {
                    x: x as u16,
                    y,
                    width: side,
                    height: side,
                },
                label: TextRun {
                    x: center_x.saturating_sub(u32::from(LABEL_TEXT_X_OFFSET)) as u16,
                    baseline_y: (center_y + u32::from(LABEL_TEXT_Y_OFFSET)) as u16,
                    text: BUTTON_LABELS[index],
                },
            });
        }
    }

    buttons
}

fn grid_cell_side(width: u16, height: u16) -> Option<u16> {
    if width == 0 || height == 0 {
        return None;
    }
    let row_gap = CELL_MARGIN * 2;
    let fixed_vertical_overhead = GRID_TOP_INSET + (GRID_ROWS - 1) * row_gap + STATUS_BAR_HEIGHT;
    let side = (width / GRID_COLUMNS)
        .saturating_sub(CELL_MARGIN * 2)
        .min(height.saturating_sub(fixed_vertical_overhead) / GRID_ROWS);
    (side > 0).then_some(side)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORTRAIT_WIDTH: u16 = 1272;
    const PORTRAIT_HEIGHT: u16 = 1624;

    #[test]
    fn portrait_layout_matches_existing_application_geometry_without_exit() {
        let ui = ApplicationUi::default();
        let layout = ui
            .layout(PORTRAIT_WIDTH, PORTRAIT_HEIGHT)
            .expect("portrait layout");

        assert_eq!(
            layout.title,
            TextRun {
                x: 20,
                baseline_y: 40,
                text: TITLE_TEXT,
            }
        );
        assert_eq!(layout.buttons.len(), 9);
        for (index, button) in layout.buttons.iter().enumerate() {
            assert_eq!(button.id, index as u8 + 1);
            assert_eq!(
                button.bounds,
                Rect {
                    x: [20, 444, 868][index % 3],
                    y: [60, 484, 908][index / 3],
                    width: 384,
                    height: 384,
                }
            );
            assert_eq!(button.label.text, BUTTON_LABELS[index]);
            assert_eq!(button.label.x, button.bounds.x + 192 - 3);
            assert_eq!(button.label.baseline_y, button.bounds.y + 192 + 5);
        }
        assert_eq!(
            layout.status_region,
            Rect {
                x: 0,
                y: 1584,
                width: PORTRAIT_WIDTH,
                height: 40,
            }
        );
        assert_eq!(layout.status, None);
    }

    #[test]
    fn status_state_is_replaceable_clearable_and_borrowed_by_layout() {
        let mut ui = ApplicationUi::default();
        ui.set_status("first");
        ui.set_status("PaperSpoon: ready");

        let layout = ui
            .layout(PORTRAIT_WIDTH, PORTRAIT_HEIGHT)
            .expect("portrait layout");
        assert_eq!(
            layout.status,
            Some(TextRun {
                x: 20,
                baseline_y: 1614,
                text: "PaperSpoon: ready",
            })
        );

        ui.clear_status();
        assert_eq!(
            ui.layout(PORTRAIT_WIDTH, PORTRAIT_HEIGHT)
                .expect("portrait layout")
                .status,
            None
        );
    }

    #[test]
    fn resized_and_small_layouts_stay_inside_remote_viewport() {
        let ui = ApplicationUi::default();
        assert!(ui.layout(0, PORTRAIT_HEIGHT).is_none());
        assert!(ui.layout(PORTRAIT_WIDTH, 0).is_none());

        for (width, height) in [(636, 776), (1273, 1624), (1696, 1200), (123, 263), (1, 1)] {
            let layout = ui.layout(width, height).expect("nonzero layout");
            for button in &layout.buttons {
                assert!(
                    u32::from(button.bounds.x) + u32::from(button.bounds.width) <= width.into()
                );
                assert!(
                    u32::from(button.bounds.y) + u32::from(button.bounds.height)
                        <= u32::from(layout.status_region.y)
                );
            }
            assert!(layout.title.x < width);
            assert!(layout.title.baseline_y < height);
        }
    }
}
