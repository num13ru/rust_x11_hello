//! Stage 0 host-only typography feasibility spike.
//!
//! This example deliberately does not import either production canvas. It
//! shapes pinned font bytes into a reusable coverage mask, then writes that
//! mask as thresholded Mono1 PBM and antialiased Gray8 PGM samples.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cosmic_text::{
    Attrs, Buffer, Color, Fallback, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
    fontdb,
};
use unicode_script::Script;

const SAMPLE_WIDTH: usize = 1180;
const MONO_THRESHOLD: u8 = 128;
const WARM_ITERATIONS: u32 = 50;
const SAMPLE_TEXT: &str = concat!(
    "ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz\n",
    "0123456789 !@#$%^&*()[]{}:;,.?/-_+=\n",
    "Cyrillic: Привет, мир — Ёж\n",
    "Combining: Cafe\u{301} A\u{30a} n\u{303}\n",
    "Now Playing: Björk — Jóga / Молчат Дома — Судно",
);

const IBM_PLEX_MONO_MEDIUM: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf");
const INTER_VARIABLE: &[u8] = include_bytes!("../assets/fonts/InterVariable.ttf");

#[derive(Clone, Debug, Eq, PartialEq)]
struct CoverageMask {
    width: usize,
    height: usize,
    coverage: Vec<u8>,
    glyph_count: usize,
    missing_glyph_count: usize,
    max_line_width_millipixels: u32,
    font_families: Vec<String>,
}

#[derive(Debug)]
struct SpikeRenderer {
    font_system: FontSystem,
    glyph_cache: SwashCache,
}

#[derive(Debug)]
struct EmbeddedFallback;

impl Fallback for EmbeddedFallback {
    fn common_fallback(&self) -> &[&'static str] {
        &["Inter Variable"]
    }

    fn forbidden_fallback(&self) -> &[&'static str] {
        &[]
    }

    fn script_fallback(&self, _script: Script, _locale: &str) -> &[&'static str] {
        &[]
    }
}

impl SpikeRenderer {
    fn new(fonts: &[&'static [u8]]) -> Self {
        let mut database = fontdb::Database::new();
        for font_bytes in fonts {
            database.load_font_source(fontdb::Source::Binary(Arc::new((*font_bytes).to_vec())));
        }
        database.set_monospace_family("IBM Plex Mono");
        database.set_sans_serif_family("Inter Variable");
        Self {
            font_system: FontSystem::new_with_locale_and_db_and_fallback(
                "en-US".to_string(),
                database,
                EmbeddedFallback,
            ),
            glyph_cache: SwashCache::new(),
        }
    }

    fn render(
        &mut self,
        profile: &Profile<'_>,
        font_size: u16,
        width: usize,
        height: usize,
        text: &str,
    ) -> CoverageMask {
        let font_size = f32::from(font_size);
        let mut buffer = Buffer::new(
            &mut self.font_system,
            Metrics::new(font_size, font_size * 1.35),
        );
        let mut coverage = vec![0; width * height];
        let attrs = Attrs::new()
            .family(Family::Name(profile.family))
            .weight(profile.weight);

        let (glyph_count, missing_glyph_count, max_line_width, font_ids) = {
            let mut buffer = buffer.borrow_with(&mut self.font_system);
            buffer.set_size(Some(width as f32), Some(height as f32));
            buffer.set_wrap(Wrap::None);
            buffer.set_text(text, &attrs, Shaping::Advanced, None);

            let mut glyph_count = 0;
            let mut missing_glyph_count = 0;
            let mut max_line_width = 0.0_f32;
            let mut font_ids = BTreeSet::new();
            for run in buffer.layout_runs() {
                glyph_count += run.glyphs.len();
                missing_glyph_count += run
                    .glyphs
                    .iter()
                    .filter(|glyph| glyph.glyph_id == 0)
                    .count();
                font_ids.extend(run.glyphs.iter().map(|glyph| glyph.font_id));
                max_line_width = max_line_width.max(run.line_w);
            }

            buffer.draw(
                &mut self.glyph_cache,
                Color::rgb(0, 0, 0),
                |x, y, pixel_width, pixel_height, color| {
                    let alpha = color.a();
                    for offset_y in 0..pixel_height {
                        let Some(target_y) = y.checked_add_unsigned(offset_y) else {
                            continue;
                        };
                        let Ok(target_y) = usize::try_from(target_y) else {
                            continue;
                        };
                        if target_y >= height {
                            continue;
                        }
                        for offset_x in 0..pixel_width {
                            let Some(target_x) = x.checked_add_unsigned(offset_x) else {
                                continue;
                            };
                            let Ok(target_x) = usize::try_from(target_x) else {
                                continue;
                            };
                            if target_x >= width {
                                continue;
                            }
                            let destination = target_y * width + target_x;
                            coverage[destination] = coverage[destination].max(alpha);
                        }
                    }
                },
            );
            (glyph_count, missing_glyph_count, max_line_width, font_ids)
        };

        let font_families = font_ids
            .into_iter()
            .filter_map(|font_id| self.font_system.db().face(font_id))
            .filter_map(|face| face.families.first())
            .map(|(family, _language)| family.clone())
            .collect();

        CoverageMask {
            width,
            height,
            coverage,
            glyph_count,
            missing_glyph_count,
            max_line_width_millipixels: (max_line_width * 1000.0).round() as u32,
            font_families,
        }
    }
}

#[derive(Clone, Debug)]
struct Profile<'a> {
    name: &'a str,
    family: &'a str,
    weight: Weight,
    bytes: &'static [u8],
    font_sizes: &'a [u16],
}

fn main() -> Result<(), Box<dyn Error>> {
    let output_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".cache/typography-spike"));
    fs::create_dir_all(&output_dir)?;

    let profiles = [
        Profile {
            name: "ibm-plex-mono-medium",
            family: "IBM Plex Mono",
            weight: Weight::MEDIUM,
            bytes: IBM_PLEX_MONO_MEDIUM,
            font_sizes: &[14, 16, 18, 20, 22],
        },
        Profile {
            name: "inter-regular",
            family: "Inter Variable",
            weight: Weight::NORMAL,
            bytes: INTER_VARIABLE,
            font_sizes: &[28],
        },
    ];

    println!(
        "profile\tpx\tfont_bytes\tinit_us\tcold_us\twarm_mean_us\tglyphs\tmissing\tmax_line_px\tiiii_px\twwww_px\tav_kern_px\tcache_entries\trepeatable"
    );
    for profile in profiles {
        for &font_size in profile.font_sizes {
            run_profile(&output_dir, &profile, font_size)?;
        }
    }
    verify_embedded_fallback()?;
    Ok(())
}

fn verify_embedded_fallback() -> Result<(), Box<dyn Error>> {
    let profile = Profile {
        name: "ibm-plex-mono-with-inter-fallback",
        family: "IBM Plex Mono",
        weight: Weight::MEDIUM,
        bytes: IBM_PLEX_MONO_MEDIUM,
        font_sizes: &[18],
    };
    let mut renderer = SpikeRenderer::new(&[IBM_PLEX_MONO_MEDIUM, INTER_VARIABLE]);
    let fallback = renderer.render(&profile, 18, 64, 36, "Ω");
    if fallback.missing_glyph_count != 0 || fallback.font_families != ["Inter Variable".to_string()]
    {
        let available_families = renderer
            .font_system
            .db()
            .faces()
            .map(|face| face.families.clone())
            .collect::<Vec<_>>();
        return Err(format!(
            "embedded fallback probe expected Inter Variable, got {:?}; database faces: {:?}",
            fallback.font_families, available_families
        )
        .into());
    }
    println!("fallback-probe\tIBM Plex Mono Ω -> Inter Variable");
    Ok(())
}

fn run_profile(
    output_dir: &Path,
    profile: &Profile<'_>,
    font_size: u16,
) -> Result<(), Box<dyn Error>> {
    let height = usize::from(font_size) * 8;

    let started = Instant::now();
    let mut renderer = SpikeRenderer::new(&[profile.bytes]);
    let initialization = started.elapsed();

    let started = Instant::now();
    let cold = renderer.render(profile, font_size, SAMPLE_WIDTH, height, SAMPLE_TEXT);
    let cold_duration = started.elapsed();

    let repeat = renderer.render(profile, font_size, SAMPLE_WIDTH, height, SAMPLE_TEXT);
    let repeatable = cold == repeat;
    if cold.glyph_count == 0 {
        return Err(format!("{} {font_size}px shaped no glyphs", profile.name).into());
    }
    if cold.missing_glyph_count != 0 {
        return Err(format!(
            "{} {font_size}px produced {} missing glyphs",
            profile.name, cold.missing_glyph_count
        )
        .into());
    }
    if !repeatable {
        return Err(format!("{} {font_size}px repeat render differed", profile.name).into());
    }
    if !cold
        .coverage
        .iter()
        .any(|coverage| *coverage >= MONO_THRESHOLD)
    {
        return Err(format!(
            "{} {font_size}px produced no thresholded Mono1 ink",
            profile.name
        )
        .into());
    }
    if !cold
        .coverage
        .iter()
        .any(|coverage| (1..u8::MAX).contains(coverage))
    {
        return Err(format!(
            "{} {font_size}px produced no intermediate Gray8 coverage",
            profile.name
        )
        .into());
    }

    let iiii_width = measured_width(&mut renderer, profile, font_size, "iiii");
    let wwww_width = measured_width(&mut renderer, profile, font_size, "WWWW");
    let a_width = measured_width(&mut renderer, profile, font_size, "A");
    let v_width = measured_width(&mut renderer, profile, font_size, "V");
    let av_width = measured_width(&mut renderer, profile, font_size, "AV");
    let av_kerning = i64::from(a_width) + i64::from(v_width) - i64::from(av_width);
    if profile.family == "IBM Plex Mono" && iiii_width != wwww_width {
        return Err(format!(
            "{} {font_size}px did not preserve equal monospaced advances",
            profile.name
        )
        .into());
    }
    if profile.family == "Inter Variable" && iiii_width == wwww_width {
        return Err(format!(
            "{} {font_size}px did not produce proportional advances",
            profile.name
        )
        .into());
    }
    if profile.family == "Inter Variable" && av_kerning <= 0 {
        return Err(format!(
            "{} {font_size}px did not apply the expected AV kerning",
            profile.name
        )
        .into());
    }

    let started = Instant::now();
    for _ in 0..WARM_ITERATIONS {
        let _ = renderer.render(profile, font_size, SAMPLE_WIDTH, height, SAMPLE_TEXT);
    }
    let warm_mean = started.elapsed() / WARM_ITERATIONS;

    let stem = format!("{}-{font_size}px", profile.name);
    write_pbm(
        &output_dir.join(format!("{stem}-mono1.pbm")),
        &cold,
        MONO_THRESHOLD,
    )?;
    write_pgm(&output_dir.join(format!("{stem}-gray8.pgm")), &cold)?;

    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{}\t{}",
        profile.name,
        font_size,
        profile.bytes.len(),
        as_micros(initialization),
        as_micros(cold_duration),
        as_micros(warm_mean),
        cold.glyph_count,
        cold.missing_glyph_count,
        cold.max_line_width_millipixels as f64 / 1000.0,
        f64::from(iiii_width) / 1000.0,
        f64::from(wwww_width) / 1000.0,
        av_kerning as f64 / 1000.0,
        renderer.glyph_cache.image_cache.len(),
        repeatable,
    );
    Ok(())
}

fn measured_width(
    renderer: &mut SpikeRenderer,
    profile: &Profile<'_>,
    font_size: u16,
    text: &str,
) -> u32 {
    renderer
        .render(profile, font_size, 512, usize::from(font_size) * 2, text)
        .max_line_width_millipixels
}

fn as_micros(duration: Duration) -> u128 {
    duration.as_micros()
}

fn write_pgm(path: &Path, mask: &CoverageMask) -> Result<(), Box<dyn Error>> {
    let mut bytes = format!("P5\n{} {}\n255\n", mask.width, mask.height).into_bytes();
    bytes.extend(mask.coverage.iter().map(|coverage| u8::MAX - coverage));
    fs::write(path, bytes)?;
    Ok(())
}

fn write_pbm(path: &Path, mask: &CoverageMask, threshold: u8) -> Result<(), Box<dyn Error>> {
    let stride = mask.width.div_ceil(8);
    let mut header = String::new();
    write!(&mut header, "P4\n{} {}\n", mask.width, mask.height)?;
    let mut bytes = header.into_bytes();
    bytes.resize(bytes.len() + stride * mask.height, 0);
    let pixel_offset = bytes.len() - stride * mask.height;

    for y in 0..mask.height {
        for x in 0..mask.width {
            if mask.coverage[y * mask.width + x] < threshold {
                continue;
            }
            let byte = pixel_offset + y * stride + x / 8;
            bytes[byte] |= 0x80 >> (x % 8);
        }
    }
    fs::write(path, bytes)?;
    Ok(())
}
