//! Thin read-only `/dev/fb0` mapping probe. No pixel bytes are accessed.

use anyhow::{Context, Result, ensure};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

pub(super) fn check_read_only_mapping(file: &File, length: usize) -> Result<()> {
    ensure!(length > 0, "cannot map an empty framebuffer region");
    // SAFETY: `file` remains open, the length was checked against kernel-reported
    // framebuffer memory, and this mapping grants no write access. The returned
    // pointer is never dereferenced; it is passed only to `munmap` below.
    let address = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            length,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if address == libc::MAP_FAILED {
        return Err(io::Error::last_os_error()).context("mmap /dev/fb0 read-only");
    }

    // SAFETY: `address` came from a successful `mmap` with exactly `length`
    // bytes. Nothing uses it after this call.
    let result = unsafe { libc::munmap(address, length) };
    if result != 0 {
        return Err(io::Error::last_os_error()).context("munmap /dev/fb0 read-only");
    }
    Ok(())
}
