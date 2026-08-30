//! X11 display, window, and graphics-context lifecycle.

use anyhow::{Context, Result};
use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, Font, Gcontext, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::ui::geometry::{WINDOW_HEIGHT, WINDOW_WIDTH};

pub const WINDOW_X: i16 = 80;
pub const WINDOW_Y: i16 = 120;

/// Event mask selecting the core events the touch prototype consumes.
pub fn touch_event_mask() -> EventMask {
    EventMask::EXPOSURE
        | EventMask::STRUCTURE_NOTIFY
        | EventMask::BUTTON_PRESS
        | EventMask::BUTTON_RELEASE
        | EventMask::POINTER_MOTION
}
/// Connect to the X server and create, map, and publish the test window.
///
/// Opens the largest available fixed font, sets it on the GC, and returns
/// `(window, gc, font)`; `font` is `None` when the server exposes no
/// candidate font, in which case the GC uses the server default.
pub fn setup_window(conn: &RustConnection) -> Result<(Window, Gcontext, Option<Font>)> {
    let screen = &conn.setup().roots[0];

    let font = open_large_font(conn)?;

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

    let mut gc_aux = CreateGCAux::new()
        .foreground(screen.black_pixel)
        .background(screen.white_pixel);
    if let Some(font_id) = font {
        gc_aux = gc_aux.font(font_id);
    }
    conn.create_gc(gc, win, &gc_aux)
        .context("failed to send create-GC request")?
        .check()
        .context("X11 server rejected create-GC request")?;

    conn.map_window(win)
        .context("failed to send map-window request")?
        .check()
        .context("X11 server rejected map-window request")?;
    conn.flush().context("failed to flush map request")?;

    Ok((win, gc, font))
}

/// Largest fixed fonts most X servers (including the Kindle's) expose, in
/// preference order. Returns the first that opens, or `None` if none do.
#[allow(clippy::collapsible_if)]
fn open_large_font(conn: &RustConnection) -> Result<Option<Font>> {
    const CANDIDATES: &[&str] = &[
        "-misc-fixed-medium-r-normal--20-200-75-75-c-100-iso10646-1",
        "-misc-fixed-medium-r-normal--18-180-75-75-c-90-iso10646-1",
        "-misc-fixed-medium-r-normal--14-130-75-75-c-70-iso10646-1",
    ];
    for name in CANDIDATES {
        if let Ok(font_id) = conn.generate_id() {
            if conn.open_font(font_id, name.as_bytes()).is_ok() {
                return Ok(Some(font_id));
            }
        }
    }
    Ok(None)
}

/// Release the window and GC, destroying the window unless it is already gone.
pub fn cleanup(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    font: Option<Font>,
    destroy_window: bool,
) -> Result<()> {
    let mut first_error = None;

    if let Some(font_id) = font {
        record_cleanup_error(
            &mut first_error,
            conn.close_font(font_id)
                .context("failed to send close-font request")
                .and_then(|cookie| {
                    cookie
                        .check()
                        .context("X11 server rejected close-font request")
                }),
        );
    }
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

/// Print the X11 screen facts needed for offline evidence interpretation.
pub fn print_screen_info(conn: &RustConnection, screen_num: usize) {
    let screen = &conn.setup().roots[screen_num];

    eprintln!("connected to X11");
    eprintln!("screen_num: {screen_num}");
    eprintln!("root: 0x{:x}", screen.root);
    eprintln!("width: {}", screen.width_in_pixels);
    eprintln!("height: {}", screen.height_in_pixels);
    eprintln!("root_depth: {}", screen.root_depth);
    eprintln!("black_pixel: {}", screen.black_pixel);
    eprintln!("white_pixel: {}", screen.white_pixel);
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
