//! Native evdev polling for bounded MXCFB benchmarks.

use super::evdev::{DecodedInput, EvdevReader};
use crate::app::{Activation, AppState};
use anyhow::{Context, Result};

pub(crate) struct BenchmarkExitPoll {
    app: AppState,
}

impl BenchmarkExitPoll {
    pub(crate) fn new(size: (u16, u16)) -> Self {
        Self {
            app: AppState::new(size),
        }
    }

    pub(crate) fn shutdown(&mut self, input_reader: &mut EvdevReader) {
        if input_reader.shutdown().is_some() {
            self.app.cancel_contact();
            eprintln!("input source=evdev action=cancel reason=benchmark_shutdown");
        }
    }

    pub(crate) fn poll(&mut self, input_reader: &mut EvdevReader) -> Result<bool> {
        let decoded = match input_reader.drain() {
            Ok(decoded) => decoded,
            Err(error) => {
                if error.cancel_contact() {
                    self.app.cancel_contact();
                    eprintln!("input source=evdev action=cancel reason=reader_failure");
                }
                return Err(error).context("native evdev input reader failed during benchmark");
            }
        };

        for input in decoded {
            match input {
                DecodedInput::Pointer(event) => {
                    eprintln!(
                        "input source=evdev type={:?} detail={} physical_x={} physical_y={}",
                        event.kind, event.detail, event.point.x, event.point.y
                    );
                    if self
                        .app
                        .handle_pointer(event)
                        .activation()
                        .is_some_and(dispatch_activation)
                    {
                        return Ok(true);
                    }
                }
                DecodedInput::CancelContact => {
                    self.app.cancel_contact();
                    eprintln!("input source=evdev action=cancel reason=decoder_state");
                }
            }
        }

        Ok(false)
    }
}

fn dispatch_activation(activation: Activation) -> bool {
    match activation {
        Activation::Exit => {
            eprintln!("ui action=activate system=exit");
            true
        }
    }
}
