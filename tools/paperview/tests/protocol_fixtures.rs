use std::io::Cursor;

use paper_protocol::{PixelFormat, encode_v2_frame};
use paperview::framebuffer::convert_frame;
use paperview::receiver::read_next_frame;

struct Fixture {
    name: &'static str,
    hex: &'static str,
    frame_id: u64,
    width: u16,
    height: u16,
    pixel_format: PixelFormat,
    expected_luma: &'static [u8],
}

const MONO1_BLACK_WHITE_LUMA: &[u8] = &[
    0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255,
];
const MONO1_ODD_WIDTH_LUMA: &[u8] = &[
    0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0,
];
const GRAY8_GRADIENT_LUMA: &[u8] = &[
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "mono1-black-white-8x2",
        hex: include_str!("fixtures/mono1-black-white-8x2.ppfb.hex"),
        frame_id: 1,
        width: 8,
        height: 2,
        pixel_format: PixelFormat::Mono1,
        expected_luma: MONO1_BLACK_WHITE_LUMA,
    },
    Fixture {
        name: "mono1-odd-width-10x2",
        hex: include_str!("fixtures/mono1-odd-width-10x2.ppfb.hex"),
        frame_id: 2,
        width: 10,
        height: 2,
        pixel_format: PixelFormat::Mono1,
        expected_luma: MONO1_ODD_WIDTH_LUMA,
    },
    Fixture {
        name: "gray8-gradient-8x2",
        hex: include_str!("fixtures/gray8-gradient-8x2.ppfb.hex"),
        frame_id: 3,
        width: 8,
        height: 2,
        pixel_format: PixelFormat::Gray8,
        expected_luma: GRAY8_GRADIENT_LUMA,
    },
];

#[test]
fn checked_in_frames_roundtrip_through_protocol_and_presentation() {
    for fixture in FIXTURES {
        let encoded = decode_hex_fixture(fixture.name, fixture.hex);
        let mut input = Cursor::new(encoded.as_slice());
        let received = read_next_frame(&mut input, fixture.width, fixture.height)
            .unwrap_or_else(|error| panic!("{} failed to decode: {error}", fixture.name))
            .unwrap_or_else(|| panic!("{} ended before its frame", fixture.name));

        assert_eq!(received.frame_id(), fixture.frame_id, "{}", fixture.name);
        assert_eq!(
            (received.frame().width(), received.frame().height()),
            (fixture.width, fixture.height),
            "{}",
            fixture.name
        );
        assert_eq!(
            received.frame().pixel_format(),
            fixture.pixel_format,
            "{}",
            fixture.name
        );

        let presentation = convert_frame(received.frame())
            .unwrap_or_else(|error| panic!("{} failed conversion: {error}", fixture.name));
        let expected_rgb: Vec<u32> = fixture
            .expected_luma
            .iter()
            .map(|value| {
                let value = u32::from(*value);
                (value << 16) | (value << 8) | value
            })
            .collect();
        assert_eq!(presentation.pixels(), expected_rgb, "{}", fixture.name);

        let reencoded = encode_v2_frame(received.frame_id(), received.frame())
            .unwrap_or_else(|error| panic!("{} failed to re-encode: {error}", fixture.name));
        assert_eq!(reencoded, encoded, "{}", fixture.name);
    }
}

fn decode_hex_fixture(name: &str, input: &str) -> Vec<u8> {
    let mut encoded = Vec::new();
    for line in input.lines() {
        let data = line.split_once('#').map_or(line, |(data, _)| data);
        for token in data.split_whitespace() {
            assert!(
                token.len().is_multiple_of(2),
                "{name} contains odd-length hex token {token:?}"
            );
            for offset in (0..token.len()).step_by(2) {
                encoded.push(
                    u8::from_str_radix(&token[offset..offset + 2], 16)
                        .unwrap_or_else(|error| panic!("{name} contains invalid hex: {error}")),
                );
            }
        }
    }
    encoded
}
