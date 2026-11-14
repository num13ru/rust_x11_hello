//! Pure Linux evdev Type-B multitouch decoding.
//!
//! Descriptor I/O and capability discovery remain separate so recorded and
//! synthetic event sequences exercise the same state machine.

#![cfg_attr(not(test), allow(dead_code))]

use crate::ui::button::{PRIMARY_BUTTON_DETAIL, PointerEvent, PointerEventKind};
use crate::ui::screen::PhysicalPoint;
#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{Result, ensure};
#[cfg(target_os = "linux")]
use std::fs::{File, OpenOptions};
use std::io;
#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use super::probe::select_touch_device;

pub(crate) const EV_SYN: u16 = 0x00;
pub(crate) const EV_ABS: u16 = 0x03;
pub(crate) const SYN_REPORT: u16 = 0x00;
pub(crate) const SYN_DROPPED: u16 = 0x03;
pub(crate) const ABS_MT_SLOT: u16 = 0x2f;
pub(crate) const ABS_MT_POSITION_X: u16 = 0x35;
pub(crate) const ABS_MT_POSITION_Y: u16 = 0x36;
pub(crate) const ABS_MT_TRACKING_ID: u16 = 0x39;

#[cfg(target_os = "linux")]
const EVENTS_PER_READ: usize = 64;
#[cfg(target_os = "linux")]
const MAX_EVENTS_PER_DRAIN: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AxisRange {
    minimum: i32,
    maximum: i32,
}

impl AxisRange {
    pub(crate) fn new(minimum: i32, maximum: i32) -> Result<Self> {
        ensure!(
            minimum < maximum,
            "evdev axis range must increase: minimum={minimum} maximum={maximum}"
        );
        Ok(Self { minimum, maximum })
    }

    fn normalize(self, value: i32, extent: u16) -> i16 {
        let clamped = value.clamp(self.minimum, self.maximum);
        let offset = i64::from(clamped) - i64::from(self.minimum);
        let span = i64::from(self.maximum) - i64::from(self.minimum);
        let target_max = i64::from(extent - 1);
        let scaled = (offset * target_max + span / 2) / span;
        i16::try_from(scaled).expect("validated physical coordinate extent")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TouchGeometry {
    x: AxisRange,
    y: AxisRange,
    width: u16,
    height: u16,
}

impl TouchGeometry {
    pub(crate) fn new(x: AxisRange, y: AxisRange, width: u16, height: u16) -> Result<Self> {
        ensure!(
            width != 0 && height != 0,
            "touchscreen extent must be nonzero"
        );
        ensure!(
            u32::from(width - 1) <= i16::MAX as u32 && u32::from(height - 1) <= i16::MAX as u32,
            "touchscreen extent exceeds signed physical-coordinate range: {width}x{height}"
        );
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    fn normalize(self, raw_x: i32, raw_y: i32) -> PhysicalPoint {
        PhysicalPoint {
            x: self.x.normalize(raw_x, self.width),
            y: self.y.normalize(raw_y, self.height),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecodedInput {
    Pointer(PointerEvent),
    CancelContact,
}

/// Decoder for the observed PW6 Type-B protocol.
///
/// Slot zero is the primary contact. Other slots are ignored rather than
/// promoted, which keeps one physical contact paired with one PaperPad contact.
pub(crate) struct TypeBDecoder {
    geometry: TouchGeometry,
    current_slot: Option<u16>,
    tracking_id: Option<i32>,
    raw_x: Option<i32>,
    raw_y: Option<i32>,
    last_point: Option<PhysicalPoint>,
    press_emitted: bool,
    release_pending: bool,
    discard_until_report: bool,
}

impl TypeBDecoder {
    pub(crate) fn new(geometry: TouchGeometry) -> Self {
        Self {
            geometry,
            current_slot: Some(0),
            tracking_id: None,
            raw_x: None,
            raw_y: None,
            last_point: None,
            press_emitted: false,
            release_pending: false,
            discard_until_report: false,
        }
    }

    pub(crate) fn feed(&mut self, event_type: u16, code: u16, value: i32) -> Option<DecodedInput> {
        if self.discard_until_report {
            if event_type == EV_SYN && code == SYN_REPORT {
                self.discard_until_report = false;
            }
            return None;
        }

        if event_type == EV_SYN && code == SYN_DROPPED {
            return self.lose_sync();
        }

        match (event_type, code) {
            (EV_SYN, SYN_REPORT) => self.finish_report(),
            (EV_ABS, ABS_MT_SLOT) => self.select_slot(value),
            (EV_ABS, ABS_MT_TRACKING_ID) if self.current_slot == Some(0) => {
                self.update_tracking(value)
            }
            (EV_ABS, ABS_MT_POSITION_X) if self.current_slot == Some(0) => {
                self.raw_x = Some(value);
                None
            }
            (EV_ABS, ABS_MT_POSITION_Y) if self.current_slot == Some(0) => {
                self.raw_y = Some(value);
                None
            }
            _ => None,
        }
    }

    /// Clear a contact after EOF, read failure, or descriptor disappearance.
    pub(crate) fn disconnect(&mut self) -> Option<DecodedInput> {
        let cancel = self.press_emitted;
        self.reset();
        cancel.then_some(DecodedInput::CancelContact)
    }

    fn select_slot(&mut self, value: i32) -> Option<DecodedInput> {
        match u16::try_from(value) {
            Ok(slot) => {
                self.current_slot = Some(slot);
                None
            }
            Err(_) => self.lose_sync(),
        }
    }

    fn update_tracking(&mut self, value: i32) -> Option<DecodedInput> {
        match value {
            -1 => {
                if self.tracking_id.take().is_some() {
                    self.release_pending = true;
                }
                None
            }
            0.. => {
                if self.tracking_id == Some(value) {
                    return None;
                }
                let cancel = self.press_emitted;
                self.tracking_id = Some(value);
                self.raw_x = None;
                self.raw_y = None;
                self.last_point = None;
                self.press_emitted = false;
                self.release_pending = false;
                cancel.then_some(DecodedInput::CancelContact)
            }
            _ => self.lose_sync(),
        }
    }

    fn finish_report(&mut self) -> Option<DecodedInput> {
        if self.release_pending {
            let release = if self.press_emitted {
                self.current_point()
                    .or(self.last_point)
                    .map(|point| DecodedInput::Pointer(pointer(PointerEventKind::Release, point)))
                    .or(Some(DecodedInput::CancelContact))
            } else {
                None
            };
            self.clear_primary_contact();
            return release;
        }

        self.tracking_id?;

        let point = self.current_point()?;
        self.last_point = Some(point);
        if self.press_emitted {
            return None;
        }
        self.press_emitted = true;
        Some(DecodedInput::Pointer(pointer(
            PointerEventKind::Press,
            point,
        )))
    }

    fn current_point(&self) -> Option<PhysicalPoint> {
        Some(self.geometry.normalize(self.raw_x?, self.raw_y?))
    }

    fn lose_sync(&mut self) -> Option<DecodedInput> {
        let cancel = self.press_emitted;
        self.reset();
        self.discard_until_report = true;
        cancel.then_some(DecodedInput::CancelContact)
    }

    fn clear_primary_contact(&mut self) {
        self.tracking_id = None;
        self.raw_x = None;
        self.raw_y = None;
        self.last_point = None;
        self.press_emitted = false;
        self.release_pending = false;
    }

    fn reset(&mut self) {
        self.current_slot = Some(0);
        self.clear_primary_contact();
        self.discard_until_report = false;
    }
}

/// Nonblocking owner of the capability-selected Type-B touchscreen.
///
/// This reader never requests `EVIOCGRAB`. Callers must process every returned
/// decoder outcome in order and honor `EvdevReadError::cancel_contact` before
/// handling or reporting a terminal read failure.
#[cfg(target_os = "linux")]
pub(crate) struct EvdevReader {
    file: File,
    path: PathBuf,
    name: String,
    decoder: TypeBDecoder,
    bytes: Vec<u8>,
}

#[cfg(target_os = "linux")]
impl EvdevReader {
    pub(crate) fn open(screen_size: (u16, u16)) -> Result<Self> {
        let selected = select_touch_device()?;
        let (x, y) = selected.multitouch_axes.with_context(|| {
            format!(
                "selected direct touchscreen {} does not expose Type-B multitouch axes",
                selected.path.display()
            )
        })?;
        let geometry = TouchGeometry::new(
            AxisRange::new(x.minimum, x.maximum)?,
            AxisRange::new(y.minimum, y.maximum)?,
            screen_size.0,
            screen_size.1,
        )?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&selected.path)
            .with_context(|| {
                format!(
                    "open selected touchscreen {} nonblocking",
                    selected.path.display()
                )
            })?;
        let event_size = std::mem::size_of::<libc::input_event>();
        eprintln!(
            "input reader: opened path={} name={:?} axes_x={}..{} axes_y={}..{} screen={}x{} eviocgrab=false",
            selected.path.display(),
            selected.name,
            x.minimum,
            x.maximum,
            y.minimum,
            y.maximum,
            screen_size.0,
            screen_size.1
        );
        Ok(Self {
            file,
            path: selected.path,
            name: selected.name,
            decoder: TypeBDecoder::new(geometry),
            bytes: vec![0; event_size * EVENTS_PER_READ],
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Drain at most `MAX_EVENTS_PER_DRAIN` currently available records.
    pub(crate) fn drain(&mut self) -> Result<Vec<DecodedInput>, EvdevReadError> {
        let mut decoded = Vec::new();
        let mut event_count = 0usize;

        while event_count < MAX_EVENTS_PER_DRAIN {
            let read = match self.file.read(&mut self.bytes) {
                Ok(0) => {
                    return Err(self.fail(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "event device reached EOF or disappeared",
                    )));
                }
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(self.fail(error)),
            };

            let record_count =
                match complete_record_count(read, std::mem::size_of::<libc::input_event>()) {
                    Ok(record_count) => record_count,
                    Err(error) => return Err(self.fail(error)),
                };
            for chunk in self.bytes[..read].chunks_exact(std::mem::size_of::<libc::input_event>()) {
                // SAFETY: divisibility was checked above, each chunk contains
                // exactly one input_event, and unaligned reads accept byte buffers.
                let event: libc::input_event =
                    unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast()) };
                if let Some(input) = self.decoder.feed(event.type_, event.code, event.value) {
                    decoded.push(input);
                }
            }
            event_count += record_count;
        }

        Ok(decoded)
    }

    /// Cancel decoder state before normal reader shutdown.
    pub(crate) fn shutdown(&mut self) -> Option<DecodedInput> {
        self.decoder.disconnect()
    }

    fn fail(&mut self, source: io::Error) -> EvdevReadError {
        // A terminal failure discards this drain's decoded batch. Even if the
        // decoder just observed a release, its caller has not received that
        // release, so cancellation must be unconditional and idempotent.
        let _ = self.decoder.disconnect();
        EvdevReadError {
            path: self.path.clone(),
            source,
            cancel_contact: true,
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(crate) struct EvdevReadError {
    path: PathBuf,
    source: io::Error,
    cancel_contact: bool,
}

#[cfg(target_os = "linux")]
impl EvdevReadError {
    pub(crate) fn cancel_contact(&self) -> bool {
        self.cancel_contact
    }
}

#[cfg(target_os = "linux")]
impl std::fmt::Display for EvdevReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "read touchscreen {}: {}",
            self.path.display(),
            self.source
        )
    }
}

#[cfg(target_os = "linux")]
impl std::error::Error for EvdevReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn complete_record_count(byte_count: usize, record_size: usize) -> io::Result<usize> {
    if byte_count.checked_rem(record_size) != Some(0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("read {byte_count} bytes not divisible by input_event size {record_size}"),
        ));
    }
    Ok(byte_count / record_size)
}

fn pointer(kind: PointerEventKind, point: PhysicalPoint) -> PointerEvent {
    PointerEvent {
        kind,
        detail: PRIMARY_BUTTON_DETAIL,
        point,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pw6_decoder() -> TypeBDecoder {
        TypeBDecoder::new(
            TouchGeometry::new(
                AxisRange::new(0, 1271).unwrap(),
                AxisRange::new(0, 1695).unwrap(),
                1272,
                1696,
            )
            .unwrap(),
        )
    }

    fn pointer_input(kind: PointerEventKind, x: i16, y: i16) -> Option<DecodedInput> {
        Some(DecodedInput::Pointer(PointerEvent {
            kind,
            detail: PRIMARY_BUTTON_DETAIL,
            point: PhysicalPoint { x, y },
        }))
    }

    fn begin_contact(decoder: &mut TypeBDecoder, tracking_id: i32, x: i32, y: i32) {
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_TRACKING_ID, tracking_id), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_POSITION_X, x), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_POSITION_Y, y), None);
    }

    #[test]
    fn observed_pw6_sequence_emits_identity_mapped_primary_pair() {
        let mut decoder = pw6_decoder();
        begin_contact(&mut decoder, 0, 636, 848);
        assert_eq!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            pointer_input(PointerEventKind::Press, 636, 848)
        );
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_TRACKING_ID, -1), None);
        assert_eq!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            pointer_input(PointerEventKind::Release, 636, 848)
        );
    }

    #[test]
    fn release_uses_latest_coordinates_from_final_report() {
        let mut decoder = pw6_decoder();
        begin_contact(&mut decoder, 1, 100, 200);
        assert_eq!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            pointer_input(PointerEventKind::Press, 100, 200)
        );
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_POSITION_X, 1200), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_POSITION_Y, 1600), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_TRACKING_ID, -1), None);
        assert_eq!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            pointer_input(PointerEventKind::Release, 1200, 1600)
        );
    }

    #[test]
    fn incomplete_reports_defer_press_and_never_emit_orphan_release() {
        let mut decoder = pw6_decoder();
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_TRACKING_ID, 7), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_POSITION_X, 100), None);
        assert_eq!(decoder.feed(EV_SYN, SYN_REPORT, 0), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_POSITION_Y, 200), None);
        assert_eq!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            pointer_input(PointerEventKind::Press, 100, 200)
        );
        assert_eq!(decoder.disconnect(), Some(DecodedInput::CancelContact));
        assert_eq!(decoder.disconnect(), None);

        assert_eq!(decoder.feed(EV_ABS, ABS_MT_TRACKING_ID, 8), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_TRACKING_ID, -1), None);
        assert_eq!(decoder.feed(EV_SYN, SYN_REPORT, 0), None);
    }

    #[test]
    fn secondary_slot_is_ignored_and_never_promoted() {
        let mut decoder = pw6_decoder();
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_SLOT, 1), None);
        begin_contact(&mut decoder, 10, 300, 400);
        assert_eq!(decoder.feed(EV_SYN, SYN_REPORT, 0), None);

        assert_eq!(decoder.feed(EV_ABS, ABS_MT_SLOT, 0), None);
        begin_contact(&mut decoder, 11, 500, 600);
        assert_eq!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            pointer_input(PointerEventKind::Press, 500, 600)
        );

        assert_eq!(decoder.feed(EV_ABS, ABS_MT_SLOT, 1), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_TRACKING_ID, -1), None);
        assert_eq!(decoder.feed(EV_SYN, SYN_REPORT, 0), None);
        assert_eq!(decoder.disconnect(), Some(DecodedInput::CancelContact));
    }

    #[test]
    fn syn_dropped_cancels_once_discards_report_and_recovers() {
        let mut decoder = pw6_decoder();
        begin_contact(&mut decoder, 20, 700, 800);
        assert!(matches!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            Some(DecodedInput::Pointer(_))
        ));
        assert_eq!(
            decoder.feed(EV_SYN, SYN_DROPPED, 0),
            Some(DecodedInput::CancelContact)
        );
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_TRACKING_ID, 21), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_POSITION_X, 900), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_POSITION_Y, 1000), None);
        assert_eq!(decoder.feed(EV_SYN, SYN_REPORT, 0), None);

        begin_contact(&mut decoder, 22, 901, 1001);
        assert_eq!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            pointer_input(PointerEventKind::Press, 901, 1001)
        );
    }

    #[test]
    fn replacement_tracking_id_cancels_before_new_press() {
        let mut decoder = pw6_decoder();
        begin_contact(&mut decoder, 30, 10, 20);
        assert!(matches!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            Some(DecodedInput::Pointer(_))
        ));
        assert_eq!(
            decoder.feed(EV_ABS, ABS_MT_TRACKING_ID, 31),
            Some(DecodedInput::CancelContact)
        );
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_POSITION_X, 11), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_POSITION_Y, 21), None);
        assert_eq!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            pointer_input(PointerEventKind::Press, 11, 21)
        );
    }

    #[test]
    fn malformed_primary_state_cancels_and_waits_for_report_boundary() {
        let mut decoder = pw6_decoder();
        begin_contact(&mut decoder, 40, 100, 100);
        assert!(matches!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            Some(DecodedInput::Pointer(_))
        ));
        assert_eq!(
            decoder.feed(EV_ABS, ABS_MT_TRACKING_ID, -2),
            Some(DecodedInput::CancelContact)
        );
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_TRACKING_ID, 41), None);
        assert_eq!(decoder.feed(EV_SYN, SYN_REPORT, 0), None);
        assert_eq!(decoder.feed(EV_ABS, ABS_MT_SLOT, -1), None);
        assert_eq!(decoder.feed(EV_SYN, SYN_REPORT, 0), None);
    }

    #[test]
    fn coordinates_scale_round_and_clamp_to_physical_extent() {
        let geometry = TouchGeometry::new(
            AxisRange::new(100, 1100).unwrap(),
            AxisRange::new(-100, 900).unwrap(),
            101,
            201,
        )
        .unwrap();
        let mut decoder = TypeBDecoder::new(geometry);
        begin_contact(&mut decoder, 50, 600, 400);
        assert_eq!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            pointer_input(PointerEventKind::Press, 50, 100)
        );
        assert_eq!(decoder.disconnect(), Some(DecodedInput::CancelContact));

        begin_contact(&mut decoder, 51, 2000, -200);
        assert_eq!(
            decoder.feed(EV_SYN, SYN_REPORT, 0),
            pointer_input(PointerEventKind::Press, 100, 0)
        );
    }

    #[test]
    fn invalid_axis_ranges_and_extents_are_rejected() {
        assert!(AxisRange::new(1, 1).is_err());
        let valid = AxisRange::new(0, 1).unwrap();
        assert!(TouchGeometry::new(valid, valid, 0, 1).is_err());
        assert!(TouchGeometry::new(valid, valid, 1, 0).is_err());
        assert!(TouchGeometry::new(valid, valid, 32_769, 1).is_err());
    }

    #[test]
    fn event_record_reads_require_complete_fixed_size_records() {
        assert_eq!(complete_record_count(48, 16).unwrap(), 3);
        assert_eq!(
            complete_record_count(47, 16).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            complete_record_count(0, 0).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
