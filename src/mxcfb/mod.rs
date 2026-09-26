//! Direct-framebuffer diagnostics and, eventually, the MXCFB display backend.

#[cfg(target_os = "linux")]
mod abi;
#[cfg(target_os = "linux")]
mod hwtcon;
#[cfg(any(target_os = "linux", test))]
mod layout;
#[cfg(any(target_os = "linux", test))]
mod memory;
#[cfg(target_os = "linux")]
mod probe;
// Pure conversion is staged before the framebuffer writer is connected.
#[allow(dead_code)]
mod pixels;

#[cfg(target_os = "linux")]
pub(crate) use probe::inspect_framebuffer;

#[cfg(not(target_os = "linux"))]
pub(crate) fn inspect_framebuffer() -> anyhow::Result<()> {
    anyhow::bail!("MXCFB framebuffer inspection requires Linux and /dev/fb0")
}
