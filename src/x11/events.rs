//! Event loop, raw event diagnostics, and rendering onto X11.
//!
//! This is the boundary layer: it translates `x11rb::protocol::Event` into
//! [`crate::ui::button::PointerEvent`] and renders logical layouts with
//! core-X11 requests. The UI layer never sees X11 types.

use crate::net::Companion;
use crate::ui::action::{SemanticAction, action_for_button};
use crate::ui::button::{ContactTracker, PointerEvent, PointerEventKind, handle_pointer_event};
use crate::ui::geometry::{Point, WINDOW_HEIGHT, WINDOW_WIDTH, draw_layout};
use anyhow::{Context, Result, anyhow};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ButtonPressEvent, ConnectionExt, Gcontext, Rectangle, Window};
use x11rb::rust_connection::RustConnection;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventLoopExit {
    WindowDestroyed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GeometryUpdate {
    IgnoredZero,
    Unchanged,
    Changed { width: u16, height: u16 },
}
/// Left inset of the status text drawn below the grid.
const STATUS_TEXT_X: u16 = 20;
/// Vertical distance from the window's bottom edge to the status baseline.
const STATUS_TEXT_BOTTOM_MARGIN: u16 = 10;

/// Run the event loop until the window is destroyed or the connection fails.
pub fn event_loop(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    companion: &mut Companion,
) -> Result<EventLoopExit> {
    let mut width = WINDOW_WIDTH;
    let mut height = WINDOW_HEIGHT;
    let mut contact = ContactTracker::default();
    let mut status_text: Option<String> = None;

    if let Some(text) = companion.poll_display() {
        status_text = Some(text);
        draw(conn, win, gc, width, height, status_text.as_deref())
            .context("failed to redraw status after companion command")?;
    }
    loop {
        // Drain any companion command received since the last X11 event.
        if let Some(text) = companion.poll_display() {
            eprintln!("display: {text}");
            status_text = Some(text);
            draw(conn, win, gc, width, height, status_text.as_deref())
                .context("failed to redraw status after companion command")?;
        }
        let event = conn
            .wait_for_event()
            .context("X11 connection failed while waiting for an event")?;

        match event {
            Event::Expose(event) if event.window == win && event.count == 0 => {
                draw(conn, win, gc, width, height, status_text.as_deref())
                    .context("failed to redraw final Expose batch")?;
            }
            Event::Expose(_) => {}
            Event::ConfigureNotify(event) if event.window == win => {
                match geometry_update((width, height), (event.width, event.height)) {
                    GeometryUpdate::IgnoredZero => {
                        eprintln!(
                            "event type=ConfigureNotify ignored=zero_geometry width={} height={}",
                            event.width, event.height
                        );
                    }
                    GeometryUpdate::Unchanged => {}
                    GeometryUpdate::Changed {
                        width: new_width,
                        height: new_height,
                    } => {
                        contact.cancel();
                        width = new_width;
                        height = new_height;
                        eprintln!(
                            "event type=ConfigureNotify x={} y={} width={width} height={height} window=0x{:x}",
                            event.x, event.y, event.window
                        );
                        draw(conn, win, gc, width, height, status_text.as_deref())
                            .context("failed to redraw after ConfigureNotify")?;
                    }
                }
            }
            Event::ConfigureNotify(_) => {}
            Event::ButtonPress(event) if event.event == win => {
                eprintln!("{}", format_pointer_event("ButtonPress", &event));
                let Some(button_id) = handle_pointer_event(
                    &mut contact,
                    PointerEvent {
                        kind: PointerEventKind::Press,
                        detail: event.detail,
                        point: Point {
                            x: event.event_x,
                            y: event.event_y,
                        },
                    },
                    width,
                    height,
                ) else {
                    continue;
                };
                if log_activation(button_id, companion) {
                    conn.destroy_window(win)
                        .context("failed to destroy window after exit")?
                        .check()
                        .context("X11 server rejected destroy-window")?;
                }
            }
            Event::ButtonPress(_) => {}
            Event::ButtonRelease(event) if event.event == win => {
                eprintln!("{}", format_pointer_event("ButtonRelease", &event));
                let Some(button_id) = handle_pointer_event(
                    &mut contact,
                    PointerEvent {
                        kind: PointerEventKind::Release,
                        detail: event.detail,
                        point: Point {
                            x: event.event_x,
                            y: event.event_y,
                        },
                    },
                    width,
                    height,
                ) else {
                    continue;
                };
                if log_activation(button_id, companion) {
                    conn.destroy_window(win)
                        .context("failed to destroy window after exit")?
                        .check()
                        .context("X11 server rejected destroy-window")?;
                }
            }
            Event::ButtonRelease(_) => {}
            Event::MotionNotify(_) => {
                // Motion is selected so drag behavior can be observed later, but normal
                // motion must not drown out press/release diagnostics on an e-ink device.
            }
            Event::MapNotify(event) if event.window == win => {
                eprintln!("event type=MapNotify window=0x{:x}", event.window);
            }
            Event::MapNotify(_) => {}
            Event::UnmapNotify(event) if event.window == win => {
                contact.cancel();
                eprintln!("event type=UnmapNotify window=0x{:x}", event.window);
            }
            Event::UnmapNotify(_) => {}
            Event::DestroyNotify(event) if event.window == win => {
                eprintln!("event type=DestroyNotify window=0x{:x}", event.window);
                return Ok(EventLoopExit::WindowDestroyed);
            }
            Event::DestroyNotify(_) => {}
            Event::Error(error) => return Err(anyhow!("X11 server error: {error:?}")),
            Event::Unknown(bytes) => {
                eprintln!("event type=Unknown bytes={}", bytes.len());
            }
            _ => eprintln!("event type=Other"),
        }
    }
}

/// One-line raw diagnostic for a press or release event.
pub fn format_pointer_event(event_type: &str, event: &ButtonPressEvent) -> String {
    format!(
        "input type={event_type} detail={} event_x={} event_y={} root_x={} root_y={} time={} window=0x{:x} root=0x{:x} child=0x{:x} state=0x{:04x} same_screen={}",
        event.detail,
        event.event_x,
        event.event_y,
        event.root_x,
        event.root_y,
        event.time,
        event.event,
        event.root,
        event.child,
        u16::from(event.state),
        event.same_screen,
    )
}

/// Log one activation and send its semantic action over the transport.
///
/// Returns `true` when the activated action requests window teardown
/// (the exit button). The caller then destroys the window to end the loop
/// cleanly instead of waiting for the watchdog.
fn log_activation(button_id: u8, companion: &mut Companion) -> bool {
    match action_for_button(button_id) {
        Some(action) => {
            eprintln!(
                "ui action=activate button={button_id} semantic={}",
                action.id()
            );
            if action == SemanticAction::Exit {
                return true;
            }
            if let Err(error) = companion.send_action(action.id()) {
                eprintln!("transport error: {error:#}");
            }
            false
        }
        None => {
            eprintln!("ui action=activate button={button_id} semantic=unknown");
            false
        }
    }
}
fn geometry_update(current: (u16, u16), reported: (u16, u16)) -> GeometryUpdate {
    if reported.0 == 0 || reported.1 == 0 {
        GeometryUpdate::IgnoredZero
    } else if reported == current {
        GeometryUpdate::Unchanged
    } else {
        GeometryUpdate::Changed {
            width: reported.0,
            height: reported.1,
        }
    }
}

fn draw(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
    status_text: Option<&str>,
) -> Result<()> {
    let Some(layout) = draw_layout(width, height) else {
        return Ok(());
    };

    conn.clear_area(false, win, 0, 0, width, height)
        .context("failed to send clear-area request")?
        .check()
        .context("X11 server rejected clear-area request")?;

    let rectangles: Vec<Rectangle> = layout
        .rectangles
        .iter()
        .map(|rectangle| Rectangle {
            x: rectangle.x,
            y: rectangle.y,
            width: rectangle.width,
            height: rectangle.height,
        })
        .collect();
    if !rectangles.is_empty() {
        conn.poly_rectangle(win, gc, &rectangles)
            .context("failed to send rectangle draw request")?
            .check()
            .context("X11 server rejected rectangle draw request")?;
    }

    for placement in layout.text {
        if placement.y > 0 {
            draw_text(conn, win, gc, placement.x, placement.y, placement.text)?;
        }
    }

    if let Some(text) = status_text {
        // Below the grid's bottom inset: the last 20 reference px are clear.
        let status_y = height
            .saturating_sub(STATUS_TEXT_BOTTOM_MARGIN)
            .min(i16::MAX as u16) as i16;
        draw_text(
            conn,
            win,
            gc,
            STATUS_TEXT_X as i16,
            status_y,
            text.as_bytes(),
        )?;
    }

    conn.flush().context("failed to flush draw requests")?;
    Ok(())
}

fn draw_text(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    x: i16,
    y: i16,
    text: &[u8],
) -> Result<()> {
    conn.image_text8(win, gc, x, y, text)
        .context("failed to send text draw request")?
        .check()
        .context("X11 server rejected text draw request")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::xproto::KeyButMask;

    #[test]
    fn pointer_diagnostic_preserves_raw_non_primary_and_outside_coordinates() {
        let event = ButtonPressEvent {
            response_type: 4,
            detail: 3,
            sequence: 7,
            time: 123_456,
            root: 0x50d,
            event: 0x2600001,
            child: 0,
            root_x: 79,
            root_y: 480,
            event_x: -1,
            event_y: 360,
            state: KeyButMask::SHIFT | KeyButMask::BUTTON1,
            same_screen: true,
        };

        assert_eq!(
            format_pointer_event("ButtonRelease", &event),
            "input type=ButtonRelease detail=3 event_x=-1 event_y=360 root_x=79 root_y=480 time=123456 window=0x2600001 root=0x50d child=0x0 state=0x0101 same_screen=true"
        );
    }

    #[test]
    fn geometry_updates_reject_zero_and_distinguish_actual_changes() {
        assert_eq!(
            geometry_update((WINDOW_WIDTH, WINDOW_HEIGHT), (0, WINDOW_HEIGHT)),
            GeometryUpdate::IgnoredZero
        );
        assert_eq!(
            geometry_update((WINDOW_WIDTH, WINDOW_HEIGHT), (WINDOW_WIDTH, 0)),
            GeometryUpdate::IgnoredZero
        );
        assert_eq!(
            geometry_update((WINDOW_WIDTH, WINDOW_HEIGHT), (WINDOW_WIDTH, WINDOW_HEIGHT)),
            GeometryUpdate::Unchanged
        );
        assert_eq!(
            geometry_update(
                (WINDOW_WIDTH, WINDOW_HEIGHT),
                (WINDOW_WIDTH / 2, WINDOW_HEIGHT / 2)
            ),
            GeometryUpdate::Changed {
                width: WINDOW_WIDTH / 2,
                height: WINDOW_HEIGHT / 2,
            }
        );
    }

    #[test]
    fn pointer_events_activate_only_within_the_same_button() {
        let (width, height) = (WINDOW_WIDTH, WINDOW_HEIGHT);
        let mut contact = ContactTracker::default();

        // Primary press inside button 1, release inside button 2: no activation.
        let button_1 = Point {
            x: (GRID_RECT_LEFT + GRID_ROWS_CELL_WIDTH / 2) as i16,
            y: (GRID_RECT_TOP + GRID_ROWS_CELL_HEIGHT / 2) as i16,
        };
        let button_2 = Point {
            x: (GRID_RECT_LEFT + GRID_ROWS_CELL_WIDTH + GRID_ROWS_CELL_WIDTH / 2) as i16,
            y: (GRID_RECT_TOP + GRID_ROWS_CELL_HEIGHT / 2) as i16,
        };
        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEvent {
                    kind: PointerEventKind::Press,
                    detail: 1,
                    point: button_1,
                },
                width,
                height,
            ),
            None
        );
        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEvent {
                    kind: PointerEventKind::Release,
                    detail: 1,
                    point: button_2,
                },
                width,
                height,
            ),
            None
        );

        // Press outside the grid: no activation.
        let outside = Point { x: 5, y: 5 };
        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEvent {
                    kind: PointerEventKind::Press,
                    detail: 1,
                    point: outside,
                },
                width,
                height,
            ),
            None
        );
        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEvent {
                    kind: PointerEventKind::Release,
                    detail: 1,
                    point: outside,
                },
                width,
                height,
            ),
            None
        );

        // Same-button press/release in button 4: exactly one activation.
        let inside_4 = Point {
            x: (GRID_RECT_LEFT + GRID_ROWS_CELL_WIDTH / 2) as i16,
            y: (GRID_RECT_TOP + GRID_ROWS_CELL_HEIGHT + GRID_ROWS_CELL_HEIGHT / 2) as i16,
        };
        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEvent {
                    kind: PointerEventKind::Press,
                    detail: 1,
                    point: inside_4,
                },
                width,
                height,
            ),
            None
        );
        assert_eq!(
            handle_pointer_event(
                &mut contact,
                PointerEvent {
                    kind: PointerEventKind::Release,
                    detail: 1,
                    point: inside_4,
                },
                width,
                height,
            ),
            Some(4)
        );
    }

    use crate::ui::geometry::{
        GRID_RECT_LEFT, GRID_RECT_TOP, GRID_ROWS_CELL_HEIGHT, GRID_ROWS_CELL_WIDTH, Point,
    };
}
