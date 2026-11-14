//! X11 event polling, decoding, and raw event diagnostics.
//!
//! This is the boundary layer: it translates `x11rb::protocol::Event` into
//! [`crate::ui::button::PointerEvent`]. Rendering is delegated to the sibling
//! adapter, and the UI layer never sees X11 types.

#[cfg(target_os = "linux")]
use crate::app::{Activation, AppState};
use crate::display::{DisplayBackend, RedrawCause};
use crate::latency;
use crate::net::Paperspoon;
use crate::runtime::{GeometryOutcome, PaperpadRuntime, RuntimeControl, RuntimePointerInput};
use crate::ui::button::{PointerEvent, PointerEventKind};
use crate::ui::screen::PhysicalPoint;
use anyhow::{Context, Result, anyhow};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ButtonPressEvent, Window};
use x11rb::rust_connection::RustConnection;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventLoopExit {
    ShutdownRequested,
    WindowDestroyed,
}

/// Minimal X11 input pump for the local MXCFB benchmark. It deliberately
/// forwards no remote pointer messages, but preserves PaperPad's local Exit
/// behavior and aborts if the fixed framebuffer/window geometry diverges.
#[cfg(target_os = "linux")]
pub(crate) struct LocalExitPoll {
    app: AppState,
    window_destroyed: bool,
}

#[cfg(target_os = "linux")]
impl LocalExitPoll {
    pub(crate) fn new(size: (u16, u16)) -> Self {
        Self {
            app: AppState::new(size),
            window_destroyed: false,
        }
    }

    pub(crate) fn window_destroyed(&self) -> bool {
        self.window_destroyed
    }

    pub(crate) fn poll(&mut self, conn: &RustConnection, win: Window) -> Result<bool> {
        while let Some(event) = conn
            .poll_for_event()
            .context("X11 connection failed while polling benchmark input")?
        {
            match event {
                Event::ButtonPress(event) if event.event == win => {
                    eprintln!("{}", format_pointer_event("ButtonPress", &event));
                    if self.handle_pointer(PointerEventKind::Press, &event) {
                        return Ok(true);
                    }
                }
                Event::ButtonRelease(event) if event.event == win => {
                    eprintln!("{}", format_pointer_event("ButtonRelease", &event));
                    if self.handle_pointer(PointerEventKind::Release, &event) {
                        return Ok(true);
                    }
                }
                Event::ConfigureNotify(event) if event.window == win => {
                    ensure_benchmark_geometry(self.app.size(), (event.width, event.height))?;
                }
                Event::UnmapNotify(event) if event.window == win => {
                    self.app.cancel_contact();
                    eprintln!("event type=UnmapNotify window=0x{:x}", event.window);
                    return Ok(true);
                }
                Event::DestroyNotify(event) if event.window == win => {
                    eprintln!("event type=DestroyNotify window=0x{:x}", event.window);
                    self.window_destroyed = true;
                    return Ok(true);
                }
                Event::Error(error) => return Err(anyhow!("X11 server error: {error:?}")),
                _ => {}
            }
        }
        Ok(false)
    }

    fn handle_pointer(&mut self, kind: PointerEventKind, event: &ButtonPressEvent) -> bool {
        let outcome = self.app.handle_pointer(PointerEvent {
            kind,
            detail: event.detail,
            point: PhysicalPoint {
                x: event.event_x,
                y: event.event_y,
            },
        });
        outcome.activation().is_some_and(dispatch_activation)
    }
}

#[cfg(target_os = "linux")]
fn ensure_benchmark_geometry(expected: (u16, u16), reported: (u16, u16)) -> Result<()> {
    anyhow::ensure!(
        reported == expected,
        "MXCFB benchmark X11 window geometry {reported:?} differs from fixed framebuffer {expected:?}"
    );
    Ok(())
}

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Poll X11 until the runtime requests shutdown, the window is destroyed, or
/// event processing fails.
pub(crate) fn event_loop(
    conn: &RustConnection,
    win: Window,
    display: &mut dyn DisplayBackend,
    paperspoon: &mut Paperspoon,
) -> Result<EventLoopExit> {
    let mut runtime = PaperpadRuntime::new(display.dimensions());

    loop {
        runtime.service(display, paperspoon)?;
        let Some(event) = conn
            .poll_for_event()
            .context("X11 connection failed while waiting for an event")?
        else {
            std::thread::sleep(EVENT_POLL_INTERVAL);
            continue;
        };

        match event {
            Event::Expose(event) if event.window == win && event.count == 0 => {
                runtime
                    .redraw_surface(
                        display,
                        RedrawCause::SurfaceDamage,
                        paperspoon.is_connected(),
                    )
                    .context("failed to redraw final Expose batch")?;
            }
            Event::Expose(_) => {}
            Event::ConfigureNotify(event) if event.window == win => {
                match runtime.update_geometry((event.width, event.height), display, paperspoon)? {
                    GeometryOutcome::IgnoredZero => {
                        eprintln!(
                            "event type=ConfigureNotify ignored=zero_geometry width={} height={}",
                            event.width, event.height
                        );
                    }
                    GeometryOutcome::Unchanged => {}
                    GeometryOutcome::Redrawn => {
                        eprintln!(
                            "event type=ConfigureNotify x={} y={} width={} height={} window=0x{:x}",
                            event.x, event.y, event.width, event.height, event.window
                        );
                    }
                }
            }
            Event::ConfigureNotify(_) => {}
            Event::ButtonPress(event) if event.event == win => {
                let input_stamp = latency::stamp();
                eprintln!("{}", format_pointer_event("ButtonPress", &event));
                let control = runtime.handle_pointer(
                    RuntimePointerInput::new(
                        PointerEvent {
                            kind: PointerEventKind::Press,
                            detail: event.detail,
                            point: PhysicalPoint {
                                x: event.event_x,
                                y: event.event_y,
                            },
                        },
                        input_stamp,
                        Some(u64::from(event.time)),
                    ),
                    paperspoon,
                );
                if control == RuntimeControl::ShutdownRequested {
                    return Ok(EventLoopExit::ShutdownRequested);
                }
            }
            Event::ButtonPress(_) => {}
            Event::ButtonRelease(event) if event.event == win => {
                let input_stamp = latency::stamp();
                eprintln!("{}", format_pointer_event("ButtonRelease", &event));
                let control = runtime.handle_pointer(
                    RuntimePointerInput::new(
                        PointerEvent {
                            kind: PointerEventKind::Release,
                            detail: event.detail,
                            point: PhysicalPoint {
                                x: event.event_x,
                                y: event.event_y,
                            },
                        },
                        input_stamp,
                        Some(u64::from(event.time)),
                    ),
                    paperspoon,
                );
                if control == RuntimeControl::ShutdownRequested {
                    return Ok(EventLoopExit::ShutdownRequested);
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
                runtime.cancel_contact();
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

/// Log the local system activation and request window teardown.
#[cfg(target_os = "linux")]
fn dispatch_activation(activation: Activation) -> bool {
    match activation {
        Activation::Exit => {
            eprintln!("ui action=activate system=exit");
            true
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
