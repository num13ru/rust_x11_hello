//! Pure application state for the Ratatui operator console.
//!
//! Owns focus, button selection, editors, log viewport state and typed
//! command construction. Owns NO terminal state, NO dispatch and NO log
//! buffer; the binary drives it and executes the returned effects through
//! the shared dispatcher.

use std::path::PathBuf;

use crate::now_playing_provider::FakeChange;
use crate::operator::{DiagnosticFrame, DiagnosticPattern, LiveCommand, OperatorCommand};
use crate::operator_log::{LogBuffer, LogPosition, LogWindow};

// ---------- Keys ----------

/// Terminal-independent key input. The binary translates crossterm events
/// into this enum at the edge; everything else stays free of terminal crates.
#[derive(Clone, Debug, PartialEq)]
pub enum TuiKey {
    TabForward,
    TabBackward,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Esc,
    Backspace,
    Char(char),
    /// Toggle follow-tail of the log.
    FollowToggle,
    /// Non-editing quit shortcut (q/Q).
    Quit,
}

// ---------- Focus ----------

#[derive(Clone, Debug, PartialEq)]
pub enum Focus {
    /// The log pane.
    Log,
    /// An action group, by index.
    Group(usize),
}

// ---------- Editors ----------

#[derive(Clone, Debug, PartialEq)]
pub enum EditorKind {
    Viewport,
    /// Gray… — unsigned integer 0..=255.
    GrayValue,
    /// JPEG… — local path (spaces, Unicode).
    JpegPath,
    /// Seek… — finite nonnegative seconds.
    SeekSeconds,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditorState {
    pub kind: EditorKind,
    pub value: String,
}

// ---------- Effects ----------

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// The operator pressed Q. Main should shut down.
    Quit,
    /// A validated, typed command ready for the shared dispatcher.
    Command(OperatorCommand),
    /// A message to log as an operator error.
    Error(String),
    /// A compact status update for the footer line.
    Status(String),
}

// ---------- Buttons ----------

#[derive(Clone, Debug, PartialEq)]
pub enum ButtonKind {
    Viewport,
    Diagnostic(DiagnosticPattern),
    Application,
    GrayEditor,
    JpegEditor,
    NowPlaying,
    Live(LiveCommand),
    SeekEditor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    pub label: String,
    pub kind: ButtonKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub title: String,
    pub buttons: Vec<Button>,
}

/// Runtime context the app needs to decide availability.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppContext {
    /// Negotiated remote viewport in Auto mode; None when disconnected.
    pub viewport: Option<(u16, u16)>,
    /// Whether the selected live provider is a fake provider.
    pub fake_provider_enabled: bool,
}

// ---------- App ----------

pub struct TuiApp {
    context: AppContext,
    formats: Option<paper_protocol::V2PixelFormats>,
    explicit_viewport: Option<(u16, u16)>,
    groups: Vec<Group>,
    focus: Focus,
    selected: Vec<usize>,
    editor: Option<EditorState>,
    /// False = viewport paused; anchor_id is the first visible record id.
    follow: bool,
    anchor_id: u64,
    first_id: u64,
    next_id: u64,
    unseen: u64,
    log_column: u16,
    visible_rows: usize,
    /// Footer status line.
    status: String,
    /// Header bar text (left and right sides plus subtitle line).
    header_left: String,
    header_right: String,
    header_subtitle: String,
}

impl TuiApp {
    pub fn new(context: AppContext) -> Self {
        let groups = Self::build_groups();
        let selected = vec![0; groups.len()];
        Self {
            formats: context.viewport.map(|_| {
                paper_protocol::V2PixelFormats::MONO1.with(paper_protocol::PixelFormat::Gray8)
            }),
            context,
            explicit_viewport: None,
            groups,
            focus: Focus::Group(0),
            selected,
            editor: None,
            follow: true,
            anchor_id: 0,
            first_id: 0,
            next_id: 0,
            unseen: 0,
            log_column: 0,
            visible_rows: 1,
            status: "Ready".to_string(),
            header_left: "PaperSpoon".to_string(),
            header_right: "".to_string(),
            header_subtitle: "".to_string(),
        }
    }

    fn build_groups() -> Vec<Group> {
        vec![
            Group {
                title: "Application".to_string(),
                buttons: vec![
                    Button {
                        label: "Show UI".to_string(),
                        kind: ButtonKind::Application,
                    },
                    Button {
                        label: "Viewport: Auto".to_string(),
                        kind: ButtonKind::Viewport,
                    },
                ],
            },
            Group {
                title: "Diagnostic".to_string(),
                buttons: vec![
                    Button {
                        label: "White".to_string(),
                        kind: ButtonKind::Diagnostic(DiagnosticPattern::White),
                    },
                    Button {
                        label: "Black".to_string(),
                        kind: ButtonKind::Diagnostic(DiagnosticPattern::Black),
                    },
                    Button {
                        label: "Horizontal".to_string(),
                        kind: ButtonKind::Diagnostic(DiagnosticPattern::Horizontal),
                    },
                    Button {
                        label: "Checkerboard".to_string(),
                        kind: ButtonKind::Diagnostic(DiagnosticPattern::Checkerboard),
                    },
                    Button {
                        label: "Border".to_string(),
                        kind: ButtonKind::Diagnostic(DiagnosticPattern::Border),
                    },
                    Button {
                        label: "Corners".to_string(),
                        kind: ButtonKind::Diagnostic(DiagnosticPattern::Corners),
                    },
                ],
            },
            Group {
                title: "Gray8".to_string(),
                buttons: vec![
                    Button {
                        label: "Gradient".to_string(),
                        kind: ButtonKind::Diagnostic(DiagnosticPattern::GrayGradient),
                    },
                    Button {
                        label: "Bars".to_string(),
                        kind: ButtonKind::Diagnostic(DiagnosticPattern::GrayBars),
                    },
                    Button {
                        label: "Gray…".to_string(),
                        kind: ButtonKind::GrayEditor,
                    },
                ],
            },
            Group {
                title: "Images".to_string(),
                buttons: vec![Button {
                    label: "JPEG…".to_string(),
                    kind: ButtonKind::JpegEditor,
                }],
            },
            Group {
                title: "Now Playing".to_string(),
                buttons: vec![
                    Button {
                        label: "Start".to_string(),
                        kind: ButtonKind::Live(LiveCommand::Start),
                    },
                    Button {
                        label: "Stop".to_string(),
                        kind: ButtonKind::Live(LiveCommand::Stop),
                    },
                    Button {
                        label: "Status".to_string(),
                        kind: ButtonKind::Live(LiveCommand::Status),
                    },
                    Button {
                        label: "Refresh".to_string(),
                        kind: ButtonKind::Live(LiveCommand::Refresh),
                    },
                    Button {
                        label: "Snapshot".to_string(),
                        kind: ButtonKind::NowPlaying,
                    },
                ],
            },
            Group {
                title: "Test provider".to_string(),
                buttons: vec![
                    Button {
                        label: "Play".to_string(),
                        kind: ButtonKind::Live(LiveCommand::Fake(FakeChange::Play)),
                    },
                    Button {
                        label: "Pause".to_string(),
                        kind: ButtonKind::Live(LiveCommand::Fake(FakeChange::Pause)),
                    },
                    Button {
                        label: "Next".to_string(),
                        kind: ButtonKind::Live(LiveCommand::Fake(FakeChange::Next)),
                    },
                    Button {
                        label: "Seek…".to_string(),
                        kind: ButtonKind::SeekEditor,
                    },
                    Button {
                        label: "Fail".to_string(),
                        kind: ButtonKind::Live(LiveCommand::Fake(FakeChange::Fail)),
                    },
                    Button {
                        label: "Recover".to_string(),
                        kind: ButtonKind::Live(LiveCommand::Fake(FakeChange::Recover)),
                    },
                    Button {
                        label: "Slow".to_string(),
                        kind: ButtonKind::Live(LiveCommand::Fake(FakeChange::Slow)),
                    },
                    Button {
                        label: "Normal".to_string(),
                        kind: ButtonKind::Live(LiveCommand::Fake(FakeChange::Normal)),
                    },
                ],
            },
        ]
    }

    // ---- runtime context ----

    pub fn set_display(&mut self, display: Option<((u16, u16), paper_protocol::V2PixelFormats)>) {
        self.context.viewport = display.map(|(viewport, _)| viewport);
        self.formats = display.map(|(_, formats)| formats);
    }

    fn frame_dimensions(&self) -> Option<(u16, u16)> {
        self.explicit_viewport.or(self.context.viewport)
    }

    pub fn set_fake_provider_enabled(&mut self, enabled: bool) {
        self.context.fake_provider_enabled = enabled;
    }

    pub fn set_header(&mut self, left: impl Into<String>, right: impl Into<String>) {
        self.header_left = left.into();
        self.header_right = right.into();
    }

    pub fn set_subtitle(&mut self, subtitle: impl Into<String>) {
        self.header_subtitle = subtitle.into();
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub fn set_visible_rows(&mut self, rows: u16) {
        self.visible_rows = rows as usize;
    }

    // ---- log window observation ----

    /// Observe the latest emitted log window and derive unseen/eviction state.
    /// Call once per render.
    pub fn observe_log(&mut self, window: &LogWindow) {
        self.first_id = window.first_id;
        self.next_id = window.next_id;

        if self.follow {
            self.unseen = 0;
            return;
        }

        // Anchor eviction: if the visible records now start later than the
        // stored anchor, clamp it forward instead of resurrecting a stale id.
        if window.start_id != self.anchor_id {
            self.anchor_id = window.start_id;
        }

        let visible_end = window.start_id.saturating_add(window.records.len() as u64);
        self.unseen = self.next_id.saturating_sub(visible_end);
    }

    // ---- button availability ----

    fn button_enabled(&self, kind: &ButtonKind) -> bool {
        use paper_protocol::PixelFormat;
        let supports = |format| self.formats.is_some_and(|formats| formats.supports(format));
        match kind {
            ButtonKind::Viewport => true,
            ButtonKind::Diagnostic(pattern) => {
                self.frame_dimensions().is_some() && supports(pattern.pixel_format())
            }
            ButtonKind::Application => {
                self.frame_dimensions().is_some() && supports(PixelFormat::Mono1)
            }
            ButtonKind::GrayEditor => {
                self.frame_dimensions().is_some() && supports(PixelFormat::Gray8)
            }
            ButtonKind::JpegEditor => supports(PixelFormat::Gray8),
            ButtonKind::NowPlaying => cfg!(target_os = "macos") && supports(PixelFormat::Gray8),
            ButtonKind::Live(LiveCommand::Fake(_)) => self.context.fake_provider_enabled,
            ButtonKind::Live(_) => true,
            ButtonKind::SeekEditor => self.context.fake_provider_enabled,
        }
    }

    fn selected_button_enabled(&self, group_index: usize, button_index: usize) -> bool {
        self.groups
            .get(group_index)
            .and_then(|group| group.buttons.get(button_index))
            .is_some_and(|button| self.button_enabled(&button.kind))
    }

    // ---- navigation ----

    fn next_focus(&mut self, forward: bool) {
        let last = self.groups.len().saturating_sub(1);
        let current = self.focus.clone();
        self.focus = match (current, forward) {
            (Focus::Log, true) => Focus::Group(0),
            (Focus::Log, false) => Focus::Group(last),
            (Focus::Group(i), true) => {
                if i >= last {
                    Focus::Log
                } else {
                    Focus::Group(i + 1)
                }
            }
            (Focus::Group(0), false) => Focus::Log,
            (Focus::Group(i), false) => Focus::Group(i - 1),
        };
    }

    fn navigate(&mut self, delta: i64) {
        match self.focus {
            Focus::Log => {
                self.scroll_log(delta);
            }
            Focus::Group(i) => {
                let buttons = self.groups[i].buttons.len();
                if buttons == 0 {
                    return;
                }
                let mut candidate = self.selected[i] as i64;
                for _ in 0..buttons {
                    candidate = (candidate + delta).rem_euclid(buttons as i64);
                    if self.selected_button_enabled(i, candidate as usize) {
                        self.selected[i] = candidate as usize;
                        return;
                    }
                }
                // No enabled button reachable; keep the current selection.
            }
        }
    }

    fn scroll_log(&mut self, delta: i64) {
        if delta == 0 {
            return;
        }
        let rows = self.visible_rows.max(1) as i64;
        let max_start = self.next_id.saturating_sub(rows as u64) as i64;
        let start = if self.follow {
            max_start
        } else {
            self.anchor_id as i64
        };
        let new_start =
            (start + delta).clamp(self.first_id as i64, max_start.max(self.first_id as i64));
        let wants_follow = new_start + rows >= self.next_id as i64 && self.next_id > 0;
        self.anchor_id = new_start as u64;
        self.follow = wants_follow;
        if self.follow {
            self.anchor_id = 0;
            self.unseen = 0;
        }
    }

    fn log_home(&mut self) {
        self.follow = false;
        self.anchor_id = self.first_id;
    }

    fn log_end(&mut self) {
        self.follow = true;
        self.anchor_id = 0;
        self.unseen = 0;
    }

    fn toggle_follow(&mut self) {
        if self.follow {
            self.follow = false;
            let rows = self.visible_rows.max(1);
            self.anchor_id = self.next_id.saturating_sub(rows as u64);
        } else {
            self.log_end();
        }
    }

    fn activate(&mut self) -> Option<Effect> {
        match self.focus {
            Focus::Log => None,
            Focus::Group(i) => {
                let button_index = self.selected[i];
                if !self.selected_button_enabled(i, button_index) {
                    return Some(Effect::Status("Action disabled".to_string()));
                }
                let kind = self.groups[i].buttons[button_index].kind.clone();
                self.dispatch(kind)
            }
        }
    }

    pub fn click_button(&mut self, group: usize, button: usize) -> Option<Effect> {
        if self.editor.is_some() || !self.selected_button_enabled(group, button) {
            return None;
        }
        self.focus = Focus::Group(group);
        self.selected[group] = button;
        self.activate()
    }

    pub fn scroll_log_by(&mut self, rows: i64) {
        self.scroll_log(rows);
    }

    fn dispatch(&mut self, kind: ButtonKind) -> Option<Effect> {
        match kind {
            ButtonKind::Viewport => {
                self.open_editor(EditorKind::Viewport);
                Some(Effect::Status("Viewport: WIDTHxHEIGHT or Auto".into()))
            }
            ButtonKind::Diagnostic(pattern) => {
                let (width, height) = match self.frame_dimensions() {
                    Some((w, h)) => (w, h),
                    None => {
                        return Some(Effect::Error(
                            "No active viewport; cannot send a dimension-dependent frame"
                                .to_string(),
                        ));
                    }
                };
                match DiagnosticFrame::new(pattern, width, height) {
                    Ok(frame) => Some(Effect::Command(OperatorCommand::Frame(frame))),
                    Err(error) => Some(Effect::Error(error)),
                }
            }
            ButtonKind::Application => {
                let (width, height) = match self.frame_dimensions() {
                    Some((w, h)) => (w, h),
                    None => {
                        return Some(Effect::Error(
                            "No active viewport; cannot send the application frame".to_string(),
                        ));
                    }
                };
                match OperatorCommand::application_frame(width, height) {
                    Ok(command) => Some(Effect::Command(command)),
                    Err(error) => Some(Effect::Error(error)),
                }
            }
            ButtonKind::GrayEditor => {
                self.open_editor(EditorKind::GrayValue);
                Some(Effect::Status(
                    "Enter gray value 0-255 — Enter to send, Esc to cancel".to_string(),
                ))
            }
            ButtonKind::JpegEditor => {
                self.open_editor(EditorKind::JpegPath);
                Some(Effect::Status(
                    "Enter JPEG path — Enter to send, Esc to cancel".to_string(),
                ))
            }
            ButtonKind::NowPlaying => Some(Effect::Command(OperatorCommand::NowPlaying)),
            ButtonKind::Live(command) => {
                if matches!(command, LiveCommand::Fake(_)) && !self.context.fake_provider_enabled {
                    return Some(Effect::Error(
                        "Fake provider actions are only available with --now-playing-fake"
                            .to_string(),
                    ));
                }
                Some(Effect::Command(OperatorCommand::LiveNowPlaying(command)))
            }
            ButtonKind::SeekEditor => {
                if !self.context.fake_provider_enabled {
                    return Some(Effect::Error(
                        "Fake provider actions are only available with --now-playing-fake"
                            .to_string(),
                    ));
                }
                self.open_editor(EditorKind::SeekSeconds);
                Some(Effect::Status(
                    "Enter seek position in seconds — Enter to send, Esc to cancel".to_string(),
                ))
            }
        }
    }

    fn open_editor(&mut self, kind: EditorKind) {
        self.editor = Some(EditorState {
            kind,
            value: String::new(),
        });
    }

    // ---- editors ----

    fn handle_editor_key(&mut self, key: TuiKey) -> Option<Effect> {
        let editor = self.editor.as_mut().unwrap();
        match key {
            TuiKey::Esc => {
                self.editor = None;
                Some(Effect::Status("Cancelled".to_string()))
            }
            TuiKey::Enter => self.confirm_editor(),
            TuiKey::Backspace => {
                editor.value.pop();
                None
            }
            TuiKey::Char(c) if !c.is_control() => {
                editor.value.push(c);
                None
            }
            _ => None,
        }
    }

    fn confirm_editor(&mut self) -> Option<Effect> {
        let editor = self.editor.as_ref()?;
        let kind = editor.kind.clone();
        let value = editor.value.trim().to_string();
        match kind {
            EditorKind::Viewport => {
                self.explicit_viewport = if value.is_empty() || value.eq_ignore_ascii_case("auto") {
                    None
                } else {
                    match crate::operator::parse_dimensions(&value) {
                        Ok(dimensions) => Some(dimensions),
                        Err(error) => return Some(Effect::Error(error)),
                    }
                };
                self.editor = None;
                Some(Effect::Status(self.viewport_label()))
            }
            EditorKind::GrayValue => {
                let parsed = value.parse::<u8>();
                let Ok(gray) = parsed else {
                    let err_value = value.clone();
                    self.editor = Some(EditorState {
                        kind: EditorKind::GrayValue,
                        value,
                    });
                    return Some(Effect::Error(format!(
                        "Invalid gray value '{err_value}'; expected an integer 0 through 255"
                    )));
                };
                self.editor = None;
                let (width, height) = match self.frame_dimensions() {
                    Some((w, h)) => (w, h),
                    None => {
                        return Some(Effect::Error(
                            "No active viewport; cannot send a Gray8 frame".to_string(),
                        ));
                    }
                };
                match DiagnosticFrame::new(DiagnosticPattern::Gray(gray), width, height) {
                    Ok(frame) => Some(Effect::Command(OperatorCommand::Frame(frame))),
                    Err(error) => Some(Effect::Error(error)),
                }
            }
            EditorKind::JpegPath => {
                if value.is_empty() {
                    self.editor = Some(EditorState {
                        kind: EditorKind::JpegPath,
                        value,
                    });
                    return Some(Effect::Error("JPEG path cannot be empty".to_string()));
                }
                self.editor = None;
                Some(Effect::Command(OperatorCommand::Jpeg(PathBuf::from(value))))
            }
            EditorKind::SeekSeconds => {
                let parsed = value
                    .parse::<f64>()
                    .ok()
                    .filter(|seconds| seconds.is_finite() && *seconds >= 0.0);
                let Some(seconds) = parsed else {
                    let err_value = value.clone();
                    self.editor = Some(EditorState {
                        kind: EditorKind::SeekSeconds,
                        value,
                    });
                    return Some(Effect::Error(format!(
                        "Invalid seek position '{err_value}'; expected finite nonnegative seconds"
                    )));
                };
                self.editor = None;
                Some(Effect::Command(OperatorCommand::LiveNowPlaying(
                    LiveCommand::Fake(FakeChange::Seek(seconds)),
                )))
            }
        }
    }

    // ---- keys ----

    pub fn handle_key(&mut self, key: TuiKey) -> Option<Effect> {
        if self.editor.is_some() {
            return self.handle_editor_key(key);
        }

        match key {
            TuiKey::Quit => return Some(Effect::Quit),
            TuiKey::TabForward => self.next_focus(true),
            TuiKey::TabBackward => self.next_focus(false),
            TuiKey::Left if self.focus == Focus::Log => {
                self.log_column = self.log_column.saturating_sub(8);
            }
            TuiKey::Right if self.focus == Focus::Log => {
                self.log_column = self.log_column.saturating_add(8);
            }
            TuiKey::Up | TuiKey::Left => self.navigate(-1),
            TuiKey::Down | TuiKey::Right => self.navigate(1),
            TuiKey::PageUp => self.scroll_log(-(self.visible_rows.max(1) as i64)),
            TuiKey::PageDown => self.scroll_log(self.visible_rows.max(1) as i64),
            TuiKey::Home => self.log_home(),
            TuiKey::End => self.log_end(),
            TuiKey::FollowToggle => self.toggle_follow(),
            TuiKey::Enter => return self.activate(),
            _ => {}
        }
        None
    }

    // ---- views ----

    fn viewport_label(&self) -> String {
        self.explicit_viewport.map_or_else(
            || "Viewport: Auto".into(),
            |(width, height)| format!("Viewport: {width}x{height}"),
        )
    }

    pub fn resize_log(&mut self, area: ratatui::layout::Rect) {
        self.set_visible_rows(crate::tui::view::log_rows(area, self.groups.len()));
    }

    /// Build the pure view data model for one frame.
    pub fn view(&self, window: LogWindow) -> crate::tui::view::TuiView<'_> {
        let records: Vec<String> = window
            .records
            .into_iter()
            .map(|record| record.message)
            .collect();
        let (focused_group, selected_button) = match self.focus {
            Focus::Log => (None, 0),
            Focus::Group(i) => (Some(i), self.selected[i]),
        };
        crate::tui::view::TuiView {
            header_left: &self.header_left,
            header_right: &self.header_right,
            header_subtitle: &self.header_subtitle,
            records,
            follow: self.follow,
            unseen: self.unseen,
            log_column: self.log_column,
            evicted: window.first_id,
            dropped: window.dropped,
            groups: self
                .groups
                .iter()
                .enumerate()
                .map(|(index, group)| crate::tui::view::GroupSpec {
                    title: group.title.clone(),
                    buttons: group
                        .buttons
                        .iter()
                        .enumerate()
                        .map(|(button_index, button)| crate::tui::view::ButtonSpec {
                            label: if matches!(button.kind, ButtonKind::Viewport) {
                                self.viewport_label()
                            } else {
                                button.label.clone()
                            },
                            enabled: self.selected_button_enabled(index, button_index),
                        })
                        .collect(),
                })
                .collect(),
            status: &self.status,
            editor: self.editor.as_ref().map(|editor| editor.value.as_str()),
            hints: "Tab: group  Arrows: select  Enter: run  F: follow  Q: quit",
            focused_group,
            selected_button,
        }
    }

    /// The app's current log window, e.g. for status-line signals.
    pub fn log_window(&self, buffer: &LogBuffer) -> LogWindow {
        let position = if self.follow {
            LogPosition::Follow
        } else {
            LogPosition::From(self.anchor_id)
        };
        buffer.window(position, self.visible_rows.max(1))
    }

    pub fn editor(&self) -> Option<&EditorState> {
        self.editor.as_ref()
    }

    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    pub fn focus(&self) -> &Focus {
        &self.focus
    }

    pub fn follow(&self) -> bool {
        self.follow
    }

    pub fn unseen(&self) -> u64 {
        self.unseen
    }

    pub fn status(&self) -> &str {
        &self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator_log::{Level, LogBuffer};

    fn test_context() -> AppContext {
        AppContext {
            viewport: Some((16, 24)),
            fake_provider_enabled: true,
        }
    }

    fn app_with_context(context: AppContext) -> TuiApp {
        TuiApp::new(context)
    }

    fn focus_group(app: &mut TuiApp, group: usize) {
        for _ in 0..50 {
            if app.focus() == &Focus::Group(group) {
                return;
            }
            app.handle_key(TuiKey::TabForward);
        }
        panic!("did not reach group {group}");
    }

    fn select_button(app: &mut TuiApp, group: usize, button: usize) {
        for _ in 0..50 {
            if app.focus() != &Focus::Group(group) {
                app.handle_key(TuiKey::TabForward);
                continue;
            }
            let current = app.selected[group];
            if current == button {
                return;
            }
            app.handle_key(TuiKey::Right);
        }
        panic!("did not reach button {button} in group {group}");
    }

    fn selected_button(app: &TuiApp, group: usize) -> usize {
        app.selected[group]
    }

    #[test]
    fn focus_cycles_through_groups_and_log() {
        let mut app = app_with_context(test_context());
        assert_eq!(app.focus(), &Focus::Group(0));
        app.handle_key(TuiKey::TabForward);
        assert_eq!(app.focus(), &Focus::Group(1));
        app.handle_key(TuiKey::TabBackward);
        assert_eq!(app.focus(), &Focus::Group(0));
        app.handle_key(TuiKey::TabBackward);
        assert_eq!(app.focus(), &Focus::Log);
        app.handle_key(TuiKey::TabForward);
        assert_eq!(app.focus(), &Focus::Group(0));
        // From Log, Shift+Tab stays on the Log.
        app.handle_key(TuiKey::TabBackward);
        assert_eq!(app.focus(), &Focus::Log);
    }

    #[test]
    fn show_ui_dispatches_typed_application_frame() {
        let mut app = app_with_context(test_context());
        focus_group(&mut app, 0);
        let effect = app.handle_key(TuiKey::Enter).unwrap();
        let Effect::Command(OperatorCommand::ApplicationFrame { width, height }) = effect else {
            panic!("expected application frame command, got {effect:?}");
        };
        assert_eq!((width, height), (16, 24));
    }

    #[test]
    fn show_ui_disabled_without_viewport() {
        let mut app = app_with_context(AppContext {
            viewport: None,
            fake_provider_enabled: true,
        });
        focus_group(&mut app, 0);
        let effect = app.handle_key(TuiKey::Enter).unwrap();
        assert!(matches!(effect, Effect::Status(_)));
    }

    #[test]
    fn diagnostic_buttons_dispatch_validated_frames() {
        let mut app = app_with_context(test_context());
        select_button(&mut app, 1, 0); // White
        let effect = app.handle_key(TuiKey::Enter).unwrap();
        let Effect::Command(OperatorCommand::Frame(frame)) = effect else {
            panic!("expected diagnostic frame command, got {effect:?}");
        };
        assert_eq!(frame.pattern_name(), "white");
        assert_eq!(frame.dimensions(), (16, 24));
    }

    #[test]
    fn gray_editor_flow_dispatches_gray_frame() {
        let mut app = app_with_context(test_context());
        select_button(&mut app, 2, 2); // Gray…
        let effect = app.handle_key(TuiKey::Enter).unwrap();
        assert!(matches!(effect, Effect::Status(_)));
        assert!(app.editor().is_some());
        app.handle_key(TuiKey::Char('1'));
        app.handle_key(TuiKey::Char('3'));
        app.handle_key(TuiKey::Char('2'));
        let effect = app.handle_key(TuiKey::Enter).unwrap();
        let Effect::Command(OperatorCommand::Frame(frame)) = effect else {
            panic!("expected diagnostic frame command, got {effect:?}");
        };
        assert_eq!(frame.pattern_name(), "gray");
        assert_eq!(frame.dimensions(), (16, 24));
        assert!(app.editor().is_none());
    }

    #[test]
    fn gray_editor_rejects_invalid_values_without_closing() {
        let mut app = app_with_context(test_context());
        select_button(&mut app, 2, 2);
        app.handle_key(TuiKey::Enter);
        app.handle_key(TuiKey::Char('2'));
        app.handle_key(TuiKey::Char('5'));
        app.handle_key(TuiKey::Char('6'));
        let effect = app.handle_key(TuiKey::Enter).unwrap();
        assert!(matches!(effect, Effect::Error(_)));
        assert!(app.editor().is_some());
        assert_eq!(app.editor().unwrap().value, "256");
    }

    #[test]
    fn gray_editor_esc_cancels() {
        let mut app = app_with_context(test_context());
        select_button(&mut app, 2, 2);
        app.handle_key(TuiKey::Enter);
        app.handle_key(TuiKey::Esc);
        assert!(app.editor().is_none());
    }

    #[test]
    fn jpeg_editor_accepts_spaces_and_dispatches() {
        let mut app = app_with_context(test_context());
        select_button(&mut app, 3, 0);
        app.handle_key(TuiKey::Enter);
        for c in "image with spaces.jpg".chars() {
            app.handle_key(TuiKey::Char(c));
        }
        let effect = app.handle_key(TuiKey::Enter).unwrap();
        let Effect::Command(OperatorCommand::Jpeg(path)) = effect else {
            panic!("expected JPEG command, got {effect:?}");
        };
        assert_eq!(path, PathBuf::from("image with spaces.jpg"));
    }

    #[test]
    fn seek_editor_dispatches_fake_seek() {
        let mut app = app_with_context(test_context());
        select_button(&mut app, 5, 3); // Seek…
        app.handle_key(TuiKey::Enter);
        for c in "15".chars() {
            app.handle_key(TuiKey::Char(c));
        }
        let effect = app.handle_key(TuiKey::Enter).unwrap();
        let Effect::Command(OperatorCommand::LiveNowPlaying(LiveCommand::Fake(FakeChange::Seek(
            seconds,
        )))) = effect
        else {
            panic!("expected fake seek command, got {effect:?}");
        };
        assert_eq!(seconds, 15.0);
    }

    #[test]
    fn fake_provider_disabled_blocks_fake_actions() {
        let mut app = app_with_context(AppContext {
            viewport: Some((16, 24)),
            fake_provider_enabled: false,
        });
        select_button(&mut app, 5, 0);
        assert_eq!(selected_button(&app, 5), 0);
        let effect = app.handle_key(TuiKey::Enter).unwrap();
        assert!(matches!(effect, Effect::Status(_)));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn now_playing_snapshot_dispatches_now_playing_frame() {
        let mut app = app_with_context(test_context());
        select_button(&mut app, 4, 4); // Snapshot
        let effect = app.handle_key(TuiKey::Enter).unwrap();
        assert!(matches!(
            effect,
            Effect::Command(OperatorCommand::NowPlaying)
        ));
    }

    #[test]
    fn quit_key_returns_quit_effect() {
        let mut app = app_with_context(test_context());
        let effect = app.handle_key(TuiKey::Quit);
        assert!(matches!(effect, Some(Effect::Quit)));
    }

    #[test]
    fn log_focus_page_up_pauses_and_end_resumes() {
        let buffer = LogBuffer::new();
        for index in 0..100 {
            buffer.push_str(Level::Info, &format!("record {index}"));
        }
        let mut app = app_with_context(test_context());
        app.set_visible_rows(10);
        app.set_header("PaperSpoon", "");
        app.handle_key(TuiKey::TabForward); // group 0 => group 1 … cycle to Log
        // cycle until Log
        for _ in 0..10 {
            if app.focus() == &Focus::Log {
                break;
            }
            app.handle_key(TuiKey::TabForward);
        }
        assert_eq!(app.focus(), &Focus::Log);

        // initial: follow
        let window = app.log_window(&buffer);
        app.observe_log(&window);
        assert!(app.follow());

        app.handle_key(TuiKey::PageUp);
        assert!(!app.follow());
        let window = app.log_window(&buffer);
        app.observe_log(&window);
        let paused_start = window.start_id;
        assert!(paused_start > 0);

        app.handle_key(TuiKey::End);
        assert!(app.follow());
    }

    #[test]
    fn log_follow_and_unseen_count_are_reported() {
        let buffer = LogBuffer::new();
        for index in 0..50 {
            buffer.push_str(Level::Info, &format!("record {index}"));
        }
        let mut app = app_with_context(test_context());
        app.set_visible_rows(10);
        let window = app.log_window(&buffer);
        app.observe_log(&window);
        assert_eq!(app.unseen(), 0);

        // Pause at the top of the buffer, then add records.
        app.handle_key(TuiKey::Home);
        assert!(!app.follow());
        for index in 50..60 {
            buffer.push_str(Level::Info, &format!("record {index}"));
        }
        let window = app.log_window(&buffer);
        app.observe_log(&window);
        assert!(app.unseen() > 0);
    }

    #[test]
    fn eviction_clamps_anchor_forward() {
        let buffer = LogBuffer::new();
        for index in 0..5005 {
            buffer.push_str(Level::Info, &format!("record {index}"));
        }
        let mut app = app_with_context(test_context());
        app.set_visible_rows(10);
        app.handle_key(TuiKey::Home);
        let window = app.log_window(&buffer);
        app.observe_log(&window);
        // Anchor was 0; after eviction, it clamps to the first retained id.
        assert!(app.anchor_id >= 5);
    }

    #[test]
    fn view_builds_groups_and_selection_markers() {
        let buffer = LogBuffer::new();
        buffer.push_str(Level::Info, "log record");
        let mut app = app_with_context(test_context());
        app.set_visible_rows(10);
        focus_group(&mut app, 0);
        let view = app.view(app.log_window(&buffer));
        assert_eq!(view.groups.len(), 6);
        assert_eq!(view.focused_group, Some(0));
        assert_eq!(view.selected_button, 0);
        let first_group = &view.groups[0];
        let button = &first_group.buttons[0];
        assert!(button.enabled);
        assert_eq!(button.label, "Show UI");
    }
}
