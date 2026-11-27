use std::collections::TryReserveError;
use std::fmt;
use std::io::{self, Read};

use paper_protocol::v3;
use paper_protocol::{
    Frame, FrameError, V2_HEADER_LEN, V2DecodeError, V2DecodeResult, V2MessageType, V2Payload,
    V2PayloadError, decode_v2_message, decode_v2_payload,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceivedFrame {
    frame_id: u64,
    frame: Frame,
    pub(crate) encoding: v3::Encoding,
    pub(crate) wire_bytes: usize,
    pub(crate) decode_elapsed: std::time::Duration,
}

impl ReceivedFrame {
    pub fn frame_id(&self) -> u64 {
        self.frame_id
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    pub fn into_frame(self) -> Frame {
        self.frame
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadPhase {
    Header,
    Payload,
}

#[derive(Debug)]
pub enum FrameReadError {
    InvalidViewport {
        width: u16,
        height: u16,
    },
    Io {
        phase: ReadPhase,
        received: usize,
        source: io::Error,
    },
    TruncatedHeader {
        received: usize,
    },
    Decode(V2DecodeError),
    Allocation {
        requested: usize,
        source: TryReserveError,
    },
    TruncatedPayload {
        expected: usize,
        received: usize,
    },
    FramingInvariant(String),
    Payload(V2PayloadError),
    UnexpectedMessage(V2MessageType),
    ViewportMismatch {
        expected_width: u16,
        expected_height: u16,
        actual_width: u16,
        actual_height: u16,
    },
    Frame(FrameError),
}

impl fmt::Display for FrameReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidViewport { width, height } => {
                write!(
                    formatter,
                    "expected viewport must be nonzero, got {width}x{height}"
                )
            }
            Self::Io {
                phase,
                received,
                source,
            } => {
                write!(
                    formatter,
                    "I/O error after {received} bytes while reading {phase:?}: {source}"
                )
            }
            Self::TruncatedHeader { received } => write!(
                formatter,
                "PPFB stream ended after {received} of {V2_HEADER_LEN} header bytes"
            ),
            Self::Decode(error) => write!(formatter, "invalid PPFB v2 header: {error}"),
            Self::Allocation { requested, source } => write!(
                formatter,
                "could not allocate {requested} bytes for bounded PPFB message: {source}"
            ),
            Self::TruncatedPayload { expected, received } => write!(
                formatter,
                "PPFB stream ended after {received} of {expected} payload bytes"
            ),
            Self::FramingInvariant(message) => {
                write!(formatter, "PPFB framing invariant failed: {message}")
            }
            Self::Payload(error) => write!(formatter, "invalid PPFB v2 payload: {error}"),
            Self::UnexpectedMessage(message_type) => write!(
                formatter,
                "unexpected server-to-client PPFB message {message_type:?}; expected Frame"
            ),
            Self::ViewportMismatch {
                expected_width,
                expected_height,
                actual_width,
                actual_height,
            } => write!(
                formatter,
                "frame viewport {actual_width}x{actual_height} does not match negotiated {expected_width}x{expected_height}"
            ),
            Self::Frame(error) => write!(formatter, "could not own validated PPFB frame: {error}"),
        }
    }
}

impl std::error::Error for FrameReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Decode(error) => Some(error),
            Self::Allocation { source, .. } => Some(source),
            Self::Payload(error) => Some(error),
            Self::Frame(error) => Some(error),
            Self::InvalidViewport { .. }
            | Self::TruncatedHeader { .. }
            | Self::TruncatedPayload { .. }
            | Self::FramingInvariant(_)
            | Self::UnexpectedMessage(_)
            | Self::ViewportMismatch { .. } => None,
        }
    }
}

pub fn read_next_frame(
    reader: &mut impl Read,
    expected_width: u16,
    expected_height: u16,
) -> Result<Option<ReceivedFrame>, FrameReadError> {
    if expected_width == 0 || expected_height == 0 {
        return Err(FrameReadError::InvalidViewport {
            width: expected_width,
            height: expected_height,
        });
    }

    let Some(header) = read_header(reader)? else {
        return Ok(None);
    };
    let additional = match decode_v2_message(&header).map_err(FrameReadError::Decode)? {
        V2DecodeResult::Incomplete { additional } => additional,
        V2DecodeResult::Complete { .. } => 0,
    };
    let total = V2_HEADER_LEN.checked_add(additional).ok_or_else(|| {
        FrameReadError::FramingInvariant("message length overflowed usize".into())
    })?;
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(total)
        .map_err(|source| FrameReadError::Allocation {
            requested: total,
            source,
        })?;
    encoded.extend_from_slice(&header);
    encoded.resize(total, 0);
    if additional != 0 {
        let received =
            read_until_eof_or_full(reader, &mut encoded[V2_HEADER_LEN..], ReadPhase::Payload)?;
        if received != additional {
            return Err(FrameReadError::TruncatedPayload {
                expected: additional,
                received,
            });
        }
    }

    let message = match decode_v2_message(&encoded).map_err(FrameReadError::Decode)? {
        V2DecodeResult::Complete { message, consumed } if consumed == encoded.len() => message,
        V2DecodeResult::Complete { consumed, .. } => {
            return Err(FrameReadError::FramingInvariant(format!(
                "decoder consumed {consumed} of {} bytes",
                encoded.len()
            )));
        }
        V2DecodeResult::Incomplete { additional } => {
            return Err(FrameReadError::FramingInvariant(format!(
                "decoder still needs {additional} bytes after exact payload read"
            )));
        }
    };
    let message_type = message.message_type();
    let payload = decode_v2_payload(message).map_err(FrameReadError::Payload)?;
    let V2Payload::Frame(frame) = payload else {
        return Err(FrameReadError::UnexpectedMessage(message_type));
    };
    if frame.width() != expected_width || frame.height() != expected_height {
        return Err(FrameReadError::ViewportMismatch {
            expected_width,
            expected_height,
            actual_width: frame.width(),
            actual_height: frame.height(),
        });
    }
    let frame_id = frame.frame_id();
    let frame = frame.to_owned_frame().map_err(FrameReadError::Frame)?;
    Ok(Some(ReceivedFrame {
        frame_id,
        frame,
        encoding: v3::Encoding::Raw,
        wire_bytes: encoded.len(),
        decode_elapsed: std::time::Duration::ZERO,
    }))
}

pub(crate) fn read_next_v3_frame(
    reader: &mut impl Read,
    decoder: &mut v3::FrameDecoder,
    viewport: v3::Viewport,
    encodings: v3::Encodings,
) -> Result<Option<ReceivedFrame>, FrameReadError> {
    let Some(bytes) = read_header(reader)? else {
        return Ok(None);
    };
    let header = v3::Header::decode(bytes).map_err(|source| FrameReadError::Io {
        phase: ReadPhase::Header,
        received: V2_HEADER_LEN,
        source,
    })?;
    let received = decoder
        .read_frame(reader, header, viewport, encodings)
        .map_err(|source| FrameReadError::Io {
            phase: ReadPhase::Payload,
            received: 0,
            source,
        })?;
    Ok(Some(ReceivedFrame {
        frame_id: received.id,
        frame: received.frame,
        encoding: received.encoding,
        wire_bytes: received.encoded_len,
        decode_elapsed: received.decode_elapsed,
    }))
}

fn read_header(reader: &mut impl Read) -> Result<Option<[u8; V2_HEADER_LEN]>, FrameReadError> {
    let mut header = [0; V2_HEADER_LEN];
    let received = read_until_eof_or_full(reader, &mut header, ReadPhase::Header)?;
    match received {
        0 => Ok(None),
        V2_HEADER_LEN => Ok(Some(header)),
        received => Err(FrameReadError::TruncatedHeader { received }),
    }
}

fn read_until_eof_or_full(
    reader: &mut impl Read,
    buffer: &mut [u8],
    phase: ReadPhase,
) -> Result<usize, FrameReadError> {
    let mut received = 0;
    while received < buffer.len() {
        match reader.read(&mut buffer[received..]) {
            Ok(0) => break,
            Ok(count) => received += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(source) => {
                return Err(FrameReadError::Io {
                    phase,
                    received,
                    source,
                });
            }
        }
    }
    Ok(received)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use paper_protocol::{
        Gray8Frame, Mono1Frame, V2_FRAME_PREFIX_LEN, V2_MAGIC, V2_MAX_PAYLOAD_LEN, V2_VERSION,
        V2Hello, V2PixelFormats, encode_v2_frame, encode_v2_message,
    };

    use super::*;

    struct ChunkedReader<R> {
        inner: R,
        maximum_chunk: usize,
        interrupt_next: bool,
    }

    impl<R: Read> Read for ChunkedReader<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.interrupt_next {
                self.interrupt_next = false;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            self.interrupt_next = true;
            let limit = buffer.len().min(self.maximum_chunk);
            self.inner.read(&mut buffer[..limit])
        }
    }

    #[test]
    fn split_and_interrupted_reads_preserve_consecutive_frame_boundaries() {
        let mono = Mono1Frame::new(9, 2, vec![0xaa, 0x80, 0x55, 0x00])
            .expect("valid odd-width Mono1 frame");
        let gray = Gray8Frame::new(9, 2, (0..18).map(|value| value * 15).collect())
            .expect("valid Gray8 frame");
        let mut encoded = encode_v2_frame(41, &mono).expect("encode Mono1 frame");
        encoded.extend(encode_v2_frame(42, &gray).expect("encode Gray8 frame"));
        let mut reader = ChunkedReader {
            inner: Cursor::new(encoded),
            maximum_chunk: 3,
            interrupt_next: false,
        };

        let first = read_next_frame(&mut reader, 9, 2)
            .expect("read first frame")
            .expect("first frame present");
        assert_eq!(first.frame_id(), 41);
        assert_eq!(first.frame(), &Frame::Mono1(mono));

        let second = read_next_frame(&mut reader, 9, 2)
            .expect("read second frame")
            .expect("second frame present");
        assert_eq!(second.frame_id(), 42);
        assert_eq!(second.frame(), &Frame::Gray8(gray));
        assert_eq!(read_next_frame(&mut reader, 9, 2).expect("clean EOF"), None);
    }

    #[test]
    fn clean_eof_is_distinct_from_truncated_header_and_payload() {
        assert_eq!(
            read_next_frame(&mut Cursor::new(Vec::<u8>::new()), 9, 2).expect("clean EOF"),
            None
        );

        let frame = Gray8Frame::new(9, 2, vec![0; 18]).expect("valid Gray8 frame");
        let encoded = encode_v2_frame(1, &frame).expect("encode frame");
        let header_error = read_next_frame(&mut Cursor::new(encoded[..5].to_vec()), 9, 2)
            .expect_err("partial header must fail");
        assert!(
            matches!(
                header_error,
                FrameReadError::TruncatedHeader { received: 5 }
            ),
            "{header_error}"
        );

        let expected_payload = encoded.len() - V2_HEADER_LEN;
        let received_payload = expected_payload - 3;
        let payload_error = read_next_frame(
            &mut Cursor::new(encoded[..V2_HEADER_LEN + received_payload].to_vec()),
            9,
            2,
        )
        .expect_err("partial payload must fail");
        assert!(
            matches!(
                payload_error,
                FrameReadError::TruncatedPayload { expected, received }
                    if expected == expected_payload && received == received_payload
            ),
            "{payload_error}"
        );
    }

    #[test]
    fn oversized_header_is_rejected_before_payload_read_or_allocation() {
        let mut header = [0; V2_HEADER_LEN];
        header[0..4].copy_from_slice(&V2_MAGIC);
        header[4] = V2_VERSION;
        header[5] = V2MessageType::Frame as u8;
        header[8..12].copy_from_slice(&((V2_MAX_PAYLOAD_LEN + 1) as u32).to_be_bytes());

        let error = read_next_frame(&mut Cursor::new(header), 9, 2)
            .expect_err("oversized header must fail");
        assert!(
            matches!(
                error,
                FrameReadError::Decode(V2DecodeError::PayloadTooLarge { length, maximum })
                    if length == V2_MAX_PAYLOAD_LEN + 1 && maximum == V2_MAX_PAYLOAD_LEN
            ),
            "{error}"
        );
    }

    #[test]
    fn malformed_header_and_unexpected_message_are_rejected() {
        let mut invalid_magic = [0; V2_HEADER_LEN];
        invalid_magic[0..4].copy_from_slice(b"NOPE");
        invalid_magic[4] = V2_VERSION;
        invalid_magic[5] = V2MessageType::Frame as u8;
        let error = read_next_frame(&mut Cursor::new(invalid_magic), 9, 2)
            .expect_err("invalid magic must fail");
        assert!(
            matches!(
                error,
                FrameReadError::Decode(V2DecodeError::InvalidMagic(_))
            ),
            "{error}"
        );

        let mut unknown_type = [0; V2_HEADER_LEN];
        unknown_type[0..4].copy_from_slice(&V2_MAGIC);
        unknown_type[4] = V2_VERSION;
        unknown_type[5] = 0xff;
        let error = read_next_frame(&mut Cursor::new(unknown_type), 9, 2)
            .expect_err("unknown message type must fail");
        assert!(
            matches!(
                error,
                FrameReadError::Decode(V2DecodeError::UnknownMessageType(0xff))
            ),
            "{error}"
        );

        let hello = V2Hello::with_pixel_formats(9, 2, V2PixelFormats::MONO1)
            .encode_message()
            .expect("encode unexpected Hello");
        let error =
            read_next_frame(&mut Cursor::new(hello), 9, 2).expect_err("server Hello must fail");
        assert!(
            matches!(
                error,
                FrameReadError::UnexpectedMessage(V2MessageType::Hello)
            ),
            "{error}"
        );
    }

    #[test]
    fn typed_payload_validation_precedes_frame_ownership() {
        let truncated_metadata =
            encode_v2_message(V2MessageType::Frame, &[0; V2_FRAME_PREFIX_LEN - 1])
                .expect("encode malformed frame payload");
        let error = read_next_frame(&mut Cursor::new(truncated_metadata), 9, 2)
            .expect_err("truncated frame metadata must fail");
        assert!(
            matches!(
                error,
                FrameReadError::Payload(V2PayloadError::TruncatedFrameMetadata { .. })
            ),
            "{error}"
        );

        let mut invalid_padding = Vec::with_capacity(V2_FRAME_PREFIX_LEN + 2);
        invalid_padding.extend_from_slice(&7_u64.to_be_bytes());
        invalid_padding.extend_from_slice(&9_u16.to_be_bytes());
        invalid_padding.extend_from_slice(&1_u16.to_be_bytes());
        invalid_padding.push(1);
        invalid_padding.extend_from_slice(&[0; 3]);
        invalid_padding.extend_from_slice(&[0x00, 0x01]);
        let invalid_padding = encode_v2_message(V2MessageType::Frame, &invalid_padding)
            .expect("encode invalid-padding frame payload");
        let error = read_next_frame(&mut Cursor::new(invalid_padding), 9, 1)
            .expect_err("nonwhite Mono1 padding must fail");
        assert!(
            matches!(error, FrameReadError::Payload(V2PayloadError::Mono1(_))),
            "{error}"
        );

        let mut unsupported_format = Vec::with_capacity(V2_FRAME_PREFIX_LEN);
        unsupported_format.extend_from_slice(&8_u64.to_be_bytes());
        unsupported_format.extend_from_slice(&9_u16.to_be_bytes());
        unsupported_format.extend_from_slice(&2_u16.to_be_bytes());
        unsupported_format.push(0xff);
        unsupported_format.extend_from_slice(&[0; 3]);
        let unsupported_format = encode_v2_message(V2MessageType::Frame, &unsupported_format)
            .expect("encode unsupported-format frame payload");
        let error = read_next_frame(&mut Cursor::new(unsupported_format), 9, 2)
            .expect_err("unsupported frame format must fail");
        assert!(
            matches!(
                error,
                FrameReadError::Payload(V2PayloadError::UnsupportedPixelFormat(0xff))
            ),
            "{error}"
        );
    }

    #[test]
    fn mismatched_or_zero_expected_viewports_are_rejected() {
        let frame = Gray8Frame::new(9, 2, vec![0; 18]).expect("valid Gray8 frame");
        let encoded = encode_v2_frame(1, &frame).expect("encode frame");
        let error = read_next_frame(&mut Cursor::new(encoded), 10, 2)
            .expect_err("mismatched viewport must fail");
        assert!(
            matches!(
                error,
                FrameReadError::ViewportMismatch {
                    expected_width: 10,
                    expected_height: 2,
                    actual_width: 9,
                    actual_height: 2,
                }
            ),
            "{error}"
        );

        let error = read_next_frame(&mut Cursor::new(Vec::<u8>::new()), 0, 2)
            .expect_err("zero expected viewport must fail");
        assert!(
            matches!(
                error,
                FrameReadError::InvalidViewport {
                    width: 0,
                    height: 2
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn non_interrupted_io_errors_preserve_phase_and_source() {
        struct FailingReader;

        impl Read for FailingReader {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
            }
        }

        let error = read_next_frame(&mut FailingReader, 9, 2)
            .expect_err("reader failure must be preserved");
        assert!(
            matches!(
                error,
                FrameReadError::Io {
                    phase: ReadPhase::Header,
                    received: 0,
                    ref source,
                } if source.kind() == io::ErrorKind::PermissionDenied
            ),
            "{error}"
        );
    }
}
