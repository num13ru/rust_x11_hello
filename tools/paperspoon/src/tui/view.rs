//! Pure Ratatui rendering for the operator console.
//!
//! Presentation only: it borrows plain model data and draws it. Interactive
//! state (focus, editor contents, viewport follow) lives elsewhere.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ButtonSpec {
    pub label: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupSpec {
    pub title: String,
    pub buttons: Vec<ButtonSpec>,
}

/// Everything the view needs for one frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TuiView<'a> {
    /// Left side of the header bar, e.g. `PaperSpoon ─ TCP :5581`.
    pub header_left: &'a str,
    /// Right side of the header bar, e.g. `Connected ─ PPFB v3`.
    pub header_right: &'a str,
    /// Line under the header bar, e.g. `Peer: 192.168.0.3:56724 | Viewport: 1272x1624`.
    pub header_subtitle: &'a str,
    /// Visible log window, oldest first. Empty renders a waiting hint.
    pub records: Vec<String>,
    pub follow: bool,
    /// Records arriving while the viewport was paused.
    pub unseen: u64,
    pub log_column: u16,
    pub evicted: u64,
    pub dropped: u64,
    pub groups: Vec<GroupSpec>,
    /// Status line, e.g. last command outcome.
    pub status: &'a str,
    pub editor: Option<&'a str>,
    /// Footer hints line.
    pub hints: &'a str,
    /// Index of the group that currently owns focus, or None when the log is focused.
    pub focused_group: Option<usize>,
    /// Index of the selected button within the focused group.
    pub selected_button: usize,
}

const MIN_WIDTH: u16 = 30;
const MIN_HEIGHT: u16 = 10;

const DIM: Color = Color::DarkGray;
const ACCENT: Color = Color::Cyan;

fn areas(area: Rect, groups: usize) -> Option<[Rect; 4]> {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return None;
    }
    let group_rows = if area.height >= groups as u16 + MIN_HEIGHT {
        groups.max(1) as u16
    } else {
        1
    };
    let constraints = [
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(group_rows),
        Constraint::Length(3),
    ];
    Some(Layout::vertical(constraints).areas(area))
}

pub fn log_rows(area: Rect, groups: usize) -> u16 {
    areas(area, groups).map_or(0, |[_, log, _, _]| log.height.saturating_sub(2))
}

#[derive(Default)]
pub struct HitAreas {
    log: Rect,
    buttons: Vec<(Rect, usize, usize)>,
}

impl HitAreas {
    pub fn log_contains(&self, column: u16, row: u16) -> bool {
        self.log.contains((column, row).into())
    }

    pub fn button_at(&self, column: u16, row: u16) -> Option<(usize, usize)> {
        self.buttons.iter().find_map(|(area, group, button)| {
            area.contains((column, row).into())
                .then_some((*group, *button))
        })
    }
}

pub fn render(view: &TuiView<'_>, frame: &mut Frame<'_>) -> HitAreas {
    let area = frame.area();
    let Some([header, log, actions, footer]) = areas(area, view.groups.len()) else {
        render_too_small(area, frame);
        return HitAreas::default();
    };
    render_header(view, header, frame);
    render_log(view, log, frame);
    let buttons = render_groups(view, actions, frame);
    render_footer(view, footer, frame);
    HitAreas { log, buttons }
}

fn render_header(view: &TuiView<'_>, area: Rect, frame: &mut Frame<'_>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::from(vec![
            Span::styled(
                view.header_left,
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::new()),
            Span::styled(view.header_right, Style::new().fg(DIM)),
        ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            view.header_subtitle,
            Style::new().fg(DIM),
        ))),
        inner,
    );
}

fn render_log(view: &TuiView<'_>, area: Rect, frame: &mut Frame<'_>) {
    let title = if view.follow {
        " Realtime log ─ FOLLOW ".to_string()
    } else {
        format!(" Realtime log ─ PAUSED ─ {} unseen ", view.unseen)
    };
    let title = if view.dropped > 0 || view.evicted > 0 {
        format!(
            "{title}· {} evicted · {} dropped ",
            view.evicted, view.dropped
        )
    } else {
        title
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::new().fg(DIM));
    let mut lines: Vec<Line<'_>> = view
        .records
        .iter()
        .map(|record| Line::from(record.as_str()))
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(waiting for events)",
            Style::new().fg(DIM),
        )));
    }
    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((0, view.log_column));
    frame.render_widget(paragraph, area);
}

fn render_groups(
    view: &TuiView<'_>,
    area: Rect,
    frame: &mut Frame<'_>,
) -> Vec<(Rect, usize, usize)> {
    let mut hits = Vec::new();
    let first_group = if (area.height as usize) < view.groups.len() {
        view.focused_group.unwrap_or_default()
    } else {
        0
    };
    for (row, (group_index, group)) in view
        .groups
        .iter()
        .enumerate()
        .skip(first_group)
        .take(area.height as usize)
        .enumerate()
    {
        let chunk = Rect::new(area.x, area.y + row as u16, area.width, 1);
        let title = Span::styled(
            format!("[{}] ", group.title),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        );
        let title_width = title.width();
        let mut labels = Vec::new();
        for (button_index, button) in group.buttons.iter().enumerate() {
            let is_selected =
                view.focused_group == Some(group_index) && button_index == view.selected_button;
            let style = if is_selected {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else if button.enabled {
                Style::new().fg(Color::White)
            } else {
                Style::new().fg(DIM).add_modifier(Modifier::DIM)
            };
            let label = if !button.enabled {
                format!("({}) ", button.label)
            } else if is_selected {
                format!("»{}« ", button.label)
            } else {
                format!("[{}] ", button.label)
            };
            labels.push(Span::styled(label, style));
        }
        let selected = if view.focused_group == Some(group_index) {
            view.selected_button
        } else {
            0
        };
        let first = if title_width
            + labels
                .iter()
                .take(selected + 1)
                .map(Span::width)
                .sum::<usize>()
            > area.width as usize
        {
            selected
        } else {
            0
        };
        let mut spans = vec![title];
        let mut x = title_width;
        for (index, label) in labels.into_iter().enumerate().skip(first) {
            let width = label.width();
            if x + width > area.width as usize && index != selected {
                break;
            }
            if x < area.width as usize {
                let hit_width = width.min(area.width as usize - x) as u16;
                if group.buttons[index].enabled {
                    hits.push((
                        Rect::new(area.x + x as u16, chunk.y, hit_width, 1),
                        group_index,
                        index,
                    ));
                }
                spans.push(label);
            }
            x += width;
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), chunk);
    }
    hits
}

fn render_footer(view: &TuiView<'_>, area: Rect, frame: &mut Frame<'_>) {
    if let Some(value) = view.editor {
        let [prompt, input] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        frame.render_widget(
            Paragraph::new(view.status).style(Style::new().fg(ACCENT)),
            prompt,
        );
        let line = Line::from(format!("> {value}▏"));
        let scroll = line
            .width()
            .saturating_sub(input.width as usize)
            .min(u16::MAX as usize) as u16;
        frame.render_widget(Paragraph::new(line).scroll((0, scroll)), input);
        return;
    }
    let hints = if area.width < 66 {
        "Tab · Enter · F · Q"
    } else {
        view.hints
    };
    let paragraph = Paragraph::new(vec![
        Line::styled(view.status, Style::new().fg(ACCENT)),
        Line::styled(hints, Style::new().fg(DIM)),
    ])
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::new().fg(DIM)),
    );
    frame.render_widget(paragraph, area);
}

fn render_too_small(area: Rect, frame: &mut Frame<'_>) {
    let message = Paragraph::new(vec![
        Line::from(Span::styled(
            "Terminal too small — resize",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled("(or quit with Ctrl-C)", Style::new().fg(DIM))),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(ACCENT)),
    );
    frame.render_widget(message, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn sample_view() -> TuiView<'static> {
        TuiView {
            header_left: "PaperSpoon ─ TCP :5581",
            header_right: "Connected ─ PPFB v3",
            header_subtitle: "Peer: 192.168.0.3:56724 | Viewport: 1272x1624",
            records: vec![
                "discovery request from=192.168.0.3:5582 nonce=01".to_string(),
                "connected: 192.168.0.3:56724".to_string(),
                "uneven: café 🦀 unicode record that is reasonably long".to_string(),
                String::new(),
            ],
            follow: true,
            unseen: 0,
            log_column: 0,
            evicted: 0,
            dropped: 0,
            groups: vec![
                GroupSpec {
                    title: "Application".to_string(),
                    buttons: vec![
                        ButtonSpec {
                            label: "Show UI".to_string(),
                            enabled: true,
                        },
                        ButtonSpec {
                            label: "Viewport: Auto".to_string(),
                            enabled: true,
                        },
                    ],
                },
                GroupSpec {
                    title: "Diagnostic".to_string(),
                    buttons: vec![
                        ButtonSpec {
                            label: "White".to_string(),
                            enabled: true,
                        },
                        ButtonSpec {
                            label: "Black".to_string(),
                            enabled: false,
                        },
                    ],
                },
            ],
            status: "listening",
            editor: None,
            hints: "Tab: group  Arrows: select  Enter: run  Q: quit",
            focused_group: None,
            selected_button: 0,
        }
    }

    fn render_to_string(view: &TuiView, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(view, frame);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_header_log_groups_and_footer_at_regular_size() {
        let output = render_to_string(&sample_view(), 80, 24);
        assert!(output.contains("PaperSpoon ─ TCP :5581"));
        assert!(output.contains("Connected ─ PPFB v3"));
        assert!(output.contains("192.168.0.3:56724"));
        assert!(output.contains("Realtime log"));
        assert!(output.contains("FOLLOW"));
        assert!(output.contains("discovery request"));
        assert!(output.contains("café 🦀"));
        assert!(output.contains("[Application]"));
        assert!(output.contains("[Show UI]"));
        assert!(output.contains("[White]"));
        assert!(output.contains("(Black)"));
        assert!(output.contains("listening"));
        assert!(output.contains("Tab: group"));
    }

    #[test]
    fn renders_small_terminal_prompt() {
        let output = render_to_string(&sample_view(), 29, 9);
        assert!(output.contains("Terminal too small"));
        assert!(output.contains("Ctrl-C"));
    }

    #[test]
    fn empty_log_shows_waiting_hint() {
        let mut view = sample_view();
        view.records.clear();
        let output = render_to_string(&view, 80, 24);
        assert!(output.contains("(waiting for events)"));
    }

    #[test]
    fn paused_follow_shows_unseen_count() {
        let mut view = sample_view();
        view.follow = false;
        view.unseen = 7;
        let output = render_to_string(&view, 80, 24);
        assert!(output.contains("PAUSED"));
        assert!(output.contains("7 unseen"));
    }

    #[test]
    fn zero_size_terminal_does_not_panic() {
        let _ = render_to_string(&sample_view(), 1, 1);
        let _ = render_to_string(&sample_view(), 0, 0);
    }

    #[test]
    fn compact_layout_renders_focused_group_and_editor() {
        let mut view = sample_view();
        view.focused_group = Some(1);
        view.editor = Some("café qf.jpg");
        let output = render_to_string(&view, 34, 10);
        assert!(output.contains("»White«"));
        assert!(output.contains("> café qf.jpg"));
        assert!(!output.contains("[Application]"));
    }

    #[test]
    fn only_enabled_drawn_buttons_are_mouse_targets() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let view = sample_view();
        let mut hits = HitAreas::default();
        terminal.draw(|frame| hits = render(&view, frame)).unwrap();
        let [_, log, actions, _] = areas(Rect::new(0, 0, 80, 24), 2).unwrap();

        assert!(hits.log_contains(log.x + 1, log.y + 1));
        assert_eq!(hits.button_at(14, actions.y), Some((0, 0)));
        assert_eq!(hits.button_at(14, actions.y + 1), Some((1, 0)));
        assert_eq!(hits.button_at(23, actions.y + 1), None);
    }
}
