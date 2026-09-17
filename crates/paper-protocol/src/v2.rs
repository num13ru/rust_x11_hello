//! Protocol-v2 binary message framing.

use std::fmt;

/// Binary protocol marker at the start of every v2 message.
pub const V2_MAGIC: [u8; 4] = *b"PPFB";
/// Current binary protocol version.
pub const V2_VERSION: u8 = 2;
/// Fixed header length: magic, version, type, flags, and payload length.
pub const V2_HEADER_LEN: usize = 12;
/// Hard upper bound checked from the header before reading a payload.
pub const V2_MAX_PAYLOAD_LEN: usize = 16 * 1024 * 1024;

/// Initial protocol-v2 message types.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum V2MessageType {
    Hello = 1,
    PointerDown = 2,
    PointerUp = 3,
    ViewportChanged = 4,
    Frame = 5,
}

impl TryFrom<u8> for V2MessageType {
    type Error = V2DecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::PointerDown),
            3 => Ok(Self::PointerUp),
            4 => Ok(Self::ViewportChanged),
            5 => Ok(Self::Frame),
            value => Err(V2DecodeError::UnknownMessageType(value)),
        }
    }
}

/// Validated metadata for one protocol-v2 message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V2Header {
    message_type: V2MessageType,
    payload_len: u32,
}

impl V2Header {
    pub fn new(message_type: V2MessageType, payload_len: usize) -> Result<Self, V2EncodeError> {
        if payload_len > V2_MAX_PAYLOAD_LEN {
            return Err(V2EncodeError::PayloadTooLarge {
                length: payload_len,
                maximum: V2_MAX_PAYLOAD_LEN,
            });
        }

        let payload_len =
            u32::try_from(payload_len).map_err(|_| V2EncodeError::PayloadTooLarge {
                length: payload_len,
                maximum: V2_MAX_PAYLOAD_LEN,
            })?;
        Ok(Self {
            message_type,
            payload_len,
        })
    }

    pub fn message_type(self) -> V2MessageType {
        self.message_type
    }

    pub fn payload_len(self) -> usize {
        self.payload_len as usize
    }

    /// Encode the fixed-width header in network byte order.
    pub fn encode(self) -> [u8; V2_HEADER_LEN] {
        let mut encoded = [0; V2_HEADER_LEN];
        encoded[0..4].copy_from_slice(&V2_MAGIC);
        encoded[4] = V2_VERSION;
        encoded[5] = self.message_type as u8;
        encoded[6..8].copy_from_slice(&0_u16.to_be_bytes());
        encoded[8..12].copy_from_slice(&self.payload_len.to_be_bytes());
        encoded
    }
}

/// One complete borrowed protocol-v2 message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V2Message<'a> {
    header: V2Header,
    payload: &'a [u8],
}

impl<'a> V2Message<'a> {
    pub fn header(self) -> V2Header {
        self.header
    }

    pub fn message_type(self) -> V2MessageType {
        self.header.message_type()
    }

    pub fn payload(self) -> &'a [u8] {
        self.payload
    }
}

/// Result of attempting to decode one message from a stream buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum V2DecodeResult<'a> {
    Incomplete {
        /// Minimum additional bytes needed to decode this message.
        additional: usize,
    },
    Complete {
        message: V2Message<'a>,
        /// Bytes consumed, leaving any following message in the caller's buffer.
        consumed: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum V2EncodeError {
    PayloadTooLarge { length: usize, maximum: usize },
}

impl fmt::Display for V2EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge { length, maximum } => write!(
                formatter,
                "protocol-v2 payload is {length} bytes, maximum is {maximum}"
            ),
        }
    }
}

impl std::error::Error for V2EncodeError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum V2DecodeError {
    InvalidMagic([u8; 4]),
    UnsupportedVersion(u8),
    UnknownMessageType(u8),
    NonZeroFlags(u16),
    PayloadTooLarge { length: usize, maximum: usize },
}

impl fmt::Display for V2DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic(magic) => {
                write!(formatter, "invalid protocol-v2 magic {magic:02x?}")
            }
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported protocol version {version}")
            }
            Self::UnknownMessageType(message_type) => {
                write!(formatter, "unknown protocol-v2 message type {message_type}")
            }
            Self::NonZeroFlags(flags) => {
                write!(
                    formatter,
                    "protocol-v2 reserved flags must be zero, got 0x{flags:04x}"
                )
            }
            Self::PayloadTooLarge { length, maximum } => write!(
                formatter,
                "protocol-v2 payload is {length} bytes, maximum is {maximum}"
            ),
        }
    }
}

impl std::error::Error for V2DecodeError {}

/// Encode one complete protocol-v2 message.
pub fn encode_v2_message(
    message_type: V2MessageType,
    payload: &[u8],
) -> Result<Vec<u8>, V2EncodeError> {
    let header = V2Header::new(message_type, payload.len())?;
    let mut encoded = Vec::with_capacity(V2_HEADER_LEN + payload.len());
    encoded.extend_from_slice(&header.encode());
    encoded.extend_from_slice(payload);
    Ok(encoded)
}

/// Decode one message from the start of `input` without allocating.
///
/// Call again with `&input[consumed..]` to decode consecutive messages. Retain
/// the current bytes and append at least `additional` bytes after an incomplete
/// result.
pub fn decode_v2_message(input: &[u8]) -> Result<V2DecodeResult<'_>, V2DecodeError> {
    if input.len() < V2_HEADER_LEN {
        return Ok(V2DecodeResult::Incomplete {
            additional: V2_HEADER_LEN - input.len(),
        });
    }

    let magic: [u8; 4] = input[0..4].try_into().expect("four-byte header slice");
    if magic != V2_MAGIC {
        return Err(V2DecodeError::InvalidMagic(magic));
    }
    if input[4] != V2_VERSION {
        return Err(V2DecodeError::UnsupportedVersion(input[4]));
    }
    let message_type = V2MessageType::try_from(input[5])?;
    let flags = u16::from_be_bytes(input[6..8].try_into().expect("two-byte header slice"));
    if flags != 0 {
        return Err(V2DecodeError::NonZeroFlags(flags));
    }
    let payload_len =
        u32::from_be_bytes(input[8..12].try_into().expect("four-byte header slice")) as usize;
    if payload_len > V2_MAX_PAYLOAD_LEN {
        return Err(V2DecodeError::PayloadTooLarge {
            length: payload_len,
            maximum: V2_MAX_PAYLOAD_LEN,
        });
    }

    let consumed = V2_HEADER_LEN + payload_len;
    if input.len() < consumed {
        return Ok(V2DecodeResult::Incomplete {
            additional: consumed - input.len(),
        });
    }

    Ok(V2DecodeResult::Complete {
        message: V2Message {
            header: V2Header {
                message_type,
                payload_len: payload_len as u32,
            },
            payload: &input[V2_HEADER_LEN..consumed],
        },
        consumed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_encoding_is_fixed_width_and_big_endian() {
        let header = V2Header::new(V2MessageType::Frame, 0x01_02_03).expect("valid header");
        assert_eq!(
            header.encode(),
            [b'P', b'P', b'F', b'B', 2, 5, 0, 0, 0, 1, 2, 3]
        );
        assert_eq!(header.message_type(), V2MessageType::Frame);
        assert_eq!(header.payload_len(), 0x01_02_03);
    }

    #[test]
    fn every_initial_message_type_roundtrips() {
        for message_type in [
            V2MessageType::Hello,
            V2MessageType::PointerDown,
            V2MessageType::PointerUp,
            V2MessageType::ViewportChanged,
            V2MessageType::Frame,
        ] {
            let encoded = encode_v2_message(message_type, b"payload").expect("encode");
            let V2DecodeResult::Complete { message, consumed } =
                decode_v2_message(&encoded).expect("decode")
            else {
                panic!("complete message expected");
            };
            assert_eq!(message.message_type(), message_type);
            assert_eq!(message.header().payload_len(), 7);
            assert_eq!(message.payload(), b"payload");
            assert_eq!(consumed, encoded.len());
        }
    }

    #[test]
    fn partial_header_and_payload_report_additional_bytes() {
        let encoded = encode_v2_message(V2MessageType::Hello, b"hello").expect("encode");
        for available in 0..encoded.len() {
            let additional = if available < V2_HEADER_LEN {
                V2_HEADER_LEN - available
            } else {
                encoded.len() - available
            };
            assert_eq!(
                decode_v2_message(&encoded[..available]),
                Ok(V2DecodeResult::Incomplete { additional }),
                "available={available}"
            );
        }
    }

    #[test]
    fn consecutive_messages_leave_precise_consumed_boundary() {
        let first = encode_v2_message(V2MessageType::Hello, b"one").expect("encode first");
        let second =
            encode_v2_message(V2MessageType::ViewportChanged, b"two").expect("encode second");
        let mut stream = first.clone();
        stream.extend_from_slice(&second);

        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(&stream).expect("decode first")
        else {
            panic!("first message expected");
        };
        assert_eq!(message.payload(), b"one");
        assert_eq!(consumed, first.len());

        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(&stream[consumed..]).expect("decode second")
        else {
            panic!("second message expected");
        };
        assert_eq!(message.message_type(), V2MessageType::ViewportChanged);
        assert_eq!(message.payload(), b"two");
        assert_eq!(consumed, second.len());
    }

    #[test]
    fn invalid_header_fields_are_rejected() {
        let valid = V2Header::new(V2MessageType::Hello, 0)
            .expect("valid header")
            .encode();

        for (index, value, expected) in [
            (0, b'X', V2DecodeError::InvalidMagic(*b"XPFB")),
            (4, 3, V2DecodeError::UnsupportedVersion(3)),
            (5, 0xff, V2DecodeError::UnknownMessageType(0xff)),
        ] {
            let mut header = valid;
            header[index] = value;
            assert_eq!(decode_v2_message(&header), Err(expected));
        }

        let mut nonzero_flags = valid;
        nonzero_flags[6..8].copy_from_slice(&0x1234_u16.to_be_bytes());
        assert_eq!(
            decode_v2_message(&nonzero_flags),
            Err(V2DecodeError::NonZeroFlags(0x1234))
        );
    }

    #[test]
    fn oversized_payload_is_rejected_from_header_alone() {
        assert_eq!(
            V2Header::new(V2MessageType::Frame, V2_MAX_PAYLOAD_LEN + 1),
            Err(V2EncodeError::PayloadTooLarge {
                length: V2_MAX_PAYLOAD_LEN + 1,
                maximum: V2_MAX_PAYLOAD_LEN,
            })
        );

        let mut header = V2Header::new(V2MessageType::Frame, 0)
            .expect("valid header")
            .encode();
        header[8..12].copy_from_slice(&((V2_MAX_PAYLOAD_LEN as u32) + 1).to_be_bytes());
        assert_eq!(
            decode_v2_message(&header),
            Err(V2DecodeError::PayloadTooLarge {
                length: V2_MAX_PAYLOAD_LEN + 1,
                maximum: V2_MAX_PAYLOAD_LEN,
            })
        );
    }
}
