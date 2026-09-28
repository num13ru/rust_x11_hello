//! Protocol framebuffer pixels to the probed 8-bit grayscale layout.
//!
//! This module has no device access. It assumes the standard Linux
//! `FB_VISUAL_MONO10` polarity (`0x00` black, `0xff` white). Mono1 is expanded
//! to those endpoints; Gray8 luminance bytes are copied without inversion.

use crate::display::RemoteFrame;
use crate::ui::screen::{ScreenLayout, ScreenRect};
use anyhow::{Context, Result, ensure};
use paper_protocol::PixelFormat;

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

impl FramebufferSpec {
    pub(super) fn validate_format(self) -> Result<()> {
        ensure!(
            self.kind == FB_TYPE_PACKED_PIXELS
                && self.visual == FB_VISUAL_MONO10
                && self.bits_per_pixel == 8
                && self.grayscale == 1
                && self.nonstd == 0,
            "unsupported framebuffer format: type={} visual={} bpp={} grayscale={} nonstd={}",
            self.kind,
            self.visual,
            self.bits_per_pixel,
            self.grayscale,
            self.nonstd
        );
        Ok(())
    }
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
    spec.validate_format()?;

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
        match frame.pixel_format() {
            PixelFormat::Mono1 => {
                for (column, byte) in destination.iter_mut().enumerate() {
                    let black = source[column / 8] & (0x80 >> (column % 8)) != 0;
                    *byte = if black { 0x00 } else { 0xff };
                }
            }
            PixelFormat::Gray8 => destination.copy_from_slice(source),
        }
    }
    Ok(region)
}

/// Fully prepared native rows. Only `rows()` exposes byte ranges for the
/// remote viewport; bytes representing PaperPad's local UI stay private.
pub(super) struct PreparedRemote {
    region: ScreenRect,
    spec: FramebufferSpec,
    buffer: Vec<u8>,
    first_offset: usize,
    line_length: usize,
    width: usize,
    height: usize,
}

impl PreparedRemote {
    pub(super) fn region(&self) -> ScreenRect {
        self.region
    }

    pub(super) fn mapped_len(&self) -> usize {
        self.buffer.len()
    }

    pub(super) fn spec(&self) -> FramebufferSpec {
        self.spec
    }

    pub(super) fn rows(&self) -> impl Iterator<Item = (usize, &[u8])> {
        (0..self.height).map(move |row| {
            let offset = self.first_offset + row * self.line_length;
            (offset, &self.buffer[offset..offset + self.width])
        })
    }
}

/// Prepare every remote byte before a later framebuffer write. The temporary
/// buffer spans only the mapped visible page; reservation errors return before
/// any device write.
pub(super) fn prepare_remote(
    frame: RemoteFrame<'_>,
    screen: ScreenLayout,
    spec: FramebufferSpec,
    mapped_len: usize,
) -> Result<PreparedRemote> {
    ensure!(mapped_len > 0, "empty framebuffer mapping");
    let visible_bottom = spec
        .yoffset
        .checked_add(spec.visible_height)
        .context("framebuffer vertical bounds overflow")?;
    let expected_len = usize::try_from(visible_bottom)?
        .checked_mul(spec.line_length)
        .context("framebuffer visible byte length overflow")?;
    ensure!(
        mapped_len == expected_len,
        "framebuffer mapping length {mapped_len} does not match visible span {expected_len}"
    );
    ensure!(
        mapped_len <= spec.memory_len && mapped_len <= isize::MAX as usize,
        "invalid framebuffer mapping length"
    );
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(mapped_len)
        .context("allocate native framebuffer staging buffer")?;
    buffer.resize(mapped_len, 0);
    let region = blit_remote(frame, screen, spec, &mut buffer)?;

    let first_x = usize::try_from(spec.xoffset)?
        .checked_add(usize::from(region.x))
        .context("remote framebuffer column overflow")?;
    let first_y = usize::try_from(spec.yoffset)?
        .checked_add(usize::from(region.y))
        .context("remote framebuffer row overflow")?;
    let first_offset = first_y
        .checked_mul(spec.line_length)
        .and_then(|offset| offset.checked_add(first_x))
        .context("remote framebuffer byte offset overflow")?;
    let width = usize::from(region.width);
    let height = usize::from(region.height);
    let last_end = (height - 1)
        .checked_mul(spec.line_length)
        .and_then(|offset| offset.checked_add(first_offset))
        .and_then(|offset| offset.checked_add(width))
        .context("remote framebuffer byte offset overflow")?;
    ensure!(
        last_end <= mapped_len,
        "remote rows exceed framebuffer mapping"
    );

    Ok(PreparedRemote {
        region,
        spec,
        buffer,
        first_offset,
        line_length: spec.line_length,
        width,
        height,
    })
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
            let frame = RemoteFrame::new(
                1,
                width,
                1,
                paper_protocol::PixelFormat::Mono1,
                pixels.len(),
                &pixels,
                0,
            );
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
        let frame =
            RemoteFrame::new(2, 9, 2, paper_protocol::PixelFormat::Mono1, 2, &pixels, 0).unwrap();
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
    fn gray8_copies_luminance_with_offsets_padding_and_exit_untouched() {
        let screen = screen(5, 2);
        let mut spec = spec(screen, 0);
        spec.xoffset = 2;
        spec.yoffset = 1;
        spec.virtual_width = 7;
        spec.virtual_height += 1;
        spec.line_length = 10;
        spec.memory_len = spec.line_length * usize::try_from(spec.virtual_height).unwrap();

        let pixels = [0, 64, 128, 192, 255, 255, 192, 128, 64, 0];
        let frame =
            RemoteFrame::new(2, 5, 2, paper_protocol::PixelFormat::Gray8, 5, &pixels, 0).unwrap();
        let mut memory = vec![0x5a; spec.memory_len];
        let mut expected = memory.clone();
        expected[spec.line_length + 2..spec.line_length + 7].copy_from_slice(&pixels[..5]);
        expected[spec.line_length * 2 + 2..spec.line_length * 2 + 7].copy_from_slice(&pixels[5..]);

        assert_eq!(
            blit_remote(frame, screen, spec, &mut memory).unwrap(),
            screen.remote_viewport
        );
        assert_eq!(memory, expected);
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
        let frame =
            RemoteFrame::new(3, 9, 1, paper_protocol::PixelFormat::Mono1, 2, &pixels, 0).unwrap();
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
        let frame = RemoteFrame::new(
            6,
            1272,
            1624,
            paper_protocol::PixelFormat::Mono1,
            159,
            &pixels,
            0,
        )
        .unwrap();
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
    fn prepared_rows_match_direct_blit_with_offsets_and_padding() {
        let screen = screen(9, 2);
        let mut spec = spec(screen, 0);
        spec.xoffset = 2;
        spec.yoffset = 1;
        spec.virtual_width = 11;
        spec.virtual_height += 1;
        spec.line_length = 16;
        spec.memory_len = spec.line_length * usize::try_from(spec.virtual_height).unwrap();
        let pixels = mono(9, 2, &[(0, 0), (8, 1)]);
        let frame =
            RemoteFrame::new(7, 9, 2, paper_protocol::PixelFormat::Mono1, 2, &pixels, 0).unwrap();

        let prepared = prepare_remote(frame, screen, spec, spec.memory_len).unwrap();
        let mut actual = vec![0x5a; spec.memory_len];
        let rows: Vec<_> = prepared.rows().collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, spec.line_length + 2);
        assert_eq!(rows[1].0, spec.line_length * 2 + 2);
        for (offset, bytes) in rows {
            actual[offset..offset + bytes.len()].copy_from_slice(bytes);
        }

        let mut expected = vec![0x5a; spec.memory_len];
        assert_eq!(
            prepared.region(),
            blit_remote(frame, screen, spec, &mut expected).unwrap()
        );
        assert_eq!(actual, expected);
    }

    #[test]
    fn prepared_rows_reject_short_mapping_and_unsupported_format() {
        let screen = screen(9, 1);
        let spec = spec(screen, 3);
        let pixels = mono(9, 1, &[]);
        let frame =
            RemoteFrame::new(8, 9, 1, paper_protocol::PixelFormat::Mono1, 2, &pixels, 0).unwrap();
        assert!(prepare_remote(frame, screen, spec, 8).is_err());
        assert!(prepare_remote(frame, screen, spec, spec.memory_len + 1).is_err());
        assert!(
            prepare_remote(
                frame,
                screen,
                FramebufferSpec { visual: 0, ..spec },
                spec.memory_len
            )
            .is_err()
        );
    }

    #[test]
    fn prepared_paperwhite_rows_stop_at_exit_strip() {
        let screen = ScreenLayout::new(1272, 1696).unwrap();
        let mut spec = spec(screen, 0);
        spec.virtual_height = 3392;
        spec.memory_len = 4_314_624;
        let mapped_len = 1272 * 1696;
        let pixels = mono(1272, 1624, &[(1271, 1623)]);
        let frame = RemoteFrame::new(
            9,
            1272,
            1624,
            paper_protocol::PixelFormat::Mono1,
            159,
            &pixels,
            0,
        )
        .unwrap();
        let prepared = prepare_remote(frame, screen, spec, mapped_len).unwrap();

        assert_eq!(prepared.region(), screen.remote_viewport);
        assert_eq!(prepared.rows().count(), 1624);
        let (last_offset, last_row) = prepared.rows().last().unwrap();
        assert_eq!(last_offset + last_row.len(), 1272 * 1624);
        assert!(last_offset + last_row.len() < mapped_len);
    }

    #[test]
    fn invalid_format_geometry_and_bounds_leave_memory_unchanged() {
        let screen = screen(9, 1);
        let base = spec(screen, 3);
        let pixels = mono(9, 1, &[]);
        let frame =
            RemoteFrame::new(4, 9, 1, paper_protocol::PixelFormat::Mono1, 2, &pixels, 0).unwrap();
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
        let wrong_frame = RemoteFrame::new(
            5,
            8,
            1,
            paper_protocol::PixelFormat::Mono1,
            1,
            &wrong_pixels,
            0,
        )
        .unwrap();
        let mut memory = vec![0x5a; base.memory_len];
        assert!(blit_remote(wrong_frame, screen, base, &mut memory).is_err());
        assert!(memory.iter().all(|&byte| byte == 0x5a));

        let mut short = vec![0x5a; 8];
        assert!(blit_remote(frame, screen, base, &mut short).is_err());
        assert!(short.iter().all(|&byte| byte == 0x5a));
    }
}
