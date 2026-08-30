//! Kindle KUAL touch-input prototype.
//!
//! Entry point only: connects to X11, runs the event loop, and tears down.
//! Display/event handling lives in [`x11`]; logical UI concepts (geometry,
//! hit testing, contact state) live in [`ui`].

use anyhow::{Context, Result};
use std::env;
use x11rb::rust_connection::RustConnection;

use x11::display;
use x11::events::{EventLoopExit, event_loop};

mod ui;
mod x11;

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

    display::print_screen_info(&conn, screen_num);
    let (win, gc) = display::setup_window(&conn)?;

    eprintln!(
        "window mapped id=0x{win:x} x={} y={} width={} height={}",
        x11::display::WINDOW_X,
        x11::display::WINDOW_Y,
        ui::geometry::WINDOW_WIDTH,
        ui::geometry::WINDOW_HEIGHT
    );

    let event_result = event_loop(&conn, win, gc);
    let destroy_window = match &event_result {
        Ok(EventLoopExit::WindowDestroyed) => false,
        Err(_) => true,
    };
    let cleanup_result = display::cleanup(&conn, win, gc, destroy_window);

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
