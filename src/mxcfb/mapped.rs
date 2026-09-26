//! Owned writable framebuffer mapping. No production caller writes through it yet.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::ptr::NonNull;

use anyhow::{Context, Result, bail, ensure};

use super::layout::{FramebufferGeometry, visible_mapping_len};

pub(super) struct WritableFramebuffer {
    address: NonNull<u8>,
    len: usize,
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
        Ok(Self { address, len })
    }

    /// Write a fully in-bounds byte run. A bad range changes no bytes.
    pub(super) fn write_at(&mut self, offset: usize, bytes: &[u8]) -> Result<()> {
        let end = offset
            .checked_add(bytes.len())
            .context("framebuffer write offset overflow")?;
        ensure!(end <= self.len, "framebuffer write exceeds mapping");

        for (index, &byte) in bytes.iter().enumerate() {
            // SAFETY: the checked end is within the live mapping, and `&mut
            // self` excludes other Rust writers through this owner. Volatile
            // stores avoid assuming the driver-owned bytes are ordinary RAM.
            unsafe { std::ptr::write_volatile(self.address.as_ptr().add(offset + index), byte) };
        }
        Ok(())
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

    use super::*;

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
}
