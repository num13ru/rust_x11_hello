//! Read-only evdev capability inspection.
//!
//! This diagnostic never reads event records and never requests `EVIOCGRAB`.

// Host tests compile the Linux UAPI helpers on non-Linux systems, where the
// device-walking entry point is intentionally unreachable.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use anyhow::{Context, Result, bail};
use std::fs::{self, File};
use std::io;
#[cfg(target_os = "linux")]
use std::io::Read;
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

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

const SYN_REPORT: u16 = 0x00;
const SYN_CONFIG: u16 = 0x01;
const SYN_MT_REPORT: u16 = 0x02;
const SYN_DROPPED: u16 = 0x03;

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

#[cfg(target_os = "linux")]
const TRACE_DURATION: Duration = Duration::from_secs(15);
#[cfg(target_os = "linux")]
const MAX_TRACE_EVENTS: usize = 4096;

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

struct CapabilityBits {
    event_types: Vec<u8>,
    keys: Vec<u8>,
    absolute: Vec<u8>,
    direct: bool,
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
    let capabilities = read_capabilities(&file, path)?;
    let protocols = touch_protocols(
        &capabilities.event_types,
        &capabilities.keys,
        &capabilities.absolute,
    );
    eprintln!(
        "input probe: device path={} name={name:?} id={:04x}:{:04x}:{:04x}:{:04x} event_types={} properties={} btn_touch={} btn_tool_finger={} touch_candidate={} protocol={}",
        path.display(),
        id.bustype,
        id.vendor,
        id.product,
        id.version,
        format_event_types(&capabilities.event_types),
        if capabilities.direct {
            "direct"
        } else {
            "not-direct-or-unknown"
        },
        bit_is_set(&capabilities.keys, BTN_TOUCH),
        bit_is_set(&capabilities.keys, BTN_TOOL_FINGER),
        protocols.is_candidate(),
        protocols.label()
    );

    for code in 0..=ABS_MAX {
        if bit_is_set(&capabilities.absolute, code) {
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

fn read_capabilities(file: &File, path: &Path) -> Result<CapabilityBits> {
    let event_types = read_bitmap(file, eviocgbit(0, bitmap_len(EV_MAX)), EV_MAX)
        .context("EVIOCGBIT event types")?;
    let keys = if bit_is_set(&event_types, EV_KEY) {
        read_bitmap(file, eviocgbit(EV_KEY, bitmap_len(KEY_MAX)), KEY_MAX)
            .context("EVIOCGBIT keys")?
    } else {
        vec![0; bitmap_len(KEY_MAX)]
    };
    let absolute = if bit_is_set(&event_types, EV_ABS) {
        read_bitmap(file, eviocgbit(EV_ABS, bitmap_len(ABS_MAX)), ABS_MAX)
            .context("EVIOCGBIT absolute axes")?
    } else {
        vec![0; bitmap_len(ABS_MAX)]
    };
    let direct = match read_bitmap(file, eviocgprop(bitmap_len(INPUT_PROP_MAX)), INPUT_PROP_MAX) {
        Ok(properties) => bit_is_set(&properties, INPUT_PROP_DIRECT),
        Err(error) => {
            eprintln!(
                "input probe: device path={} property_error={error}",
                path.display()
            );
            false
        }
    };
    Ok(CapabilityBits {
        event_types,
        keys,
        absolute,
        direct,
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn trace_input() -> Result<()> {
    let (path, name) = select_touch_device()?;
    eprintln!(
        "input trace: start path={} name={name:?} duration_ms={} max_events={} eviocgrab=false",
        path.display(),
        TRACE_DURATION.as_millis(),
        MAX_TRACE_EVENTS
    );
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&path)
        .with_context(|| format!("open {} for nonblocking event trace", path.display()))?;
    let event_size = std::mem::size_of::<libc::input_event>();
    let mut bytes = vec![0u8; event_size * 64];
    let started = Instant::now();
    let deadline = started + TRACE_DURATION;
    let mut sequence = 0usize;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout_ms = remaining.as_millis().clamp(1, 250) as libc::c_int;
        let mut descriptor = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `descriptor` is a valid writable pollfd for one open file.
        let poll_result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if poll_result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("poll touchscreen event device");
        }
        if descriptor.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            bail!(
                "input trace: device poll failed revents=0x{:x}",
                descriptor.revents
            );
        }
        if descriptor.revents & libc::POLLIN == 0 {
            continue;
        }

        let read = match file.read(&mut bytes) {
            Ok(0) => bail!("input trace: event device reached EOF or disappeared"),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("read touchscreen event records"),
        };
        if read % event_size != 0 {
            bail!("input trace: read {read} bytes not divisible by input_event size {event_size}");
        }
        for chunk in bytes[..read].chunks_exact(event_size) {
            if sequence == MAX_TRACE_EVENTS {
                bail!("input trace: event limit reached; trace is incomplete");
            }
            // SAFETY: `chunk` has exactly `size_of::<input_event>()` readable
            // bytes. `read_unaligned` avoids assuming the byte buffer alignment.
            let event: libc::input_event =
                unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast()) };
            eprintln!(
                "input trace: seq={sequence} elapsed_us={} type={} code={} value={}",
                started.elapsed().as_micros(),
                event_type_name(event.type_),
                event_code_name(event.type_, event.code),
                event.value
            );
            sequence += 1;
        }
    }

    if sequence == 0 {
        bail!("input trace: no event records observed during bounded trace");
    }
    eprintln!(
        "input trace: complete events={sequence} duration_ms={} eviocgrab=false",
        TRACE_DURATION.as_millis()
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn select_touch_device() -> Result<(PathBuf, String)> {
    let mut paths = event_paths()?;
    paths.sort_by_key(|path| event_number(path).unwrap_or(u32::MAX));
    let mut candidates = Vec::new();
    for path in paths {
        let file = File::open(&path)
            .with_context(|| format!("open {} while selecting touchscreen", path.display()))?;
        let name = read_string(&file, eviocgname(256), 256)
            .with_context(|| format!("EVIOCGNAME {}", path.display()))?;
        let capabilities = read_capabilities(&file, &path)?;
        let protocols = touch_protocols(
            &capabilities.event_types,
            &capabilities.keys,
            &capabilities.absolute,
        );
        if protocols.is_candidate() && capabilities.direct {
            candidates.push((path, name));
        }
    }
    match candidates.len() {
        1 => Ok(candidates.pop().expect("one candidate")),
        0 => bail!("input trace: no readable direct touchscreen candidate found"),
        count => bail!("input trace: {count} direct touchscreen candidates are ambiguous"),
    }
}

#[cfg(target_os = "linux")]
fn event_type_name(event_type: u16) -> String {
    match event_type {
        EV_SYN => "EV_SYN".to_owned(),
        EV_KEY => "EV_KEY".to_owned(),
        EV_REL => "EV_REL".to_owned(),
        EV_ABS => "EV_ABS".to_owned(),
        EV_MSC => "EV_MSC".to_owned(),
        EV_SW => "EV_SW".to_owned(),
        _ => format!("0x{event_type:04x}"),
    }
}

#[cfg(target_os = "linux")]
fn event_code_name(event_type: u16, code: u16) -> String {
    let name = match (event_type, code) {
        (EV_SYN, SYN_REPORT) => Some("SYN_REPORT"),
        (EV_SYN, SYN_CONFIG) => Some("SYN_CONFIG"),
        (EV_SYN, SYN_MT_REPORT) => Some("SYN_MT_REPORT"),
        (EV_SYN, SYN_DROPPED) => Some("SYN_DROPPED"),
        (EV_KEY, BTN_TOOL_FINGER) => Some("BTN_TOOL_FINGER"),
        (EV_KEY, BTN_TOUCH) => Some("BTN_TOUCH"),
        (EV_ABS, ABS_X) => Some("ABS_X"),
        (EV_ABS, ABS_Y) => Some("ABS_Y"),
        (EV_ABS, ABS_MT_SLOT) => Some("ABS_MT_SLOT"),
        (EV_ABS, 0x30) => Some("ABS_MT_TOUCH_MAJOR"),
        (EV_ABS, ABS_MT_POSITION_X) => Some("ABS_MT_POSITION_X"),
        (EV_ABS, ABS_MT_POSITION_Y) => Some("ABS_MT_POSITION_Y"),
        (EV_ABS, 0x37) => Some("ABS_MT_TOOL_TYPE"),
        (EV_ABS, ABS_MT_TRACKING_ID) => Some("ABS_MT_TRACKING_ID"),
        _ => None,
    };
    match name {
        Some(name) => format!("{name}(0x{code:04x})"),
        None => format!("0x{code:04x}"),
    }
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
