//! Host-side playback samples and commands for the live widget.

use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use image::GrayImage;

#[cfg(target_os = "macos")]
mod apple_music;
#[cfg(target_os = "macos")]
pub use apple_music::AppleMusic;

const MAX_EXTRAPOLATION: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: Option<String>,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub shuffle: Option<bool>,
    pub repeat: Option<RepeatMode>,
    pub rate: f64,
    pub sampled_at: Instant,
}

impl Track {
    pub fn duration(&self) -> Option<f64> {
        self.duration
            .filter(|value| value.is_finite() && *value > 0.0)
    }

    fn clamp_position(&self, position: f64) -> Option<f64> {
        (position.is_finite() && position >= 0.0).then(|| {
            self.duration()
                .map_or(position, |duration| position.min(duration))
        })
    }
}

#[derive(Clone, Debug)]
pub enum Playback {
    Unavailable,
    Stopped,
    Playing(Track),
    Paused(Track),
    Seeking(Track),
    Stale(Option<Track>),
}

impl Playback {
    pub fn track(&self) -> Option<&Track> {
        match self {
            Self::Playing(track) | Self::Paused(track) | Self::Seeking(track) => Some(track),
            Self::Stale(track) => track.as_ref(),
            Self::Unavailable | Self::Stopped => None,
        }
    }

    pub fn controllable_track(&self) -> Option<&Track> {
        match self {
            Self::Playing(track) | Self::Paused(track) if track.id.is_some() => Some(track),
            _ => None,
        }
    }

    pub fn position(&self, now: Instant) -> Option<f64> {
        let track = self.track()?;
        let sampled = track.clamp_position(track.position?)?;
        let elapsed = now.saturating_duration_since(track.sampled_at);
        match self {
            Self::Playing(_) if track.rate.is_finite() && track.rate > 0.0 => track.clamp_position(
                sampled + elapsed.min(MAX_EXTRAPOLATION).as_secs_f64() * track.rate,
            ),
            _ => Some(sampled),
        }
    }

    pub fn stale(&self, now: Instant) -> Self {
        Self::Stale(self.track().cloned().map(|mut track| {
            track.position = self.position(now);
            track
        }))
    }

    pub fn progress_active(&self, now: Instant) -> bool {
        let Self::Playing(track) = self else {
            return false;
        };
        now.saturating_duration_since(track.sampled_at) < MAX_EXTRAPOLATION
            && track.rate.is_finite()
            && track.rate > 0.0
            && self
                .position(now)
                .is_some_and(|position| track.duration().is_none_or(|duration| position < duration))
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Unavailable => "MUSIC UNAVAILABLE",
            Self::Stopped => "STOPPED",
            Self::Playing(_) => "PLAYING",
            Self::Paused(_) => "PAUSED",
            Self::Seeking(_) => "SEEKING",
            Self::Stale(_) => "PLAYBACK STALE",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlaybackCommand {
    Previous,
    PlayPause,
    Next,
    Seek(f64),
    ToggleShuffle,
    CycleRepeat,
}

pub trait MediaProvider: Send {
    fn name(&self) -> &'static str;
    fn sample(&mut self, cancel: &AtomicBool) -> Result<Playback, String>;
    fn simulate(&mut self, _change: FakeChange, _now: Instant) -> Result<(), String> {
        Err("playback simulation requires the fake provider".into())
    }
    fn command(
        &mut self,
        track_id: &str,
        command: PlaybackCommand,
        cancel: &AtomicBool,
    ) -> Result<(), String>;
    fn artwork(&mut self, track_id: &str, cancel: &AtomicBool)
    -> Result<Option<GrayImage>, String>;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FakeChange {
    Play,
    Pause,
    Next,
    Seek(f64),
    Fail,
    Recover,
    Slow,
    Normal,
}

#[derive(Debug)]
pub struct FakeMusic {
    index: usize,
    playing: bool,
    failed: bool,
    slow: bool,
    shuffle: bool,
    repeat: RepeatMode,
    position: f64,
    anchor: Instant,
}

impl Default for FakeMusic {
    fn default() -> Self {
        Self {
            index: 0,
            playing: true,
            failed: false,
            slow: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            position: 12.0,
            anchor: Instant::now(),
        }
    }
}

impl FakeMusic {
    const IDS: [&str; 3] = ["first", "second", "stream"];

    pub fn change(&mut self, change: FakeChange, now: Instant) {
        self.position = self.position_at(now);
        self.anchor = now;
        match change {
            FakeChange::Play => self.playing = true,
            FakeChange::Pause => self.playing = false,
            FakeChange::Next => {
                self.index = (self.index + 1) % Self::IDS.len();
                self.position = 0.0;
            }
            FakeChange::Seek(position) if position.is_finite() => {
                self.position = position.clamp(0.0, self.duration().unwrap_or(f64::MAX));
            }
            FakeChange::Seek(_) => {}
            FakeChange::Fail => self.failed = true,
            FakeChange::Recover => self.failed = false,
            FakeChange::Slow => self.slow = true,
            FakeChange::Normal => self.slow = false,
        }
    }

    fn duration(&self) -> Option<f64> {
        [Some(180.0), Some(240.0), None][self.index]
    }

    fn position_at(&self, now: Instant) -> f64 {
        let elapsed = if self.playing {
            now.saturating_duration_since(self.anchor).as_secs_f64()
        } else {
            0.0
        };
        (self.position + elapsed).min(self.duration().unwrap_or(f64::MAX))
    }

    pub fn sample_at(&self, now: Instant) -> Result<Playback, String> {
        if self.failed {
            return Err("simulated player failure".into());
        }
        let track = Track {
            id: Some(Self::IDS[self.index].into()),
            title: [
                "First Light",
                "A long track title for checking the bounds of the Now Playing widget on narrow viewports",
                "Live Studio Session",
            ][self.index].into(),
            artist: Some("PaperSpoon Demo".into()),
            album: Some(["Morning", "Evening Sessions", "Live Radio"][self.index].into()),
            position: Some(self.position_at(now)),
            duration: self.duration(),
            shuffle: Some(self.shuffle),
            repeat: Some(self.repeat),
            rate: 1.0,
            sampled_at: now,
        };
        Ok(if self.playing {
            Playback::Playing(track)
        } else {
            Playback::Paused(track)
        })
    }
}

impl MediaProvider for FakeMusic {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn sample(&mut self, _cancel: &AtomicBool) -> Result<Playback, String> {
        self.sample_at(Instant::now())
    }

    fn simulate(&mut self, change: FakeChange, now: Instant) -> Result<(), String> {
        self.change(change, now);
        Ok(())
    }

    fn command(
        &mut self,
        track_id: &str,
        command: PlaybackCommand,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        if self.slow {
            fake_delay(cancel, Duration::from_millis(800))?;
        }
        if self.failed || Self::IDS[self.index] != track_id {
            return Err("simulated player or track changed".into());
        }
        let now = Instant::now();
        match command {
            PlaybackCommand::Previous => {
                self.index = (self.index + Self::IDS.len() - 1) % Self::IDS.len();
                self.position = 0.0;
                self.anchor = now;
            }
            PlaybackCommand::PlayPause => self.change(
                if self.playing {
                    FakeChange::Pause
                } else {
                    FakeChange::Play
                },
                now,
            ),
            PlaybackCommand::Next => self.change(FakeChange::Next, now),
            PlaybackCommand::Seek(position)
                if position.is_finite() && self.duration().is_some() =>
            {
                self.change(FakeChange::Seek(position), now);
            }
            PlaybackCommand::Seek(_) => return Err("seek position or duration unavailable".into()),
            PlaybackCommand::ToggleShuffle => self.shuffle = !self.shuffle,
            PlaybackCommand::CycleRepeat => self.repeat = self.repeat.next(),
        }
        Ok(())
    }

    fn artwork(
        &mut self,
        track_id: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<GrayImage>, String> {
        match track_id {
            "first" => Ok(Some(GrayImage::from_fn(192, 192, |x, y| {
                image::Luma([(x + y / 3) as u8])
            }))),
            "second" => {
                fake_delay(cancel, Duration::from_millis(500))?;
                Ok(Some(GrayImage::from_fn(192, 192, |x, y| {
                    image::Luma([if (x / 24 + y / 24) % 2 == 0 { 55 } else { 195 }])
                })))
            }
            _ => Ok(None),
        }
    }
}

fn fake_delay(cancel: &AtomicBool, duration: Duration) -> Result<(), String> {
    let end = Instant::now() + duration;
    while Instant::now() < end {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("media operation cancelled".into());
        }
        std::thread::sleep(
            end.saturating_duration_since(Instant::now())
                .min(Duration::from_millis(20)),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_clock_advances_clamps_and_freezes() {
        let now = Instant::now();
        let mut provider = FakeMusic::default();
        provider.change(FakeChange::Seek(178.0), now);
        let playing = provider.sample_at(now).unwrap();
        assert_eq!(playing.position(now + Duration::from_secs(1)), Some(179.0));
        assert_eq!(playing.position(now + Duration::from_secs(10)), Some(180.0));
        provider.change(FakeChange::Pause, now);
        assert_eq!(
            provider
                .sample_at(now)
                .unwrap()
                .position(now + Duration::from_secs(30)),
            Some(178.0)
        );
    }

    #[test]
    fn stale_clock_is_bounded_and_external_changes_reanchor() {
        let now = Instant::now();
        let mut provider = FakeMusic::default();
        provider.change(FakeChange::Seek(40.0), now);
        let playing = provider.sample_at(now).unwrap();
        let later = now + Duration::from_secs(40);
        assert_eq!(playing.position(later), Some(45.0));
        assert_eq!(
            playing
                .stale(later)
                .position(later + Duration::from_secs(40)),
            Some(45.0)
        );
        provider.change(FakeChange::Fail, later);
        assert!(provider.sample_at(later).is_err());
        provider.change(FakeChange::Recover, later);
        provider.change(FakeChange::Seek(13.0), later);
        assert_eq!(
            provider.sample_at(later).unwrap().position(later),
            Some(13.0)
        );
        provider.change(FakeChange::Next, later);
        let next = provider.sample_at(later).unwrap();
        assert_eq!(next.track().unwrap().id.as_deref(), Some("second"));
        assert_eq!(next.position(later), Some(0.0));
    }

    #[test]
    fn duration_and_position_validate_separately() {
        let now = Instant::now();
        let mut provider = FakeMusic::default();
        provider.change(FakeChange::Next, now);
        provider.change(FakeChange::Next, now);
        let Playback::Playing(mut track) = provider.sample_at(now).unwrap() else {
            panic!()
        };
        assert_eq!(track.duration(), None);
        track.duration = Some(0.0);
        track.position = Some(f64::NAN);
        assert_eq!(track.duration(), None);
        assert_eq!(Playback::Playing(track).position(now), None);
        assert!(
            provider
                .command(
                    "stream",
                    PlaybackCommand::Seek(10.0),
                    &AtomicBool::new(false)
                )
                .is_err()
        );
    }
}
