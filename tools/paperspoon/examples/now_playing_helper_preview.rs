//! Exercise the production Now Playing helper boundary and write a Gray8 PGM.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use paperspoon::now_playing::{HELPER_ARGUMENT, load_now_playing_frame, run_preview_helper};

const WIDTH: u16 = 1272;
const HEIGHT: u16 = 1624;

fn main() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).map(String::as_str) == Some(HELPER_ARGUMENT) {
        let width = args.get(2).ok_or("missing helper width")?.parse::<u16>()?;
        let height = args.get(3).ok_or("missing helper height")?.parse::<u16>()?;
        run_preview_helper(width, height);
    }

    let output = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".cache/typography-spike/now-playing-1272x1624.pgm"));
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let started = Instant::now();
    let frame = load_now_playing_frame(WIDTH, HEIGHT)?;
    let mut bytes = format!("P5\n{} {}\n255\n", frame.width(), frame.height()).into_bytes();
    bytes.extend_from_slice(frame.pixels());
    fs::write(&output, bytes)?;
    eprintln!(
        "wrote {} in {} us",
        output.display(),
        started.elapsed().as_micros()
    );
    Ok(())
}
