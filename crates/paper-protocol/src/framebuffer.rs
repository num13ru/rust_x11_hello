//! Transport-independent framebuffer representations.

use std::fmt;

/// Pixel encoding used by a remote-content framebuffer.
///
/// `Mono1` pixels are packed MSB-first with `0 = white` and `1 = black`.
/// `Gray8` pixels are unpacked luminance bytes with `0 = black` and
/// `255 = white`. The opposite endpoint polarity is intentional and is not
/// silently converted by this crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    Mono1,
    Gray8,
}

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

/// Validation failure for a [`Gray8Frame`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Gray8FrameError {
    ZeroDimension { width: u16, height: u16 },
    PayloadSizeOverflow { width: u16, height: u16 },
    PayloadLength { expected: usize, actual: usize },
}

impl fmt::Display for Gray8FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDimension { width, height } => {
                write!(
                    formatter,
                    "Gray8 frame dimensions must be nonzero, got {width}x{height}"
                )
            }
            Self::PayloadSizeOverflow { width, height } => {
                write!(
                    formatter,
                    "Gray8 payload size overflows usize for {width}x{height}"
                )
            }
            Self::PayloadLength { expected, actual } => write!(
                formatter,
                "Gray8 payload length mismatch: expected {expected} bytes, got {actual}"
            ),
        }
    }
}

impl std::error::Error for Gray8FrameError {}

/// Validation failure for a format-aware [`Frame`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameError {
    Mono1(Mono1FrameError),
    Gray8(Gray8FrameError),
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mono1(error) => error.fmt(formatter),
            Self::Gray8(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mono1(error) => Some(error),
            Self::Gray8(error) => Some(error),
        }
    }
}

impl From<Mono1FrameError> for FrameError {
    fn from(error: Mono1FrameError) -> Self {
        Self::Mono1(error)
    }
}

impl From<Gray8FrameError> for FrameError {
    fn from(error: Gray8FrameError) -> Self {
        Self::Gray8(error)
    }
}

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
        let stride = validate_mono1_pixels(width, height, &pixels)?;

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

/// Row-major eight-bit luminance pixels for a remote content viewport.
///
/// Each row occupies [`gray8_stride`] bytes with no row padding. Each byte is
/// one pixel (`0 = black`, `255 = white`), and intermediate values are
/// luminance levels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Gray8Frame {
    width: u16,
    height: u16,
    stride: usize,
    pixels: Vec<u8>,
}

impl Gray8Frame {
    /// Validate and construct a non-empty Gray8 frame.
    pub fn new(width: u16, height: u16, pixels: Vec<u8>) -> Result<Self, Gray8FrameError> {
        let stride = validate_gray8_pixels(width, height, &pixels)?;

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

    /// Return one in-bounds luminance value, or `None` outside the frame.
    pub fn pixel(&self, x: u16, y: u16) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }

        Some(self.pixels[usize::from(y) * self.stride + usize::from(x)])
    }
}

/// Validated owned remote framebuffer in one of the supported formats.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Frame {
    Mono1(Mono1Frame),
    Gray8(Gray8Frame),
}

impl Frame {
    /// Validate pixels according to `pixel_format` and construct a frame.
    pub fn new(
        width: u16,
        height: u16,
        pixel_format: PixelFormat,
        pixels: Vec<u8>,
    ) -> Result<Self, FrameError> {
        match pixel_format {
            PixelFormat::Mono1 => Mono1Frame::new(width, height, pixels)
                .map(Self::Mono1)
                .map_err(FrameError::Mono1),
            PixelFormat::Gray8 => Gray8Frame::new(width, height, pixels)
                .map(Self::Gray8)
                .map_err(FrameError::Gray8),
        }
    }

    pub fn width(&self) -> u16 {
        match self {
            Self::Mono1(frame) => frame.width(),
            Self::Gray8(frame) => frame.width(),
        }
    }

    pub fn height(&self) -> u16 {
        match self {
            Self::Mono1(frame) => frame.height(),
            Self::Gray8(frame) => frame.height(),
        }
    }

    pub fn pixel_format(&self) -> PixelFormat {
        match self {
            Self::Mono1(_) => PixelFormat::Mono1,
            Self::Gray8(_) => PixelFormat::Gray8,
        }
    }

    pub fn stride(&self) -> usize {
        match self {
            Self::Mono1(frame) => frame.stride(),
            Self::Gray8(frame) => frame.stride(),
        }
    }

    pub fn pixels(&self) -> &[u8] {
        match self {
            Self::Mono1(frame) => frame.pixels(),
            Self::Gray8(frame) => frame.pixels(),
        }
    }

    pub fn into_pixels(self) -> Vec<u8> {
        match self {
            Self::Mono1(frame) => frame.into_pixels(),
            Self::Gray8(frame) => frame.into_pixels(),
        }
    }
}

impl From<Mono1Frame> for Frame {
    fn from(frame: Mono1Frame) -> Self {
        Self::Mono1(frame)
    }
}

impl From<Gray8Frame> for Frame {
    fn from(frame: Gray8Frame) -> Self {
        Self::Gray8(frame)
    }
}

/// Validate borrowed Mono1 pixels and return their row stride.
pub fn validate_mono1_pixels(
    width: u16,
    height: u16,
    pixels: &[u8],
) -> Result<usize, Mono1FrameError> {
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

    Ok(stride)
}

/// Validate borrowed Gray8 pixels and return their row stride.
pub fn validate_gray8_pixels(
    width: u16,
    height: u16,
    pixels: &[u8],
) -> Result<usize, Gray8FrameError> {
    if width == 0 || height == 0 {
        return Err(Gray8FrameError::ZeroDimension { width, height });
    }

    let stride = gray8_stride(width);
    let expected = gray8_payload_len(width, height)
        .ok_or(Gray8FrameError::PayloadSizeOverflow { width, height })?;
    if pixels.len() != expected {
        return Err(Gray8FrameError::PayloadLength {
            expected,
            actual: pixels.len(),
        });
    }

    Ok(stride)
}

/// Number of bytes occupied by one Mono1 row.
pub fn mono1_stride(width: u16) -> usize {
    usize::from(width).div_ceil(8)
}

/// Exact payload length for a Mono1 frame, if representable by this platform.
pub fn mono1_payload_len(width: u16, height: u16) -> Option<usize> {
    mono1_stride(width).checked_mul(usize::from(height))
}

/// Number of bytes occupied by one Gray8 row.
pub fn gray8_stride(width: u16) -> usize {
    usize::from(width)
}

/// Exact payload length of a Gray8 frame, if representable by the platform.
pub fn gray8_payload_len(width: u16, height: u16) -> Option<usize> {
    gray8_stride(width).checked_mul(usize::from(height))
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

    #[test]
    fn gray8_requires_exact_unpadded_rows() {
        assert_eq!(gray8_stride(3), 3);
        assert_eq!(gray8_payload_len(3, 2), Some(6));
        assert_eq!(
            Gray8Frame::new(3, 2, vec![0; 5]),
            Err(Gray8FrameError::PayloadLength {
                expected: 6,
                actual: 5,
            })
        );
        assert_eq!(
            Gray8Frame::new(3, 2, vec![0; 7]),
            Err(Gray8FrameError::PayloadLength {
                expected: 6,
                actual: 7,
            })
        );
    }

    #[test]
    fn gray8_rejects_zero_dimensions() {
        for (width, height) in [(0, 1), (1, 0), (0, 0)] {
            assert_eq!(
                Gray8Frame::new(width, height, Vec::new()),
                Err(Gray8FrameError::ZeroDimension { width, height })
            );
        }
    }

    #[test]
    fn gray8_preserves_luminance_endpoints_and_intermediate_values() {
        let frame =
            Gray8Frame::new(3, 2, vec![0, 64, 128, 192, 254, 255]).expect("valid Gray8 frame");

        assert_eq!(frame.stride(), 3);
        assert_eq!(frame.pixel(0, 0), Some(0));
        assert_eq!(frame.pixel(2, 0), Some(128));
        assert_eq!(frame.pixel(0, 1), Some(192));
        assert_eq!(frame.pixel(2, 1), Some(255));
        assert_eq!(frame.pixel(3, 0), None);
        assert_eq!(frame.pixel(0, 2), None);
        assert_eq!(frame.clone().into_pixels(), frame.pixels());
    }

    #[test]
    fn format_aware_frame_preserves_format_dimensions_stride_and_pixels() {
        let mono1 =
            Frame::new(9, 1, PixelFormat::Mono1, vec![0xaa, 0x80]).expect("valid Mono1 frame");
        assert_eq!(mono1.pixel_format(), PixelFormat::Mono1);
        assert_eq!((mono1.width(), mono1.height(), mono1.stride()), (9, 1, 2));
        assert_eq!(mono1.pixels(), &[0xaa, 0x80]);

        let gray8 =
            Frame::new(2, 2, PixelFormat::Gray8, vec![0, 64, 128, 255]).expect("valid Gray8 frame");
        assert_eq!(gray8.pixel_format(), PixelFormat::Gray8);
        assert_eq!((gray8.width(), gray8.height(), gray8.stride()), (2, 2, 2));
        assert_eq!(gray8.pixels(), &[0, 64, 128, 255]);
    }
}
