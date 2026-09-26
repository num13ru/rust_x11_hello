//! ABI reference:
//!   NiLuJe/FBInk `eink/mtk-kindle.h`
//!   commit 886f25f13368859ad8a899b88d04c26e19cda32e
//!
//! FBInk states that these HWTCON definitions were updated from the
//! Paperwhite 6 firmware 5.17.1.0.4 kernel `linux/hwtcon_ioctl_cmd.h`.
//!
//! This module contains only the minimal userspace ABI definitions needed
//! by PaperPad; no FBInk rendering or device-handling implementation is used.
//!
//! The original Kindle kernel header has not yet been independently verified.
//! The read-only panel-info query was accepted on the project device; no
//! update ioctl has been tested.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

#[repr(C)]
struct PanelInfo {
    wf_file_name: [libc::c_char; 100],
    vcom_value: libc::c_int,
    temp: libc::c_int,
    temp_zone: libc::c_int,
}

impl Default for PanelInfo {
    fn default() -> Self {
        Self {
            wf_file_name: [0; 100],
            vcom_value: 0,
            temp: 0,
            temp_zone: 0,
        }
    }
}

// Linux asm-generic _IOR('F', 0x130, struct mxcfb_panel_info). The 0x130
// argument deliberately extends into the encoded type field, as in the
// firmware-matched header; do not truncate it to the low eight bits.
const GET_PANEL_INFO_MTK: u32 =
    (2u32 << 30) | ((std::mem::size_of::<PanelInfo>() as u32) << 16) | ((b'F' as u32) << 8) | 0x130;

const _: () = assert!(std::mem::size_of::<PanelInfo>() == 112);
const _: () = assert!(GET_PANEL_INFO_MTK == 0x8070_4730);

pub(super) fn query_panel_info(file: &File) -> io::Result<()> {
    let mut info = PanelInfo::default();
    // SAFETY: `file` remains open and `info` is a writable, correctly sized
    // `repr(C)` buffer for the read-only GET_PANEL_INFO_MTK request. Its contents
    // are discarded rather than interpreted until this ABI is device-verified.
    // libc uses a signed 32-bit request on this ARM musl target. Casting at
    // the call boundary preserves the Linux ioctl request's 32-bit bit pattern.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), GET_PANEL_INFO_MTK as _, &mut info) };
    match result {
        0 => Ok(()),
        -1 => Err(io::Error::last_os_error()),
        other => Err(io::Error::other(format!(
            "unexpected GET_PANEL_INFO_MTK return value {other}"
        ))),
    }
}
