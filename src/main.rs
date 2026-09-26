//! Paperpad Kindle/KUAL process entry point.
//!
//! Entry point only: chooses a display backend, keeps X11 for input and window
//! lifecycle, runs the event loop, and tears down. Logical UI concepts live
//! in [`ui`].

use crate::display::DisplayBackend;
use anyhow::{Context, Result, bail};
#[cfg(target_os = "linux")]
use mxcfb::MxcfbDisplayBackend;
use std::env;
use x11rb::protocol::xproto::{Gcontext, Window};
use x11rb::rust_connection::RustConnection;

use x11::X11DisplayBackend;
use x11::display as x11_display;
use x11::events::{EventLoopExit, event_loop, remote_viewport_size};

mod app;
mod config;
mod discovery;
mod display;
mod mxcfb;
mod net;
mod ui;
mod x11;

fn main() -> Result<()> {
    print_environment();
    run()
}

fn print_environment() {
    eprintln!("PaperPad Kindle/KUAL runtime");
    eprintln!("target_arch: {}", env::consts::ARCH);
    eprintln!("target_os: {}", env::consts::OS);
    eprintln!("DISPLAY={:?}", env::var("DISPLAY").ok());
    eprintln!("XAUTHORITY={:?}", env::var("XAUTHORITY").ok());
}

fn run() -> Result<()> {
    let mut args = env::args_os();
    let _program = args.next();
    match (args.next(), args.next()) {
        (None, None) => {}
        (Some(arg), None) if arg == "--inspect-framebuffer" => {
            return mxcfb::inspect_framebuffer();
        }
        _ => bail!("usage: rust_x11_hello [--inspect-framebuffer]"),
    }

    let display_kind = config::DisplayBackendKind::from_env()?;
    #[cfg(not(target_os = "linux"))]
    if display_kind == config::DisplayBackendKind::Mxcfb {
        bail!("display backend=mxcfb requires Linux and /dev/fb0");
    }
    let paperpad_config = config::PaperpadConfig::from_env()?;

    let (conn, screen_num) = RustConnection::connect(None)
        .context("failed to connect to X11 display; check DISPLAY and /tmp/.X11-unix/X0")?;

    x11_display::print_screen_info(&conn, screen_num);
    let (win, gc, size) = x11_display::setup_window(&conn, screen_num)?;

    eprintln!(
        "window mapped id=0x{win:x} x={} y={} width={} height={}",
        x11::display::WINDOW_X,
        x11::display::WINDOW_Y,
        size.0,
        size.1
    );

    let mut display_backend = match create_display_backend(display_kind, &conn, win, gc, size) {
        Ok(backend) => backend,
        Err(primary) => {
            if let Err(cleanup_error) = x11_display::cleanup(&conn, win, gc, true) {
                eprintln!(
                    "cleanup after display initialization failure also failed: {cleanup_error:#}"
                );
            }
            return Err(primary);
        }
    };
    let mut paperspoon = net::Paperspoon::start(paperpad_config, remote_viewport_size(size));
    let event_result = event_loop(&conn, win, display_backend.as_mut(), &mut paperspoon);
    drop(display_backend);

    let destroy_window = match &event_result {
        Ok(EventLoopExit::WindowDestroyed) => false,
        Err(_) => true,
    };
    let cleanup_result = x11_display::cleanup(&conn, win, gc, destroy_window);

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

fn create_display_backend<'a>(
    kind: config::DisplayBackendKind,
    conn: &'a RustConnection,
    win: Window,
    gc: Gcontext,
    size: (u16, u16),
) -> Result<Box<dyn DisplayBackend + 'a>> {
    match kind {
        config::DisplayBackendKind::X11 => {
            Ok(Box::new(X11DisplayBackend::new(conn, win, gc, size)))
        }
        config::DisplayBackendKind::Mxcfb => {
            #[cfg(target_os = "linux")]
            {
                let backend = MxcfbDisplayBackend::open()?;
                anyhow::ensure!(
                    backend.dimensions() == size,
                    "MXCFB framebuffer {:?} differs from X11 input window {:?}",
                    backend.dimensions(),
                    size
                );
                Ok(Box::new(backend))
            }
            #[cfg(not(target_os = "linux"))]
            {
                bail!("display backend=mxcfb requires Linux and /dev/fb0")
            }
        }
    }
}
