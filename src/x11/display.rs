//! X11 display, window, and graphics-context lifecycle.

use anyhow::{Context, Result};
use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, Gcontext, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

pub const WINDOW_X: i16 = 0;
pub const WINDOW_Y: i16 = 0;
/// A border would extend beyond the full-screen client area.
pub const WINDOW_BORDER_WIDTH: u16 = 0;
/// X11 wire value meaning "inherit from parent" for the window visual.
pub const VISUAL_COPY_FROM_PARENT: u32 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowMode {
    Drawable,
    InputOnly,
}

impl WindowMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Drawable => "input-output",
            Self::InputOnly => "input-only",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappedWindow {
    pub window: Window,
    pub gc: Option<Gcontext>,
    pub size: (u16, u16),
    pub mode: WindowMode,
}

/// Events that do not require a drawable X11 surface.
pub fn input_event_mask() -> EventMask {
    EventMask::STRUCTURE_NOTIFY
        | EventMask::BUTTON_PRESS
        | EventMask::BUTTON_RELEASE
        | EventMask::POINTER_MOTION
}

/// Event mask selecting the core events Paperpad consumes.
pub fn touch_event_mask() -> EventMask {
    EventMask::EXPOSURE | input_event_mask()
}
/// Connect to the X server and create, map, and publish the test window.
///
/// Returns the window, GC, and initial screen extent; the caller owns cleanup.
pub fn setup_window(
    conn: &RustConnection,
    screen_num: usize,
    mode: WindowMode,
) -> Result<MappedWindow> {
    let screen = conn
        .setup()
        .roots
        .get(screen_num)
        .context("selected X11 screen is missing")?;
    let size = (screen.width_in_pixels, screen.height_in_pixels);
    anyhow::ensure!(size.0 > 0 && size.1 > 0, "X11 screen has zero dimensions");

    let win = conn.generate_id().context("failed to generate window id")?;
    let event_mask = match mode {
        WindowMode::Drawable => touch_event_mask(),
        WindowMode::InputOnly => input_event_mask(),
    };
    let (class, attributes) = match mode {
        WindowMode::Drawable => (
            WindowClass::INPUT_OUTPUT,
            CreateWindowAux::new()
                .background_pixel(screen.white_pixel)
                .border_pixel(screen.black_pixel)
                .override_redirect(1)
                .event_mask(event_mask),
        ),
        WindowMode::InputOnly => (
            WindowClass::INPUT_ONLY,
            CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(event_mask),
        ),
    };

    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        WINDOW_X,
        WINDOW_Y,
        size.0,
        size.1,
        WINDOW_BORDER_WIDTH,
        class,
        VISUAL_COPY_FROM_PARENT,
        &attributes,
    )
    .context("failed to send create-window request")?
    .check()
    .context("X11 server rejected create-window request")?;

    let gc = match mode {
        WindowMode::Drawable => {
            let gc = conn.generate_id().context("failed to generate GC id")?;
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
            Some(gc)
        }
        WindowMode::InputOnly => None,
    };

    conn.map_window(win)
        .context("failed to send map-window request")?
        .check()
        .context("X11 server rejected map-window request")?;
    conn.flush().context("failed to flush map request")?;

    Ok(MappedWindow {
        window: win,
        gc,
        size,
        mode,
    })
}

/// Release the window and GC, destroying the window unless it is already gone.
pub fn cleanup(conn: &RustConnection, mapped: MappedWindow, destroy_window: bool) -> Result<()> {
    let mut first_error = None;

    if let Some(gc) = mapped.gc {
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
    }
    if destroy_window {
        record_cleanup_error(
            &mut first_error,
            conn.destroy_window(mapped.window)
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

    #[test]
    fn input_only_mask_excludes_exposure_but_preserves_input_and_lifecycle() {
        let mask = u32::from(input_event_mask());
        assert_eq!(mask & u32::from(EventMask::EXPOSURE), 0);
        for required in [
            EventMask::STRUCTURE_NOTIFY,
            EventMask::BUTTON_PRESS,
            EventMask::BUTTON_RELEASE,
            EventMask::POINTER_MOTION,
        ] {
            assert_eq!(mask & u32::from(required), u32::from(required));
        }
    }
}
