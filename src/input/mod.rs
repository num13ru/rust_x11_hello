//! Native Linux input diagnostics and, later, event decoding.

#[cfg(any(target_os = "linux", test))]
mod probe;

#[cfg(target_os = "linux")]
pub(crate) use probe::inspect_input;

#[cfg(not(target_os = "linux"))]
pub(crate) fn inspect_input() -> anyhow::Result<()> {
    anyhow::bail!("input inspection requires Linux and /dev/input/event*")
}
