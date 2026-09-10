//! PaperPad screen ownership and viewport geometry.

/// Height reserved for PaperPad-owned system UI at the bottom of the screen.
pub(crate) const SYSTEM_UI_HEIGHT: u16 = 72;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScreenRect {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
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
}
