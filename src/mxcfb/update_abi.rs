//! Staged HWTCON update ABI layout; no ioctl is called from this module yet.
//!
//! `mxcfb_update_data_mtk` and `MXCFB_SEND_UPDATE_MTK` come from FBInk's
//! `eink/mtk-kindle.h` at commit 886f25f13368859ad8a899b88d04c26e19cda32e,
//! which states it was updated from the PW6 FW 5.17.1.0.4 kernel header.
//! `mxcfb_rect`, `mxcfb_alt_buffer_data`, and the completion marker come from
//! that commit's included `eink/mxcfb-kindle.h`. These are source-derived
//! layouts, not yet verified by an update submission on this device.

use std::mem::{offset_of, size_of};

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(super) struct UpdateRegion {
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[allow(dead_code)]
pub(super) struct AltBufferData {
    pub phys_addr: u32,
    pub width: u32,
    pub height: u32,
    pub alt_update_region: UpdateRegion,
}

#[repr(C)]
#[allow(dead_code)]
pub(super) struct SwipeData {
    pub direction: u32,
    pub steps: u32,
}

#[repr(C)]
#[allow(dead_code)]
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

#[repr(C)]
#[allow(dead_code)]
pub(super) struct UpdateMarkerData {
    pub update_marker: u32,
    pub collision_test: u32,
}

// Linux asm-generic _IOW('F', 0x2e, struct mxcfb_update_data_mtk)
// and _IOWR('F', 0x2f, struct mxcfb_update_marker_data).
#[allow(dead_code)]
pub(super) const SEND_UPDATE_MTK: u32 =
    (1u32 << 30) | ((size_of::<UpdateDataMtk>() as u32) << 16) | ((b'F' as u32) << 8) | 0x2e;
#[allow(dead_code)]
pub(super) const WAIT_FOR_UPDATE_COMPLETE: u32 =
    (3u32 << 30) | ((size_of::<UpdateMarkerData>() as u32) << 16) | ((b'F' as u32) << 8) | 0x2f;

const _: () = {
    assert!(size_of::<UpdateRegion>() == 16);
    assert!(size_of::<AltBufferData>() == 28);
    assert!(size_of::<SwipeData>() == 8);
    assert!(size_of::<UpdateDataMtk>() == 96);
    assert!(size_of::<UpdateMarkerData>() == 8);
    assert!(offset_of!(UpdateDataMtk, alt_buffer_data) == 44);
    assert!(offset_of!(UpdateDataMtk, swipe_data) == 72);
    assert!(offset_of!(UpdateDataMtk, ts_epdc) == 92);
    assert!(SEND_UPDATE_MTK == 0x4060_462e);
    assert!(WAIT_FOR_UPDATE_COMPLETE == 0xc008_462f);
};
