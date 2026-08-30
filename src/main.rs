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

mod bonjour;
mod mcast;
mod net;
mod proto;
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
    // Diagnostic-only multicast probe: bounded listen on 224.0.0.251:5353
    // while the operator floods the group from the Mac. Exits without X11.
    if env::var("RUST_X11_HELLO_MCAST_PROBE").is_ok() {
        let timeout = env::var("RUST_X11_HELLO_MCAST_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(std::time::Duration::from_millis)
            .unwrap_or(mcast::DEFAULT_PROBE_TIMEOUT);
        let received = mcast::probe_multicast(timeout)?;
        return if received {
            eprintln!("multicast probe result=received");
            Ok(())
        } else {
            Err(anyhow::anyhow!("multicast probe: no multicast received"))
        };
    }

    // Diagnostic-only Bonjour browse: when requested, run a bounded browse
    // of `_paperspoon._tcp.local.` (or the overridden service type) and exit
    // without touching X11. This is the Phase 3 device probe; the normal
    // path below is unchanged.
    if env::var("RUST_X11_HELLO_BONJOUR_DIAGNOSTIC").is_ok() {
        let service_type = env::var("RUST_X11_HELLO_BONJOUR_SERVICE")
            .unwrap_or_else(|_| bonjour::SERVICE_TYPE.to_string());
        let timeout = env::var("RUST_X11_HELLO_BONJOUR_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(std::time::Duration::from_millis)
            .unwrap_or(bonjour::DEFAULT_BROWSE_TIMEOUT);
        let result = bonjour::browse_bonjour(&service_type, timeout)?;
        match result {
            Some(candidate) => {
                eprintln!(
                    "discovery bonjour result instance={} address={} port={}",
                    candidate.instance,
                    candidate.addr.ip(),
                    candidate.addr.port()
                );
                return Ok(());
            }
            None => {
                return Err(anyhow::anyhow!(
                    "bonjour diagnostic: no usable {service_type} service found"
                ));
            }
        }
    }

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

    let mut paperspoon = match net::Paperspoon::connect() {
        Ok(paperspoon) => {
            eprintln!("transport: connected to PaperSpoon");
            paperspoon
        }
        Err(error) => {
            // A PaperSpoon that is down at startup is not fatal: the event
            // loop runs, and each activation attempts a (bounded) reconnect.
            eprintln!("transport error at startup: {error:#}");
            net::Paperspoon::disconnected()
        }
    };

    let event_result = event_loop(&conn, win, gc, &mut paperspoon);

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
