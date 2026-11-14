//! X11 lifecycle polling and raw event diagnostics.
//!
//! Linux runs dispatch native evdev input and ignore X11 button events. X11
//! pointer decoding remains only for the non-Linux fallback. Rendering is
//! delegated to the sibling adapter, and the UI layer never sees X11 types.

#[cfg(target_os = "linux")]
use crate::app::{Activation, AppState};
use crate::display::{DisplayBackend, RedrawCause};
#[cfg(target_os = "linux")]
use crate::input::evdev::{DecodedInput, EvdevReader};
use crate::latency;
use crate::net::Paperspoon;
use crate::runtime::{GeometryOutcome, PaperpadRuntime, RuntimeControl, RuntimePointerInput};
#[cfg(not(target_os = "linux"))]
use crate::ui::button::{PointerEvent, PointerEventKind};
#[cfg(not(target_os = "linux"))]
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

/// Native local-Exit and X11 lifecycle pump for the MXCFB benchmark.
///
/// It deliberately forwards no remote pointer messages. X11 button events are
/// diagnostic-only, while window loss and fixed-geometry divergence still stop
/// the benchmark.
#[cfg(target_os = "linux")]
pub(crate) struct BenchmarkExitPoll {
    app: AppState,
    window_destroyed: bool,
}

#[cfg(target_os = "linux")]
impl BenchmarkExitPoll {
    pub(crate) fn new(size: (u16, u16)) -> Self {
        Self {
            app: AppState::new(size),
            window_destroyed: false,
        }
    }

    pub(crate) fn window_destroyed(&self) -> bool {
        self.window_destroyed
    }

    pub(crate) fn shutdown(&mut self, input_reader: &mut EvdevReader) {
        if input_reader.shutdown().is_some() {
            self.app.cancel_contact();
            eprintln!("input source=evdev action=cancel reason=benchmark_shutdown");
        }
    }

    pub(crate) fn poll(
        &mut self,
        input_reader: &mut EvdevReader,
        conn: &RustConnection,
        win: Window,
    ) -> Result<bool> {
        let decoded = match input_reader.drain() {
            Ok(decoded) => decoded,
            Err(error) => {
                if error.cancel_contact() {
                    self.app.cancel_contact();
                    eprintln!("input source=evdev action=cancel reason=reader_failure");
                }
                return Err(error).context("native evdev input reader failed during benchmark");
            }
        };

        for input in decoded {
            match input {
                DecodedInput::Pointer(event) => {
                    eprintln!(
                        "input source=evdev type={:?} detail={} physical_x={} physical_y={}",
                        event.kind, event.detail, event.point.x, event.point.y
                    );
                    if self
                        .app
                        .handle_pointer(event)
                        .activation()
                        .is_some_and(dispatch_activation)
                    {
                        return Ok(true);
                    }
                }
                DecodedInput::CancelContact => {
                    self.app.cancel_contact();
                    eprintln!("input source=evdev action=cancel reason=decoder_state");
                }
            }
        }

        while let Some(event) = conn
            .poll_for_event()
            .context("X11 connection failed while polling benchmark lifecycle")?
        {
            match event {
                Event::ButtonPress(event) if event.event == win => {
                    eprintln!("{}", format_pointer_event("ButtonPress", &event));
                    eprintln!("input source=x11 ignored=true reason=evdev_active");
                }
                Event::ButtonRelease(event) if event.event == win => {
                    eprintln!("{}", format_pointer_event("ButtonRelease", &event));
                    eprintln!("input source=x11 ignored=true reason=evdev_active");
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

#[cfg(target_os = "linux")]
fn service_native_input(
    runtime: &mut PaperpadRuntime,
    input_reader: &mut EvdevReader,
    paperspoon: &mut Paperspoon,
) -> Result<RuntimeControl> {
    let decoded = match input_reader.drain() {
        Ok(decoded) => decoded,
        Err(error) => {
            if error.cancel_contact() {
                runtime.cancel_contact();
                eprintln!("input source=evdev action=cancel reason=reader_failure");
            }
            return Err(error).context("native evdev input reader failed");
        }
    };

    for input in decoded {
        match input {
            DecodedInput::Pointer(event) => {
                let input_stamp = latency::stamp();
                eprintln!(
                    "input source=evdev type={:?} detail={} physical_x={} physical_y={}",
                    event.kind, event.detail, event.point.x, event.point.y
                );
                let control = runtime.handle_pointer(
                    RuntimePointerInput::new(event, input_stamp, None),
                    paperspoon,
                );
                if control == RuntimeControl::ShutdownRequested {
                    return Ok(control);
                }
            }
            DecodedInput::CancelContact => {
                runtime.cancel_contact();
                eprintln!("input source=evdev action=cancel reason=decoder_state");
            }
        }
    }

    Ok(RuntimeControl::Continue)
}

/// Poll X11 until the runtime requests shutdown, the window is destroyed, or
/// event processing fails.
pub(crate) fn event_loop(
    conn: &RustConnection,
    win: Window,
    display: &mut dyn DisplayBackend,
    paperspoon: &mut Paperspoon,
    #[cfg(target_os = "linux")] input_reader: &mut EvdevReader,
) -> Result<EventLoopExit> {
    let mut runtime = PaperpadRuntime::new(display.dimensions());

    loop {
        runtime.service(display, paperspoon)?;
        #[cfg(target_os = "linux")]
        if service_native_input(&mut runtime, input_reader, paperspoon)?
            == RuntimeControl::ShutdownRequested
        {
            return Ok(EventLoopExit::ShutdownRequested);
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
                eprintln!("{}", format_pointer_event("ButtonPress", &event));
                #[cfg(target_os = "linux")]
                eprintln!("input source=x11 ignored=true reason=evdev_active");
                #[cfg(not(target_os = "linux"))]
                {
                    let input_stamp = latency::stamp();
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
            }
            Event::ButtonPress(_) => {}
            Event::ButtonRelease(event) if event.event == win => {
                eprintln!("{}", format_pointer_event("ButtonRelease", &event));
                #[cfg(target_os = "linux")]
                eprintln!("input source=x11 ignored=true reason=evdev_active");
                #[cfg(not(target_os = "linux"))]
                {
                    let input_stamp = latency::stamp();
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
