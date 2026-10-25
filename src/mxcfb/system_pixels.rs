//! Pure 8-bit pixels for PaperPad's device-owned system UI strip.
//! No framebuffer mapping or panel update happens here.

use anyhow::{Context, Result, ensure};

use crate::ui::screen::{SYSTEM_UI_HEIGHT, ScreenLayout, ScreenRect};
use crate::ui::system::{connection_indicator_bounds, exit_bounds};

const WHITE: u8 = 0xff;
const BLACK: u8 = 0x00;
const BORDER_THICKNESS: usize = 2;
const MAX_GLYPH_SCALE: usize = 5;
const GLYPH_WIDTH: usize = 5;
const GLYPH_HEIGHT: usize = 7;
const TEXT_UNITS: usize = 4 * GLYPH_WIDTH + 3;

// Five-column glyphs, top row first. These are deliberately backend-local;
// X11 continues using its own ImageText8 font.
const EXIT_GLYPHS: [[u8; GLYPH_HEIGHT]; 4] = [
    [
        0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
    ], // E
    [
        0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
    ], // X
    [
        0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
    ], // I
    [
        0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
    ], // T
];

pub(super) struct SystemUiPixels {
    pub region: ScreenRect,
    pub stride: usize,
    pub bytes: Vec<u8>,
}

/// Rasterize only the local strip, sharing Exit and indicator geometry with UI logic.
/// The returned bytes are row-major, one native gray byte per pixel.
pub(super) fn rasterize_system_ui(screen: ScreenLayout, connected: bool) -> Result<SystemUiPixels> {
    let (screen_width, screen_height) = screen.physical_size();
    let bounds = exit_bounds(screen_width, screen_height)
        .context("screen cannot contain PaperPad Exit control")?;
    let region = screen.system_ui_region;
    ensure!(
        region.height == SYSTEM_UI_HEIGHT,
        "incomplete system UI strip"
    );

    let stride = usize::from(region.width);
    let len = stride
        .checked_mul(usize::from(region.height))
        .context("system UI pixel length overflow")?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .context("allocate system UI pixels")?;
    bytes.resize(len, WHITE);

    let left = usize::from(bounds.x - region.x);
    let top = usize::from(bounds.y - region.y);
    let width = usize::from(bounds.width);
    let height = usize::from(bounds.height);
    for y in top..top + height {
        for x in left..left + width {
            if x - left < BORDER_THICKNESS
                || left + width - 1 - x < BORDER_THICKNESS
                || y - top < BORDER_THICKNESS
                || top + height - 1 - y < BORDER_THICKNESS
            {
                bytes[y * stride + x] = BLACK;
            }
        }
    }

    // Very narrow screens still retain a visible, touchable Exit outline.
    // Scale the label down to fit entirely inside that same local rectangle.
    let scale = MAX_GLYPH_SCALE.min(width.saturating_sub(2 * BORDER_THICKNESS) / TEXT_UNITS);
    if scale > 0 {
        let text_width = TEXT_UNITS * scale;
        let text_height = GLYPH_HEIGHT * scale;
        let text_left = left + (width - text_width) / 2;
        let text_top = top + (height - text_height) / 2;
        for (glyph_index, glyph) in EXIT_GLYPHS.iter().enumerate() {
            for (glyph_y, row) in glyph.iter().enumerate() {
                for glyph_x in 0..GLYPH_WIDTH {
                    if row & (1 << (GLYPH_WIDTH - 1 - glyph_x)) == 0 {
                        continue;
                    }
                    let x = text_left + (glyph_index * (GLYPH_WIDTH + 1) + glyph_x) * scale;
                    let y = text_top + glyph_y * scale;
                    for pixel_y in y..y + scale {
                        bytes[pixel_y * stride + x..pixel_y * stride + x + scale].fill(BLACK);
                    }
                }
            }
        }
    }

    if let Some(indicator) = connection_indicator_bounds(screen_width, screen_height) {
        let indicator_left = usize::from(indicator.x - region.x);
        let indicator_top = usize::from(indicator.y - region.y);
        let diameter = usize::from(indicator.width);
        let outer_diameter_squared = (diameter * diameter) as isize;
        let inner_diameter = diameter.saturating_sub(BORDER_THICKNESS * 2);
        let inner_diameter_squared = (inner_diameter * inner_diameter) as isize;

        for y in 0..diameter {
            let dy = (2 * y + 1) as isize - diameter as isize;
            for x in 0..diameter {
                let dx = (2 * x + 1) as isize - diameter as isize;
                let distance_squared = dx * dx + dy * dy;
                let inside_outer = distance_squared <= outer_diameter_squared;
                let inside_inner = distance_squared < inner_diameter_squared;
                if inside_outer && (connected || !inside_inner) {
                    bytes[(indicator_top + y) * stride + indicator_left + x] = BLACK;
                }
            }
        }
    }

    Ok(SystemUiPixels {
        region,
        stride,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(ui: &SystemUiPixels, x: usize, y: usize) -> u8 {
        ui.bytes[y * ui.stride + x]
    }

    #[test]
    fn pw6_system_ui_renders_exit_and_connection_states() {
        let screen = ScreenLayout::new(1272, 1696).unwrap();
        let disconnected = rasterize_system_ui(screen, false).unwrap();
        let connected = rasterize_system_ui(screen, true).unwrap();
        assert_eq!(disconnected.region, screen.system_ui_region);
        assert_eq!(disconnected.bytes.len(), 1272 * 72);
        assert_eq!(pixel(&disconnected, 0, 0), WHITE);
        assert_eq!(pixel(&disconnected, 63, 0), WHITE);
        assert_eq!(pixel(&disconnected, 64, 0), BLACK);
        assert_eq!(pixel(&disconnected, 1251, 71), BLACK);
        assert_eq!(pixel(&disconnected, 1252, 71), WHITE);
        assert_eq!(pixel(&disconnected, 600, 18), BLACK); // first E glyph pixel
        assert_eq!(pixel(&disconnected, 600, 17), WHITE);
        assert_eq!(pixel(&disconnected, 31, 24), BLACK); // circle outline
        assert_eq!(pixel(&disconnected, 31, 35), WHITE); // empty center
        assert_eq!(pixel(&connected, 31, 35), BLACK); // filled center
    }

    #[test]
    fn narrow_screen_keeps_outline_and_clips_label_by_omission() {
        let ui = rasterize_system_ui(ScreenLayout::new(41, 72).unwrap(), false).unwrap();
        assert_eq!(ui.bytes.len(), 41 * 72);
        assert_eq!(pixel(&ui, 19, 36), WHITE);
        assert_eq!(pixel(&ui, 20, 36), BLACK);
        assert_eq!(pixel(&ui, 21, 36), WHITE);
    }

    #[test]
    fn unsupported_local_geometry_fails_explicitly() {
        for (width, height) in [(40, 72), (1272, 71)] {
            let screen = ScreenLayout::new(width, height).unwrap();
            assert!(rasterize_system_ui(screen, false).is_err());
        }
    }
}
