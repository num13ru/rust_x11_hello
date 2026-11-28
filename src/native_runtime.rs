//! Normal native MXCFB PaperPad runtime.

use crate::config::PaperpadConfig;
use crate::input::evdev::{DecodedInput, EvdevReader};
use crate::kindle_power::KindlePowerMonitor;
use crate::latency;
use crate::mxcfb::MxcfbDisplayBackend;
use crate::net::Paperspoon;
use crate::runtime::{PaperpadRuntime, RuntimeControl, RuntimePointerInput, remote_viewport_size};
use crate::runtime_wait::{RuntimeNotifier, RuntimeWait};
use anyhow::{Context, Result};
use std::time::Instant;

pub(crate) fn run(
    shutdown_requested: impl Fn() -> bool,
    register_shutdown_wake: impl FnOnce(RuntimeNotifier) -> Result<()>,
) -> Result<()> {
    let config = PaperpadConfig::from_env()?;
    let run_label = "PaperPad native runtime";
    eprintln!("{run_label} starting transport=true eviocgrab=true");

    let mut power = KindlePowerMonitor::start()?;
    let mut display = MxcfbDisplayBackend::open()?;
    let size = display.dimensions();
    let mut input_reader = EvdevReader::open_exclusive(size)?;
    let (runtime_wait, runtime_notifier) = RuntimeWait::new(input_reader.raw_fd())
        .context("initialize native PaperPad runtime readiness wait")?;
    register_shutdown_wake(runtime_notifier.clone())
        .context("register native PaperPad shutdown wake")?;
    let mut paperspoon = Paperspoon::start(
        config,
        remote_viewport_size(size),
        display.pixel_formats(),
        runtime_notifier,
    );
    let mut runtime = PaperpadRuntime::new(size);

    let mut run_result = (|| -> Result<()> {
        eprintln!(
            "{run_label} ready width={} height={} exit=local",
            size.0, size.1
        );

        let ready = runtime_wait
            .wait_for_session(power.fd(), Some(Instant::now()))
            .context("check Kindle power before initial draw")?;
        if ready.power.is_some() {
            return Ok(());
        }
        if shutdown_requested() {
            eprintln!("{run_label} stopped reason=signal");
            return Ok(());
        }
        display.start()?;
        runtime
            .service(&mut display, &mut paperspoon)
            .context("service initial native PaperPad state")?;

        loop {
            if shutdown_requested() {
                runtime.cancel_contact();
                eprintln!("{run_label} stopped reason=signal");
                return Ok(());
            }

            let ready = runtime_wait
                .wait_for_session(power.fd(), display.next_maintenance_deadline())
                .context("wait for native PaperPad runtime readiness")?;
            if ready.power.is_some() {
                return Ok(());
            }
            if shutdown_requested() {
                runtime.cancel_contact();
                eprintln!("{run_label} stopped reason=signal");
                return Ok(());
            }

            runtime
                .service(&mut display, &mut paperspoon)
                .context("service native PaperPad runtime")?;

            if ready.input.readable {
                let decoded = match input_reader.drain() {
                    Ok(decoded) => decoded,
                    Err(error) => {
                        if error.cancel_contact() {
                            runtime.cancel_contact();
                            eprintln!("input source=evdev action=cancel reason=reader_failure");
                        }
                        return Err(error).context("native PaperPad evdev reader failed");
                    }
                };
                for input in decoded {
                    match input {
                        DecodedInput::Pointer(event) => {
                            let input_stamp = latency::stamp();
                            eprintln!(
                                "input source=evdev type={:?} detail={} physical_x={} physical_y={}",
                                event.kind, event.detail, event.point.x, event.point.y,
                            );
                            let control = runtime.handle_pointer(
                                RuntimePointerInput::new(event, input_stamp, None),
                                &mut paperspoon,
                            );
                            if control == RuntimeControl::ShutdownRequested {
                                eprintln!("{run_label} stopped reason=local_exit");
                                return Ok(());
                            }
                        }
                        DecodedInput::CancelContact => {
                            runtime.cancel_contact();
                            eprintln!("input source=evdev action=cancel reason=decoder_state");
                        }
                    }
                }
            }

            if ready.input.error || ready.input.hung_up || ready.input.invalid {
                runtime.cancel_contact();
                eprintln!(
                    "input source=evdev action=cancel reason=poll_terminal error={} hangup={} invalid={}",
                    ready.input.error, ready.input.hung_up, ready.input.invalid
                );
                return Err(anyhow::anyhow!(
                    "native PaperPad evdev readiness failed: error={} hangup={} invalid={}",
                    ready.input.error,
                    ready.input.hung_up,
                    ready.input.invalid
                ));
            }
        }
    })();

    runtime.cancel_contact();
    if input_reader.shutdown().is_some() {
        eprintln!("input source=evdev action=cancel reason=runtime_shutdown");
    }
    if let Err(release_error) = input_reader.release_exclusive_grab() {
        if run_result.is_err() {
            eprintln!(
                "EVIOCGRAB release after native runtime failure also failed: {release_error:#}"
            );
        } else {
            run_result = Err(release_error);
        }
    }
    // A power transition may race a manual exit or a runtime error.
    let ready = runtime_wait
        .wait_for_session(power.fd(), Some(Instant::now()))
        .context("check Kindle power before restoring the launch screen");
    drop(runtime_wait);
    drop(input_reader);

    let cleanup_result = ready.and_then(|ready| match ready.power {
        Some(status) if status.readable || status.hung_up => power.report_transition(),
        Some(status) => anyhow::bail!("Kindle power listener readiness failed: {status:?}"),
        None => display.shutdown(),
    });
    drop(display);
    drop(paperspoon);

    match (run_result, cleanup_result) {
        (Err(primary), Err(cleanup_error)) => {
            eprintln!(
                "Display cleanup after native runtime failure also failed: {cleanup_error:#}"
            );
            Err(primary)
        }
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
        (Ok(()), Ok(())) => Ok(()),
    }
}
