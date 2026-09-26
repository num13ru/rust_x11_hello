//! Thin read-only `/dev/fb0` access probe. No pixel bytes are modified.

use anyhow::{Context, Result, ensure};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

pub(super) fn check_read_only_access(file: &File, length: usize) -> Result<()> {
    ensure!(length > 0, "cannot map an empty framebuffer region");
    ensure!(
        length <= isize::MAX as usize,
        "framebuffer mapping exceeds pointer offset range"
    );
    // SAFETY: `file` remains open, the length was checked against kernel-reported
    // framebuffer memory, and this mapping grants no write access. The returned
    // pointer is accessed only within the mapped span below.
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

    // SAFETY: the nonempty mapping covers [0, length), so both addresses are
    // in bounds. Volatile reads ensure the probe really accesses both ends;
    // their values are neither interpreted nor logged. The device must honor
    // the kernel-reported framebuffer memory length used to bound `length`.
    unsafe {
        let bytes = address.cast::<u8>();
        std::ptr::read_volatile(bytes);
        std::ptr::read_volatile(bytes.add(length - 1));
    }

    // SAFETY: `address` came from a successful `mmap` with exactly `length`
    // bytes. Nothing uses it after this call.
    let result = unsafe { libc::munmap(address, length) };
    if result != 0 {
        return Err(io::Error::last_os_error()).context("munmap /dev/fb0 read-only");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_ends_of_a_read_only_mapping() {
        let file = File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
        let length = usize::try_from(file.metadata().unwrap().len()).unwrap();
        assert!(length > 1);
        check_read_only_access(&file, length).unwrap();
    }

    #[test]
    fn rejects_invalid_mapping_lengths() {
        let file = File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
        assert!(check_read_only_access(&file, 0).is_err());
        assert!(check_read_only_access(&file, isize::MAX as usize + 1).is_err());
    }
}
