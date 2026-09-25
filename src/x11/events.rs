//! Event loop and raw X11 event diagnostics.
//!
//! This is the boundary layer: it translates `x11rb::protocol::Event` into
//! [`crate::ui::button::PointerEvent`]. Rendering is delegated to the sibling
//! adapter, and the UI layer never sees X11 types.

use super::framebuffer::{PreparedBitmap, X11BitmapAdapter};
use super::render::draw;

use crate::app::{Activation, AppState, GeometryUpdate, Redraw, RemotePointerEvent};
use crate::display::RemoteFrameCache;
use crate::net::Paperspoon;
use crate::ui::button::{PointerEvent, PointerEventKind};
use crate::ui::screen::{PhysicalPoint, ScreenLayout};
use anyhow::{Context, Result, anyhow};
use paper_protocol::V2PointerPhase;
use std::time::{Duration, Instant};
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
    let framebuffer_adapter = match X11BitmapAdapter::from_connection(conn) {
        Ok(adapter) => {
            eprintln!("framebuffer adapter ready {adapter}");
            Some(adapter)
        }
        Err(error) => {
            eprintln!("framebuffer adapter unavailable: {error:#}");
            None
        }
    };
    let mut remote_frame_cache = RemoteFrameCache::default();

    loop {
        if let Some(frame) = paperspoon.poll_frame() {
            let receive_decode_us = frame.receive_decode_us();
            match framebuffer_adapter.as_ref() {
                Some(adapter) => {
                    let prepare_started = Instant::now();
                    let prepared = adapter.prepare(frame.width(), frame.height(), frame.pixels());
                    let prepare_us = prepare_started.elapsed().as_micros();
                    match prepared {
                        Ok(bitmap) => {
                            // Checked PutImage requests and flush are included in upload time.
                            let upload_started = Instant::now();
                            let uploaded = adapter.blit(
                                conn,
                                win,
                                gc,
                                remote_viewport_origin(app.size()),
                                &bitmap,
                            );
                            let x11_upload_us = upload_started.elapsed().as_micros();
                            match uploaded {
                                Ok(chunks) => {
                                    eprintln!(
                                        "frame uploaded id={} width={} height={} wire_stride={} wire_bytes={} x11_stride={} x11_bytes={} chunks={} receive_decode_us={receive_decode_us} prepare_us={prepare_us} x11_upload_us={x11_upload_us} cache=updated",
                                        frame.frame_id(),
                                        frame.width(),
                                        frame.height(),
                                        frame.stride(),
                                        frame.pixels().len(),
                                        bitmap.stride(),
                                        bitmap.bytes().len(),
                                        chunks
                                    );
                                    remote_frame_cache.replace(
                                        frame.frame_id(),
                                        frame.width(),
                                        frame.height(),
                                        bitmap,
                                    );
                                }
                                Err(error) => eprintln!(
                                    "frame upload error id={} width={} height={} receive_decode_us={receive_decode_us} prepare_us={prepare_us} x11_upload_us={x11_upload_us}: {error:#}",
                                    frame.frame_id(),
                                    frame.width(),
                                    frame.height()
                                ),
                            }
                        }
                        Err(error) => eprintln!(
                            "frame prepare error id={} width={} height={} receive_decode_us={receive_decode_us} prepare_us={prepare_us}: {error:#}",
                            frame.frame_id(),
                            frame.width(),
                            frame.height()
                        ),
                    }
                }
                None => eprintln!(
                    "frame accepted id={} width={} height={} stride={} bytes={} receive_decode_us={receive_decode_us} render=unavailable",
                    frame.frame_id(),
                    frame.width(),
                    frame.height(),
                    frame.stride(),
                    frame.pixels().len()
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
                draw_surface(
                    conn,
                    win,
                    gc,
                    app.redraw(),
                    framebuffer_adapter.as_ref(),
                    &remote_frame_cache,
                    "Expose",
                )
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
                        if let Some(frame) = remote_frame_cache.invalidate_mismatched(viewport) {
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
                        draw_surface(
                            conn,
                            win,
                            gc,
                            redraw,
                            framebuffer_adapter.as_ref(),
                            &remote_frame_cache,
                            "ConfigureNotify",
                        )
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

fn remote_viewport_origin(physical_size: (u16, u16)) -> (i16, i16) {
    ScreenLayout::new(physical_size.0, physical_size.1)
        .map(|layout| {
            (
                layout.remote_viewport.x as i16,
                layout.remote_viewport.y as i16,
            )
        })
        .unwrap_or((0, 0))
}

fn draw_local_ui(conn: &RustConnection, win: Window, gc: Gcontext, redraw: Redraw) -> Result<()> {
    let (width, height) = redraw.size();
    draw(conn, win, gc, width, height)
}

fn draw_surface(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    redraw: Redraw,
    framebuffer_adapter: Option<&X11BitmapAdapter>,
    remote_frame_cache: &RemoteFrameCache<PreparedBitmap>,
    cause: &str,
) -> Result<()> {
    let size = redraw.size();
    draw_local_ui(conn, win, gc, redraw)?;

    let (Some(adapter), Some(frame)) = (framebuffer_adapter, remote_frame_cache.current()) else {
        return Ok(());
    };
    match adapter.blit(conn, win, gc, remote_viewport_origin(size), frame.payload()) {
        Ok(chunks) => eprintln!(
            "frame redrawn id={} width={} height={} chunks={} cause={} cache=hit",
            frame.frame_id(),
            frame.dimensions().0,
            frame.dimensions().1,
            chunks,
            cause
        ),
        Err(error) => eprintln!(
            "frame redraw error id={} width={} height={} cause={}: {error:#}",
            frame.frame_id(),
            frame.dimensions().0,
            frame.dimensions().1,
            cause
        ),
    }
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
        assert_eq!(remote_viewport_origin((1272, 1696)), (0, 0));
    }
}
