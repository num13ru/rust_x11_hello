//! Pure visible-screen update-region validation, separate from HWTCON ioctls.

use anyhow::{Context, Result, ensure};

use crate::ui::screen::ScreenLayout;

use super::update_abi::UpdateRegion;

#[allow(dead_code)]
pub(super) fn checked_visible_region(
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    visible_width: u32,
    visible_height: u32,
) -> Result<UpdateRegion> {
    ensure!(
        visible_width > 0 && visible_height > 0,
        "empty visible screen"
    );
    ensure!(width > 0 && height > 0, "empty update region");
    let right = left
        .checked_add(width)
        .context("update region x overflow")?;
    let bottom = top
        .checked_add(height)
        .context("update region y overflow")?;
    ensure!(
        right <= visible_width && bottom <= visible_height,
        "update region exceeds visible screen"
    );
    Ok(UpdateRegion {
        top,
        left,
        width,
        height,
    })
}

/// The first MXCFB policy refreshes the entire remote viewport, never the
/// PaperPad-owned Exit strip. `visible_*` must come from framebuffer metadata.
#[allow(dead_code)]
pub(super) fn remote_update_region(
    screen: ScreenLayout,
    visible_width: u32,
    visible_height: u32,
) -> Result<UpdateRegion> {
    let (screen_width, screen_height) = screen.physical_size();
    ensure!(
        (u32::from(screen_width), u32::from(screen_height)) == (visible_width, visible_height),
        "framebuffer visible dimensions do not match PaperPad screen"
    );

    let remote = screen.remote_viewport;
    let remote_bottom = u32::from(remote.y)
        .checked_add(u32::from(remote.height))
        .context("remote viewport bottom overflow")?;
    ensure!(
        remote_bottom <= u32::from(screen.system_ui_region.y),
        "remote update region overlaps PaperPad system UI"
    );
    checked_visible_region(
        u32::from(remote.x),
        u32::from(remote.y),
        u32::from(remote.width),
        u32::from(remote.height),
        visible_width,
        visible_height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pw6_remote_update_stops_before_exit_strip() {
        let screen = ScreenLayout::new(1272, 1696).unwrap();
        assert_eq!(
            remote_update_region(screen, 1272, 1696).unwrap(),
            UpdateRegion {
                top: 0,
                left: 0,
                width: 1272,
                height: 1624,
            }
        );
        assert_eq!(screen.system_ui_region.y, 1624);
    }

    #[test]
    fn local_only_or_mismatched_screens_have_no_remote_update() {
        let local_only = ScreenLayout::new(100, 40).unwrap();
        assert!(remote_update_region(local_only, 100, 40).is_err());

        let screen = ScreenLayout::new(1272, 1696).unwrap();
        assert!(remote_update_region(screen, 1271, 1696).is_err());
        assert!(remote_update_region(screen, 1272, 1695).is_err());
    }

    #[test]
    fn visible_region_accepts_last_pixel_but_rejects_invalid_bounds() {
        assert_eq!(
            checked_visible_region(1271, 1695, 1, 1, 1272, 1696).unwrap(),
            UpdateRegion {
                top: 1695,
                left: 1271,
                width: 1,
                height: 1,
            }
        );
        for (left, top, width, height, visible_width, visible_height) in [
            (0, 0, 0, 1, 1272, 1696),
            (0, 0, 1, 0, 1272, 1696),
            (0, 0, 1, 1, 0, 1696),
            (0, 0, 1, 1, 1272, 0),
            (1272, 0, 1, 1, 1272, 1696),
            (0, 1696, 1, 1, 1272, 1696),
            (u32::MAX, 0, 2, 1, u32::MAX, 1696),
            (0, u32::MAX, 1, 2, 1272, u32::MAX),
        ] {
            assert!(
                checked_visible_region(left, top, width, height, visible_width, visible_height)
                    .is_err(),
                "accepted {left},{top} {width}x{height} in {visible_width}x{visible_height}"
            );
        }
    }
}
