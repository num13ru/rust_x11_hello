//! Deterministic software drawing primitives for protocol-native Mono1 frames.

use paper_protocol::{Mono1Frame, Mono1FrameError, Mono1Pixel, mono1_payload_len, mono1_stride};

/// Mutable one-bit canvas using the protocol's MSB-first row representation.
#[derive(Debug, Eq, PartialEq)]
pub struct MonoCanvas {
    width: u16,
    height: u16,
    stride: usize,
    pixels: Vec<u8>,
}

impl MonoCanvas {
    /// Create an all-white, nonempty canvas.
    pub fn new(width: u16, height: u16) -> Result<Self, Mono1FrameError> {
        if width == 0 || height == 0 {
            return Err(Mono1FrameError::ZeroDimension { width, height });
        }
        let pixel_len = mono1_payload_len(width, height)
            .ok_or(Mono1FrameError::PayloadSizeOverflow { width, height })?;
        Ok(Self {
            width,
            height,
            stride: mono1_stride(width),
            pixels: vec![0; pixel_len],
        })
    }

    /// Canvas width and height in remote-viewport pixels.
    pub fn dimensions(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// Borrow the protocol-native pixel bytes.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Replace every visible pixel with one color.
    pub fn clear(&mut self, color: Mono1Pixel) {
        match color {
            Mono1Pixel::White => self.pixels.fill(0),
            Mono1Pixel::Black => {
                self.pixels.fill(u8::MAX);
                let used_bits = self.width % 8;
                if used_bits != 0 {
                    let visible_mask = u8::MAX << (8 - used_bits);
                    for row in 0..usize::from(self.height) {
                        self.pixels[(row + 1) * self.stride - 1] = visible_mask;
                    }
                }
            }
        }
    }

    /// Set one pixel, returning `false` when the point is outside the canvas.
    pub fn set_pixel(&mut self, x: u16, y: u16, color: Mono1Pixel) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let byte = usize::from(y) * self.stride + usize::from(x / 8);
        let mask = 0x80 >> (x % 8);
        match color {
            Mono1Pixel::White => self.pixels[byte] &= !mask,
            Mono1Pixel::Black => self.pixels[byte] |= mask,
        }
        true
    }

    /// Fill a half-open rectangle, clipping it to the canvas.
    pub fn fill_rect(&mut self, x: u16, y: u16, width: u16, height: u16, color: Mono1Pixel) {
        let right = (u32::from(x) + u32::from(width)).min(u32::from(self.width));
        let bottom = (u32::from(y) + u32::from(height)).min(u32::from(self.height));
        for row in u32::from(y).min(bottom)..bottom {
            for column in u32::from(x).min(right)..right {
                self.set_pixel(column as u16, row as u16, color);
            }
        }
    }

    /// Draw a one-pixel rectangle outline, clipping it to the canvas.
    pub fn outline_rect(&mut self, x: u16, y: u16, width: u16, height: u16, color: Mono1Pixel) {
        if width == 0 || height == 0 {
            return;
        }
        self.fill_rect(x, y, width, 1, color);
        if height > 1 {
            self.fill_rect(x, y.saturating_add(height - 1), width, 1, color);
        }
        if height > 2 {
            self.fill_rect(x, y.saturating_add(1), 1, height - 2, color);
            if width > 1 {
                self.fill_rect(
                    x.saturating_add(width - 1),
                    y.saturating_add(1),
                    1,
                    height - 2,
                    color,
                );
            }
        }
    }

    /// Validate and freeze this canvas as a transport-native frame.
    pub fn into_frame(self) -> Result<Mono1Frame, Mono1FrameError> {
        Mono1Frame::new(self.width, self.height, self.pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_rejects_zero_dimensions() {
        assert_eq!(
            MonoCanvas::new(0, 8),
            Err(Mono1FrameError::ZeroDimension {
                width: 0,
                height: 8,
            })
        );
        assert_eq!(
            MonoCanvas::new(8, 0),
            Err(Mono1FrameError::ZeroDimension {
                width: 8,
                height: 0,
            })
        );
    }

    #[test]
    fn clear_preserves_white_padding_for_odd_widths() {
        let mut canvas = MonoCanvas::new(10, 2).expect("canvas");
        canvas.clear(Mono1Pixel::Black);
        assert_eq!(canvas.pixels(), &[0xff, 0xc0, 0xff, 0xc0]);

        canvas.clear(Mono1Pixel::White);
        assert_eq!(canvas.pixels(), &[0, 0, 0, 0]);
        canvas.into_frame().expect("valid frame");
    }

    #[test]
    fn pixels_and_filled_rectangles_clip_at_every_edge() {
        let mut canvas = MonoCanvas::new(9, 4).expect("canvas");
        assert!(canvas.set_pixel(8, 0, Mono1Pixel::Black));
        assert!(!canvas.set_pixel(9, 0, Mono1Pixel::Black));
        canvas.fill_rect(7, 2, 5, 5, Mono1Pixel::Black);

        let frame = canvas.into_frame().expect("valid frame");
        assert_eq!(frame.pixel(8, 0), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(7, 1), Some(Mono1Pixel::White));
        for y in 2..4 {
            assert_eq!(frame.pixel(6, y), Some(Mono1Pixel::White));
            assert_eq!(frame.pixel(7, y), Some(Mono1Pixel::Black));
            assert_eq!(frame.pixel(8, y), Some(Mono1Pixel::Black));
        }
    }

    #[test]
    fn outlines_have_black_edges_and_white_interiors() {
        let mut canvas = MonoCanvas::new(8, 7).expect("canvas");
        canvas.outline_rect(1, 1, 5, 4, Mono1Pixel::Black);
        canvas.outline_rect(7, 6, 1, 1, Mono1Pixel::Black);

        let frame = canvas.into_frame().expect("valid frame");
        for x in 1..6 {
            assert_eq!(frame.pixel(x, 1), Some(Mono1Pixel::Black));
            assert_eq!(frame.pixel(x, 4), Some(Mono1Pixel::Black));
        }
        for y in 2..4 {
            assert_eq!(frame.pixel(1, y), Some(Mono1Pixel::Black));
            assert_eq!(frame.pixel(5, y), Some(Mono1Pixel::Black));
        }
        assert_eq!(frame.pixel(3, 2), Some(Mono1Pixel::White));
        assert_eq!(frame.pixel(7, 6), Some(Mono1Pixel::Black));
    }
}
