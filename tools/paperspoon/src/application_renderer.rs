//! Pure composition of host application state into a remote Mono1 frame.

use crate::application_ui::{ApplicationLayout, ApplicationUi, TextRun};
use crate::mono_renderer::MonoCanvas;
use paper_protocol::{
    Mono1Frame, Mono1FrameError, Mono1Pixel, V2_FRAME_PREFIX_LEN, V2_MAX_PAYLOAD_LEN,
    encode_v2_frame, mono1_payload_len,
};

/// Render and encode an application frame after enforcing the v2 payload cap.
pub fn encode_application(
    frame_id: u64,
    ui: &ApplicationUi,
    width: u16,
    height: u16,
) -> Result<Vec<u8>, String> {
    let pixel_bytes = mono1_payload_len(width, height)
        .ok_or_else(|| "application frame dimensions overflow payload size".to_string())?;
    let payload_bytes = V2_FRAME_PREFIX_LEN
        .checked_add(pixel_bytes)
        .ok_or_else(|| "application frame payload size overflow".to_string())?;
    if payload_bytes > V2_MAX_PAYLOAD_LEN {
        return Err(format!(
            "application frame payload is {payload_bytes} bytes; maximum is {V2_MAX_PAYLOAD_LEN}"
        ));
    }

    let frame = render_application(ui, width, height)
        .map_err(|error| format!("failed to render application frame: {error}"))?;
    encode_v2_frame(frame_id, &frame)
        .map_err(|error| format!("failed to encode application frame: {error}"))
}

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
    use paper_protocol::{V2DecodeResult, V2Payload, decode_v2_message, decode_v2_payload};

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
    fn encoding_roundtrips_and_rejects_oversized_extent_before_rendering() {
        let ui = ApplicationUi::default();
        let encoded = encode_application(42, &ui, 9, 8).expect("encoded application frame");
        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(&encoded).expect("decoded v2 message")
        else {
            panic!("complete Frame expected");
        };
        assert_eq!(consumed, encoded.len());
        let V2Payload::Frame(frame) = decode_v2_payload(message).expect("typed Frame") else {
            panic!("Frame expected");
        };
        assert_eq!(frame.frame_id(), 42);
        assert_eq!((frame.width(), frame.height()), (9, 8));

        let error =
            encode_application(43, &ui, u16::MAX, u16::MAX).expect_err("oversized frame rejected");
        assert!(error.contains("maximum"));
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
