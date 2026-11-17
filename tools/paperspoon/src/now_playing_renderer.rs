//! Pure, deterministic Now Playing snapshot composition into a Gray8 frame.

use unicode_segmentation::UnicodeSegmentation;

use crate::gray_renderer::GrayCanvas;
use crate::now_playing::NowPlayingSnapshot;
use crate::text_renderer::{
    HorizontalAlignment, TextOverflow, TextRect, TextRenderer, TextRequest, TextRole,
};
use paper_protocol::Gray8Frame;

const BLACK: u8 = 0;
const MID_GRAY: u8 = 128;
const MAX_RENDERED_TEXT_CHARS: usize = 256;

/// Gray8 Now Playing compositor with reusable pinned-font caches.
#[derive(Debug)]
pub struct NowPlayingRenderer {
    text: TextRenderer,
}

impl NowPlayingRenderer {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            text: TextRenderer::new()
                .map_err(|error| format!("failed to initialize Now Playing text: {error}"))?,
        })
    }

    pub fn render(
        &mut self,
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
            self.draw_artwork_placeholder(
                &mut canvas,
                artwork_x,
                artwork_y,
                artwork_side,
                width,
                height,
            )?;
        }

        let line_gap = (margin / 3).clamp(2, 8);
        let mut text_y = artwork_y
            .saturating_add(artwork_side)
            .saturating_add(margin);
        let title = sanitize_single_line(&snapshot.title);
        let Some(next_y) = self.draw_line(
            &mut canvas,
            margin,
            text_y,
            inner_width,
            &title,
            TextRole::NowPlayingTitle,
            HorizontalAlignment::Left,
            BLACK,
            line_gap,
            width,
            height,
        )?
        else {
            return canvas.into_frame();
        };
        text_y = next_y;

        let artist = sanitize_single_line(&format!(
            "Artist: {}",
            snapshot.artist.as_deref().unwrap_or("Unknown")
        ));
        let Some(next_y) = self.draw_line(
            &mut canvas,
            margin,
            text_y,
            inner_width,
            &artist,
            TextRole::NowPlayingMetadata,
            HorizontalAlignment::Left,
            BLACK,
            line_gap,
            width,
            height,
        )?
        else {
            return canvas.into_frame();
        };
        text_y = next_y;

        let album = sanitize_single_line(&format!(
            "Album: {}",
            snapshot.album.as_deref().unwrap_or("Unknown")
        ));
        let Some(next_y) = self.draw_line(
            &mut canvas,
            margin,
            text_y,
            inner_width,
            &album,
            TextRole::NowPlayingMetadata,
            HorizontalAlignment::Left,
            BLACK,
            line_gap,
            width,
            height,
        )?
        else {
            return canvas.into_frame();
        };
        text_y = next_y;

        let state = match snapshot.is_playing {
            Some(true) => "PLAYING",
            Some(false) => "PAUSED",
            None => "PLAYBACK UNKNOWN",
        };
        let Some(next_y) = self.draw_line(
            &mut canvas,
            margin,
            text_y,
            inner_width,
            state,
            TextRole::NowPlayingState,
            HorizontalAlignment::Left,
            BLACK,
            line_gap,
            width,
            height,
        )?
        else {
            return canvas.into_frame();
        };
        text_y = next_y;

        if let Some((elapsed, duration, ratio)) = valid_progress(snapshot) {
            self.draw_progress(
                &mut canvas,
                margin,
                text_y,
                inner_width,
                elapsed,
                duration,
                ratio,
                width,
                height,
            )?;
        }

        canvas.into_frame()
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_line(
        &mut self,
        canvas: &mut GrayCanvas,
        x: u16,
        y: u16,
        width: u16,
        text: &str,
        role: TextRole,
        alignment: HorizontalAlignment,
        luminance: u8,
        gap: u16,
        canvas_width: u16,
        canvas_height: u16,
    ) -> Result<Option<u16>, String> {
        let line_height = role.style().line_height;
        if y >= canvas_height || canvas_height - y < line_height {
            return Ok(None);
        }
        let measurement = self
            .text
            .measure(role, text, width, TextOverflow::Ellipsis)
            .map_err(|error| format!("failed to measure {role:?} text: {error}"))?;
        let baseline_offset = (measurement.baseline_from_top_millipixels + 500) / 1_000;
        let baseline_y = i32::from(y)
            .checked_add(
                i32::try_from(baseline_offset)
                    .map_err(|_| format!("{role:?} baseline does not fit coordinates"))?,
            )
            .ok_or_else(|| format!("{role:?} baseline coordinate overflow"))?;
        let raster = self
            .text
            .rasterize(
                TextRequest {
                    role,
                    text,
                    area: TextRect {
                        x: i32::from(x),
                        y: i32::from(y),
                        width,
                        height: line_height,
                    },
                    baseline_y,
                    alignment,
                    overflow: TextOverflow::Ellipsis,
                },
                canvas_width,
                canvas_height,
            )
            .map_err(|error| format!("failed to rasterize {role:?} text: {error}"))?;
        canvas.draw_coverage(&raster.mask, luminance);
        Ok(Some(y.saturating_add(line_height).saturating_add(gap)))
    }

    fn draw_artwork_placeholder(
        &mut self,
        canvas: &mut GrayCanvas,
        x: u16,
        y: u16,
        side: u16,
        canvas_width: u16,
        canvas_height: u16,
    ) -> Result<(), String> {
        let line_height = TextRole::NowPlayingState.style().line_height;
        if side < line_height {
            return Ok(());
        }
        let line_y = y.saturating_add((side - line_height) / 2);
        self.draw_line(
            canvas,
            x,
            line_y,
            side,
            "NO ART",
            TextRole::NowPlayingState,
            HorizontalAlignment::Center,
            MID_GRAY,
            0,
            canvas_width,
            canvas_height,
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_progress(
        &mut self,
        canvas: &mut GrayCanvas,
        margin: u16,
        text_y: u16,
        inner_width: u16,
        elapsed: f64,
        duration: f64,
        ratio: f64,
        canvas_width: u16,
        canvas_height: u16,
    ) -> Result<(), String> {
        let gap = (margin / 2).clamp(4, 12);
        let bar_height = (canvas_width.min(canvas_height) / 100).clamp(4, 12);
        let bar_y = text_y.saturating_add(gap);
        let time_height = TextRole::NowPlayingMetadata.style().line_height;
        let required_height = bar_height.saturating_add(gap).saturating_add(time_height);
        if bar_y >= canvas_height || canvas_height - bar_y < required_height {
            return Ok(());
        }

        canvas.outline_rect(margin, bar_y, inner_width, bar_height, BLACK);
        let interior_width = inner_width.saturating_sub(2);
        let filled = (f64::from(interior_width) * ratio).round() as u16;
        if filled > 0 && bar_height > 2 {
            canvas.fill_rect(
                margin.saturating_add(1),
                bar_y.saturating_add(1),
                filled,
                bar_height - 2,
                MID_GRAY,
            );
        }

        let time_y = bar_y.saturating_add(bar_height).saturating_add(gap);
        let times = format!("{} / {}", format_time(elapsed), format_time(duration));
        self.draw_line(
            canvas,
            margin,
            time_y,
            inner_width,
            &times,
            TextRole::NowPlayingMetadata,
            HorizontalAlignment::Left,
            BLACK,
            0,
            canvas_width,
            canvas_height,
        )?;
        Ok(())
    }

    #[cfg(test)]
    fn cached_glyph_count(&self) -> usize {
        self.text.cached_glyph_count()
    }
}

/// One-shot helper entry point. Font initialization is intentionally inside
/// this call because production invokes it in the short-lived helper process.
pub fn render_now_playing(
    snapshot: &NowPlayingSnapshot,
    width: u16,
    height: u16,
) -> Result<Gray8Frame, String> {
    NowPlayingRenderer::new()?.render(snapshot, width, height)
}

fn sanitize_single_line(value: &str) -> String {
    let mut output = String::new();
    let mut used_chars = 0_usize;
    for grapheme in value.graphemes(true) {
        let grapheme_chars = grapheme.chars().count();
        if used_chars.saturating_add(grapheme_chars) > MAX_RENDERED_TEXT_CHARS {
            break;
        }
        for character in grapheme.chars() {
            output.push(if character.is_control() {
                ' '
            } else {
                character
            });
        }
        used_chars += grapheme_chars;
    }
    output
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
    fn complete_track_uses_antialiased_text_in_portrait_gray8() {
        let artwork = DynamicImage::ImageLuma8(ImageBuffer::from_pixel(8, 4, Luma([u8::MAX])));
        let frame = render_now_playing(&complete_snapshot(Some(artwork)), 1272, 1624)
            .expect("render Now Playing");
        assert_eq!(
            (frame.width(), frame.height(), frame.stride()),
            (1272, 1624, 1272)
        );
        assert_eq!(frame.pixels().len(), 1272 * 1624);
        assert!(frame.pixels().contains(&BLACK));
        assert!(
            frame
                .pixels()
                .iter()
                .any(|&pixel| pixel > BLACK && pixel < u8::MAX)
        );
    }

    #[test]
    fn missing_metadata_and_artwork_render_placeholder() {
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
    fn long_unicode_metadata_is_deterministic_and_reuses_cache() {
        let mut snapshot = complete_snapshot(None);
        snapshot.title = "Café Музыка e\u{301} ".repeat(100);
        snapshot.artist = Some("Long artist ".repeat(100));
        snapshot.album = Some("Long album ".repeat(100));
        let mut renderer = NowPlayingRenderer::new().expect("renderer");
        let first = renderer
            .render(&snapshot, 1273, 1625)
            .expect("first odd frame");
        let cached = renderer.cached_glyph_count();
        let second = renderer
            .render(&snapshot, 1273, 1625)
            .expect("second odd frame");
        assert_eq!(first, second);
        assert_eq!(renderer.cached_glyph_count(), cached);
    }

    #[test]
    fn tiny_and_short_wide_viewports_omit_lower_priority_content() {
        let snapshot = complete_snapshot(None);
        for (width, height) in [(1, 1), (9, 8), (800, 200), (1273, 97)] {
            let frame = render_now_playing(&snapshot, width, height).expect("small frame");
            assert_eq!((frame.width(), frame.height()), (width, height));
            assert_eq!(
                frame.pixels().len(),
                usize::from(width) * usize::from(height)
            );
        }
    }

    #[test]
    fn unsupported_metadata_glyph_returns_contextual_error() {
        let mut snapshot = complete_snapshot(None);
        snapshot.title = "unsupported 😀".to_string();
        let error =
            render_now_playing(&snapshot, 1272, 1624).expect_err("missing glyph must reject frame");
        assert!(error.contains("NowPlayingTitle"));
        assert!(error.contains("no bundled glyph"));
    }

    #[test]
    fn sanitation_replaces_controls_without_splitting_graphemes() {
        let cluster = "e\u{301}";
        let value = format!("line\n{}{}", "a".repeat(253), cluster);
        let sanitized = sanitize_single_line(&value);
        assert!(sanitized.starts_with("line "));
        assert!(!sanitized.ends_with(cluster));
        assert!(sanitized.chars().count() <= MAX_RENDERED_TEXT_CHARS);
    }

    #[test]
    fn pinned_font_fixture_has_stable_pixel_checksum() {
        let snapshot = NowPlayingSnapshot {
            title: "Café Музыка e\u{301}".to_string(),
            artist: Some("Pinned Inter preview".to_string()),
            album: Some("PaperSpoon typography".to_string()),
            is_playing: Some(true),
            elapsed_time: Some(83.0),
            duration: Some(257.0),
            bundle_id: Some("org.paperspoon.preview".to_string()),
            bundle_name: Some("PaperSpoon Preview".to_string()),
            artwork: None,
        };
        let frame = render_now_playing(&snapshot, 1272, 1624).expect("pinned-font fixture");
        let checksum = frame
            .pixels()
            .iter()
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        assert_eq!(checksum, 0x3f70_9b2f_14d1_f996);
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
