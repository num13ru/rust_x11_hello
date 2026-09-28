//! Pure, deterministic Now Playing snapshot composition into a Gray8 frame.

use paper_protocol::Gray8Frame;

use crate::gray_renderer::GrayCanvas;
use crate::now_playing::NowPlayingSnapshot;

const BLACK: u8 = 0;
const MID_GRAY: u8 = 128;
const MAX_RENDERED_TEXT_CHARS: usize = 256;

pub fn render_now_playing(
    snapshot: &NowPlayingSnapshot,
    width: u16,
    height: u16,
) -> Result<Gray8Frame, String> {
    let mut canvas = GrayCanvas::new(width, height)?;
    let short_side = width.min(height);
    let margin = (short_side / 32).clamp(2, 24);
    let inner_width = width.saturating_sub(margin.saturating_mul(2)).max(1);
    let artwork_height_cap = (u32::from(height) * 11 / 20) as u16;
    let artwork_side = inner_width.min(artwork_height_cap).max(1);
    let artwork_x = width.saturating_sub(artwork_side) / 2;
    let artwork_y = margin.min(height.saturating_sub(1));

    let artwork_rendered = snapshot.artwork.as_ref().is_some_and(|artwork| {
        canvas
            .draw_image_fit_contain(artwork, artwork_x, artwork_y, artwork_side, artwork_side)
            .is_ok()
    });
    canvas.outline_rect(artwork_x, artwork_y, artwork_side, artwork_side, BLACK);
    if !artwork_rendered {
        let placeholder_scale = text_scale(width).min(2);
        let label_y = artwork_y.saturating_add(artwork_side / 2);
        canvas.draw_text(
            artwork_x.saturating_add(margin),
            label_y,
            "NO ART",
            placeholder_scale,
            MID_GRAY,
        );
    }

    let title_scale = text_scale(width);
    let detail_scale = title_scale.saturating_sub(1).max(1);
    let mut text_y = artwork_y
        .saturating_add(artwork_side)
        .saturating_add(margin);
    draw_line(
        &mut canvas,
        margin,
        &mut text_y,
        &single_line(&snapshot.title),
        title_scale,
    );
    draw_line(
        &mut canvas,
        margin,
        &mut text_y,
        &format!(
            "Artist: {}",
            snapshot.artist.as_deref().unwrap_or("Unknown")
        ),
        detail_scale,
    );
    draw_line(
        &mut canvas,
        margin,
        &mut text_y,
        &format!("Album: {}", snapshot.album.as_deref().unwrap_or("Unknown")),
        detail_scale,
    );
    let state = match snapshot.is_playing {
        Some(true) => "PLAYING",
        Some(false) => "PAUSED",
        None => "PLAYBACK UNKNOWN",
    };
    draw_line(&mut canvas, margin, &mut text_y, state, detail_scale);

    if let Some((elapsed, duration, ratio)) = valid_progress(snapshot) {
        let gap = detail_scale.saturating_mul(2);
        text_y = text_y.saturating_add(gap);
        let bar_height = detail_scale.saturating_mul(4).max(4);
        if text_y < height {
            canvas.outline_rect(margin, text_y, inner_width, bar_height, BLACK);
            let interior_width = inner_width.saturating_sub(2);
            let filled = (f64::from(interior_width) * ratio).round() as u16;
            if filled > 0 && bar_height > 2 {
                canvas.fill_rect(
                    margin.saturating_add(1),
                    text_y.saturating_add(1),
                    filled,
                    bar_height - 2,
                    MID_GRAY,
                );
            }
            text_y = text_y.saturating_add(bar_height).saturating_add(gap);
            let times = format!("{} / {}", format_time(elapsed), format_time(duration));
            draw_line(&mut canvas, margin, &mut text_y, &times, detail_scale);
        }
    }

    canvas.into_frame()
}

fn text_scale(width: u16) -> u16 {
    (width / 300).clamp(1, 4)
}

fn draw_line(canvas: &mut GrayCanvas, x: u16, y: &mut u16, text: &str, scale: u16) {
    let (_, height) = canvas.dimensions();
    if *y >= height {
        return;
    }
    let text = single_line(text);
    canvas.draw_text(x, *y, &text, scale, BLACK);
    let line_height = 14_u16.saturating_mul(scale);
    *y = y
        .saturating_add(line_height)
        .saturating_add(scale.saturating_mul(2));
}

fn single_line(value: &str) -> String {
    value
        .chars()
        .take(MAX_RENDERED_TEXT_CHARS)
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn valid_progress(snapshot: &NowPlayingSnapshot) -> Option<(f64, f64, f64)> {
    let elapsed = snapshot.elapsed_time?;
    let duration = snapshot.duration?;
    if !elapsed.is_finite() || !duration.is_finite() || duration <= 0.0 {
        return None;
    }
    let elapsed = elapsed.clamp(0.0, duration);
    Some((elapsed, duration, elapsed / duration))
}

fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Luma};

    fn complete_snapshot(artwork: Option<DynamicImage>) -> NowPlayingSnapshot {
        NowPlayingSnapshot {
            title: "A Track".to_string(),
            artist: Some("An Artist".to_string()),
            album: Some("An Album".to_string()),
            is_playing: Some(true),
            elapsed_time: Some(83.0),
            duration: Some(257.0),
            bundle_id: Some("com.example.player".to_string()),
            bundle_name: Some("Player".to_string()),
            artwork,
        }
    }

    #[test]
    fn complete_track_with_artwork_renders_portrait_gray8() {
        let artwork =
            DynamicImage::ImageLuma8(ImageBuffer::from_fn(8, 4, |x, _| Luma([(x * 32) as u8])));
        let frame = render_now_playing(&complete_snapshot(Some(artwork)), 1272, 1624)
            .expect("render Now Playing");
        assert_eq!(
            (frame.width(), frame.height(), frame.stride()),
            (1272, 1624, 1272)
        );
        assert_eq!(frame.pixels().len(), 1272 * 1624);
        assert!(frame.pixels().contains(&0));
        assert!(frame.pixels().iter().any(|&pixel| pixel > 0 && pixel < 255));
    }

    #[test]
    fn missing_artwork_artist_and_album_still_render() {
        let mut snapshot = complete_snapshot(None);
        snapshot.artist = None;
        snapshot.album = None;
        let frame = render_now_playing(&snapshot, 321, 479).expect("placeholder frame");
        assert_eq!(
            (frame.width(), frame.height(), frame.stride()),
            (321, 479, 321)
        );
        assert!(frame.pixels().contains(&MID_GRAY));
    }

    #[test]
    fn paused_and_long_title_render_on_odd_and_tiny_viewports() {
        let mut snapshot = complete_snapshot(None);
        snapshot.is_playing = Some(false);
        snapshot.title = "Long title ".repeat(10_000);
        render_now_playing(&snapshot, 1273, 1625).expect("odd frame");
        let tiny = render_now_playing(&snapshot, 9, 8).expect("tiny frame");
        assert_eq!(tiny.pixels().len(), 72);
    }

    #[test]
    fn progress_rejects_invalid_values_and_clamps_elapsed() {
        let mut snapshot = complete_snapshot(None);
        assert_eq!(valid_progress(&snapshot), Some((83.0, 257.0, 83.0 / 257.0)));
        snapshot.elapsed_time = Some(999.0);
        assert_eq!(valid_progress(&snapshot), Some((257.0, 257.0, 1.0)));
        snapshot.duration = Some(0.0);
        assert_eq!(valid_progress(&snapshot), None);
        snapshot.duration = Some(f64::NAN);
        assert_eq!(valid_progress(&snapshot), None);
        snapshot.elapsed_time = Some(f64::INFINITY);
        snapshot.duration = Some(10.0);
        assert_eq!(valid_progress(&snapshot), None);
    }
}
