//! Host-only JPEG decoding and deterministic Gray8 fit-contain rendering.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek};
use std::path::Path;

#[cfg(test)]
use std::io::Cursor;

use image::codecs::jpeg::JpegDecoder;
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, ImageDecoder};
use paper_protocol::{Gray8Frame, V2_FRAME_PREFIX_LEN, V2_MAX_PAYLOAD_LEN};

const MAX_DECODED_JPEG_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) fn decode_jpeg_path_fit_contain(
    path: &Path,
    width: u16,
    height: u16,
) -> Result<Gray8Frame, String> {
    let file = File::open(path)
        .map_err(|error| format!("failed to open JPEG {}: {error}", path.display()))?;
    decode_jpeg(BufReader::new(file), width, height)
        .map_err(|error| format!("failed to load JPEG {}: {error}", path.display()))
}

#[cfg(test)]
fn decode_jpeg_fit_contain(jpeg: &[u8], width: u16, height: u16) -> Result<Gray8Frame, String> {
    decode_jpeg(Cursor::new(jpeg), width, height)
}

fn decode_jpeg<R: BufRead + Seek>(
    reader: R,
    width: u16,
    height: u16,
) -> Result<Gray8Frame, String> {
    validate_target_size(width, height)?;
    let decoder = JpegDecoder::new(reader)
        .map_err(|error| format!("failed to decode JPEG header: {error}"))?;
    let decoded_bytes = decoder.total_bytes();
    if decoded_bytes > MAX_DECODED_JPEG_BYTES {
        return Err(format!(
            "decoded JPEG requires {decoded_bytes} bytes; maximum is {MAX_DECODED_JPEG_BYTES}"
        ));
    }
    let decoded = DynamicImage::from_decoder(decoder)
        .map_err(|error| format!("failed to decode JPEG pixels: {error}"))?
        .into_luma8();
    fit_contain(&decoded, width, height)
}

fn validate_target_size(width: u16, height: u16) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err(format!(
            "JPEG target dimensions must be nonzero, got {width}x{height}"
        ));
    }
    let pixel_bytes = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or_else(|| "JPEG target payload size overflow".to_string())?;
    let payload_bytes = V2_FRAME_PREFIX_LEN
        .checked_add(pixel_bytes)
        .ok_or_else(|| "JPEG frame payload size overflow".to_string())?;
    if payload_bytes > V2_MAX_PAYLOAD_LEN {
        return Err(format!(
            "JPEG frame payload is {payload_bytes} bytes; maximum is {V2_MAX_PAYLOAD_LEN}"
        ));
    }
    Ok(())
}

fn fit_contain(source: &GrayImage, width: u16, height: u16) -> Result<Gray8Frame, String> {
    let source_width = source.width();
    let source_height = source.height();
    if source_width == 0 || source_height == 0 {
        return Err("JPEG source dimensions must be nonzero".to_string());
    }

    let (scaled_width, scaled_height) = fit_dimensions(
        source_width,
        source_height,
        u32::from(width),
        u32::from(height),
    );
    let scaled = image::imageops::resize(source, scaled_width, scaled_height, FilterType::Triangle);

    let target_width = usize::from(width);
    let target_height = usize::from(height);
    let pixel_bytes = target_width
        .checked_mul(target_height)
        .ok_or_else(|| "JPEG target payload size overflow".to_string())?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(pixel_bytes)
        .map_err(|error| format!("failed to allocate JPEG target framebuffer: {error}"))?;
    pixels.resize(pixel_bytes, u8::MAX);

    let scaled_width = usize::try_from(scaled_width)
        .map_err(|_| "scaled JPEG width does not fit usize".to_string())?;
    let scaled_height = usize::try_from(scaled_height)
        .map_err(|_| "scaled JPEG height does not fit usize".to_string())?;
    let offset_x = (target_width - scaled_width) / 2;
    let offset_y = (target_height - scaled_height) / 2;
    for row in 0..scaled_height {
        let source_start = row * scaled_width;
        let destination_start = (offset_y + row) * target_width + offset_x;
        pixels[destination_start..destination_start + scaled_width]
            .copy_from_slice(&scaled.as_raw()[source_start..source_start + scaled_width]);
    }

    Gray8Frame::new(width, height, pixels)
        .map_err(|error| format!("failed to build JPEG Gray8 frame: {error}"))
}

fn fit_dimensions(
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> (u32, u32) {
    let width_limited = u64::from(source_width) * u64::from(target_height)
        > u64::from(target_width) * u64::from(source_height);
    if width_limited {
        let scaled_height =
            (u64::from(source_height) * u64::from(target_width) / u64::from(source_width)).max(1);
        (
            target_width,
            u32::try_from(scaled_height).expect("scaled height is bounded by target height"),
        )
    } else {
        let scaled_width =
            (u64::from(source_width) * u64::from(target_height) / u64::from(source_height)).max(1);
        (
            u32::try_from(scaled_width).expect("scaled width is bounded by target width"),
            target_height,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ImageBuffer, Luma};

    const EXAMPLE_JPEG: &[u8] = include_bytes!("../assets/grayscale-example.jpg");

    fn black_jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = ImageBuffer::from_pixel(width, height, Luma([0_u8]));
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, 100)
            .encode_image(&DynamicImage::ImageLuma8(image))
            .expect("encode test JPEG");
        encoded
    }

    fn assert_white_rows(frame: &Gray8Frame, rows: std::ops::Range<usize>) {
        let width = usize::from(frame.width());
        for row in rows {
            assert!(
                frame.pixels()[row * width..(row + 1) * width]
                    .iter()
                    .all(|&pixel| pixel == u8::MAX)
            );
        }
    }

    #[test]
    fn fit_contain_handles_landscape_portrait_exact_small_and_odd_targets() {
        let landscape = decode_jpeg_fit_contain(&black_jpeg(4, 2), 5, 9).unwrap();
        assert_eq!((landscape.width(), landscape.height()), (5, 9));
        assert_white_rows(&landscape, 0..3);
        assert!(
            landscape.pixels()[3 * 5..5 * 5]
                .iter()
                .all(|&pixel| pixel < 16)
        );
        assert_white_rows(&landscape, 5..9);

        let portrait = decode_jpeg_fit_contain(&black_jpeg(2, 4), 7, 9).unwrap();
        for row in portrait.pixels().chunks_exact(7) {
            assert_eq!(row[0], u8::MAX);
            assert!(row[1..5].iter().all(|&pixel| pixel < 16));
            assert!(row[5..].iter().all(|&pixel| pixel == u8::MAX));
        }

        let exact = decode_jpeg_fit_contain(&black_jpeg(3, 2), 6, 4).unwrap();
        assert!(exact.pixels().iter().all(|&pixel| pixel < 16));

        let tiny = decode_jpeg_fit_contain(&black_jpeg(1, 1), 5, 3).unwrap();
        for row in tiny.pixels().chunks_exact(5) {
            assert_eq!(row[0], u8::MAX);
            assert!(row[1..4].iter().all(|&pixel| pixel < 16));
            assert_eq!(row[4], u8::MAX);
        }
    }

    #[test]
    fn project_fixture_decodes_to_centered_gray8_and_bad_jpeg_is_rejected() {
        let fixture_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/grayscale-example.jpg");
        let frame =
            decode_jpeg_path_fit_contain(&fixture_path, 12, 16).expect("decode fixture path");
        assert_eq!(frame.stride(), 12);
        assert_white_rows(&frame, 0..4);
        assert_white_rows(&frame, 12..16);
        assert!(
            frame.pixels()[4 * 12..12 * 12]
                .iter()
                .any(|&pixel| pixel < 64)
        );

        assert!(decode_jpeg_fit_contain(b"not a JPEG", 12, 16).is_err());
        assert!(decode_jpeg_fit_contain(EXAMPLE_JPEG, 0, 16).is_err());
        assert!(decode_jpeg_fit_contain(EXAMPLE_JPEG, u16::MAX, u16::MAX).is_err());
        assert!(
            decode_jpeg_path_fit_contain(Path::new("missing-grayscale-example.jpg"), 12, 16)
                .is_err()
        );
    }
}
