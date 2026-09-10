//! Transport-independent monochrome framebuffer representation.

use std::fmt;

/// A decoded one-bit pixel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mono1Pixel {
    White,
    Black,
}

/// Validation failure for a [`Mono1Frame`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Mono1FrameError {
    ZeroDimension { width: u16, height: u16 },
    PayloadSizeOverflow { width: u16, height: u16 },
    PayloadLength { expected: usize, actual: usize },
    NonWhitePadding { row: u16, byte: u8 },
}

impl fmt::Display for Mono1FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDimension { width, height } => {
                write!(
                    formatter,
                    "Mono1 frame dimensions must be nonzero, got {width}x{height}"
                )
            }
            Self::PayloadSizeOverflow { width, height } => {
                write!(
                    formatter,
                    "Mono1 payload size overflows usize for {width}x{height}"
                )
            }
            Self::PayloadLength { expected, actual } => write!(
                formatter,
                "Mono1 payload length mismatch: expected {expected} bytes, got {actual}"
            ),
            Self::NonWhitePadding { row, byte } => write!(
                formatter,
                "Mono1 row {row} has non-white trailing bits in byte 0x{byte:02x}"
            ),
        }
    }
}

impl std::error::Error for Mono1FrameError {}

/// Row-major monochrome pixels for a remote content viewport.
///
/// Each row occupies [`mono1_stride`] bytes. Pixels are MSB-first within each
/// byte (`0 = white`, `1 = black`). For widths that are not divisible by eight,
/// unused low bits in the row's final byte must be zero. No bytes are inserted
/// between rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mono1Frame {
    width: u16,
    height: u16,
    stride: usize,
    pixels: Vec<u8>,
}

impl Mono1Frame {
    /// Validate and construct a non-empty Mono1 frame.
    pub fn new(width: u16, height: u16, pixels: Vec<u8>) -> Result<Self, Mono1FrameError> {
        if width == 0 || height == 0 {
            return Err(Mono1FrameError::ZeroDimension { width, height });
        }

        let stride = mono1_stride(width);
        let expected = mono1_payload_len(width, height)
            .ok_or(Mono1FrameError::PayloadSizeOverflow { width, height })?;
        if pixels.len() != expected {
            return Err(Mono1FrameError::PayloadLength {
                expected,
                actual: pixels.len(),
            });
        }

        match width % 8 {
            0 => {}
            used_bits => {
                let padding_mask = !(u8::MAX << (8 - used_bits));
                for row in 0..height {
                    let final_byte = pixels[(usize::from(row) + 1) * stride - 1];
                    if final_byte & padding_mask != 0 {
                        return Err(Mono1FrameError::NonWhitePadding {
                            row,
                            byte: final_byte,
                        });
                    }
                }
            }
        }

        Ok(Self {
            width,
            height,
            stride,
            pixels,
        })
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// Return one in-bounds pixel, or `None` for coordinates outside the frame.
    pub fn pixel(&self, x: u16, y: u16) -> Option<Mono1Pixel> {
        if x >= self.width || y >= self.height {
            return None;
        }

        let byte = self.pixels[usize::from(y) * self.stride + usize::from(x / 8)];
        let mask = 0x80 >> (x % 8);
        Some(if byte & mask == 0 {
            Mono1Pixel::White
        } else {
            Mono1Pixel::Black
        })
    }
}

/// Number of bytes occupied by one Mono1 row.
pub fn mono1_stride(width: u16) -> usize {
    usize::from(width).div_ceil(8)
}

/// Exact payload length for a Mono1 frame, if representable by this platform.
pub fn mono1_payload_len(width: u16, height: u16) -> Option<usize> {
    mono1_stride(width).checked_mul(usize::from(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stride_rounds_odd_widths_up_to_the_next_byte() {
        for (width, expected) in [
            (0, 0),
            (1, 1),
            (7, 1),
            (8, 1),
            (9, 2),
            (15, 2),
            (16, 2),
            (u16::MAX, 8192),
        ] {
            assert_eq!(mono1_stride(width), expected, "width={width}");
        }
        assert_eq!(mono1_payload_len(9, 3), Some(6));
        assert_eq!(mono1_payload_len(u16::MAX, u16::MAX), Some(536_862_720));
    }

    #[test]
    fn exact_payload_length_is_required() {
        assert_eq!(
            Mono1Frame::new(9, 2, vec![0; 3]),
            Err(Mono1FrameError::PayloadLength {
                expected: 4,
                actual: 3,
            })
        );
        assert_eq!(
            Mono1Frame::new(9, 2, vec![0; 5]),
            Err(Mono1FrameError::PayloadLength {
                expected: 4,
                actual: 5,
            })
        );
        assert!(Mono1Frame::new(9, 2, vec![0; 4]).is_ok());
    }

    #[test]
    fn zero_dimensions_do_not_form_remote_frames() {
        for (width, height) in [(0, 1), (1, 0), (0, 0)] {
            assert_eq!(
                Mono1Frame::new(width, height, Vec::new()),
                Err(Mono1FrameError::ZeroDimension { width, height })
            );
        }
    }

    #[test]
    fn pixels_are_msb_first_across_bytes_and_rows() {
        let frame = Mono1Frame::new(
            9,
            2,
            vec![0b1000_0001, 0b1000_0000, 0b0100_0000, 0b0000_0000],
        )
        .expect("valid frame");

        assert_eq!(frame.stride(), 2);
        assert_eq!(frame.pixel(0, 0), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(1, 0), Some(Mono1Pixel::White));
        assert_eq!(frame.pixel(7, 0), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(8, 0), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(0, 1), Some(Mono1Pixel::White));
        assert_eq!(frame.pixel(1, 1), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(8, 1), Some(Mono1Pixel::White));
        assert_eq!(frame.pixel(9, 0), None);
        assert_eq!(frame.pixel(0, 2), None);
    }

    #[test]
    fn all_white_and_all_black_frames_preserve_exact_bytes() {
        let white = Mono1Frame::new(10, 2, vec![0x00, 0x00, 0x00, 0x00]).expect("all-white frame");
        assert_eq!(white.width(), 10);
        assert_eq!(white.height(), 2);
        assert!(
            (0..white.height())
                .all(|y| (0..white.width()).all(|x| white.pixel(x, y) == Some(Mono1Pixel::White)))
        );

        let black = Mono1Frame::new(10, 2, vec![0xff, 0xc0, 0xff, 0xc0]).expect("all-black frame");
        assert!(
            (0..black.height())
                .all(|y| (0..black.width()).all(|x| black.pixel(x, y) == Some(Mono1Pixel::Black)))
        );
        assert_eq!(black.pixels(), &[0xff, 0xc0, 0xff, 0xc0]);
        assert_eq!(black.clone().into_pixels(), black.pixels());
    }

    #[test]
    fn checkerboard_pattern_crosses_byte_boundary() {
        let frame =
            Mono1Frame::new(10, 2, vec![0xaa, 0x80, 0x55, 0x40]).expect("checkerboard frame");

        for y in 0..frame.height() {
            for x in 0..frame.width() {
                let expected = if (x + y) % 2 == 0 {
                    Mono1Pixel::Black
                } else {
                    Mono1Pixel::White
                };
                assert_eq!(frame.pixel(x, y), Some(expected), "({x}, {y})");
            }
        }
    }

    #[test]
    fn nonwhite_unused_row_bits_are_rejected() {
        assert_eq!(
            Mono1Frame::new(9, 2, vec![0x00, 0x80, 0x00, 0x01]),
            Err(Mono1FrameError::NonWhitePadding { row: 1, byte: 1 })
        );
    }
}
