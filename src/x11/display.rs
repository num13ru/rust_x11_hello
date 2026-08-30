//! X11 display, window, and graphics-context lifecycle.

use anyhow::{Context, Result};
use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, Gcontext, Window, WindowClass,
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
/// Returns the connection, window, and GC; the caller owns their lifecycle.
pub fn setup_window(conn: &RustConnection) -> Result<(Window, Gcontext)> {
    let screen = &conn.setup().roots[0];

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

    Ok((win, gc))
}

/// Release the window and GC, destroying the window unless it is already gone.
pub fn cleanup(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    destroy_window: bool,
) -> Result<()> {
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
