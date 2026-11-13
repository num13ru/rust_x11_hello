//! Read-only evdev capability inspection.
//!
//! This diagnostic never reads event records and never requests `EVIOCGRAB`.

// Host tests compile the Linux UAPI helpers on non-Linux systems, where the
// device-walking entry point is intentionally unreachable.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use anyhow::{Context, Result, bail};
use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

const EVDEV_IOCTL_TYPE: u32 = b'E' as u32;
const IOC_READ: u32 = 2;
const IOC_TYPE_SHIFT: u32 = 8;
const IOC_SIZE_SHIFT: u32 = 16;
const IOC_DIR_SHIFT: u32 = 30;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;
const EV_MSC: u16 = 0x04;
const EV_SW: u16 = 0x05;
const EV_MAX: u16 = 0x1f;

const BTN_TOOL_FINGER: u16 = 0x145;
const BTN_TOUCH: u16 = 0x14a;
const KEY_MAX: u16 = 0x2ff;

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const ABS_MAX: u16 = 0x3f;

const INPUT_PROP_DIRECT: u16 = 0x01;
const INPUT_PROP_MAX: u16 = 0x1f;

// libc follows each target C ABI: musl's ioctl request is a signed int,
// while Darwin and glibc expose an unsigned long request.
#[cfg(all(target_os = "linux", target_env = "musl"))]
type IoctlRequest = libc::c_int;
#[cfg(not(all(target_os = "linux", target_env = "musl")))]
type IoctlRequest = libc::c_ulong;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct InputAbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

const _: () = {
    assert!(std::mem::size_of::<InputId>() == 8);
    assert!(std::mem::size_of::<InputAbsInfo>() == 24);
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TouchProtocols {
    legacy: bool,
    multitouch: bool,
}

impl TouchProtocols {
    fn is_candidate(self) -> bool {
        self.legacy || self.multitouch
    }

    fn label(self) -> &'static str {
        match (self.legacy, self.multitouch) {
            (true, true) => "legacy+multitouch",
            (true, false) => "legacy",
            (false, true) => "multitouch",
            (false, false) => "none",
        }
    }
}

pub(crate) fn inspect_input() -> Result<()> {
    eprintln!("input probe: read-only /dev/input/event* capability inspection");
    eprintln!("input probe: event reads=false eviocgrab=false");

    let mut paths = event_paths()?;
    paths.sort_by_key(|path| event_number(path).unwrap_or(u32::MAX));
    if paths.is_empty() {
        bail!("input probe: no /dev/input/event* device nodes found");
    }

    let mut readable = 0usize;
    let mut candidates = 0usize;
    for path in &paths {
        match inspect_device(path) {
            Ok(protocols) => {
                readable += 1;
                candidates += usize::from(protocols.is_candidate());
            }
            Err(error) => {
                eprintln!(
                    "input probe: device path={} error={error:#}",
                    path.display()
                );
            }
        }
    }

    eprintln!(
        "input probe: summary nodes={} readable={} touch_candidates={}",
        paths.len(),
        readable,
        candidates
    );
    if readable == 0 {
        bail!("input probe: no event device allowed read-only capability inspection");
    }
    if candidates == 0 {
        bail!(
            "input probe: no readable device exposed a supported legacy or multitouch capability set"
        );
    }
    Ok(())
}

fn event_paths() -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir("/dev/input").context("input probe: read /dev/input")?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.context("input probe: read /dev/input directory entry")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            eprintln!("input probe: ignored non-UTF-8 device node name");
            continue;
        };
        if name.strip_prefix("event").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

fn event_number(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_prefix("event")?
        .parse()
        .ok()
}

fn inspect_device(path: &Path) -> Result<TouchProtocols> {
    let file = File::open(path).with_context(|| format!("open {} read-only", path.display()))?;
    let name = read_string(&file, eviocgname(256), 256).context("EVIOCGNAME")?;
    let id = read_value::<InputId>(&file, eviocgid()).context("EVIOCGID")?;
    let event_types = read_bitmap(&file, eviocgbit(0, bitmap_len(EV_MAX)), EV_MAX)
        .context("EVIOCGBIT event types")?;

    let keys = if bit_is_set(&event_types, EV_KEY) {
        read_bitmap(&file, eviocgbit(EV_KEY, bitmap_len(KEY_MAX)), KEY_MAX)
            .context("EVIOCGBIT keys")?
    } else {
        vec![0; bitmap_len(KEY_MAX)]
    };
    let absolute = if bit_is_set(&event_types, EV_ABS) {
        read_bitmap(&file, eviocgbit(EV_ABS, bitmap_len(ABS_MAX)), ABS_MAX)
            .context("EVIOCGBIT absolute axes")?
    } else {
        vec![0; bitmap_len(ABS_MAX)]
    };
    let properties = match read_bitmap(
        &file,
        eviocgprop(bitmap_len(INPUT_PROP_MAX)),
        INPUT_PROP_MAX,
    ) {
        Ok(properties) => Some(properties),
        Err(error) => {
            eprintln!(
                "input probe: device path={} property_error={error}",
                path.display()
            );
            None
        }
    };

    let protocols = touch_protocols(&event_types, &keys, &absolute);
    let direct = properties
        .as_deref()
        .is_some_and(|bits| bit_is_set(bits, INPUT_PROP_DIRECT));
    eprintln!(
        "input probe: device path={} name={name:?} id={:04x}:{:04x}:{:04x}:{:04x} event_types={} properties={} btn_touch={} btn_tool_finger={} touch_candidate={} protocol={}",
        path.display(),
        id.bustype,
        id.vendor,
        id.product,
        id.version,
        format_event_types(&event_types),
        if direct {
            "direct"
        } else {
            "not-direct-or-unknown"
        },
        bit_is_set(&keys, BTN_TOUCH),
        bit_is_set(&keys, BTN_TOOL_FINGER),
        protocols.is_candidate(),
        protocols.label()
    );

    for code in 0..=ABS_MAX {
        if bit_is_set(&absolute, code) {
            let info = read_value::<InputAbsInfo>(&file, eviocgabs(code))
                .with_context(|| format!("EVIOCGABS code=0x{code:02x}"))?;
            eprintln!(
                "input probe: device path={} axis={} code=0x{code:02x} value={} min={} max={} fuzz={} flat={} resolution={}",
                path.display(),
                absolute_axis_name(code),
                info.value,
                info.minimum,
                info.maximum,
                info.fuzz,
                info.flat,
                info.resolution
            );
        }
    }

    Ok(protocols)
}

fn touch_protocols(event_types: &[u8], keys: &[u8], absolute: &[u8]) -> TouchProtocols {
    if !bit_is_set(event_types, EV_ABS) {
        return TouchProtocols {
            legacy: false,
            multitouch: false,
        };
    }
    TouchProtocols {
        legacy: bit_is_set(absolute, ABS_X)
            && bit_is_set(absolute, ABS_Y)
            && (bit_is_set(keys, BTN_TOUCH) || bit_is_set(keys, BTN_TOOL_FINGER)),
        multitouch: bit_is_set(absolute, ABS_MT_POSITION_X)
            && bit_is_set(absolute, ABS_MT_POSITION_Y)
            && bit_is_set(absolute, ABS_MT_TRACKING_ID),
    }
}

fn format_event_types(bits: &[u8]) -> String {
    let mut labels = Vec::new();
    for code in 0..=EV_MAX {
        if bit_is_set(bits, code) {
            labels.push(match code {
                EV_SYN => "syn".to_owned(),
                EV_KEY => "key".to_owned(),
                EV_REL => "rel".to_owned(),
                EV_ABS => "abs".to_owned(),
                EV_MSC => "msc".to_owned(),
                EV_SW => "sw".to_owned(),
                _ => format!("0x{code:02x}"),
            });
        }
    }
    if labels.is_empty() {
        "none".to_owned()
    } else {
        labels.join(",")
    }
}

fn absolute_axis_name(code: u16) -> &'static str {
    match code {
        ABS_X => "ABS_X",
        ABS_Y => "ABS_Y",
        ABS_MT_SLOT => "ABS_MT_SLOT",
        ABS_MT_POSITION_X => "ABS_MT_POSITION_X",
        ABS_MT_POSITION_Y => "ABS_MT_POSITION_Y",
        ABS_MT_TRACKING_ID => "ABS_MT_TRACKING_ID",
        _ => "other",
    }
}

fn bitmap_len(maximum_code: u16) -> usize {
    usize::from(maximum_code) / 8 + 1
}

fn bit_is_set(bits: &[u8], code: u16) -> bool {
    let byte = usize::from(code) / 8;
    bits.get(byte)
        .is_some_and(|value| value & (1 << (code % 8)) != 0)
}

fn read_string(file: &File, request: IoctlRequest, length: usize) -> io::Result<String> {
    let mut bytes = vec![0u8; length];
    ioctl_read(file, request, bytes.as_mut_ptr().cast())?;
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

fn read_bitmap(file: &File, request: IoctlRequest, maximum_code: u16) -> io::Result<Vec<u8>> {
    let mut bytes = vec![0u8; bitmap_len(maximum_code)];
    ioctl_read(file, request, bytes.as_mut_ptr().cast())?;
    Ok(bytes)
}

fn read_value<T: Default>(file: &File, request: IoctlRequest) -> io::Result<T> {
    let mut value = T::default();
    ioctl_read(file, request, (&mut value as *mut T).cast())?;
    Ok(value)
}

fn ioctl_read(file: &File, request: IoctlRequest, output: *mut libc::c_void) -> io::Result<()> {
    // SAFETY: every caller supplies a writable buffer whose size is encoded in
    // `request`; `file` remains open for the duration of the metadata-only call.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request, output) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

const fn ioc_read(number: u32, size: usize) -> IoctlRequest {
    ((IOC_READ << IOC_DIR_SHIFT)
        | (EVDEV_IOCTL_TYPE << IOC_TYPE_SHIFT)
        | ((size as u32) << IOC_SIZE_SHIFT)
        | number) as IoctlRequest
}

const fn eviocgid() -> IoctlRequest {
    ioc_read(0x02, std::mem::size_of::<InputId>())
}

const fn eviocgname(length: usize) -> IoctlRequest {
    ioc_read(0x06, length)
}

const fn eviocgprop(length: usize) -> IoctlRequest {
    ioc_read(0x09, length)
}

const fn eviocgbit(event_type: u16, length: usize) -> IoctlRequest {
    ioc_read(0x20 + event_type as u32, length)
}

const fn eviocgabs(code: u16) -> IoctlRequest {
    ioc_read(0x40 + code as u32, std::mem::size_of::<InputAbsInfo>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(maximum: u16, codes: &[u16]) -> Vec<u8> {
        let mut bits = vec![0; bitmap_len(maximum)];
        for code in codes {
            bits[usize::from(*code) / 8] |= 1 << (*code % 8);
        }
        bits
    }

    #[test]
    fn ioctl_numbers_match_linux_input_uapi() {
        assert_eq!(eviocgid() as u32, 0x8008_4502);
        assert_eq!(eviocgname(256) as u32, 0x8100_4506);
        assert_eq!(eviocgabs(ABS_X) as u32, 0x8018_4540);
    }

    #[test]
    fn identifies_legacy_and_multitouch_capability_sets() {
        let event_types = bits(EV_MAX, &[EV_SYN, EV_KEY, EV_ABS]);
        let legacy = touch_protocols(
            &event_types,
            &bits(KEY_MAX, &[BTN_TOUCH]),
            &bits(ABS_MAX, &[ABS_X, ABS_Y]),
        );
        assert_eq!(
            legacy,
            TouchProtocols {
                legacy: true,
                multitouch: false
            }
        );

        let multitouch = touch_protocols(
            &event_types,
            &bits(KEY_MAX, &[]),
            &bits(
                ABS_MAX,
                &[ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_TRACKING_ID],
            ),
        );
        assert_eq!(
            multitouch,
            TouchProtocols {
                legacy: false,
                multitouch: true
            }
        );
    }

    #[test]
    fn rejects_pointer_like_devices_without_complete_touch_capabilities() {
        let event_types = bits(EV_MAX, &[EV_SYN, EV_KEY, EV_ABS]);
        let incomplete = touch_protocols(
            &event_types,
            &bits(KEY_MAX, &[BTN_TOUCH]),
            &bits(ABS_MAX, &[ABS_X]),
        );
        assert!(!incomplete.is_candidate());

        let no_absolute_events = touch_protocols(
            &bits(EV_MAX, &[EV_SYN, EV_KEY]),
            &bits(KEY_MAX, &[BTN_TOUCH]),
            &bits(ABS_MAX, &[ABS_X, ABS_Y]),
        );
        assert!(!no_absolute_events.is_candidate());
    }
}
