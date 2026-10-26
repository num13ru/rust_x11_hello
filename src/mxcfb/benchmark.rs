//! Bounded, opt-in PW6 MXCFB completion benchmark.
//!
//! The benchmark bypasses PaperSpoon so network throughput cannot be confused
//! with panel timing. It advances only after the kernel reports the submitted
//! marker complete. That is still not proof of visual fidelity; a camera and
//! operator observation remain necessary for that claim.

#[cfg(target_os = "linux")]
use std::env;
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

#[cfg(target_os = "linux")]
use anyhow::bail;
use anyhow::{Context, Result, ensure};
#[cfg(target_os = "linux")]
use paper_protocol::PixelFormat;

#[cfg(target_os = "linux")]
use crate::display::RemoteFrame;

#[cfg(target_os = "linux")]
use super::controller;
#[cfg(target_os = "linux")]
use super::opened::OpenedFramebuffer;
#[cfg(target_os = "linux")]
use super::present::{present_remote_with_waveform, present_system_ui_with};
#[cfg(target_os = "linux")]
use super::update_marker::UpdateMarkerSequence;
use super::update_request::MxcfbWaveform;
#[cfg(target_os = "linux")]
use super::update_request::next_full_screen_gc16_request;

const WAVEFORM_ENV: &str = "PAPERPAD_MXCFB_BENCHMARK_WAVEFORM";
const FRAMES_ENV: &str = "PAPERPAD_MXCFB_BENCHMARK_FRAMES";
const DEFAULT_WAVEFORM: MxcfbWaveform = MxcfbWaveform::Gc16;
const DEFAULT_FRAMES: usize = 8;
const MIN_FRAMES: usize = 2;
const MAX_FRAMES: usize = 120;
#[cfg(any(target_os = "linux", test))]
const MIN_ATTRIBUTABLE_WAIT_US: u128 = 1_000;
#[cfg(target_os = "linux")]
const MAX_COMPLETION_WAITS: usize = 1;
// The source-derived marker-specific ioctl returns zero for completion,
// a missing marker, and its five-second timeout. Durations outside these
// bounds are not attributable to the submitted update.
#[cfg(any(target_os = "linux", test))]
const DRIVER_TIMEOUT_FLOOR_US: u128 = 4_500_000;
#[cfg(target_os = "linux")]
const DRIVER_COMPLETION_WAIT_TIMEOUT_MS: u64 = 5_000;
const SIXTY_FPS_BUDGET_US: u128 = 16_667;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BenchmarkConfig {
    waveform: MxcfbWaveform,
    frames: usize,
}

impl BenchmarkConfig {
    #[cfg(target_os = "linux")]
    pub(crate) fn from_env() -> Result<Self> {
        let waveform = optional_env(WAVEFORM_ENV)?;
        let frames = optional_env(FRAMES_ENV)?;
        Self::parse(waveform.as_deref(), frames.as_deref())
    }

    fn parse(waveform: Option<&str>, frames: Option<&str>) -> Result<Self> {
        let waveform = match waveform {
            None => DEFAULT_WAVEFORM,
            Some(name) => MxcfbWaveform::from_name(name).with_context(|| {
                format!("invalid {WAVEFORM_ENV}={name:?}; expected gc16, du, a2, or du4")
            })?,
        };
        let frames = match frames {
            None => DEFAULT_FRAMES,
            Some(value) => value
                .parse::<usize>()
                .with_context(|| format!("invalid {FRAMES_ENV}={value:?}"))?,
        };
        ensure!(
            (MIN_FRAMES..=MAX_FRAMES).contains(&frames),
            "{FRAMES_ENV} must be in {MIN_FRAMES}..={MAX_FRAMES}, got {frames}"
        );
        Ok(Self { waveform, frames })
    }
}

#[cfg(target_os = "linux")]
fn optional_env(name: &str) -> Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => bail!("{name} is not valid Unicode"),
    }
}

#[cfg(target_os = "linux")]
pub(crate) struct MxcfbBenchmark {
    opened: OpenedFramebuffer,
    markers: UpdateMarkerSequence,
    config: BenchmarkConfig,
    initial_screen: Option<Vec<u8>>,
    screen_modified: bool,
}

#[cfg(target_os = "linux")]
impl MxcfbBenchmark {
    pub(crate) fn open(config: BenchmarkConfig) -> Result<Self> {
        let opened = OpenedFramebuffer::open().context("initialize MXCFB benchmark")?;
        let initial_screen = opened
            .mapping
            .snapshot_visible()
            .context("snapshot framebuffer before MXCFB benchmark")?;
        eprintln!(
            "mxcfb benchmark completion mode=marker-specific ioctl=0xc008462f return_semantics=ambiguous snapshot_bytes={}",
            initial_screen.len()
        );
        eprintln!(
            "mxcfb benchmark configured waveform={} frames={} region={}x{} pattern=alternating-solid",
            config.waveform.name(),
            config.frames,
            opened.info.screen.remote_viewport.width,
            opened.info.screen.remote_viewport.height
        );
        Ok(Self {
            opened,
            markers: UpdateMarkerSequence::for_process(),
            config,
            initial_screen: Some(initial_screen),
            screen_modified: false,
        })
    }

    pub(crate) fn dimensions(&self) -> (u16, u16) {
        self.opened.info.screen.physical_size()
    }

    pub(crate) fn run(
        &mut self,
        mut exit_requested: impl FnMut() -> Result<bool>,
    ) -> Result<BenchmarkOutcome> {
        if exit_requested()? {
            return Ok(BenchmarkOutcome::Stopped { completed: 0 });
        }

        self.screen_modified = true;
        let system_ui = present_system_ui_with(
            &mut self.opened,
            &mut self.markers,
            false,
            controller::submit_update,
        )
        .context("MXCFB benchmark draw local system UI")?;
        if self
            .wait_for_marker(system_ui.marker, &mut exit_requested)
            .context("MXCFB benchmark wait for local system UI")?
            .is_none()
        {
            eprintln!("mxcfb benchmark stopped by local Exit completed=0 phase=system-ui");
            return Ok(BenchmarkOutcome::Stopped { completed: 0 });
        }

        let viewport = self.opened.info.screen.remote_viewport;
        let pixel_count = usize::from(viewport.width)
            .checked_mul(usize::from(viewport.height))
            .context("MXCFB benchmark frame size overflow")?;
        let black = solid_frame(pixel_count, 0x00)?;
        let white = solid_frame(pixel_count, 0xff)?;

        // Warm the controller and establish a known first transition. This
        // completion is logged separately and excluded from summary samples.
        let warmup = self.present_frame(1, &white)?;
        let Some(warmup_wait) = self.wait_for_marker(warmup.marker, &mut exit_requested)? else {
            eprintln!("mxcfb benchmark stopped by local Exit completed=0 phase=warmup");
            return Ok(BenchmarkOutcome::Stopped { completed: 0 });
        };
        eprintln!(
            "mxcfb benchmark warmup marker={} completion_wait_us={}",
            warmup.marker,
            warmup_wait.as_micros()
        );

        let mut samples = Vec::new();
        samples
            .try_reserve_exact(self.config.frames)
            .context("allocate MXCFB benchmark samples")?;

        for index in 0..self.config.frames {
            if exit_requested()? {
                eprintln!(
                    "mxcfb benchmark stopped by local Exit completed={}",
                    samples.len()
                );
                return Ok(BenchmarkOutcome::Stopped {
                    completed: samples.len(),
                });
            }

            let pixels = if index % 2 == 0 { &black } else { &white };
            let frame_started = Instant::now();
            let prepare_submit_started = Instant::now();
            let submitted = self.present_frame(index as u64 + 2, pixels)?;
            let prepare_write_submit = prepare_submit_started.elapsed();
            let Some(completion_wait) =
                self.wait_for_marker(submitted.marker, &mut exit_requested)?
            else {
                eprintln!(
                    "mxcfb benchmark stopped by local Exit completed={} phase=sample-wait",
                    samples.len()
                );
                return Ok(BenchmarkOutcome::Stopped {
                    completed: samples.len(),
                });
            };
            let total = frame_started.elapsed();
            let sample = BenchmarkSample {
                prepare_write_submit,
                completion_wait,
                total,
            };
            eprintln!(
                "mxcfb benchmark sample={}/{} marker={} prepare_write_submit_us={} completion_wait_us={} total_us={} within_60fps_budget={}",
                index + 1,
                self.config.frames,
                submitted.marker,
                sample.prepare_write_submit.as_micros(),
                sample.completion_wait.as_micros(),
                sample.total.as_micros(),
                sample.total.as_micros() <= SIXTY_FPS_BUDGET_US
            );
            samples.push(sample);
        }

        let summary = BenchmarkSummary::from_samples(&samples)?;
        eprintln!(
            "mxcfb benchmark summary waveform={} completed={} total_us={} end_to_end_fps_milli={} total_p50_us={} total_p95_us={} total_max_us={} software_60fps_budget_met={} visual_60fps_verified=false",
            self.config.waveform.name(),
            summary.completed,
            summary.total.as_micros(),
            summary.fps_milli,
            summary.p50_us,
            summary.p95_us,
            summary.max_us,
            summary.software_60fps_budget_met
        );
        Ok(BenchmarkOutcome::Completed)
    }

    fn present_frame(
        &mut self,
        frame_id: u64,
        pixels: &[u8],
    ) -> Result<super::present::SubmittedUpdate> {
        let viewport = self.opened.info.screen.remote_viewport;
        let frame = RemoteFrame::new(
            frame_id,
            viewport.width,
            viewport.height,
            PixelFormat::Gray8,
            usize::from(viewport.width),
            pixels,
            0,
        )
        .context("construct MXCFB benchmark frame")?;
        present_remote_with_waveform(
            &mut self.opened,
            &mut self.markers,
            frame,
            self.config.waveform,
            controller::submit_update,
        )
        .context("present MXCFB benchmark frame")
    }

    pub(crate) fn restore_initial_screen(&mut self) -> Result<()> {
        if !self.screen_modified {
            return Ok(());
        }
        let snapshot = self
            .initial_screen
            .as_deref()
            .context("MXCFB benchmark initial framebuffer snapshot is missing")?;
        self.opened
            .mapping
            .restore_visible(snapshot)
            .context("restore pre-benchmark framebuffer bytes")?;

        let spec = self.opened.info.spec;
        let mut request = next_full_screen_gc16_request(
            spec.visible_width,
            spec.visible_height,
            &mut self.markers,
        )
        .context("prepare full-screen MXCFB restoration refresh")?;
        let marker = request.update_marker;
        controller::submit_update(
            &self.opened.file,
            &mut request,
            spec.visible_width,
            spec.visible_height,
        )
        .context("submit full-screen MXCFB restoration refresh")?;
        let wait_started = Instant::now();
        controller::wait_for_update_complete(&self.opened.file, marker)
            .context("wait for full-screen MXCFB restoration refresh")?;
        let wait_elapsed = wait_started.elapsed();
        eprintln!(
            "mxcfb benchmark cleanup restored_snapshot_bytes={} marker={} wait_us={} classification={} ioctl_completion_proven=false visual_completion_verified=false",
            snapshot.len(),
            marker,
            wait_elapsed.as_micros(),
            marker_wait_classification(wait_elapsed).label()
        );
        self.screen_modified = false;
        self.initial_screen = None;
        Ok(())
    }

    fn wait_for_marker(
        &self,
        expected: u32,
        exit_requested: &mut impl FnMut() -> Result<bool>,
    ) -> Result<Option<Duration>> {
        let started = Instant::now();
        for attempt in 1..=MAX_COMPLETION_WAITS {
            if exit_requested().context("poll local Exit while waiting for MXCFB completion")? {
                return Ok(None);
            }
            let wait_started = Instant::now();
            controller::wait_for_update_complete(&self.opened.file, expected)
                .with_context(|| format!("wait for MXCFB marker {expected}"))?;
            let wait_elapsed = wait_started.elapsed();
            let classification = marker_wait_classification(wait_elapsed);
            eprintln!(
                "mxcfb benchmark marker-specific wait marker={expected} elapsed_us={} classification={} ioctl_completion_proven=false visual_completion_verified=false",
                wait_elapsed.as_micros(),
                classification.label()
            );
            match classification {
                MarkerWaitClassification::Attributable => {}
                MarkerWaitClassification::ImmediateOrMissing => bail!(
                    "MXCFB marker {expected} wait returned too quickly to attribute ({} us)",
                    wait_elapsed.as_micros()
                ),
                MarkerWaitClassification::DriverTimeout => bail!(
                    "MXCFB marker {expected} wait reached the ambiguous driver-timeout range ({} us)",
                    wait_elapsed.as_micros()
                ),
            }
            let completed = [expected];
            let decision = completion_batch_decision(expected, &completed, attempt);
            if decision == CompletionBatchDecision::Completed {
                return Ok(Some(started.elapsed()));
            }
            let will_retry = decision == CompletionBatchDecision::Retry;
            if completed.is_empty() {
                eprintln!(
                    "mxcfb benchmark completion wait timed out marker={expected} attempt={attempt}/{MAX_COMPLETION_WAITS} driver_wait_ms={DRIVER_COMPLETION_WAIT_TIMEOUT_MS} will_retry={will_retry}"
                );
            } else {
                eprintln!(
                    "mxcfb benchmark completion batch omitted marker={expected} attempt={attempt}/{MAX_COMPLETION_WAITS} completed={completed:?} will_retry={will_retry}"
                );
            }
            if decision == CompletionBatchDecision::Exhausted {
                break;
            }
        }
        bail!(
            "MXCFB marker {expected} absent after {MAX_COMPLETION_WAITS} bounded completion waits (up to {} ms)",
            DRIVER_COMPLETION_WAIT_TIMEOUT_MS * MAX_COMPLETION_WAITS as u64
        )
    }
}

#[cfg(target_os = "linux")]
impl Drop for MxcfbBenchmark {
    fn drop(&mut self) {
        if let Err(error) = self.restore_initial_screen() {
            eprintln!("mxcfb benchmark cleanup failed during drop: {error:#}");
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompletionBatchDecision {
    Completed,
    Retry,
    Exhausted,
}

#[cfg(target_os = "linux")]
fn completion_batch_decision(
    expected: u32,
    completed: &[u32],
    attempt: usize,
) -> CompletionBatchDecision {
    if completed.contains(&expected) {
        CompletionBatchDecision::Completed
    } else if attempt < MAX_COMPLETION_WAITS {
        CompletionBatchDecision::Retry
    } else {
        CompletionBatchDecision::Exhausted
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MarkerWaitClassification {
    Attributable,
    ImmediateOrMissing,
    DriverTimeout,
}

#[cfg(target_os = "linux")]
impl MarkerWaitClassification {
    const fn label(self) -> &'static str {
        match self {
            Self::Attributable => "attributable-kernel-wait",
            Self::ImmediateOrMissing => "ambiguous-immediate-or-missing",
            Self::DriverTimeout => "ambiguous-driver-timeout",
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn marker_wait_classification(elapsed: Duration) -> MarkerWaitClassification {
    if elapsed.as_micros() < MIN_ATTRIBUTABLE_WAIT_US {
        MarkerWaitClassification::ImmediateOrMissing
    } else if elapsed.as_micros() >= DRIVER_TIMEOUT_FLOOR_US {
        MarkerWaitClassification::DriverTimeout
    } else {
        MarkerWaitClassification::Attributable
    }
}

#[cfg(target_os = "linux")]
fn solid_frame(pixel_count: usize, value: u8) -> Result<Vec<u8>> {
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(pixel_count)
        .context("allocate MXCFB benchmark pixels")?;
    pixels.resize(pixel_count, value);
    Ok(pixels)
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct BenchmarkSample {
    prepare_write_submit: Duration,
    completion_wait: Duration,
    total: Duration,
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) struct BenchmarkSummary {
    completed: usize,
    total: Duration,
    fps_milli: u128,
    p50_us: u128,
    p95_us: u128,
    max_us: u128,
    software_60fps_budget_met: bool,
}

impl BenchmarkSummary {
    fn from_samples(samples: &[BenchmarkSample]) -> Result<Self> {
        ensure!(!samples.is_empty(), "MXCFB benchmark produced no samples");
        let mut total_us: Vec<u128> = samples
            .iter()
            .map(|sample| sample.total.as_micros())
            .collect();
        total_us.sort_unstable();
        let summed_us = total_us.iter().copied().sum::<u128>();
        ensure!(summed_us > 0, "MXCFB benchmark duration rounded to zero");
        let completed = samples.len();
        let fps_milli = (completed as u128)
            .checked_mul(1_000_000_000)
            .context("MXCFB benchmark FPS overflow")?
            / summed_us;
        let max_us = *total_us.last().expect("samples checked nonempty");
        Ok(Self {
            completed,
            total: Duration::from_micros(
                u64::try_from(summed_us).context("MXCFB benchmark duration overflow")?,
            ),
            fps_milli,
            p50_us: percentile(&total_us, 50),
            p95_us: percentile(&total_us, 95),
            max_us,
            software_60fps_budget_met: fps_milli >= 60_000 && max_us <= SIXTY_FPS_BUDGET_US,
        })
    }
}

#[derive(Clone, Copy, Debug)]
#[cfg(target_os = "linux")]
pub(crate) enum BenchmarkOutcome {
    Completed,
    Stopped { completed: usize },
}

fn percentile(sorted: &[u128], percent: usize) -> u128 {
    debug_assert!(!sorted.is_empty());
    debug_assert!((1..=100).contains(&percent));
    let rank = sorted.len().saturating_mul(percent).div_ceil(100);
    sorted[rank.saturating_sub(1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_are_bounded_and_conservative() {
        assert_eq!(
            BenchmarkConfig::parse(None, None).unwrap(),
            BenchmarkConfig {
                waveform: MxcfbWaveform::Gc16,
                frames: 8,
            }
        );
    }

    #[test]
    fn config_accepts_supported_modes_and_rejects_bad_input() {
        for (name, expected) in [
            ("gc16", MxcfbWaveform::Gc16),
            ("du", MxcfbWaveform::Du),
            ("a2", MxcfbWaveform::A2),
            ("du4", MxcfbWaveform::Du4),
        ] {
            assert_eq!(
                BenchmarkConfig::parse(Some(name), Some("12")).unwrap(),
                BenchmarkConfig {
                    waveform: expected,
                    frames: 12,
                }
            );
        }
        assert!(BenchmarkConfig::parse(Some("auto"), Some("12")).is_err());
        assert!(BenchmarkConfig::parse(Some("gc16"), Some("1")).is_err());
        assert!(BenchmarkConfig::parse(Some("gc16"), Some("121")).is_err());
        assert!(BenchmarkConfig::parse(Some("gc16"), Some("many")).is_err());
    }

    #[test]
    fn summary_requires_every_sample_to_meet_sixty_fps_budget() {
        let fast = BenchmarkSample {
            prepare_write_submit: Duration::from_micros(2_000),
            completion_wait: Duration::from_micros(10_000),
            total: Duration::from_micros(12_000),
        };
        let summary = BenchmarkSummary::from_samples(&[fast, fast]).unwrap();
        assert_eq!(summary.fps_milli, 83_333);
        assert!(summary.software_60fps_budget_met);

        let slow = BenchmarkSample {
            total: Duration::from_micros(20_000),
            ..fast
        };
        let summary = BenchmarkSummary::from_samples(&[fast, slow]).unwrap();
        assert!(!summary.software_60fps_budget_met);
        assert_eq!(summary.p50_us, 12_000);
        assert_eq!(summary.p95_us, 20_000);
    }

    #[test]
    fn marker_specific_wait_classifies_unattributable_boundaries() {
        assert_eq!(
            marker_wait_classification(Duration::from_micros(999)),
            MarkerWaitClassification::ImmediateOrMissing
        );
        assert_eq!(
            marker_wait_classification(Duration::from_micros(1_000)),
            MarkerWaitClassification::Attributable
        );
        assert_eq!(
            marker_wait_classification(Duration::from_millis(100)),
            MarkerWaitClassification::Attributable
        );
        assert_eq!(
            marker_wait_classification(Duration::from_micros(4_499_999)),
            MarkerWaitClassification::Attributable
        );
        assert_eq!(
            marker_wait_classification(Duration::from_millis(4_500)),
            MarkerWaitClassification::DriverTimeout
        );
    }
}
