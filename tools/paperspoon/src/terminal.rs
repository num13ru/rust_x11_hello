//! Terminal lifecycle, event loop, and worker wiring for the operator console.

use std::io;
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use paperspoon::operator::OperatorCommand;
use paperspoon::operator_log;
use paperspoon::tui::app::{AppContext, Effect as TuiEffect, TuiApp, TuiKey};
use paperspoon::tui::view;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor;
use ratatui::crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::dispatch::Operator;
use crate::frame_sender::FrameSender;
use crate::latency;
use crate::listener::run_listener;
use crate::live_now_playing::{LiveNowPlaying, ProviderKind};
use crate::options::Options;
use crate::server::{CurrentConnection, FrameTarget, SharedStatus};

/// Bounded operator command queue between the UI and the command worker.
pub(crate) const COMMAND_QUEUE_CAPACITY: usize = 8;

struct QueuedCommand {
    command: OperatorCommand,
    target: Option<FrameTarget>,
}

/// Restores the terminal on drop, even on early returns or panics inside the
/// event loop.
struct TuiTerminalGuard;

impl TuiTerminalGuard {
    fn enter() -> io::Result<(Terminal<CrosstermBackend<io::Stdout>>, Self)> {
        let mut stdout = io::stdout();
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(
            stdout,
            EnterAlternateScreen,
            cursor::Hide,
            EnableMouseCapture
        )?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok((terminal, guard))
    }
}

impl Drop for TuiTerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            cursor::Show,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

/// Translate a crossterm key event into a TUI key, or None to ignore.
fn translate_key(key: KeyEvent, editing: bool) -> Option<TuiKey> {
    if key.kind == KeyEventKind::Release
        || (key.kind == KeyEventKind::Repeat && key.code == KeyCode::Enter)
    {
        return None;
    }
    match key.code {
        KeyCode::Enter => Some(TuiKey::Enter),
        KeyCode::Esc => Some(TuiKey::Esc),
        KeyCode::Backspace => Some(TuiKey::Backspace),
        KeyCode::Tab => Some(if key.modifiers.contains(KeyModifiers::SHIFT) {
            TuiKey::TabBackward
        } else {
            TuiKey::TabForward
        }),
        KeyCode::BackTab => Some(TuiKey::TabBackward),
        KeyCode::Up => Some(TuiKey::Up),
        KeyCode::Down => Some(TuiKey::Down),
        KeyCode::Left => Some(TuiKey::Left),
        KeyCode::Right => Some(TuiKey::Right),
        KeyCode::PageUp => Some(TuiKey::PageUp),
        KeyCode::PageDown => Some(TuiKey::PageDown),
        KeyCode::Home => Some(TuiKey::Home),
        KeyCode::End => Some(TuiKey::End),
        KeyCode::Char(_)
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            None
        }
        KeyCode::Char('f' | 'F') if !editing => Some(TuiKey::FollowToggle),
        KeyCode::Char('q' | 'Q') if !editing => Some(TuiKey::Quit),
        KeyCode::Char(character) => Some(TuiKey::Char(character)),
        _ => None,
    }
}

/// Handle Ctrl+C as a raw-mode key event: quit immediately regardless of
/// editor state.
fn is_ctrl_c(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
}

/// The main event loop. Returns on quit; the guard restores the terminal.
fn run_tui_event_loop(
    app: &mut TuiApp,
    status: &SharedStatus,
    command_tx: &mpsc::SyncSender<QueuedCommand>,
    outcome_rx: &mpsc::Receiver<Result<String, String>>,
    live: &LiveNowPlaying,
    header: &str,
) -> io::Result<()> {
    let (mut terminal, _guard) = TuiTerminalGuard::enter()?;
    let buffer = operator_log::history();

    let mut last_render = Instant::now() - Duration::from_secs(1);
    let mut last_status_poll = Instant::now() - Duration::from_secs(1);
    let mut hits = view::HitAreas::default();
    let mut pending = 0usize;
    let mut visible_target = None;
    let mut live_status = String::new();
    let mut dirty = true;
    let mut last_log = (0, 0);
    let mut last_header = ("", String::new());

    loop {
        // Signal-based shutdown (SIGINT/SIGTERM).
        if status.stopping() {
            return Ok(());
        }
        for outcome in outcome_rx.try_iter().take(COMMAND_QUEUE_CAPACITY + 1) {
            dirty = true;
            pending = pending.saturating_sub(1);
            if pending == 0 {
                app.set_status(outcome.unwrap_or_else(|error| error));
            } else {
                app.set_status(format!("{pending} commands pending"));
            }
        }

        if event::poll(Duration::from_millis(50))? {
            dirty = true;
            let effect = match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Release && is_ctrl_c(key) {
                        return Ok(());
                    }
                    translate_key(key, app.is_editing()).and_then(|key| app.handle_key(key))
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => hits
                        .button_at(mouse.column, mouse.row)
                        .and_then(|(group, button)| app.click_button(group, button)),
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                        if hits.log_contains(mouse.column, mouse.row) =>
                    {
                        app.scroll_log_by(if mouse.kind == MouseEventKind::ScrollUp {
                            -3
                        } else {
                            3
                        });
                        None
                    }
                    _ => None,
                },
                Event::Resize(_, _) => {
                    hits = view::HitAreas::default();
                    None
                }
                _ => None,
            };
            if let Some(effect) = effect {
                let error = match effect {
                    TuiEffect::Quit => return Ok(()),
                    TuiEffect::Command(command)
                        if !matches!(command, OperatorCommand::LiveNowPlaying(_))
                            && visible_target.is_none() =>
                    {
                        Some("No negotiated PaperPad connection".into())
                    }
                    TuiEffect::Command(command) => match command_tx.try_send(QueuedCommand {
                        command,
                        target: visible_target.clone(),
                    }) {
                        Ok(()) => {
                            pending += 1;
                            app.set_status(format!("{pending} pending"));
                            None
                        }
                        Err(mpsc::TrySendError::Full(_)) => {
                            Some("Command queue is full".to_owned())
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => {
                            Some("Command worker is unavailable".to_owned())
                        }
                    },
                    TuiEffect::Error(message) => Some(message),
                    TuiEffect::Status(message) => {
                        app.set_status(message);
                        None
                    }
                };
                if let Some(message) = error {
                    operator_log::error!("{message}");
                    app.set_status(message);
                }
            }
        }

        let now = Instant::now();
        if now.duration_since(last_status_poll) >= Duration::from_millis(100) {
            last_status_poll = now;
            if let Some(status) = live.try_status() {
                live_status = status;
            }
            let display = status.snapshot();
            app.set_display(
                display
                    .as_ref()
                    .map(|display| (display.target.viewport(), display.formats)),
            );
            let (connection, details) = if let Some(display) = &display {
                let (width, height) = display.target.viewport();
                let details = format!(
                    "Peer: {} | Viewport: {width}x{height} | {live_status}",
                    display.peer
                );
                let connection = if display.encodings.is_some() {
                    "Connected · PPFB v3"
                } else {
                    "Connected · PPFB v2"
                };
                (connection, details)
            } else {
                ("Disconnected", live_status.clone())
            };
            visible_target = display.map(|display| display.target);
            if last_header != (connection, details.clone()) {
                app.set_header(header, connection);
                app.set_subtitle(&details);
                last_header = (connection, details);
                dirty = true;
            }
        }

        if now.duration_since(last_render) >= Duration::from_millis(50) {
            let counts = buffer.window(operator_log::LogPosition::Follow, 0);
            if !dirty && last_log == (counts.next_id, counts.dropped) {
                continue;
            }
            last_render = now;
            dirty = false;
            terminal.draw(|frame| {
                app.resize_log(frame.area());
                let window = app.log_window(buffer);
                last_log = (window.next_id, window.dropped);
                app.observe_log(&window);
                hits = view::render(&app.view(window), frame);
            })?;
        }
    }
}

/// Start the console and its workers, and shut them down when the console exits.
pub(crate) fn run(opts: Options) -> io::Result<()> {
    let status = SharedStatus::default();
    #[cfg(unix)]
    let _signals = StopSignals::register(&status.stop_flag())?;
    latency::init(&opts.latency_log_path);

    let listener = TcpListener::bind(("0.0.0.0", opts.port))?;
    listener.set_nonblocking(true)?;
    let tcp_port = listener.local_addr()?.port();
    let discovery_socket = crate::discovery::bind_discovery_socket()?;
    discovery_socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let discovery_status = status.clone();
    let discovery_worker = std::thread::Builder::new()
        .name("paperspoon-discovery".to_string())
        .spawn(move || {
            if let Err(error) =
                crate::discovery::run_discovery_listener(discovery_socket, tcp_port, || {
                    discovery_status.stopping()
                })
            {
                operator_log::error!("discovery listener error: {error}");
            }
        })?;

    operator_log::info!(
        "listening on 0.0.0.0:{}, logging to {}",
        tcp_port,
        opts.log_path
    );
    operator_log::info!(
        "discovery listening address=0.0.0.0:{}",
        paper_protocol::DISCOVERY_PORT
    );

    let current = CurrentConnection::default();
    let frame_sender = FrameSender::new(current.clone());
    let live = Arc::new(LiveNowPlaying::new(
        frame_sender.clone(),
        opts.live_provider,
        opts.progress_interval,
        opts.log_path.clone(),
    ));

    let (application_viewport_tx, application_viewport_rx) = mpsc::channel();
    let operator = Operator {
        frame_sender: frame_sender.clone(),
        live: Arc::clone(&live),
        application_ui: paperspoon::application_ui::ApplicationUi::default(),
        application_viewport_tx,
        log_path: opts.log_path.clone(),
    };
    let (command_tx, command_rx) = mpsc::sync_channel::<QueuedCommand>(COMMAND_QUEUE_CAPACITY);
    let (outcome_tx, outcome_rx) = mpsc::channel();
    let command_status = status.clone();
    let command_worker = std::thread::Builder::new()
        .name("paperspoon-commands".to_string())
        .spawn(move || {
            for QueuedCommand { command, target } in command_rx {
                if command_status.stopping() {
                    break;
                }
                let outcome = operator.dispatch(command, target.as_ref());
                if outcome_tx.send(outcome).is_err() {
                    break;
                }
            }
        })?;

    let status_for_thread = status.clone();
    let listener_live = Arc::clone(&live);
    let fake_provider_enabled = opts.live_provider == ProviderKind::Fake;
    let provider = match opts.live_provider {
        ProviderKind::Fake => "Fake",
        ProviderKind::AppleMusic => "Apple Music",
    };
    let header = format!(
        "PaperSpoon TCP :{tcp_port} UDP :{} · {:?} · {provider}",
        paper_protocol::DISCOVERY_PORT,
        opts.compression
    );
    let session_worker = std::thread::Builder::new()
        .name("paperspoon-accept".to_string())
        .spawn(move || {
            run_listener(
                listener,
                &opts,
                current,
                frame_sender,
                listener_live,
                application_viewport_rx,
                status_for_thread,
            );
        })?;

    let mut app = TuiApp::new(AppContext {
        viewport: None,
        fake_provider_enabled,
    });
    app.set_header(&header, "Disconnected");

    let result = if status.stopping() {
        Ok(())
    } else {
        run_tui_event_loop(&mut app, &status, &command_tx, &outcome_rx, &live, &header)
    };

    status.stop();
    drop(command_tx);
    live.shutdown();
    for worker in [session_worker, command_worker, discovery_worker] {
        if worker.join().is_err() {
            return result.and(Err(io::Error::other("PaperSpoon worker panicked")));
        }
    }
    result
}

#[cfg(unix)]
#[derive(Default)]
struct StopSignals(Vec<signal_hook::SigId>);

#[cfg(unix)]
impl StopSignals {
    fn register(stop: &Arc<AtomicBool>) -> io::Result<Self> {
        use signal_hook::consts::{SIGINT, SIGTERM};

        let mut signals = Self::default();
        for signal in [SIGINT, SIGTERM] {
            signals
                .0
                .push(signal_hook::flag::register(signal, Arc::clone(stop))?);
        }
        Ok(signals)
    }
}

#[cfg(unix)]
impl Drop for StopSignals {
    fn drop(&mut self) {
        for id in self.0.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}
