//! Read-only device metadata collection before any framebuffer format or
//! Kindle-specific MXCFB update ABI is selected.

use anyhow::{Context, Result};
use std::fs::{self, File};

use super::{abi, layout, memory};

pub(crate) fn inspect_framebuffer() -> Result<()> {
    eprintln!("mxcfb probe: read-only /dev/fb0 inspection");
    log_optional_file("kernel", "/proc/sys/kernel/osrelease");
    log_optional_file("framebuffer drivers", "/proc/fb");

    let file = File::open("/dev/fb0").context("mxcfb probe: open /dev/fb0 for reading")?;
    let fixed = abi::fixed_info(&file).context("mxcfb probe: FBIOGET_FSCREENINFO")?;
    let variable = abi::variable_info(&file).context("mxcfb probe: FBIOGET_VSCREENINFO")?;

    let id_end = fixed
        .id
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(fixed.id.len());
    let id = String::from_utf8_lossy(&fixed.id[..id_end]);
    eprintln!(
        "mxcfb probe: fixed id={id:?} type={} visual={} line_length={} smem_len={} capabilities=0x{:x}",
        fixed.kind, fixed.visual, fixed.line_length, fixed.smem_len, fixed.capabilities
    );
    eprintln!(
        "mxcfb probe: visible={}x{} virtual={}x{} offset={},{} bpp={} grayscale={} nonstd={} rotate={}",
        variable.xres,
        variable.yres,
        variable.xres_virtual,
        variable.yres_virtual,
        variable.xoffset,
        variable.yoffset,
        variable.bits_per_pixel,
        variable.grayscale,
        variable.nonstd,
        variable.rotate
    );
    eprintln!(
        "mxcfb probe: red={:?} green={:?} blue={:?} alpha={:?}",
        variable.red, variable.green, variable.blue, variable.transp
    );

    let mapping_length = layout::visible_mapping_len(layout::FramebufferGeometry {
        xres: variable.xres,
        yres: variable.yres,
        xres_virtual: variable.xres_virtual,
        yres_virtual: variable.yres_virtual,
        xoffset: variable.xoffset,
        yoffset: variable.yoffset,
        bits_per_pixel: variable.bits_per_pixel,
        line_length: fixed.line_length,
        smem_len: fixed.smem_len,
    })
    .context("mxcfb probe: visible framebuffer mapping bounds")?;
    memory::check_read_only_access(&file, mapping_length)
        .context("mxcfb probe: read-only framebuffer access")?;
    eprintln!(
        "mxcfb probe: read-only mmap and boundary-byte reads accepted length={mapping_length}"
    );
    Ok(())
}

fn log_optional_file(label: &str, path: &str) {
    match fs::read_to_string(path) {
        Ok(value) => eprintln!("mxcfb probe: {label}={:?}", value.trim()),
        Err(error) => eprintln!("mxcfb probe: {label} unavailable: {error}"),
    }
}
