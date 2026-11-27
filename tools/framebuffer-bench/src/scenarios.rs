use std::io::BufWriter;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use image::DynamicImage;
use paper_protocol::{Frame, PixelFormat, V2Pointer, V2PointerPhase};
use paperspoon::application_input::ApplicationInput;
use paperspoon::application_renderer::ApplicationRenderer;
use paperspoon::application_ui::ApplicationUi;
use paperspoon::now_playing_provider::{
    FakeChange, FakeMusic, MediaProvider, Playback, RepeatMode,
};
use paperspoon::now_playing_renderer::LiveNowPlayingRenderer;

use crate::corpus::{Header, Layout, Sample, write_corpus};

const VIEWPORT: (u16, u16) = (1272, 1624);
const FIXTURE: (u16, u16) = (67, 37);
const TICK_US: u64 = 1_000_000;

pub const NAMES: &[&str] = &[
    "now-playing",
    "application-grid",
    "motion-mono-4hz",
    "motion-mono-10hz",
    "motion-gray-4hz",
    "motion-gray-10hz",
    "synthetic-mono",
    "synthetic-gray",
];

pub fn generate(name: &str, path: &Path) -> Result<()> {
    let (format, size, frame_count) = match name {
        "now-playing" => (PixelFormat::Gray8, VIEWPORT, 129),
        "application-grid" => (PixelFormat::Mono1, VIEWPORT, 8),
        "motion-mono-4hz" => (PixelFormat::Mono1, VIEWPORT, 8),
        "motion-mono-10hz" => (PixelFormat::Mono1, VIEWPORT, 20),
        "motion-gray-4hz" => (PixelFormat::Gray8, VIEWPORT, 8),
        "motion-gray-10hz" => (PixelFormat::Gray8, VIEWPORT, 20),
        "synthetic-mono" => (PixelFormat::Mono1, FIXTURE, 11),
        "synthetic-gray" => (PixelFormat::Gray8, FIXTURE, 11),
        _ => bail!(
            "unknown scenario {name:?}, choose from {}",
            NAMES.join(", ")
        ),
    };
    let layout = Layout::new(format, size.0, size.1)?;
    let header = Header {
        scenario: name.into(),
        layout,
        frame_count,
    };
    let mut staged = crate::stage_output(path)?;
    let output = BufWriter::new(staged.as_file_mut());
    match name {
        "now-playing" => now_playing(output, &header)?,
        "application-grid" => application_grid(output, &header)?,
        "synthetic-mono" | "synthetic-gray" => write_corpus(output, &header, |index| {
            Ok(Sample {
                timestamp_us: u64::from(index) * TICK_US,
                frame: synthetic(layout, index)?,
            })
        })?,
        _ => {
            let fps = frame_count / 2;
            write_corpus(output, &header, |index| {
                Ok(Sample {
                    timestamp_us: u64::from(index) * (TICK_US / u64::from(fps)),
                    frame: motion(layout, index, fps)?,
                })
            })?;
        }
    }
    crate::save_output(staged, path)?;
    println!("generated {frame_count} frames in {}", path.display());
    Ok(())
}

fn now_playing(output: impl std::io::Write, header: &Header) -> Result<()> {
    let mut renderer = LiveNowPlayingRenderer::new().map_err(|error| anyhow!(error))?;
    let mut provider = FakeMusic::default();
    let anchor = Instant::now();
    provider.change(FakeChange::Seek(12.0), anchor);
    let cancel = AtomicBool::new(false);
    let mut artwork = None;
    let mut artwork_track = None;
    let mut artwork_revision = 0;
    write_corpus(output, header, |index| {
        let now = anchor + Duration::from_secs(u64::from(index));
        match index {
            121 => provider.change(FakeChange::Seek(172.0), now),
            122 => provider.change(FakeChange::Seek(42.0), now),
            123 => provider.change(FakeChange::Pause, now),
            125 => provider.change(FakeChange::Play, now),
            126 => provider.change(FakeChange::Next, now),
            _ => {}
        }
        let mut playback = provider.sample_at(now).map_err(|error| anyhow!(error))?;
        if let Playback::Playing(track) = &mut playback {
            if index >= 127 {
                track.shuffle = Some(true);
                track.repeat = Some(RepeatMode::All);
            }
            if index == 128 {
                track.title = "A long track title that exercises wrapping, clipping and ellipsizing across the complete Now Playing header".repeat(3);
                track.artist = Some(
                    "An artist with an intentionally long name for the metadata transition"
                        .repeat(3),
                );
            }
        }
        let track_id = playback.track().and_then(|track| track.id.as_deref());
        if track_id != artwork_track.as_deref() {
            artwork_track = track_id.map(str::to_owned);
            artwork = match track_id {
                Some(id) => provider
                    .artwork(id, &cancel)
                    .map_err(|error| anyhow!(error))?
                    .map(DynamicImage::ImageLuma8),
                None => None,
            };
            artwork_revision += 1;
        }
        let frame = renderer
            .render(
                VIEWPORT,
                &playback,
                artwork.as_ref(),
                artwork_revision,
                None,
                now,
            )
            .map_err(|error| anyhow!(error))?;
        Ok(Sample {
            timestamp_us: u64::from(index) * TICK_US,
            frame: Frame::Gray8(frame),
        })
    })
}

fn application_grid(output: impl std::io::Write, header: &Header) -> Result<()> {
    let mut renderer = ApplicationRenderer::new().map_err(|error| anyhow!(error))?;
    let mut ui = ApplicationUi::default();
    let mut input = ApplicationInput::default();
    input.set_viewport(&mut ui, Some(VIEWPORT));
    let buttons = ui
        .layout(VIEWPORT.0, VIEWPORT.1)
        .context("application viewport has no layout")?
        .buttons;
    let pointer = |phase, index: usize| {
        let bounds = buttons[index].bounds;
        V2Pointer::new(
            phase,
            bounds.x + bounds.width / 2,
            bounds.y + bounds.height / 2,
        )
    };
    write_corpus(output, header, |index| {
        let frame = match index {
            5 => {
                input.set_viewport(&mut ui, None);
                header.layout.frame(vec![u8::MAX; header.layout.len])?
            }
            6 => header.layout.frame(vec![0; header.layout.len])?,
            _ => {
                match index {
                    1 => {
                        input.handle_pointer(&mut ui, pointer(V2PointerPhase::Down, 0));
                    }
                    2 => {
                        input.handle_pointer(&mut ui, pointer(V2PointerPhase::Up, 0));
                    }
                    3 => {
                        input.handle_pointer(&mut ui, pointer(V2PointerPhase::Down, 8));
                        ui.set_status("Application selection");
                    }
                    4 => {
                        input.handle_pointer(&mut ui, pointer(V2PointerPhase::Up, 8));
                    }
                    7 => {
                        ui.clear_status();
                        input.set_viewport(&mut ui, Some(VIEWPORT));
                    }
                    _ => {}
                }
                Frame::Mono1(
                    renderer
                        .render_bounded(&ui, VIEWPORT.0, VIEWPORT.1)
                        .map_err(|error| anyhow!(error))?,
                )
            }
        };
        Ok(Sample {
            timestamp_us: u64::from(index) * 250_000,
            frame,
        })
    })
}

fn motion(layout: Layout, index: u32, fps: u32) -> Result<Frame> {
    let phase = index % fps;
    let scene_cut = index >= fps;
    let x_offset = 24 + phase as usize * 17;
    let y_offset = 58 + phase as usize * 23;
    raster(layout, |x, y| {
        let sprite =
            (x_offset..x_offset + 160).contains(&x) && (y_offset..y_offset + 300).contains(&y);
        if scene_cut {
            if sprite {
                245
            } else {
                ((x * 3 + y * 5 + phase as usize * 27) % 256) as u8
            }
        } else if sprite {
            15
        } else {
            ((x / 5 + y / 7) % 192 + 32) as u8
        }
    })
}

fn synthetic(layout: Layout, index: u32) -> Result<Frame> {
    let mut noise = 0x7265_7374_u32;
    raster(layout, |x, y| match index {
        0 | 1 | 10 => 255,
        2 => {
            if (x, y) == (66, 36) {
                0
            } else {
                255
            }
        }
        3 => {
            if y == 3 {
                0
            } else {
                255
            }
        }
        4 => {
            if (2..9).contains(&x) && (3..8).contains(&y) {
                0
            } else {
                255
            }
        }
        5 => {
            if ((2..9).contains(&x) && (3..8).contains(&y)) || (x >= 63 && y >= 33) {
                0
            } else {
                255
            }
        }
        6 => {
            if (x + y) % 2 == 0 {
                0
            } else {
                255
            }
        }
        7 => (x * 255 / (usize::from(layout.width) - 1)) as u8,
        8 => {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            (noise >> 24) as u8
        }
        _ => 0,
    })
}

fn raster(layout: Layout, mut luma: impl FnMut(usize, usize) -> u8) -> Result<Frame> {
    let mut pixels = vec![0; layout.len];
    for (y, row) in pixels.chunks_exact_mut(layout.stride).enumerate() {
        match layout.format {
            PixelFormat::Gray8 => {
                for (x, pixel) in row.iter_mut().enumerate() {
                    *pixel = luma(x, y);
                }
            }
            PixelFormat::Mono1 => {
                for x in 0..usize::from(layout.width) {
                    row[x / 8] |= u8::from(luma(x, y) < 128) << (7 - x % 8);
                }
            }
        }
    }
    layout.frame(pixels)
}
