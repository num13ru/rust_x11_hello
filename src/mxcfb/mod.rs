//! Direct-framebuffer diagnostics and, eventually, the MXCFB display backend.

#[cfg(target_os = "linux")]
mod abi;
// Submission is staged but not connected to the display or probe path.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
mod controller;
#[cfg(target_os = "linux")]
mod hwtcon;
#[cfg(any(target_os = "linux", test))]
mod layout;
// Bounded framebuffer writes are staged, not yet connected to frame display.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
mod mapped;
#[cfg(any(target_os = "linux", test))]
mod memory;
// Pure interpretation of kernel-reported framebuffer metadata.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
mod metadata;
// Backend opening is staged but not used by startup or the probe.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
mod opened;
#[cfg(target_os = "linux")]
mod probe;
#[cfg(any(target_os = "linux", test))]
mod region;
#[cfg(any(target_os = "linux", test))]
mod update_request;
// Markers are staged independently of update submission.
#[cfg(any(target_os = "linux", test))]
#[allow(dead_code)]
mod update_marker;
// Source-derived update layouts are compiled but not used for device calls yet.
mod update_abi;
// Pure conversion is staged before the framebuffer writer is connected.
#[allow(dead_code)]
mod pixels;

#[cfg(target_os = "linux")]
pub(crate) use probe::inspect_framebuffer;

#[cfg(not(target_os = "linux"))]
pub(crate) fn inspect_framebuffer() -> anyhow::Result<()> {
    anyhow::bail!("MXCFB framebuffer inspection requires Linux and /dev/fb0")
}
