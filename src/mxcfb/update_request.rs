//! Pure construction of the first conservative HWTCON remote update request.
//! No framebuffer memory or ioctl is accessed here.

use anyhow::{Result, ensure};

use crate::ui::screen::ScreenLayout;

use super::region::remote_update_region;
use super::update_abi::UpdateDataMtk;

// PW6-matched values from FBInk `eink/mtk-kindle.h` and its included
// `eink/mxcfb-kindle.h` at 886f25f13368859ad8a899b88d04c26e19cda32e.
// FBInk's `refresh_kindle_mtk` initializes the remaining fields to zero for
// this GC16 path. Physical behavior still needs a Kindle update test.
const WAVEFORM_GC16: u32 = 2;
const WAVEFORM_DU: u32 = 1;
const UPDATE_MODE_FULL: u32 = 1;
const TEMP_USE_AMBIENT: i32 = 0x1000;

#[allow(dead_code)]
pub(super) fn full_gc16_remote_request(
    screen: ScreenLayout,
    visible_width: u32,
    visible_height: u32,
    marker: u32,
) -> Result<UpdateDataMtk> {
    ensure!(marker != 0, "HWTCON update marker must be nonzero");
    let update_region = remote_update_region(screen, visible_width, visible_height)?;
    Ok(UpdateDataMtk {
        update_region,
        waveform_mode: WAVEFORM_GC16,
        update_mode: UPDATE_MODE_FULL,
        update_marker: marker,
        temp: TEMP_USE_AMBIENT,
        hist_bw_waveform_mode: WAVEFORM_DU,
        hist_gray_waveform_mode: WAVEFORM_GC16,
        ..UpdateDataMtk::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pw6_request_is_full_gc16_for_remote_viewport_only() {
        let screen = ScreenLayout::new(1272, 1696).unwrap();
        let request = full_gc16_remote_request(screen, 1272, 1696, 17).unwrap();
        assert_eq!(request.update_region.top, 0);
        assert_eq!(request.update_region.left, 0);
        assert_eq!(request.update_region.width, 1272);
        assert_eq!(request.update_region.height, 1624);
        assert_eq!(request.waveform_mode, 2);
        assert_eq!(request.update_mode, 1);
        assert_eq!(request.update_marker, 17);
        assert_eq!(request.temp, 0x1000);
        assert_eq!(request.hist_bw_waveform_mode, 1);
        assert_eq!(request.hist_gray_waveform_mode, 2);
        assert_eq!(request.flags, 0);
        assert_eq!(request.dither_mode, 0);
        assert_eq!(request.quant_bit, 0);
        assert_eq!(request.alt_buffer_data, Default::default());
        assert_eq!(request.swipe_data, Default::default());
        assert_eq!(request.ts_pxp, 0);
        assert_eq!(request.ts_epdc, 0);
    }

    #[test]
    fn zero_marker_and_invalid_viewports_are_rejected() {
        let screen = ScreenLayout::new(1272, 1696).unwrap();
        assert!(full_gc16_remote_request(screen, 1272, 1696, 0).is_err());
        assert!(full_gc16_remote_request(screen, 1271, 1696, 1).is_err());
        assert!(full_gc16_remote_request(screen, 1272, 1695, 1).is_err());

        let local_only = ScreenLayout::new(100, 40).unwrap();
        assert!(full_gc16_remote_request(local_only, 100, 40, 1).is_err());
    }
}
