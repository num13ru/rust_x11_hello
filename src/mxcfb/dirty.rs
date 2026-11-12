//! Pure remote-frame difference bounds for MXCFB update regions.

use paper_protocol::PixelFormat;

use crate::ui::screen::ScreenRect;

/// Return the smallest changed rectangle in remote-viewport coordinates.
///
/// Mono1 horizontal bounds expand to whole source bytes. Both payloads have
/// already crossed [`crate::display::RemoteFrame`] validation, so their stride
/// and length match the supplied format and dimensions.
pub(super) fn changed_region(
    width: u16,
    height: u16,
    pixel_format: PixelFormat,
    stride: usize,
    previous: Option<&[u8]>,
    current: &[u8],
) -> Option<ScreenRect> {
    let Some(previous) = previous.filter(|pixels| pixels.len() == current.len()) else {
        return Some(ScreenRect {
            x: 0,
            y: 0,
            width,
            height,
        });
    };

    let mut left = usize::from(width);
    let mut right = 0usize;
    let mut top = usize::from(height);
    let mut bottom = 0usize;

    for y in 0..usize::from(height) {
        let row_start = y * stride;
        let previous_row = &previous[row_start..row_start + stride];
        let current_row = &current[row_start..row_start + stride];
        for (index, (&before, &after)) in previous_row.iter().zip(current_row).enumerate() {
            if before == after {
                continue;
            }
            let (pixel_left, pixel_right) = match pixel_format {
                PixelFormat::Mono1 => (index * 8, ((index + 1) * 8).min(usize::from(width))),
                PixelFormat::Gray8 => (index, index + 1),
            };
            left = left.min(pixel_left);
            right = right.max(pixel_right);
            top = top.min(y);
            bottom = bottom.max(y + 1);
        }
    }

    (right > left && bottom > top).then(|| ScreenRect {
        x: u16::try_from(left).expect("dirty x fits validated frame width"),
        y: u16::try_from(top).expect("dirty y fits validated frame height"),
        width: u16::try_from(right - left).expect("dirty width fits validated frame width"),
        height: u16::try_from(bottom - top).expect("dirty height fits validated frame height"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_or_mismatched_frame_refreshes_the_full_viewport() {
        let full = ScreenRect {
            x: 0,
            y: 0,
            width: 9,
            height: 2,
        };
        assert_eq!(
            changed_region(9, 2, PixelFormat::Mono1, 2, None, &[0; 4]),
            Some(full)
        );
        assert_eq!(
            changed_region(9, 2, PixelFormat::Mono1, 2, Some(&[0; 2]), &[0; 4]),
            Some(full)
        );
    }

    #[test]
    fn identical_pixels_need_no_update() {
        let pixels = [0, 1, 2, 3];
        assert_eq!(
            changed_region(2, 2, PixelFormat::Gray8, 2, Some(&pixels), &pixels),
            None
        );
    }

    #[test]
    fn gray8_bounds_only_changed_pixels() {
        let previous = [0; 12];
        let mut current = previous;
        current[1] = 1;
        current[10] = 1;
        assert_eq!(
            changed_region(4, 3, PixelFormat::Gray8, 4, Some(&previous), &current,),
            Some(ScreenRect {
                x: 1,
                y: 0,
                width: 2,
                height: 3,
            })
        );
    }

    #[test]
    fn mono1_bounds_expand_to_source_bytes_and_clip_last_byte() {
        let previous = [0; 6];
        let mut current = previous;
        current[1] = 0x80;
        current[4] = 0x40;
        assert_eq!(
            changed_region(9, 3, PixelFormat::Mono1, 2, Some(&previous), &current,),
            Some(ScreenRect {
                x: 0,
                y: 0,
                width: 9,
                height: 3,
            })
        );

        let mut last_byte_only = previous;
        last_byte_only[5] = 0x80;
        assert_eq!(
            changed_region(
                9,
                3,
                PixelFormat::Mono1,
                2,
                Some(&previous),
                &last_byte_only,
            ),
            Some(ScreenRect {
                x: 8,
                y: 2,
                width: 1,
                height: 1,
            })
        );
    }
}
