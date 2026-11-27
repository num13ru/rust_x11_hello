//! Pure, deterministic Now Playing snapshot composition into a Gray8 frame.

use std::time::Instant;

use image::DynamicImage;
use unicode_segmentation::UnicodeSegmentation;

use crate::gray_renderer::GrayCanvas;
use crate::now_playing::NowPlayingSnapshot;
use crate::now_playing_provider::{Playback, PlaybackCommand, RepeatMode};
use crate::text_renderer::{
    HorizontalAlignment, TextOverflow, TextRect, TextRenderer, TextRequest, TextRole,
};
use paper_protocol::Gray8Frame;

const BLACK: u8 = 0;
const MAX_RENDERED_TEXT_CHARS: usize = 256;

struct HeaderLayout {
    margin: u16,
    inner_width: u16,
    artwork_side: u16,
    artwork_y: u16,
    artwork_gap: u16,
    line_gap: u16,
}

impl HeaderLayout {
    fn new(width: u16, height: u16, artwork_max: u16) -> Self {
        let margin = (width.min(height) / 32).clamp(2, 24);
        let inner_width = width.saturating_sub(margin * 2).max(1);
        let artwork_cap = (u32::from(height) * 11 / 20) as u16;
        Self {
            margin,
            inner_width,
            artwork_side: inner_width.min(artwork_cap).min(artwork_max).max(1),
            artwork_y: margin.min(height.saturating_sub(1)),
            artwork_gap: margin,
            line_gap: (margin / 3).clamp(2, 8),
        }
    }

    fn text_start(&self) -> u16 {
        self.artwork_y
            .saturating_add(self.artwork_side)
            .saturating_add(self.artwork_gap)
    }

    fn artwork_bounds(&self, width: u16) -> ControlRect {
        ControlRect {
            x: width.saturating_sub(self.artwork_side) / 2,
            y: self.artwork_y,
            width: self.artwork_side,
            height: self.artwork_side,
        }
    }

    fn metadata_height(&self) -> u16 {
        TextRole::NowPlayingTitle.style().line_height
            + TextRole::NowPlayingMetadata.style().line_height * 2
            + self.line_gap * 3
    }
}

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
        let state = match snapshot.is_playing {
            Some(true) => "PLAYING",
            Some(false) => "PAUSED",
            None => "PLAYBACK UNKNOWN",
        };
        self.render_canvas(
            snapshot,
            snapshot.artwork.as_ref(),
            Some(state),
            (width, height),
            &HeaderLayout::new(width, height, height),
        )?
        .into_frame()
    }

    fn render_canvas(
        &mut self,
        snapshot: &NowPlayingSnapshot,
        artwork: Option<&DynamicImage>,
        state: Option<&str>,
        (width, height): (u16, u16),
        layout: &HeaderLayout,
    ) -> Result<GrayCanvas, String> {
        let mut canvas = GrayCanvas::new(width, height)?;
        let HeaderLayout {
            margin,
            inner_width,
            line_gap,
            ..
        } = *layout;
        let ControlRect {
            x: artwork_x,
            y: artwork_y,
            width: artwork_side,
            ..
        } = layout.artwork_bounds(width);

        let artwork_rendered = artwork.is_some_and(|artwork| {
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

        let mut text_y = layout.text_start();
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
            return Ok(canvas);
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
            return Ok(canvas);
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
            return Ok(canvas);
        };
        text_y = next_y;

        let Some(state) = state else {
            return Ok(canvas);
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
            return Ok(canvas);
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

        Ok(canvas)
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
        canvas.draw_binary_coverage(&raster.mask, luminance);
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
            BLACK,
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
                BLACK,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveControl {
    Cover,
    Previous,
    PlayPause,
    Next,
    Seek,
    Shuffle,
    Repeat,
}

impl LiveControl {
    pub fn enabled(self, playback: &Playback) -> bool {
        playback
            .controllable_track()
            .is_some_and(|track| match self {
                Self::Seek => track.duration().is_some(),
                Self::Shuffle => track.shuffle.is_some(),
                Self::Repeat => track.repeat.is_some(),
                Self::Cover | Self::Previous | Self::PlayPause | Self::Next => true,
            })
    }
}

#[derive(Clone, Copy)]
struct ControlRect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

impl ControlRect {
    fn contains(self, x: u16, y: u16) -> bool {
        (self.x..self.x + self.width).contains(&x) && (self.y..self.y + self.height).contains(&y)
    }
}

pub struct LiveLayout {
    header: HeaderLayout,
    cover: ControlRect,
    controls: Option<LiveControls>,
}

struct LiveControls {
    progress: ControlRect,
    time_y: u16,
    text_role: TextRole,
    buttons: [(LiveControl, ControlRect); 3],
    mode_buttons: Option<[(LiveControl, ControlRect); 2]>,
}

impl LiveControls {
    fn buttons(&self) -> impl Iterator<Item = (LiveControl, ControlRect)> {
        self.buttons
            .into_iter()
            .chain(self.mode_buttons.into_iter().flatten())
    }
}

const BUTTON_GAP: u16 = 8;

fn button_row<const N: usize>(
    controls: [LiveControl; N],
    x: u16,
    y: u16,
    width: u16,
    height: u16,
) -> [(LiveControl, ControlRect); N] {
    let button_width = (width - BUTTON_GAP * (N as u16 - 1)) / N as u16;
    std::array::from_fn(|index| {
        (
            controls[index],
            ControlRect {
                x: x + index as u16 * (button_width + BUTTON_GAP),
                y,
                width: button_width,
                height,
            },
        )
    })
}

impl LiveLayout {
    pub fn new((width, height): (u16, u16)) -> Self {
        const GAP: u16 = 8;
        let short_side = width.min(height);
        let progress_height = (short_side / 18).clamp(28, 66);
        let button_height = (short_side / 10).clamp(44, 96);
        let text_role = if short_side >= 480 {
            TextRole::ButtonLabel
        } else {
            TextRole::Status
        };
        let mut header = HeaderLayout::new(width, height, height);
        let text_height = header.metadata_height()
            + TextRole::NowPlayingState.style().line_height
            + header.line_gap;
        let time_height = text_role.style().line_height;
        let timeline_height = progress_height + time_height;
        let bottom_gap = (short_side / 40).clamp(4, 24);
        let artwork_gap = header.margin * 4;
        let reserved = header.margin
            + artwork_gap
            + text_height
            + timeline_height
            + button_height
            + GAP * 2
            + bottom_gap;
        let modes_height = button_height + BUTTON_GAP;
        let show_modes = header.inner_width >= 260 && height > reserved + modes_height;
        let reserved = reserved + if show_modes { modes_height } else { 0 };
        let artwork_max = height.saturating_sub(reserved);
        let controls = (artwork_max > 0 && header.inner_width >= 88).then(|| {
            header.artwork_side = header.artwork_side.min(artwork_max);
            header.artwork_y += artwork_max - header.artwork_side;
            header.artwork_gap = artwork_gap;
            let timeline_y = header.text_start() + text_height + GAP;
            let progress = ControlRect {
                x: header.margin,
                y: timeline_y,
                width: header.inner_width,
                height: progress_height,
            };
            let buttons_y = timeline_y + timeline_height + GAP;
            let buttons = button_row(
                [
                    LiveControl::Previous,
                    LiveControl::PlayPause,
                    LiveControl::Next,
                ],
                header.margin,
                buttons_y,
                header.inner_width,
                button_height,
            );
            let mode_buttons = show_modes.then(|| {
                button_row(
                    [LiveControl::Shuffle, LiveControl::Repeat],
                    header.margin,
                    buttons_y + modes_height,
                    header.inner_width,
                    button_height,
                )
            });
            LiveControls {
                progress,
                time_y: timeline_y + progress_height,
                text_role,
                buttons,
                mode_buttons,
            }
        });
        let cover = header.artwork_bounds(width);
        Self {
            header,
            cover,
            controls,
        }
    }

    pub fn control_at(&self, x: u16, y: u16) -> Option<LiveControl> {
        if self.cover.contains(x, y) {
            return Some(LiveControl::Cover);
        }
        let controls = self.controls.as_ref()?;
        if controls.progress.contains(x, y) {
            return Some(LiveControl::Seek);
        }
        controls
            .buttons()
            .find_map(|(control, rect)| rect.contains(x, y).then_some(control))
    }

    pub fn command(
        &self,
        control: LiveControl,
        x: u16,
        playback: &Playback,
    ) -> Option<PlaybackCommand> {
        if !control.enabled(playback) {
            return None;
        }
        Some(match control {
            LiveControl::Cover => PlaybackCommand::RevealTrack,
            LiveControl::Previous => PlaybackCommand::Previous,
            LiveControl::PlayPause => PlaybackCommand::PlayPause,
            LiveControl::Next => PlaybackCommand::Next,
            LiveControl::Shuffle => PlaybackCommand::ToggleShuffle,
            LiveControl::Repeat => PlaybackCommand::CycleRepeat,
            LiveControl::Seek => {
                let duration = playback.track()?.duration()?;
                let progress = self.controls.as_ref()?.progress;
                let offset = x.saturating_sub(progress.x).min(progress.width - 1);
                let ratio = f64::from(offset) / f64::from(progress.width - 1);
                PlaybackCommand::Seek(duration * ratio)
            }
        })
    }
}

#[derive(PartialEq, Eq)]
struct LiveHeader {
    viewport: (u16, u16),
    title: String,
    artist: Option<String>,
    album: Option<String>,
    artwork_revision: u64,
}

pub struct LiveNowPlayingRenderer {
    renderer: NowPlayingRenderer,
    header: Option<(LiveHeader, GrayCanvas)>,
}

impl LiveNowPlayingRenderer {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            renderer: NowPlayingRenderer::new()?,
            header: None,
        })
    }

    pub fn render(
        &mut self,
        viewport: (u16, u16),
        playback: &Playback,
        artwork: Option<&DynamicImage>,
        artwork_revision: u64,
        pressed: Option<LiveControl>,
        now: Instant,
    ) -> Result<Gray8Frame, String> {
        let layout = LiveLayout::new(viewport);
        let track = playback.track();
        let header = LiveHeader {
            viewport,
            title: track.map_or_else(|| "Now Playing".into(), |track| track.title.clone()),
            artist: track.and_then(|track| track.artist.clone()),
            album: track.and_then(|track| track.album.clone()),
            artwork_revision,
        };
        if self
            .header
            .as_ref()
            .is_none_or(|(cached, _)| *cached != header)
        {
            let snapshot = NowPlayingSnapshot {
                title: header.title.clone(),
                artist: header.artist.clone(),
                album: header.album.clone(),
                is_playing: None,
                elapsed_time: None,
                duration: None,
                bundle_id: None,
                bundle_name: None,
                artwork: None,
            };
            let canvas =
                self.renderer
                    .render_canvas(&snapshot, artwork, None, viewport, &layout.header)?;
            self.header = Some((header, canvas));
        }
        let mut canvas = self
            .header
            .as_ref()
            .expect("static Now Playing canvas")
            .1
            .clone();
        let (width, height) = viewport;
        let header = layout.header;
        self.renderer.draw_line(
            &mut canvas,
            header.margin,
            header.text_start().saturating_add(header.metadata_height()),
            header.inner_width,
            playback.label(),
            TextRole::NowPlayingState,
            HorizontalAlignment::Left,
            BLACK,
            0,
            width,
            height,
        )?;
        if let Some(controls) = layout.controls {
            self.draw_live_controls(&mut canvas, controls, playback, pressed, now)?;
        }
        canvas.into_frame()
    }

    fn draw_live_controls(
        &mut self,
        canvas: &mut GrayCanvas,
        controls: LiveControls,
        playback: &Playback,
        pressed: Option<LiveControl>,
        now: Instant,
    ) -> Result<(), String> {
        let (width, height) = canvas.dimensions();
        let ControlRect {
            x,
            y,
            width: bar_width,
            height: hit_height,
        } = controls.progress;
        let bar_height = if pressed == Some(LiveControl::Seek) {
            20
        } else {
            12
        };
        let bar_y = y + (hit_height - bar_height) / 2;
        let position = playback.position(now);
        let duration = playback.track().and_then(|track| track.duration());
        canvas.outline_rect(x, bar_y, bar_width, bar_height, BLACK);
        if LiveControl::Seek.enabled(playback) {
            canvas.outline_rect(x + 1, bar_y + 1, bar_width - 2, bar_height - 2, BLACK);
        }
        if let Some((position, duration)) = position.zip(duration) {
            let filled = (f64::from(bar_width - 4) * (position / duration)).round() as u16;
            canvas.fill_rect(x + 2, bar_y + 2, filled, bar_height - 4, BLACK);
        }
        let elapsed =
            position.map_or_else(|| "--:--".into(), |position| format_time(position.floor()));
        let remaining = duration.zip(position).map_or_else(
            || "- --:--".into(),
            |(duration, position)| format!("-{}", format_time(duration.ceil() - position.floor())),
        );
        for (start, alignment, label) in [
            (x, HorizontalAlignment::Left, elapsed),
            (x + bar_width / 2, HorizontalAlignment::Right, remaining),
        ] {
            self.renderer.draw_line(
                canvas,
                start,
                controls.time_y,
                bar_width / 2,
                &label,
                controls.text_role,
                alignment,
                BLACK,
                0,
                width,
                height,
            )?;
        }
        let track = playback.track();
        for (control, rect) in controls.buttons() {
            let armed = pressed == Some(control);
            canvas.outline_rect(rect.x, rect.y, rect.width, rect.height, BLACK);
            if control.enabled(playback) {
                canvas.outline_rect(
                    rect.x + 1,
                    rect.y + 1,
                    rect.width - 2,
                    rect.height - 2,
                    BLACK,
                );
            }
            if armed {
                canvas.fill_rect(rect.x, rect.y, rect.width, rect.height, BLACK);
            }
            let label = match control {
                LiveControl::Previous => "PREV",
                LiveControl::PlayPause if matches!(playback, Playback::Playing(_)) => "PAUSE",
                LiveControl::PlayPause => "PLAY",
                LiveControl::Next => "NEXT",
                LiveControl::Shuffle => match track.and_then(|track| track.shuffle) {
                    Some(true) => "SHUFFLE ON",
                    Some(false) => "SHUFFLE OFF",
                    None => "SHUFFLE --",
                },
                LiveControl::Repeat => match track.and_then(|track| track.repeat) {
                    Some(RepeatMode::Off) => "REPEAT OFF",
                    Some(RepeatMode::All) => "REPEAT ALL",
                    Some(RepeatMode::One) => "REPEAT ONE",
                    None => "REPEAT --",
                },
                LiveControl::Cover | LiveControl::Seek => unreachable!("not a button"),
            };
            self.renderer.draw_line(
                canvas,
                rect.x + 1,
                rect.y + (rect.height - controls.text_role.style().line_height) / 2,
                rect.width - 2,
                label,
                controls.text_role,
                HorizontalAlignment::Center,
                if armed { u8::MAX } else { BLACK },
                0,
                width,
                height,
            )?;
        }
        Ok(())
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
    use crate::now_playing_provider::{FakeChange, FakeMusic};

    #[test]
    fn live_progress_preserves_static_artwork_and_a_backward_seek_restores_pixels() {
        use std::time::Duration;
        let now = Instant::now();
        let mut provider = FakeMusic::default();
        provider.change(FakeChange::Seek(45.0), now);
        let playback = provider.sample_at(now).unwrap();
        let mut renderer = LiveNowPlayingRenderer::new().unwrap();
        let artwork =
            DynamicImage::ImageLuma8(image::GrayImage::from_pixel(48, 48, image::Luma([111])));
        let viewport = (320, 480);
        let frame = renderer
            .render(viewport, &playback, Some(&artwork), 1, None, now)
            .unwrap();
        let later = now + Duration::from_secs(4);
        let advanced = renderer
            .render(viewport, &playback, Some(&artwork), 1, None, later)
            .unwrap();
        let static_pixels =
            320 * usize::from(LiveLayout::new(viewport).controls.unwrap().progress.y);
        assert_eq!(
            &frame.pixels()[..static_pixels],
            &advanced.pixels()[..static_pixels]
        );
        assert_ne!(frame, advanced);
        assert_eq!(frame.pixels()[22 * 320 + 160], 111);

        provider.change(FakeChange::Seek(45.0), later);
        let rewound = provider.sample_at(later).unwrap();
        assert_eq!(
            frame,
            renderer
                .render(viewport, &rewound, Some(&artwork), 1, None, later)
                .unwrap()
        );
        let placeholder = renderer
            .render(viewport, &rewound, None, 2, None, later)
            .unwrap();
        assert_ne!(
            &frame.pixels()[..static_pixels],
            &placeholder.pixels()[..static_pixels]
        );
    }
    use image::{DynamicImage, ImageBuffer, Luma};

    #[test]
    fn kindle_progress_bar_renders_and_seeks_across_the_full_content_width() {
        let viewport = (1272, 1624);
        let now = Instant::now();
        let playback = FakeMusic::default().sample_at(now).unwrap();
        let layout = LiveLayout::new(viewport);
        let progress = layout.controls.as_ref().unwrap().progress;
        let first = progress.x;
        let last = progress.x + progress.width - 1;
        assert_eq!((first, last), (24, 1247));

        let frame = LiveNowPlayingRenderer::new()
            .unwrap()
            .render(viewport, &playback, None, 0, None, now)
            .unwrap();
        let outline_y = progress.y + (progress.height - 12) / 2;
        let row = frame
            .pixels()
            .chunks_exact(usize::from(viewport.0))
            .nth(usize::from(outline_y))
            .unwrap();
        assert!(
            row[usize::from(first)..=usize::from(last)]
                .iter()
                .all(|&pixel| pixel == BLACK)
        );

        for (x, seconds) in [(first, 0.0), (last, 180.0)] {
            let control = layout.control_at(x, outline_y).unwrap();
            assert_eq!(control, LiveControl::Seek);
            assert_eq!(
                layout.command(control, x, &playback),
                Some(PlaybackCommand::Seek(seconds))
            );
        }
        assert_eq!(layout.control_at(first - 1, outline_y), None);
        assert_eq!(layout.control_at(last + 1, outline_y), None);
    }

    #[test]
    fn live_widget_anchors_the_rendered_buttons_and_hit_boxes_above_the_viewport_edge() {
        let now = Instant::now();
        let playback = FakeMusic::default().sample_at(now).unwrap();
        let mut renderer = LiveNowPlayingRenderer::new().unwrap();
        for (viewport, gap) in [((1272, 1624), 24), ((800, 480), 12), ((240, 420), 6)] {
            let (width, height) = viewport;
            let layout = LiveLayout::new(viewport);
            let (control, button) = layout.controls.as_ref().unwrap().buttons().last().unwrap();
            let button_bottom = height - gap;
            assert_eq!(button.y + button.height, button_bottom);
            let x = button.x + button.width / 2;
            assert_eq!(layout.control_at(x, button_bottom - 1), Some(control));
            assert_eq!(layout.control_at(x, button_bottom), None);

            let frame = renderer
                .render(viewport, &playback, None, 0, None, now)
                .unwrap();
            let width = usize::from(width);
            assert_eq!(
                frame.pixels()[usize::from(button_bottom - 1) * width + usize::from(x)],
                BLACK
            );
            assert!(
                frame.pixels()[usize::from(button_bottom) * width..]
                    .iter()
                    .all(|&pixel| pixel == u8::MAX)
            );
            assert!(
                frame.pixels()[..usize::from(layout.header.artwork_y) * width]
                    .iter()
                    .all(|&pixel| pixel == u8::MAX)
            );
            if viewport == (1272, 1624) {
                assert!(layout.header.artwork_y > layout.header.margin);
                assert_eq!(
                    layout.header.artwork_side,
                    HeaderLayout::new(1272, 1624, 1624).artwork_side
                );
                let artwork_bottom = layout.header.artwork_y + layout.header.artwork_side;
                let title_y = layout.header.text_start();
                assert_eq!(title_y - artwork_bottom, 96);
                assert!(
                    frame.pixels()
                        [usize::from(artwork_bottom) * width..usize::from(title_y) * width]
                        .iter()
                        .all(|&pixel| pixel == u8::MAX)
                );
            }
        }
    }

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

    fn default_now_playing_viewport() -> (u16, u16) {
        let fixture = include_str!("../../paperview/tests/fixtures/now-playing-viewport.txt");
        let (width, height) = fixture
            .trim()
            .split_once('x')
            .expect("Now Playing viewport fixture must be WIDTHxHEIGHT");
        (
            width.parse().expect("fixture width must be u16"),
            height.parse().expect("fixture height must be u16"),
        )
    }

    #[test]
    fn complete_track_uses_binary_ui_and_grayscale_artwork() {
        let artwork = DynamicImage::ImageLuma8(ImageBuffer::from_pixel(8, 4, Luma([127])));
        let (width, height) = default_now_playing_viewport();
        let frame = render_now_playing(&complete_snapshot(Some(artwork)), width, height)
            .expect("render Now Playing");
        assert_eq!(
            (frame.width(), frame.height(), frame.stride()),
            (width, height, usize::from(width))
        );
        assert_eq!(
            frame.pixels().len(),
            usize::from(width) * usize::from(height)
        );
        assert!(frame.pixels().contains(&BLACK));
        assert!(frame.pixels().contains(&127));
        let layout = HeaderLayout::new(width, height, height);
        let metadata = usize::from(width) * usize::from(layout.text_start());
        assert!(
            frame.pixels()[metadata..]
                .iter()
                .all(|&pixel| matches!(pixel, 0 | 255))
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
        assert!(frame.pixels().contains(&BLACK));
        assert!(frame.pixels().iter().all(|&pixel| matches!(pixel, 0 | 255)));
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
        assert_eq!(checksum, 0xe911_2b92_5887_a885);
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
