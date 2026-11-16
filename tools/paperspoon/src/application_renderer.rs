//! Pure composition of host application state into a remote Mono1 frame.

use crate::application_ui::{ApplicationLayout, ApplicationUi, TextRun, VerticalTextPlacement};
use crate::mono_renderer::{MONO_TEXT_THRESHOLD, MonoCanvas};
use crate::text_renderer::{TextMeasurement, TextRect, TextRenderer, TextRequest};
use paper_protocol::{
    Mono1Frame, Mono1Pixel, V2_FRAME_PREFIX_LEN, V2_MAX_PAYLOAD_LEN, mono1_payload_len,
};

/// Long-lived application compositor. The text engine and glyph cache are
/// intentionally reused across frames.
#[derive(Debug)]
pub struct ApplicationRenderer {
    text: TextRenderer,
}

impl ApplicationRenderer {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            text: TextRenderer::new()
                .map_err(|error| format!("failed to initialize application text: {error}"))?,
        })
    }

    /// Enforce the wire payload cap before allocating candidate pixels.
    pub fn render_bounded(
        &mut self,
        ui: &ApplicationUi,
        width: u16,
        height: u16,
    ) -> Result<Mono1Frame, String> {
        validate_payload_size(width, height)?;
        self.render(ui, width, height)
    }

    /// Render one complete PaperSpoon-owned application viewport.
    pub fn render(
        &mut self,
        ui: &ApplicationUi,
        width: u16,
        height: u16,
    ) -> Result<Mono1Frame, String> {
        let layout = ui
            .layout(width, height)
            .ok_or_else(|| format!("application viewport must be nonzero, got {width}x{height}"))?;
        let mut canvas = MonoCanvas::new(width, height)
            .map_err(|error| format!("failed to create application canvas: {error}"))?;
        self.draw_layout(&mut canvas, &layout)?;
        canvas
            .into_frame()
            .map_err(|error| format!("failed to finalize application frame: {error}"))
    }

    fn draw_layout(
        &mut self,
        canvas: &mut MonoCanvas,
        layout: &ApplicationLayout<'_>,
    ) -> Result<(), String> {
        self.draw_text(canvas, layout, layout.title, Mono1Pixel::Black)?;
        for button in &layout.buttons {
            let label_color = if button.pressed {
                canvas.fill_rect(
                    button.bounds.x,
                    button.bounds.y,
                    button.bounds.width,
                    button.bounds.height,
                    Mono1Pixel::Black,
                );
                Mono1Pixel::White
            } else {
                canvas.outline_rect(
                    button.bounds.x,
                    button.bounds.y,
                    button.bounds.width,
                    button.bounds.height,
                    Mono1Pixel::Black,
                );
                Mono1Pixel::Black
            };
            self.draw_text(canvas, layout, button.label, label_color)?;
        }
        if let Some(status) = layout.status {
            self.draw_text(canvas, layout, status, Mono1Pixel::Black)?;
        }
        Ok(())
    }

    fn draw_text(
        &mut self,
        canvas: &mut MonoCanvas,
        layout: &ApplicationLayout<'_>,
        run: TextRun<'_>,
        color: Mono1Pixel,
    ) -> Result<(), String> {
        let measurement = self
            .text
            .measure(run.role, run.text, run.area.width, run.overflow)
            .map_err(|error| format!("failed to measure {:?} text: {error}", run.role))?;
        let baseline_y = resolve_baseline(run, &measurement)?;
        let raster = self
            .text
            .rasterize(
                TextRequest {
                    role: run.role,
                    text: run.text,
                    area: TextRect {
                        x: i32::from(run.area.x),
                        y: i32::from(run.area.y),
                        width: run.area.width,
                        height: run.area.height,
                    },
                    baseline_y,
                    alignment: run.alignment,
                    overflow: run.overflow,
                },
                layout.width,
                layout.height,
            )
            .map_err(|error| format!("failed to rasterize {:?} text: {error}", run.role))?;
        canvas.draw_coverage(&raster.mask, MONO_TEXT_THRESHOLD, color);
        Ok(())
    }

    #[cfg(test)]
    fn cached_glyph_count(&self) -> usize {
        self.text.cached_glyph_count()
    }
}

/// One-shot convenience for callers that do not own a renderer. Production
/// frame sending uses [`ApplicationRenderer`] directly so caches survive.
pub fn render_application_bounded(
    ui: &ApplicationUi,
    width: u16,
    height: u16,
) -> Result<Mono1Frame, String> {
    validate_payload_size(width, height)?;
    ApplicationRenderer::new()?.render(ui, width, height)
}

/// One-shot unbounded convenience used by focused host tests.
pub fn render_application(
    ui: &ApplicationUi,
    width: u16,
    height: u16,
) -> Result<Mono1Frame, String> {
    ApplicationRenderer::new()?.render(ui, width, height)
}

fn validate_payload_size(width: u16, height: u16) -> Result<(), String> {
    let pixel_bytes = mono1_payload_len(width, height)
        .ok_or_else(|| "application frame dimensions overflow payload size".to_string())?;
    let payload_bytes = V2_FRAME_PREFIX_LEN
        .checked_add(pixel_bytes)
        .ok_or_else(|| "application frame payload size overflow".to_string())?;
    if payload_bytes > V2_MAX_PAYLOAD_LEN {
        return Err(format!(
            "application frame payload {payload_bytes} bytes; maximum is {V2_MAX_PAYLOAD_LEN}"
        ));
    }
    Ok(())
}

fn resolve_baseline(run: TextRun<'_>, measurement: &TextMeasurement) -> Result<i32, String> {
    match run.vertical {
        VerticalTextPlacement::Baseline(baseline_y) => Ok(i32::from(baseline_y)),
        VerticalTextPlacement::Center => {
            let area_top = i64::from(run.area.y) * 1000;
            let area_height = i64::from(run.area.height) * 1000;
            let line_height = i64::from(measurement.line_height_millipixels);
            let baseline_from_top = i64::from(measurement.baseline_from_top_millipixels);
            let baseline = area_top + (area_height - line_height) / 2 + baseline_from_top;
            let rounded = if baseline >= 0 {
                (baseline + 500) / 1000
            } else {
                (baseline - 500) / 1000
            };
            i32::try_from(rounded)
                .map_err(|_| "centered application text baseline overflow".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paper_protocol::{
        V2DecodeResult, V2Payload, decode_v2_message, decode_v2_payload, encode_v2_frame,
        mono1_stride,
    };

    const WIDTH: u16 = 1272;
    const HEIGHT: u16 = 1624;

    #[test]
    fn zero_viewports_are_rejected_before_rendering() {
        let error = render_application(&ApplicationUi::default(), 0, HEIGHT)
            .expect_err("zero width must fail");
        assert!(error.contains("must be nonzero"));
    }

    #[test]
    fn bounded_render_roundtrips_and_rejects_oversized_extent_before_rendering() {
        let ui = ApplicationUi::default();
        let candidate = render_application_bounded(&ui, 9, 8).expect("application frame");
        let encoded = encode_v2_frame(42, &candidate).expect("encoded application frame");
        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(&encoded).expect("decoded v2 message")
        else {
            panic!("complete Frame expected");
        };
        assert_eq!(consumed, encoded.len());
        let V2Payload::Frame(frame) = decode_v2_payload(message).expect("typed frame") else {
            panic!("Frame payload expected");
        };
        assert_eq!(frame.frame_id(), 42);
        assert_eq!((frame.width(), frame.height()), (9, 8));

        let error = render_application_bounded(&ui, u16::MAX, u16::MAX)
            .expect_err("oversized frame must fail");
        assert!(error.contains("maximum"));
    }

    #[test]
    fn default_layout_has_open_buttons_centered_labels_and_empty_status() {
        let frame = render_application(&ApplicationUi::default(), WIDTH, HEIGHT)
            .expect("application frame");
        assert_eq!((frame.width(), frame.height()), (WIDTH, HEIGHT));
        assert_eq!(frame.pixel(20, 60), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(403, 60), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(20, 443), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(21, 61), Some(Mono1Pixel::White));

        let label =
            ink_bounds(&frame, 100, 140, 324, 340, Mono1Pixel::Black).expect("button label ink");
        assert!((i32::from(label.center_x()) - 212).abs() <= 1);
        assert!((i32::from(label.center_y()) - 252).abs() <= 1);
        assert!(
            (1584..HEIGHT)
                .all(|y| (0..WIDTH).all(|x| frame.pixel(x, y) == Some(Mono1Pixel::White)))
        );
    }

    #[test]
    fn pressed_button_is_filled_and_its_measured_label_is_inverted() {
        let mut ui = ApplicationUi::default();
        assert!(ui.set_pressed_button(Some(1)));
        let frame = render_application(&ui, WIDTH, HEIGHT).expect("pressed application frame");
        assert_eq!(frame.pixel(21, 61), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(100, 100), Some(Mono1Pixel::Black));
        let label =
            ink_bounds(&frame, 100, 140, 324, 340, Mono1Pixel::White).expect("inverted label ink");
        assert!((i32::from(label.center_x()) - 212).abs() <= 1);
        assert!((i32::from(label.center_y()) - 252).abs() <= 1);
        assert_eq!(frame.pixel(500, 500), Some(Mono1Pixel::White));
    }

    #[test]
    fn long_status_is_deterministic_ellipsized_and_confined_to_status_region() {
        let mut ui = ApplicationUi::default();
        ui.set_status("status ".repeat(2_000));
        let mut renderer = ApplicationRenderer::new().expect("renderer");
        let first = renderer.render(&ui, WIDTH, HEIGHT).expect("first frame");
        let cached = renderer.cached_glyph_count();
        let second = renderer.render(&ui, WIDTH, HEIGHT).expect("second frame");
        assert_eq!(first, second);
        assert_eq!(renderer.cached_glyph_count(), cached);
        let status = ink_bounds(&first, 0, HEIGHT - 40, WIDTH, HEIGHT, Mono1Pixel::Black)
            .expect("status ink");
        assert!(status.x >= 20);
        assert!(status.right() <= WIDTH - 20);
        assert!(status.y >= HEIGHT - 40);
        assert!(status.bottom() <= HEIGHT);
        assert!((0..20).all(|x| first.pixel(x, HEIGHT - 20) == Some(Mono1Pixel::White)));
        assert!(
            (WIDTH - 20..WIDTH).all(|x| first.pixel(x, HEIGHT - 20) == Some(Mono1Pixel::White))
        );
    }

    #[test]
    fn narrow_and_odd_width_viewports_remain_valid() {
        for (width, height) in [(1, 1), (9, 8), (37, 80), (123, 263), (1273, HEIGHT)] {
            let frame = render_application(&ApplicationUi::default(), width, height)
                .expect("valid narrow application frame");
            assert_eq!((frame.width(), frame.height()), (width, height));
            assert_eq!(frame.stride(), mono1_stride(width));
            assert_eq!(
                frame.pixels().len(),
                mono1_stride(width) * usize::from(height)
            );
        }
    }

    #[test]
    fn unsupported_status_returns_context_without_malformed_frame() {
        let mut ui = ApplicationUi::default();
        ui.set_status("unsupported 😀");
        let error = render_application(&ui, WIDTH, HEIGHT).expect_err("missing glyph must fail");
        assert!(error.contains("Status"));
        assert!(error.contains("no bundled glyph"));
    }

    #[derive(Clone, Copy, Debug)]
    struct PixelBounds {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    }

    impl PixelBounds {
        fn center_x(self) -> u16 {
            self.x + self.width / 2
        }

        fn center_y(self) -> u16 {
            self.y + self.height / 2
        }

        fn right(self) -> u16 {
            self.x + self.width
        }

        fn bottom(self) -> u16 {
            self.y + self.height
        }
    }

    fn ink_bounds(
        frame: &Mono1Frame,
        left: u16,
        top: u16,
        right: u16,
        bottom: u16,
        color: Mono1Pixel,
    ) -> Option<PixelBounds> {
        let mut minimum_x = right;
        let mut minimum_y = bottom;
        let mut maximum_x = left;
        let mut maximum_y = top;
        let mut found = false;
        for y in top..bottom {
            for x in left..right {
                if frame.pixel(x, y) != Some(color) {
                    continue;
                }
                found = true;
                minimum_x = minimum_x.min(x);
                minimum_y = minimum_y.min(y);
                maximum_x = maximum_x.max(x);
                maximum_y = maximum_y.max(y);
            }
        }
        found.then_some(PixelBounds {
            x: minimum_x,
            y: minimum_y,
            width: maximum_x - minimum_x + 1,
            height: maximum_y - minimum_y + 1,
        })
    }
}
