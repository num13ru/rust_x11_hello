//! Remote and local-system-UI presentation for the MXCFB backend.
//! A successful return means the kernel accepted an update request, not that
//! the physical panel completed it.

use std::fs::File;

use anyhow::{Context, Result, ensure};

use crate::display::RemoteFrame;
use crate::ui::screen::ScreenRect;

use super::controller;
use super::opened::OpenedFramebuffer;
use super::pixels::prepare_remote;
use super::system_pixels::rasterize_system_ui;
use super::update_abi::UpdateDataMtk;
use super::update_marker::UpdateMarkerSequence;
use super::update_request::{next_full_gc16_remote_request, next_full_gc16_system_ui_request};

pub(super) struct SubmittedUpdate {
    pub marker: u32,
    pub region: ScreenRect,
}

pub(super) fn present_remote(
    opened: &mut OpenedFramebuffer,
    markers: &mut UpdateMarkerSequence,
    frame: RemoteFrame<'_>,
) -> Result<SubmittedUpdate> {
    present_remote_with(opened, markers, frame, controller::submit_update)
}

pub(super) fn present_remote_with(
    opened: &mut OpenedFramebuffer,
    markers: &mut UpdateMarkerSequence,
    frame: RemoteFrame<'_>,
    mut submit: impl FnMut(&File, &mut UpdateDataMtk, u32, u32) -> Result<()>,
) -> Result<SubmittedUpdate> {
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
    ensure_matching_update_region(region, &request)?;

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
    Ok(SubmittedUpdate { marker, region })
}

pub(super) fn present_system_ui(
    opened: &mut OpenedFramebuffer,
    markers: &mut UpdateMarkerSequence,
) -> Result<SubmittedUpdate> {
    present_system_ui_with(opened, markers, controller::submit_update)
}

pub(super) fn present_system_ui_with(
    opened: &mut OpenedFramebuffer,
    markers: &mut UpdateMarkerSequence,
    mut submit: impl FnMut(&File, &mut UpdateDataMtk, u32, u32) -> Result<()>,
) -> Result<SubmittedUpdate> {
    let info = &opened.info;
    let pixels = rasterize_system_ui(info.screen).context("MXCFB prepare local Exit pixels")?;
    let mut request = next_full_gc16_system_ui_request(
        info.screen,
        info.spec.visible_width,
        info.spec.visible_height,
        markers,
    )
    .context("MXCFB prepare local Exit update")?;
    ensure_matching_update_region(pixels.region, &request)?;

    opened
        .mapping
        .write_system_ui(&pixels, info.spec)
        .context("MXCFB write local Exit framebuffer rows")?;
    let marker = request.update_marker;
    submit(
        &opened.file,
        &mut request,
        info.spec.visible_width,
        info.spec.visible_height,
    )
    .context("MXCFB submit local Exit update")?;
    Ok(SubmittedUpdate {
        marker,
        region: pixels.region,
    })
}

fn ensure_matching_update_region(region: ScreenRect, request: &UpdateDataMtk) -> Result<()> {
    ensure!(
        request.update_region.left == u32::from(region.x)
            && request.update_region.top == u32::from(region.y)
            && request.update_region.width == u32::from(region.width)
            && request.update_region.height == u32::from(region.height),
        "MXCFB prepared pixels and update region differ"
    );
    Ok(())
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

    fn fake_framebuffer(width: u32, line_length: u32) -> OpenedFramebuffer {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/zero")
            .unwrap();
        let geometry = FramebufferGeometry {
            xres: width,
            yres: 73,
            xres_virtual: width,
            yres_virtual: 73,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 8,
            line_length,
            smem_len: line_length * 73,
        };
        // SAFETY: /dev/zero supports the validated shared mapping
        // for this host-only test; the mapping lives with its descriptor.
        let mapping = unsafe { WritableFramebuffer::map_visible(&file, geometry) }.unwrap();
        let info = FramebufferInfo {
            geometry,
            spec: FramebufferSpec {
                visible_width: width,
                visible_height: 73,
                virtual_width: width,
                virtual_height: 73,
                xoffset: 0,
                yoffset: 0,
                line_length: usize::try_from(line_length).unwrap(),
                memory_len: usize::try_from(line_length * 73).unwrap(),
                kind: 0,
                visual: 1,
                bits_per_pixel: 8,
                grayscale: 1,
                nonstd: 0,
            },
            screen: ScreenLayout::new(u16::try_from(width).unwrap(), 73).unwrap(),
            mapping_len: usize::try_from(line_length * 73).unwrap(),
        };
        OpenedFramebuffer {
            mapping,
            file,
            info,
        }
    }

    #[test]
    fn submits_only_after_a_valid_remote_frame() {
        let mut opened = fake_framebuffer(9, 12);
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
        let mut opened = fake_framebuffer(9, 12);
        let mut markers = UpdateMarkerSequence::for_process();
        let frame = RemoteFrame::new(7, 9, 1, 2, &[0x80, 0x80], 0).unwrap();
        let error = present_remote(&mut opened, &mut markers, frame)
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("MXCFB_SEND_UPDATE_MTK"));
    }

    #[test]
    fn local_exit_submits_only_its_strip_after_validation() {
        let mut opened = fake_framebuffer(64, 68);
        let mut markers = UpdateMarkerSequence::for_process();
        let called = Cell::new(false);
        let submitted = present_system_ui_with(&mut opened, &mut markers, |_, request, w, h| {
            called.set(true);
            assert_eq!((w, h), (64, 73));
            assert_eq!(
                (request.update_region.top, request.update_region.height),
                (1, 72)
            );
            assert_eq!(
                (request.update_region.left, request.update_region.width),
                (0, 64)
            );
            Ok(())
        })
        .unwrap();
        assert!(called.get());
        assert_eq!(submitted.marker, std::process::id().max(1));
        assert_eq!(submitted.region, opened.info.screen.system_ui_region);

        opened.info.spec.visual = 0;
        called.set(false);
        assert!(
            present_system_ui_with(&mut opened, &mut markers, |_, _, _, _| {
                called.set(true);
                Ok(())
            })
            .is_err()
        );
        assert!(!called.get());
    }

    #[test]
    fn local_exit_kernel_rejection_is_not_reported_as_success() {
        let mut opened = fake_framebuffer(64, 68);
        let mut markers = UpdateMarkerSequence::for_process();
        let error = present_system_ui(&mut opened, &mut markers).err().unwrap();
        assert!(format!("{error:#}").contains("MXCFB_SEND_UPDATE_MTK"));
    }
}
