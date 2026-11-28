//! Thin HWTCON update submission. A PW6 run accepted requests and displayed
//! PaperPad; exact panel-update completion behavior remains unverified.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

use anyhow::{Context, Result, bail, ensure};

use super::region::checked_visible_region;
use super::update_abi::{
    SEND_UPDATE_MTK, UpdateDataMtk, UpdateMarkerData, WAIT_FOR_UPDATE_COMPLETE,
};

/// Submit an already prepared update for a kernel-reported visible screen.
/// Kernel acceptance does not establish that the panel completed the update.
pub(super) fn submit_update(
    file: &File,
    request: &mut UpdateDataMtk,
    visible_width: u32,
    visible_height: u32,
) -> Result<()> {
    ensure!(
        request.update_marker != 0,
        "HWTCON update marker must be nonzero"
    );
    let region = request.update_region;
    checked_visible_region(
        region.left,
        region.top,
        region.width,
        region.height,
        visible_width,
        visible_height,
    )
    .context("HWTCON send-update region")?;

    // SAFETY: `file` stays open during ioctl and `request` is a stable, mutable
    // repr(C) buffer with the compile-checked HWTCON layout. The ioctl request
    // is firmware-attributed, not yet independently kernel-source verified.
    // The ARM libc request cast preserves its 32-bit ioctl bit pattern.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), SEND_UPDATE_MTK as _, request) };
    match result {
        0 => Ok(()),
        -1 => Err(io::Error::last_os_error()).context("MXCFB_SEND_UPDATE_MTK"),
        other => bail!("unexpected MXCFB_SEND_UPDATE_MTK return value {other}"),
    }
}

/// Wait for one process-owned marker using the firmware's marker-specific
/// path. The PW6 driver returns zero whether the marker completed, was absent,
/// or timed out, so callers must classify the elapsed time conservatively.
pub(super) fn wait_for_update_complete(file: &File, marker: u32) -> Result<()> {
    ensure!(marker != 0, "MXCFB completion marker must be nonzero");
    let mut marker_data = UpdateMarkerData {
        update_marker: marker,
        collision_test: 0,
    };

    // SAFETY: `file` remains open and `marker_data` is a stable, mutable
    // repr(C) buffer matching the source-verified 8-byte PW6 ABI.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            WAIT_FOR_UPDATE_COMPLETE as _,
            &mut marker_data,
        )
    };
    match result {
        0 => Ok(()),
        -1 => Err(io::Error::last_os_error()).context("MXCFB_WAIT_FOR_UPDATE_COMPLETE"),
        other => bail!("unexpected MXCFB_WAIT_FOR_UPDATE_COMPLETE return value {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::update_abi::UpdateRegion;
    use super::*;

    fn request() -> UpdateDataMtk {
        UpdateDataMtk {
            update_region: UpdateRegion {
                top: 0,
                left: 0,
                width: 1,
                height: 1,
            },
            update_marker: 1,
            ..UpdateDataMtk::default()
        }
    }

    #[test]
    fn rejects_invalid_requests_before_ioctl() {
        let file = File::open("/dev/null").unwrap();
        let mut update = request();
        update.update_marker = 0;
        assert!(
            submit_update(&file, &mut update, 1, 1)
                .unwrap_err()
                .to_string()
                .contains("marker")
        );

        update.update_marker = 1;
        update.update_region.left = 1;
        assert!(
            submit_update(&file, &mut update, 1, 1)
                .unwrap_err()
                .to_string()
                .contains("region")
        );
    }

    #[test]
    fn propagates_kernel_rejection() {
        let file = File::open("/dev/null").unwrap();
        let error = submit_update(&file, &mut request(), 1, 1).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<io::Error>()
                .and_then(io::Error::raw_os_error),
            Some(libc::ENOTTY)
        );
    }

    #[test]
    fn completion_ioctls_preserve_errno() {
        let file = File::open("/dev/null").unwrap();
        let error = wait_for_update_complete(&file, 1).unwrap_err();
        let source = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<io::Error>())
            .expect("an ioctl errno");
        assert_eq!(source.raw_os_error(), Some(libc::ENOTTY));
    }
}
