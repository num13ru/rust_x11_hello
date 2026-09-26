//! Direct-framebuffer diagnostics and, eventually, the MXCFB display backend.

#[cfg(target_os = "linux")]
mod abi;
#[cfg(target_os = "linux")]
mod probe;

#[cfg(target_os = "linux")]
pub(crate) use probe::inspect_framebuffer;

#[cfg(not(target_os = "linux"))]
pub(crate) fn inspect_framebuffer() -> anyhow::Result<()> {
    anyhow::bail!("MXCFB framebuffer inspection requires Linux and /dev/fb0")
}
