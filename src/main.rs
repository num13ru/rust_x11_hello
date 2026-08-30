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
const PRIMARY_BUTTON_DETAIL: u8 = 1;
const GRID_COLUMNS: u16 = 3;
const GRID_ROWS: u16 = 2;
const GRID_LEFT_INSET: u16 = 20;
const GRID_RIGHT_INSET: u16 = 20;
const GRID_TOP_INSET: u16 = 60;
const GRID_BOTTOM_INSET: u16 = 20;
const TITLE_X: u16 = 20;
const TITLE_BASELINE: u16 = 40;
const TITLE_TEXT: &[u8] = b"Core X11 button grid: tap 1-6";
const BUTTON_LABELS: [&[u8]; 6] = [b"1", b"2", b"3", b"4", b"5", b"6"];

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Point {
    x: i16,
    y: i16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LogicalRect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

impl LogicalRect {
    fn contains(self, point: Point) -> bool {
        let point_x = i32::from(point.x);
        let point_y = i32::from(point.y);
        let left = i32::from(self.x);
        let top = i32::from(self.y);

        point_x >= left
            && point_y >= top
            && point_x < left + i32::from(self.width)
            && point_y < top + i32::from(self.height)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LogicalButton {
    id: u8,
    bounds: LogicalRect,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ContactState {
    #[default]
    Idle,
    Armed(u8),
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ContactTracker {
    state: ContactState,
}

#[derive(Clone, Copy, Debug)]
struct TextPlacement {
    x: i16,
    y: i16,
    text: &'static [u8],
}

#[derive(Debug)]
struct DrawLayout {
    rectangles: Vec<Rectangle>,
    text: Vec<TextPlacement>,
}

impl ContactTracker {
    fn press(&mut self, detail: u8, point: Point, buttons: &[LogicalButton]) {
        if detail != PRIMARY_BUTTON_DETAIL {
            return;
        }

        self.state = match self.state {
            ContactState::Idle => hit_button(buttons, point)
                .map(|button| ContactState::Armed(button.id))
                .unwrap_or(ContactState::Cancelled),
            ContactState::Armed(_) | ContactState::Cancelled => ContactState::Cancelled,
        };
    }

    fn release(&mut self, detail: u8, point: Point, buttons: &[LogicalButton]) -> Option<u8> {
        if detail != PRIMARY_BUTTON_DETAIL {
            return None;
        }

        match std::mem::take(&mut self.state) {
            ContactState::Armed(armed_id)
                if hit_button(buttons, point).map(|button| button.id) == Some(armed_id) =>
            {
                Some(armed_id)
            }
            ContactState::Idle | ContactState::Armed(_) | ContactState::Cancelled => None,
        }
    }

    fn cancel(&mut self) {
        self.state = ContactState::Idle;
    }
}

fn hit_button(buttons: &[LogicalButton], point: Point) -> Option<&LogicalButton> {
    buttons.iter().find(|button| button.bounds.contains(point))
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
    let mut contact = ContactTracker::default();

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
                        contact.cancel();
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
                let buttons = button_grid(width, height);
                contact.press(
                    event.detail,
                    Point {
                        x: event.event_x,
                        y: event.event_y,
                    },
                    &buttons,
                );
            }
            Event::ButtonPress(_) => {}
            Event::ButtonRelease(event) if event.event == win => {
                eprintln!("{}", format_pointer_event("ButtonRelease", &event));
                let buttons = button_grid(width, height);
                if let Some(button_id) = contact.release(
                    event.detail,
                    Point {
                        x: event.event_x,
                        y: event.event_y,
                    },
                    &buttons,
                ) {
                    eprintln!("ui action=activate button={button_id}");
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

fn bounded_insets(
    actual_extent: u16,
    minimum_content: u16,
    reference_leading: u16,
    reference_trailing: u16,
    reference_extent: u16,
) -> (u16, u16) {
    let margin_budget = actual_extent.saturating_sub(minimum_content);
    let leading = scale_u16(reference_leading, actual_extent, reference_extent).min(margin_budget);
    let trailing = scale_u16(reference_trailing, actual_extent, reference_extent)
        .min(margin_budget.saturating_sub(leading));
    (leading, trailing)
}

fn partition_offset(extent: u16, index: u16, parts: u16) -> u16 {
    ((u32::from(extent) * u32::from(index)) / u32::from(parts)) as u16
}

fn button_grid(width: u16, height: u16) -> Vec<LogicalButton> {
    if width < GRID_COLUMNS || height < GRID_ROWS {
        return Vec::new();
    }

    let (left, right) = bounded_insets(
        width,
        GRID_COLUMNS,
        GRID_LEFT_INSET,
        GRID_RIGHT_INSET,
        WINDOW_WIDTH,
    );
    let (top, bottom) = bounded_insets(
        height,
        GRID_ROWS,
        GRID_TOP_INSET,
        GRID_BOTTOM_INSET,
        WINDOW_HEIGHT,
    );
    let grid_width = width - left - right;
    let grid_height = height - top - bottom;
    let mut buttons = Vec::with_capacity(usize::from(GRID_COLUMNS * GRID_ROWS));

    for row in 0..GRID_ROWS {
        let row_start = partition_offset(grid_height, row, GRID_ROWS);
        let row_end = partition_offset(grid_height, row + 1, GRID_ROWS);
        for column in 0..GRID_COLUMNS {
            let column_start = partition_offset(grid_width, column, GRID_COLUMNS);
            let column_end = partition_offset(grid_width, column + 1, GRID_COLUMNS);
            buttons.push(LogicalButton {
                id: (row * GRID_COLUMNS + column + 1) as u8,
                bounds: LogicalRect {
                    x: left + column_start,
                    y: top + row_start,
                    width: column_end - column_start,
                    height: row_end - row_start,
                },
            });
        }
    }

    buttons
}

fn logical_coordinate(value: u32) -> i16 {
    value.min(i16::MAX as u32) as i16
}

fn draw_layout(width: u16, height: u16) -> Option<DrawLayout> {
    if width == 0 || height == 0 {
        return None;
    }

    let buttons = button_grid(width, height);
    let rectangles = buttons
        .iter()
        .map(|button| Rectangle {
            x: logical_coordinate(u32::from(button.bounds.x)),
            y: logical_coordinate(u32::from(button.bounds.y)),
            width: button.bounds.width,
            height: button.bounds.height,
        })
        .collect();
    let mut text = Vec::with_capacity(BUTTON_LABELS.len() + 1);
    text.push(TextPlacement {
        x: scale_i16(TITLE_X, width, WINDOW_WIDTH),
        y: scale_i16(TITLE_BASELINE, height, WINDOW_HEIGHT),
        text: TITLE_TEXT,
    });
    let baseline_offset = u32::from(scale_u16(5, height, WINDOW_HEIGHT).max(1));
    for (button, label) in buttons.iter().zip(BUTTON_LABELS) {
        let center_x = u32::from(button.bounds.x) + u32::from(button.bounds.width) / 2;
        let center_y = u32::from(button.bounds.y) + u32::from(button.bounds.height) / 2;
        text.push(TextPlacement {
            x: logical_coordinate(center_x.saturating_sub(3)),
            y: logical_coordinate(center_y + baseline_offset),
            text: label,
        });
    }

    Some(DrawLayout { rectangles, text })
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

    for placement in layout.text {
        if placement.y > 0 {
            draw_text(conn, win, gc, placement.x, placement.y, placement.text)?;
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
    fn default_layout_draws_the_six_button_grid() {
        let layout = draw_layout(WINDOW_WIDTH, WINDOW_HEIGHT).expect("nonzero geometry");

        assert_eq!(layout.rectangles.len(), 6);
        assert_rectangle(&layout.rectangles[0], 20, 60, 240, 140);
        assert_rectangle(&layout.rectangles[1], 260, 60, 240, 140);
        assert_rectangle(&layout.rectangles[2], 500, 60, 240, 140);
        assert_rectangle(&layout.rectangles[3], 20, 200, 240, 140);
        assert_rectangle(&layout.rectangles[4], 260, 200, 240, 140);
        assert_rectangle(&layout.rectangles[5], 500, 200, 240, 140);
        assert_eq!(layout.text.len(), 7);
        assert_text(&layout.text[0], 20, 40, TITLE_TEXT);
        assert_text(&layout.text[1], 137, 135, b"1");
        assert_text(&layout.text[6], 617, 275, b"6");
    }

    #[test]
    fn half_size_layout_scales_the_grid_and_labels() {
        let layout = draw_layout(380, 180).expect("nonzero geometry");

        assert_eq!(layout.rectangles.len(), 6);
        assert_rectangle(&layout.rectangles[0], 10, 30, 120, 70);
        assert_rectangle(&layout.rectangles[1], 130, 30, 120, 70);
        assert_rectangle(&layout.rectangles[5], 250, 100, 120, 70);
        assert_text(&layout.text[0], 10, 20, TITLE_TEXT);
        assert_text(&layout.text[1], 67, 67, b"1");
        assert_text(&layout.text[6], 307, 137, b"6");
    }

    #[test]
    fn zero_tiny_and_maximum_geometry_are_safe() {
        assert!(draw_layout(0, WINDOW_HEIGHT).is_none());
        assert!(draw_layout(WINDOW_WIDTH, 0).is_none());

        let tiny = draw_layout(1, 1).expect("one-pixel geometry");
        assert!(tiny.rectangles.is_empty());
        assert_eq!(tiny.text.len(), 1);
        assert_text(&tiny.text[0], 0, 0, TITLE_TEXT);
        assert!(button_grid(1, 1).is_empty());

        let minimum = button_grid(GRID_COLUMNS, GRID_ROWS);
        assert_eq!(minimum.len(), 6);
        for button in &minimum {
            assert_eq!(button.bounds.width, 1);
            assert_eq!(button.bounds.height, 1);
        }

        let maximum = draw_layout(u16::MAX, u16::MAX).expect("maximum geometry");
        assert_eq!(maximum.rectangles.len(), 6);
        assert_rectangles_within(&maximum.rectangles, u16::MAX, u16::MAX);
        for placement in maximum.text {
            assert!(placement.x >= 0);
            assert!(placement.y >= 0);
        }
    }

    #[test]
    fn grid_hit_testing_uses_half_open_edges() {
        let buttons = button_grid(WINDOW_WIDTH, WINDOW_HEIGHT);

        assert_eq!(hit_button(&buttons, Point { x: 20, y: 60 }).unwrap().id, 1);
        assert_eq!(
            hit_button(&buttons, Point { x: 259, y: 199 }).unwrap().id,
            1
        );
        assert_eq!(hit_button(&buttons, Point { x: 260, y: 60 }).unwrap().id, 2);
        assert_eq!(
            hit_button(&buttons, Point { x: 739, y: 339 }).unwrap().id,
            6
        );
        assert!(hit_button(&buttons, Point { x: 19, y: 60 }).is_none());
        assert!(hit_button(&buttons, Point { x: 740, y: 339 }).is_none());
        assert!(hit_button(&buttons, Point { x: 739, y: 340 }).is_none());
        assert!(hit_button(&buttons, Point { x: -1, y: 100 }).is_none());
    }

    #[test]
    fn primary_press_and_same_button_release_activate_once() {
        let buttons = button_grid(WINDOW_WIDTH, WINDOW_HEIGHT);
        let mut contact = ContactTracker::default();
        let inside_button_4 = Point { x: 140, y: 270 };

        contact.press(PRIMARY_BUTTON_DETAIL, inside_button_4, &buttons);
        assert_eq!(contact.state, ContactState::Armed(4));

        contact.press(9, Point { x: 150, y: 275 }, &buttons);
        assert_eq!(contact.state, ContactState::Armed(4));
        assert_eq!(contact.release(9, inside_button_4, &buttons), None);
        assert_eq!(contact.state, ContactState::Armed(4));

        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, inside_button_4, &buttons),
            Some(4)
        );
        assert_eq!(contact.state, ContactState::Idle);
        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, inside_button_4, &buttons),
            None
        );
    }

    #[test]
    fn primary_contact_cancels_on_mismatch_repeat_outside_or_geometry_change() {
        let buttons = button_grid(WINDOW_WIDTH, WINDOW_HEIGHT);
        let button_1 = Point { x: 140, y: 130 };
        let button_2 = Point { x: 380, y: 130 };
        let outside = Point { x: 5, y: 5 };
        let mut contact = ContactTracker::default();

        contact.press(PRIMARY_BUTTON_DETAIL, button_1, &buttons);
        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, button_2, &buttons),
            None
        );

        contact.press(PRIMARY_BUTTON_DETAIL, outside, &buttons);
        assert_eq!(contact.state, ContactState::Cancelled);
        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, button_1, &buttons),
            None
        );

        contact.press(PRIMARY_BUTTON_DETAIL, button_1, &buttons);
        contact.press(PRIMARY_BUTTON_DETAIL, button_1, &buttons);
        assert_eq!(contact.state, ContactState::Cancelled);
        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, button_1, &buttons),
            None
        );

        contact.press(PRIMARY_BUTTON_DETAIL, button_1, &buttons);
        contact.cancel();
        assert_eq!(contact.state, ContactState::Idle);
        assert_eq!(
            contact.release(PRIMARY_BUTTON_DETAIL, button_1, &buttons),
            None
        );
    }

    fn assert_text(placement: &TextPlacement, x: i16, y: i16, text: &[u8]) {
        assert_eq!(placement.x, x);
        assert_eq!(placement.y, y);
        assert_eq!(placement.text, text);
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
            assert!(u32::from(rectangle.x as u16) + u32::from(rectangle.width) <= u32::from(width));
            assert!(
                u32::from(rectangle.y as u16) + u32::from(rectangle.height) <= u32::from(height)
            );
        }
    }
}
