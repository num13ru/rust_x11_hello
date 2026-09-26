//! Mono1 to the probed 8-bit grayscale framebuffer layout.
//!
//! This module has no device access. It assumes the standard Linux
//! `FB_VISUAL_MONO10` polarity (`0x00` black, `0xff` white), which still needs
//! a physical display test on this Kindle.

use crate::display::RemoteFrame;
use crate::ui::screen::{ScreenLayout, ScreenRect};
use anyhow::{Context, Result, ensure};

const FB_TYPE_PACKED_PIXELS: u32 = 0;
const FB_VISUAL_MONO10: u32 = 1;

#[derive(Clone, Copy, Debug)]
pub(super) struct FramebufferSpec {
    pub visible_width: u32,
    pub visible_height: u32,
    pub virtual_width: u32,
    pub virtual_height: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub line_length: usize,
    pub memory_len: usize,
    pub kind: u32,
    pub visual: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    pub nonstd: u32,
}

/// Convert one validated remote frame into a memory buffer representing
/// `/dev/fb0`. All layout and range checks complete before any byte changes.
/// The returned rectangle is in visible-screen coordinates for a later update
/// submission; this function does not submit one.
pub(super) fn blit_remote(
    frame: RemoteFrame<'_>,
    screen: ScreenLayout,
    spec: FramebufferSpec,
    memory: &mut [u8],
) -> Result<ScreenRect> {
    ensure!(
        spec.kind == FB_TYPE_PACKED_PIXELS
            && spec.visual == FB_VISUAL_MONO10
            && spec.bits_per_pixel == 8
            && spec.grayscale == 1
            && spec.nonstd == 0,
        "unsupported framebuffer format: type={} visual={} bpp={} grayscale={} nonstd={}",
        spec.kind,
        spec.visual,
        spec.bits_per_pixel,
        spec.grayscale,
        spec.nonstd
    );

    let (screen_width, screen_height) = screen.physical_size();
    ensure!(
        spec.visible_width == u32::from(screen_width)
            && spec.visible_height == u32::from(screen_height),
        "framebuffer visible dimensions {}x{} do not match screen {}x{}",
        spec.visible_width,
        spec.visible_height,
        screen_width,
        screen_height
    );

    let region = screen.remote_viewport;
    ensure!(
        (frame.width(), frame.height()) == (region.width, region.height),
        "remote frame {}x{} does not match viewport {}x{}",
        frame.width(),
        frame.height(),
        region.width,
        region.height
    );
    ensure!(
        u32::from(region.y) + u32::from(region.height) <= u32::from(screen.system_ui_region.y),
        "remote viewport overlaps PaperPad system UI"
    );

    let visible_right = spec
        .xoffset
        .checked_add(spec.visible_width)
        .context("framebuffer horizontal bounds overflow")?;
    let visible_bottom = spec
        .yoffset
        .checked_add(spec.visible_height)
        .context("framebuffer vertical bounds overflow")?;
    ensure!(
        visible_right <= spec.virtual_width && visible_bottom <= spec.virtual_height,
        "visible framebuffer lies outside virtual dimensions"
    );

    let virtual_width = usize::try_from(spec.virtual_width)?;
    let virtual_height = usize::try_from(spec.virtual_height)?;
    ensure!(
        spec.line_length >= virtual_width,
        "8-bit framebuffer row is shorter than virtual width"
    );
    let virtual_bytes = spec
        .line_length
        .checked_mul(virtual_height)
        .context("framebuffer byte length overflow")?;
    ensure!(
        virtual_bytes <= spec.memory_len,
        "virtual framebuffer exceeds reported memory length"
    );

    let first_x = usize::try_from(spec.xoffset)?
        .checked_add(usize::from(region.x))
        .context("remote framebuffer column overflow")?;
    let first_y = usize::try_from(spec.yoffset)?
        .checked_add(usize::from(region.y))
        .context("remote framebuffer row overflow")?;
    let width = usize::from(region.width);
    let height = usize::from(region.height);
    ensure!(width > 0 && height > 0, "empty remote viewport");
    let last_row = first_y
        .checked_add(height - 1)
        .context("remote framebuffer row overflow")?;
    let end = last_row
        .checked_mul(spec.line_length)
        .and_then(|offset| offset.checked_add(first_x))
        .and_then(|offset| offset.checked_add(width))
        .context("remote framebuffer byte offset overflow")?;
    ensure!(
        end <= spec.memory_len && end <= memory.len(),
        "remote frame exceeds mapped framebuffer memory"
    );

    let pixels = frame.pixels();
    for row in 0..height {
        let source = &pixels[row * frame.stride()..][..frame.stride()];
        let destination_start = (first_y + row) * spec.line_length + first_x;
        let destination = &mut memory[destination_start..destination_start + width];
        for (column, byte) in destination.iter_mut().enumerate() {
            let black = source[column / 8] & (0x80 >> (column % 8)) != 0;
            *byte = if black { 0x00 } else { 0xff };
        }
    }
    Ok(region)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::screen::SYSTEM_UI_HEIGHT;

    fn screen(width: u16, remote_height: u16) -> ScreenLayout {
        ScreenLayout::new(width, remote_height + SYSTEM_UI_HEIGHT).unwrap()
    }

    fn spec(screen: ScreenLayout, line_padding: usize) -> FramebufferSpec {
        let (width, height) = screen.physical_size();
        let line_length = usize::from(width) + line_padding;
        FramebufferSpec {
            visible_width: u32::from(width),
            visible_height: u32::from(height),
            virtual_width: u32::from(width),
            virtual_height: u32::from(height),
            xoffset: 0,
            yoffset: 0,
            line_length,
            memory_len: line_length * usize::from(height),
            kind: FB_TYPE_PACKED_PIXELS,
            visual: FB_VISUAL_MONO10,
            bits_per_pixel: 8,
            grayscale: 1,
            nonstd: 0,
        }
    }

    fn mono(width: u16, height: u16, black: &[(usize, usize)]) -> Vec<u8> {
        let stride = usize::from(width).div_ceil(8);
        let mut pixels = vec![0; stride * usize::from(height)];
        for &(x, y) in black {
            pixels[y * stride + x / 8] |= 0x80 >> (x % 8);
        }
        pixels
    }

    #[test]
    fn awkward_widths_preserve_row_padding_and_local_exit_strip() {
        for width in [1, 7, 8, 9, 1271, 1272] {
            let screen = screen(width, 1);
            let spec = spec(screen, 3);
            let pixels = mono(width, 1, &[(0, 0), (usize::from(width) - 1, 0)]);
            let frame = RemoteFrame::new(1, width, 1, pixels.len(), &pixels, 0);
            // The test uses a single Mono1 row, so its payload length is its stride.
            let frame = frame.unwrap();
            let mut memory = vec![0x5a; spec.memory_len];

            let region = blit_remote(frame, screen, spec, &mut memory).unwrap();
            assert_eq!(region, screen.remote_viewport);
            for (x, &actual) in memory.iter().take(usize::from(width)).enumerate() {
                let expected = if x == 0 || x == usize::from(width) - 1 {
                    0x00
                } else {
                    0xff
                };
                assert_eq!(actual, expected, "width={width} x={x}");
            }
            assert!(
                memory[usize::from(width)..]
                    .iter()
                    .all(|&byte| byte == 0x5a)
            );
        }
    }

    #[test]
    fn last_remote_row_stops_before_system_ui() {
        let screen = screen(9, 2);
        let spec = spec(screen, 3);
        let pixels = mono(9, 2, &[(0, 0), (8, 1)]);
        let frame = RemoteFrame::new(2, 9, 2, 2, &pixels, 0).unwrap();
        let mut memory = vec![0x5a; spec.memory_len];

        blit_remote(frame, screen, spec, &mut memory).unwrap();

        assert_eq!(memory[0], 0x00);
        assert_eq!(memory[spec.line_length + 8], 0x00);
        assert!(
            memory[spec.line_length * 2..]
                .iter()
                .all(|&byte| byte == 0x5a)
        );
    }

    #[test]
    fn virtual_offsets_and_stride_are_honored() {
        let screen = screen(9, 1);
        let mut spec = spec(screen, 0);
        spec.xoffset = 2;
        spec.yoffset = 1;
        spec.virtual_width = 11;
        spec.virtual_height += 1;
        spec.line_length = 16;
        spec.memory_len = spec.line_length * usize::try_from(spec.virtual_height).unwrap();
        let pixels = mono(9, 1, &[(0, 0), (8, 0)]);
        let frame = RemoteFrame::new(3, 9, 1, 2, &pixels, 0).unwrap();
        let mut memory = vec![0x5a; spec.memory_len];

        blit_remote(frame, screen, spec, &mut memory).unwrap();

        assert_eq!(memory[spec.line_length + 2], 0x00);
        assert_eq!(memory[spec.line_length + 10], 0x00);
        assert_eq!(memory[spec.line_length + 3], 0xff);
        assert!(
            memory[..spec.line_length + 2]
                .iter()
                .all(|&byte| byte == 0x5a)
        );
        assert!(
            memory[spec.line_length + 11..]
                .iter()
                .all(|&byte| byte == 0x5a)
        );
    }

    #[test]
    fn probed_paperwhite_geometry_keeps_exit_and_second_page_untouched() {
        let screen = ScreenLayout::new(1272, 1696).unwrap();
        let mut spec = spec(screen, 0);
        spec.virtual_height = 3392;
        spec.memory_len = 4_314_624;
        let pixels = mono(1272, 1624, &[(0, 0), (1271, 1623)]);
        let frame = RemoteFrame::new(6, 1272, 1624, 159, &pixels, 0).unwrap();
        let mut memory = vec![0x5a; spec.memory_len];

        blit_remote(frame, screen, spec, &mut memory).unwrap();

        assert_eq!(memory[0], 0x00);
        assert_eq!(memory[1623 * spec.line_length + 1271], 0x00);
        assert!(
            memory[1624 * spec.line_length..]
                .iter()
                .all(|&byte| byte == 0x5a)
        );
    }

    #[test]
    fn invalid_format_geometry_and_bounds_leave_memory_unchanged() {
        let screen = screen(9, 1);
        let base = spec(screen, 3);
        let pixels = mono(9, 1, &[]);
        let frame = RemoteFrame::new(4, 9, 1, 2, &pixels, 0).unwrap();
        let cases = vec![
            FramebufferSpec {
                bits_per_pixel: 4,
                ..base
            },
            FramebufferSpec { visual: 0, ..base },
            FramebufferSpec {
                xoffset: u32::MAX,
                ..base
            },
            FramebufferSpec {
                line_length: usize::MAX,
                ..base
            },
            FramebufferSpec {
                memory_len: 1,
                ..base
            },
        ];
        for invalid in cases {
            let mut memory = vec![0x5a; base.memory_len];
            assert!(blit_remote(frame, screen, invalid, &mut memory).is_err());
            assert!(memory.iter().all(|&byte| byte == 0x5a));
        }

        let wrong_pixels = mono(8, 1, &[]);
        let wrong_frame = RemoteFrame::new(5, 8, 1, 1, &wrong_pixels, 0).unwrap();
        let mut memory = vec![0x5a; base.memory_len];
        assert!(blit_remote(wrong_frame, screen, base, &mut memory).is_err());
        assert!(memory.iter().all(|&byte| byte == 0x5a));

        let mut short = vec![0x5a; 8];
        assert!(blit_remote(frame, screen, base, &mut short).is_err());
        assert!(short.iter().all(|&byte| byte == 0x5a));
    }
}
