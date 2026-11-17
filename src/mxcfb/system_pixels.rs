//! Pure 8-bit pixels for PaperPad's device-owned system UI strip.
//! No framebuffer mapping or panel update happens here.

use anyhow::{Context, Result, ensure};

use super::exit_label_bitmap::{
    EXIT_LABEL_BITS, EXIT_LABEL_HEIGHT, EXIT_LABEL_STRIDE, EXIT_LABEL_WIDTH,
};
use crate::ui::screen::{SYSTEM_UI_HEIGHT, ScreenLayout, ScreenRect};
use crate::ui::system::{connection_indicator_bounds, exit_bounds};

const WHITE: u8 = 0xff;
const BLACK: u8 = 0x00;
const BORDER_THICKNESS: usize = 2;

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

    // The font is baked at its physical size. Narrow layouts keep the local
    // outline and hit target but omit the label instead of distorting it.
    let label_fits = EXIT_LABEL_WIDTH <= width.saturating_sub(2 * BORDER_THICKNESS)
        && EXIT_LABEL_HEIGHT <= height.saturating_sub(2 * BORDER_THICKNESS);
    if label_fits {
        let text_left = left + (width - EXIT_LABEL_WIDTH) / 2;
        let text_top = top + (height - EXIT_LABEL_HEIGHT) / 2;
        for glyph_y in 0..EXIT_LABEL_HEIGHT {
            for glyph_x in 0..EXIT_LABEL_WIDTH {
                let byte = EXIT_LABEL_BITS[glyph_y * EXIT_LABEL_STRIDE + glyph_x / 8];
                if byte & (0x80 >> (glyph_x % 8)) != 0 {
                    bytes[(text_top + glyph_y) * stride + text_left + glyph_x] = BLACK;
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
        let exit = exit_bounds(1272, 1696).expect("Exit bounds");
        let label_left = usize::from(exit.x) + (usize::from(exit.width) - EXIT_LABEL_WIDTH) / 2;
        let label_top = (usize::from(exit.height) - EXIT_LABEL_HEIGHT) / 2;
        for glyph_y in 0..EXIT_LABEL_HEIGHT {
            for glyph_x in 0..EXIT_LABEL_WIDTH {
                let byte = EXIT_LABEL_BITS[glyph_y * EXIT_LABEL_STRIDE + glyph_x / 8];
                let expected = if byte & (0x80 >> (glyph_x % 8)) != 0 {
                    BLACK
                } else {
                    WHITE
                };
                assert_eq!(
                    pixel(&disconnected, label_left + glyph_x, label_top + glyph_y),
                    expected
                );
            }
        }
        assert_eq!(pixel(&disconnected, label_left - 1, label_top), WHITE);
        assert_eq!(pixel(&disconnected, label_left, label_top - 1), WHITE);
        assert_eq!(pixel(&disconnected, 31, 24), BLACK); // circle outline
        assert_eq!(pixel(&disconnected, 31, 35), WHITE); // empty center
        assert_eq!(pixel(&connected, 31, 35), BLACK); // filled center
    }

    #[test]
    fn baked_exit_label_has_valid_msb_first_rows_and_padding() {
        assert_eq!(EXIT_LABEL_WIDTH, 110);
        assert_eq!(EXIT_LABEL_HEIGHT, 34);
        assert_eq!(EXIT_LABEL_STRIDE, EXIT_LABEL_WIDTH.div_ceil(8));
        assert_eq!(EXIT_LABEL_BITS.len(), EXIT_LABEL_STRIDE * EXIT_LABEL_HEIGHT);
        let padding_bits = 8 - EXIT_LABEL_WIDTH % 8;
        assert_ne!(padding_bits, 8);
        let padding_mask = (1_u8 << padding_bits) - 1;
        assert!(
            EXIT_LABEL_BITS
                .chunks_exact(EXIT_LABEL_STRIDE)
                .all(|row| row[EXIT_LABEL_STRIDE - 1] & padding_mask == 0)
        );
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
