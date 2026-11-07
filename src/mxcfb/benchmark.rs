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

use crate::ui::screen::ScreenRect;
#[cfg(target_os = "linux")]
use anyhow::bail;
use anyhow::{Context, Result, ensure};

#[cfg(target_os = "linux")]
use super::controller;
#[cfg(target_os = "linux")]
use super::opened::OpenedFramebuffer;
#[cfg(target_os = "linux")]
use super::present::present_system_ui_with;
#[cfg(target_os = "linux")]
use super::update_marker::UpdateMarkerSequence;
use super::update_request::MxcfbWaveform;
#[cfg(target_os = "linux")]
use super::update_request::{next_benchmark_request, next_full_screen_gc16_request};

const WAVEFORM_ENV: &str = "PAPERPAD_MXCFB_BENCHMARK_WAVEFORM";
const FRAMES_ENV: &str = "PAPERPAD_MXCFB_BENCHMARK_FRAMES";
const REGION_ENV: &str = "PAPERPAD_MXCFB_BENCHMARK_REGION";
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
    region: BenchmarkRegion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BenchmarkRegion {
    Full,
    Size { width: u16, height: u16 },
}

impl BenchmarkRegion {
    fn parse(value: Option<&str>) -> Result<Self> {
        let Some(value) = value else {
            return Ok(Self::Full);
        };
        if value == "full" {
            return Ok(Self::Full);
        }

        let (width, height) = value.split_once('x').with_context(|| {
            format!("invalid {REGION_ENV}={value:?}; expected full or WIDTHxHEIGHT")
        })?;
        ensure!(
            !width.is_empty() && !height.is_empty() && !height.contains('x'),
            "invalid {REGION_ENV}={value:?}; expected full or WIDTHxHEIGHT"
        );
        let width = width
            .parse::<u16>()
            .with_context(|| format!("invalid {REGION_ENV} width in {value:?}"))?;
        let height = height
            .parse::<u16>()
            .with_context(|| format!("invalid {REGION_ENV} height in {value:?}"))?;
        ensure!(
            width > 0 && height > 0,
            "{REGION_ENV} dimensions must be nonzero"
        );
        Ok(Self::Size { width, height })
    }

    fn resolve(self, viewport: ScreenRect) -> Result<ScreenRect> {
        let (width, height) = match self {
            Self::Full => (viewport.width, viewport.height),
            Self::Size { width, height } => (width, height),
        };
        ensure!(
            width <= viewport.width && height <= viewport.height,
            "{REGION_ENV} {width}x{height} exceeds remote viewport {}x{}",
            viewport.width,
            viewport.height
        );
        let x = viewport
            .x
            .checked_add((viewport.width - width) / 2)
            .context("MXCFB benchmark region x overflow")?;
        let y = viewport
            .y
            .checked_add((viewport.height - height) / 2)
            .context("MXCFB benchmark region y overflow")?;
        Ok(ScreenRect {
            x,
            y,
            width,
            height,
        })
    }
}

impl BenchmarkConfig {
    #[cfg(target_os = "linux")]
    pub(crate) fn from_env() -> Result<Self> {
        let waveform = optional_env(WAVEFORM_ENV)?;
        let frames = optional_env(FRAMES_ENV)?;
        let region = optional_env(REGION_ENV)?;
        Self::parse(waveform.as_deref(), frames.as_deref(), region.as_deref())
    }

    fn parse(waveform: Option<&str>, frames: Option<&str>, region: Option<&str>) -> Result<Self> {
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
        Ok(Self {
            waveform,
            frames,
            region: BenchmarkRegion::parse(region)?,
        })
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
    region: ScreenRect,
    initial_screen: Option<Vec<u8>>,
    screen_modified: bool,
}

#[cfg(target_os = "linux")]
impl MxcfbBenchmark {
    pub(crate) fn open(config: BenchmarkConfig) -> Result<Self> {
        let opened = OpenedFramebuffer::open().context("initialize MXCFB benchmark")?;
        let region = config
            .region
            .resolve(opened.info.screen.remote_viewport)
            .context("validate MXCFB benchmark region")?;
        let initial_screen = opened
            .mapping
            .snapshot_visible()
            .context("snapshot framebuffer before MXCFB benchmark")?;
        eprintln!(
            "mxcfb benchmark completion mode=marker-specific ioctl=0xc008462f return_semantics=ambiguous snapshot_bytes={}",
            initial_screen.len()
        );
        eprintln!(
            "mxcfb benchmark configured waveform={} frames={} region={}x{}+{}+{} pattern=alternating-solid",
            config.waveform.name(),
            config.frames,
            region.width,
            region.height,
            region.x,
            region.y
        );
        Ok(Self {
            opened,
            markers: UpdateMarkerSequence::for_process(),
            config,
            region,
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

        // Warm the controller and establish a known first transition. This
        // completion is logged separately and excluded from summary samples.
        let warmup_marker = self.present_frame(0xff)?;
        let Some(warmup_wait) = self.wait_for_marker(warmup_marker, &mut exit_requested)? else {
            eprintln!("mxcfb benchmark stopped by local Exit completed=0 phase=warmup");
            return Ok(BenchmarkOutcome::Stopped { completed: 0 });
        };
        eprintln!(
            "mxcfb benchmark warmup marker={} completion_wait_us={}",
            warmup_marker,
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

            let value = if index % 2 == 0 { 0x00 } else { 0xff };
            let frame_started = Instant::now();
            let prepare_submit_started = Instant::now();
            let marker = self.present_frame(value)?;
            let prepare_write_submit = prepare_submit_started.elapsed();
            let Some(completion_wait) = self.wait_for_marker(marker, &mut exit_requested)? else {
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
                marker,
                sample.prepare_write_submit.as_micros(),
                sample.completion_wait.as_micros(),
                sample.total.as_micros(),
                sample.total.as_micros() <= SIXTY_FPS_BUDGET_US
            );
            samples.push(sample);
        }

        let summary = BenchmarkSummary::from_samples(&samples)?;
        eprintln!(
            "mxcfb benchmark summary waveform={} region={}x{}+{}+{} completed={} total_us={} end_to_end_fps_milli={} total_p50_us={} total_p95_us={} total_max_us={} software_60fps_budget_met={} visual_60fps_verified=false",
            self.config.waveform.name(),
            self.region.width,
            self.region.height,
            self.region.x,
            self.region.y,
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

    fn present_frame(&mut self, value: u8) -> Result<u32> {
        let info = &self.opened.info;
        let mut request = next_benchmark_request(
            info.screen,
            self.region,
            info.spec.visible_width,
            info.spec.visible_height,
            &mut self.markers,
            self.config.waveform,
        )
        .context("prepare MXCFB benchmark update")?;
        self.opened
            .mapping
            .fill_solid_region(self.region, value, info.spec)
            .context("write MXCFB benchmark region")?;
        let marker = request.update_marker;
        controller::submit_update(
            &self.opened.file,
            &mut request,
            info.spec.visible_width,
            info.spec.visible_height,
        )
        .context("submit MXCFB benchmark update")?;
        Ok(marker)
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
            BenchmarkConfig::parse(None, None, None).unwrap(),
            BenchmarkConfig {
                waveform: MxcfbWaveform::Gc16,
                frames: 8,
                region: BenchmarkRegion::Full,
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
                BenchmarkConfig::parse(Some(name), Some("12"), Some("256x128")).unwrap(),
                BenchmarkConfig {
                    waveform: expected,
                    frames: 12,
                    region: BenchmarkRegion::Size {
                        width: 256,
                        height: 128,
                    },
                }
            );
        }
        assert!(BenchmarkConfig::parse(Some("auto"), Some("12"), None).is_err());
        assert!(BenchmarkConfig::parse(Some("gc16"), Some("1"), None).is_err());
        assert!(BenchmarkConfig::parse(Some("gc16"), Some("121"), None).is_err());
        assert!(BenchmarkConfig::parse(Some("gc16"), Some("many"), None).is_err());
        for bad_region in ["", "0x1", "1x0", "1", "1x2x3", "65536x1"] {
            assert!(BenchmarkConfig::parse(Some("a2"), Some("12"), Some(bad_region)).is_err());
        }
    }

    #[test]
    fn benchmark_region_is_centered_and_bounded_by_remote_viewport() {
        let viewport = ScreenRect {
            x: 0,
            y: 0,
            width: 1272,
            height: 1624,
        };
        assert_eq!(BenchmarkRegion::Full.resolve(viewport).unwrap(), viewport);
        assert_eq!(
            BenchmarkRegion::Size {
                width: 256,
                height: 128,
            }
            .resolve(viewport)
            .unwrap(),
            ScreenRect {
                x: 508,
                y: 748,
                width: 256,
                height: 128,
            }
        );
        assert!(
            BenchmarkRegion::Size {
                width: 1273,
                height: 1,
            }
            .resolve(viewport)
            .is_err()
        );
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
