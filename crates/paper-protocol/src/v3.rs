//! PPFB v3 framing and negotiation for independent frame transport.

use std::io::{self, ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use crate::compression::{ZstdDecoder, ZstdEncoder};
use crate::{
    Frame, PixelFormat, V2_HEADER_LEN, V2_MAGIC, V2_MAX_PAYLOAD_LEN, V2FrameRef, V2PixelFormats,
    gray8_payload_len, mono1_payload_len,
};

pub const VERSION: u8 = 3;
pub const HEADER_LEN: usize = V2_HEADER_LEN;
pub const MAX_PAYLOAD_LEN: usize = V2_MAX_PAYLOAD_LEN;
pub const RAW_PREFIX_LEN: usize = 16;
pub const ZSTD_PREFIX_LEN: usize = 20;
pub const MAX_PIXELS_LEN: usize = MAX_PAYLOAD_LEN - RAW_PREFIX_LEN;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Kind {
    Hello = 1,
    PointerDown = 2,
    PointerUp = 3,
    ViewportChanged = 4,
    Frame = 5,
    HelloAck = 6,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub kind: Kind,
    pub payload_len: usize,
}

impl Header {
    pub fn decode(bytes: [u8; HEADER_LEN]) -> io::Result<Self> {
        if bytes[..4] != V2_MAGIC || bytes[4] != VERSION {
            return Err(invalid("expected PPFB version 3 header"));
        }
        let kind = match bytes[5] {
            1 => Kind::Hello,
            2 => Kind::PointerDown,
            3 => Kind::PointerUp,
            4 => Kind::ViewportChanged,
            5 => Kind::Frame,
            6 => Kind::HelloAck,
            value => return Err(invalid(format!("unknown PPFB v3 message type {value}"))),
        };
        let payload_len = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        if bytes[6..8] != [0, 0] || payload_len > MAX_PAYLOAD_LEN {
            return Err(invalid("invalid PPFB v3 flags or payload length"));
        }
        Ok(Self { kind, payload_len })
    }

    fn encode(self) -> [u8; HEADER_LEN] {
        let mut bytes = [0; HEADER_LEN];
        bytes[..4].copy_from_slice(&V2_MAGIC);
        bytes[4] = VERSION;
        bytes[5] = self.kind as u8;
        bytes[8..12].copy_from_slice(&(self.payload_len as u32).to_be_bytes());
        bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Encoding {
    Raw = 0,
    Zstd = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Encodings(u8);

impl Encodings {
    pub const RAW: Self = Self(1);
    pub const RAW_ZSTD: Self = Self(3);

    pub const fn supports(self, encoding: Encoding) -> bool {
        self.0 & (1 << encoding as u8) != 0
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hello {
    pub width: u16,
    pub height: u16,
    pub formats: V2PixelFormats,
    pub encodings: Encodings,
}

impl Hello {
    pub fn encode(self) -> [u8; HEADER_LEN + 8] {
        let mut bytes = [0; HEADER_LEN + 8];
        bytes[..HEADER_LEN].copy_from_slice(
            &Header {
                kind: Kind::Hello,
                payload_len: 8,
            }
            .encode(),
        );
        bytes[HEADER_LEN..HEADER_LEN + 3].copy_from_slice(&[
            VERSION,
            self.formats.mask(),
            self.encodings.0,
        ]);
        bytes[HEADER_LEN + 4..HEADER_LEN + 6].copy_from_slice(&self.width.to_be_bytes());
        bytes[HEADER_LEN + 6..].copy_from_slice(&self.height.to_be_bytes());
        bytes
    }

    pub fn decode(payload: &[u8]) -> io::Result<Self> {
        let [VERSION, formats, encodings, 0, w0, w1, h0, h1] = *payload else {
            return Err(invalid("invalid PPFB v3 Hello"));
        };
        let formats = V2PixelFormats::from_mask(formats)
            .ok_or_else(|| invalid("invalid Hello pixel format mask"))?;
        if encodings & Encodings::RAW.0 == 0 {
            return Err(invalid("PPFB v3 Hello must offer Raw"));
        }
        Ok(Self {
            width: u16::from_be_bytes([w0, w1]),
            height: u16::from_be_bytes([h0, h1]),
            formats,
            encodings: Encodings(encodings & Encodings::RAW_ZSTD.0),
        })
    }
}

pub fn encode_ack(encodings: Encodings) -> [u8; HEADER_LEN + 4] {
    let mut bytes = [0; HEADER_LEN + 4];
    bytes[..HEADER_LEN].copy_from_slice(
        &Header {
            kind: Kind::HelloAck,
            payload_len: 4,
        }
        .encode(),
    );
    bytes[HEADER_LEN] = encodings.0;
    bytes
}

pub fn decode_ack(payload: &[u8], offered: Encodings) -> io::Result<Encodings> {
    let [mask, 0, 0, 0] = *payload else {
        return Err(invalid("invalid PPFB v3 HelloAck"));
    };
    if mask & Encodings::RAW.0 == 0 || mask & !offered.0 != 0 {
        return Err(invalid(
            "PPFB v3 HelloAck has missing Raw or unadvertised encodings",
        ));
    }
    Ok(Encodings(mask))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClientMode {
    #[default]
    PreferV3,
    Legacy,
    Raw,
    Auto,
    Zstd,
}

impl std::str::FromStr for ClientMode {
    type Err = io::Error;

    fn from_str(value: &str) -> io::Result<Self> {
        match value {
            "auto" => Ok(Self::PreferV3),
            "v2" => Ok(Self::Legacy),
            "v3-raw" => Ok(Self::Raw),
            "v3-auto" => Ok(Self::Auto),
            "v3-zstd" => Ok(Self::Zstd),
            _ => Err(io::Error::new(
                ErrorKind::InvalidInput,
                "expected auto, v2, v3-raw, v3-auto, or v3-zstd",
            )),
        }
    }
}

impl ClientMode {
    /// Automatic mode retries v2 on a fresh connection when the peer closes
    /// before acknowledging v3. Both connections must have bounded timeouts.
    pub fn negotiate<S: Read + Write, E: From<io::Error>>(
        self,
        mut stream: S,
        viewport: Viewport,
        reconnect: impl FnOnce() -> Result<S, E>,
    ) -> Result<(S, Option<Encodings>), E> {
        if self != Self::Legacy {
            if let Some(encodings) = self.negotiate_v3(&mut stream, viewport)? {
                return Ok((stream, Some(encodings)));
            }
            drop(stream);
            stream = reconnect()?;
        }
        let hello =
            crate::V2Hello::with_pixel_formats(viewport.width, viewport.height, viewport.formats);
        let encoded = hello.encode_message().map_err(io::Error::other)?;
        stream.write_all(&encoded)?;
        Ok((stream, None))
    }

    fn negotiate_v3(
        self,
        stream: &mut (impl Read + Write),
        viewport: Viewport,
    ) -> io::Result<Option<Encodings>> {
        let Viewport {
            width,
            height,
            formats,
        } = viewport;
        let encodings = if self == Self::Raw {
            Encodings::RAW
        } else {
            Encodings::RAW_ZSTD
        };
        stream.write_all(
            &Hello {
                width,
                height,
                formats,
                encodings,
            }
            .encode(),
        )?;
        let mut bytes = [0; HEADER_LEN];
        if let Err(error) = stream.read_exact(&mut bytes[..1]) {
            if self == Self::PreferV3
                && matches!(
                    error.kind(),
                    ErrorKind::UnexpectedEof
                        | ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionAborted
                )
            {
                return Ok(None);
            }
            return Err(io::Error::new(
                error.kind(),
                format!("PPFB v3 HelloAck unavailable: {error}. Use v2 with a legacy host"),
            ));
        }
        stream.read_exact(&mut bytes[1..])?;
        let header = Header::decode(bytes)?;
        if header.kind != Kind::HelloAck || header.payload_len != 4 {
            return Err(invalid("expected PPFB v3 HelloAck before frames"));
        }
        let mut ack = [0; 4];
        stream.read_exact(&mut ack)?;
        let agreed = decode_ack(&ack, encodings)?;
        if self == Self::Zstd && !agreed.supports(Encoding::Zstd) {
            return Err(invalid(
                "peer did not enable required Zstandard compression",
            ));
        }
        Ok(Some(agreed))
    }
}

/// Version-only translation for the identical control payloads in v2 and v3.
pub fn encode_control(message: &mut [u8]) -> io::Result<()> {
    translate_control_version(message, crate::V2_VERSION, VERSION)
}

pub fn decode_control(message: &mut [u8]) -> io::Result<crate::V2Payload<'_>> {
    translate_control_version(message, VERSION, crate::V2_VERSION)?;
    let Ok(crate::V2DecodeResult::Complete {
        message: decoded,
        consumed,
    }) = crate::decode_v2_message(message)
    else {
        return Err(invalid("invalid PPFB v3 control message"));
    };
    if consumed != message.len() {
        return Err(invalid("trailing data after PPFB v3 control message"));
    }
    crate::decode_v2_payload(decoded).map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
}

fn translate_control_version(message: &mut [u8], from: u8, to: u8) -> io::Result<()> {
    if message.len() < HEADER_LEN
        || message[..4] != V2_MAGIC
        || message[4] != from
        || !matches!(message[5], 2..=4)
    {
        return Err(invalid("expected PPFB pointer or viewport control message"));
    }
    message[4] = to;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
    pub formats: V2PixelFormats,
}

#[derive(Clone, Copy, Debug)]
pub struct FramePrefix {
    pub id: u64,
    pub width: u16,
    pub height: u16,
    pub format: PixelFormat,
    pub encoding: Encoding,
    pub decoded_len: usize,
}

impl FramePrefix {
    /// Check the fixed prefix before allocating space for the payload body.
    pub fn decode(
        prefix: [u8; RAW_PREFIX_LEN],
        payload_len: usize,
        viewport: Viewport,
        encodings: Encodings,
    ) -> io::Result<Self> {
        let id = u64::from_be_bytes(prefix[..8].try_into().expect("fixed prefix range"));
        let width = u16::from_be_bytes([prefix[8], prefix[9]]);
        let height = u16::from_be_bytes([prefix[10], prefix[11]]);
        if (width, height) != (viewport.width, viewport.height) {
            return Err(invalid(format!(
                "frame viewport {width}x{height} does not match {}x{}",
                viewport.width, viewport.height
            )));
        }
        let format = match prefix[12] {
            1 => PixelFormat::Mono1,
            2 => PixelFormat::Gray8,
            value => return Err(invalid(format!("unknown frame pixel format {value}"))),
        };
        if !viewport.formats.supports(format) {
            return Err(invalid("frame pixel format was not advertised"));
        }
        let encoding = match prefix[13] {
            0 => Encoding::Raw,
            1 => Encoding::Zstd,
            value => return Err(invalid(format!("unknown frame encoding {value}"))),
        };
        if prefix[14..] != [0, 0] || !encodings.supports(encoding) {
            return Err(invalid(
                "invalid frame reserved field or unnegotiated encoding",
            ));
        }
        let decoded_len = checked_layout(width, height, format)?;
        let valid = match encoding {
            Encoding::Raw => payload_len == RAW_PREFIX_LEN + decoded_len,
            Encoding::Zstd => (ZSTD_PREFIX_LEN + 1..=MAX_PAYLOAD_LEN).contains(&payload_len),
        };
        if !valid {
            return Err(invalid("invalid PPFB v3 frame payload length"));
        }
        Ok(Self {
            id,
            width,
            height,
            format,
            encoding,
            decoded_len,
        })
    }

    pub fn decode_size(self, declared: [u8; 4]) -> io::Result<()> {
        if u32::from_be_bytes(declared) as usize != self.decoded_len {
            return Err(invalid("declared decoded length does not match layout"));
        }
        Ok(())
    }

    pub fn into_frame(self, pixels: Vec<u8>) -> io::Result<Frame> {
        Frame::new(self.width, self.height, self.format, pixels)
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
    }
}

pub fn raw_frame<'a>(
    id: u64,
    frame: impl Into<V2FrameRef<'a>>,
    message: &mut Vec<u8>,
) -> io::Result<()> {
    let frame = frame.into();
    checked_layout(frame.width(), frame.height(), frame.pixel_format())?;
    encode_frame(id, frame, Encoding::Raw, frame.pixels(), message)
}

/// Encode a Zstandard frame when worthwhile. A false result leaves `message`
/// untouched, allowing the caller to encode Raw without writing to the socket.
pub fn compressed_frame<'a>(
    id: u64,
    frame: impl Into<V2FrameRef<'a>>,
    encoder: &mut ZstdEncoder,
    scratch: &mut Vec<u8>,
    message: &mut Vec<u8>,
) -> io::Result<bool> {
    let frame = frame.into();
    checked_layout(frame.width(), frame.height(), frame.pixel_format())?;
    let Some(encoded) =
        encoder.encode_if_smaller(frame.pixels(), RAW_PREFIX_LEN, ZSTD_PREFIX_LEN, scratch)?
    else {
        return Ok(false);
    };
    encode_frame(id, frame, Encoding::Zstd, encoded, message)?;
    Ok(true)
}

fn encode_frame(
    id: u64,
    frame: V2FrameRef<'_>,
    encoding: Encoding,
    pixels: &[u8],
    message: &mut Vec<u8>,
) -> io::Result<()> {
    let prefix_len = match encoding {
        Encoding::Raw => RAW_PREFIX_LEN,
        Encoding::Zstd => ZSTD_PREFIX_LEN,
    };
    let payload_len = prefix_len + pixels.len();
    message.clear();
    message
        .try_reserve_exact(HEADER_LEN + payload_len)
        .map_err(io::Error::other)?;
    message.extend_from_slice(
        &Header {
            kind: Kind::Frame,
            payload_len,
        }
        .encode(),
    );
    message.extend_from_slice(&id.to_be_bytes());
    message.extend_from_slice(&frame.width().to_be_bytes());
    message.extend_from_slice(&frame.height().to_be_bytes());
    let format = match frame.pixel_format() {
        PixelFormat::Mono1 => 1,
        PixelFormat::Gray8 => 2,
    };
    message.extend_from_slice(&[format, encoding as u8, 0, 0]);
    if encoding == Encoding::Zstd {
        message.extend_from_slice(&(frame.pixels().len() as u32).to_be_bytes());
    }
    message.extend_from_slice(pixels);
    Ok(())
}

/// A validated frame and its transport metadata.
#[derive(Debug)]
pub struct ReceivedFrame {
    pub id: u64,
    pub encoding: Encoding,
    pub encoded_len: usize,
    pub decode_elapsed: Duration,
    pub validation_elapsed: Duration,
    pub frame: Frame,
}

/// Reuse input storage and the decoder context independently of pixel ownership.
#[derive(Default)]
pub struct FrameDecoder {
    decoder: Option<ZstdDecoder>,
    encoded: Vec<u8>,
}

impl FrameDecoder {
    /// Read from the start of a Frame payload. After any error the caller must
    /// close the connection, since a payload may have been partially consumed.
    pub fn read_frame(
        &mut self,
        reader: &mut impl Read,
        header: Header,
        viewport: Viewport,
        encodings: Encodings,
    ) -> io::Result<ReceivedFrame> {
        if header.kind != Kind::Frame
            || !(RAW_PREFIX_LEN..=MAX_PAYLOAD_LEN).contains(&header.payload_len)
        {
            return Err(invalid("expected a bounded PPFB v3 Frame"));
        }
        let mut prefix = [0; RAW_PREFIX_LEN];
        reader.read_exact(&mut prefix)?;
        let prefix = FramePrefix::decode(prefix, header.payload_len, viewport, encodings)?;
        let mut pixels = Vec::new();
        let mut decode_elapsed = Duration::ZERO;
        match prefix.encoding {
            Encoding::Raw => read_bytes(reader, prefix.decoded_len, &mut pixels)?,
            Encoding::Zstd => {
                let mut declared = [0; 4];
                reader.read_exact(&mut declared)?;
                prefix.decode_size(declared)?;
                read_bytes(
                    reader,
                    header.payload_len - ZSTD_PREFIX_LEN,
                    &mut self.encoded,
                )?;
                if self.decoder.is_none() {
                    self.decoder = Some(ZstdDecoder::new()?);
                }
                let started = Instant::now();
                self.decoder.as_mut().expect("initialized decoder").decode(
                    &self.encoded,
                    prefix.decoded_len,
                    &mut pixels,
                )?;
                decode_elapsed = started.elapsed();
            }
        }
        let validation_started = Instant::now();
        let frame = prefix.into_frame(pixels)?;
        Ok(ReceivedFrame {
            id: prefix.id,
            encoding: prefix.encoding,
            encoded_len: HEADER_LEN + header.payload_len,
            decode_elapsed,
            validation_elapsed: validation_started.elapsed(),
            frame,
        })
    }
}

fn read_bytes(reader: &mut impl Read, len: usize, bytes: &mut Vec<u8>) -> io::Result<()> {
    bytes.clear();
    bytes.try_reserve_exact(len).map_err(io::Error::other)?;
    bytes.resize(len, 0);
    reader.read_exact(bytes)
}

fn checked_layout(width: u16, height: u16, format: PixelFormat) -> io::Result<usize> {
    let len = match format {
        PixelFormat::Mono1 => mono1_payload_len(width, height),
        PixelFormat::Gray8 => gray8_payload_len(width, height),
    };
    len.filter(|&len| (1..=MAX_PIXELS_LEN).contains(&len))
        .ok_or_else(|| invalid("frame dimensions have zero or oversized pixel payload"))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &[u8] = include_bytes!("../tests/fixtures/v3-hello.bin");
    const ACK: &[u8] = include_bytes!("../tests/fixtures/v3-ack.bin");
    const RAW: &[u8] = include_bytes!("../tests/fixtures/v3-mono1-raw.bin");
    const ZSTD: &[u8] = include_bytes!("../tests/fixtures/v3-gray8-zstd.bin");

    fn header_and_body(message: &[u8]) -> (Header, &[u8]) {
        let (header, body) = message.split_at(HEADER_LEN);
        (Header::decode(header.try_into().unwrap()).unwrap(), body)
    }

    #[test]
    fn golden_hello_and_ack_enforce_negotiated_encodings() {
        let (header, payload) = header_and_body(HELLO);
        assert_eq!(header.kind, Kind::Hello);
        assert_eq!(header.payload_len, 8);
        let offer = Hello::decode(payload).unwrap();
        assert_eq!((offer.width, offer.height), (1272, 1624));
        assert_eq!(
            offer.formats,
            V2PixelFormats::MONO1.with(PixelFormat::Gray8)
        );
        assert_eq!(offer.encodings, Encodings::RAW_ZSTD);
        assert_eq!(offer.encode().as_slice(), HELLO);
        assert_eq!(encode_ack(Encodings::RAW_ZSTD).as_slice(), ACK);
        let (header, payload) = header_and_body(ACK);
        assert_eq!(header.kind, Kind::HelloAck);
        assert_eq!(
            decode_ack(payload, offer.encodings).unwrap(),
            Encodings::RAW_ZSTD
        );
        assert!(decode_ack(payload, Encodings::RAW).is_err());
        assert!(decode_ack(&[7, 0, 0, 0], offer.encodings).is_err());
        let mut unknown = HELLO[HEADER_LEN..].to_vec();
        unknown[2] = 0x81;
        assert_eq!(Hello::decode(&unknown).unwrap().encodings, Encodings::RAW);
        unknown[2] = 0x82;
        assert!(Hello::decode(&unknown).is_err());
        assert!(matches!(
            crate::decode_v2_message(HELLO),
            Err(crate::V2DecodeError::UnsupportedVersion(3))
        ));
        let legacy = crate::V2Hello::new(1272, 1624).encode_message().unwrap();
        assert!(Header::decode(legacy[..HEADER_LEN].try_into().unwrap()).is_err());
    }

    #[test]
    fn automatic_mode_rejects_partial_and_invalid_acknowledgments() {
        let viewport = Viewport {
            width: 1272,
            height: 1624,
            formats: V2PixelFormats::GRAY8,
        };
        let unknown_ack_bits = encode_ack(Encodings(7));
        for reply in [&ACK[..1], &unknown_ack_bits] {
            // Reserve the write portion before the simulated reply.
            let mut exchange = vec![0; HELLO.len()];
            exchange.extend_from_slice(reply);
            let result: io::Result<_> =
                ClientMode::default().negotiate(io::Cursor::new(exchange), viewport, || {
                    panic!("v3 replies must not trigger a v2 retry")
                });
            assert!(result.is_err());
        }
    }

    #[test]
    fn golden_pixels_survive_raw_and_zstandard_decoding() {
        let mut decoder = FrameDecoder::default();
        let mut message = Vec::new();
        for (golden, width, height, format, pixels) in [
            (RAW, 9, 2, PixelFormat::Mono1, vec![0xaa, 0x80, 0x55, 0]),
            (ZSTD, 32, 16, PixelFormat::Gray8, vec![0xa5; 512]),
        ] {
            let (header, mut body) = header_and_body(golden);
            let viewport = Viewport {
                width,
                height,
                formats: V2PixelFormats::new(format),
            };
            let received = decoder
                .read_frame(&mut body, header, viewport, Encodings::RAW_ZSTD)
                .unwrap();
            assert_eq!(received.id, 42);
            assert_eq!(received.frame.pixels(), pixels);
            assert_eq!(received.encoded_len, golden.len());
            assert!(body.is_empty());
            if received.encoding == Encoding::Raw {
                raw_frame(42, &received.frame, &mut message).unwrap();
                assert_eq!(message, RAW);
            } else {
                let mut encoder = ZstdEncoder::new(1).unwrap();
                assert!(
                    compressed_frame(
                        42,
                        &received.frame,
                        &mut encoder,
                        &mut Vec::new(),
                        &mut message
                    )
                    .unwrap()
                );
                let (header, mut body) = header_and_body(&message);
                let roundtrip = decoder
                    .read_frame(&mut body, header, viewport, Encodings::RAW_ZSTD)
                    .unwrap();
                assert_eq!(roundtrip.frame, received.frame);
            }
        }
    }

    #[test]
    fn rejects_bad_metadata_before_reading_encoded_body() {
        let mut decoder = FrameDecoder::default();
        let viewport = Viewport {
            width: 32,
            height: 16,
            formats: V2PixelFormats::GRAY8,
        };
        let (header, body) = header_and_body(ZSTD);
        let mut unnegotiated = body;
        assert!(
            decoder
                .read_frame(&mut unnegotiated, header, viewport, Encodings::RAW)
                .is_err()
        );
        assert_eq!(unnegotiated.len(), body.len() - RAW_PREFIX_LEN);
        let mut wrong_length = body.to_vec();
        wrong_length[RAW_PREFIX_LEN] = 1;
        let mut stream = wrong_length.as_slice();
        assert!(
            decoder
                .read_frame(&mut stream, header, viewport, Encodings::RAW_ZSTD)
                .is_err()
        );
        assert_eq!(stream.len(), body.len() - ZSTD_PREFIX_LEN);
        let mut too_large = ZSTD[..HEADER_LEN].try_into().unwrap();
        let bytes: &mut [u8; HEADER_LEN] = &mut too_large;
        bytes[8..12].copy_from_slice(&((MAX_PAYLOAD_LEN + 1) as u32).to_be_bytes());
        assert!(Header::decode(too_large).is_err());
        let (header, body) = header_and_body(RAW);
        let mut invalid_padding = body.to_vec();
        invalid_padding[RAW_PREFIX_LEN + 1] |= 1;
        let viewport = Viewport {
            width: 9,
            height: 2,
            formats: V2PixelFormats::MONO1,
        };
        assert!(
            decoder
                .read_frame(
                    &mut invalid_padding.as_slice(),
                    header,
                    viewport,
                    Encodings::RAW
                )
                .is_err()
        );
    }

    #[test]
    fn rejects_unknown_frame_encoding_before_reading_the_body() {
        let (header, body) = header_and_body(ZSTD);
        let mut invalid_encoding = body.to_vec();
        invalid_encoding[13] = u8::MAX;
        let mut stream = invalid_encoding.as_slice();
        let viewport = Viewport {
            width: 32,
            height: 16,
            formats: V2PixelFormats::GRAY8,
        };

        let error = FrameDecoder::default()
            .read_frame(&mut stream, header, viewport, Encodings::RAW_ZSTD)
            .expect_err("unknown frame encoding is rejected");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert_eq!(stream, &invalid_encoding[RAW_PREFIX_LEN..]);
    }

    #[test]
    fn rejects_invalid_frame_dimensions_before_reading_the_body() {
        let (header, body) = header_and_body(ZSTD);
        let mut decoder = FrameDecoder::default();
        for (width, height, viewport_width, viewport_height) in [
            (0, 16, 0, 16),
            (32, 0, 32, 0),
            (31, 16, 32, 16),
            (u16::MAX, u16::MAX, u16::MAX, u16::MAX),
        ] {
            let mut invalid_dimensions = body.to_vec();
            invalid_dimensions[8..10].copy_from_slice(&width.to_be_bytes());
            invalid_dimensions[10..12].copy_from_slice(&height.to_be_bytes());
            let mut stream = invalid_dimensions.as_slice();
            let viewport = Viewport {
                width: viewport_width,
                height: viewport_height,
                formats: V2PixelFormats::GRAY8,
            };

            let error = decoder
                .read_frame(&mut stream, header, viewport, Encodings::RAW_ZSTD)
                .expect_err("invalid frame dimensions are rejected");
            assert_eq!(error.kind(), ErrorKind::InvalidData);
            assert_eq!(stream, &invalid_dimensions[RAW_PREFIX_LEN..]);
        }
    }

    #[test]
    fn v3_controls_keep_existing_payload_fields() {
        let trace = crate::V2PointerTrace::new(8, 9, 10);
        let pointer = crate::V2Pointer::new_traced(crate::V2PointerPhase::Up, 14, 7, trace);
        let mut message = pointer.encode_message().unwrap();
        encode_control(&mut message).unwrap();
        assert_eq!(
            Header::decode(message[..HEADER_LEN].try_into().unwrap())
                .unwrap()
                .kind,
            Kind::PointerUp
        );
        assert_eq!(
            decode_control(&mut message).unwrap(),
            crate::V2Payload::Pointer(pointer)
        );
    }
}
