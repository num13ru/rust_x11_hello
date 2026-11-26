use std::fmt;

use crate::framebuffer::PresentationBuffer;

pub const LETTERBOX_RGB: u32 = 0x0080_8080;
pub const PAN_STEP_SOURCE_PIXELS: i32 = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FitRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZoomLevel {
    X1,
    X2,
    X4,
}

impl ZoomLevel {
    pub const fn factor(self) -> u32 {
        match self {
            Self::X1 => 1,
            Self::X2 => 2,
            Self::X4 => 4,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::X1 => "1x",
            Self::X2 => "2x",
            Self::X4 => "4x",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PanOffset {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ViewMode {
    #[default]
    Fit,
    Zoom(ZoomLevel),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ViewportState {
    mode: ViewMode,
    pan: PanOffset,
}

impl ViewportState {
    pub fn mode(self) -> ViewMode {
        self.mode
    }

    pub fn pan(self) -> PanOffset {
        self.pan
    }

    pub fn select(
        &mut self,
        mode: ViewMode,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
    ) -> bool {
        let previous = *self;
        self.mode = mode;
        self.pan = centered_pan(
            mode,
            source_width,
            source_height,
            target_width,
            target_height,
        );
        *self != previous
    }

    pub fn center(
        &mut self,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
    ) -> bool {
        let pan = centered_pan(
            self.mode,
            source_width,
            source_height,
            target_width,
            target_height,
        );
        let changed = self.pan != pan;
        self.pan = pan;
        changed
    }

    pub fn clamp(
        &mut self,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
    ) -> bool {
        let pan = clamp_pan(
            self.mode,
            self.pan,
            source_width,
            source_height,
            target_width,
            target_height,
        );
        let changed = self.pan != pan;
        self.pan = pan;
        changed
    }

    pub fn pan_by(
        &mut self,
        delta_x: i32,
        delta_y: i32,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
    ) -> bool {
        let ViewMode::Zoom(zoom) = self.mode else {
            return false;
        };
        let maximum = maximum_pan(
            zoom,
            source_width,
            source_height,
            target_width,
            target_height,
        );
        let pan = PanOffset {
            x: add_signed_clamped(self.pan.x, delta_x, maximum.x),
            y: add_signed_clamped(self.pan.y, delta_y, maximum.y),
        };
        let changed = self.pan != pan;
        self.pan = pan;
        changed
    }

    pub fn label(self) -> String {
        match self.mode {
            ViewMode::Fit => "fit".to_string(),
            ViewMode::Zoom(zoom) => {
                format!("{} pan {},{}", zoom.label(), self.pan.x, self.pan.y)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZoomRect {
    pub target_x: u32,
    pub target_y: u32,
    pub width: u32,
    pub height: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub zoom: ZoomLevel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourcePoint {
    pub x: u16,
    pub y: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderError {
    TargetSizeOverflow { width: u32, height: u32 },
    BufferLength { expected: usize, actual: usize },
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TargetSizeOverflow { width, height } => {
                write!(
                    formatter,
                    "target pixel count overflows usize for {width}x{height}"
                )
            }
            Self::BufferLength { expected, actual } => write!(
                formatter,
                "target buffer length mismatch: expected {expected} pixels, got {actual}"
            ),
        }
    }
}

impl std::error::Error for RenderError {}

pub fn fit_rect(
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> Option<FitRect> {
    if source_width == 0 || source_height == 0 || target_width == 0 || target_height == 0 {
        return None;
    }

    let (width, height) = if u64::from(target_width) * u64::from(source_height)
        <= u64::from(target_height) * u64::from(source_width)
    {
        let height =
            (u64::from(source_height) * u64::from(target_width) / u64::from(source_width)) as u32;
        (target_width, height.max(1))
    } else {
        let width =
            (u64::from(source_width) * u64::from(target_height) / u64::from(source_height)) as u32;
        (width.max(1), target_height)
    };
    Some(FitRect {
        x: (target_width - width) / 2,
        y: (target_height - height) / 2,
        width,
        height,
    })
}

pub fn maximum_pan(
    zoom: ZoomLevel,
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> PanOffset {
    PanOffset {
        x: maximum_pan_axis(source_width, target_width, zoom.factor()),
        y: maximum_pan_axis(source_height, target_height, zoom.factor()),
    }
}

pub fn centered_pan(
    mode: ViewMode,
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> PanOffset {
    let ViewMode::Zoom(zoom) = mode else {
        return PanOffset::default();
    };
    let maximum = maximum_pan(
        zoom,
        source_width,
        source_height,
        target_width,
        target_height,
    );
    PanOffset {
        x: maximum.x / 2,
        y: maximum.y / 2,
    }
}

pub fn zoom_rect(
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    zoom: ZoomLevel,
    pan: PanOffset,
) -> Option<ZoomRect> {
    if source_width == 0 || source_height == 0 || target_width == 0 || target_height == 0 {
        return None;
    }
    let (target_x, width, source_x) = zoom_axis(source_width, target_width, zoom, pan.x);
    let (target_y, height, source_y) = zoom_axis(source_height, target_height, zoom, pan.y);
    Some(ZoomRect {
        target_x,
        target_y,
        width,
        height,
        source_x,
        source_y,
        zoom,
    })
}

pub fn map_target_to_source(
    state: ViewportState,
    source_width: u16,
    source_height: u16,
    target_width: u32,
    target_height: u32,
    target_x: f64,
    target_y: f64,
) -> Option<SourcePoint> {
    let target_x = target_pixel(target_x, target_width)?;
    let target_y = target_pixel(target_y, target_height)?;
    let source_width_u32 = u32::from(source_width);
    let source_height_u32 = u32::from(source_height);

    let (source_x, source_y) = match state.mode() {
        ViewMode::Fit => {
            let rect = fit_rect(
                source_width_u32,
                source_height_u32,
                target_width,
                target_height,
            )?;
            let relative_x = target_x.checked_sub(rect.x).filter(|x| *x < rect.width)?;
            let relative_y = target_y.checked_sub(rect.y).filter(|y| *y < rect.height)?;
            (
                u32::try_from(
                    u64::from(relative_x) * u64::from(source_width) / u64::from(rect.width),
                )
                .ok()?,
                u32::try_from(
                    u64::from(relative_y) * u64::from(source_height) / u64::from(rect.height),
                )
                .ok()?,
            )
        }
        ViewMode::Zoom(zoom) => {
            let rect = zoom_rect(
                source_width_u32,
                source_height_u32,
                target_width,
                target_height,
                zoom,
                state.pan(),
            )?;
            let relative_x = target_x
                .checked_sub(rect.target_x)
                .filter(|x| *x < rect.width)?;
            let relative_y = target_y
                .checked_sub(rect.target_y)
                .filter(|y| *y < rect.height)?;
            (
                rect.source_x + relative_x / zoom.factor(),
                rect.source_y + relative_y / zoom.factor(),
            )
        }
    };

    Some(SourcePoint {
        x: u16::try_from(source_x).ok()?,
        y: u16::try_from(source_y).ok()?,
    })
}

pub fn render_view(
    state: ViewportState,
    source: &PresentationBuffer,
    target_width: u32,
    target_height: u32,
    target: &mut [u32],
) -> Result<(), RenderError> {
    match state.mode() {
        ViewMode::Fit => render_fit(source, target_width, target_height, target).map(drop),
        ViewMode::Zoom(zoom) => render_zoom(
            source,
            target_width,
            target_height,
            zoom,
            state.pan(),
            target,
        )
        .map(drop),
    }
}

pub fn render_fit(
    source: &PresentationBuffer,
    target_width: u32,
    target_height: u32,
    target: &mut [u32],
) -> Result<Option<FitRect>, RenderError> {
    validate_target(target_width, target_height, target.len())?;
    target.fill(LETTERBOX_RGB);
    let Some(rect) = fit_rect(
        u32::from(source.width()),
        u32::from(source.height()),
        target_width,
        target_height,
    ) else {
        return Ok(None);
    };

    let source_width = usize::from(source.width());
    for target_y in 0..rect.height {
        let source_y =
            (u64::from(target_y) * u64::from(source.height()) / u64::from(rect.height)) as usize;
        let target_row = usize::try_from(rect.y + target_y).expect("target row fits usize")
            * usize::try_from(target_width).expect("target width fits usize");
        for target_x in 0..rect.width {
            let source_x =
                (u64::from(target_x) * u64::from(source.width()) / u64::from(rect.width)) as usize;
            let target_index =
                target_row + usize::try_from(rect.x + target_x).expect("target column fits usize");
            target[target_index] = source.pixels()[source_y * source_width + source_x];
        }
    }
    Ok(Some(rect))
}

pub fn render_zoom(
    source: &PresentationBuffer,
    target_width: u32,
    target_height: u32,
    zoom: ZoomLevel,
    pan: PanOffset,
    target: &mut [u32],
) -> Result<Option<ZoomRect>, RenderError> {
    validate_target(target_width, target_height, target.len())?;
    target.fill(LETTERBOX_RGB);
    let Some(rect) = zoom_rect(
        u32::from(source.width()),
        u32::from(source.height()),
        target_width,
        target_height,
        zoom,
        pan,
    ) else {
        return Ok(None);
    };

    let source_width = usize::from(source.width());
    let zoom = rect.zoom.factor();
    for target_y in 0..rect.height {
        let source_y =
            usize::try_from(rect.source_y + target_y / zoom).expect("source row fits usize");
        let target_row = usize::try_from(rect.target_y + target_y).expect("target row fits usize")
            * usize::try_from(target_width).expect("target width fits usize");
        for target_x in 0..rect.width {
            let source_x =
                usize::try_from(rect.source_x + target_x / zoom).expect("source column fits usize");
            let target_index = target_row
                + usize::try_from(rect.target_x + target_x).expect("target column fits usize");
            target[target_index] = source.pixels()[source_y * source_width + source_x];
        }
    }
    Ok(Some(rect))
}

fn validate_target(
    target_width: u32,
    target_height: u32,
    actual: usize,
) -> Result<(), RenderError> {
    let expected =
        usize::try_from(u64::from(target_width) * u64::from(target_height)).map_err(|_| {
            RenderError::TargetSizeOverflow {
                width: target_width,
                height: target_height,
            }
        })?;
    if actual != expected {
        return Err(RenderError::BufferLength { expected, actual });
    }
    Ok(())
}

fn target_pixel(value: f64, extent: u32) -> Option<u32> {
    if !value.is_finite() || value < 0.0 || value >= f64::from(extent) {
        return None;
    }
    Some(value.floor() as u32)
}

fn maximum_pan_axis(source: u32, target: u32, zoom: u32) -> u32 {
    if source == 0 || target == 0 {
        return 0;
    }
    let visible_source_pixels = target.div_ceil(zoom);
    source.saturating_sub(visible_source_pixels)
}

fn clamp_pan(
    mode: ViewMode,
    pan: PanOffset,
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> PanOffset {
    let ViewMode::Zoom(zoom) = mode else {
        return PanOffset::default();
    };
    let maximum = maximum_pan(
        zoom,
        source_width,
        source_height,
        target_width,
        target_height,
    );
    PanOffset {
        x: pan.x.min(maximum.x),
        y: pan.y.min(maximum.y),
    }
}

fn add_signed_clamped(value: u32, delta: i32, maximum: u32) -> u32 {
    if delta.is_negative() {
        value.saturating_sub(delta.unsigned_abs())
    } else {
        value.saturating_add(delta as u32).min(maximum)
    }
}

fn zoom_axis(source: u32, target: u32, zoom: ZoomLevel, pan: u32) -> (u32, u32, u32) {
    let scaled = u64::from(source) * u64::from(zoom.factor());
    if scaled <= u64::from(target) {
        let scaled = scaled as u32;
        ((target - scaled) / 2, scaled, 0)
    } else {
        (
            0,
            target,
            pan.min(maximum_pan_axis(source, target, zoom.factor())),
        )
    }
}

#[cfg(test)]
mod tests {
    use paper_protocol::{Frame, Gray8Frame};

    use crate::framebuffer::convert_frame;

    use super::*;

    fn presentation(width: u16, height: u16, pixels: Vec<u8>) -> PresentationBuffer {
        let frame = Gray8Frame::new(width, height, pixels).expect("valid Gray8 frame");
        convert_frame(&Frame::Gray8(frame)).expect("convert frame")
    }

    #[test]
    fn fit_preserves_aspect_ratio_and_centers_letterboxing() {
        assert_eq!(
            fit_rect(2, 4, 8, 4),
            Some(FitRect {
                x: 3,
                y: 0,
                width: 2,
                height: 4,
            })
        );
        assert_eq!(
            fit_rect(4, 2, 4, 8),
            Some(FitRect {
                x: 0,
                y: 3,
                width: 4,
                height: 2,
            })
        );
        assert_eq!(
            fit_rect(4, 2, 8, 4),
            Some(FitRect {
                x: 0,
                y: 0,
                width: 8,
                height: 4,
            })
        );
    }

    #[test]
    fn zero_extent_has_no_fit_rectangle() {
        assert_eq!(fit_rect(0, 2, 8, 8), None);
        assert_eq!(fit_rect(2, 0, 8, 8), None);
        assert_eq!(fit_rect(2, 2, 0, 8), None);
        assert_eq!(fit_rect(2, 2, 8, 0), None);
    }

    #[test]
    fn nearest_neighbor_upscale_repeats_exact_source_pixels() {
        let source = presentation(2, 2, vec![0, 64, 128, 255]);
        let mut target = vec![0; 16];
        assert_eq!(
            render_fit(&source, 4, 4, &mut target).expect("render fit"),
            Some(FitRect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            })
        );
        assert_eq!(
            target,
            vec![
                0x0000_0000,
                0x0000_0000,
                0x0040_4040,
                0x0040_4040,
                0x0000_0000,
                0x0000_0000,
                0x0040_4040,
                0x0040_4040,
                0x0080_8080,
                0x0080_8080,
                0x00ff_ffff,
                0x00ff_ffff,
                0x0080_8080,
                0x0080_8080,
                0x00ff_ffff,
                0x00ff_ffff,
            ]
        );
    }

    #[test]
    fn downscale_uses_predictable_floor_sampling() {
        let source = presentation(4, 1, vec![0, 64, 128, 255]);
        let mut target = vec![0; 2];
        render_fit(&source, 2, 1, &mut target).expect("render fit");
        assert_eq!(target, vec![0x0000_0000, 0x0080_8080]);
    }

    #[test]
    fn letterbox_pixels_remain_neutral() {
        let source = presentation(2, 4, vec![255; 8]);
        let mut target = vec![0; 8 * 4];
        render_fit(&source, 8, 4, &mut target).expect("render fit");
        for row in target.chunks_exact(8) {
            assert_eq!(
                row,
                &[
                    LETTERBOX_RGB,
                    LETTERBOX_RGB,
                    LETTERBOX_RGB,
                    0x00ff_ffff,
                    0x00ff_ffff,
                    LETTERBOX_RGB,
                    LETTERBOX_RGB,
                    LETTERBOX_RGB
                ]
            );
        }
    }

    #[test]
    fn one_to_one_zoom_centers_without_resampling() {
        let source = presentation(2, 2, vec![0, 64, 128, 255]);
        let mut target = vec![0; 4 * 4];
        assert_eq!(
            render_zoom(
                &source,
                4,
                4,
                ZoomLevel::X1,
                PanOffset::default(),
                &mut target,
            )
            .expect("render 1x"),
            Some(ZoomRect {
                target_x: 1,
                target_y: 1,
                width: 2,
                height: 2,
                source_x: 0,
                source_y: 0,
                zoom: ZoomLevel::X1,
            })
        );
        assert_eq!(
            target,
            vec![
                LETTERBOX_RGB,
                LETTERBOX_RGB,
                LETTERBOX_RGB,
                LETTERBOX_RGB,
                LETTERBOX_RGB,
                0x0000_0000,
                0x0040_4040,
                LETTERBOX_RGB,
                LETTERBOX_RGB,
                0x0080_8080,
                0x00ff_ffff,
                LETTERBOX_RGB,
                LETTERBOX_RGB,
                LETTERBOX_RGB,
                LETTERBOX_RGB,
                LETTERBOX_RGB,
            ]
        );
    }

    #[test]
    fn integer_zoom_repeats_each_source_pixel_exactly() {
        let source = presentation(2, 1, vec![0, 255]);
        let mut target = vec![0; 8];
        render_zoom(
            &source,
            8,
            1,
            ZoomLevel::X2,
            PanOffset::default(),
            &mut target,
        )
        .expect("render 2x");
        assert_eq!(
            target,
            vec![
                LETTERBOX_RGB,
                LETTERBOX_RGB,
                0x0000_0000,
                0x0000_0000,
                0x00ff_ffff,
                0x00ff_ffff,
                LETTERBOX_RGB,
                LETTERBOX_RGB,
            ]
        );
    }

    #[test]
    fn oversized_zoom_uses_clamped_source_pixel_pan() {
        let source = presentation(4, 1, vec![0, 64, 128, 255]);
        let mut target = vec![0; 2];
        assert_eq!(
            render_zoom(
                &source,
                2,
                1,
                ZoomLevel::X1,
                PanOffset { x: 99, y: 99 },
                &mut target,
            )
            .expect("render panned 1x"),
            Some(ZoomRect {
                target_x: 0,
                target_y: 0,
                width: 2,
                height: 1,
                source_x: 2,
                source_y: 0,
                zoom: ZoomLevel::X1,
            })
        );
        assert_eq!(target, vec![0x0080_8080, 0x00ff_ffff]);
    }

    #[test]
    fn viewport_state_centers_and_bounds_pan_after_resizes() {
        let mut state = ViewportState::default();
        assert!(state.select(ViewMode::Zoom(ZoomLevel::X2), 10, 8, 8, 8));
        assert_eq!(state.pan(), PanOffset { x: 3, y: 2 });
        assert!(state.pan_by(PAN_STEP_SOURCE_PIXELS, 0, 10, 8, 8, 8));
        assert_eq!(state.pan(), PanOffset { x: 6, y: 2 });
        assert!(!state.pan_by(PAN_STEP_SOURCE_PIXELS, 0, 10, 8, 8, 8));
        assert!(state.pan_by(-PAN_STEP_SOURCE_PIXELS, 0, 10, 8, 8, 8));
        assert_eq!(state.pan(), PanOffset { x: 0, y: 2 });

        assert!(state.clamp(10, 8, 20, 20));
        assert_eq!(state.pan(), PanOffset::default());
        assert!(!state.pan_by(1, 1, 10, 8, 20, 20));
    }

    #[test]
    fn fit_mode_ignores_pan_requests() {
        let mut state = ViewportState::default();
        assert!(!state.pan_by(32, 32, 100, 100, 10, 10));
        assert_eq!(state.pan(), PanOffset::default());
    }

    #[test]
    fn fit_pointer_mapping_matches_render_sampling_and_rejects_letterbox() {
        let state = ViewportState::default();
        assert_eq!(
            map_target_to_source(state, 4, 2, 8, 8, 0.0, 2.0),
            Some(SourcePoint { x: 0, y: 0 })
        );
        assert_eq!(
            map_target_to_source(state, 4, 2, 8, 8, 7.999, 5.999),
            Some(SourcePoint { x: 3, y: 1 })
        );
        assert_eq!(map_target_to_source(state, 4, 2, 8, 8, 0.0, 1.999), None);
        assert_eq!(map_target_to_source(state, 4, 2, 8, 8, 0.0, 6.0), None);
    }

    #[test]
    fn zoom_pointer_mapping_accounts_for_pan_and_centered_letterboxing() {
        let mut panned = ViewportState::default();
        assert!(panned.select(ViewMode::Zoom(ZoomLevel::X2), 10, 8, 8, 8));
        assert_eq!(
            map_target_to_source(panned, 10, 8, 8, 8, 0.0, 0.0),
            Some(SourcePoint { x: 3, y: 2 })
        );
        assert_eq!(
            map_target_to_source(panned, 10, 8, 8, 8, 7.999, 7.999),
            Some(SourcePoint { x: 6, y: 5 })
        );

        let mut centered = ViewportState::default();
        assert!(centered.select(ViewMode::Zoom(ZoomLevel::X2), 2, 2, 8, 8));
        assert_eq!(map_target_to_source(centered, 2, 2, 8, 8, 1.999, 2.0), None);
        assert_eq!(
            map_target_to_source(centered, 2, 2, 8, 8, 2.0, 2.0),
            Some(SourcePoint { x: 0, y: 0 })
        );
        assert_eq!(
            map_target_to_source(centered, 2, 2, 8, 8, 5.999, 5.999),
            Some(SourcePoint { x: 1, y: 1 })
        );
        assert_eq!(map_target_to_source(centered, 2, 2, 8, 8, 6.0, 2.0), None);
    }

    #[test]
    fn pointer_mapping_uses_physical_target_pixels_for_high_dpi_windows() {
        assert_eq!(
            map_target_to_source(ViewportState::default(), 100, 50, 400, 200, 200.0, 100.0,),
            Some(SourcePoint { x: 50, y: 25 })
        );
    }

    #[test]
    fn pointer_mapping_rejects_invalid_coordinates_and_zero_extents() {
        let state = ViewportState::default();
        assert_eq!(map_target_to_source(state, 4, 4, 4, 4, -0.1, 0.0), None);
        assert_eq!(map_target_to_source(state, 4, 4, 4, 4, f64::NAN, 0.0), None);
        assert_eq!(map_target_to_source(state, 4, 4, 4, 4, 4.0, 0.0), None);
        assert_eq!(map_target_to_source(state, 0, 4, 4, 4, 0.0, 0.0), None);
        assert_eq!(map_target_to_source(state, 4, 4, 0, 4, 0.0, 0.0), None);
    }

    #[test]
    fn target_buffer_length_is_validated_before_drawing() {
        let source = presentation(1, 1, vec![255]);
        let mut target = vec![0xdead_beef; 3];
        let error = render_fit(&source, 2, 2, &mut target).expect_err("short buffer must fail");
        assert_eq!(
            error,
            RenderError::BufferLength {
                expected: 4,
                actual: 3,
            }
        );
        assert_eq!(target, vec![0xdead_beef; 3]);
    }
}
