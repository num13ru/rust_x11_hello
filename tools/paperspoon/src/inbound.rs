//! Protocol-v2 session and pointer framing from PaperPad.

use paper_protocol::{
    V2_HEADER_LEN, V2DecodeResult, V2Hello, V2Payload, V2Pointer, V2Viewport, decode_v2_message,
    decode_v2_payload,
};
use std::io::{self, BufRead, Read};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InboundMessage {
    Hello(V2Hello),
    Pointer(V2Pointer),
    ViewportChanged(V2Viewport),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionMessage {
    Pointer(V2Pointer),
    ViewportChanged(V2Viewport),
}

pub(crate) fn read_inbound_message<R: BufRead>(
    reader: &mut R,
) -> io::Result<Option<InboundMessage>> {
    if reader.fill_buf()?.is_empty() {
        return Ok(None);
    }

    read_v2_message(reader).map(Some)
}

pub(crate) fn read_session_hello<R: BufRead>(reader: &mut R) -> io::Result<Option<V2Hello>> {
    match read_inbound_message(reader)? {
        Some(InboundMessage::Hello(hello)) => Ok(Some(hello)),
        Some(InboundMessage::Pointer(_) | InboundMessage::ViewportChanged(_)) => {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "initial PaperPad message was not Hello",
            ))
        }
        None => Ok(None),
    }
}

pub(crate) fn read_session_message<R: BufRead>(
    reader: &mut R,
) -> io::Result<Option<SessionMessage>> {
    match read_inbound_message(reader)? {
        Some(InboundMessage::Pointer(pointer)) => Ok(Some(SessionMessage::Pointer(pointer))),
        Some(InboundMessage::ViewportChanged(viewport)) => {
            Ok(Some(SessionMessage::ViewportChanged(viewport)))
        }
        Some(InboundMessage::Hello(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "duplicate Hello from PaperPad",
        )),
        None => Ok(None),
    }
}

fn read_v2_message<R: Read>(reader: &mut R) -> io::Result<InboundMessage> {
    let mut encoded = vec![0; V2_HEADER_LEN];
    reader.read_exact(&mut encoded)?;

    let additional = match decode_v2_message(&encoded).map_err(invalid_data)? {
        V2DecodeResult::Incomplete { additional } => additional,
        V2DecodeResult::Complete { .. } => 0,
    };
    if additional > 0 {
        let header_len = encoded.len();
        encoded.resize(header_len + additional, 0);
        reader.read_exact(&mut encoded[header_len..])?;
    }

    let V2DecodeResult::Complete { message, consumed } =
        decode_v2_message(&encoded).map_err(invalid_data)?
    else {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "protocol-v2 message remained incomplete after exact read",
        ));
    };
    if consumed != encoded.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "protocol-v2 decoder did not consume complete message",
        ));
    }

    let message_type = message.message_type();
    match decode_v2_payload(message).map_err(invalid_data)? {
        V2Payload::Hello(hello) => Ok(InboundMessage::Hello(hello)),
        V2Payload::Pointer(pointer) => Ok(InboundMessage::Pointer(pointer)),
        V2Payload::ViewportChanged(viewport) => Ok(InboundMessage::ViewportChanged(viewport)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected {message_type:?} message from PaperPad"),
        )),
    }
}

fn invalid_data(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use paper_protocol::{
        Mono1Frame, V2MessageType, V2PointerPhase, V2Viewport, encode_v2_frame, encode_v2_message,
    };
    use std::io::Cursor;

    #[test]
    fn hello_then_viewport_and_pointer_establish_session_order() {
        let hello = V2Hello::new(1272, 1624);
        let viewport = V2Viewport::new(800, 600);
        let pointer = V2Pointer::new(V2PointerPhase::Down, 12, 34);
        let mut encoded = hello.encode_message().expect("encode Hello");
        encoded.extend_from_slice(&viewport.encode_message().expect("encode viewport"));
        encoded.extend_from_slice(&pointer.encode_message().expect("encode pointer"));
        let mut reader = Cursor::new(encoded);

        assert_eq!(
            read_session_hello(&mut reader).expect("read Hello"),
            Some(hello)
        );
        assert_eq!(
            read_session_message(&mut reader).expect("read viewport"),
            Some(SessionMessage::ViewportChanged(viewport))
        );
        assert_eq!(
            read_session_message(&mut reader).expect("read pointer"),
            Some(SessionMessage::Pointer(pointer))
        );
    }

    #[test]
    fn pointer_before_hello_and_duplicate_hello_are_rejected() {
        let pointer = V2Pointer::new(V2PointerPhase::Down, 12, 34)
            .encode_message()
            .expect("encode pointer");
        assert_eq!(
            read_session_hello(&mut Cursor::new(pointer))
                .expect_err("pointer before Hello")
                .kind(),
            io::ErrorKind::InvalidData
        );

        let viewport = V2Viewport::new(800, 600)
            .encode_message()
            .expect("encode viewport");
        assert_eq!(
            read_session_hello(&mut Cursor::new(viewport))
                .expect_err("viewport before Hello")
                .kind(),
            io::ErrorKind::InvalidData
        );

        let hello = V2Hello::new(1272, 1624)
            .encode_message()
            .expect("encode Hello");
        assert_eq!(
            read_session_message(&mut Cursor::new(hello))
                .expect_err("duplicate Hello")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn consecutive_pointer_messages_keep_exact_boundaries() {
        let down = V2Pointer::new(V2PointerPhase::Down, 0x1234, 0xabcd);
        let up = V2Pointer::new(V2PointerPhase::Up, u16::MAX, 0);
        let mut encoded = down.encode_message().expect("encode down");
        encoded.extend_from_slice(&up.encode_message().expect("encode up"));
        let mut reader = Cursor::new(encoded);

        assert_eq!(
            read_inbound_message(&mut reader).expect("read down"),
            Some(InboundMessage::Pointer(down))
        );
        assert_eq!(
            read_inbound_message(&mut reader).expect("read up"),
            Some(InboundMessage::Pointer(up))
        );
        assert_eq!(read_inbound_message(&mut reader).expect("EOF"), None);
    }

    #[test]
    fn malformed_and_unexpected_v2_messages_are_rejected() {
        let malformed = encode_v2_message(V2MessageType::PointerDown, &[0; 3])
            .expect("encode malformed typed payload");
        assert_eq!(
            read_inbound_message(&mut Cursor::new(malformed))
                .expect_err("short pointer payload")
                .kind(),
            io::ErrorKind::InvalidData
        );

        let frame = encode_v2_frame(
            1,
            &Mono1Frame::new(8, 1, vec![0]).expect("valid test frame"),
        )
        .expect("encode frame");
        assert_eq!(
            read_inbound_message(&mut Cursor::new(frame))
                .expect_err("unexpected frame")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn legacy_lines_are_rejected_as_invalid_v2_headers() {
        let legacy = b"legacy line protocol\n".to_vec();
        assert_eq!(
            read_inbound_message(&mut Cursor::new(legacy))
                .expect_err("legacy line")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn truncated_v2_message_is_an_unexpected_eof() {
        let pointer = V2Pointer::new(V2PointerPhase::Down, 1, 2)
            .encode_message()
            .expect("encode pointer");
        assert_eq!(
            read_inbound_message(&mut Cursor::new(&pointer[..V2_HEADER_LEN - 1]))
                .expect_err("truncated header")
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(
            read_inbound_message(&mut Cursor::new(&pointer[..pointer.len() - 1]))
                .expect_err("truncated payload")
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
