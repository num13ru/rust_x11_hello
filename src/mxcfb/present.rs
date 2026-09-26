//! Staged remote-frame presentation. Startup does not call this module yet.
//! A successful return means the kernel accepted an update request, not that
//! the physical panel completed it.

use std::fs::File;

use anyhow::{Context, Result, ensure};

use crate::display::RemoteFrame;
use crate::ui::screen::ScreenRect;

use super::controller;
use super::opened::OpenedFramebuffer;
use super::pixels::prepare_remote;
use super::update_abi::UpdateDataMtk;
use super::update_marker::UpdateMarkerSequence;
use super::update_request::next_full_gc16_remote_request;

pub(super) struct SubmittedRemoteUpdate {
    pub marker: u32,
    pub region: ScreenRect,
}

pub(super) fn present_remote(
    opened: &mut OpenedFramebuffer,
    markers: &mut UpdateMarkerSequence,
    frame: RemoteFrame<'_>,
) -> Result<SubmittedRemoteUpdate> {
    present_remote_with(opened, markers, frame, controller::submit_update)
}

fn present_remote_with(
    opened: &mut OpenedFramebuffer,
    markers: &mut UpdateMarkerSequence,
    frame: RemoteFrame<'_>,
    mut submit: impl FnMut(&File, &mut UpdateDataMtk, u32, u32) -> Result<()>,
) -> Result<SubmittedRemoteUpdate> {
    let info = &opened.info;
    // Finish conversion and all bounds checks before the first framebuffer
    // store. The update request is also validated before writing pixels.
    let prepared = prepare_remote(frame, info.screen, info.spec, info.mapping_len)
        .context("MXCFB prepare remote frame")?;
    let mut request = next_full_gc16_remote_request(
        info.screen,
        info.spec.visible_width,
        info.spec.visible_height,
        markers,
    )
    .context("MXCFB prepare remote update")?;
    let region = prepared.region();
    ensure!(
        request.update_region.left == u32::from(region.x)
            && request.update_region.top == u32::from(region.y)
            && request.update_region.width == u32::from(region.width)
            && request.update_region.height == u32::from(region.height),
        "MXCFB prepared pixels and update region differ"
    );

    opened
        .mapping
        .write_prepared(&prepared)
        .context("MXCFB write remote framebuffer rows")?;
    let marker = request.update_marker;
    submit(
        &opened.file,
        &mut request,
        info.spec.visible_width,
        info.spec.visible_height,
    )
    .context("MXCFB submit remote update")?;
    Ok(SubmittedRemoteUpdate { marker, region })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs::OpenOptions;

    use super::*;
    use crate::mxcfb::layout::FramebufferGeometry;
    use crate::mxcfb::mapped::WritableFramebuffer;
    use crate::mxcfb::metadata::FramebufferInfo;
    use crate::mxcfb::pixels::FramebufferSpec;
    use crate::ui::screen::ScreenLayout;

    fn fake_framebuffer() -> OpenedFramebuffer {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/zero")
            .unwrap();
        let geometry = FramebufferGeometry {
            xres: 9,
            yres: 73,
            xres_virtual: 9,
            yres_virtual: 73,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 8,
            line_length: 12,
            smem_len: 12 * 73,
        };
        // SAFETY: /dev/zero supports the validated 876-byte shared mapping
        // for this host-only test; the mapping lives with its descriptor.
        let mapping = unsafe { WritableFramebuffer::map_visible(&file, geometry) }.unwrap();
        let info = FramebufferInfo {
            geometry,
            spec: FramebufferSpec {
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
            },
            screen: ScreenLayout::new(9, 73).unwrap(),
            mapping_len: 12 * 73,
        };
        OpenedFramebuffer {
            mapping,
            file,
            info,
        }
    }

    #[test]
    fn submits_only_after_a_valid_remote_frame() {
        let mut opened = fake_framebuffer();
        let mut markers = UpdateMarkerSequence::for_process();
        let called = Cell::new(false);
        let frame = RemoteFrame::new(7, 9, 1, 2, &[0x80, 0x80], 0).unwrap();
        let submitted =
            present_remote_with(&mut opened, &mut markers, frame, |_, request, w, h| {
                called.set(true);
                assert_eq!((w, h), (9, 73));
                assert_eq!(request.update_region.width, 9);
                assert_eq!(request.update_region.height, 1);
                Ok(())
            })
            .unwrap();
        assert!(called.get());
        assert_eq!(submitted.marker, std::process::id().max(1));
        assert_eq!(submitted.region, opened.info.screen.remote_viewport);

        let bad = RemoteFrame::new(8, 8, 1, 1, &[0x80], 0).unwrap();
        called.set(false);
        assert!(
            present_remote_with(&mut opened, &mut markers, bad, |_, _, _, _| {
                called.set(true);
                Ok(())
            })
            .is_err()
        );
        assert!(!called.get());
    }

    #[test]
    fn kernel_rejection_is_not_reported_as_success() {
        let mut opened = fake_framebuffer();
        let mut markers = UpdateMarkerSequence::for_process();
        let frame = RemoteFrame::new(7, 9, 1, 2, &[0x80, 0x80], 0).unwrap();
        let error = present_remote(&mut opened, &mut markers, frame)
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("MXCFB_SEND_UPDATE_MTK"));
    }
}
