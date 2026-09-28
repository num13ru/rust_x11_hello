//! One-shot macOS Now Playing acquisition isolated from PaperSpoon state.
//!
//! `NowPlayingPerl` does not synchronously join or kill its stream child on
//! `Drop`. PaperSpoon therefore runs each acquisition in a short-lived process
//! group and terminates that whole group after receiving one rendered frame.

use image::DynamicImage;
use paper_protocol::Gray8Frame;

pub const HELPER_ARGUMENT: &str = "--now-playing-helper";

#[cfg(target_os = "macos")]
const MAX_METADATA_CHARS: usize = 512;
#[cfg(target_os = "macos")]
const MAX_ARTWORK_BYTES: usize = 256 * 1024 * 1024;
#[cfg(target_os = "macos")]
const INITIAL_SNAPSHOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(target_os = "macos")]
const ARTWORK_GRACE_PERIOD: std::time::Duration = std::time::Duration::from_millis(400);
#[cfg(target_os = "macos")]
const HELPER_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);
const HELPER_MAGIC: [u8; 4] = *b"PNP1";
const HELPER_HEADER_LEN: usize = 9;
const HELPER_ERROR_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct NowPlayingSnapshot {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub is_playing: Option<bool>,
    pub elapsed_time: Option<f64>,
    pub duration: Option<f64>,
    pub bundle_id: Option<String>,
    pub bundle_name: Option<String>,
    pub artwork: Option<DynamicImage>,
}

/// Acquire and render through an isolated helper whose process group includes
/// the Perl adapter. This function returns only after that group is gone.
#[cfg(target_os = "macos")]
pub fn load_now_playing_frame(width: u16, height: u16) -> Result<Gray8Frame, String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    let expected_pixels = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or_else(|| "Now Playing frame dimensions overflow".to_string())?;
    // Validate protocol dimensions and payload cap before launching MediaRemote.
    let validation = crate::gray_renderer::GrayCanvas::new(width, height)?;
    drop(validation);

    let executable = std::env::current_exe()
        .map_err(|error| format!("failed to locate PaperSpoon executable: {error}"))?;
    let mut child = Command::new(executable)
        .arg(HELPER_ARGUMENT)
        .arg(width.to_string())
        .arg(height.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("failed to start Now Playing helper: {error}"))?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_process_group(&mut child)?;
            return Err("Now Playing helper stdout was not captured".to_string());
        }
    };
    let (response_tx, response_rx) = mpsc::sync_channel(1);
    let reader_result = std::thread::Builder::new()
        .name("paperspoon-now-playing-response".to_string())
        .spawn(move || {
            let mut stdout = stdout;
            let mut header = [0_u8; HELPER_HEADER_LEN];
            let response = stdout
                .read_exact(&mut header)
                .map_err(|error| format!("failed to read Now Playing helper header: {error}"))
                .and_then(|()| decode_helper_header(header, expected_pixels))
                .and_then(|(status, length)| {
                    let mut payload = Vec::new();
                    payload.try_reserve_exact(length).map_err(|error| {
                        format!("failed to allocate Now Playing helper response: {error}")
                    })?;
                    payload.resize(length, 0);
                    stdout.read_exact(&mut payload).map_err(|error| {
                        format!("failed to read Now Playing helper payload: {error}")
                    })?;
                    decode_helper_payload(status, payload)
                });
            let _ = response_tx.send(response);
        });
    let reader = match reader_result {
        Ok(reader) => reader,
        Err(error) => {
            terminate_process_group(&mut child)?;
            return Err(format!(
                "failed to start Now Playing response reader: {error}"
            ));
        }
    };

    let response = response_rx
        .recv_timeout(HELPER_RESPONSE_TIMEOUT)
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => {
                "Now Playing acquisition timed out before a usable snapshot arrived".to_string()
            }
            mpsc::RecvTimeoutError::Disconnected => {
                "Now Playing helper response reader stopped unexpectedly".to_string()
            }
        });
    let cleanup = terminate_process_group(&mut child);
    let reader_result = reader.join();
    cleanup?;
    if reader_result.is_err() && response.is_ok() {
        return Err("Now Playing response reader panicked".to_string());
    }
    let pixels = response??;
    Gray8Frame::new(width, height, pixels)
        .map_err(|error| format!("failed to build Now Playing Gray8 frame: {error}"))
}

#[cfg(not(target_os = "macos"))]
pub fn load_now_playing_frame(_width: u16, _height: u16) -> Result<Gray8Frame, String> {
    Err("frame nowplaying is supported only on macOS".to_string())
}

/// Internal helper entry point. The parent intentionally terminates this
/// process group after reading the response, which also stops the Perl child.
pub fn run_helper(width: u16, height: u16) -> ! {
    use std::io::Write;

    let response = acquire_snapshot().and_then(|snapshot| {
        crate::now_playing_renderer::render_now_playing(&snapshot, width, height)
    });
    let (status, payload) = match response {
        Ok(frame) => (0_u8, frame.pixels().to_vec()),
        Err(error) => (1_u8, bounded_error_bytes(&error)),
    };
    let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&HELPER_MAGIC);
    let _ = stdout.write_all(&[status]);
    let _ = stdout.write_all(&length.to_be_bytes());
    let _ = stdout.write_all(&payload);
    let _ = stdout.flush();

    // Keep the group alive until the parent has consumed the complete response
    // and can terminate both this process and the adapter deterministically.
    loop {
        std::thread::park();
    }
}

#[cfg(target_os = "macos")]
fn acquire_snapshot() -> Result<NowPlayingSnapshot, String> {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::time::{Duration, Instant};

    use media_remote::NowPlayingPerl;

    let provider = catch_unwind(AssertUnwindSafe(NowPlayingPerl::new))
        .map_err(|_| "MediaRemote adapter failed to initialize".to_string())?;
    let started = Instant::now();
    let initial_deadline = started + INITIAL_SNAPSHOT_TIMEOUT;
    let overall_deadline = initial_deadline + ARTWORK_GRACE_PERIOD;
    let mut artwork_deadline = None;
    let mut best = None;

    loop {
        let candidate = {
            let info = provider.get_info();
            info.as_ref().and_then(snapshot_from_info)
        };
        if let Some(candidate) = candidate {
            if candidate.artwork.is_some() {
                return Ok(candidate);
            }
            best = Some(candidate);
            artwork_deadline.get_or_insert_with(|| {
                (Instant::now() + ARTWORK_GRACE_PERIOD).min(overall_deadline)
            });
        }

        let now = Instant::now();
        if artwork_deadline.is_some_and(|deadline| now >= deadline) {
            return best.ok_or_else(|| "no usable Now Playing metadata arrived".to_string());
        }
        if now >= initial_deadline && best.is_none() {
            return Err("no usable Now Playing state arrived before the 2s deadline".to_string());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(target_os = "macos"))]
fn acquire_snapshot() -> Result<NowPlayingSnapshot, String> {
    Err("frame nowplaying is supported only on macOS".to_string())
}

#[cfg(target_os = "macos")]
fn snapshot_from_info(info: &media_remote::NowPlayingInfo) -> Option<NowPlayingSnapshot> {
    let title = bounded_required(info.title.as_deref())?;
    let artwork = info.album_cover.as_ref().and_then(|artwork| {
        (artwork.as_bytes().len() <= MAX_ARTWORK_BYTES).then(|| artwork.clone())
    });
    Some(NowPlayingSnapshot {
        title,
        artist: bounded_optional(info.artist.as_deref()),
        album: bounded_optional(info.album.as_deref()),
        is_playing: info.is_playing,
        elapsed_time: info.elapsed_time,
        duration: info.duration,
        bundle_id: bounded_optional(info.bundle_id.as_deref()),
        bundle_name: bounded_optional(info.bundle_name.as_deref()),
        artwork,
    })
}

#[cfg(target_os = "macos")]
fn bounded_required(value: Option<&str>) -> Option<String> {
    bounded_optional(value).filter(|value| !value.is_empty())
}

#[cfg(target_os = "macos")]
fn bounded_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(MAX_METADATA_CHARS).collect())
}

fn decode_helper_header(
    header: [u8; HELPER_HEADER_LEN],
    expected_pixels: usize,
) -> Result<(u8, usize), String> {
    if header[..4] != HELPER_MAGIC {
        return Err("Now Playing helper returned an invalid response magic".to_string());
    }
    let status = header[4];
    let length = u32::from_be_bytes(header[5..9].try_into().expect("four length bytes")) as usize;
    match status {
        0 if length == expected_pixels => Ok((status, length)),
        0 => Err(format!(
            "Now Playing helper returned {length} pixels; expected {expected_pixels}"
        )),
        1 if length <= HELPER_ERROR_LIMIT => Ok((status, length)),
        1 => Err("Now Playing helper error response exceeds size limit".to_string()),
        _ => Err(format!(
            "Now Playing helper returned invalid status {status}"
        )),
    }
}

fn decode_helper_payload(status: u8, payload: Vec<u8>) -> Result<Vec<u8>, String> {
    if status == 0 {
        return Ok(payload);
    }
    let message = String::from_utf8(payload)
        .map_err(|_| "Now Playing helper returned a non-UTF-8 error".to_string())?;
    Err(message)
}

fn bounded_error_bytes(error: &str) -> Vec<u8> {
    let mut output = String::new();
    for character in error.chars() {
        if output.len() + character.len_utf8() > HELPER_ERROR_LIMIT {
            break;
        }
        output.push(character);
    }
    output.into_bytes()
}

#[cfg(target_os = "macos")]
fn terminate_process_group(child: &mut std::process::Child) -> Result<(), String> {
    use std::io;
    use std::time::{Duration, Instant};

    let process_group = i32::try_from(child.id())
        .map_err(|_| "Now Playing helper process id does not fit i32".to_string())?;
    signal_process_group(process_group, "TERM")?;
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut child_reaped = false;
    while Instant::now() < deadline {
        if child
            .try_wait()
            .map_err(|error| format!("failed to inspect Now Playing helper: {error}"))?
            .is_some()
        {
            child_reaped = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !child_reaped {
        signal_process_group(process_group, "KILL")?;
        child
            .wait()
            .map_err(|error| format!("failed to reap Now Playing helper: {error}"))?;
    }

    // Once the group leader is reaped, any remaining group member is the Perl
    // adapter (or one of its descendants), not a zombie helper.
    if process_group_exists(process_group)? {
        signal_process_group(process_group, "KILL")?;
    }

    let gone_deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < gone_deadline && process_group_exists(process_group)? {
        std::thread::sleep(Duration::from_millis(10));
    }
    if process_group_exists(process_group)? {
        return Err("Now Playing helper process group did not terminate".to_string());
    }
    return Ok(());

    fn signal_process_group(process_group: i32, signal: &str) -> Result<(), String> {
        // Separate argv entries avoid a shell; the validated negative id targets
        // only the dedicated group created for this helper.
        let output = std::process::Command::new("/bin/kill")
            .arg(format!("-{signal}"))
            .arg("--")
            .arg(format!("-{process_group}"))
            .output()
            .map_err(|error| format!("failed to launch /bin/kill for helper group: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !process_group_exists(process_group)? {
            Ok(())
        } else {
            Err(format!(
                "failed to signal Now Playing helper group with {signal}: {}",
                stderr.trim()
            ))
        }
    }

    fn process_group_exists(process_group: i32) -> Result<bool, String> {
        // SAFETY: Signal 0 performs an existence/permission check only.
        let result = unsafe { libc::kill(-process_group, 0) };
        if result == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ESRCH) => Ok(false),
            Some(libc::EPERM) => Ok(true),
            _ => Err(format!(
                "failed to inspect Now Playing helper group: {error}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_protocol_requires_exact_bounded_payloads() {
        let mut header = [0_u8; HELPER_HEADER_LEN];
        header[..4].copy_from_slice(&HELPER_MAGIC);
        header[4] = 0;
        header[5..].copy_from_slice(&8_u32.to_be_bytes());
        assert_eq!(decode_helper_header(header, 8), Ok((0, 8)));
        assert!(decode_helper_header(header, 7).is_err());

        header[4] = 1;
        header[5..].copy_from_slice(&12_u32.to_be_bytes());
        assert_eq!(decode_helper_header(header, 8), Ok((1, 12)));
        assert_eq!(
            decode_helper_payload(1, b"adapter error".to_vec()),
            Err("adapter error".to_string())
        );
    }

    #[test]
    fn helper_errors_are_utf8_and_bounded() {
        let error = "x".repeat(HELPER_ERROR_LIMIT + 100);
        let bytes = bounded_error_bytes(&error);
        assert_eq!(bytes.len(), HELPER_ERROR_LIMIT);
        assert!(String::from_utf8(bytes).is_ok());
    }
}
