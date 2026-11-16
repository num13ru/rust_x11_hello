//! Deterministic, host-only OpenType shaping and glyph coverage.
//!
//! This module owns font selection, single-line shaping, measurement, fallback,
//! ellipsis, and rasterization into an 8-bit coverage mask. It does not know
//! about Mono1, Gray8, transport, application layout, or Kindle display state.
//! Callers supply an absolute first-line baseline and generic clipping bounds.
//!
//! Newlines and other control characters are normalized to spaces. The current
//! bundled fonts guarantee the tested Latin, Cyrillic, punctuation, and
//! combining-mark repertoire. Other scripts and emoji fail descriptively when
//! neither bundled face supplies a glyph; no system font is consulted. This
//! single-line API does not promise full bidirectional or complex-script
//! coverage beyond what the bundled faces and focused tests establish.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, Color, Fallback, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
    fontdb,
};
use unicode_script::Script;
use unicode_segmentation::UnicodeSegmentation;

const IBM_PLEX_MONO_MEDIUM: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf");
const INTER_VARIABLE: &[u8] = include_bytes!("../assets/fonts/InterVariable.ttf");
const IBM_PLEX_MONO_FAMILY: &str = "IBM Plex Mono";
const INTER_FAMILY: &str = "Inter Variable";
const FIXED_LOCALE: &str = "en-US";
const ELLIPSIS: &str = "…";
const MAX_TEXT_BYTES: usize = 16 * 1024;

/// Semantic text role. Font details remain centralized here rather than at UI
/// call sites.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextRole {
    ApplicationTitle,
    ButtonLabel,
    Status,
    NowPlayingTitle,
    NowPlayingMetadata,
    NowPlayingState,
}

/// Bundled face selected by a semantic role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FontFace {
    IbmPlexMono,
    Inter,
}

/// Stable style values associated with a [`TextRole`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextStyle {
    pub face: FontFace,
    pub weight: u16,
    pub pixel_size: u16,
    pub line_height: u16,
}

impl TextRole {
    pub const fn style(self) -> TextStyle {
        match self {
            Self::ApplicationTitle => mono_style(18, 24),
            Self::ButtonLabel => mono_style(28, 36),
            Self::Status => mono_style(18, 24),
            Self::NowPlayingTitle => inter_style(42, 52),
            Self::NowPlayingMetadata => inter_style(28, 36),
            Self::NowPlayingState => inter_style(24, 32),
        }
    }
}

const fn mono_style(pixel_size: u16, line_height: u16) -> TextStyle {
    TextStyle {
        face: FontFace::IbmPlexMono,
        weight: 500,
        pixel_size,
        line_height,
    }
}

const fn inter_style(pixel_size: u16, line_height: u16) -> TextStyle {
    TextStyle {
        face: FontFace::Inter,
        weight: 400,
        pixel_size,
        line_height,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HorizontalAlignment {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextOverflow {
    Clip,
    Ellipsis,
}

/// Signed origin with nonnegative extent. Rectangles are half-open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextRect {
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
}

/// A single-line baseline-anchored raster request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextRequest<'a> {
    pub role: TextRole,
    pub text: &'a str,
    pub area: TextRect,
    pub baseline_y: i32,
    pub alignment: HorizontalAlignment,
    pub overflow: TextOverflow,
}

/// Measurement uses integer millipixels so results remain exactly comparable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextMeasurement {
    pub displayed_text: String,
    pub truncated: bool,
    pub advance_millipixels: u32,
    pub baseline_from_top_millipixels: u32,
    pub line_height_millipixels: u32,
    pub font_families: Vec<String>,
}

/// Dense coverage clipped to both the request rectangle and canvas bounds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoverageMask {
    origin_x: u16,
    origin_y: u16,
    width: u16,
    height: u16,
    pixels: Vec<u8>,
}

impl CoverageMask {
    pub const fn origin(&self) -> (u16, u16) {
        (self.origin_x, self.origin_y)
    }

    pub const fn dimensions(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    fn empty(origin_x: u16, origin_y: u16) -> Self {
        Self {
            origin_x,
            origin_y,
            width: 0,
            height: 0,
            pixels: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        origin_x: u16,
        origin_y: u16,
        width: u16,
        height: u16,
        pixels: Vec<u8>,
    ) -> Self {
        assert_eq!(pixels.len(), usize::from(width) * usize::from(height));
        Self {
            origin_x,
            origin_y,
            width,
            height,
            pixels,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RasterizedText {
    pub measurement: TextMeasurement,
    pub text_origin_x: i32,
    pub baseline_y: i32,
    pub ink_bounds: Option<TextRect>,
    pub mask: CoverageMask,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextError {
    InvalidCanvas { width: u16, height: u16 },
    InvalidArea { width: u16, height: u16 },
    CoordinateOverflow,
    TextTooLong { bytes: usize, maximum: usize },
    FontLoad { family: &'static str },
    MissingGlyph { byte_index: usize },
    LayoutUnavailable,
    AllocationOverflow,
    AllocationFailed(String),
}

impl fmt::Display for TextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCanvas { width, height } => {
                write!(
                    formatter,
                    "text canvas must be nonzero, got {width}x{height}"
                )
            }
            Self::InvalidArea { width, height } => {
                write!(formatter, "text area must be nonzero, got {width}x{height}")
            }
            Self::CoordinateOverflow => write!(formatter, "text rectangle coordinate overflow"),
            Self::TextTooLong { bytes, maximum } => {
                write!(formatter, "text is {bytes} bytes; maximum is {maximum}")
            }
            Self::FontLoad { family } => write!(formatter, "failed to load bundled {family} font"),
            Self::MissingGlyph { byte_index } => {
                write!(
                    formatter,
                    "no bundled glyph for text at byte index {byte_index}"
                )
            }
            Self::LayoutUnavailable => write!(formatter, "text engine produced no layout line"),
            Self::AllocationOverflow => write!(formatter, "text coverage allocation overflow"),
            Self::AllocationFailed(message) => {
                write!(formatter, "failed to allocate text coverage: {message}")
            }
        }
    }
}

impl Error for TextError {}

#[derive(Debug)]
struct EmbeddedFallback;

impl Fallback for EmbeddedFallback {
    fn common_fallback(&self) -> &[&'static str] {
        &[INTER_FAMILY]
    }

    fn forbidden_fallback(&self) -> &[&'static str] {
        &[]
    }

    fn script_fallback(&self, _script: Script, _locale: &str) -> &[&'static str] {
        &[]
    }
}

#[derive(Debug)]
pub struct TextRenderer {
    font_system: FontSystem,
    glyph_cache: SwashCache,
}

impl TextRenderer {
    pub fn new() -> Result<Self, TextError> {
        Self::from_font_bytes(IBM_PLEX_MONO_MEDIUM, INTER_VARIABLE)
    }

    fn from_font_bytes(mono: &[u8], inter: &[u8]) -> Result<Self, TextError> {
        let mut database = fontdb::Database::new();
        let mono_ids = database.load_font_source(fontdb::Source::Binary(Arc::new(mono.to_vec())));
        if mono_ids.is_empty() || !ids_have_family(&database, &mono_ids, IBM_PLEX_MONO_FAMILY) {
            return Err(TextError::FontLoad {
                family: IBM_PLEX_MONO_FAMILY,
            });
        }
        let inter_ids = database.load_font_source(fontdb::Source::Binary(Arc::new(inter.to_vec())));
        if inter_ids.is_empty() || !ids_have_family(&database, &inter_ids, INTER_FAMILY) {
            return Err(TextError::FontLoad {
                family: INTER_FAMILY,
            });
        }
        database.set_monospace_family(IBM_PLEX_MONO_FAMILY);
        database.set_sans_serif_family(INTER_FAMILY);

        Ok(Self {
            font_system: FontSystem::new_with_locale_and_db_and_fallback(
                FIXED_LOCALE.to_string(),
                database,
                EmbeddedFallback,
            ),
            glyph_cache: SwashCache::new(),
        })
    }

    pub fn measure(
        &mut self,
        role: TextRole,
        text: &str,
        maximum_width: u16,
        overflow: TextOverflow,
    ) -> Result<TextMeasurement, TextError> {
        if maximum_width == 0 {
            return Err(TextError::InvalidArea {
                width: 0,
                height: role.style().line_height,
            });
        }
        let normalized = normalize_text(text)?;
        let (displayed_text, truncated) =
            self.fit_text(role, &normalized, maximum_width, overflow)?;
        let raw = self.measure_raw(role, &displayed_text)?;
        Ok(TextMeasurement {
            displayed_text,
            truncated,
            advance_millipixels: raw.advance_millipixels,
            baseline_from_top_millipixels: raw.baseline_from_top_millipixels,
            line_height_millipixels: raw.line_height_millipixels,
            font_families: raw.font_families,
        })
    }

    pub fn rasterize(
        &mut self,
        request: TextRequest<'_>,
        canvas_width: u16,
        canvas_height: u16,
    ) -> Result<RasterizedText, TextError> {
        if canvas_width == 0 || canvas_height == 0 {
            return Err(TextError::InvalidCanvas {
                width: canvas_width,
                height: canvas_height,
            });
        }
        if request.area.width == 0 || request.area.height == 0 {
            return Err(TextError::InvalidArea {
                width: request.area.width,
                height: request.area.height,
            });
        }
        let clip = intersect_canvas(request.area, canvas_width, canvas_height)?;
        let measurement = self.measure(
            request.role,
            request.text,
            request.area.width,
            request.overflow,
        )?;
        let measured_width = measurement.advance_millipixels as f64 / 1000.0;
        let alignment_offset = match request.alignment {
            HorizontalAlignment::Left => 0.0,
            HorizontalAlignment::Center => (f64::from(request.area.width) - measured_width) / 2.0,
            HorizontalAlignment::Right => f64::from(request.area.width) - measured_width,
        };
        let text_origin_x = request
            .area
            .x
            .checked_add(alignment_offset.round() as i32)
            .ok_or(TextError::CoordinateOverflow)?;

        let Some(clip) = clip else {
            return Ok(RasterizedText {
                measurement,
                text_origin_x,
                baseline_y: request.baseline_y,
                ink_bounds: None,
                mask: CoverageMask::empty(0, 0),
            });
        };
        if measurement.displayed_text.is_empty() {
            return Ok(RasterizedText {
                measurement,
                text_origin_x,
                baseline_y: request.baseline_y,
                ink_bounds: None,
                mask: CoverageMask::empty(clip.x, clip.y),
            });
        }

        let pixel_count = usize::from(clip.width)
            .checked_mul(usize::from(clip.height))
            .ok_or(TextError::AllocationOverflow)?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(pixel_count)
            .map_err(|error| TextError::AllocationFailed(error.to_string()))?;
        pixels.resize(pixel_count, 0);

        let style = request.role.style();
        let mut buffer = new_buffer(&mut self.font_system, style);
        let attrs = attrs_for(style);
        {
            let mut buffer = buffer.borrow_with(&mut self.font_system);
            buffer.set_size(None, Some(f32::from(style.line_height)));
            buffer.set_wrap(Wrap::None);
            buffer.set_text(&measurement.displayed_text, &attrs, Shaping::Advanced, None);
            let line_y = buffer
                .layout_runs()
                .next()
                .ok_or(TextError::LayoutUnavailable)?
                .line_y;
            let vertical_shift = request
                .baseline_y
                .checked_sub(line_y.round() as i32)
                .ok_or(TextError::CoordinateOverflow)?;
            buffer.draw(
                &mut self.glyph_cache,
                Color::rgb(0, 0, 0),
                |x, y, pixel_width, pixel_height, color| {
                    composite_coverage(
                        &mut pixels,
                        clip,
                        text_origin_x,
                        vertical_shift,
                        x,
                        y,
                        pixel_width,
                        pixel_height,
                        color.a(),
                    );
                },
            );
        }

        let mask = CoverageMask {
            origin_x: clip.x,
            origin_y: clip.y,
            width: clip.width,
            height: clip.height,
            pixels,
        };
        let ink_bounds = ink_bounds(&mask);
        Ok(RasterizedText {
            measurement,
            text_origin_x,
            baseline_y: request.baseline_y,
            ink_bounds,
            mask,
        })
    }

    pub fn cached_glyph_count(&self) -> usize {
        self.glyph_cache.image_cache.len()
    }

    fn fit_text(
        &mut self,
        role: TextRole,
        text: &str,
        maximum_width: u16,
        overflow: TextOverflow,
    ) -> Result<(String, bool), TextError> {
        let full = self.measure_raw(role, text)?;
        if overflow == TextOverflow::Clip
            || full.advance_millipixels <= u32::from(maximum_width) * 1000
        {
            return Ok((text.to_string(), false));
        }

        let ellipsis_width = self.measure_raw(role, ELLIPSIS)?.advance_millipixels;
        if ellipsis_width > u32::from(maximum_width) * 1000 {
            return Ok((String::new(), true));
        }
        let mut boundaries = text
            .grapheme_indices(true)
            .map(|(byte_index, _)| byte_index)
            .collect::<Vec<_>>();
        boundaries.push(text.len());
        let grapheme_count = boundaries.len().saturating_sub(1);
        let mut lower = 0;
        let mut upper = grapheme_count;
        while lower < upper {
            let middle = (lower + upper).div_ceil(2);
            let candidate = format!("{}{}", &text[..boundaries[middle]], ELLIPSIS);
            let width = self.measure_raw(role, &candidate)?.advance_millipixels;
            if width <= u32::from(maximum_width) * 1000 {
                lower = middle;
            } else {
                upper = middle - 1;
            }
        }
        Ok((format!("{}{}", &text[..boundaries[lower]], ELLIPSIS), true))
    }

    fn measure_raw(&mut self, role: TextRole, text: &str) -> Result<RawMeasurement, TextError> {
        let style = role.style();
        let shaping_text = if text.is_empty() { " " } else { text };
        let mut buffer = new_buffer(&mut self.font_system, style);
        let attrs = attrs_for(style);
        let (advance, baseline, line_height, font_ids) = {
            let mut buffer = buffer.borrow_with(&mut self.font_system);
            buffer.set_size(None, Some(f32::from(style.line_height)));
            buffer.set_wrap(Wrap::None);
            buffer.set_text(shaping_text, &attrs, Shaping::Advanced, None);
            let run = buffer
                .layout_runs()
                .next()
                .ok_or(TextError::LayoutUnavailable)?;
            if let Some(glyph) = run.glyphs.iter().find(|glyph| glyph.glyph_id == 0) {
                return Err(TextError::MissingGlyph {
                    byte_index: glyph.start,
                });
            }
            let font_ids = run
                .glyphs
                .iter()
                .map(|glyph| glyph.font_id)
                .collect::<BTreeSet<_>>();
            (
                if text.is_empty() { 0.0 } else { run.line_w },
                run.line_y,
                run.line_height,
                font_ids,
            )
        };
        let font_families = font_ids
            .into_iter()
            .filter_map(|font_id| self.font_system.db().face(font_id))
            .filter_map(|face| face.families.first())
            .map(|(family, _language)| family.clone())
            .collect();
        Ok(RawMeasurement {
            advance_millipixels: to_millipixels(advance),
            baseline_from_top_millipixels: to_millipixels(baseline),
            line_height_millipixels: to_millipixels(line_height),
            font_families,
        })
    }
}

#[derive(Debug)]
struct RawMeasurement {
    advance_millipixels: u32,
    baseline_from_top_millipixels: u32,
    line_height_millipixels: u32,
    font_families: Vec<String>,
}

fn new_buffer(font_system: &mut FontSystem, style: TextStyle) -> Buffer {
    Buffer::new(
        font_system,
        Metrics::new(f32::from(style.pixel_size), f32::from(style.line_height)),
    )
}

fn attrs_for(style: TextStyle) -> Attrs<'static> {
    let family = match style.face {
        FontFace::IbmPlexMono => IBM_PLEX_MONO_FAMILY,
        FontFace::Inter => INTER_FAMILY,
    };
    Attrs::new()
        .family(Family::Name(family))
        .weight(Weight(style.weight))
}

fn normalize_text(text: &str) -> Result<String, TextError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(TextError::TextTooLong {
            bytes: text.len(),
            maximum: MAX_TEXT_BYTES,
        });
    }
    Ok(text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect())
}

fn ids_have_family(database: &fontdb::Database, ids: &[fontdb::ID], expected_family: &str) -> bool {
    ids.iter().any(|font_id| {
        database.face(*font_id).is_some_and(|face| {
            face.families
                .iter()
                .any(|(family, _language)| family == expected_family)
        })
    })
}

#[derive(Clone, Copy, Debug)]
struct ClippedRect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

fn intersect_canvas(
    area: TextRect,
    canvas_width: u16,
    canvas_height: u16,
) -> Result<Option<ClippedRect>, TextError> {
    let area_right = area
        .x
        .checked_add(i32::from(area.width))
        .ok_or(TextError::CoordinateOverflow)?;
    let area_bottom = area
        .y
        .checked_add(i32::from(area.height))
        .ok_or(TextError::CoordinateOverflow)?;
    let left = area.x.max(0);
    let top = area.y.max(0);
    let right = area_right.min(i32::from(canvas_width));
    let bottom = area_bottom.min(i32::from(canvas_height));
    if left >= right || top >= bottom {
        return Ok(None);
    }
    Ok(Some(ClippedRect {
        x: u16::try_from(left).map_err(|_| TextError::CoordinateOverflow)?,
        y: u16::try_from(top).map_err(|_| TextError::CoordinateOverflow)?,
        width: u16::try_from(right - left).map_err(|_| TextError::CoordinateOverflow)?,
        height: u16::try_from(bottom - top).map_err(|_| TextError::CoordinateOverflow)?,
    }))
}

#[allow(clippy::too_many_arguments)]
fn composite_coverage(
    pixels: &mut [u8],
    clip: ClippedRect,
    horizontal_shift: i32,
    vertical_shift: i32,
    glyph_x: i32,
    glyph_y: i32,
    glyph_width: u32,
    glyph_height: u32,
    coverage: u8,
) {
    for offset_y in 0..glyph_height {
        let Some(y) = glyph_y
            .checked_add(vertical_shift)
            .and_then(|value| value.checked_add_unsigned(offset_y))
        else {
            continue;
        };
        let Some(relative_y) = y.checked_sub(i32::from(clip.y)) else {
            continue;
        };
        if relative_y < 0 || relative_y >= i32::from(clip.height) {
            continue;
        }
        for offset_x in 0..glyph_width {
            let Some(x) = glyph_x
                .checked_add(horizontal_shift)
                .and_then(|value| value.checked_add_unsigned(offset_x))
            else {
                continue;
            };
            let Some(relative_x) = x.checked_sub(i32::from(clip.x)) else {
                continue;
            };
            if relative_x < 0 || relative_x >= i32::from(clip.width) {
                continue;
            }
            let Ok(relative_x) = usize::try_from(relative_x) else {
                continue;
            };
            let Ok(relative_y) = usize::try_from(relative_y) else {
                continue;
            };
            let destination = relative_y * usize::from(clip.width) + relative_x;
            pixels[destination] = pixels[destination].max(coverage);
        }
    }
}

fn ink_bounds(mask: &CoverageMask) -> Option<TextRect> {
    let mut minimum_x = mask.width;
    let mut minimum_y = mask.height;
    let mut maximum_x = 0;
    let mut maximum_y = 0;
    let mut found = false;
    for y in 0..mask.height {
        for x in 0..mask.width {
            if mask.pixels[usize::from(y) * usize::from(mask.width) + usize::from(x)] == 0 {
                continue;
            }
            found = true;
            minimum_x = minimum_x.min(x);
            minimum_y = minimum_y.min(y);
            maximum_x = maximum_x.max(x);
            maximum_y = maximum_y.max(y);
        }
    }
    found.then(|| TextRect {
        x: i32::from(mask.origin_x + minimum_x),
        y: i32::from(mask.origin_y + minimum_y),
        width: maximum_x - minimum_x + 1,
        height: maximum_y - minimum_y + 1,
    })
}

fn to_millipixels(value: f32) -> u32 {
    (value.max(0.0) * 1000.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer() -> TextRenderer {
        TextRenderer::new().expect("bundled fonts")
    }

    fn request<'a>(text: &'a str) -> TextRequest<'a> {
        TextRequest {
            role: TextRole::ApplicationTitle,
            text,
            area: TextRect {
                x: 0,
                y: 0,
                width: 240,
                height: 40,
            },
            baseline_y: 24,
            alignment: HorizontalAlignment::Left,
            overflow: TextOverflow::Clip,
        }
    }

    #[test]
    fn semantic_styles_are_explicit_and_distinct() {
        assert_eq!(TextRole::ApplicationTitle.style(), mono_style(18, 24));
        assert_eq!(TextRole::ButtonLabel.style(), mono_style(28, 36));
        assert_eq!(TextRole::NowPlayingTitle.style(), inter_style(42, 52));
        assert_eq!(TextRole::NowPlayingMetadata.style(), inter_style(28, 36));
    }

    #[test]
    fn mono_advances_match_and_inter_is_proportional_and_kerned() {
        let mut renderer = renderer();
        let mono_i = renderer
            .measure(TextRole::ApplicationTitle, "iiii", 500, TextOverflow::Clip)
            .expect("mono i");
        let mono_w = renderer
            .measure(TextRole::ApplicationTitle, "WWWW", 500, TextOverflow::Clip)
            .expect("mono W");
        assert_eq!(mono_i.advance_millipixels, mono_w.advance_millipixels);

        let inter_i = renderer
            .measure(
                TextRole::NowPlayingMetadata,
                "iiii",
                500,
                TextOverflow::Clip,
            )
            .expect("Inter i");
        let inter_w = renderer
            .measure(
                TextRole::NowPlayingMetadata,
                "WWWW",
                500,
                TextOverflow::Clip,
            )
            .expect("Inter W");
        assert_ne!(inter_i.advance_millipixels, inter_w.advance_millipixels);
        let a = renderer
            .measure(TextRole::NowPlayingMetadata, "A", 500, TextOverflow::Clip)
            .expect("A")
            .advance_millipixels;
        let v = renderer
            .measure(TextRole::NowPlayingMetadata, "V", 500, TextOverflow::Clip)
            .expect("V")
            .advance_millipixels;
        let av = renderer
            .measure(TextRole::NowPlayingMetadata, "AV", 500, TextOverflow::Clip)
            .expect("AV")
            .advance_millipixels;
        assert!(av < a + v);
    }

    #[test]
    fn baseline_whitespace_and_empty_text_have_defined_metrics() {
        let mut renderer = renderer();
        let empty = renderer
            .measure(TextRole::Status, "", 100, TextOverflow::Clip)
            .expect("empty measurement");
        assert_eq!(empty.advance_millipixels, 0);
        assert!(empty.baseline_from_top_millipixels > 0);
        assert_eq!(empty.line_height_millipixels, 24_000);

        let spaces = renderer
            .measure(TextRole::Status, "   ", 100, TextOverflow::Clip)
            .expect("spaces");
        assert!(spaces.advance_millipixels > 0);
        let raster = renderer
            .rasterize(request("   "), 240, 40)
            .expect("whitespace raster");
        assert!(raster.mask.pixels().iter().all(|coverage| *coverage == 0));
        assert_eq!(raster.ink_bounds, None);
    }

    #[test]
    fn bundled_fallback_handles_greek_without_system_fonts() {
        let mut renderer = renderer();
        let measurement = renderer
            .measure(TextRole::ApplicationTitle, "Ω", 100, TextOverflow::Clip)
            .expect("Greek fallback");
        assert_eq!(measurement.font_families, [INTER_FAMILY.to_string()]);
    }

    #[test]
    fn clipping_handles_negative_origins_descenders_and_canvas_edges() {
        let mut renderer = renderer();
        let raster = renderer
            .rasterize(
                TextRequest {
                    area: TextRect {
                        x: -12,
                        y: -5,
                        width: 40,
                        height: 28,
                    },
                    baseline_y: 18,
                    ..request("gyp")
                },
                25,
                20,
            )
            .expect("clipped raster");
        assert_eq!(raster.mask.origin(), (0, 0));
        assert_eq!(raster.mask.dimensions(), (25, 20));
        assert_eq!(raster.mask.pixels().len(), 25 * 20);
        let bounds = raster.ink_bounds.expect("visible ink");
        assert!(bounds.x >= 0 && bounds.y >= 0);
        assert!(bounds.x + i32::from(bounds.width) <= 25);
        assert!(bounds.y + i32::from(bounds.height) <= 20);
    }

    #[test]
    fn alignment_changes_origin_without_changing_measurement() {
        let mut renderer = renderer();
        let left = renderer
            .rasterize(request("center"), 240, 40)
            .expect("left");
        let center = renderer
            .rasterize(
                TextRequest {
                    alignment: HorizontalAlignment::Center,
                    ..request("center")
                },
                240,
                40,
            )
            .expect("center");
        let right = renderer
            .rasterize(
                TextRequest {
                    alignment: HorizontalAlignment::Right,
                    ..request("center")
                },
                240,
                40,
            )
            .expect("right");
        assert_eq!(left.measurement, center.measurement);
        assert_eq!(center.measurement, right.measurement);
        assert!(left.text_origin_x < center.text_origin_x);
        assert!(center.text_origin_x < right.text_origin_x);
    }

    #[test]
    fn ellipsis_uses_measured_width_and_preserves_graphemes() {
        let mut renderer = renderer();
        let source = "Cafe\u{301} Cafe\u{301} Cafe\u{301}";
        let measurement = renderer
            .measure(
                TextRole::NowPlayingMetadata,
                source,
                90,
                TextOverflow::Ellipsis,
            )
            .expect("ellipsized text");
        assert!(measurement.truncated);
        assert!(measurement.displayed_text.ends_with(ELLIPSIS));
        assert!(measurement.advance_millipixels <= 90_000);
        let prefix = measurement.displayed_text.trim_end_matches(ELLIPSIS);
        assert!(source.starts_with(prefix));
        assert!(
            prefix.len() == source.len()
                || source
                    .grapheme_indices(true)
                    .any(|(byte_index, _)| byte_index == prefix.len()),
            "prefix ends at a source grapheme boundary"
        );
    }

    #[test]
    fn controls_are_single_line_and_cache_is_reused() {
        let mut renderer = renderer();
        let first = renderer
            .rasterize(request("first\nsecond\tthird"), 240, 40)
            .expect("first raster");
        assert_eq!(first.measurement.displayed_text, "first second third");
        let cached = renderer.cached_glyph_count();
        assert!(cached > 0);
        let second = renderer
            .rasterize(request("first\nsecond\tthird"), 240, 40)
            .expect("second raster");
        assert_eq!(first, second);
        assert_eq!(renderer.cached_glyph_count(), cached);
    }

    #[test]
    fn invalid_and_unsupported_inputs_fail_descriptively() {
        let mut renderer = renderer();
        assert_eq!(
            renderer.rasterize(request("x"), 0, 40),
            Err(TextError::InvalidCanvas {
                width: 0,
                height: 40
            })
        );
        assert_eq!(
            renderer.rasterize(
                TextRequest {
                    area: TextRect {
                        width: 0,
                        ..request("x").area
                    },
                    ..request("x")
                },
                40,
                40
            ),
            Err(TextError::InvalidArea {
                width: 0,
                height: 40
            })
        );
        assert_eq!(
            renderer.measure(
                TextRole::Status,
                &"x".repeat(MAX_TEXT_BYTES + 1),
                100,
                TextOverflow::Clip
            ),
            Err(TextError::TextTooLong {
                bytes: MAX_TEXT_BYTES + 1,
                maximum: MAX_TEXT_BYTES
            })
        );
        assert!(matches!(
            renderer.measure(TextRole::Status, "😀", 100, TextOverflow::Clip),
            Err(TextError::MissingGlyph { .. })
        ));
    }

    #[test]
    fn corrupt_font_data_is_rejected() {
        assert!(matches!(
            TextRenderer::from_font_bytes(b"not a font", INTER_VARIABLE),
            Err(TextError::FontLoad {
                family: IBM_PLEX_MONO_FAMILY
            })
        ));
        assert!(matches!(
            TextRenderer::from_font_bytes(IBM_PLEX_MONO_MEDIUM, b"not a font"),
            Err(TextError::FontLoad {
                family: INTER_FAMILY
            })
        ));
        assert!(matches!(
            TextRenderer::from_font_bytes(INTER_VARIABLE, INTER_VARIABLE),
            Err(TextError::FontLoad {
                family: IBM_PLEX_MONO_FAMILY
            })
        ));
    }

    #[test]
    fn baseline_moves_ink_and_off_canvas_or_overflowing_areas_stay_safe() {
        let mut renderer = renderer();
        let upper = renderer
            .rasterize(request("baseline"), 240, 80)
            .expect("upper baseline");
        let lower = renderer
            .rasterize(
                TextRequest {
                    baseline_y: 44,
                    area: TextRect {
                        height: 80,
                        ..request("baseline").area
                    },
                    ..request("baseline")
                },
                240,
                80,
            )
            .expect("lower baseline");
        assert_eq!(
            lower.ink_bounds.expect("lower ink").y - upper.ink_bounds.expect("upper ink").y,
            20
        );

        let outside = renderer
            .rasterize(
                TextRequest {
                    area: TextRect {
                        x: -100,
                        y: -100,
                        width: 20,
                        height: 20,
                    },
                    ..request("outside")
                },
                40,
                40,
            )
            .expect("off-canvas request");
        assert_eq!(outside.mask.dimensions(), (0, 0));
        assert_eq!(outside.ink_bounds, None);

        assert_eq!(
            renderer.rasterize(
                TextRequest {
                    area: TextRect {
                        x: i32::MAX,
                        width: 2,
                        ..request("overflow").area
                    },
                    ..request("overflow")
                },
                40,
                40,
            ),
            Err(TextError::CoordinateOverflow)
        );
    }
}
