//! Bounded, opt-in MXCFB ownership trial that never connects to X11.

use crate::app::{Activation, AppState};
use crate::config::PaperpadConfig;
use crate::display::DisplayBackend;
use crate::input::evdev::{DecodedInput, EvdevReader};
use crate::latency;
use crate::mxcfb::MxcfbDisplayBackend;
use crate::net::Paperspoon;
use crate::runtime::{PaperpadRuntime, RuntimeControl, RuntimePointerInput, remote_viewport_size};
use anyhow::{Context, Result};
use std::time::{Duration, Instant};

const TRIAL_DURATION: Duration = Duration::from_secs(15);
const CONNECTED_TRIAL_DURATION: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) fn run(shutdown_requested: impl Fn() -> bool) -> Result<()> {
    run_with_input_grab(false, shutdown_requested)
}

pub(crate) fn run_exclusive(shutdown_requested: impl Fn() -> bool) -> Result<()> {
    run_with_input_grab(true, shutdown_requested)
}

fn run_with_input_grab(grab: bool, shutdown_requested: impl Fn() -> bool) -> Result<()> {
    let trial_name = if grab {
        "no-X11 exclusive-input trial"
    } else {
        "no-X11 ownership trial"
    };
    eprintln!(
        "{trial_name} starting duration_s={} transport=false eviocgrab={grab}",
        TRIAL_DURATION.as_secs(),
    );

    let mut display = MxcfbDisplayBackend::open()?;
    let size = display.dimensions();
    let input_result = if grab {
        EvdevReader::open_exclusive_trial(size)
    } else {
        EvdevReader::open(size)
    };
    let mut input_reader = match input_result {
        Ok(reader) => reader,
        Err(primary) => {
            if let Err(cleanup_error) = display.shutdown() {
                eprintln!(
                    "MXCFB cleanup after no-X11 input initialization failure also failed: {cleanup_error:#}"
                );
            }
            return Err(primary);
        }
    };
    let mut app = AppState::new(size);

    let mut trial_result = (|| -> Result<()> {
        display
            .draw_system_ui(false)
            .context("draw no-X11 trial system UI")?;
        let deadline = Instant::now() + TRIAL_DURATION;
        eprintln!(
            "{trial_name} ready width={} height={} exit=local deadline_s={}",
            size.0,
            size.1,
            TRIAL_DURATION.as_secs()
        );

        loop {
            if shutdown_requested() {
                app.cancel_contact();
                eprintln!("{trial_name} stopped reason=signal");
                return Ok(());
            }
            if Instant::now() >= deadline {
                app.cancel_contact();
                eprintln!("{trial_name} stopped reason=deadline");
                return Ok(());
            }

            display
                .maintain()
                .context("maintain no-X11 trial display")?;

            let decoded = match input_reader.drain() {
                Ok(decoded) => decoded,
                Err(error) => {
                    if error.cancel_contact() {
                        app.cancel_contact();
                        eprintln!("input source=evdev action=cancel reason=reader_failure");
                    }
                    return Err(error)
                        .context("native evdev input reader failed during no-X11 trial");
                }
            };

            for input in decoded {
                match input {
                    DecodedInput::Pointer(event) => {
                        eprintln!(
                            "input source=evdev type={:?} detail={} physical_x={} physical_y={}",
                            event.kind, event.detail, event.point.x, event.point.y
                        );
                        if app.handle_pointer(event).activation() == Some(Activation::Exit) {
                            eprintln!("ui action=activate system=exit");
                            eprintln!("{trial_name} stopped reason=local_exit");
                            return Ok(());
                        }
                    }
                    DecodedInput::CancelContact => {
                        app.cancel_contact();
                        eprintln!("input source=evdev action=cancel reason=decoder_state");
                    }
                }
            }

            std::thread::sleep(POLL_INTERVAL);
        }
    })();

    if input_reader.shutdown().is_some() {
        app.cancel_contact();
        eprintln!("input source=evdev action=cancel reason=trial_shutdown");
    }
    if let Err(release_error) = input_reader.release_exclusive_grab() {
        if trial_result.is_err() {
            eprintln!(
                "EVIOCGRAB release after no-X11 trial failure also failed: {release_error:#}"
            );
        } else {
            trial_result = Err(release_error);
        }
    }
    drop(input_reader);

    let cleanup_result = display.shutdown();
    drop(display);

    match (trial_result, cleanup_result) {
        (Err(primary), Err(cleanup_error)) => {
            eprintln!("MXCFB cleanup after no-X11 trial failure also failed: {cleanup_error:#}");
            Err(primary)
        }
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

pub(crate) fn run_connected(shutdown_requested: impl Fn() -> bool) -> Result<()> {
    let config = PaperpadConfig::from_env()?;
    eprintln!(
        "no-X11 connected trial starting duration_s={} transport=true eviocgrab=true",
        CONNECTED_TRIAL_DURATION.as_secs(),
    );

    let mut display = MxcfbDisplayBackend::open()?;
    let size = display.dimensions();
    let mut input_reader = match EvdevReader::open_exclusive_trial(size) {
        Ok(reader) => reader,
        Err(primary) => {
            if let Err(cleanup_error) = display.shutdown() {
                eprintln!(
                    "MXCFB cleanup after connected no-X11 input initialization failure also failed: {cleanup_error:#}"
                );
            }
            return Err(primary);
        }
    };
    let mut paperspoon =
        Paperspoon::start(config, remote_viewport_size(size), display.pixel_formats());
    let mut runtime = PaperpadRuntime::new(size);

    let mut trial_result = (|| -> Result<()> {
        let deadline = Instant::now() + CONNECTED_TRIAL_DURATION;
        eprintln!(
            "no-X11 connected trial ready width={} height={} exit=local deadline_s={}",
            size.0,
            size.1,
            CONNECTED_TRIAL_DURATION.as_secs(),
        );

        loop {
            if shutdown_requested() {
                runtime.cancel_contact();
                eprintln!("no-X11 connected trial stopped reason=signal");
                return Ok(());
            }
            if Instant::now() >= deadline {
                runtime.cancel_contact();
                eprintln!("no-X11 connected trial stopped reason=deadline");
                return Ok(());
            }

            runtime
                .service(&mut display, &mut paperspoon)
                .context("service connected no-X11 runtime")?;
            let decoded = match input_reader.drain() {
                Ok(decoded) => decoded,
                Err(error) => {
                    if error.cancel_contact() {
                        runtime.cancel_contact();
                        eprintln!("input source=evdev action=cancel reason=reader_failure");
                    }
                    return Err(error).context("connected no-X11 evdev reader failed");
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
                            eprintln!("no-X11 connected trial stopped reason=local_exit");
                            return Ok(());
                        }
                    }
                    DecodedInput::CancelContact => {
                        runtime.cancel_contact();
                        eprintln!("input source=evdev action=cancel reason=decoder_state");
                    }
                }
            }

            std::thread::sleep(POLL_INTERVAL);
        }
    })();

    if input_reader.shutdown().is_some() {
        runtime.cancel_contact();
        eprintln!("input source=evdev action=cancel reason=trial_shutdown");
    }
    if let Err(release_error) = input_reader.release_exclusive_grab() {
        if trial_result.is_err() {
            eprintln!(
                "EVIOCGRAB release after connected no-X11 trial failure also failed: {release_error:#}"
            );
        } else {
            trial_result = Err(release_error);
        }
    }
    drop(input_reader);
    drop(paperspoon);

    let cleanup_result = display.shutdown();
    drop(display);

    match (trial_result, cleanup_result) {
        (Err(primary), Err(cleanup_error)) => {
            eprintln!(
                "MXCFB cleanup after connected no-X11 trial failure also failed: {cleanup_error:#}"
            );
            Err(primary)
        }
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
        (Ok(()), Ok(())) => Ok(()),
    }
}
