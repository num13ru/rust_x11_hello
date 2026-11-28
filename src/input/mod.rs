//! Native Linux touchscreen selection and event decoding.

#[cfg(any(target_os = "linux", test))]
mod device;
#[cfg(any(target_os = "linux", test))]
pub(crate) mod evdev;
