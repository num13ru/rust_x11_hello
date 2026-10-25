//! Thin HWTCON update submission. A PW6 run accepted requests and displayed
//! PaperPad; exact panel-update completion behavior remains unverified.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

use anyhow::{Context, Result, bail, ensure};

use super::region::checked_visible_region;
use super::update_abi::{
    MAX_PENDING_UPDATES, SEND_UPDATE_MTK, UpdateDataMtk, WAIT_FOR_ANY_UPDATE_COMPLETE_CHECK,
    WAIT_FOR_ANY_UPDATE_COMPLETE_MTK,
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

/// Confirm that the PW6 wait-for-any-completion extension is accepted before
/// a benchmark changes framebuffer pixels. A nonnegative feature value means
/// the ioctl exists; its REAGL feature bits are logged but otherwise unused.
pub(super) fn check_completion_wait_support(file: &File) -> Result<i32> {
    let mut markers = [0u32; MAX_PENDING_UPDATES];
    markers[0] = WAIT_FOR_ANY_UPDATE_COMPLETE_CHECK;

    // SAFETY: the pinned PW6 ABI documents a caller-owned array of 64 `u32`s.
    // `file` and the array remain live for the duration of the ioctl.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            WAIT_FOR_ANY_UPDATE_COMPLETE_MTK as _,
            markers.as_mut_ptr(),
        )
    };
    match result {
        -1 => Err(io::Error::last_os_error())
            .context("MXCFB_WAIT_FOR_ANY_UPDATE_COMPLETE_MTK feature check"),
        feature if feature >= 0 => Ok(feature),
        other => bail!(
            "unexpected MXCFB_WAIT_FOR_ANY_UPDATE_COMPLETE_MTK feature-check return value {other}"
        ),
    }
}

/// Block until the driver reports one or more completed update markers, or
/// until its 2-second FW 5.17.1.0.4 timeout returns zero markers. A zero-byte
/// result is one timed-out wait, not proof that the submitted update failed;
/// callers decide whether and how many times to retry.
pub(super) fn wait_for_any_update_complete(file: &File) -> Result<Vec<u32>> {
    let mut markers = [0u32; MAX_PENDING_UPDATES];

    // SAFETY: see `check_completion_wait_support`. The kernel return value is
    // decoded as a byte count before any array element is trusted.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            WAIT_FOR_ANY_UPDATE_COMPLETE_MTK as _,
            markers.as_mut_ptr(),
        )
    };
    match result {
        -1 => Err(io::Error::last_os_error()).context("MXCFB_WAIT_FOR_ANY_UPDATE_COMPLETE_MTK"),
        bytes if bytes >= 0 => decode_completed_markers(bytes as usize, &markers),
        other => bail!("unexpected MXCFB_WAIT_FOR_ANY_UPDATE_COMPLETE_MTK return value {other}"),
    }
}

fn decode_completed_markers(
    bytes: usize,
    markers: &[u32; MAX_PENDING_UPDATES],
) -> Result<Vec<u32>> {
    ensure!(
        bytes <= std::mem::size_of_val(markers),
        "completion marker byte count {bytes} exceeds buffer"
    );
    ensure!(
        bytes % std::mem::size_of::<u32>() == 0,
        "completion marker byte count {bytes} is not u32-aligned"
    );

    let count = bytes / std::mem::size_of::<u32>();
    let completed = markers[..count].to_vec();
    ensure!(
        completed.iter().all(|&marker| marker != 0),
        "driver returned zero completion marker"
    );
    Ok(completed)
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
    fn completion_marker_byte_count_is_validated() {
        let mut markers = [0u32; MAX_PENDING_UPDATES];
        markers[0] = 11;
        markers[1] = 12;

        assert_eq!(
            decode_completed_markers(0, &markers).unwrap(),
            Vec::<u32>::new()
        );
        assert_eq!(decode_completed_markers(8, &markers).unwrap(), vec![11, 12]);
        assert!(decode_completed_markers(3, &markers).is_err());
        assert!(decode_completed_markers(std::mem::size_of_val(&markers) + 4, &markers).is_err());

        markers[1] = 0;
        assert!(decode_completed_markers(8, &markers).is_err());
    }

    #[test]
    fn completion_ioctls_preserve_errno() {
        let file = File::open("/dev/null").unwrap();
        for error in [
            check_completion_wait_support(&file).unwrap_err(),
            wait_for_any_update_complete(&file).unwrap_err(),
        ] {
            let source = error
                .chain()
                .find_map(|cause| cause.downcast_ref::<io::Error>())
                .expect("io error source");
            assert_eq!(source.raw_os_error(), Some(libc::ENOTTY));
        }
    }
}
