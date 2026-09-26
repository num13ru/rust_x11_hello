//! Standard Linux framebuffer information ABI, not the Kindle MXCFB update ABI.
//!
//! Field order and ioctl numbers are from `linux/fb.h` in the ARM Linux UAPI
//! headers installed in the repository's Docker builder. No update ioctl or
//! device-specific waveform value is defined here.

use std::ffi::{c_int, c_ulong};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

const FBIOGET_VSCREENINFO: c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: c_ulong = 0x4602;

unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FbBitfield {
    pub(super) offset: u32,
    pub(super) length: u32,
    pub(super) msb_right: u32,
}

#[repr(C)]
#[derive(Default)]
#[allow(dead_code)]
pub(super) struct FbFixScreeninfo {
    pub(super) id: [u8; 16],
    pub(super) smem_start: c_ulong,
    pub(super) smem_len: u32,
    pub(super) kind: u32,
    pub(super) type_aux: u32,
    pub(super) visual: u32,
    pub(super) xpanstep: u16,
    pub(super) ypanstep: u16,
    pub(super) ywrapstep: u16,
    pub(super) line_length: u32,
    pub(super) mmio_start: c_ulong,
    pub(super) mmio_len: u32,
    pub(super) accel: u32,
    pub(super) capabilities: u16,
    pub(super) reserved: [u16; 2],
}

#[repr(C)]
#[derive(Default)]
#[allow(dead_code)]
pub(super) struct FbVarScreeninfo {
    pub(super) xres: u32,
    pub(super) yres: u32,
    pub(super) xres_virtual: u32,
    pub(super) yres_virtual: u32,
    pub(super) xoffset: u32,
    pub(super) yoffset: u32,
    pub(super) bits_per_pixel: u32,
    pub(super) grayscale: u32,
    pub(super) red: FbBitfield,
    pub(super) green: FbBitfield,
    pub(super) blue: FbBitfield,
    pub(super) transp: FbBitfield,
    pub(super) nonstd: u32,
    pub(super) activate: u32,
    pub(super) height: u32,
    pub(super) width: u32,
    pub(super) accel_flags: u32,
    pub(super) pixclock: u32,
    pub(super) left_margin: u32,
    pub(super) right_margin: u32,
    pub(super) upper_margin: u32,
    pub(super) lower_margin: u32,
    pub(super) hsync_len: u32,
    pub(super) vsync_len: u32,
    pub(super) sync: u32,
    pub(super) vmode: u32,
    pub(super) rotate: u32,
    pub(super) colorspace: u32,
    pub(super) reserved: [u32; 4],
}

const _: () = {
    assert!(std::mem::size_of::<FbVarScreeninfo>() == 160);
    #[cfg(target_pointer_width = "32")]
    assert!(std::mem::size_of::<FbFixScreeninfo>() == 68);
    #[cfg(target_pointer_width = "64")]
    assert!(std::mem::size_of::<FbFixScreeninfo>() == 80);
};

pub(super) fn fixed_info(file: &File) -> io::Result<FbFixScreeninfo> {
    let mut info = FbFixScreeninfo::default();
    // SAFETY: `file` remains open, `info` is writable and has the Linux UAPI
    // layout checked above, and this request only reads framebuffer metadata.
    let result = unsafe { ioctl(file.as_raw_fd(), FBIOGET_FSCREENINFO, &mut info) };
    ioctl_result(result)?;
    Ok(info)
}

pub(super) fn variable_info(file: &File) -> io::Result<FbVarScreeninfo> {
    let mut info = FbVarScreeninfo::default();
    // SAFETY: `file` remains open, `info` is writable and has the Linux UAPI
    // layout checked above, and this request only reads framebuffer metadata.
    let result = unsafe { ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut info) };
    ioctl_result(result)?;
    Ok(info)
}

fn ioctl_result(result: c_int) -> io::Result<()> {
    match result {
        0 => Ok(()),
        -1 => Err(io::Error::last_os_error()),
        other => Err(io::Error::other(format!(
            "unexpected framebuffer ioctl return value {other}"
        ))),
    }
}
