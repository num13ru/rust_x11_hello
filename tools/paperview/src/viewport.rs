use std::fmt;

use crate::framebuffer::PresentationBuffer;

pub const LETTERBOX_RGB: u32 = 0x0080_8080;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FitRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderError {
    TargetSizeOverflow { width: u32, height: u32 },
    BufferLength { expected: usize, actual: usize },
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TargetSizeOverflow { width, height } => {
                write!(
                    formatter,
                    "target pixel count overflows usize for {width}x{height}"
                )
            }
            Self::BufferLength { expected, actual } => write!(
                formatter,
                "target buffer length mismatch: expected {expected} pixels, got {actual}"
            ),
        }
    }
}

impl std::error::Error for RenderError {}

pub fn fit_rect(
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> Option<FitRect> {
    if source_width == 0 || source_height == 0 || target_width == 0 || target_height == 0 {
        return None;
    }

    let (width, height) = if u64::from(target_width) * u64::from(source_height)
        <= u64::from(target_height) * u64::from(source_width)
    {
        let height =
            (u64::from(source_height) * u64::from(target_width) / u64::from(source_width)) as u32;
        (target_width, height.max(1))
    } else {
        let width =
            (u64::from(source_width) * u64::from(target_height) / u64::from(source_height)) as u32;
        (width.max(1), target_height)
    };
    Some(FitRect {
        x: (target_width - width) / 2,
        y: (target_height - height) / 2,
        width,
        height,
    })
}

pub fn render_fit(
    source: &PresentationBuffer,
    target_width: u32,
    target_height: u32,
    target: &mut [u32],
) -> Result<Option<FitRect>, RenderError> {
    let expected =
        usize::try_from(u64::from(target_width) * u64::from(target_height)).map_err(|_| {
            RenderError::TargetSizeOverflow {
                width: target_width,
                height: target_height,
            }
        })?;
    if target.len() != expected {
        return Err(RenderError::BufferLength {
            expected,
            actual: target.len(),
        });
    }
    target.fill(LETTERBOX_RGB);
    let Some(rect) = fit_rect(
        u32::from(source.width()),
        u32::from(source.height()),
        target_width,
        target_height,
    ) else {
        return Ok(None);
    };

    let source_width = usize::from(source.width());
    for target_y in 0..rect.height {
        let source_y =
            (u64::from(target_y) * u64::from(source.height()) / u64::from(rect.height)) as usize;
        let target_row = usize::try_from(rect.y + target_y).expect("target row fits usize")
            * usize::try_from(target_width).expect("target width fits usize");
        for target_x in 0..rect.width {
            let source_x =
                (u64::from(target_x) * u64::from(source.width()) / u64::from(rect.width)) as usize;
            let target_index =
                target_row + usize::try_from(rect.x + target_x).expect("target column fits usize");
            target[target_index] = source.pixels()[source_y * source_width + source_x];
        }
    }
    Ok(Some(rect))
}

#[cfg(test)]
mod tests {
    use paper_protocol::{Frame, Gray8Frame};

    use crate::framebuffer::convert_frame;

    use super::*;

    fn presentation(width: u16, height: u16, pixels: Vec<u8>) -> PresentationBuffer {
        let frame = Gray8Frame::new(width, height, pixels).expect("valid Gray8 frame");
        convert_frame(&Frame::Gray8(frame)).expect("convert frame")
    }

    #[test]
    fn fit_preserves_aspect_ratio_and_centers_letterboxing() {
        assert_eq!(
            fit_rect(2, 4, 8, 4),
            Some(FitRect {
                x: 3,
                y: 0,
                width: 2,
                height: 4,
            })
        );
        assert_eq!(
            fit_rect(4, 2, 4, 8),
            Some(FitRect {
                x: 0,
                y: 3,
                width: 4,
                height: 2,
            })
        );
        assert_eq!(
            fit_rect(4, 2, 8, 4),
            Some(FitRect {
                x: 0,
                y: 0,
                width: 8,
                height: 4,
            })
        );
    }

    #[test]
    fn zero_extent_has_no_fit_rectangle() {
        assert_eq!(fit_rect(0, 2, 8, 8), None);
        assert_eq!(fit_rect(2, 0, 8, 8), None);
        assert_eq!(fit_rect(2, 2, 0, 8), None);
        assert_eq!(fit_rect(2, 2, 8, 0), None);
    }

    #[test]
    fn nearest_neighbor_upscale_repeats_exact_source_pixels() {
        let source = presentation(2, 2, vec![0, 64, 128, 255]);
        let mut target = vec![0; 16];
        assert_eq!(
            render_fit(&source, 4, 4, &mut target).expect("render fit"),
            Some(FitRect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            })
        );
        assert_eq!(
            target,
            vec![
                0x0000_0000,
                0x0000_0000,
                0x0040_4040,
                0x0040_4040,
                0x0000_0000,
                0x0000_0000,
                0x0040_4040,
                0x0040_4040,
                0x0080_8080,
                0x0080_8080,
                0x00ff_ffff,
                0x00ff_ffff,
                0x0080_8080,
                0x0080_8080,
                0x00ff_ffff,
                0x00ff_ffff,
            ]
        );
    }

    #[test]
    fn downscale_uses_predictable_floor_sampling() {
        let source = presentation(4, 1, vec![0, 64, 128, 255]);
        let mut target = vec![0; 2];
        render_fit(&source, 2, 1, &mut target).expect("render fit");
        assert_eq!(target, vec![0x0000_0000, 0x0080_8080]);
    }

    #[test]
    fn letterbox_pixels_remain_neutral() {
        let source = presentation(2, 4, vec![255; 8]);
        let mut target = vec![0; 8 * 4];
        render_fit(&source, 8, 4, &mut target).expect("render fit");
        for row in target.chunks_exact(8) {
            assert_eq!(
                row,
                &[
                    LETTERBOX_RGB,
                    LETTERBOX_RGB,
                    LETTERBOX_RGB,
                    0x00ff_ffff,
                    0x00ff_ffff,
                    LETTERBOX_RGB,
                    LETTERBOX_RGB,
                    LETTERBOX_RGB
                ]
            );
        }
    }

    #[test]
    fn target_buffer_length_is_validated_before_drawing() {
        let source = presentation(1, 1, vec![255]);
        let mut target = vec![0xdead_beef; 3];
        let error = render_fit(&source, 2, 2, &mut target).expect_err("short buffer must fail");
        assert_eq!(
            error,
            RenderError::BufferLength {
                expected: 4,
                actual: 3,
            }
        );
        assert_eq!(target, vec![0xdead_beef; 3]);
    }
}
