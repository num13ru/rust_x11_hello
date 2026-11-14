//! Paperpad Kindle/KUAL process entry point.
//!
//! Entry point only: chooses a display backend, keeps X11 for window ownership
//! and lifecycle, runs the event loop, and tears down. Logical UI concepts live
//! in [`ui`].

use crate::display::DisplayBackend;
use anyhow::{Context, Result, bail};
#[cfg(target_os = "linux")]
use input::evdev::EvdevReader;
#[cfg(target_os = "linux")]
use mxcfb::MxcfbDisplayBackend;
use std::env;
#[cfg(target_os = "linux")]
use std::io;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
use x11rb::rust_connection::RustConnection;

use runtime::remote_viewport_size;
use x11::X11DisplayBackend;
use x11::display::{self as x11_display, MappedWindow, WindowMode};
#[cfg(target_os = "linux")]
use x11::events::BenchmarkExitPoll;
use x11::events::{EventLoopExit, event_loop};

mod app;
mod config;
mod discovery;
mod display;
mod input;
mod latency;
mod mxcfb;
#[cfg(target_os = "linux")]
mod native_trial;
mod net;
mod runtime;
mod ui;
mod x11;

#[cfg(target_os = "linux")]
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

fn main() -> Result<()> {
    latency::init_from_env();
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

#[cfg(target_os = "linux")]
extern "C" fn request_shutdown(_signal: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

#[cfg(target_os = "linux")]
fn install_signal_handlers() -> Result<()> {
    // SAFETY: zero is a valid initial representation before all fields used by
    // sigaction(2) are initialized below.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = request_shutdown as usize;
    action.sa_flags = 0;
    // SAFETY: `action.sa_mask` is a valid writable sigset_t.
    if unsafe { libc::sigemptyset(&mut action.sa_mask) } != 0 {
        return Err(io::Error::last_os_error()).context("initialize PaperPad signal mask");
    }
    for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM] {
        // SAFETY: `action` remains valid for the call and the handler performs
        // only an atomic store. No previous disposition needs restoring because
        // PaperPad exits after this run.
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0 {
            return Err(io::Error::last_os_error())
                .with_context(|| format!("install PaperPad handler for signal {signal}"));
        }
    }
    Ok(())
}

fn run() -> Result<()> {
    let mut args = env::args_os();
    let _program = args.next();
    match (args.next(), args.next()) {
        (None, None) => {}
        (Some(arg), None) if arg == "--inspect-framebuffer" => {
            return mxcfb::inspect_framebuffer();
        }
        (Some(arg), None) if arg == "--inspect-input" => {
            return input::inspect_input();
        }
        (Some(arg), None) if arg == "--trace-input" => {
            return input::trace_input();
        }
        (Some(arg), None) if arg == "--benchmark-mxcfb" => {
            return run_mxcfb_benchmark();
        }
        (Some(arg), None) if arg == "--trial-mxcfb-no-x11" => {
            return run_mxcfb_no_x11_trial();
        }
        (Some(arg), None) if arg == "--trial-mxcfb-grab-no-x11" => {
            return run_mxcfb_grab_no_x11_trial();
        }
        _ => {
            bail!(
                "usage: rust_x11_hello [--inspect-framebuffer|--inspect-input|--trace-input|--benchmark-mxcfb|--trial-mxcfb-no-x11|--trial-mxcfb-grab-no-x11]"
            )
        }
    }

    #[cfg(target_os = "linux")]
    install_signal_handlers()?;

    let display_kind = config::DisplayBackendKind::from_env()?;
    let framebuffer_size = match display_kind {
        config::DisplayBackendKind::X11 => None,
        config::DisplayBackendKind::Mxcfb => {
            #[cfg(target_os = "linux")]
            {
                Some(mxcfb::validated_framebuffer_dimensions()?)
            }
            #[cfg(not(target_os = "linux"))]
            {
                bail!("display backend=mxcfb requires Linux and /dev/fb0");
            }
        }
    };
    let paperpad_config = config::PaperpadConfig::from_env()?;

    let (conn, screen_num) = RustConnection::connect(None)
        .context("failed to connect to X11 display; check DISPLAY and /tmp/.X11-unix/X0")?;

    x11_display::print_screen_info(&conn, screen_num);
    let window_mode = match display_kind {
        config::DisplayBackendKind::X11 => WindowMode::Drawable,
        config::DisplayBackendKind::Mxcfb => WindowMode::DirectFramebuffer,
    };
    let mapped = x11_display::setup_window(&conn, screen_num, window_mode)?;
    let win = mapped.window;
    let size = if let Some(framebuffer_size) = framebuffer_size {
        anyhow::ensure!(
            mapped.size == framebuffer_size,
            "X11 ownership window {:?} differs from validated MXCFB framebuffer {:?}",
            mapped.size,
            framebuffer_size
        );
        framebuffer_size
    } else {
        mapped.size
    };

    eprintln!(
        "window mapped id=0x{win:x} class={} x={} y={} width={} height={}",
        mapped.mode.label(),
        x11::display::WINDOW_X,
        x11::display::WINDOW_Y,
        size.0,
        size.1
    );

    #[cfg(target_os = "linux")]
    let mut input_reader = match EvdevReader::open(size) {
        Ok(reader) => reader,
        Err(primary) => {
            if let Err(cleanup_error) = x11_display::cleanup(&conn, mapped, true) {
                eprintln!(
                    "X11 cleanup after input initialization failure also failed: {cleanup_error:#}"
                );
            }
            return Err(primary);
        }
    };

    let mut display_backend = match create_display_backend(display_kind, &conn, mapped) {
        Ok(backend) => backend,
        Err(primary) => {
            if let Err(cleanup_error) = x11_display::cleanup(&conn, mapped, true) {
                eprintln!(
                    "cleanup after display initialization failure also failed: {cleanup_error:#}"
                );
            }
            return Err(primary);
        }
    };
    let mut paperspoon = net::Paperspoon::start(
        paperpad_config,
        remote_viewport_size(size),
        display_backend.pixel_formats(),
    );
    #[cfg(target_os = "linux")]
    let event_result = event_loop(
        &conn,
        win,
        display_backend.as_mut(),
        &mut paperspoon,
        &mut input_reader,
        || SHUTDOWN_REQUESTED.load(Ordering::Relaxed),
    );
    #[cfg(not(target_os = "linux"))]
    let event_result = event_loop(&conn, win, display_backend.as_mut(), &mut paperspoon);

    #[cfg(target_os = "linux")]
    {
        if input_reader.shutdown().is_some() {
            eprintln!("input reader: shutdown canceled active contact");
        }
        drop(input_reader);
    }

    let display_cleanup_result = display_backend.shutdown();
    drop(display_backend);

    let destroy_window = !matches!(&event_result, Ok(EventLoopExit::WindowDestroyed));
    let cleanup_result = x11_display::cleanup(&conn, mapped, destroy_window);

    match event_result {
        Err(primary) => {
            if let Err(cleanup_error) = display_cleanup_result {
                eprintln!(
                    "display cleanup after event-loop failure also failed: {cleanup_error:#}"
                );
            }
            if let Err(cleanup_error) = cleanup_result {
                eprintln!("X11 cleanup after event-loop failure also failed: {cleanup_error:#}");
            }
            Err(primary)
        }
        Ok(_) => match (display_cleanup_result, cleanup_result) {
            (Err(primary), Err(cleanup_error)) => {
                eprintln!(
                    "X11 cleanup after display cleanup failure also failed: {cleanup_error:#}"
                );
                Err(primary)
            }
            (Err(primary), Ok(())) => Err(primary),
            (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
            (Ok(()), Ok(())) => Ok(()),
        },
    }
}

#[cfg(target_os = "linux")]
fn run_mxcfb_benchmark() -> Result<()> {
    let config = mxcfb::BenchmarkConfig::from_env()?;
    install_signal_handlers()?;
    let (conn, screen_num) = RustConnection::connect(None)
        .context("failed to connect to X11 display; check DISPLAY and /tmp/.X11-unix/X0")?;
    x11_display::print_screen_info(&conn, screen_num);
    let mapped = x11_display::setup_window(&conn, screen_num, WindowMode::DirectFramebuffer)?;
    let win = mapped.window;
    let size = mapped.size;
    eprintln!(
        "benchmark input window mapped id=0x{win:x} class={} x={} y={} width={} height={}",
        mapped.mode.label(),
        x11::display::WINDOW_X,
        x11::display::WINDOW_Y,
        size.0,
        size.1
    );

    let benchmark_result = (|| -> Result<(mxcfb::BenchmarkOutcome, bool)> {
        let mut benchmark = mxcfb::MxcfbBenchmark::open(config)?;
        anyhow::ensure!(
            benchmark.dimensions() == size,
            "MXCFB framebuffer {:?} differs from X11 input window {:?}",
            benchmark.dimensions(),
            size
        );
        let mut input_reader = EvdevReader::open(size)?;
        let mut benchmark_exit = BenchmarkExitPoll::new(size);
        let run_result = benchmark.run(|| {
            if SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
                eprintln!("mxcfb benchmark graceful shutdown requested by signal");
                Ok(true)
            } else {
                benchmark_exit.poll(&mut input_reader, &conn, win)
            }
        });
        benchmark_exit.shutdown(&mut input_reader);
        drop(input_reader);
        let window_destroyed = benchmark_exit.window_destroyed();
        let restore_result = benchmark.restore_initial_screen();
        match (run_result, restore_result) {
            (Err(primary), Err(restore_error)) => {
                eprintln!(
                    "framebuffer restoration after benchmark failure also failed: {restore_error:#}"
                );
                Err(primary)
            }
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(restore_error)) => Err(restore_error),
            (Ok(outcome), Ok(())) => Ok((outcome, window_destroyed)),
        }
    })();

    let destroy_window = !matches!(&benchmark_result, Ok((_, true)));
    let cleanup_result = x11_display::cleanup(&conn, mapped, destroy_window);

    match (benchmark_result, cleanup_result) {
        (Err(primary), Err(cleanup_error)) => {
            eprintln!("cleanup after benchmark failure also failed: {cleanup_error:#}");
            Err(primary)
        }
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(cleanup_error)) => Err(cleanup_error),
        (Ok((mxcfb::BenchmarkOutcome::Completed, _)), Ok(())) => Ok(()),
        (Ok((mxcfb::BenchmarkOutcome::Stopped { completed }, _)), Ok(())) => {
            eprintln!("mxcfb benchmark exited early completed={completed}");
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
fn run_mxcfb_no_x11_trial() -> Result<()> {
    install_signal_handlers()?;
    native_trial::run(|| SHUTDOWN_REQUESTED.load(Ordering::Relaxed))
}

#[cfg(target_os = "linux")]
fn run_mxcfb_grab_no_x11_trial() -> Result<()> {
    install_signal_handlers()?;
    native_trial::run_exclusive(|| SHUTDOWN_REQUESTED.load(Ordering::Relaxed))
}

#[cfg(not(target_os = "linux"))]
fn run_mxcfb_no_x11_trial() -> Result<()> {
    bail!("no-X11 MXCFB ownership trial requires Linux and /dev/fb0")
}

#[cfg(not(target_os = "linux"))]
fn run_mxcfb_grab_no_x11_trial() -> Result<()> {
    bail!("no-X11 MXCFB exclusive-input trial requires Linux and /dev/fb0")
}

#[cfg(not(target_os = "linux"))]
fn run_mxcfb_benchmark() -> Result<()> {
    bail!("MXCFB benchmark requires Linux and /dev/fb0")
}

fn create_display_backend<'a>(
    kind: config::DisplayBackendKind,
    conn: &'a RustConnection,
    mapped: MappedWindow,
) -> Result<Box<dyn DisplayBackend + 'a>> {
    let win = mapped.window;
    let size = mapped.size;
    match kind {
        config::DisplayBackendKind::X11 => {
            let gc = mapped
                .gc
                .context("drawable X11 backend window is missing its graphics context")?;
            Ok(Box::new(X11DisplayBackend::new(conn, win, gc, size)))
        }
        config::DisplayBackendKind::Mxcfb => {
            #[cfg(target_os = "linux")]
            {
                anyhow::ensure!(
                    mapped.mode == WindowMode::DirectFramebuffer && mapped.gc.is_none(),
                    "MXCFB window must occlude lower X11 windows without a graphics context"
                );
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
