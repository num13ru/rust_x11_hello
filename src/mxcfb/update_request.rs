//! Pure construction of the first conservative HWTCON remote update request.
//! No framebuffer memory or ioctl is accessed here.

use anyhow::Result;
#[cfg(test)]
use anyhow::ensure;

use crate::ui::screen::ScreenLayout;

use super::region::{remote_update_region, system_ui_update_region};
use super::update_abi::{UpdateDataMtk, UpdateRegion};
use super::update_marker::UpdateMarkerSequence;

// PW6-matched values from FBInk `eink/mtk-kindle.h` and its included
// `eink/mxcfb-kindle.h` at 886f25f13368859ad8a899b88d04c26e19cda32e.
// FBInk's `refresh_kindle_mtk` initializes the remaining fields to zero for
// this GC16 path. Kernel acceptance was observed on a PW6; panel completion
// timing and exact pixel behavior remain unverified.
const WAVEFORM_GC16: u32 = 2;
const WAVEFORM_DU: u32 = 1;
const UPDATE_MODE_FULL: u32 = 1;
const TEMP_USE_AMBIENT: i32 = 0x1000;

#[cfg(test)]
pub(super) fn full_gc16_remote_request(
    screen: ScreenLayout,
    visible_width: u32,
    visible_height: u32,
    marker: u32,
) -> Result<UpdateDataMtk> {
    ensure!(marker != 0, "HWTCON update marker must be nonzero");
    let update_region = remote_update_region(screen, visible_width, visible_height)?;
    Ok(full_gc16_request_for_region(update_region, marker))
}

/// Validate before consuming a process-local marker. A failed request does
/// not advance the sequence; no update is submitted here.
pub(super) fn next_full_gc16_remote_request(
    screen: ScreenLayout,
    visible_width: u32,
    visible_height: u32,
    markers: &mut UpdateMarkerSequence,
) -> Result<UpdateDataMtk> {
    let update_region = remote_update_region(screen, visible_width, visible_height)?;
    Ok(full_gc16_request_for_region(
        update_region,
        markers.next_marker(),
    ))
}

/// Same conservative GC16 policy, restricted to PaperPad's local Exit strip.
/// Validation happens before the process-local marker is consumed.
pub(super) fn next_full_gc16_system_ui_request(
    screen: ScreenLayout,
    visible_width: u32,
    visible_height: u32,
    markers: &mut UpdateMarkerSequence,
) -> Result<UpdateDataMtk> {
    let update_region = system_ui_update_region(screen, visible_width, visible_height)?;
    Ok(full_gc16_request_for_region(
        update_region,
        markers.next_marker(),
    ))
}

fn full_gc16_request_for_region(update_region: UpdateRegion, marker: u32) -> UpdateDataMtk {
    UpdateDataMtk {
        update_region,
        waveform_mode: WAVEFORM_GC16,
        update_mode: UPDATE_MODE_FULL,
        update_marker: marker,
        temp: TEMP_USE_AMBIENT,
        hist_bw_waveform_mode: WAVEFORM_DU,
        hist_gray_waveform_mode: WAVEFORM_GC16,
        ..UpdateDataMtk::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_ui_request_uses_shared_policy_and_marker_sequence() {
        let screen = ScreenLayout::new(1272, 1696).unwrap();
        let mut markers = UpdateMarkerSequence::for_process();
        assert!(next_full_gc16_system_ui_request(screen, 1271, 1696, &mut markers).is_err());
        let local = next_full_gc16_system_ui_request(screen, 1272, 1696, &mut markers).unwrap();
        let remote = next_full_gc16_remote_request(screen, 1272, 1696, &mut markers).unwrap();
        assert_eq!(local.update_marker, std::process::id().max(1));
        assert_eq!(
            remote.update_marker,
            local.update_marker.wrapping_add(1).max(1)
        );
        assert_eq!(
            (local.update_region.top, local.update_region.height),
            (1624, 72)
        );
        assert_eq!(
            (remote.update_region.top, remote.update_region.height),
            (0, 1624)
        );
        assert_eq!(local.waveform_mode, remote.waveform_mode);
        assert_eq!(local.update_mode, remote.update_mode);
    }

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

    #[test]
    fn sequenced_requests_advance_only_after_validation() {
        let screen = ScreenLayout::new(1272, 1696).unwrap();
        let mut markers = UpdateMarkerSequence::for_process();
        let first_marker = std::process::id().max(1);

        assert!(next_full_gc16_remote_request(screen, 1271, 1696, &mut markers).is_err());
        let first = next_full_gc16_remote_request(screen, 1272, 1696, &mut markers).unwrap();
        let second = next_full_gc16_remote_request(screen, 1272, 1696, &mut markers).unwrap();

        assert_eq!(first.update_marker, first_marker);
        assert_eq!(second.update_marker, first_marker.wrapping_add(1).max(1));
        assert_eq!(first.update_region, second.update_region);
    }
}
