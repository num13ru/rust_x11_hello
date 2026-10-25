//! Pure construction of validated HWTCON update requests.
//! No framebuffer memory or ioctl is accessed here.

use anyhow::{Result, ensure};

use crate::ui::screen::ScreenLayout;

use super::region::{remote_update_region, system_ui_update_region};
use super::update_abi::{UpdateDataMtk, UpdateRegion};
use super::update_marker::UpdateMarkerSequence;

// PW6-matched values from FBInk `eink/mtk-kindle.h` and its included
// `eink/mxcfb-kindle.h` at 886f25f13368859ad8a899b88d04c26e19cda32e.
// Only GC16 has device evidence in PaperPad. The faster modes are exposed only
// to the explicit benchmark and must not silently change normal presentation.
const WAVEFORM_DU: u32 = 1;
const WAVEFORM_GC16: u32 = 2;
const WAVEFORM_A2: u32 = 6;
const WAVEFORM_DU4: u32 = 7;
const UPDATE_MODE_PARTIAL: u32 = 0;
const UPDATE_MODE_FULL: u32 = 1;
const TEMP_USE_AMBIENT: i32 = 0x1000;
const EPDC_FLAG_FORCE_MONOCHROME: u32 = 0x02;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MxcfbWaveform {
    Gc16,
    Du,
    A2,
    Du4,
}

impl MxcfbWaveform {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            "gc16" => Some(Self::Gc16),
            "du" => Some(Self::Du),
            "a2" => Some(Self::A2),
            "du4" => Some(Self::Du4),
            _ => None,
        }
    }

    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Gc16 => "gc16",
            Self::Du => "du",
            Self::A2 => "a2",
            Self::Du4 => "du4",
        }
    }

    const fn mode(self) -> u32 {
        match self {
            Self::Gc16 => WAVEFORM_GC16,
            Self::Du => WAVEFORM_DU,
            Self::A2 => WAVEFORM_A2,
            Self::Du4 => WAVEFORM_DU4,
        }
    }

    const fn update_mode(self) -> u32 {
        match self {
            Self::Gc16 => UPDATE_MODE_FULL,
            Self::Du | Self::A2 | Self::Du4 => UPDATE_MODE_PARTIAL,
        }
    }

    const fn flags(self) -> u32 {
        match self {
            Self::A2 => EPDC_FLAG_FORCE_MONOCHROME,
            Self::Gc16 | Self::Du | Self::Du4 => 0,
        }
    }
}

#[cfg(test)]
pub(super) fn full_gc16_remote_request(
    screen: ScreenLayout,
    visible_width: u32,
    visible_height: u32,
    marker: u32,
) -> Result<UpdateDataMtk> {
    let update_region = remote_update_region(screen, visible_width, visible_height)?;
    request_for_region(update_region, marker, MxcfbWaveform::Gc16)
}

/// Validate the remote update region before advancing the process-local marker.
pub(super) fn next_remote_request(
    screen: ScreenLayout,
    visible_width: u32,
    visible_height: u32,
    markers: &mut UpdateMarkerSequence,
    waveform: MxcfbWaveform,
) -> Result<UpdateDataMtk> {
    let update_region = remote_update_region(screen, visible_width, visible_height)?;
    request_for_region(update_region, markers.next_marker(), waveform)
}

/// Preserve the established conservative policy for normal PaperPad frames.
pub(super) fn next_full_gc16_remote_request(
    screen: ScreenLayout,
    visible_width: u32,
    visible_height: u32,
    markers: &mut UpdateMarkerSequence,
) -> Result<UpdateDataMtk> {
    next_remote_request(
        screen,
        visible_width,
        visible_height,
        markers,
        MxcfbWaveform::Gc16,
    )
}

/// GC16 request for PaperPad's local system strip, sharing the process-local
/// marker sequence with remote updates.
pub(super) fn next_full_gc16_system_ui_request(
    screen: ScreenLayout,
    visible_width: u32,
    visible_height: u32,
    markers: &mut UpdateMarkerSequence,
) -> Result<UpdateDataMtk> {
    let update_region = system_ui_update_region(screen, visible_width, visible_height)?;
    request_for_region(update_region, markers.next_marker(), MxcfbWaveform::Gc16)
}

fn request_for_region(
    update_region: UpdateRegion,
    marker: u32,
    waveform: MxcfbWaveform,
) -> Result<UpdateDataMtk> {
    ensure!(marker != 0, "HWTCON update marker must be nonzero");
    Ok(UpdateDataMtk {
        update_region,
        waveform_mode: waveform.mode(),
        update_mode: waveform.update_mode(),
        update_marker: marker,
        temp: TEMP_USE_AMBIENT,
        flags: waveform.flags(),
        hist_bw_waveform_mode: WAVEFORM_DU,
        hist_gray_waveform_mode: WAVEFORM_GC16,
        ..UpdateDataMtk::default()
    })
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
        assert_eq!(request.waveform_mode, WAVEFORM_GC16);
        assert_eq!(request.update_mode, UPDATE_MODE_FULL);
        assert_eq!(request.update_marker, 17);
        assert_eq!(request.temp, TEMP_USE_AMBIENT);
        assert_eq!(request.hist_bw_waveform_mode, WAVEFORM_DU);
        assert_eq!(request.hist_gray_waveform_mode, WAVEFORM_GC16);
        assert_eq!(request.flags, 0);
        assert_eq!(request.dither_mode, 0);
        assert_eq!(request.quant_bit, 0);
        assert_eq!(request.alt_buffer_data, Default::default());
        assert_eq!(request.swipe_data, Default::default());
        assert_eq!(request.ts_pxp, 0);
        assert_eq!(request.ts_epdc, 0);
    }

    #[test]
    fn benchmark_waveforms_use_expected_mode_and_flags() {
        let region = UpdateRegion {
            top: 0,
            left: 0,
            width: 1,
            height: 1,
        };
        for (waveform, mode, update_mode, flags) in [
            (MxcfbWaveform::Gc16, WAVEFORM_GC16, UPDATE_MODE_FULL, 0),
            (MxcfbWaveform::Du, WAVEFORM_DU, UPDATE_MODE_PARTIAL, 0),
            (
                MxcfbWaveform::A2,
                WAVEFORM_A2,
                UPDATE_MODE_PARTIAL,
                EPDC_FLAG_FORCE_MONOCHROME,
            ),
            (MxcfbWaveform::Du4, WAVEFORM_DU4, UPDATE_MODE_PARTIAL, 0),
        ] {
            let request = request_for_region(region, 1, waveform).unwrap();
            assert_eq!(request.waveform_mode, mode);
            assert_eq!(request.update_mode, update_mode);
            assert_eq!(request.flags, flags);
            assert_eq!(MxcfbWaveform::from_name(waveform.name()), Some(waveform));
        }
        assert_eq!(MxcfbWaveform::from_name("auto"), None);
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
