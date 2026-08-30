//! Shared, bounded wire protocol for Kindle/companion transport.
//!
//! TCP remains the ordered data transport. UDP messages in this crate are
//! discovery-only. All versioned messages are semicolon-terminated ASCII
//! envelopes; `display` retains its existing free-text form.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, BufRead, Read};

pub const PROTOCOL_VERSION: u8 = 1;
pub const DEFAULT_TCP_PORT: u16 = 5581;
pub const DEFAULT_DISCOVERY_PORT: u16 = 5580;
pub const MAX_DISCOVERY_DATAGRAM_BYTES: usize = 512;
pub const MAX_STREAM_FRAME_BYTES: usize = 1024;
pub const MAX_DISPLAY_TEXT_BYTES: usize = 256;
pub const HEX_ID_BYTES: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryMessage {
    Discover {
        nonce: String,
    },
    Offer {
        nonce: String,
        companion: String,
        port: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AckStatus {
    Logged,
}

impl AckStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Logged => "logged",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamMessage {
    Hello {
        session: String,
    },
    Welcome {
        companion: String,
    },
    Event {
        session: String,
        seq: u64,
        action: String,
    },
    Ack {
        session: String,
        seq: u64,
        status: AckStatus,
    },
    Display {
        text: String,
    },
    LegacyEvent {
        action: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    Empty,
    TooLong { limit: usize, actual: usize },
    InvalidUtf8,
    ControlCharacter,
    MissingTerminator,
    MissingField(&'static str),
    DuplicateField(String),
    UnknownField(String),
    MalformedField(String),
    UnsupportedVersion(String),
    InvalidValue { field: &'static str, value: String },
    UnknownMessage(String),
    WrongChannel(&'static str),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty protocol message"),
            Self::TooLong { limit, actual } => {
                write!(f, "protocol message is {actual} bytes; limit is {limit}")
            }
            Self::InvalidUtf8 => write!(f, "protocol message is not valid UTF-8"),
            Self::ControlCharacter => write!(f, "protocol message contains a control character"),
            Self::MissingTerminator => write!(f, "versioned protocol message lacks ';' terminator"),
            Self::MissingField(field) => {
                write!(f, "protocol message lacks required field '{field}'")
            }
            Self::DuplicateField(field) => write!(f, "protocol message repeats field '{field}'"),
            Self::UnknownField(field) => write!(f, "protocol message has unknown field '{field}'"),
            Self::MalformedField(field) => write!(f, "malformed protocol field '{field}'"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported protocol version '{version}'")
            }
            Self::InvalidValue { field, value } => {
                write!(f, "invalid value '{value}' for field '{field}'")
            }
            Self::UnknownMessage(message) => write!(f, "unknown protocol message '{message}'"),
            Self::WrongChannel(channel) => write!(f, "message is not valid on {channel}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Debug)]
pub enum ReadFrameError {
    Io(io::Error),
    TooLong { limit: usize },
    InvalidUtf8,
}

/// Generate a 128-bit hexadecimal identifier from the Unix OS random device.
///
/// The supported hosts are macOS and Linux (including the Kindle target), both
/// of which provide `/dev/urandom`. Failure is explicit; callers must not fall
/// back to time, PID, or an IP address as identity.
pub fn generate_hex_id() -> io::Result<String> {
    let mut random = [0_u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let mut encoded = String::with_capacity(HEX_ID_BYTES);
    for byte in random {
        use fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded)
}

impl fmt::Display for ReadFrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "failed to read protocol frame: {error}"),
            Self::TooLong { limit } => write!(f, "protocol frame exceeds {limit} bytes"),
            Self::InvalidUtf8 => write!(f, "protocol frame is not valid UTF-8"),
        }
    }
}

impl std::error::Error for ReadFrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::TooLong { .. } | Self::InvalidUtf8 => None,
        }
    }
}

impl From<io::Error> for ReadFrameError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn format_discovery_message(message: &DiscoveryMessage) -> Result<String, ProtocolError> {
    match message {
        DiscoveryMessage::Discover { nonce } => {
            validate_hex_id("nonce", nonce)?;
            Ok(format!("discover v={PROTOCOL_VERSION} nonce={nonce};"))
        }
        DiscoveryMessage::Offer {
            nonce,
            companion,
            port,
        } => {
            validate_hex_id("nonce", nonce)?;
            validate_hex_id("companion", companion)?;
            validate_port(*port)?;
            Ok(format!(
                "offer v={PROTOCOL_VERSION} nonce={nonce} companion={companion} port={port};"
            ))
        }
    }
}

pub fn parse_discovery_message(input: &[u8]) -> Result<DiscoveryMessage, ProtocolError> {
    if input.len() > MAX_DISCOVERY_DATAGRAM_BYTES {
        return Err(ProtocolError::TooLong {
            limit: MAX_DISCOVERY_DATAGRAM_BYTES,
            actual: input.len(),
        });
    }
    let line = std::str::from_utf8(input).map_err(|_| ProtocolError::InvalidUtf8)?;
    if line.is_empty() {
        return Err(ProtocolError::Empty);
    }
    if line.chars().any(char::is_control) {
        return Err(ProtocolError::ControlCharacter);
    }
    match parse_versioned(line)? {
        ParsedVersioned::Discover { nonce } => Ok(DiscoveryMessage::Discover { nonce }),
        ParsedVersioned::Offer {
            nonce,
            companion,
            port,
        } => Ok(DiscoveryMessage::Offer {
            nonce,
            companion,
            port,
        }),
        _ => Err(ProtocolError::WrongChannel("UDP discovery")),
    }
}

pub fn format_stream_message(message: &StreamMessage) -> Result<String, ProtocolError> {
    let line = match message {
        StreamMessage::Hello { session } => {
            validate_hex_id("session", session)?;
            format!("hello v={PROTOCOL_VERSION} session={session};\n")
        }
        StreamMessage::Welcome { companion } => {
            validate_hex_id("companion", companion)?;
            format!("welcome v={PROTOCOL_VERSION} companion={companion};\n")
        }
        StreamMessage::Event {
            session,
            seq,
            action,
        } => {
            validate_hex_id("session", session)?;
            validate_action(action)?;
            format!("event v={PROTOCOL_VERSION} session={session} seq={seq} action={action};\n")
        }
        StreamMessage::Ack {
            session,
            seq,
            status,
        } => {
            validate_hex_id("session", session)?;
            format!(
                "ack v={PROTOCOL_VERSION} session={session} seq={seq} status={};\n",
                status.as_str()
            )
        }
        StreamMessage::Display { text } => {
            validate_display_text(text)?;
            format!("display {text}\n")
        }
        StreamMessage::LegacyEvent { action } => {
            validate_action(action)?;
            format!("event action={action};\n")
        }
    };
    if line.len().saturating_sub(1) > MAX_STREAM_FRAME_BYTES {
        return Err(ProtocolError::TooLong {
            limit: MAX_STREAM_FRAME_BYTES,
            actual: line.len().saturating_sub(1),
        });
    }
    Ok(line)
}

pub fn parse_stream_message(line: &str) -> Result<StreamMessage, ProtocolError> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.len() > MAX_STREAM_FRAME_BYTES {
        return Err(ProtocolError::TooLong {
            limit: MAX_STREAM_FRAME_BYTES,
            actual: line.len(),
        });
    }
    if line.is_empty() {
        return Err(ProtocolError::Empty);
    }
    if line.chars().any(char::is_control) {
        return Err(ProtocolError::ControlCharacter);
    }
    if let Some(text) = parse_display(line)? {
        return Ok(StreamMessage::Display { text });
    }
    if let Some(action) = parse_legacy_event(line)? {
        return Ok(StreamMessage::LegacyEvent { action });
    }
    match parse_versioned(line)? {
        ParsedVersioned::Hello { session } => Ok(StreamMessage::Hello { session }),
        ParsedVersioned::Welcome { companion } => Ok(StreamMessage::Welcome { companion }),
        ParsedVersioned::Event {
            session,
            seq,
            action,
        } => Ok(StreamMessage::Event {
            session,
            seq,
            action,
        }),
        ParsedVersioned::Ack {
            session,
            seq,
            status,
        } => Ok(StreamMessage::Ack {
            session,
            seq,
            status,
        }),
        ParsedVersioned::Discover { .. } | ParsedVersioned::Offer { .. } => {
            Err(ProtocolError::WrongChannel("TCP stream"))
        }
    }
}

/// Read one UTF-8 line without ever retaining more than `limit` bytes.
///
/// An oversized line is fully drained through its newline so the following
/// frame can still be read. The returned string excludes CRLF/LF delimiters.
pub fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    limit: usize,
) -> Result<Option<String>, ReadFrameError> {
    let mut output = Vec::with_capacity(limit.min(256));
    let mut oversized = false;

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if output.is_empty() && !oversized {
                return Ok(None);
            }
            break;
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |position| position + 1);
        let content = &available[..newline.unwrap_or(available.len())];
        if !oversized {
            if output.len() + content.len() > limit {
                oversized = true;
                output.clear();
            } else {
                output.extend_from_slice(content);
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }

    if oversized {
        return Err(ReadFrameError::TooLong { limit });
    }
    if output.last() == Some(&b'\r') {
        output.pop();
    }
    String::from_utf8(output)
        .map(Some)
        .map_err(|_| ReadFrameError::InvalidUtf8)
}

#[derive(Debug)]
enum ParsedVersioned {
    Discover {
        nonce: String,
    },
    Offer {
        nonce: String,
        companion: String,
        port: u16,
    },
    Hello {
        session: String,
    },
    Welcome {
        companion: String,
    },
    Event {
        session: String,
        seq: u64,
        action: String,
    },
    Ack {
        session: String,
        seq: u64,
        status: AckStatus,
    },
}

fn parse_versioned(line: &str) -> Result<ParsedVersioned, ProtocolError> {
    let body = line
        .strip_suffix(';')
        .ok_or(ProtocolError::MissingTerminator)?;
    if body.trim() != body {
        return Err(ProtocolError::MalformedField(body.to_string()));
    }
    let (kind, rest) = body
        .split_once(' ')
        .ok_or_else(|| ProtocolError::UnknownMessage(body.to_string()))?;
    let allowed = match kind {
        "discover" => &["v", "nonce"][..],
        "offer" => &["v", "nonce", "companion", "port"][..],
        "hello" => &["v", "session"][..],
        "welcome" => &["v", "companion"][..],
        "event" => &["v", "session", "seq", "action"][..],
        "ack" => &["v", "session", "seq", "status"][..],
        _ => return Err(ProtocolError::UnknownMessage(kind.to_string())),
    };
    let fields = parse_fields(rest, allowed)?;
    require_version(&fields)?;

    match kind {
        "discover" => {
            let nonce = required(&fields, "nonce")?;
            validate_hex_id("nonce", nonce)?;
            Ok(ParsedVersioned::Discover {
                nonce: nonce.to_string(),
            })
        }
        "offer" => {
            let nonce = required(&fields, "nonce")?;
            let companion = required(&fields, "companion")?;
            let port = required(&fields, "port")?
                .parse::<u16>()
                .map_err(|_| invalid("port", required(&fields, "port").unwrap_or_default()))?;
            validate_hex_id("nonce", nonce)?;
            validate_hex_id("companion", companion)?;
            validate_port(port)?;
            Ok(ParsedVersioned::Offer {
                nonce: nonce.to_string(),
                companion: companion.to_string(),
                port,
            })
        }
        "hello" => {
            let session = required(&fields, "session")?;
            validate_hex_id("session", session)?;
            Ok(ParsedVersioned::Hello {
                session: session.to_string(),
            })
        }
        "welcome" => {
            let companion = required(&fields, "companion")?;
            validate_hex_id("companion", companion)?;
            Ok(ParsedVersioned::Welcome {
                companion: companion.to_string(),
            })
        }
        "event" => {
            let session = required(&fields, "session")?;
            let sequence = required(&fields, "seq")?;
            let seq = sequence
                .parse::<u64>()
                .map_err(|_| invalid("seq", sequence))?;
            let action = required(&fields, "action")?;
            validate_hex_id("session", session)?;
            validate_action(action)?;
            Ok(ParsedVersioned::Event {
                session: session.to_string(),
                seq,
                action: action.to_string(),
            })
        }
        "ack" => {
            let session = required(&fields, "session")?;
            let sequence = required(&fields, "seq")?;
            let seq = sequence
                .parse::<u64>()
                .map_err(|_| invalid("seq", sequence))?;
            let status = match required(&fields, "status")? {
                "logged" => AckStatus::Logged,
                value => return Err(invalid("status", value)),
            };
            validate_hex_id("session", session)?;
            Ok(ParsedVersioned::Ack {
                session: session.to_string(),
                seq,
                status,
            })
        }
        _ => unreachable!("message kind was matched above"),
    }
}

fn parse_fields<'a>(
    rest: &'a str,
    allowed: &[&str],
) -> Result<BTreeMap<&'a str, &'a str>, ProtocolError> {
    let mut fields = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for token in rest.split_ascii_whitespace() {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| ProtocolError::MalformedField(token.to_string()))?;
        if key.is_empty() || value.is_empty() || value.contains('=') {
            return Err(ProtocolError::MalformedField(token.to_string()));
        }
        if !seen.insert(key) {
            return Err(ProtocolError::DuplicateField(key.to_string()));
        }
        if allowed.contains(&key) {
            fields.insert(key, value);
        } else if !key.starts_with("x-") {
            return Err(ProtocolError::UnknownField(key.to_string()));
        }
    }
    Ok(fields)
}

fn required<'a>(
    fields: &'a BTreeMap<&str, &str>,
    field: &'static str,
) -> Result<&'a str, ProtocolError> {
    fields
        .get(field)
        .copied()
        .ok_or(ProtocolError::MissingField(field))
}

fn require_version(fields: &BTreeMap<&str, &str>) -> Result<(), ProtocolError> {
    let version = required(fields, "v")?;
    if version == PROTOCOL_VERSION.to_string() {
        Ok(())
    } else {
        Err(ProtocolError::UnsupportedVersion(version.to_string()))
    }
}

fn parse_display(line: &str) -> Result<Option<String>, ProtocolError> {
    let text = if let Some(text) = line.strip_prefix("display ") {
        text
    } else if let Some(text) = line.strip_prefix("display: ") {
        text
    } else if line == "display" || line == "display:" {
        return Err(ProtocolError::MissingField("text"));
    } else {
        return Ok(None);
    };
    validate_display_text(text)?;
    Ok(Some(text.to_string()))
}

fn parse_legacy_event(line: &str) -> Result<Option<String>, ProtocolError> {
    let Some(action) = line
        .strip_prefix("event action=")
        .and_then(|rest| rest.strip_suffix(';'))
    else {
        return Ok(None);
    };
    validate_action(action)?;
    Ok(Some(action.to_string()))
}

fn validate_hex_id(field: &'static str, value: &str) -> Result<(), ProtocolError> {
    if value.len() == HEX_ID_BYTES && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(invalid(field, value))
    }
}

fn validate_action(value: &str) -> Result<(), ProtocolError> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(invalid("action", value))
    }
}

fn validate_display_text(value: &str) -> Result<(), ProtocolError> {
    if value.is_empty() {
        return Err(ProtocolError::MissingField("text"));
    }
    if value.len() > MAX_DISPLAY_TEXT_BYTES {
        return Err(ProtocolError::TooLong {
            limit: MAX_DISPLAY_TEXT_BYTES,
            actual: value.len(),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(ProtocolError::ControlCharacter);
    }
    Ok(())
}

fn validate_port(port: u16) -> Result<(), ProtocolError> {
    if port == 0 {
        Err(invalid("port", "0"))
    } else {
        Ok(())
    }
}

fn invalid(field: &'static str, value: &str) -> ProtocolError {
    ProtocolError::InvalidValue {
        field,
        value: value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const ID_A: &str = "0123456789abcdef0123456789abcdef";
    const ID_B: &str = "fedcba9876543210fedcba9876543210";

    #[test]
    fn discovery_round_trips() {
        let messages = [
            DiscoveryMessage::Discover {
                nonce: ID_A.to_string(),
            },
            DiscoveryMessage::Offer {
                nonce: ID_A.to_string(),
                companion: ID_B.to_string(),
                port: 5581,
            },
        ];
        for expected in messages {
            let encoded = format_discovery_message(&expected).expect("format discovery");
            assert_eq!(
                parse_discovery_message(encoded.as_bytes()).expect("parse discovery"),
                expected
            );
        }
    }

    #[test]
    fn stream_messages_round_trip() {
        let messages = [
            StreamMessage::Hello {
                session: ID_A.to_string(),
            },
            StreamMessage::Welcome {
                companion: ID_B.to_string(),
            },
            StreamMessage::Event {
                session: ID_A.to_string(),
                seq: u64::MAX,
                action: "media.play_pause".to_string(),
            },
            StreamMessage::Ack {
                session: ID_A.to_string(),
                seq: 42,
                status: AckStatus::Logged,
            },
            StreamMessage::Display {
                text: "hello world".to_string(),
            },
            StreamMessage::LegacyEvent {
                action: "tmux.work".to_string(),
            },
        ];
        for expected in messages {
            let encoded = format_stream_message(&expected).expect("format stream");
            assert_eq!(
                parse_stream_message(&encoded).expect("parse stream"),
                expected
            );
        }
    }

    #[test]
    fn parser_rejects_versions_fields_overflow_and_controls() {
        assert!(matches!(
            parse_stream_message(&format!("hello v=2 session={ID_A};")),
            Err(ProtocolError::UnsupportedVersion(_))
        ));
        assert!(matches!(
            parse_stream_message(&format!("hello v=1 v=1 session={ID_A};")),
            Err(ProtocolError::DuplicateField(_))
        ));
        assert!(matches!(
            parse_stream_message(&format!("hello v=1 session={ID_A} surprise=yes;")),
            Err(ProtocolError::UnknownField(_))
        ));
        assert!(parse_stream_message(&format!("hello v=1 session={ID_A} x-future=yes;")).is_ok());
        assert!(matches!(
            parse_stream_message(&format!(
                "event v=1 session={ID_A} seq=18446744073709551616 action=tmux.work;"
            )),
            Err(ProtocolError::InvalidValue { field: "seq", .. })
        ));
        assert!(matches!(
            parse_stream_message("display bad\ttext"),
            Err(ProtocolError::ControlCharacter)
        ));
    }

    #[test]
    fn malformed_versioned_event_never_falls_back_to_legacy() {
        assert!(matches!(
            parse_stream_message("event v=1 action=media.next;"),
            Err(ProtocolError::MissingField("session"))
        ));
        assert!(matches!(
            parse_stream_message("event action=media.next"),
            Err(ProtocolError::MissingTerminator)
        ));
    }

    #[test]
    fn channel_and_size_boundaries_are_enforced() {
        let discovery = format!("discover v=1 nonce={ID_A};");
        assert!(matches!(
            parse_stream_message(&discovery),
            Err(ProtocolError::WrongChannel("TCP stream"))
        ));
        let hello = format!("hello v=1 session={ID_A};");
        assert!(matches!(
            parse_discovery_message(hello.as_bytes()),
            Err(ProtocolError::WrongChannel("UDP discovery"))
        ));
        let oversized = vec![b'x'; MAX_DISCOVERY_DATAGRAM_BYTES + 1];
        assert!(matches!(
            parse_discovery_message(&oversized),
            Err(ProtocolError::TooLong { .. })
        ));
        assert!(matches!(
            parse_discovery_message(format!("discover\tv=1 nonce={ID_A};").as_bytes()),
            Err(ProtocolError::ControlCharacter)
        ));
        assert!(
            format_stream_message(&StreamMessage::Display {
                text: "x".repeat(MAX_DISPLAY_TEXT_BYTES),
            })
            .is_ok()
        );
        assert!(matches!(
            format_stream_message(&StreamMessage::Display {
                text: "x".repeat(MAX_DISPLAY_TEXT_BYTES + 1),
            }),
            Err(ProtocolError::TooLong { .. })
        ));
    }

    #[test]
    fn bounded_reader_drains_oversized_line_and_recovers() {
        let input = format!("{}\nhello\r\n", "x".repeat(9));
        let mut reader = Cursor::new(input.into_bytes());
        assert!(matches!(
            read_bounded_line(&mut reader, 8),
            Err(ReadFrameError::TooLong { limit: 8 })
        ));
        assert_eq!(
            read_bounded_line(&mut reader, 8).expect("read next frame"),
            Some("hello".to_string())
        );
        assert_eq!(read_bounded_line(&mut reader, 8).expect("read eof"), None);
    }

    #[test]
    fn bounded_reader_rejects_invalid_utf8_without_poisoning_next_line() {
        let mut reader = Cursor::new(vec![0xff, b'\n', b'o', b'k', b'\n']);
        assert!(matches!(
            read_bounded_line(&mut reader, 8),
            Err(ReadFrameError::InvalidUtf8)
        ));
        assert_eq!(
            read_bounded_line(&mut reader, 8).expect("read next frame"),
            Some("ok".to_string())
        );
    }

    #[test]
    fn generated_id_has_the_protocol_shape() {
        let id = generate_hex_id().expect("read OS randomness");
        assert_eq!(id.len(), HEX_ID_BYTES);
        assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(format_stream_message(&StreamMessage::Hello { session: id }).is_ok());
    }
}
