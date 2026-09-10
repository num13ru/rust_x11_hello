//! PaperPad screen ownership and viewport geometry.

/// Height reserved for PaperPad-owned system UI at the bottom of the screen.
pub(crate) const SYSTEM_UI_HEIGHT: u16 = 72;

/// Signed X11 window-relative coordinate received from the device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalPoint {
    pub x: i16,
    pub y: i16,
}

/// Unsigned coordinate relative to the PaperSpoon-owned remote viewport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RemotePoint {
    pub(crate) x: u16,
    pub(crate) y: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScreenRect {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

impl ScreenRect {
    fn to_relative_point(self, physical: PhysicalPoint) -> Option<RemotePoint> {
        let x = i32::from(physical.x);
        let y = i32::from(physical.y);
        let left = i32::from(self.x);
        let top = i32::from(self.y);
        let right = left + i32::from(self.width);
        let bottom = top + i32::from(self.height);

        if x < left || y < top || x >= right || y >= bottom {
            return None;
        }

        Some(RemotePoint {
            x: (x - left) as u16,
            y: (y - top) as u16,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScreenLayout {
    physical_size: (u16, u16),
    pub(crate) remote_viewport: ScreenRect,
    pub(crate) system_ui_region: ScreenRect,
}

impl ScreenLayout {
    /// Partition a non-empty X11 window into remote and local ownership.
    ///
    /// Dimensions are capped at X11's signed-coordinate limit, matching the
    /// existing drawing and input geometry behavior. If the window is shorter
    /// than the full system strip, PaperPad owns all available rows and exposes
    /// an empty remote viewport.
    pub(crate) fn new(width: u16, height: u16) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }

        let width = width.min(i16::MAX as u16);
        let height = height.min(i16::MAX as u16);
        let system_height = height.min(SYSTEM_UI_HEIGHT);
        let remote_height = height - system_height;

        Some(Self {
            physical_size: (width, height),
            remote_viewport: ScreenRect {
                x: 0,
                y: 0,
                width,
                height: remote_height,
            },
            system_ui_region: ScreenRect {
                x: 0,
                y: remote_height,
                width,
                height: system_height,
            },
        })
    }

    pub(crate) fn physical_size(self) -> (u16, u16) {
        self.physical_size
    }

    pub(crate) fn physical_to_remote(self, physical: PhysicalPoint) -> Option<RemotePoint> {
        self.remote_viewport.to_relative_point(physical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portrait_screen_reserves_bottom_system_strip() {
        let layout = ScreenLayout::new(1272, 1696).expect("screen layout");

        assert_eq!(layout.physical_size(), (1272, 1696));
        assert_eq!(
            layout.remote_viewport,
            ScreenRect {
                x: 0,
                y: 0,
                width: 1272,
                height: 1624,
            }
        );
        assert_eq!(
            layout.system_ui_region,
            ScreenRect {
                x: 0,
                y: 1624,
                width: 1272,
                height: 72,
            }
        );
    }

    #[test]
    fn regions_cover_screen_without_overlap_or_gap() {
        for (width, height) in [
            (1, 1),
            (123, 71),
            (123, 72),
            (636, 848),
            (u16::MAX, u16::MAX),
        ] {
            let layout = ScreenLayout::new(width, height).expect("screen layout");
            let (physical_width, physical_height) = layout.physical_size();

            assert_eq!(layout.remote_viewport.width, physical_width);
            assert_eq!(layout.system_ui_region.width, physical_width);
            assert_eq!(layout.remote_viewport.y, 0);
            assert_eq!(layout.system_ui_region.y, layout.remote_viewport.height);
            assert_eq!(
                layout.remote_viewport.height + layout.system_ui_region.height,
                physical_height
            );
        }
    }

    #[test]
    fn undersized_screen_is_local_only_and_zero_extent_has_no_layout() {
        let layout = ScreenLayout::new(100, 40).expect("screen layout");
        assert_eq!(layout.remote_viewport.height, 0);
        assert_eq!(layout.system_ui_region.height, 40);

        assert_eq!(ScreenLayout::new(0, 40), None);
        assert_eq!(ScreenLayout::new(100, 0), None);
    }

    #[test]
    fn physical_to_remote_accepts_only_half_open_remote_viewport() {
        let layout = ScreenLayout::new(1272, 1696).expect("screen layout");

        assert_eq!(
            layout.physical_to_remote(PhysicalPoint { x: 0, y: 0 }),
            Some(RemotePoint { x: 0, y: 0 })
        );
        assert_eq!(
            layout.physical_to_remote(PhysicalPoint { x: 1271, y: 1623 }),
            Some(RemotePoint { x: 1271, y: 1623 })
        );
        for outside in [
            PhysicalPoint { x: -1, y: 0 },
            PhysicalPoint { x: 0, y: -1 },
            PhysicalPoint { x: 1272, y: 0 },
            PhysicalPoint { x: 0, y: 1624 },
        ] {
            assert_eq!(layout.physical_to_remote(outside), None);
        }
    }

    #[test]
    fn relative_mapping_subtracts_nonzero_viewport_origin() {
        let viewport = ScreenRect {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };

        assert_eq!(
            viewport.to_relative_point(PhysicalPoint { x: 12, y: 23 }),
            Some(RemotePoint { x: 2, y: 3 })
        );
    }
}
