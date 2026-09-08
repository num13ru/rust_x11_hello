//! Event loop and raw X11 event diagnostics.
//!
//! This is the boundary layer: it translates `x11rb::protocol::Event` into
//! [`crate::ui::button::PointerEvent`]. Rendering is delegated to the sibling
//! adapter, and the UI layer never sees X11 types.

use super::render::draw;

use crate::app::{Activation, AppState, GeometryUpdate, Redraw};
use crate::net::Paperspoon;
use crate::ui::button::{PointerEvent, PointerEventKind};
use crate::ui::geometry::Point;
use anyhow::{Context, Result, anyhow};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ButtonPressEvent, ConnectionExt, Gcontext, Window};
use x11rb::rust_connection::RustConnection;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventLoopExit {
    WindowDestroyed,
}

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Run the event loop until the window is destroyed or the connection fails.
pub fn event_loop(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    initial_size: (u16, u16),
    paperspoon: &mut Paperspoon,
) -> Result<EventLoopExit> {
    let mut app = AppState::new(initial_size);

    if let Some(text) = paperspoon.poll_display() {
        draw_app(conn, win, gc, app.set_status_text(text))
            .context("failed to redraw status after PaperSpoon command")?;
    }
    loop {
        // Drain any PaperSpoon command received since the last X11 event.
        if let Some(text) = paperspoon.poll_display() {
            eprintln!("display: {text}");
            draw_app(conn, win, gc, app.set_status_text(text))
                .context("failed to redraw status after PaperSpoon command")?;
        }
        let Some(event) = conn
            .poll_for_event()
            .context("X11 connection failed while waiting for an event")?
        else {
            std::thread::sleep(EVENT_POLL_INTERVAL);
            continue;
        };

        match event {
            Event::Expose(event) if event.window == win && event.count == 0 => {
                draw_app(conn, win, gc, app.redraw())
                    .context("failed to redraw final Expose batch")?;
            }
            Event::Expose(_) => {}
            Event::ConfigureNotify(event) if event.window == win => {
                match app.update_geometry((event.width, event.height)) {
                    GeometryUpdate::IgnoredZero => {
                        eprintln!(
                            "event type=ConfigureNotify ignored=zero_geometry width={} height={}",
                            event.width, event.height
                        );
                    }
                    GeometryUpdate::Unchanged => {}
                    GeometryUpdate::Redraw(redraw) => {
                        let (width, height) = redraw.size();
                        eprintln!(
                            "event type=ConfigureNotify x={} y={} width={width} height={height} window=0x{:x}",
                            event.x, event.y, event.window
                        );
                        draw_app(conn, win, gc, redraw)
                            .context("failed to redraw after ConfigureNotify")?;
                    }
                }
            }
            Event::ConfigureNotify(_) => {}
            Event::ButtonPress(event) if event.event == win => {
                eprintln!("{}", format_pointer_event("ButtonPress", &event));
                let Some(activation) = app.handle_pointer(PointerEvent {
                    kind: PointerEventKind::Press,
                    detail: event.detail,
                    point: Point {
                        x: event.event_x,
                        y: event.event_y,
                    },
                }) else {
                    continue;
                };
                if dispatch_activation(activation, paperspoon) {
                    conn.destroy_window(win)
                        .context("failed to destroy window after exit")?
                        .check()
                        .context("X11 server rejected destroy-window")?;
                }
            }
            Event::ButtonPress(_) => {}
            Event::ButtonRelease(event) if event.event == win => {
                eprintln!("{}", format_pointer_event("ButtonRelease", &event));
                let Some(activation) = app.handle_pointer(PointerEvent {
                    kind: PointerEventKind::Release,
                    detail: event.detail,
                    point: Point {
                        x: event.event_x,
                        y: event.event_y,
                    },
                }) else {
                    continue;
                };
                if dispatch_activation(activation, paperspoon) {
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
                app.cancel_contact();
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

fn draw_app(conn: &RustConnection, win: Window, gc: Gcontext, redraw: Redraw<'_>) -> Result<()> {
    let (width, height) = redraw.size();
    draw(conn, win, gc, width, height, redraw.status_text())
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
fn dispatch_activation(activation: Activation, paperspoon: &mut Paperspoon) -> bool {
    match activation {
        Activation::Send { button_id, action } => {
            eprintln!(
                "ui action=activate button={button_id} semantic={}",
                action.id()
            );
            if let Err(error) = paperspoon.send_action(action.id()) {
                eprintln!("transport error: {error:#}");
            }
            false
        }
        Activation::Exit { button_id } => {
            eprintln!(
                "ui action=activate button={button_id} semantic={}",
                crate::ui::action::SemanticAction::Exit.id()
            );
            true
        }
        Activation::Unknown { button_id } => {
            eprintln!("ui action=activate button={button_id} semantic=unknown");
            false
        }
    }
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
}
