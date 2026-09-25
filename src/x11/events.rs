//! Event loop and raw X11 event diagnostics.
//!
//! This is the boundary layer: it translates `x11rb::protocol::Event` into
//! [`crate::ui::button::PointerEvent`]. Rendering is delegated to the sibling
//! adapter, and the UI layer never sees X11 types.

use crate::app::{Activation, AppState, GeometryUpdate, Redraw, RemotePointerEvent};
use crate::display::{DisplayBackend, RedrawCause, RemoteFrame};
use crate::net::Paperspoon;
use crate::ui::button::{PointerEvent, PointerEventKind};
use crate::ui::screen::{PhysicalPoint, ScreenLayout};
use anyhow::{Context, Result, anyhow};
use paper_protocol::V2PointerPhase;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ButtonPressEvent, ConnectionExt, Window};
use x11rb::rust_connection::RustConnection;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventLoopExit {
    WindowDestroyed,
}

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Run the event loop until the window is destroyed or the connection fails.
pub(crate) fn event_loop(
    conn: &RustConnection,
    win: Window,
    display: &mut dyn DisplayBackend,
    paperspoon: &mut Paperspoon,
) -> Result<EventLoopExit> {
    let mut app = AppState::new(display.dimensions());

    loop {
        if let Some(frame) = paperspoon.poll_frame() {
            match RemoteFrame::new(
                frame.frame_id(),
                frame.width(),
                frame.height(),
                frame.stride(),
                frame.pixels(),
                frame.receive_decode_us(),
            ) {
                Ok(frame) => display.display_remote_frame(frame),
                Err(error) => eprintln!(
                    "frame validation error id={} width={} height={}: {error}",
                    frame.frame_id(),
                    frame.width(),
                    frame.height()
                ),
            }
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
                draw_surface(display, app.redraw(), RedrawCause::SurfaceDamage)
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
                        let viewport = remote_viewport_size((width, height));
                        if let Err(error) = paperspoon.send_viewport_changed(viewport) {
                            eprintln!("transport error sending viewport change: {error:#}");
                        }
                        if let Some(frame) = display.set_dimensions((width, height)) {
                            eprintln!(
                                "frame cache invalidated id={} width={} height={} viewport_width={} viewport_height={}",
                                frame.frame_id(),
                                frame.dimensions().0,
                                frame.dimensions().1,
                                viewport.0,
                                viewport.1
                            );
                        }
                        eprintln!(
                            "event type=ConfigureNotify x={} y={} width={width} height={height} window=0x{:x}",
                            event.x, event.y, event.window
                        );
                        draw_surface(display, redraw, RedrawCause::GeometryChange)
                            .context("failed to redraw after ConfigureNotify")?;
                    }
                }
            }
            Event::ConfigureNotify(_) => {}
            Event::ButtonPress(event) if event.event == win => {
                eprintln!("{}", format_pointer_event("ButtonPress", &event));
                let outcome = app.handle_pointer(PointerEvent {
                    kind: PointerEventKind::Press,
                    detail: event.detail,
                    point: PhysicalPoint {
                        x: event.event_x,
                        y: event.event_y,
                    },
                });
                if let Some(pointer) = outcome.remote() {
                    forward_remote_pointer(pointer, paperspoon);
                }
                let Some(activation) = outcome.activation() else {
                    continue;
                };
                if dispatch_activation(activation) {
                    conn.destroy_window(win)
                        .context("failed to destroy window after exit")?
                        .check()
                        .context("X11 server rejected destroy-window")?;
                }
            }
            Event::ButtonPress(_) => {}
            Event::ButtonRelease(event) if event.event == win => {
                eprintln!("{}", format_pointer_event("ButtonRelease", &event));
                let outcome = app.handle_pointer(PointerEvent {
                    kind: PointerEventKind::Release,
                    detail: event.detail,
                    point: PhysicalPoint {
                        x: event.event_x,
                        y: event.event_y,
                    },
                });
                if let Some(pointer) = outcome.remote() {
                    forward_remote_pointer(pointer, paperspoon);
                }
                let Some(activation) = outcome.activation() else {
                    continue;
                };
                if dispatch_activation(activation) {
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

pub(crate) fn remote_viewport_size(physical_size: (u16, u16)) -> (u16, u16) {
    ScreenLayout::new(physical_size.0, physical_size.1)
        .map(|layout| (layout.remote_viewport.width, layout.remote_viewport.height))
        .unwrap_or((0, 0))
}

fn draw_surface(display: &dyn DisplayBackend, redraw: Redraw, cause: RedrawCause) -> Result<()> {
    debug_assert_eq!(display.dimensions(), redraw.size());
    display.draw_system_ui()?;
    display.redraw_cached_frame(cause);
    Ok(())
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

/// Log the local system activation and request window teardown.
fn dispatch_activation(activation: Activation) -> bool {
    match activation {
        Activation::Exit => {
            eprintln!("ui action=activate system=exit");
            true
        }
    }
}

fn forward_remote_pointer(pointer: RemotePointerEvent, paperspoon: &mut Paperspoon) {
    let phase = match pointer.kind() {
        PointerEventKind::Press => V2PointerPhase::Down,
        PointerEventKind::Release => V2PointerPhase::Up,
    };
    let point = pointer.point();
    if let Err(error) = paperspoon.send_pointer(phase, point.x, point.y) {
        eprintln!(
            "pointer transport error phase={phase:?} x={} y={}: {error:#}",
            point.x, point.y
        );
    } else {
        eprintln!(
            "remote pointer queued phase={phase:?} x={} y={}",
            point.x, point.y
        );
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

    #[test]
    fn remote_viewport_size_excludes_local_system_ui() {
        assert_eq!(remote_viewport_size((1272, 1696)), (1272, 1624));
        assert_eq!(remote_viewport_size((100, 40)), (100, 0));
        assert_eq!(remote_viewport_size((0, 40)), (0, 0));
    }
}
