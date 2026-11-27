use super::{MediaProvider, Playback, PlaybackCommand, RepeatMode, Track};
use image::{GrayImage, ImageReader, Limits, imageops::FilterType};
use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const SCRIPT: &str = include_str!("apple_music.applescript");
const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_TEXT_BYTES: u64 = 64 * 1024;
const MAX_ARTWORK_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Default)]
pub struct AppleMusic;

impl MediaProvider for AppleMusic {
    fn name(&self) -> &'static str {
        "Apple Music"
    }

    fn sample(&mut self, cancel: &AtomicBool) -> Result<Playback, String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let response = run_script(&["sample"], temp.path(), cancel)?;
        parse_sample(&response, Instant::now())
    }

    fn command(
        &mut self,
        track_id: &str,
        command: PlaybackCommand,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let seek_ms;
        let args = match command {
            PlaybackCommand::RevealTrack => vec!["reveal", track_id],
            PlaybackCommand::Previous => vec!["previous", track_id],
            PlaybackCommand::PlayPause => vec!["playpause", track_id],
            PlaybackCommand::Next => vec!["next", track_id],
            PlaybackCommand::ToggleShuffle => vec!["shuffle", track_id],
            PlaybackCommand::CycleRepeat => vec!["repeat", track_id],
            PlaybackCommand::Seek(position) if position.is_finite() && position >= 0.0 => {
                seek_ms = ((position * 1000.0).round() as u64).to_string();
                vec!["seek", track_id, &seek_ms]
            }
            PlaybackCommand::Seek(_) => return Err("invalid seek position".into()),
        };
        let response = run_script(&args, temp.path(), cancel)?;
        match response.as_str() {
            "ok" => Ok(()),
            "revealed" if command == PlaybackCommand::RevealTrack => Ok(()),
            _ => Err(format!("Music command rejected: {response}")),
        }
    }

    fn artwork(
        &mut self,
        track_id: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<GrayImage>, String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = temp.path().join("artwork");
        let path_text = path.to_str().ok_or("non-UTF-8 artwork path")?;
        let response = run_script(&["artwork", track_id, path_text], temp.path(), cancel)?;
        match response.as_str() {
            "missing" => return Ok(None),
            "ok" => {}
            _ => return Err(format!("Music artwork unavailable: {response}")),
        }
        if fs::metadata(&path)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_ARTWORK_BYTES
        {
            return Err("Music artwork exceeds the encoded size limit".into());
        }
        let mut reader = ImageReader::open(path)
            .and_then(ImageReader::with_guessed_format)
            .map_err(|error| format!("opening Music artwork: {error}"))?;
        if !matches!(
            reader.format(),
            Some(image::ImageFormat::Jpeg | image::ImageFormat::Png)
        ) {
            return Err("Music artwork format is neither JPEG nor PNG".into());
        }
        let mut limits = Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let artwork = reader
            .decode()
            .map_err(|error| format!("decoding Music artwork: {error}"))?;
        let artwork = if artwork.width().max(artwork.height()) > 768 {
            artwork.resize(768, 768, FilterType::Triangle)
        } else {
            artwork
        };
        Ok(Some(artwork.into_luma8()))
    }
}

fn run_script(args: &[&str], directory: &Path, cancel: &AtomicBool) -> Result<String, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Music operation cancelled".into());
    }
    let stdout_path = directory.join("stdout");
    let stderr_path = directory.join("stderr");
    let stdout = File::create(&stdout_path).map_err(|error| error.to_string())?;
    let stderr = File::create(&stderr_path).map_err(|error| error.to_string())?;
    let mut child = Command::new("/usr/bin/osascript")
        .args(["-e", SCRIPT])
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .map_err(|error| format!("launching osascript: {error}"))?;
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if cancel.load(Ordering::Relaxed) => {
                break Err("Music operation cancelled".into());
            }
            Ok(None) if Instant::now() >= deadline => break Err("Music operation timed out".into()),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => break Err(format!("waiting for osascript: {error}")),
        }
    };
    let status = status.inspect_err(|_| {
        let _ = child.kill();
        let _ = child.wait();
    })?;
    if !status.success() {
        return Err(format!("osascript {status}: {}", read_text(&stderr_path)?));
    }
    read_text(&stdout_path)
}

fn read_text(path: &Path) -> Result<String, String> {
    use std::io::Read as _;
    let file = File::open(path).map_err(|error| error.to_string())?;
    let mut text = String::new();
    file.take(MAX_TEXT_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.len() as u64 > MAX_TEXT_BYTES {
        return Err("Music response exceeds the size limit".into());
    }
    Ok(text.trim_end_matches(['\n', '\r']).to_string())
}

fn parse_sample(response: &str, sampled_at: Instant) -> Result<Playback, String> {
    match response {
        "unavailable" => return Ok(Playback::Unavailable),
        "stopped" => return Ok(Playback::Stopped),
        _ => {}
    }
    let fields: Vec<_> = response.split('\u{1f}').collect();
    let &[
        state,
        id,
        title,
        artist,
        album,
        position,
        duration,
        shuffle,
        repeat,
    ] = fields.as_slice()
    else {
        return Err("invalid Music snapshot".into());
    };
    let optional = |value: &str| {
        (!value.trim().is_empty()).then(|| value.trim().chars().take(512).collect::<String>())
    };
    let number = |value: &str| {
        value
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
    };
    let track = Track {
        id: optional(id),
        title: optional(title).unwrap_or_else(|| "Unknown track".into()),
        artist: optional(artist),
        album: optional(album),
        position: number(position).filter(|value| *value >= 0.0),
        duration: number(duration).filter(|value| *value > 0.0),
        shuffle: match shuffle {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        },
        repeat: match repeat {
            "off" => Some(RepeatMode::Off),
            "all" => Some(RepeatMode::All),
            "one" => Some(RepeatMode::One),
            _ => None,
        },
        rate: if state == "playing" { 1.0 } else { 0.0 },
        sampled_at,
    };
    match state {
        "playing" => Ok(Playback::Playing(track)),
        "paused" => Ok(Playback::Paused(track)),
        "fast forwarding" | "rewinding" => Ok(Playback::Seeking(track)),
        _ => Err(format!("unrecognized Music player state: {state}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_optional_metadata_and_localized_numbers() {
        let now = Instant::now();
        let sample = parse_sample(
            "playing\x1fpid:1\x1fA title\x1f\x1fAlbum\x1f2,5\x1f180\x1fon\x1fone",
            now,
        )
        .unwrap();
        assert_eq!(sample.position(now), Some(2.5));
        let track = sample.controllable_track().unwrap();
        assert_eq!(track.duration(), Some(180.0));
        assert_eq!(track.artist, None);
        assert_eq!(track.shuffle, Some(true));
        assert_eq!(track.repeat, Some(RepeatMode::One));
    }

    #[test]
    fn distinguishes_unavailable_stopped_and_failed_acquisition() {
        let now = Instant::now();
        assert!(matches!(
            parse_sample("unavailable", now),
            Ok(Playback::Unavailable)
        ));
        assert!(matches!(
            parse_sample("stopped", now),
            Ok(Playback::Stopped)
        ));
        assert!(parse_sample("partial\x1fsnapshot", now).is_err());
        assert!(
            parse_sample("paused\x1f\x1fTrack\x1f\x1f\x1fNaN\x1f0\x1f\x1f", now)
                .unwrap()
                .controllable_track()
                .is_none()
        );
        let sample =
            parse_sample("paused\x1fpid:2\x1fTrack\x1f\x1f\x1fNaN\x1f0\x1f\x1f", now).unwrap();
        assert_eq!(sample.position(now), None);
        let track = sample.controllable_track().unwrap();
        assert_eq!(track.duration(), None);
        assert_eq!(track.shuffle, None);
        assert_eq!(track.repeat, None);
    }
}
