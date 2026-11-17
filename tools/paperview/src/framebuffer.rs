use std::collections::TryReserveError;
use std::fmt;

use paper_protocol::{Frame, V2_FRAME_PREFIX_LEN, V2_MAX_PAYLOAD_LEN};

pub const BLACK_RGB: u32 = 0x0000_0000;
pub const WHITE_RGB: u32 = 0x00ff_ffff;
pub const MAX_PRESENTATION_PIXELS: usize = V2_MAX_PAYLOAD_LEN - V2_FRAME_PREFIX_LEN;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationBuffer {
    width: u16,
    height: u16,
    pixels: Vec<u32>,
}

impl PresentationBuffer {
    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    pub fn into_pixels(self) -> Vec<u32> {
        self.pixels
    }

    pub fn update_from_frame(&mut self, frame: &Frame) -> Result<(), ConversionError> {
        let (width, height, pixel_count, byte_count) = validated_layout(frame)?;
        if self.pixels.capacity() < pixel_count {
            self.pixels
                .try_reserve_exact(pixel_count.saturating_sub(self.pixels.len()))
                .map_err(|source| ConversionError::Allocation {
                    pixel_count,
                    byte_count,
                    source,
                })?;
        }

        self.pixels.clear();
        write_pixels(frame, &mut self.pixels);
        self.width = width;
        self.height = height;
        debug_assert_eq!(self.pixels.len(), pixel_count);
        Ok(())
    }
}

#[derive(Debug)]
pub enum ConversionError {
    PixelCountOverflow {
        width: u16,
        height: u16,
    },
    ByteCountOverflow {
        pixel_count: usize,
    },
    TooLarge {
        pixel_count: usize,
        maximum: usize,
    },
    Allocation {
        pixel_count: usize,
        byte_count: usize,
        source: TryReserveError,
    },
}

impl fmt::Display for ConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PixelCountOverflow { width, height } => {
                write!(
                    formatter,
                    "presentation pixel count overflows for {width}x{height}"
                )
            }
            Self::ByteCountOverflow { pixel_count } => write!(
                formatter,
                "presentation byte count overflows for {pixel_count} RGB pixels"
            ),
            Self::TooLarge {
                pixel_count,
                maximum,
            } => write!(
                formatter,
                "presentation needs {pixel_count} pixels, maximum is {maximum}"
            ),
            Self::Allocation {
                pixel_count,
                byte_count,
                source,
            } => write!(
                formatter,
                "could not allocate {byte_count} bytes for {pixel_count} presentation pixels: {source}"
            ),
        }
    }
}

impl std::error::Error for ConversionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Allocation { source, .. } => Some(source),
            Self::PixelCountOverflow { .. }
            | Self::ByteCountOverflow { .. }
            | Self::TooLarge { .. } => None,
        }
    }
}

pub fn convert_frame(frame: &Frame) -> Result<PresentationBuffer, ConversionError> {
    let (width, height, pixel_count, byte_count) = validated_layout(frame)?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(pixel_count)
        .map_err(|source| ConversionError::Allocation {
            pixel_count,
            byte_count,
            source,
        })?;
    write_pixels(frame, &mut pixels);
    debug_assert_eq!(pixels.len(), pixel_count);
    Ok(PresentationBuffer {
        width,
        height,
        pixels,
    })
}

fn validated_layout(frame: &Frame) -> Result<(u16, u16, usize, usize), ConversionError> {
    let width = frame.width();
    let height = frame.height();
    let pixel_count = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or(ConversionError::PixelCountOverflow { width, height })?;
    let byte_count = pixel_count
        .checked_mul(std::mem::size_of::<u32>())
        .ok_or(ConversionError::ByteCountOverflow { pixel_count })?;
    if pixel_count > MAX_PRESENTATION_PIXELS {
        return Err(ConversionError::TooLarge {
            pixel_count,
            maximum: MAX_PRESENTATION_PIXELS,
        });
    }
    Ok((width, height, pixel_count, byte_count))
}

fn write_pixels(frame: &Frame, pixels: &mut Vec<u32>) {
    let width = frame.width();
    match frame {
        Frame::Mono1(frame) => {
            for row in frame.pixels().chunks_exact(frame.stride()) {
                for x in 0..usize::from(width) {
                    let byte = row[x / 8];
                    let mask = 0x80 >> (x % 8);
                    pixels.push(if byte & mask == 0 {
                        WHITE_RGB
                    } else {
                        BLACK_RGB
                    });
                }
            }
        }
        Frame::Gray8(frame) => {
            pixels.extend(frame.pixels().iter().map(|value| {
                let value = u32::from(*value);
                (value << 16) | (value << 8) | value
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use paper_protocol::{Gray8Frame, Mono1Frame};

    use super::*;

    #[test]
    fn mono1_uses_msb_first_bits_and_correct_polarity() {
        let frame = Mono1Frame::new(10, 2, vec![0x80, 0x00, 0x55, 0x40])
            .expect("valid odd-width Mono1 frame");
        let converted = convert_frame(&Frame::Mono1(frame)).expect("convert Mono1");

        assert_eq!((converted.width(), converted.height()), (10, 2));
        assert_eq!(
            &converted.pixels()[0..10],
            &[
                BLACK_RGB, WHITE_RGB, WHITE_RGB, WHITE_RGB, WHITE_RGB, WHITE_RGB, WHITE_RGB,
                WHITE_RGB, WHITE_RGB, WHITE_RGB,
            ]
        );
        assert_eq!(
            &converted.pixels()[10..20],
            &[
                WHITE_RGB, BLACK_RGB, WHITE_RGB, BLACK_RGB, WHITE_RGB, BLACK_RGB, WHITE_RGB,
                BLACK_RGB, WHITE_RGB, BLACK_RGB,
            ]
        );
    }

    #[test]
    fn mono1_padding_is_excluded_and_rows_keep_exact_boundaries() {
        let frame =
            Mono1Frame::new(9, 2, vec![0x00, 0x00, 0xff, 0x80]).expect("valid padded Mono1 rows");
        let converted = convert_frame(&Frame::Mono1(frame)).expect("convert Mono1");

        assert_eq!(converted.pixels().len(), 18);
        assert!(
            converted.pixels()[0..9]
                .iter()
                .all(|pixel| *pixel == WHITE_RGB)
        );
        assert!(
            converted.pixels()[9..18]
                .iter()
                .all(|pixel| *pixel == BLACK_RGB)
        );
    }

    #[test]
    fn gray8_maps_every_luminance_to_equal_rgb_channels() {
        let values = vec![0, 1, 64, 127, 128, 192, 254, 255];
        let frame = Gray8Frame::new(4, 2, values).expect("valid Gray8 frame");
        let converted = convert_frame(&Frame::Gray8(frame)).expect("convert Gray8");

        assert_eq!((converted.width(), converted.height()), (4, 2));
        assert_eq!(
            converted.pixels(),
            &[
                0x0000_0000,
                0x0001_0101,
                0x0040_4040,
                0x007f_7f7f,
                0x0080_8080,
                0x00c0_c0c0,
                0x00fe_fefe,
                0x00ff_ffff,
            ]
        );
    }

    #[test]
    fn oversized_mono1_frame_is_rejected_before_rgb_allocation() {
        let width = 4096;
        let height = 4096;
        let frame = Mono1Frame::new(width, height, vec![0; 4096 / 8 * 4096])
            .expect("valid large Mono1 frame");
        let error = convert_frame(&Frame::Mono1(frame)).expect_err("oversized RGB must fail");
        assert!(
            matches!(
                error,
                ConversionError::TooLarge {
                    pixel_count,
                    maximum,
                } if pixel_count == 4096 * 4096 && maximum == MAX_PRESENTATION_PIXELS
            ),
            "{error}"
        );
    }

    #[test]
    fn matching_dimensions_reuse_the_existing_pixel_allocation() {
        let initial = Gray8Frame::new(4, 2, vec![0; 8]).expect("valid initial Gray8 frame");
        let mut converted = convert_frame(&Frame::Gray8(initial)).expect("convert initial frame");
        let allocation = converted.pixels.as_ptr();
        let capacity = converted.pixels.capacity();
        let replacement =
            Gray8Frame::new(4, 2, vec![255; 8]).expect("valid replacement Gray8 frame");

        converted
            .update_from_frame(&Frame::Gray8(replacement))
            .expect("update presentation");

        assert_eq!(converted.pixels.as_ptr(), allocation);
        assert_eq!(converted.pixels.capacity(), capacity);
        assert!(converted.pixels().iter().all(|pixel| *pixel == WHITE_RGB));
    }

    #[test]
    fn rejected_update_preserves_the_previous_presentation() {
        let initial = Gray8Frame::new(2, 2, vec![42; 4]).expect("valid initial Gray8 frame");
        let mut converted = convert_frame(&Frame::Gray8(initial)).expect("convert initial frame");
        let previous = converted.clone();
        let oversized = Mono1Frame::new(4096, 4096, vec![0; 4096 / 8 * 4096])
            .expect("valid protocol frame larger than RGB presentation limit");

        let error = converted
            .update_from_frame(&Frame::Mono1(oversized))
            .expect_err("oversized replacement must fail");

        assert!(matches!(error, ConversionError::TooLarge { .. }));
        assert_eq!(converted, previous);
    }

    #[test]
    fn presentation_pixels_can_be_taken_without_copying() {
        let frame = Gray8Frame::new(1, 1, vec![42]).expect("valid Gray8 frame");
        let converted = convert_frame(&Frame::Gray8(frame)).expect("convert Gray8");
        assert_eq!(converted.into_pixels(), vec![0x002a_2a2a]);
    }
}
