//! Deterministic Gray8 software drawing primitives for host-rendered frames.

use image::DynamicImage;
use image::imageops::FilterType;
use paper_protocol::{Gray8Frame, V2_FRAME_PREFIX_LEN, V2_MAX_PAYLOAD_LEN};

use crate::font5x7;

pub const MAX_SOURCE_IMAGE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrayCanvas {
    width: u16,
    height: u16,
    pixels: Vec<u8>,
}

impl GrayCanvas {
    pub fn new(width: u16, height: u16) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err(format!(
                "Gray8 target dimensions must be nonzero, got {width}x{height}"
            ));
        }
        let pixel_bytes = usize::from(width)
            .checked_mul(usize::from(height))
            .ok_or_else(|| "Gray8 target payload size overflow".to_string())?;
        let payload_bytes = V2_FRAME_PREFIX_LEN
            .checked_add(pixel_bytes)
            .ok_or_else(|| "Gray8 frame payload size overflow".to_string())?;
        if payload_bytes > V2_MAX_PAYLOAD_LEN {
            return Err(format!(
                "Gray8 frame payload is {payload_bytes} bytes; maximum is {V2_MAX_PAYLOAD_LEN}"
            ));
        }

        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(pixel_bytes)
            .map_err(|error| format!("failed to allocate Gray8 framebuffer: {error}"))?;
        pixels.resize(pixel_bytes, u8::MAX);
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn dimensions(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn set_pixel(&mut self, x: u16, y: u16, luminance: u8) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.pixels[usize::from(y) * usize::from(self.width) + usize::from(x)] = luminance;
        true
    }

    pub fn fill_rect(&mut self, x: u16, y: u16, width: u16, height: u16, luminance: u8) {
        let right = (u32::from(x) + u32::from(width)).min(u32::from(self.width));
        let bottom = (u32::from(y) + u32::from(height)).min(u32::from(self.height));
        let left = usize::from(x.min(self.width));
        for row in u32::from(y).min(bottom)..bottom {
            let start = row as usize * usize::from(self.width) + left;
            let end = row as usize * usize::from(self.width) + right as usize;
            self.pixels[start..end].fill(luminance);
        }
    }

    pub fn outline_rect(&mut self, x: u16, y: u16, width: u16, height: u16, luminance: u8) {
        if width == 0 || height == 0 {
            return;
        }
        self.fill_rect(x, y, width, 1, luminance);
        if height > 1 {
            self.fill_rect(x, y.saturating_add(height - 1), width, 1, luminance);
        }
        if height > 2 {
            self.fill_rect(x, y.saturating_add(1), 1, height - 2, luminance);
            self.fill_rect(
                x.saturating_add(width - 1),
                y.saturating_add(1),
                1,
                height - 2,
                luminance,
            );
        }
    }

    /// Draw text at a top-left origin. `scale` expands both font axes.
    pub fn draw_text(&mut self, x: u16, y: u16, text: &str, scale: u16, luminance: u8) {
        let scale = scale.max(1);
        let mut glyph_x = u32::from(x);
        for character in text.chars() {
            if glyph_x >= u32::from(self.width) {
                break;
            }
            for (source_row, row_bits) in font5x7::rows(character).into_iter().enumerate() {
                for source_repeat in 0..font5x7::VERTICAL_SCALE {
                    for scale_y in 0..scale {
                        let target_y = u32::from(y)
                            + (source_row as u32 * u32::from(font5x7::VERTICAL_SCALE)
                                + u32::from(source_repeat))
                                * u32::from(scale)
                            + u32::from(scale_y);
                        if target_y >= u32::from(self.height) {
                            continue;
                        }
                        for column in 0..font5x7::WIDTH {
                            let mask = 1_u8 << (font5x7::WIDTH - 1 - column);
                            if row_bits & mask == 0 {
                                continue;
                            }
                            for scale_x in 0..scale {
                                let target_x = glyph_x
                                    + u32::from(column) * u32::from(scale)
                                    + u32::from(scale_x);
                                if target_x < u32::from(self.width) {
                                    self.set_pixel(target_x as u16, target_y as u16, luminance);
                                }
                            }
                        }
                    }
                }
            }
            let advance = u32::from(font5x7::ADVANCE) * u32::from(scale);
            let Some(next_x) = glyph_x.checked_add(advance) else {
                break;
            };
            glyph_x = next_x;
        }
    }

    /// Fit an in-memory image inside a clipped rectangle on a white background.
    pub fn draw_image_fit_contain(
        &mut self,
        source: &DynamicImage,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    ) -> Result<(), String> {
        if source.width() == 0 || source.height() == 0 {
            return Err("source image dimensions must be nonzero".to_string());
        }
        if source.as_bytes().len() > MAX_SOURCE_IMAGE_BYTES {
            return Err(format!(
                "source image requires {} bytes; maximum is {MAX_SOURCE_IMAGE_BYTES}",
                source.as_bytes().len()
            ));
        }
        let clipped_width = width.min(self.width.saturating_sub(x));
        let clipped_height = height.min(self.height.saturating_sub(y));
        if clipped_width == 0 || clipped_height == 0 {
            return Ok(());
        }
        self.fill_rect(x, y, clipped_width, clipped_height, u8::MAX);

        let source = source.to_luma8();
        let (scaled_width, scaled_height) = fit_dimensions(
            source.width(),
            source.height(),
            u32::from(clipped_width),
            u32::from(clipped_height),
        );
        let scaled =
            image::imageops::resize(&source, scaled_width, scaled_height, FilterType::Triangle);
        let offset_x = usize::from(x) + (usize::from(clipped_width) - scaled_width as usize) / 2;
        let offset_y = usize::from(y) + (usize::from(clipped_height) - scaled_height as usize) / 2;
        let canvas_width = usize::from(self.width);
        for row in 0..scaled_height as usize {
            let source_start = row * scaled_width as usize;
            let destination_start = (offset_y + row) * canvas_width + offset_x;
            self.pixels[destination_start..destination_start + scaled_width as usize]
                .copy_from_slice(
                    &scaled.as_raw()[source_start..source_start + scaled_width as usize],
                );
        }
        Ok(())
    }

    pub fn into_frame(self) -> Result<Gray8Frame, String> {
        Gray8Frame::new(self.width, self.height, self.pixels)
            .map_err(|error| format!("failed to build Gray8 frame: {error}"))
    }
}

fn fit_dimensions(
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> (u32, u32) {
    let width_limited = u64::from(source_width) * u64::from(target_height)
        > u64::from(target_width) * u64::from(source_height);
    if width_limited {
        let scaled_height =
            (u64::from(source_height) * u64::from(target_width) / u64::from(source_width)).max(1);
        (target_width, scaled_height as u32)
    } else {
        let scaled_width =
            (u64::from(source_width) * u64::from(target_height) / u64::from(source_height)).max(1);
        (scaled_width as u32, target_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Luma};

    #[test]
    fn fit_contain_preserves_aspect_ratio_on_odd_canvas() {
        let source = DynamicImage::ImageLuma8(ImageBuffer::from_pixel(4, 2, Luma([0])));
        let mut canvas = GrayCanvas::new(7, 9).expect("canvas");
        canvas
            .draw_image_fit_contain(&source, 1, 1, 5, 7)
            .expect("fit image");
        let frame = canvas.into_frame().expect("frame");
        assert_eq!((frame.width(), frame.height(), frame.stride()), (7, 9, 7));
        assert!(frame.pixels()[3 * 7 + 1..5 * 7 + 6].contains(&0));
        assert_eq!(frame.pixels()[8], u8::MAX);
    }

    #[test]
    fn text_and_rectangles_clip_without_panicking() {
        let mut canvas = GrayCanvas::new(9, 8).expect("canvas");
        canvas.outline_rect(7, 6, 8, 8, 0);
        canvas.draw_text(6, 5, "Long title", 3, 0);
        assert!(canvas.pixels().contains(&0));
        canvas.into_frame().expect("valid frame");
    }
}
