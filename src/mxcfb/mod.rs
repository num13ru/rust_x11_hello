//! Direct-framebuffer MXCFB display backend.

#[cfg(target_os = "linux")]
mod abi;
// The normal PaperPad runtime selects this backend directly on Linux.
#[cfg(target_os = "linux")]
mod backend;
#[cfg(target_os = "linux")]
pub(crate) use backend::MxcfbDisplayBackend;
// Update submission for the display backend.
#[cfg(target_os = "linux")]
mod controller;
#[cfg(any(target_os = "linux", test))]
mod exit_label_bitmap;
#[cfg(target_os = "linux")]
mod hwtcon;
#[cfg(any(target_os = "linux", test))]
mod layout;
// Bounded framebuffer writes serve remote frames and the local Exit strip.
#[cfg(target_os = "linux")]
mod mapped;
// Pure interpretation of kernel-reported framebuffer metadata.
#[cfg(target_os = "linux")]
mod metadata;
// Open and validate the framebuffer.
#[cfg(target_os = "linux")]
mod opened;
// Remote and local presentation are selected through the MXCFB backend.
#[cfg(target_os = "linux")]
mod present;
#[cfg(any(target_os = "linux", test))]
mod region;
// Local Exit pixels are prepared without touching /dev/fb0.
#[cfg(any(target_os = "linux", test))]
mod system_pixels;
#[cfg(any(target_os = "linux", test))]
mod update_request;
// One process-local marker sequence serves both update regions.
#[cfg(any(target_os = "linux", test))]
mod update_marker;
// Firmware-attributed layouts compile and update submission was accepted on a
// PW6; complete field semantics remain unverified.
mod update_abi;
// Pure Mono1/Gray8 conversion remains independent of framebuffer writes.
// On non-Linux hosts only its unit tests use these Linux presentation helpers.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod pixels;
#[cfg(any(target_os = "linux", test))]
mod waveform_policy;
