//! Owned writable framebuffer mapping. No production caller writes through it yet.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::ptr::NonNull;

use anyhow::{Context, Result, bail, ensure};

use super::layout::{FramebufferGeometry, visible_mapping_len};
use super::pixels::{FramebufferSpec, PreparedRemote};
use super::system_pixels::SystemUiPixels;
use crate::ui::screen::{SYSTEM_UI_HEIGHT, ScreenLayout, ScreenRect};

pub(super) struct WritableFramebuffer {
    address: NonNull<u8>,
    len: usize,
    geometry: FramebufferGeometry,
}

impl WritableFramebuffer {
    /// # Safety
    /// `geometry` must be current metadata for `file`, and its backing memory
    /// must remain valid for the mapped span until this owner is dropped.
    pub(super) unsafe fn map_visible(file: &File, geometry: FramebufferGeometry) -> Result<Self> {
        let len = visible_mapping_len(geometry).context("writable framebuffer geometry")?;
        ensure!(len <= isize::MAX as usize, "framebuffer mapping too large");

        // SAFETY: `file` remains open during this call. The caller guarantees
        // geometry describes its stable backing memory. MAP_SHARED makes
        // later writes visible to the framebuffer driver, but mmap itself
        // does not change pixels.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if address == libc::MAP_FAILED {
            return Err(io::Error::last_os_error()).context("mmap writable framebuffer");
        }
        let Some(address) = NonNull::new(address.cast::<u8>()) else {
            // SAFETY: even a null address would be a successful mapping here;
            // it cannot form Rust references, so release it before failing.
            let result = unsafe { libc::munmap(address, len) };
            if result != 0 {
                return Err(io::Error::last_os_error()).context("munmap null framebuffer mapping");
            }
            bail!("mmap returned a null framebuffer address");
        };
        Ok(Self {
            address,
            len,
            geometry,
        })
    }

    /// Write a fully in-bounds byte run. A bad range changes no bytes.
    pub(super) fn write_at(&mut self, offset: usize, bytes: &[u8]) -> Result<()> {
        self.check_range(offset, bytes.len())?;

        for (index, &byte) in bytes.iter().enumerate() {
            // SAFETY: the checked end is within the live mapping, and `&mut
            // self` excludes other Rust writers through this owner. Volatile
            // stores avoid assuming the driver-owned bytes are ordinary RAM.
            unsafe { std::ptr::write_volatile(self.address.as_ptr().add(offset + index), byte) };
        }
        Ok(())
    }

    /// Write only the remote rows of a fully prepared frame. All ranges are
    /// checked before the first store. A successful write is not a panel update.
    pub(super) fn write_prepared(&mut self, prepared: &PreparedRemote) -> Result<ScreenRect> {
        ensure!(
            self.matches_spec(prepared.spec()),
            "prepared frame and writable mapping geometry differ"
        );
        ensure!(
            prepared.mapped_len() == self.len,
            "prepared frame and writable mapping lengths differ"
        );
        for (offset, row) in prepared.rows() {
            self.check_range(offset, row.len())?;
        }
        for (offset, row) in prepared.rows() {
            self.write_at(offset, row)?;
        }
        Ok(prepared.region())
    }

    /// Write only the local system strip. Geometry, payload length, and every
    /// mapped row are checked before the first framebuffer store. This does
    /// not request a physical panel refresh.
    pub(super) fn write_system_ui(
        &mut self,
        pixels: &SystemUiPixels,
        spec: FramebufferSpec,
    ) -> Result<ScreenRect> {
        spec.validate_format()?;
        ensure!(
            self.matches_spec(spec),
            "system UI and writable mapping geometry differ"
        );
        let screen = ScreenLayout::new(
            u16::try_from(spec.visible_width)?,
            u16::try_from(spec.visible_height)?,
        )
        .context("empty system UI screen")?;
        ensure!(
            pixels.region == screen.system_ui_region && pixels.region.height == SYSTEM_UI_HEIGHT,
            "system UI pixels do not cover exactly the local strip"
        );
        let width = usize::from(pixels.region.width);
        let height = usize::from(pixels.region.height);
        ensure!(pixels.stride == width, "system UI pixel stride mismatch");
        ensure!(
            pixels.bytes.len()
                == width
                    .checked_mul(height)
                    .context("system UI byte length overflow")?,
            "system UI pixel length mismatch"
        );

        let first_x = usize::try_from(spec.xoffset)?
            .checked_add(usize::from(pixels.region.x))
            .context("system UI horizontal offset overflow")?;
        let first_y = usize::try_from(spec.yoffset)?
            .checked_add(usize::from(pixels.region.y))
            .context("system UI vertical offset overflow")?;
        let mut offsets = Vec::new();
        offsets
            .try_reserve_exact(height)
            .context("allocate system UI row offsets")?;
        for row in 0..height {
            let offset = first_y
                .checked_add(row)
                .and_then(|y| y.checked_mul(spec.line_length))
                .and_then(|start| start.checked_add(first_x))
                .context("system UI row offset overflow")?;
            self.check_range(offset, width)?;
            offsets.push(offset);
        }
        for (row, offset) in offsets.into_iter().enumerate() {
            self.write_at(offset, &pixels.bytes[row * width..(row + 1) * width])?;
        }
        Ok(pixels.region)
    }

    fn check_range(&self, offset: usize, length: usize) -> Result<()> {
        let end = offset
            .checked_add(length)
            .context("framebuffer write offset overflow")?;
        ensure!(end <= self.len, "framebuffer write exceeds mapping");
        Ok(())
    }

    fn matches_spec(&self, spec: FramebufferSpec) -> bool {
        let geometry = self.geometry;
        spec.visible_width == geometry.xres
            && spec.visible_height == geometry.yres
            && spec.virtual_width == geometry.xres_virtual
            && spec.virtual_height == geometry.yres_virtual
            && spec.xoffset == geometry.xoffset
            && spec.yoffset == geometry.yoffset
            && spec.bits_per_pixel == geometry.bits_per_pixel
            && usize::try_from(geometry.line_length).ok() == Some(spec.line_length)
            && usize::try_from(geometry.smem_len).ok() == Some(spec.memory_len)
    }
}

impl Drop for WritableFramebuffer {
    fn drop(&mut self) {
        // SAFETY: this owner holds the original successful mmap address and
        // length, and no method retains a pointer after the owner is dropped.
        if unsafe { libc::munmap(self.address.as_ptr().cast(), self.len) } != 0 {
            eprintln!(
                "mxcfb: munmap writable framebuffer failed: {}",
                io::Error::last_os_error()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;

    use super::super::pixels::prepare_remote;
    use super::super::system_pixels::rasterize_system_ui;
    use super::*;
    use crate::display::RemoteFrame;
    use crate::ui::screen::ScreenLayout;

    fn geometry() -> FramebufferGeometry {
        FramebufferGeometry {
            xres: 4,
            yres: 2,
            xres_virtual: 4,
            yres_virtual: 2,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 8,
            line_length: 4,
            smem_len: 8,
        }
    }

    fn remote_geometry(height: u32) -> FramebufferGeometry {
        FramebufferGeometry {
            xres: 9,
            yres: height,
            xres_virtual: 9,
            yres_virtual: height,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 8,
            line_length: 12,
            smem_len: 12 * height,
        }
    }

    fn prepared_remote() -> PreparedRemote {
        let screen = ScreenLayout::new(9, 73).unwrap();
        let spec = FramebufferSpec {
            visible_width: 9,
            visible_height: 73,
            virtual_width: 9,
            virtual_height: 73,
            xoffset: 0,
            yoffset: 0,
            line_length: 12,
            memory_len: 12 * 73,
            kind: 0,
            visual: 1,
            bits_per_pixel: 8,
            grayscale: 1,
            nonstd: 0,
        };
        let pixels = [0x80, 0x80];
        let frame = RemoteFrame::new(1, 9, 1, 2, &pixels, 0).unwrap();
        prepare_remote(frame, screen, spec, 12 * 73).unwrap()
    }

    fn system_geometry() -> FramebufferGeometry {
        FramebufferGeometry {
            xres: 64,
            yres: 73,
            xres_virtual: 64,
            yres_virtual: 73,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 8,
            line_length: 68,
            smem_len: 68 * 73,
        }
    }

    fn system_spec() -> FramebufferSpec {
        FramebufferSpec {
            visible_width: 64,
            visible_height: 73,
            virtual_width: 64,
            virtual_height: 73,
            xoffset: 0,
            yoffset: 0,
            line_length: 68,
            memory_len: 68 * 73,
            kind: 0,
            visual: 1,
            bits_per_pixel: 8,
            grayscale: 1,
            nonstd: 0,
        }
    }

    #[test]
    fn writes_only_checked_bytes() {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/zero")
            .unwrap();
        // SAFETY: /dev/zero supports an eight-byte shared mapping for this test.
        let mut mapping = unsafe { WritableFramebuffer::map_visible(&file, geometry()) }.unwrap();
        mapping.write_at(2, &[0x12, 0x34]).unwrap();
        mapping.write_at(7, &[0x56]).unwrap();

        // SAFETY: these offsets are within the live eight-byte mapping.
        unsafe {
            assert_eq!(std::ptr::read_volatile(mapping.address.as_ptr().add(1)), 0);
            assert_eq!(
                std::ptr::read_volatile(mapping.address.as_ptr().add(2)),
                0x12
            );
            assert_eq!(
                std::ptr::read_volatile(mapping.address.as_ptr().add(3)),
                0x34
            );
            assert_eq!(std::ptr::read_volatile(mapping.address.as_ptr().add(4)), 0);
        }

        assert!(mapping.write_at(7, &[1, 2]).is_err());
        assert!(mapping.write_at(usize::MAX, &[1]).is_err());
        // SAFETY: the rejected writes must leave the last mapped byte intact.
        assert_eq!(
            unsafe { std::ptr::read_volatile(mapping.address.as_ptr().add(7)) },
            0x56
        );
    }

    #[test]
    fn rejects_bad_geometry_and_read_only_descriptors() {
        let file = File::open("/dev/zero").unwrap();
        let mut invalid = geometry();
        invalid.smem_len = 7;
        // SAFETY: the invalid geometry is rejected before mmap, and /dev/zero
        // otherwise supports the requested eight-byte mapping.
        assert!(unsafe { WritableFramebuffer::map_visible(&file, invalid) }.is_err());
        assert!(unsafe { WritableFramebuffer::map_visible(&file, geometry()) }.is_err());
    }

    #[test]
    fn prepared_write_preserves_padding_and_local_exit_rows() {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/zero")
            .unwrap();
        // SAFETY: /dev/zero supports this 876-byte shared mapping.
        let mut mapping =
            unsafe { WritableFramebuffer::map_visible(&file, remote_geometry(73)) }.unwrap();
        mapping.write_at(0, &vec![0x5a; 12 * 73]).unwrap();
        let prepared = prepared_remote();
        assert_eq!(
            mapping.write_prepared(&prepared).unwrap(),
            prepared.region()
        );

        // SAFETY: every offset read is within the live 876-byte mapping.
        let actual: Vec<u8> = (0..12 * 73)
            .map(|offset| unsafe { std::ptr::read_volatile(mapping.address.as_ptr().add(offset)) })
            .collect();
        assert_eq!(actual[0], 0x00);
        assert!(actual[1..8].iter().all(|&byte| byte == 0xff));
        assert_eq!(actual[8], 0x00);
        assert!(actual[9..].iter().all(|&byte| byte == 0x5a));
    }

    #[test]
    fn mismatched_mapping_rejects_prepared_frame_without_writing() {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/zero")
            .unwrap();
        // SAFETY: /dev/zero supports this 12-byte shared mapping.
        let mut mapping =
            unsafe { WritableFramebuffer::map_visible(&file, remote_geometry(1)) }.unwrap();
        mapping.write_at(0, &[0x5a; 12]).unwrap();
        assert!(mapping.write_prepared(&prepared_remote()).is_err());
        // SAFETY: these offsets cover exactly the live 12-byte mapping.
        for offset in 0..12 {
            assert_eq!(
                unsafe { std::ptr::read_volatile(mapping.address.as_ptr().add(offset)) },
                0x5a
            );
        }
    }

    #[test]
    fn same_length_but_different_geometry_is_rejected() {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/zero")
            .unwrap();
        let mut geometry = remote_geometry(73);
        geometry.xres = 8;
        // SAFETY: /dev/zero supports this 876-byte shared mapping.
        let mut mapping = unsafe { WritableFramebuffer::map_visible(&file, geometry) }.unwrap();
        mapping.write_at(0, &vec![0x5a; 12 * 73]).unwrap();
        let error = mapping.write_prepared(&prepared_remote()).unwrap_err();
        assert!(error.to_string().contains("geometry"));
        // SAFETY: these offsets cover exactly the live 876-byte mapping.
        for offset in 0..12 * 73 {
            assert_eq!(
                unsafe { std::ptr::read_volatile(mapping.address.as_ptr().add(offset)) },
                0x5a
            );
        }
    }

    #[test]
    fn system_ui_write_preserves_remote_row_and_padding() {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/zero")
            .unwrap();
        // SAFETY: /dev/zero supports this validated 4,964-byte mapping.
        let mut mapping =
            unsafe { WritableFramebuffer::map_visible(&file, system_geometry()) }.unwrap();
        mapping.write_at(0, &vec![0x5a; 68 * 73]).unwrap();
        let ui = rasterize_system_ui(ScreenLayout::new(64, 73).unwrap()).unwrap();
        assert_eq!(
            mapping.write_system_ui(&ui, system_spec()).unwrap(),
            ui.region
        );

        // SAFETY: all offsets below lie within the live 4,964-byte mapping.
        let byte =
            |offset| unsafe { std::ptr::read_volatile(mapping.address.as_ptr().add(offset)) };
        assert!((0..68).all(|offset| byte(offset) == 0x5a));
        assert_eq!(byte(68), 0xff);
        assert_eq!(byte(68 + 20), 0x00);
        assert_eq!(byte(68 + 43), 0x00);
        assert_eq!(byte(68 + 44), 0xff);
        assert!((64..68).all(|x| byte(68 + x) == 0x5a));
        assert_eq!(byte(68 * 72 + 20), 0x00);
    }

    #[test]
    fn system_ui_write_honors_virtual_page_offsets() {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/zero")
            .unwrap();
        let geometry = FramebufferGeometry {
            xres_virtual: 65,
            yres_virtual: 75,
            xoffset: 1,
            yoffset: 2,
            smem_len: 68 * 75,
            ..system_geometry()
        };
        let spec = FramebufferSpec {
            virtual_width: 65,
            virtual_height: 75,
            xoffset: 1,
            yoffset: 2,
            memory_len: 68 * 75,
            ..system_spec()
        };
        // SAFETY: /dev/zero supports this validated 5,100-byte mapping.
        let mut mapping = unsafe { WritableFramebuffer::map_visible(&file, geometry) }.unwrap();
        mapping.write_at(0, &vec![0x5a; 68 * 75]).unwrap();
        let ui = rasterize_system_ui(ScreenLayout::new(64, 73).unwrap()).unwrap();
        mapping.write_system_ui(&ui, spec).unwrap();

        // SAFETY: each offset below lies within the live 5,100-byte mapping.
        let byte =
            |offset| unsafe { std::ptr::read_volatile(mapping.address.as_ptr().add(offset)) };
        assert_eq!(byte(0), 0x5a);
        assert_eq!(byte(2 * 68 + 1 + 20), 0x5a); // remote row
        assert_eq!(byte(3 * 68), 0x5a); // horizontal virtual offset
        assert_eq!(byte(3 * 68 + 1), 0xff);
        assert_eq!(byte(3 * 68 + 1 + 20), 0x00);
        assert_eq!(byte(3 * 68 + 65), 0x5a); // row padding
        assert_eq!(byte(74 * 68 + 1 + 20), 0x00);
    }

    #[test]
    fn invalid_system_ui_geometry_and_payload_write_nothing() {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/zero")
            .unwrap();
        // SAFETY: /dev/zero supports this validated 4,964-byte mapping.
        let mut mapping =
            unsafe { WritableFramebuffer::map_visible(&file, system_geometry()) }.unwrap();
        mapping.write_at(0, &vec![0x5a; 68 * 73]).unwrap();
        let mut ui = rasterize_system_ui(ScreenLayout::new(64, 73).unwrap()).unwrap();
        let mut spec = system_spec();
        spec.visual = 0;
        assert!(mapping.write_system_ui(&ui, spec).is_err());
        spec = system_spec();
        ui.region.y = 0;
        assert!(mapping.write_system_ui(&ui, spec).is_err());
        ui.region.y = 1;
        ui.bytes.pop();
        assert!(mapping.write_system_ui(&ui, spec).is_err());
        // SAFETY: all offsets lie within the live 4,964-byte mapping.
        for offset in 0..68 * 73 {
            assert_eq!(
                unsafe { std::ptr::read_volatile(mapping.address.as_ptr().add(offset)) },
                0x5a
            );
        }
    }
}
