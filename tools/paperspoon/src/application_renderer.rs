//! Pure composition of host application state into a remote Mono1 frame.

use crate::application_ui::{ApplicationLayout, ApplicationUi, TextRun};
use crate::mono_renderer::MonoCanvas;
use paper_protocol::{Mono1Frame, Mono1FrameError, Mono1Pixel};

/// Render one complete PaperSpoon-owned application viewport.
pub fn render_application(
    ui: &ApplicationUi,
    width: u16,
    height: u16,
) -> Result<Mono1Frame, Mono1FrameError> {
    let layout = ui
        .layout(width, height)
        .ok_or(Mono1FrameError::ZeroDimension { width, height })?;
    let mut canvas = MonoCanvas::new(width, height)?;
    draw_layout(&mut canvas, &layout);
    canvas.into_frame()
}

fn draw_layout(canvas: &mut MonoCanvas, layout: &ApplicationLayout<'_>) {
    draw_text(canvas, layout.title);
    for button in &layout.buttons {
        canvas.outline_rect(
            button.bounds.x,
            button.bounds.y,
            button.bounds.width,
            button.bounds.height,
            Mono1Pixel::Black,
        );
        draw_text(canvas, button.label);
    }
    if let Some(status) = layout.status {
        draw_text(canvas, status);
    }
}

fn draw_text(canvas: &mut MonoCanvas, text: TextRun<'_>) {
    canvas.draw_text(text.x, text.baseline_y, text.text, Mono1Pixel::Black);
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: u16 = 1272;
    const HEIGHT: u16 = 1624;

    #[test]
    fn zero_viewports_are_rejected_before_rendering() {
        assert_eq!(
            render_application(&ApplicationUi::default(), 0, HEIGHT),
            Err(Mono1FrameError::ZeroDimension {
                width: 0,
                height: HEIGHT,
            })
        );
    }

    #[test]
    fn portrait_frame_contains_title_button_edges_and_readable_label() {
        let frame = render_application(&ApplicationUi::default(), WIDTH, HEIGHT)
            .expect("application frame");
        assert_eq!((frame.width(), frame.height()), (WIDTH, HEIGHT));

        assert_eq!(frame.pixel(20, 27), Some(Mono1Pixel::White));
        assert_eq!(frame.pixel(21, 27), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(20, 60), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(403, 60), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(20, 443), Some(Mono1Pixel::Black));
        assert_eq!(frame.pixel(21, 61), Some(Mono1Pixel::White));

        assert_eq!(frame.pixel(209, 244), Some(Mono1Pixel::White));
        assert_eq!(frame.pixel(211, 244), Some(Mono1Pixel::Black));
        assert!(
            (1584..HEIGHT)
                .all(|y| { (0..WIDTH).all(|x| frame.pixel(x, y) == Some(Mono1Pixel::White)) })
        );
    }

    #[test]
    fn status_is_rendered_inside_remote_status_region_deterministically() {
        let mut ui = ApplicationUi::default();
        ui.set_status("ok");
        let first = render_application(&ui, WIDTH, HEIGHT).expect("first frame");
        let second = render_application(&ui, WIDTH, HEIGHT).expect("second frame");

        assert_eq!(first, second);
        assert_eq!(first.pixel(20, 1605), Some(Mono1Pixel::White));
        assert_eq!(first.pixel(21, 1605), Some(Mono1Pixel::Black));
        assert_eq!(first.pixel(0, HEIGHT - 1), Some(Mono1Pixel::White));
        assert_eq!(first.pixel(WIDTH - 1, HEIGHT - 1), Some(Mono1Pixel::White));
    }

    #[test]
    fn odd_width_render_remains_a_valid_protocol_frame() {
        let frame = render_application(&ApplicationUi::default(), 1273, HEIGHT)
            .expect("odd-width application frame");
        assert_eq!(frame.stride(), 160);
        assert_eq!(frame.pixels().len(), 160 * usize::from(HEIGHT));
    }
}
