use anyhow::{Context, Result, anyhow};
use std::env;
use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    ButtonPressEvent, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, Gcontext, Rectangle,
    Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

const WINDOW_X: i16 = 80;
const WINDOW_Y: i16 = 120;
const WINDOW_WIDTH: u16 = 760;
const WINDOW_HEIGHT: u16 = 360;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventLoopExit {
    WindowDestroyed,
}

fn main() -> Result<()> {
    print_environment();
    run()
}

fn print_environment() {
    eprintln!("Rust X11 touch prototype for Kindle");
    eprintln!("target_arch: {}", env::consts::ARCH);
    eprintln!("target_os: {}", env::consts::OS);
    eprintln!("DISPLAY={:?}", env::var("DISPLAY").ok());
    eprintln!("XAUTHORITY={:?}", env::var("XAUTHORITY").ok());
}

fn run() -> Result<()> {
    let (conn, screen_num) = RustConnection::connect(None)
        .context("failed to connect to X11 display; check DISPLAY and /tmp/.X11-unix/X0")?;
    let screen = &conn.setup().roots[screen_num];

    eprintln!("connected to X11");
    eprintln!("screen_num: {screen_num}");
    eprintln!("root: 0x{:x}", screen.root);
    eprintln!("width: {}", screen.width_in_pixels);
    eprintln!("height: {}", screen.height_in_pixels);
    eprintln!("root_depth: {}", screen.root_depth);
    eprintln!("black_pixel: {}", screen.black_pixel);
    eprintln!("white_pixel: {}", screen.white_pixel);

    let win = conn.generate_id().context("failed to generate window id")?;
    let gc = conn.generate_id().context("failed to generate GC id")?;
    let event_mask = touch_event_mask();

    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        WINDOW_X,
        WINDOW_Y,
        WINDOW_WIDTH,
        WINDOW_HEIGHT,
        2,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .background_pixel(screen.white_pixel)
            .border_pixel(screen.black_pixel)
            .override_redirect(1)
            .event_mask(event_mask),
    )
    .context("failed to send create-window request")?
    .check()
    .context("X11 server rejected create-window request")?;

    conn.create_gc(
        gc,
        win,
        &CreateGCAux::new()
            .foreground(screen.black_pixel)
            .background(screen.white_pixel),
    )
    .context("failed to send create-GC request")?
    .check()
    .context("X11 server rejected create-GC request")?;

    conn.map_window(win)
        .context("failed to send map-window request")?
        .check()
        .context("X11 server rejected map-window request")?;
    conn.flush().context("failed to flush map request")?;

    eprintln!(
        "window mapped id=0x{win:x} x={WINDOW_X} y={WINDOW_Y} width={WINDOW_WIDTH} height={WINDOW_HEIGHT}"
    );

    let event_result = event_loop(&conn, win, gc);
    let destroy_window = match &event_result {
        Ok(EventLoopExit::WindowDestroyed) => false,
        Err(_) => true,
    };
    let cleanup_result = cleanup(&conn, win, gc, destroy_window);

    match (event_result, cleanup_result) {
        (Err(primary), Err(cleanup_error)) => {
            eprintln!("cleanup after event-loop failure also failed: {cleanup_error:#}");
            Err(primary)
        }
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(cleanup_error)) => Err(cleanup_error),
        (Ok(_), Ok(())) => Ok(()),
    }
}

fn touch_event_mask() -> EventMask {
    EventMask::EXPOSURE
        | EventMask::STRUCTURE_NOTIFY
        | EventMask::BUTTON_PRESS
        | EventMask::BUTTON_RELEASE
        | EventMask::POINTER_MOTION
}

fn event_loop(conn: &RustConnection, win: Window, gc: Gcontext) -> Result<EventLoopExit> {
    let mut width = WINDOW_WIDTH;
    let mut height = WINDOW_HEIGHT;

    loop {
        let event = conn
            .wait_for_event()
            .context("X11 connection failed while waiting for an event")?;

        match event {
            Event::Expose(event) if event.window == win && event.count == 0 => {
                draw(conn, win, gc).context("failed to redraw final Expose batch")?;
            }
            Event::Expose(_) => {}
            Event::ConfigureNotify(event) if event.window == win => {
                if event.width == 0 || event.height == 0 {
                    eprintln!(
                        "event type=ConfigureNotify ignored=zero_geometry width={} height={}",
                        event.width, event.height
                    );
                } else if event.width != width || event.height != height {
                    width = event.width;
                    height = event.height;
                    eprintln!(
                        "event type=ConfigureNotify x={} y={} width={width} height={height} window=0x{:x}",
                        event.x, event.y, event.window
                    );
                }
            }
            Event::ConfigureNotify(_) => {}
            Event::ButtonPress(event) if event.event == win => {
                eprintln!("{}", format_pointer_event("ButtonPress", &event));
            }
            Event::ButtonPress(_) => {}
            Event::ButtonRelease(event) if event.event == win => {
                eprintln!("{}", format_pointer_event("ButtonRelease", &event));
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

fn format_pointer_event(event_type: &str, event: &ButtonPressEvent) -> String {
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

fn cleanup(conn: &RustConnection, win: Window, gc: Gcontext, destroy_window: bool) -> Result<()> {
    let mut first_error = None;

    record_cleanup_error(
        &mut first_error,
        conn.free_gc(gc)
            .context("failed to send free-GC request")
            .and_then(|cookie| {
                cookie
                    .check()
                    .context("X11 server rejected free-GC request")
            }),
    );
    if destroy_window {
        record_cleanup_error(
            &mut first_error,
            conn.destroy_window(win)
                .context("failed to send destroy-window request")
                .and_then(|cookie| {
                    cookie
                        .check()
                        .context("X11 server rejected destroy-window request")
                }),
        );
    }
    record_cleanup_error(
        &mut first_error,
        conn.flush().context("failed to flush X11 cleanup requests"),
    );

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn record_cleanup_error(first_error: &mut Option<anyhow::Error>, result: Result<()>) {
    if let Err(error) = result {
        if first_error.is_none() {
            *first_error = Some(error);
        } else {
            eprintln!("additional cleanup error: {error:#}");
        }
    }
}

fn draw(conn: &RustConnection, win: Window, gc: Gcontext) -> Result<()> {
    conn.poly_rectangle(
        win,
        gc,
        &[
            Rectangle {
                x: 20,
                y: 20,
                width: 720,
                height: 320,
            },
            Rectangle {
                x: 40,
                y: 40,
                width: 680,
                height: 280,
            },
        ],
    )
    .context("failed to send rectangle draw request")?
    .check()
    .context("X11 server rejected rectangle draw request")?;

    draw_text(conn, win, gc, 70, 110, b"Rust X11 Touch Prototype")?;
    draw_text(conn, win, gc, 70, 160, b"Core X11 pointer diagnostics")?;
    draw_text(conn, win, gc, 70, 210, b"Press and release inside window")?;
    draw_text(conn, win, gc, 70, 260, b"KUAL watchdog stops after 90s")?;

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
    fn touch_mask_contains_all_required_core_events() {
        let mask = u32::from(touch_event_mask());

        for required in [
            EventMask::EXPOSURE,
            EventMask::STRUCTURE_NOTIFY,
            EventMask::BUTTON_PRESS,
            EventMask::BUTTON_RELEASE,
            EventMask::POINTER_MOTION,
        ] {
            assert_eq!(mask & u32::from(required), u32::from(required));
        }
    }

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
