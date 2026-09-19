//! Mixed legacy-line and protocol-v2 input framing from PaperPad.

use paper_protocol::{
    V2_HEADER_LEN, V2_MAGIC, V2DecodeResult, V2Payload, V2Pointer, decode_v2_message,
    decode_v2_payload,
};
use std::io::{self, BufRead, Read};

/// Maximum complete legacy action line, including its newline when present.
const MAX_INBOUND_LINE_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InboundMessage {
    Line(String),
    Pointer(V2Pointer),
}

pub(crate) fn read_inbound_message<R: BufRead>(
    reader: &mut R,
) -> io::Result<Option<InboundMessage>> {
    let first = match reader.fill_buf()?.first() {
        Some(first) => *first,
        None => return Ok(None),
    };
    if first == V2_MAGIC[0] {
        read_v2_pointer(reader).map(|pointer| Some(InboundMessage::Pointer(pointer)))
    } else {
        read_legacy_line(reader).map(|line| line.map(InboundMessage::Line))
    }
}

fn read_v2_pointer<R: Read>(reader: &mut R) -> io::Result<V2Pointer> {
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
            "protocol-v2 message remained incomplete after declared payload",
        ));
    };
    if consumed != encoded.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "protocol-v2 decoder did not consume the complete message",
        ));
    }
    let message_type = message.message_type();
    match decode_v2_payload(message).map_err(invalid_data)? {
        V2Payload::Pointer(pointer) => Ok(pointer),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected {message_type:?} message from PaperPad"),
        )),
    }
}

fn read_legacy_line<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    let read = reader
        .take((MAX_INBOUND_LINE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if read == 0 {
        return Ok(None);
    }
    if bytes.len() > MAX_INBOUND_LINE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("PaperPad line exceeds {MAX_INBOUND_LINE_BYTES} bytes"),
        ));
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn invalid_data(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use paper_protocol::{V2PointerPhase, encode_v2_message};
    use std::io::Cursor;

    #[test]
    fn mixed_lines_and_pointer_messages_keep_exact_boundaries() {
        let down = V2Pointer::new(V2PointerPhase::Down, 0x1234, 0xabcd);
        let up = V2Pointer::new(V2PointerPhase::Up, u16::MAX, 0);
        let mut encoded = b"event action=media.play_pause;\r\n".to_vec();
        encoded.extend_from_slice(&down.encode_message().expect("encode down"));
        encoded.extend_from_slice(&up.encode_message().expect("encode up"));
        encoded.extend_from_slice(b"event action=tmux.work;\n");
        let mut reader = Cursor::new(encoded);

        assert_eq!(
            read_inbound_message(&mut reader).expect("read action"),
            Some(InboundMessage::Line(
                "event action=media.play_pause;".to_string()
            ))
        );
        assert_eq!(
            read_inbound_message(&mut reader).expect("read down"),
            Some(InboundMessage::Pointer(down))
        );
        assert_eq!(
            read_inbound_message(&mut reader).expect("read up"),
            Some(InboundMessage::Pointer(up))
        );
        assert_eq!(
            read_inbound_message(&mut reader).expect("read second action"),
            Some(InboundMessage::Line("event action=tmux.work;".to_string()))
        );
        assert_eq!(read_inbound_message(&mut reader).expect("EOF"), None);
    }

    #[test]
    fn malformed_or_unexpected_v2_messages_are_rejected() {
        let malformed = encode_v2_message(paper_protocol::V2MessageType::PointerDown, &[0; 3])
            .expect("encode malformed pointer");
        let error =
            read_inbound_message(&mut Cursor::new(malformed)).expect_err("invalid pointer length");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let unexpected = paper_protocol::V2Viewport::new(1272, 1624)
            .encode_message()
            .expect("encode viewport");
        let error =
            read_inbound_message(&mut Cursor::new(unexpected)).expect_err("unexpected viewport");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn legacy_lines_are_bounded_and_utf8_validated() {
        let mut exact = vec![b'x'; MAX_INBOUND_LINE_BYTES - 1];
        exact.push(b'\n');
        assert_eq!(
            read_inbound_message(&mut Cursor::new(exact))
                .expect("exact-limit line")
                .expect("line"),
            InboundMessage::Line("x".repeat(MAX_INBOUND_LINE_BYTES - 1))
        );

        let oversized = vec![b'x'; MAX_INBOUND_LINE_BYTES + 1];
        assert_eq!(
            read_inbound_message(&mut Cursor::new(oversized))
                .expect_err("oversized line")
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            read_inbound_message(&mut Cursor::new(vec![0xff, b'\n']))
                .expect_err("invalid UTF-8")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
