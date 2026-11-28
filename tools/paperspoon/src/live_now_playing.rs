//! Owns the live mode and coordinates media, artwork, and framebuffer workers.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use image::DynamicImage;
use paper_protocol::{Gray8Frame, PixelFormat, V2Pointer, V2PointerPhase};
use paperspoon::now_playing_provider::{
    FakeChange, FakeMusic, MediaProvider, Playback, PlaybackCommand,
};
use paperspoon::now_playing_renderer::{LiveControl, LiveLayout, LiveNowPlayingRenderer};

use crate::frame_sender::FrameSender;
use crate::server::FrameTarget;
use paperspoon::operator_log::{self, append_host_log_record};

const MEDIA_QUEUE_CAPACITY: usize = 8;
const ARTWORK_CACHE_CAPACITY: usize = 8;
const PLAYING_POLL: Duration = Duration::from_secs(1);
const PAUSED_POLL: Duration = Duration::from_secs(2);
const MAX_INACTIVE_POLL: Duration = Duration::from_secs(5);
const COMMAND_OBSERVATION_WINDOW: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderKind {
    Fake,
    AppleMusic,
}

impl ProviderKind {
    fn create(self) -> Result<Box<dyn MediaProvider>, String> {
        match self {
            Self::Fake => Ok(Box::new(FakeMusic::default())),
            #[cfg(target_os = "macos")]
            Self::AppleMusic => Ok(Box::new(paperspoon::now_playing_provider::AppleMusic)),
            #[cfg(not(target_os = "macos"))]
            Self::AppleMusic => Err("the Apple Music provider requires macOS".into()),
        }
    }
}

pub(crate) struct LiveNowPlaying {
    provider: Option<ProviderKind>,
    progress_interval: Duration,
    frames: FrameSender,
    log_path: String,
    lifecycle: Mutex<()>,
    shutting_down: AtomicBool,
    session: Mutex<Option<Session>>,
}

struct Session {
    shared: Arc<Shared>,
    work: mpsc::SyncSender<MediaWork>,
    workers: [JoinHandle<()>; 3],
}

enum MediaWork {
    Command {
        target_revision: u64,
        track_revision: u64,
        track_id: String,
        command: PlaybackCommand,
    },
    Fake(FakeChange),
    Refresh,
}

struct CommandObservation {
    started: Instant,
    command: PlaybackCommand,
    before: Playback,
}

impl CommandObservation {
    fn matches(&self, after: &Playback, now: Instant) -> bool {
        let Some(track) = after.controllable_track() else {
            return false;
        };
        match self.command {
            PlaybackCommand::RevealTrack => false,
            PlaybackCommand::PlayPause => after.label() != self.before.label(),
            PlaybackCommand::Next | PlaybackCommand::Previous => {
                self.before
                    .track()
                    .is_some_and(|before| track.id != before.id)
                    || self
                        .before
                        .position(now)
                        .zip(after.position(now))
                        .is_some_and(|(before, after)| before - after >= 2.0)
            }
            PlaybackCommand::Seek(seconds) => {
                self.before
                    .track()
                    .is_some_and(|before| track.id == before.id)
                    && after
                        .position(now)
                        .is_some_and(|position| (seconds - position).abs() <= 2.0)
            }
            PlaybackCommand::ToggleShuffle => self
                .before
                .track()
                .and_then(|before| before.shuffle)
                .is_some_and(|before| track.shuffle == Some(!before)),
            PlaybackCommand::CycleRepeat => self
                .before
                .track()
                .and_then(|before| before.repeat)
                .is_some_and(|before| track.repeat == Some(before.next())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArtworkRequest {
    track_id: String,
    track_revision: u64,
    refresh: u64,
}

struct Armed {
    track_id: String,
    control: LiveControl,
}

enum Contact {
    Idle,
    Armed(Armed),
    Cancelled,
}

impl Contact {
    fn armed(&self) -> Option<&Armed> {
        match self {
            Self::Armed(armed) => Some(armed),
            _ => None,
        }
    }

    fn pressed_control(&self) -> Option<LiveControl> {
        self.armed()
            .map(|armed| armed.control)
            .filter(|control| *control != LiveControl::Cover)
    }
}

struct State {
    playback: Playback,
    target: Option<FrameTarget>,
    target_revision: u64,
    visible_track: Option<String>,
    contact: Contact,
    revision: u64,
    track_revision: u64,
    artwork_revision: u64,
    artwork_refresh: u64,
    artwork: Option<Arc<DynamicImage>>,
    error: Option<String>,
}

impl State {
    fn new(target: Option<FrameTarget>) -> Self {
        Self {
            playback: Playback::Unavailable,
            target,
            target_revision: 0,
            visible_track: None,
            contact: Contact::Idle,
            revision: 0,
            track_revision: 0,
            artwork_revision: 0,
            artwork_refresh: 0,
            artwork: None,
            error: None,
        }
    }

    fn track_id(&self) -> Option<&str> {
        self.playback.track()?.id.as_deref()
    }

    fn artwork_request(&self) -> Option<ArtworkRequest> {
        Some(ArtworkRequest {
            track_id: self.track_id()?.into(),
            track_revision: self.track_revision,
            refresh: self.artwork_refresh,
        })
    }

    fn apply_artwork(
        &mut self,
        request: &ArtworkRequest,
        artwork: Option<Arc<DynamicImage>>,
    ) -> bool {
        if self.artwork_request().as_ref() != Some(request) {
            return false;
        }
        self.artwork = artwork;
        self.artwork_revision += 1;
        self.revision += 1;
        true
    }

    fn set_target(&mut self, target: Option<FrameTarget>) -> bool {
        let same = match (&self.target, &target) {
            (Some(old), Some(new)) => old.is_same(new),
            (None, None) => true,
            _ => false,
        };
        if same {
            return false;
        }
        self.target = target;
        self.target_revision += 1;
        self.visible_track = None;
        self.contact = Contact::Idle;
        self.revision += 1;
        true
    }

    fn set_playback(&mut self, playback: Playback, now: Instant) -> bool {
        let old = self.playback.track();
        let new = playback.track();
        let track_changed =
            old.and_then(|track| track.id.as_deref()) != new.and_then(|track| track.id.as_deref());
        if track_changed {
            self.track_revision += 1;
            self.artwork = None;
            self.artwork_revision += 1;
            self.contact = Contact::Idle;
        }
        let invalid_arm = self
            .contact
            .armed()
            .is_some_and(|armed| !armed.control.enabled(&playback));
        if invalid_arm {
            self.contact = Contact::Cancelled;
        }
        let metadata_changed = match (old, new) {
            (Some(old), Some(new)) => {
                old.title != new.title
                    || old.artist != new.artist
                    || old.album != new.album
                    || old.duration() != new.duration()
                    || old.rate != new.rate
                    || old.shuffle != new.shuffle
                    || old.repeat != new.repeat
            }
            (None, None) => false,
            _ => true,
        };
        let position_changed = match (self.playback.position(now), playback.position(now)) {
            (Some(old), Some(new)) if matches!(playback, Playback::Playing(_)) => {
                (old - new).abs() >= 0.4
                    || (!playback.progress_active(now) && old.floor() != new.floor())
            }
            (Some(old), Some(new)) => old != new,
            (None, None) => false,
            _ => true,
        };
        // A fresh anchor can restart progress after an overdue sample.
        let resumes_progress = !self.playback.progress_active(now) && playback.progress_active(now);
        let changed = track_changed
            || invalid_arm
            || metadata_changed
            || position_changed
            || resumes_progress
            || self.playback.label() != playback.label();
        self.playback = playback;
        if changed {
            self.revision += 1;
        }
        changed
    }

    fn pointer(&mut self, pointer: V2Pointer) -> Option<MediaWork> {
        let layout = LiveLayout::new(self.target.as_ref()?.viewport());
        let control = layout.control_at(pointer.x(), pointer.y());
        match pointer.phase() {
            V2PointerPhase::Down => {
                let pressed = self.contact.pressed_control();
                let track_id = self
                    .track_id()
                    .filter(|id| Some(*id) == self.visible_track.as_deref());
                let enabled = control.filter(|control| control.enabled(&self.playback));
                self.contact = match (track_id, enabled, &self.contact) {
                    (Some(track_id), Some(control), Contact::Idle) => Contact::Armed(Armed {
                        track_id: track_id.into(),
                        control,
                    }),
                    _ => Contact::Cancelled,
                };
                if pressed != self.contact.pressed_control() {
                    self.revision += 1;
                }
                None
            }
            V2PointerPhase::Up => {
                let Contact::Armed(armed) = std::mem::replace(&mut self.contact, Contact::Idle)
                else {
                    return None;
                };
                if armed.control != LiveControl::Cover {
                    self.revision += 1;
                }
                if control != Some(armed.control) || self.track_id() != Some(&armed.track_id) {
                    return None;
                }
                Some(MediaWork::Command {
                    target_revision: self.target_revision,
                    track_revision: self.track_revision,
                    track_id: armed.track_id,
                    command: layout.command(armed.control, pointer.x(), &self.playback)?,
                })
            }
        }
    }
}

struct Shared {
    cancel: AtomicBool,
    state: Mutex<State>,
    changed: Condvar,
    log_path: String,
}

impl Shared {
    fn log(&self, message: &str) {
        operator_log::info!("{message}");
        append_host_log_record(&self.log_path, message);
    }
}

impl LiveNowPlaying {
    pub(crate) fn new(
        frames: FrameSender,
        provider: Option<ProviderKind>,
        progress_interval: Duration,
        log_path: String,
    ) -> Self {
        Self {
            provider,
            progress_interval,
            frames,
            log_path,
            lifecycle: Mutex::new(()),
            shutting_down: AtomicBool::new(false),
            session: Mutex::new(None),
        }
    }

    pub(crate) fn start(&self) -> Result<(), String> {
        let _lifecycle = self.lifecycle.lock().expect("Now Playing lifecycle");
        if self.shutting_down.load(Ordering::Relaxed) {
            return Err("PaperSpoon is shutting down".into());
        }
        let mut slot = self.session.lock().expect("Now Playing session");
        if slot.is_some() {
            return Ok(());
        }
        let kind = self
            .provider
            .ok_or("select --now-playing-fake or --now-playing-apple-music at startup")?;
        let media = kind.create()?;
        let artwork = kind.create()?;
        let shared = Arc::new(Shared {
            cancel: AtomicBool::new(false),
            state: Mutex::new(State::new(self.target())),
            changed: Condvar::new(),
            log_path: self.log_path.clone(),
        });
        let (work, receiver) = mpsc::sync_channel(MEDIA_QUEUE_CAPACITY);
        self.frames.set_live_active(true);
        let workers = [
            spawn_worker("paperspoon-media", &shared, move |shared| {
                media_loop(&shared, media, receiver)
            }),
            spawn_worker("paperspoon-artwork", &shared, move |shared| {
                artwork_loop(&shared, artwork)
            }),
            spawn_worker("paperspoon-now-playing", &shared, {
                let frames = self.frames.clone();
                let interval = self.progress_interval;
                move |shared| render_loop(&shared, &frames, interval)
            }),
        ];
        *slot = Some(Session {
            shared,
            work,
            workers,
        });
        Ok(())
    }

    pub(crate) fn stop(&self) -> bool {
        let _lifecycle = self.lifecycle.lock().expect("Now Playing lifecycle");
        let session = self.session.lock().expect("Now Playing session").take();
        let Some(Session {
            shared,
            work,
            workers,
        }) = session
        else {
            return false;
        };
        {
            let _state = shared.state.lock().expect("Now Playing state");
            shared.cancel.store(true, Ordering::Relaxed);
            shared.changed.notify_all();
        }
        drop(work);
        for worker in workers {
            if worker.join().is_err() {
                operator_log::error!("Now Playing worker stopped unexpectedly");
            }
        }
        self.frames.set_live_active(false);
        true
    }

    pub(crate) fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Relaxed);
        self.stop();
    }

    fn target(&self) -> Option<FrameTarget> {
        self.frames.frame_target(PixelFormat::Gray8).ok()
    }

    // Called only after the server installs, resizes, or clears its current connection.
    pub(crate) fn connection_changed(&self) -> bool {
        let slot = self.session.lock().expect("Now Playing session");
        let Some(session) = slot.as_ref() else {
            return false;
        };
        let target = self.target();
        let active = target.is_some();
        if session
            .shared
            .state
            .lock()
            .expect("Now Playing state")
            .set_target(target)
        {
            session.shared.changed.notify_all();
        }
        active
    }

    pub(crate) fn pointer(&self, pointer: V2Pointer) -> bool {
        let slot = self.session.lock().expect("Now Playing session");
        let Some(session) = slot.as_ref() else {
            return false;
        };
        let mut state = session.shared.state.lock().expect("Now Playing state");
        if state.target.is_none() {
            return false;
        }
        let before = state.revision;
        let command = state.pointer(pointer);
        if state.revision != before {
            session.shared.changed.notify_all();
        }
        drop(state);
        if let Some(command) = command
            && let Err(error) = session.work.try_send(command)
        {
            operator_log::error!("Now Playing input: {error}");
        }
        true
    }

    pub(crate) fn status(&self) -> String {
        let slot = self.session.lock().expect("Now Playing session");
        let Some(session) = slot.as_ref() else {
            return "Now Playing stopped".into();
        };
        let state = session.shared.state.lock().expect("Now Playing state");
        Self::describe(&state)
    }

    pub(crate) fn try_status(&self) -> Option<String> {
        let slot = self.session.try_lock().ok()?;
        let Some(session) = slot.as_ref() else {
            return Some("Now Playing stopped".into());
        };
        let state = session.shared.state.try_lock().ok()?;
        Some(Self::describe(&state))
    }

    fn describe(state: &State) -> String {
        let client = if state.target.is_some() {
            "Gray8 connected"
        } else {
            "waiting for Gray8"
        };
        let last_error = state
            .error
            .as_ref()
            .map_or(String::new(), |error| format!(" error={error}"));
        format!(
            "Now Playing active state={} client={client}{last_error}",
            state.playback.label()
        )
    }

    pub(crate) fn refresh(&self) -> Result<(), String> {
        self.enqueue(MediaWork::Refresh)
    }

    pub(crate) fn fake(&self, change: FakeChange) -> Result<(), String> {
        if self.provider != Some(ProviderKind::Fake) {
            return Err("fake playback changes require --now-playing-fake".into());
        }
        self.enqueue(MediaWork::Fake(change))
    }

    fn enqueue(&self, work: MediaWork) -> Result<(), String> {
        let slot = self.session.lock().expect("Now Playing session");
        slot.as_ref()
            .ok_or("Now Playing is stopped")?
            .work
            .try_send(work)
            .map_err(|error| error.to_string())
    }
}

fn spawn_worker(
    name: &str,
    shared: &Arc<Shared>,
    run: impl FnOnce(Arc<Shared>) + Send + 'static,
) -> JoinHandle<()> {
    let shared = Arc::clone(shared);
    thread::Builder::new()
        .name(name.into())
        .spawn(move || run(shared))
        .expect("spawn Now Playing worker")
}

fn media_loop(
    shared: &Shared,
    mut provider: Box<dyn MediaProvider>,
    receiver: mpsc::Receiver<MediaWork>,
) {
    let mut inactive_delay = PAUSED_POLL;
    let mut observe_command: Option<CommandObservation> = None;
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            return;
        }
        let started = Instant::now();
        let result = provider.sample(&shared.cancel);
        if shared.cancel.load(Ordering::Relaxed) {
            return;
        }
        let now = Instant::now();
        let mut state = shared.state.lock().expect("Now Playing state");
        let (sample, error) = match result {
            Ok(sample) => (sample, None),
            Err(error) => (state.playback.stale(now), Some(error)),
        };
        let delay = match sample {
            Playback::Playing(_) | Playback::Seeking(_) => PLAYING_POLL,
            Playback::Paused(_) => PAUSED_POLL,
            _ => inactive_delay,
        };
        if matches!(
            sample,
            Playback::Unavailable | Playback::Stopped | Playback::Stale(_)
        ) {
            inactive_delay = (inactive_delay + PLAYING_POLL).min(MAX_INACTIVE_POLL);
        } else {
            inactive_delay = PAUSED_POLL;
        }
        let changed = state.set_playback(sample, now);
        let observed = observe_command
            .as_ref()
            .filter(|pending| {
                now.saturating_duration_since(pending.started) < COMMAND_OBSERVATION_WINDOW
                    && pending.matches(&state.playback, now)
            })
            .map(|pending| pending.started);
        let error_changed = state.error != error;
        state.error = error.clone();
        if changed {
            shared.changed.notify_all();
        }
        drop(state);
        shared.log(&format!(
            "nowplaying sample provider={} elapsed_us={} changed={} result={}",
            provider.name(),
            started.elapsed().as_micros(),
            changed,
            if error.is_some() { "error" } else { "ok" },
        ));
        if error_changed && let Some(error) = error {
            operator_log::error!("Now Playing acquisition: {error}");
        }
        if let Some(started) = observed {
            observe_command = None;
            shared.log(&format!(
                "nowplaying state change after command elapsed_us={}",
                started.elapsed().as_micros()
            ));
        } else if observe_command
            .as_ref()
            .is_some_and(|pending| pending.started.elapsed() >= COMMAND_OBSERVATION_WINDOW)
        {
            observe_command = None;
        }

        match receiver.recv_timeout(delay.saturating_sub(started.elapsed())) {
            Ok(_) if shared.cancel.load(Ordering::Relaxed) => return,
            Ok(MediaWork::Command {
                target_revision,
                track_revision,
                track_id,
                command,
            }) => {
                let state = shared.state.lock().expect("Now Playing state");
                if state.target_revision != target_revision
                    || state.track_revision != track_revision
                    || state
                        .playback
                        .controllable_track()
                        .and_then(|track| track.id.as_deref())
                        != Some(&track_id)
                {
                    continue;
                }
                let before = state.playback.clone();
                drop(state);
                let started = Instant::now();
                let result = provider.command(&track_id, command, &shared.cancel);
                shared.log(&format!(
                    "nowplaying command={command:?} elapsed_us={} result={}",
                    started.elapsed().as_micros(),
                    if result.is_ok() { "ok" } else { "error" },
                ));
                match result {
                    Ok(()) if command != PlaybackCommand::RevealTrack => {
                        observe_command = Some(CommandObservation {
                            started,
                            command,
                            before,
                        });
                    }
                    Ok(()) => {}
                    Err(error) => {
                        operator_log::error!("Now Playing command failed: {error}");
                        observe_command = None;
                    }
                };
            }
            Ok(MediaWork::Fake(change)) => {
                if let Err(error) = provider.simulate(change, Instant::now()) {
                    operator_log::error!("Now Playing simulation: {error}");
                }
            }
            Ok(MediaWork::Refresh) => {
                let mut state = shared.state.lock().expect("Now Playing state");
                state.artwork_refresh += 1;
                shared.changed.notify_all();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn artwork_loop(shared: &Shared, mut provider: Box<dyn MediaProvider>) {
    let mut cache: VecDeque<(String, Option<Arc<DynamicImage>>)> = VecDeque::new();
    let mut last_request: Option<ArtworkRequest> = None;
    loop {
        let mut state = shared.state.lock().expect("Now Playing state");
        let request = loop {
            if shared.cancel.load(Ordering::Relaxed) {
                return;
            }
            if let Some(request) = state.artwork_request()
                && last_request.as_ref() != Some(&request)
            {
                break request;
            }
            state = shared.changed.wait(state).expect("Now Playing state");
        };
        drop(state);
        let forced = last_request
            .as_ref()
            .is_some_and(|previous| previous.refresh != request.refresh);
        last_request = Some(request.clone());
        let cached = cache
            .iter()
            .position(|(track_id, _)| *track_id == request.track_id)
            .and_then(|index| cache.remove(index));
        let started = Instant::now();
        let result = match cached {
            Some((_, artwork)) if !forced => Ok(artwork),
            _ => provider
                .artwork(&request.track_id, &shared.cancel)
                .map(|artwork| artwork.map(|artwork| Arc::new(DynamicImage::ImageLuma8(artwork)))),
        };
        if shared.cancel.load(Ordering::Relaxed) {
            return;
        }
        shared.log(&format!(
            "nowplaying artwork elapsed_us={} result={}",
            started.elapsed().as_micros(),
            match &result {
                Ok(Some(_)) => "loaded",
                Ok(None) => "missing",
                Err(_) => "error",
            }
        ));
        let artwork = match result {
            Ok(artwork) => artwork,
            Err(error) => {
                operator_log::error!("Now Playing artwork: {error}");
                continue;
            }
        };
        cache.push_front((request.track_id.clone(), artwork.clone()));
        cache.truncate(ARTWORK_CACHE_CAPACITY);
        let mut state = shared.state.lock().expect("Now Playing state");
        if state.apply_artwork(&request, artwork) {
            shared.changed.notify_all();
        }
    }
}

fn render_loop(shared: &Shared, frames: &FrameSender, interval: Duration) {
    let mut renderer = match LiveNowPlayingRenderer::new() {
        Ok(renderer) => renderer,
        Err(error) => {
            operator_log::error!("Now Playing renderer: {error}");
            return;
        }
    };
    let mut rendered_revision = None;
    let mut next_progress = None;
    let mut last_sent: Option<(u64, Gray8Frame)> = None;
    let mut coalesced = 0_u64;
    let mut unchanged = 0_u64;
    loop {
        let mut state = shared.state.lock().expect("Now Playing state");
        let target = loop {
            if shared.cancel.load(Ordering::Relaxed) {
                return;
            }
            if let Some(target) = &state.target {
                if rendered_revision != Some(state.revision) {
                    break target.clone();
                }
                if let Some(deadline) = next_progress {
                    let now = Instant::now();
                    if now >= deadline {
                        break target.clone();
                    }
                    (state, _) = shared
                        .changed
                        .wait_timeout(state, deadline - now)
                        .expect("Now Playing state");
                    continue;
                }
            }
            state = shared.changed.wait(state).expect("Now Playing state");
        };
        let revision = state.revision;
        let target_revision = state.target_revision;
        let artwork = state.artwork.clone();
        let artwork_revision = state.artwork_revision;
        let pressed = state.contact.pressed_control();
        let playback = state.playback.clone();
        drop(state);
        let started = Instant::now();
        let result = renderer.render(
            target.viewport(),
            &playback,
            artwork.as_deref(),
            artwork_revision,
            pressed,
            started,
        );
        let render_time = started.elapsed();
        rendered_revision = Some(revision);
        next_progress = playback
            .progress_active(started)
            .then(|| started + interval);
        let frame = match result {
            Ok(frame) => frame,
            Err(error) => {
                operator_log::error!("Now Playing frame: {error}");
                continue;
            }
        };
        if shared.cancel.load(Ordering::Relaxed) {
            return;
        }
        if shared.state.lock().expect("Now Playing state").revision != revision {
            coalesced += 1;
            continue;
        }
        let previous = last_sent
            .as_ref()
            .filter(|(last_target, _)| *last_target == target_revision);
        if previous.is_some_and(|(_, previous)| previous == &frame) {
            mark_visible(shared, target_revision, &playback);
            unchanged += 1;
            continue;
        }
        let (changed, dirty_area) = previous.map_or(
            (frame.pixels().len(), frame.pixels().len()),
            |(_, previous)| pixel_changes(previous, &frame),
        );
        match frames.send_live_frame(&frame, &target, &shared.cancel) {
            Ok(Some(sent)) => {
                mark_visible(shared, target_revision, &playback);
                last_sent = Some((target_revision, frame));
                shared.log(&format!(
                    "sent live nowplaying frame id={} width={} height={} bytes={} changed_pixels={} dirty_area={} coalesced={} unchanged={} render_us={} encode_us={} socket_write_us={}",
                    sent.frame_id, target.viewport().0, target.viewport().1, sent.encoded_len, changed,
                    dirty_area, coalesced, unchanged,
                    render_time.as_micros(), sent.encode_elapsed.as_micros(), sent.socket_write_elapsed.as_micros(),
                ));
                coalesced = 0;
                unchanged = 0;
            }
            Ok(None) => {}
            Err(error) => operator_log::error!("Now Playing frame: {error}"),
        }
    }
}

fn pixel_changes(previous: &Gray8Frame, frame: &Gray8Frame) -> (usize, usize) {
    let width = usize::from(frame.width());
    let mut left = width;
    let mut right = 0;
    let mut top = usize::from(frame.height());
    let mut bottom = 0;
    let mut count = 0;
    for (y, (before, after)) in previous
        .pixels()
        .chunks_exact(width)
        .zip(frame.pixels().chunks_exact(width))
        .enumerate()
    {
        if before == after {
            continue;
        }
        top = top.min(y);
        bottom = y + 1;
        for (x, (a, b)) in before.iter().zip(after).enumerate() {
            if a != b {
                count += 1;
                left = left.min(x);
                right = right.max(x + 1);
            }
        }
    }
    (
        count,
        right.saturating_sub(left) * bottom.saturating_sub(top),
    )
}

fn mark_visible(shared: &Shared, target_revision: u64, playback: &Playback) {
    let mut state = shared.state.lock().expect("Now Playing state");
    if state.target_revision == target_revision {
        state.visible_track = playback
            .controllable_track()
            .and_then(|track| track.id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::CurrentConnection;
    use paper_protocol::V2PixelFormats;
    use paperspoon::now_playing_provider::RepeatMode;
    use std::net::{TcpListener, TcpStream};

    fn connected() -> (State, FakeMusic, Instant) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let current = CurrentConnection::default();
        current
            .install_for_hello(&server, V2PixelFormats::GRAY8, (320, 480))
            .unwrap();
        let mut state = State::new(Some(current.frame_target(PixelFormat::Gray8).unwrap()));
        let provider = FakeMusic::default();
        let now = Instant::now();
        state.set_playback(provider.sample_at(now).unwrap(), now);
        state.visible_track = state.track_id().map(str::to_string);
        (state, provider, now)
    }

    #[test]
    fn paired_release_dispatches_once_and_outside_or_duplicate_contact_cancels() {
        let (mut state, _, _) = connected();
        let down = V2Pointer::new(V2PointerPhase::Down, 159, 398);
        let up = V2Pointer::new(V2PointerPhase::Up, 159, 398);
        assert!(state.pointer(up).is_none());
        assert!(state.pointer(down).is_none());
        assert!(matches!(
            state.pointer(up),
            Some(MediaWork::Command {
                command: PlaybackCommand::PlayPause,
                ..
            })
        ));
        assert!(state.pointer(up).is_none());
        state.pointer(down);
        assert!(
            state
                .pointer(V2Pointer::new(V2PointerPhase::Up, 159, 10))
                .is_none()
        );
        state.pointer(down);
        state.pointer(down);
        assert!(state.pointer(up).is_none());
    }

    #[test]
    fn cover_tap_reveals_the_current_track_without_repainting() {
        let (mut state, mut provider, now) = connected();
        let down = V2Pointer::new(V2PointerPhase::Down, 160, 35);
        let up = V2Pointer::new(V2PointerPhase::Up, 160, 35);
        let revision = state.revision;
        assert!(state.pointer(up).is_none());
        assert!(state.pointer(down).is_none());
        assert!(
            state
                .pointer(V2Pointer::new(V2PointerPhase::Up, 5, 35))
                .is_none()
        );
        assert!(state.pointer(down).is_none());
        assert!(matches!(
            state.pointer(up),
            Some(MediaWork::Command {
                command: PlaybackCommand::RevealTrack,
                ..
            })
        ));
        assert!(state.pointer(up).is_none());
        assert_eq!(state.revision, revision);

        state.pointer(down);
        provider.change(FakeChange::Next, now);
        state.set_playback(provider.sample_at(now).unwrap(), now);
        assert!(state.pointer(up).is_none());
    }

    #[test]
    fn track_or_connection_change_invalidates_armed_seek_and_outdated_artwork() {
        let (mut state, mut provider, now) = connected();
        let down = V2Pointer::new(V2PointerPhase::Down, 45, 341);
        let up = V2Pointer::new(V2PointerPhase::Up, 309, 341);
        let old_art = state.artwork_request().unwrap();
        state.pointer(down);
        assert!(matches!(
            state.pointer(up),
            Some(MediaWork::Command {
                command: PlaybackCommand::Seek(180.0),
                ..
            })
        ));
        state.pointer(down);
        provider.change(FakeChange::Next, now);
        state.set_playback(provider.sample_at(now).unwrap(), now);
        assert!(state.pointer(up).is_none());
        assert!(!state.apply_artwork(&old_art, None));

        state.visible_track = state.track_id().map(str::to_string);
        let target = state.target.clone();
        state.pointer(down);
        state.set_target(None);
        state.set_target(target);
        assert!(state.pointer(up).is_none());
        assert!(state.visible_track.is_none());
    }

    #[test]
    fn mode_buttons_dispatch_and_reconcile_while_paused() {
        let (mut state, mut provider, now) = connected();
        provider.change(FakeChange::Pause, now);
        state.set_playback(provider.sample_at(now).unwrap(), now);
        for (x, command, shuffle, repeat) in [
            (80, PlaybackCommand::ToggleShuffle, true, RepeatMode::Off),
            (240, PlaybackCommand::CycleRepeat, true, RepeatMode::All),
            (240, PlaybackCommand::CycleRepeat, true, RepeatMode::One),
            (240, PlaybackCommand::CycleRepeat, true, RepeatMode::Off),
            (80, PlaybackCommand::ToggleShuffle, false, RepeatMode::Off),
        ] {
            let down = V2Pointer::new(V2PointerPhase::Down, x, 450);
            let up = V2Pointer::new(V2PointerPhase::Up, x, 450);
            assert!(state.pointer(down).is_none());
            let Some(MediaWork::Command {
                track_id,
                command: resolved,
                ..
            }) = state.pointer(up)
            else {
                panic!("no mode change resolved");
            };
            assert_eq!(resolved, command);
            assert!(state.pointer(up).is_none());
            let observation = CommandObservation {
                started: now,
                command,
                before: state.playback.clone(),
            };
            assert!(!observation.matches(&state.playback, now));
            provider
                .command(&track_id, resolved, &AtomicBool::new(false))
                .unwrap();
            let sample = provider.sample_at(now).unwrap();
            assert!(observation.matches(&sample, now));
            assert!(state.set_playback(sample, now));
            let track = state.playback.controllable_track().unwrap();
            assert_eq!(track.shuffle, Some(shuffle));
            assert_eq!(track.repeat, Some(repeat));
            assert_eq!(state.playback.label(), "PAUSED");
        }
        let Playback::Paused(mut track) = state.playback.clone() else {
            panic!()
        };
        track.shuffle = None;
        track.repeat = None;
        assert!(state.set_playback(Playback::Paused(track), now));
        for x in [80, 240] {
            assert!(
                state
                    .pointer(V2Pointer::new(V2PointerPhase::Down, x, 450))
                    .is_none()
            );
            assert!(
                state
                    .pointer(V2Pointer::new(V2PointerPhase::Up, x, 450))
                    .is_none()
            );
        }
    }

    #[test]
    fn polling_steady_pause_does_not_invalidate_frames_but_external_seek_does() {
        let now = Instant::now();
        let mut state = State::new(None);
        let mut provider = FakeMusic::default();
        provider.change(FakeChange::Pause, now);
        assert!(state.set_playback(provider.sample_at(now).unwrap(), now));
        let later = now + Duration::from_secs(2);
        assert!(!state.set_playback(provider.sample_at(later).unwrap(), later));
        provider.change(FakeChange::Seek(115.0), later);
        assert!(state.set_playback(provider.sample_at(later).unwrap(), later));
        let stale = state.playback.stale(later);
        assert!(state.set_playback(stale, later));
        assert!(!LiveControl::Seek.enabled(&state.playback));
        assert!(state.set_playback(provider.sample_at(later).unwrap(), later));
    }
}
