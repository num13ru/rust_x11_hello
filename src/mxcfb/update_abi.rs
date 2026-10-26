//! HWTCON update ABI layouts used by the MXCFB display backend.
//!
//! `mxcfb_update_data_mtk` and `MXCFB_SEND_UPDATE_MTK` come from FBInk's
//! `eink/mtk-kindle.h` at commit 886f25f13368859ad8a899b88d04c26e19cda32e,
//! which states it was updated from the PW6 FW 5.17.1.0.4 kernel header.
//! `mxcfb_rect` and `mxcfb_alt_buffer_data` come from
//! that commit's included `eink/mxcfb-kindle.h`. These are source-derived
//! layouts. A PW6 accepted send-update requests using them, but acceptance
//! does not independently validate every field or prove panel completion.

use std::mem::{offset_of, size_of};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct UpdateRegion {
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct AltBufferData {
    pub phys_addr: u32,
    pub width: u32,
    pub height: u32,
    pub alt_update_region: UpdateRegion,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct SwipeData {
    pub direction: u32,
    pub steps: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct UpdateMarkerData {
    pub update_marker: u32,
    pub collision_test: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct UpdateDataMtk {
    pub update_region: UpdateRegion,
    pub waveform_mode: u32,
    pub update_mode: u32,
    pub update_marker: u32,
    pub temp: i32,
    pub flags: u32,
    pub dither_mode: i32,
    pub quant_bit: i32,
    pub alt_buffer_data: AltBufferData,
    pub swipe_data: SwipeData,
    pub hist_bw_waveform_mode: u32,
    pub hist_gray_waveform_mode: u32,
    pub ts_pxp: u32,
    pub ts_epdc: u32,
}

// Linux asm-generic _IOW('F', 0x2e, struct mxcfb_update_data_mtk).
pub(super) const SEND_UPDATE_MTK: u32 =
    (1u32 << 30) | ((size_of::<UpdateDataMtk>() as u32) << 16) | ((b'F' as u32) << 8) | 0x2e;

// Linux asm-generic _IOWR('F', 0x2f, struct mxcfb_update_marker_data).
// The PW6 driver reads only `update_marker`, waits up to five seconds, and
// returns zero for found, missing, and timed-out markers. Callers must retain
// elapsed-time diagnostics and must not present the return value as proof of
// visual completion.
pub(super) const WAIT_FOR_UPDATE_COMPLETE: u32 =
    (3u32 << 30) | ((size_of::<UpdateMarkerData>() as u32) << 16) | ((b'F' as u32) << 8) | 0x2f;

// Bellatrix4/PW6 extension from the pinned FBInk `mtk-kindle.h`. Although the
// ioctl encoding advertises one `u32`, the driver may write up to 64 markers
// to the caller-provided array and returns the number of bytes written.
#[cfg(target_os = "linux")]
#[allow(dead_code)] // Wait-any remains documented for diagnostics only.
pub(super) const MAX_PENDING_UPDATES: usize = 64;
#[allow(dead_code)] // Global marker consumption makes this unsuitable for timing.
pub(super) const WAIT_FOR_ANY_UPDATE_COMPLETE_MTK: u32 =
    (3u32 << 30) | ((size_of::<u32>() as u32) << 16) | ((b'F' as u32) << 8) | 0x37;
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub(super) const WAIT_FOR_ANY_UPDATE_COMPLETE_CHECK: u32 = u32::MAX;

const _: () = {
    assert!(size_of::<UpdateRegion>() == 16);
    assert!(size_of::<AltBufferData>() == 28);
    assert!(size_of::<SwipeData>() == 8);
    assert!(size_of::<UpdateMarkerData>() == 8);
    assert!(size_of::<UpdateDataMtk>() == 96);
    assert!(offset_of!(UpdateDataMtk, alt_buffer_data) == 44);
    assert!(offset_of!(UpdateDataMtk, swipe_data) == 72);
    assert!(offset_of!(UpdateDataMtk, ts_epdc) == 92);
    assert!(SEND_UPDATE_MTK == 0x4060_462e);
    assert!(WAIT_FOR_UPDATE_COMPLETE == 0xc008_462f);
    assert!(WAIT_FOR_ANY_UPDATE_COMPLETE_MTK == 0xc004_4637);
};
