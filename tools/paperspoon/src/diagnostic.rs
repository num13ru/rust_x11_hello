//! Explicit stdin commands for host-generated framebuffers.

use paper_protocol::{
    Mono1Frame, V2_FRAME_PREFIX_LEN, V2_MAX_PAYLOAD_LEN, encode_v2_frame, mono1_payload_len,
    mono1_stride,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagnosticPattern {
    White,
    Black,
    Horizontal,
    Checkerboard,
    Border,
    Corners,
}

impl DiagnosticPattern {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "white" => Some(Self::White),
            "black" => Some(Self::Black),
            "horizontal" => Some(Self::Horizontal),
            "checkerboard" => Some(Self::Checkerboard),
            "border" => Some(Self::Border),
            "corners" => Some(Self::Corners),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::White => "white",
            Self::Black => "black",
            Self::Horizontal => "horizontal",
            Self::Checkerboard => "checkerboard",
            Self::Border => "border",
            Self::Corners => "corners",
        }
    }

    fn is_black(self, x: u16, y: u16, width: u16, height: u16) -> bool {
        match self {
            Self::White => false,
            Self::Black => true,
            Self::Horizontal => y & 1 == 0,
            Self::Checkerboard => (x ^ y) & 1 == 0,
            Self::Border => x == 0 || y == 0 || x == width - 1 || y == height - 1,
            Self::Corners => corner_marker(x, y, width, height),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DiagnosticFrame {
    pattern: DiagnosticPattern,
    width: u16,
    height: u16,
}

impl DiagnosticFrame {
    pub(crate) fn pattern_name(self) -> &'static str {
        self.pattern.name()
    }

    pub(crate) fn dimensions(self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub(crate) fn encode(self, frame_id: u64) -> Result<Vec<u8>, String> {
        let pixel_bytes = mono1_payload_len(self.width, self.height)
            .ok_or_else(|| "diagnostic frame dimensions overflow payload size".to_string())?;
        let payload_bytes = V2_FRAME_PREFIX_LEN
            .checked_add(pixel_bytes)
            .ok_or_else(|| "diagnostic frame payload size overflow".to_string())?;
        if payload_bytes > V2_MAX_PAYLOAD_LEN {
            return Err(format!(
                "diagnostic frame payload is {payload_bytes} bytes; maximum is {V2_MAX_PAYLOAD_LEN}"
            ));
        }

        let stride = mono1_stride(self.width);
        let mut pixels = vec![0_u8; pixel_bytes];
        for y in 0..self.height {
            for x in 0..self.width {
                if self.pattern.is_black(x, y, self.width, self.height) {
                    let byte = usize::from(y) * stride + usize::from(x / 8);
                    pixels[byte] |= 0x80 >> (x % 8);
                }
            }
        }
        let frame = Mono1Frame::new(self.width, self.height, pixels)
            .map_err(|error| format!("failed to build diagnostic frame: {error}"))?;
        encode_v2_frame(frame_id, &frame)
            .map_err(|error| format!("failed to encode diagnostic frame: {error}"))
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum StdinCommand {
    Frame(DiagnosticFrame),
    ApplicationFrame { width: u16, height: u16 },
}

pub(crate) fn parse_stdin_command(line: &str) -> Result<StdinCommand, String> {
    let mut parts = line.split_whitespace();
    match parts.next() {
        Some("ui") => {
            let dimensions = parts
                .next()
                .ok_or_else(|| "usage: ui <width>x<height>".to_string())?;
            if parts.next().is_some() {
                return Err("usage: ui <width>x<height>".to_string());
            }
            let (width, height) = parse_dimensions(dimensions)?;
            return Ok(StdinCommand::ApplicationFrame { width, height });
        }
        Some("frame") => {}
        Some(command) => return Err(format!("unknown stdin command {command:?}")),
        None => return Err("stdin command is empty".to_string()),
    }

    let pattern_name = parts
        .next()
        .ok_or_else(|| "usage: frame <pattern> <width>x<height>".to_string())?;
    let pattern = DiagnosticPattern::parse(pattern_name).ok_or_else(|| {
        format!(
            "unknown frame pattern '{pattern_name}'; expected white, black, horizontal, checkerboard, border, or corners"
        )
    })?;
    let dimensions = parts
        .next()
        .ok_or_else(|| "usage: frame <pattern> <width>x<height>".to_string())?;
    if parts.next().is_some() {
        return Err("usage: frame <pattern> <width>x<height>".to_string());
    }
    let (width, height) = parse_dimensions(dimensions)?;

    Ok(StdinCommand::Frame(DiagnosticFrame {
        pattern,
        width,
        height,
    }))
}

fn parse_dimensions(dimensions: &str) -> Result<(u16, u16), String> {
    let (width, height) = dimensions
        .split_once('x')
        .ok_or_else(|| "dimensions must use <width>x<height>".to_string())?;
    let width = width
        .parse::<u16>()
        .map_err(|_| format!("invalid frame width '{width}'"))?;
    let height = height
        .parse::<u16>()
        .map_err(|_| format!("invalid frame height '{height}'"))?;
    if width == 0 || height == 0 {
        return Err("frame dimensions must be nonzero".to_string());
    }
    Ok((width, height))
}

/// Four differently sized filled blocks make every corner distinguishable.
fn corner_marker(x: u16, y: u16, width: u16, height: u16) -> bool {
    let largest = width.min(height).min(256);
    let top_right = (largest * 3 / 4).max(1);
    let bottom_left = (largest / 2).max(1);
    let bottom_right = (largest / 4).max(1);

    (x < largest && y < largest)
        || (x >= width.saturating_sub(top_right) && y < top_right)
        || (x < bottom_left && y >= height.saturating_sub(bottom_left))
        || (x >= width.saturating_sub(bottom_right) && y >= height.saturating_sub(bottom_right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use paper_protocol::{
        Mono1Pixel, V2DecodeResult, V2Payload, decode_v2_message, decode_v2_payload,
    };

    fn decoded_frame(spec: DiagnosticFrame) -> (u64, Mono1Frame) {
        let encoded = spec.encode(42).expect("encode diagnostic frame");
        let V2DecodeResult::Complete { message, consumed } =
            decode_v2_message(&encoded).expect("decode message")
        else {
            panic!("complete message expected");
        };
        assert_eq!(consumed, encoded.len());
        let V2Payload::Frame(frame) = decode_v2_payload(message).expect("decode payload") else {
            panic!("Frame expected");
        };
        (
            frame.frame_id(),
            frame.to_owned_frame().expect("owned frame"),
        )
    }

    #[test]
    fn commands_are_v2_frames_and_syntax_is_strict() {
        assert!(parse_stdin_command("unknown hello").is_err());
        assert_eq!(
            parse_stdin_command("ui 1272x1624"),
            Ok(StdinCommand::ApplicationFrame {
                width: 1272,
                height: 1624,
            })
        );
        assert!(parse_stdin_command("ui").is_err());
        assert!(parse_stdin_command("ui 1272").is_err());
        assert!(parse_stdin_command("ui 0x1624").is_err());
        assert!(parse_stdin_command("ui 1272x1624 extra").is_err());
        assert!(parse_stdin_command("frame").is_err());
        assert!(parse_stdin_command("frame unknown 8x8").is_err());
        assert!(parse_stdin_command("frame white 8").is_err());
        assert!(parse_stdin_command("frame white 0x8").is_err());
        assert!(parse_stdin_command("frame white 8x8 extra").is_err());

        let StdinCommand::Frame(frame) =
            parse_stdin_command("frame border 9x2").expect("frame command")
        else {
            panic!("Frame command expected");
        };
        assert_eq!(frame.pattern_name(), "border");
        assert_eq!(frame.dimensions(), (9, 2));
    }

    #[test]
    fn white_black_stripes_and_checkerboard_have_expected_pixels() {
        for (pattern, expected) in [
            (DiagnosticPattern::White, Mono1Pixel::White),
            (DiagnosticPattern::Black, Mono1Pixel::Black),
        ] {
            let (frame_id, frame) = decoded_frame(DiagnosticFrame {
                pattern,
                width: 9,
                height: 2,
            });
            assert_eq!(frame_id, 42);
            assert!((0..2).all(|y| (0..9).all(|x| frame.pixel(x, y) == Some(expected))));
        }

        let (_, horizontal) = decoded_frame(DiagnosticFrame {
            pattern: DiagnosticPattern::Horizontal,
            width: 9,
            height: 2,
        });
        assert_eq!(horizontal.pixel(8, 0), Some(Mono1Pixel::Black));
        assert_eq!(horizontal.pixel(8, 1), Some(Mono1Pixel::White));

        let (_, checkerboard) = decoded_frame(DiagnosticFrame {
            pattern: DiagnosticPattern::Checkerboard,
            width: 9,
            height: 2,
        });
        assert_eq!(checkerboard.pixel(0, 0), Some(Mono1Pixel::Black));
        assert_eq!(checkerboard.pixel(1, 0), Some(Mono1Pixel::White));
        assert_eq!(checkerboard.pixel(0, 1), Some(Mono1Pixel::White));
        assert_eq!(checkerboard.pixel(8, 1), Some(Mono1Pixel::White));
    }

    #[test]
    fn border_and_asymmetric_corners_cover_exact_edges() {
        let (_, border) = decoded_frame(DiagnosticFrame {
            pattern: DiagnosticPattern::Border,
            width: 17,
            height: 9,
        });
        assert_eq!(border.pixel(0, 4), Some(Mono1Pixel::Black));
        assert_eq!(border.pixel(16, 4), Some(Mono1Pixel::Black));
        assert_eq!(border.pixel(8, 0), Some(Mono1Pixel::Black));
        assert_eq!(border.pixel(8, 8), Some(Mono1Pixel::Black));
        assert_eq!(border.pixel(8, 4), Some(Mono1Pixel::White));

        let (_, corners) = decoded_frame(DiagnosticFrame {
            pattern: DiagnosticPattern::Corners,
            width: 512,
            height: 512,
        });
        assert_eq!(corners.pixel(200, 200), Some(Mono1Pixel::Black));
        assert_eq!(corners.pixel(300, 200), Some(Mono1Pixel::White));
        assert_eq!(corners.pixel(511, 191), Some(Mono1Pixel::Black));
        assert_eq!(corners.pixel(511, 192), Some(Mono1Pixel::White));
        assert_eq!(corners.pixel(127, 511), Some(Mono1Pixel::Black));
        assert_eq!(corners.pixel(128, 511), Some(Mono1Pixel::White));
        assert_eq!(corners.pixel(511, 511), Some(Mono1Pixel::Black));
    }

    #[test]
    fn oversized_frame_is_rejected_before_pixel_allocation() {
        let frame = DiagnosticFrame {
            pattern: DiagnosticPattern::White,
            width: u16::MAX,
            height: u16::MAX,
        };
        assert!(frame.encode(1).expect_err("oversized").contains("maximum"));
    }
}
