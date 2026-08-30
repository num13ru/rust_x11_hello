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
const TEXT_X: u16 = 70;
const TEXT_BASELINES: [u16; 4] = [110, 160, 210, 260];
const TEXT_LINES: [&[u8]; 4] = [
    b"Rust X11 Touch Prototype",
    b"Core X11 pointer diagnostics",
    b"Press and release inside window",
    b"KUAL watchdog stops after 90s",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventLoopExit {
    WindowDestroyed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GeometryUpdate {
    IgnoredZero,
    Unchanged,
    Changed { width: u16, height: u16 },
}

#[derive(Debug)]
struct DrawLayout {
    rectangles: Vec<Rectangle>,
    text_origins: [(i16, i16); TEXT_LINES.len()],
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
                draw(conn, win, gc, width, height)
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
                        width = new_width;
                        height = new_height;
                        eprintln!(
                            "event type=ConfigureNotify x={} y={} width={width} height={height} window=0x{:x}",
                            event.x, event.y, event.window
                        );
                        draw(conn, win, gc, width, height)
                            .context("failed to redraw after ConfigureNotify")?;
                    }
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

fn scale_u16(reference_value: u16, actual_extent: u16, reference_extent: u16) -> u16 {
    debug_assert_ne!(reference_extent, 0);
    ((u32::from(reference_value) * u32::from(actual_extent)) / u32::from(reference_extent))
        .min(u32::from(u16::MAX)) as u16
}

fn scale_i16(reference_value: u16, actual_extent: u16, reference_extent: u16) -> i16 {
    scale_u16(reference_value, actual_extent, reference_extent).min(i16::MAX as u16) as i16
}

fn inset_rectangle(
    width: u16,
    height: u16,
    reference_x_inset: u16,
    reference_y_inset: u16,
) -> Option<Rectangle> {
    if width == 0 || height == 0 {
        return None;
    }

    let x = scale_u16(reference_x_inset, width, WINDOW_WIDTH).min(width.saturating_sub(1) / 2);
    let y = scale_u16(reference_y_inset, height, WINDOW_HEIGHT).min(height.saturating_sub(1) / 2);

    Some(Rectangle {
        x: x as i16,
        y: y as i16,
        width: width.saturating_sub(x.saturating_mul(2)),
        height: height.saturating_sub(y.saturating_mul(2)),
    })
}

fn draw_layout(width: u16, height: u16) -> Option<DrawLayout> {
    if width == 0 || height == 0 {
        return None;
    }

    let rectangles = [(20, 20), (40, 40)]
        .into_iter()
        .filter_map(|(x, y)| inset_rectangle(width, height, x, y))
        .collect();
    let text_x = scale_i16(TEXT_X, width, WINDOW_WIDTH);
    let text_origins =
        TEXT_BASELINES.map(|baseline| (text_x, scale_i16(baseline, height, WINDOW_HEIGHT)));

    Some(DrawLayout {
        rectangles,
        text_origins,
    })
}

fn draw(conn: &RustConnection, win: Window, gc: Gcontext, width: u16, height: u16) -> Result<()> {
    let Some(layout) = draw_layout(width, height) else {
        return Ok(());
    };

    conn.clear_area(false, win, 0, 0, width, height)
        .context("failed to send clear-area request")?
        .check()
        .context("X11 server rejected clear-area request")?;

    if !layout.rectangles.is_empty() {
        conn.poly_rectangle(win, gc, &layout.rectangles)
            .context("failed to send rectangle draw request")?
            .check()
            .context("X11 server rejected rectangle draw request")?;
    }

    for (text, (x, y)) in TEXT_LINES.into_iter().zip(layout.text_origins) {
        if y > 0 {
            draw_text(conn, win, gc, x, y, text)?;
        }
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
            geometry_update((WINDOW_WIDTH, WINDOW_HEIGHT), (380, 180)),
            GeometryUpdate::Changed {
                width: 380,
                height: 180,
            }
        );
    }

    #[test]
    fn default_layout_preserves_the_verified_kindle_appearance() {
        let layout = draw_layout(WINDOW_WIDTH, WINDOW_HEIGHT).expect("nonzero geometry");

        assert_eq!(layout.rectangles.len(), 2);
        assert_rectangle(&layout.rectangles[0], 20, 20, 720, 320);
        assert_rectangle(&layout.rectangles[1], 40, 40, 680, 280);
        assert_eq!(
            layout.text_origins,
            [(70, 110), (70, 160), (70, 210), (70, 260)]
        );
    }

    #[test]
    fn half_size_layout_scales_every_draw_coordinate() {
        let layout = draw_layout(380, 180).expect("nonzero geometry");

        assert_rectangle(&layout.rectangles[0], 10, 10, 360, 160);
        assert_rectangle(&layout.rectangles[1], 20, 20, 340, 140);
        assert_eq!(
            layout.text_origins,
            [(35, 55), (35, 80), (35, 105), (35, 130)]
        );
    }

    #[test]
    fn zero_tiny_and_maximum_geometry_are_safe() {
        assert!(draw_layout(0, WINDOW_HEIGHT).is_none());
        assert!(draw_layout(WINDOW_WIDTH, 0).is_none());

        let tiny = draw_layout(1, 1).expect("one-pixel geometry");
        assert_eq!(tiny.rectangles.len(), 2);
        for rectangle in &tiny.rectangles {
            assert_rectangle(rectangle, 0, 0, 1, 1);
        }
        assert_eq!(tiny.text_origins, [(0, 0); TEXT_LINES.len()]);

        let maximum = draw_layout(u16::MAX, u16::MAX).expect("maximum geometry");
        assert_rectangles_within(&maximum.rectangles, u16::MAX, u16::MAX);
        for (x, y) in maximum.text_origins {
            assert!(x >= 0);
            assert!(y >= 0);
        }
    }

    fn assert_rectangle(rectangle: &Rectangle, x: i16, y: i16, width: u16, height: u16) {
        assert_eq!(rectangle.x, x);
        assert_eq!(rectangle.y, y);
        assert_eq!(rectangle.width, width);
        assert_eq!(rectangle.height, height);
    }

    fn assert_rectangles_within(rectangles: &[Rectangle], width: u16, height: u16) {
        for rectangle in rectangles {
            assert!(rectangle.x >= 0);
            assert!(rectangle.y >= 0);
            assert!(rectangle.width > 0);
            assert!(rectangle.height > 0);
            assert!(
                u32::from(rectangle.x as u16) * 2 + u32::from(rectangle.width) <= u32::from(width)
            );
            assert!(
                u32::from(rectangle.y as u16) * 2 + u32::from(rectangle.height)
                    <= u32::from(height)
            );
        }
    }
}
