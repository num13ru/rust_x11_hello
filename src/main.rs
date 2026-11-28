//! Paperpad Kindle/KUAL process entry point.
//!
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{Result, bail};
#[cfg(target_os = "linux")]
use runtime_wait::RuntimeNotifier;
use std::env;
#[cfg(target_os = "linux")]
use std::io;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

mod app;
mod config;
mod discovery;
mod display;
mod input;
mod latency;
mod mxcfb;
#[cfg(target_os = "linux")]
mod native_runtime;
mod net;
mod runtime;
#[cfg(target_os = "linux")]
mod runtime_wait;
mod ui;

#[cfg(target_os = "linux")]
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "linux")]
static SHUTDOWN_WAKE_FD: AtomicI32 = AtomicI32::new(-1);
#[cfg(target_os = "linux")]
static SHUTDOWN_WAKE_KEEPALIVE: OnceLock<RuntimeNotifier> = OnceLock::new();

fn main() -> Result<()> {
    latency::init_from_env();
    print_environment();
    run()
}

fn print_environment() {
    eprintln!("PaperPad Kindle/KUAL runtime");
    eprintln!("target_arch: {}", env::consts::ARCH);
    eprintln!("target_os: {}", env::consts::OS);
}

#[cfg(target_os = "linux")]
extern "C" fn request_shutdown(_signal: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::Release);
    let wake_fd = SHUTDOWN_WAKE_FD.load(Ordering::Acquire);
    if wake_fd >= 0 {
        let value = 1_u64;
        // SAFETY: `write(2)` is async-signal-safe. The process-lifetime
        // `SHUTDOWN_WAKE_KEEPALIVE` owns this eventfd after publication.
        // Saturation or another write error means the atomic shutdown flag
        // remains authoritative, so the handler must not retry or inspect it.
        let _ = unsafe {
            libc::write(
                wake_fd,
                (&value as *const u64).cast::<libc::c_void>(),
                std::mem::size_of::<u64>(),
            )
        };
    }
}

#[cfg(target_os = "linux")]
fn register_shutdown_wake(notifier: RuntimeNotifier) -> Result<()> {
    let wake_fd = notifier.raw_fd();
    SHUTDOWN_WAKE_KEEPALIVE
        .set(notifier)
        .map_err(|_| anyhow::anyhow!("PaperPad shutdown wake is already registered"))?;
    SHUTDOWN_WAKE_FD.store(wake_fd, Ordering::Release);

    // Close the race where a signal set the flag while no wake FD was yet
    // published. A later signal writes the eventfd directly.
    if SHUTDOWN_REQUESTED.load(Ordering::Acquire) {
        SHUTDOWN_WAKE_KEEPALIVE
            .get()
            .expect("registered shutdown notifier")
            .notify()
            .context("wake native runtime after early shutdown signal")?;
    }
    Ok(())
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
    if env::args_os().nth(1).is_some() {
        bail!("usage: rust_x11_hello");
    }

    #[cfg(target_os = "linux")]
    install_signal_handlers()?;

    #[cfg(target_os = "linux")]
    {
        native_runtime::run(
            || SHUTDOWN_REQUESTED.load(Ordering::Acquire),
            register_shutdown_wake,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        bail!("normal PaperPad runtime requires Linux and /dev/fb0")
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::runtime_wait::RuntimeWait;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    #[test]
    fn shutdown_before_wake_registration_is_observable() {
        SHUTDOWN_REQUESTED.store(false, Ordering::Release);
        SHUTDOWN_WAKE_FD.store(-1, Ordering::Release);
        request_shutdown(libc::SIGTERM);

        let (input, _input_peer) = UnixStream::pair().expect("create input socket pair");
        let (waiter, notifier) =
            RuntimeWait::new(input.as_raw_fd()).expect("create runtime waiter");
        register_shutdown_wake(notifier).expect("register shutdown wake");

        let ready = waiter
            .wait(Some(Instant::now() + Duration::from_secs(1)))
            .expect("wait for early shutdown notification");
        assert!(SHUTDOWN_REQUESTED.load(Ordering::Acquire));
        assert!(ready.notified);

        SHUTDOWN_REQUESTED.store(false, Ordering::Release);
    }
}
