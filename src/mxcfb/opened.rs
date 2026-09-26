//! MXCFB framebuffer opener. Opening alone maps writable memory but neither
//! writes pixels nor refreshes ink.

use std::fs::{File, OpenOptions};

use anyhow::{Context, Result};

use super::abi;
use super::hwtcon;
use super::mapped::WritableFramebuffer;
use super::metadata::FramebufferInfo;

pub(super) struct OpenedFramebuffer {
    // Drop the mapping before closing the descriptor, even though mmap itself
    // remains valid after close(2).
    pub mapping: WritableFramebuffer,
    pub file: File,
    pub info: FramebufferInfo,
}

impl OpenedFramebuffer {
    pub(super) fn open() -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fb0")
            .context("MXCFB open /dev/fb0 read-write")?;
        Self::from_file(file)
    }

    fn from_file(file: File) -> Result<Self> {
        let fixed = abi::fixed_info(&file).context("MXCFB FBIOGET_FSCREENINFO")?;
        let variable = abi::variable_info(&file).context("MXCFB FBIOGET_VSCREENINFO")?;
        let info = FramebufferInfo::from_kernel(&fixed, &variable)
            .context("MXCFB framebuffer metadata")?;
        hwtcon::query_panel_info(&file).context("MXCFB GET_PANEL_INFO_MTK")?;

        // SAFETY: geometry came from the two ioctls on this same open
        // descriptor, was bounds-checked against its reported framebuffer
        // memory, and is assumed stable for the lifetime of this mapping.
        let mapping = unsafe { WritableFramebuffer::map_visible(&file, info.geometry) }
            .context("MXCFB map validated visible framebuffer")?;
        Ok(Self {
            mapping,
            file,
            info,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_framebuffer_descriptor_reports_first_ioctl_failure() {
        let file = File::open("/dev/null").unwrap();
        let error = OpenedFramebuffer::from_file(file).err().unwrap();
        assert!(format!("{error:#}").contains("MXCFB FBIOGET_FSCREENINFO"));
    }
}
