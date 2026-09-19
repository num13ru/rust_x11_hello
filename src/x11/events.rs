//! Event loop and raw X11 event diagnostics.
//!
//! This is the boundary layer: it translates `x11rb::protocol::Event` into
//! [`crate::ui::button::PointerEvent`]. Rendering is delegated to the sibling
//! adapter, and the UI layer never sees X11 types.

use super::framebuffer::{PreparedBitmap, X11BitmapAdapter};
use super::render::draw;

use crate::app::{Activation, AppState, GeometryUpdate, Redraw};
use crate::net::Paperspoon;
use crate::ui::button::{PointerEvent, PointerEventKind};
use crate::ui::screen::{PhysicalPoint, ScreenLayout};
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

struct CachedRemoteFrame<T> {
    frame_id: u64,
    width: u16,
    height: u16,
    bitmap: T,
}

struct RemoteFrameCache<T> {
    current: Option<CachedRemoteFrame<T>>,
}

impl<T> Default for RemoteFrameCache<T> {
    fn default() -> Self {
        Self { current: None }
    }
}

impl<T> RemoteFrameCache<T> {
    fn current(&self) -> Option<&CachedRemoteFrame<T>> {
        self.current.as_ref()
    }

    fn replace(&mut self, frame_id: u64, width: u16, height: u16, bitmap: T) {
        self.current = Some(CachedRemoteFrame {
            frame_id,
            width,
            height,
            bitmap,
        });
    }

    fn invalidate_mismatched(&mut self, viewport: (u16, u16)) -> Option<CachedRemoteFrame<T>> {
        let mismatched = self
            .current
            .as_ref()
            .is_some_and(|frame| (frame.width, frame.height) != viewport);
        mismatched.then(|| self.current.take().expect("mismatched frame exists"))
    }
}

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
    paperspoon.set_remote_viewport(remote_viewport_size(initial_size));

    if let Some(text) = paperspoon.poll_display() {
        draw_surface(
            conn,
            win,
            gc,
            app.set_status_text(text),
            framebuffer_adapter.as_ref(),
            &remote_frame_cache,
            "display",
        )
        .context("failed to redraw status after PaperSpoon command")?;
    }
    loop {
        if let Some(frame) = paperspoon.poll_frame() {
            match framebuffer_adapter.as_ref() {
                Some(adapter) => {
                    match adapter.prepare(frame.width(), frame.height(), frame.pixels()) {
                        Ok(bitmap) => match adapter.blit(
                            conn,
                            win,
                            gc,
                            remote_viewport_origin(app.size()),
                            &bitmap,
                        ) {
                            Ok(chunks) => {
                                eprintln!(
                                    "frame uploaded id={} width={} height={} wire_stride={} wire_bytes={} x11_stride={} x11_bytes={} chunks={} cache=updated",
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
                                "frame upload error id={} width={} height={}: {error:#}",
                                frame.frame_id(),
                                frame.width(),
                                frame.height()
                            ),
                        },
                        Err(error) => eprintln!(
                            "frame prepare error id={} width={} height={}: {error:#}",
                            frame.frame_id(),
                            frame.width(),
                            frame.height()
                        ),
                    }
                }
                None => eprintln!(
                    "frame accepted id={} width={} height={} stride={} bytes={} render=unavailable",
                    frame.frame_id(),
                    frame.width(),
                    frame.height(),
                    frame.stride(),
                    frame.pixels().len()
                ),
            }
        }
        // Drain any PaperSpoon command received since the last X11 event.
        if let Some(text) = paperspoon.poll_display() {
            eprintln!("display: {text}");
            draw_surface(
                conn,
                win,
                gc,
                app.set_status_text(text),
                framebuffer_adapter.as_ref(),
                &remote_frame_cache,
                "display",
            )
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
                        paperspoon.set_remote_viewport(viewport);
                        if let Some(frame) = remote_frame_cache.invalidate_mismatched(viewport) {
                            eprintln!(
                                "frame cache invalidated id={} width={} height={} viewport_width={} viewport_height={}",
                                frame.frame_id, frame.width, frame.height, viewport.0, viewport.1
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
                let Some(activation) = app.handle_pointer(PointerEvent {
                    kind: PointerEventKind::Press,
                    detail: event.detail,
                    point: PhysicalPoint {
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
                    point: PhysicalPoint {
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

fn remote_viewport_size(physical_size: (u16, u16)) -> (u16, u16) {
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

fn draw_app(conn: &RustConnection, win: Window, gc: Gcontext, redraw: Redraw<'_>) -> Result<()> {
    let (width, height) = redraw.size();
    draw(conn, win, gc, width, height, redraw.status_text())
}

fn draw_surface(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    redraw: Redraw<'_>,
    framebuffer_adapter: Option<&X11BitmapAdapter>,
    remote_frame_cache: &RemoteFrameCache<PreparedBitmap>,
    cause: &str,
) -> Result<()> {
    let size = redraw.size();
    draw_app(conn, win, gc, redraw)?;

    let (Some(adapter), Some(frame)) = (framebuffer_adapter, remote_frame_cache.current()) else {
        return Ok(());
    };
    match adapter.blit(conn, win, gc, remote_viewport_origin(size), &frame.bitmap) {
        Ok(chunks) => eprintln!(
            "frame redrawn id={} width={} height={} chunks={} cause={} cache=hit",
            frame.frame_id, frame.width, frame.height, chunks, cause
        ),
        Err(error) => eprintln!(
            "frame redraw error id={} width={} height={} cause={}: {error:#}",
            frame.frame_id, frame.width, frame.height, cause
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
        Activation::Exit => {
            eprintln!("ui action=activate system=exit");
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

    #[test]
    fn remote_frame_cache_replaces_and_invalidates_only_for_new_viewport() {
        let mut cache = RemoteFrameCache::default();
        assert!(cache.current().is_none());

        cache.replace(1, 9, 2, "first");
        cache.replace(2, 17, 4, "second");
        let current = cache.current().expect("replacement cached");
        assert_eq!(current.frame_id, 2);
        assert_eq!((current.width, current.height), (17, 4));
        assert_eq!(current.bitmap, "second");

        assert!(cache.invalidate_mismatched((17, 4)).is_none());
        assert_eq!(
            cache.current().expect("matching cache retained").frame_id,
            2
        );

        let invalidated = cache
            .invalidate_mismatched((17, 5))
            .expect("mismatched cache invalidated");
        assert_eq!(invalidated.frame_id, 2);
        assert!(cache.current().is_none());
    }

    #[test]
    fn remote_viewport_size_excludes_local_system_ui() {
        assert_eq!(remote_viewport_size((1272, 1696)), (1272, 1624));
        assert_eq!(remote_viewport_size((100, 40)), (100, 0));
        assert_eq!(remote_viewport_size((0, 40)), (0, 0));
        assert_eq!(remote_viewport_origin((1272, 1696)), (0, 0));
    }
}
