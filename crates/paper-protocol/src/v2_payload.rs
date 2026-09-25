//! Typed payloads carried by protocol-v2 message framing.

use std::fmt;

use super::framebuffer::{Mono1Frame, Mono1FrameError, validate_mono1_pixels};
use super::v2::{
    V2_HEADER_LEN, V2_VERSION, V2EncodeError, V2Header, V2Message, V2MessageType, encode_v2_message,
};

pub const V2_HELLO_PAYLOAD_LEN: usize = 8;
pub const V2_POINTER_PAYLOAD_LEN: usize = 4;
pub const V2_VIEWPORT_PAYLOAD_LEN: usize = 4;
pub const V2_FRAME_PREFIX_LEN: usize = 16;

const MONO1_FORMAT_VALUE: u8 = 1;
const MONO1_FORMAT_MASK: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum V2PixelFormat {
    Mono1 = MONO1_FORMAT_VALUE,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V2Hello {
    viewport_width: u16,
    viewport_height: u16,
}

impl V2Hello {
    pub fn new(viewport_width: u16, viewport_height: u16) -> Self {
        Self {
            viewport_width,
            viewport_height,
        }
    }

    pub fn protocol_version(self) -> u8 {
        V2_VERSION
    }

    pub fn viewport_width(self) -> u16 {
        self.viewport_width
    }

    pub fn viewport_height(self) -> u16 {
        self.viewport_height
    }

    pub fn supports(self, pixel_format: V2PixelFormat) -> bool {
        matches!(pixel_format, V2PixelFormat::Mono1)
    }

    pub fn encode_message(self) -> Result<Vec<u8>, V2EncodeError> {
        let mut payload = [0; V2_HELLO_PAYLOAD_LEN];
        payload[0] = V2_VERSION;
        payload[1] = MONO1_FORMAT_MASK;
        payload[4..6].copy_from_slice(&self.viewport_width.to_be_bytes());
        payload[6..8].copy_from_slice(&self.viewport_height.to_be_bytes());
        encode_v2_message(V2MessageType::Hello, &payload)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V2Viewport {
    width: u16,
    height: u16,
}

impl V2Viewport {
    pub fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }

    pub fn width(self) -> u16 {
        self.width
    }

    pub fn height(self) -> u16 {
        self.height
    }

    pub fn encode_message(self) -> Result<Vec<u8>, V2EncodeError> {
        let mut payload = [0; V2_VIEWPORT_PAYLOAD_LEN];
        payload[0..2].copy_from_slice(&self.width.to_be_bytes());
        payload[2..4].copy_from_slice(&self.height.to_be_bytes());
        encode_v2_message(V2MessageType::ViewportChanged, &payload)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum V2PointerPhase {
    Down,
    Up,
}

impl V2PointerPhase {
    fn message_type(self) -> V2MessageType {
        match self {
            Self::Down => V2MessageType::PointerDown,
            Self::Up => V2MessageType::PointerUp,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V2Pointer {
    phase: V2PointerPhase,
    x: u16,
    y: u16,
}

impl V2Pointer {
    pub fn new(phase: V2PointerPhase, x: u16, y: u16) -> Self {
        Self { phase, x, y }
    }

    pub fn phase(self) -> V2PointerPhase {
        self.phase
    }

    pub fn x(self) -> u16 {
        self.x
    }

    pub fn y(self) -> u16 {
        self.y
    }

    pub fn encode_message(self) -> Result<Vec<u8>, V2EncodeError> {
        let mut payload = [0; V2_POINTER_PAYLOAD_LEN];
        payload[0..2].copy_from_slice(&self.x.to_be_bytes());
        payload[2..4].copy_from_slice(&self.y.to_be_bytes());
        encode_v2_message(self.phase.message_type(), &payload)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V2FramePayload<'a> {
    frame_id: u64,
    width: u16,
    height: u16,
    stride: usize,
    pixels: &'a [u8],
}

impl<'a> V2FramePayload<'a> {
    pub fn frame_id(self) -> u64 {
        self.frame_id
    }

    pub fn width(self) -> u16 {
        self.width
    }

    pub fn height(self) -> u16 {
        self.height
    }

    pub fn stride(self) -> usize {
        self.stride
    }

    pub fn pixel_format(self) -> V2PixelFormat {
        V2PixelFormat::Mono1
    }

    pub fn pixels(self) -> &'a [u8] {
        self.pixels
    }

    pub fn to_owned_frame(self) -> Result<Mono1Frame, Mono1FrameError> {
        Mono1Frame::new(self.width, self.height, self.pixels.to_vec())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum V2Payload<'a> {
    Hello(V2Hello),
    Pointer(V2Pointer),
    ViewportChanged(V2Viewport),
    Frame(V2FramePayload<'a>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum V2PayloadError {
    InvalidLength {
        message_type: V2MessageType,
        expected: usize,
        actual: usize,
    },
    TruncatedFrameMetadata {
        minimum: usize,
        actual: usize,
    },
    HelloVersion(u8),
    UnsupportedPixelFormatMask(u8),
    UnsupportedPixelFormat(u8),
    NonZeroReserved {
        message_type: V2MessageType,
        value: u32,
    },
    Mono1(Mono1FrameError),
}

impl fmt::Display for V2PayloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength {
                message_type,
                expected,
                actual,
            } => write!(
                formatter,
                "{message_type:?} payload must be {expected} bytes, got {actual}"
            ),
            Self::TruncatedFrameMetadata { minimum, actual } => write!(
                formatter,
                "Frame payload needs at least {minimum} metadata bytes, got {actual}"
            ),
            Self::HelloVersion(version) => {
                write!(formatter, "Hello advertises unsupported version {version}")
            }
            Self::UnsupportedPixelFormatMask(mask) => {
                write!(
                    formatter,
                    "Hello has unsupported pixel-format mask 0x{mask:02x}"
                )
            }
            Self::UnsupportedPixelFormat(pixel_format) => {
                write!(
                    formatter,
                    "Frame has unsupported pixel format {pixel_format}"
                )
            }
            Self::NonZeroReserved {
                message_type,
                value,
            } => write!(
                formatter,
                "{message_type:?} reserved payload bytes must be zero, got 0x{value:x}"
            ),
            Self::Mono1(error) => write!(formatter, "invalid Mono1 frame payload: {error}"),
        }
    }
}

impl std::error::Error for V2PayloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mono1(error) => Some(error),
            Self::InvalidLength { .. }
            | Self::TruncatedFrameMetadata { .. }
            | Self::HelloVersion(_)
            | Self::UnsupportedPixelFormatMask(_)
            | Self::UnsupportedPixelFormat(_)
            | Self::NonZeroReserved { .. } => None,
        }
    }
}

impl From<Mono1FrameError> for V2PayloadError {
    fn from(error: Mono1FrameError) -> Self {
        Self::Mono1(error)
    }
}

/// Encode a validated Mono1 framebuffer as one complete v2 Frame message.
pub fn encode_v2_frame(frame_id: u64, frame: &Mono1Frame) -> Result<Vec<u8>, V2EncodeError> {
    let payload_len = V2_FRAME_PREFIX_LEN + frame.pixels().len();
    let header = V2Header::new(V2MessageType::Frame, payload_len)?;
    let mut encoded = Vec::with_capacity(V2_HEADER_LEN + payload_len);
    encoded.extend_from_slice(&header.encode());
    encoded.extend_from_slice(&frame_id.to_be_bytes());
    encoded.extend_from_slice(&frame.width().to_be_bytes());
    encoded.extend_from_slice(&frame.height().to_be_bytes());
    encoded.push(MONO1_FORMAT_VALUE);
    encoded.extend_from_slice(&[0; 3]);
    encoded.extend_from_slice(frame.pixels());
    Ok(encoded)
}

/// Decode and validate the typed payload of one complete v2 message.
pub fn decode_v2_payload(message: V2Message<'_>) -> Result<V2Payload<'_>, V2PayloadError> {
    let message_type = message.message_type();
    let payload = message.payload();
    match message_type {
        V2MessageType::Hello => decode_hello(payload).map(V2Payload::Hello),
        V2MessageType::PointerDown => {
            decode_pointer(V2PointerPhase::Down, payload).map(V2Payload::Pointer)
        }
        V2MessageType::PointerUp => {
            decode_pointer(V2PointerPhase::Up, payload).map(V2Payload::Pointer)
        }
        V2MessageType::ViewportChanged => decode_viewport(payload).map(V2Payload::ViewportChanged),
        V2MessageType::Frame => decode_frame(payload).map(V2Payload::Frame),
    }
}

fn decode_hello(payload: &[u8]) -> Result<V2Hello, V2PayloadError> {
    require_exact_length(V2MessageType::Hello, payload, V2_HELLO_PAYLOAD_LEN)?;
    if payload[0] != V2_VERSION {
        return Err(V2PayloadError::HelloVersion(payload[0]));
    }
    if payload[1] != MONO1_FORMAT_MASK {
        return Err(V2PayloadError::UnsupportedPixelFormatMask(payload[1]));
    }
    let reserved = u16::from_be_bytes(payload[2..4].try_into().expect("two-byte payload slice"));
    if reserved != 0 {
        return Err(V2PayloadError::NonZeroReserved {
            message_type: V2MessageType::Hello,
            value: u32::from(reserved),
        });
    }

    Ok(V2Hello::new(
        u16::from_be_bytes(payload[4..6].try_into().expect("two-byte payload slice")),
        u16::from_be_bytes(payload[6..8].try_into().expect("two-byte payload slice")),
    ))
}

fn decode_pointer(phase: V2PointerPhase, payload: &[u8]) -> Result<V2Pointer, V2PayloadError> {
    require_exact_length(phase.message_type(), payload, V2_POINTER_PAYLOAD_LEN)?;
    Ok(V2Pointer::new(
        phase,
        u16::from_be_bytes(payload[0..2].try_into().expect("two-byte payload slice")),
        u16::from_be_bytes(payload[2..4].try_into().expect("two-byte payload slice")),
    ))
}

fn decode_viewport(payload: &[u8]) -> Result<V2Viewport, V2PayloadError> {
    require_exact_length(
        V2MessageType::ViewportChanged,
        payload,
        V2_VIEWPORT_PAYLOAD_LEN,
    )?;
    Ok(V2Viewport::new(
        u16::from_be_bytes(payload[0..2].try_into().expect("two-byte payload slice")),
        u16::from_be_bytes(payload[2..4].try_into().expect("two-byte payload slice")),
    ))
}

fn decode_frame(payload: &[u8]) -> Result<V2FramePayload<'_>, V2PayloadError> {
    if payload.len() < V2_FRAME_PREFIX_LEN {
        return Err(V2PayloadError::TruncatedFrameMetadata {
            minimum: V2_FRAME_PREFIX_LEN,
            actual: payload.len(),
        });
    }

    let frame_id = u64::from_be_bytes(payload[0..8].try_into().expect("eight-byte payload slice"));
    let width = u16::from_be_bytes(payload[8..10].try_into().expect("two-byte payload slice"));
    let height = u16::from_be_bytes(payload[10..12].try_into().expect("two-byte payload slice"));
    if payload[12] != MONO1_FORMAT_VALUE {
        return Err(V2PayloadError::UnsupportedPixelFormat(payload[12]));
    }
    let reserved =
        u32::from(payload[13]) << 16 | u32::from(payload[14]) << 8 | u32::from(payload[15]);
    if reserved != 0 {
        return Err(V2PayloadError::NonZeroReserved {
            message_type: V2MessageType::Frame,
            value: reserved,
        });
    }
    let pixels = &payload[V2_FRAME_PREFIX_LEN..];
    let stride = validate_mono1_pixels(width, height, pixels)?;

    Ok(V2FramePayload {
        frame_id,
        width,
        height,
        stride,
        pixels,
    })
}

fn require_exact_length(
    message_type: V2MessageType,
    payload: &[u8],
    expected: usize,
) -> Result<(), V2PayloadError> {
    if payload.len() == expected {
        Ok(())
    } else {
        Err(V2PayloadError::InvalidLength {
            message_type,
            expected,
            actual: payload.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::v2::{V2DecodeResult, decode_v2_message};
    use super::*;

    fn typed_payload(encoded: &[u8]) -> Result<V2Payload<'_>, V2PayloadError> {
        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(encoded).expect("valid framing")
        else {
            panic!("complete message expected");
        };
        assert_eq!(consumed, encoded.len());
        decode_v2_payload(message)
    }

    #[test]
    fn hello_roundtrip_advertises_version_format_and_viewport() {
        let hello = V2Hello::new(1272, 1624);
        let encoded = hello.encode_message().expect("encode Hello");
        assert_eq!(
            &encoded[V2_HEADER_LEN..],
            &[2, 1, 0, 0, 0x04, 0xf8, 0x06, 0x58]
        );
        assert_eq!(typed_payload(&encoded), Ok(V2Payload::Hello(hello)));
        assert_eq!(hello.protocol_version(), 2);
        assert!(hello.supports(V2PixelFormat::Mono1));
    }

    #[test]
    fn pointer_phase_comes_from_message_type_and_coordinates_are_big_endian() {
        for pointer in [
            V2Pointer::new(V2PointerPhase::Down, 0x1234, 0xabcd),
            V2Pointer::new(V2PointerPhase::Up, u16::MAX, 0),
        ] {
            let encoded = pointer.encode_message().expect("encode pointer");
            assert_eq!(
                &encoded[V2_HEADER_LEN..],
                &[
                    (pointer.x() >> 8) as u8,
                    pointer.x() as u8,
                    (pointer.y() >> 8) as u8,
                    pointer.y() as u8,
                ]
            );
            assert_eq!(typed_payload(&encoded), Ok(V2Payload::Pointer(pointer)));
        }
    }

    #[test]
    fn viewport_roundtrip_allows_local_only_and_maximum_dimensions() {
        for viewport in [
            V2Viewport::new(1272, 0),
            V2Viewport::new(u16::MAX, u16::MAX),
        ] {
            let encoded = viewport.encode_message().expect("encode viewport");
            assert_eq!(
                typed_payload(&encoded),
                Ok(V2Payload::ViewportChanged(viewport))
            );
        }
    }

    #[test]
    fn mono1_frame_roundtrip_borrows_validated_pixels() {
        let frame = Mono1Frame::new(9, 2, vec![0xaa, 0x80, 0x55, 0x00]).expect("Mono1 frame");
        let encoded = encode_v2_frame(0x0102_0304_0506_0708, &frame).expect("encode Frame");
        assert_eq!(
            &encoded[V2_HEADER_LEN..V2_HEADER_LEN + V2_FRAME_PREFIX_LEN],
            &[1, 2, 3, 4, 5, 6, 7, 8, 0, 9, 0, 2, 1, 0, 0, 0,]
        );

        let Ok(V2Payload::Frame(decoded)) = typed_payload(&encoded) else {
            panic!("decoded Frame expected");
        };
        assert_eq!(decoded.frame_id(), 0x0102_0304_0506_0708);
        assert_eq!((decoded.width(), decoded.height()), (9, 2));
        assert_eq!(decoded.stride(), 2);
        assert_eq!(decoded.pixel_format(), V2PixelFormat::Mono1);
        assert_eq!(decoded.pixels(), frame.pixels());
        assert_eq!(decoded.to_owned_frame(), Ok(frame));
    }

    #[test]
    fn fixed_payloads_require_exact_lengths() {
        for (message_type, payload, expected) in [
            (V2MessageType::Hello, &[][..], V2_HELLO_PAYLOAD_LEN),
            (
                V2MessageType::PointerDown,
                &[0, 1, 0][..],
                V2_POINTER_PAYLOAD_LEN,
            ),
            (
                V2MessageType::PointerUp,
                &[0; 5][..],
                V2_POINTER_PAYLOAD_LEN,
            ),
            (
                V2MessageType::ViewportChanged,
                &[0; 5][..],
                V2_VIEWPORT_PAYLOAD_LEN,
            ),
        ] {
            let encoded = encode_v2_message(message_type, payload).expect("encode malformed body");
            assert_eq!(
                typed_payload(&encoded),
                Err(V2PayloadError::InvalidLength {
                    message_type,
                    expected,
                    actual: payload.len(),
                })
            );
        }
    }

    #[test]
    fn hello_rejects_version_format_and_reserved_mismatches() {
        for (index, value, expected) in [
            (0, 3, V2PayloadError::HelloVersion(3)),
            (1, 0, V2PayloadError::UnsupportedPixelFormatMask(0)),
            (
                2,
                1,
                V2PayloadError::NonZeroReserved {
                    message_type: V2MessageType::Hello,
                    value: 0x0100,
                },
            ),
        ] {
            let mut payload = [2, 1, 0, 0, 0, 1, 0, 1];
            payload[index] = value;
            let encoded = encode_v2_message(V2MessageType::Hello, &payload).expect("encode");
            assert_eq!(typed_payload(&encoded), Err(expected));
        }
    }

    #[test]
    fn frame_rejects_bad_metadata_and_mono1_payload() {
        let truncated = encode_v2_message(V2MessageType::Frame, &[0; 15]).expect("encode");
        assert_eq!(
            typed_payload(&truncated),
            Err(V2PayloadError::TruncatedFrameMetadata {
                minimum: V2_FRAME_PREFIX_LEN,
                actual: 15,
            })
        );

        let mut payload = [0; V2_FRAME_PREFIX_LEN + 1];
        payload[8..10].copy_from_slice(&1_u16.to_be_bytes());
        payload[10..12].copy_from_slice(&1_u16.to_be_bytes());
        payload[12] = MONO1_FORMAT_VALUE;

        payload[12] = 2;
        let encoded = encode_v2_message(V2MessageType::Frame, &payload).expect("encode");
        assert_eq!(
            typed_payload(&encoded),
            Err(V2PayloadError::UnsupportedPixelFormat(2))
        );

        payload[12] = MONO1_FORMAT_VALUE;
        payload[15] = 1;
        let encoded = encode_v2_message(V2MessageType::Frame, &payload).expect("encode");
        assert_eq!(
            typed_payload(&encoded),
            Err(V2PayloadError::NonZeroReserved {
                message_type: V2MessageType::Frame,
                value: 1,
            })
        );

        payload[15] = 0;
        payload[8..10].copy_from_slice(&9_u16.to_be_bytes());
        let encoded = encode_v2_message(V2MessageType::Frame, &payload).expect("encode");
        assert_eq!(
            typed_payload(&encoded),
            Err(V2PayloadError::Mono1(Mono1FrameError::PayloadLength {
                expected: 2,
                actual: 1,
            }))
        );

        payload[8..10].copy_from_slice(&1_u16.to_be_bytes());
        payload[V2_FRAME_PREFIX_LEN] = 1;
        let encoded = encode_v2_message(V2MessageType::Frame, &payload).expect("encode");
        assert_eq!(
            typed_payload(&encoded),
            Err(V2PayloadError::Mono1(Mono1FrameError::NonWhitePadding {
                row: 0,
                byte: 1,
            }))
        );
    }
}
