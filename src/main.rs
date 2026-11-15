//! Paperpad Kindle/KUAL process entry point.
//!
//! Entry point only: chooses a diagnostic, benchmark, or normal native MXCFB
//! lifecycle and tears it down. Logical UI concepts live in [`ui`].

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{Result, bail};
#[cfg(target_os = "linux")]
use input::evdev::EvdevReader;
use std::env;
#[cfg(target_os = "linux")]
use std::io;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "linux")]
use x11rb::rust_connection::RustConnection;

#[cfg(target_os = "linux")]
use x11::display::{self as x11_display, WindowMode};
#[cfg(target_os = "linux")]
use x11::events::BenchmarkExitPoll;

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
        (Some(arg), None) if arg == "--trial-connected-no-x11" => {
            return run_connected_no_x11_trial();
        }
        _ => {
            bail!(
                "usage: rust_x11_hello [--inspect-framebuffer|--inspect-input|--trace-input|--benchmark-mxcfb|--trial-mxcfb-no-x11|--trial-mxcfb-grab-no-x11|--trial-connected-no-x11]"
            )
        }
    }

    #[cfg(target_os = "linux")]
    install_signal_handlers()?;

    #[cfg(target_os = "linux")]
    {
        native_trial::run_normal(|| SHUTDOWN_REQUESTED.load(Ordering::Relaxed))
    }
    #[cfg(not(target_os = "linux"))]
    {
        bail!("normal PaperPad runtime requires Linux and /dev/fb0")
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

#[cfg(target_os = "linux")]
fn run_connected_no_x11_trial() -> Result<()> {
    install_signal_handlers()?;
    native_trial::run_connected(|| SHUTDOWN_REQUESTED.load(Ordering::Relaxed))
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
fn run_connected_no_x11_trial() -> Result<()> {
    bail!("connected no-X11 trial requires Linux and /dev/fb0")
}

#[cfg(not(target_os = "linux"))]
fn run_mxcfb_benchmark() -> Result<()> {
    bail!("MXCFB benchmark requires Linux and /dev/fb0")
}
