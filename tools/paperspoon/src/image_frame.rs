//! Host-only JPEG decoding and deterministic Gray8 fit-contain rendering.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek};
use std::path::Path;

#[cfg(test)]
use std::io::Cursor;

use image::codecs::jpeg::JpegDecoder;
use image::{DynamicImage, ImageDecoder};
use paper_protocol::Gray8Frame;
use paperspoon::gray_renderer::GrayCanvas;

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
    // Validate the target before decoding potentially expensive external input.
    let mut canvas = GrayCanvas::new(width, height)?;
    let decoder = JpegDecoder::new(reader)
        .map_err(|error| format!("failed to decode JPEG header: {error}"))?;
    let decoded_bytes = decoder.total_bytes();
    if decoded_bytes > MAX_DECODED_JPEG_BYTES {
        return Err(format!(
            "decoded JPEG requires {decoded_bytes} bytes; maximum is {MAX_DECODED_JPEG_BYTES}"
        ));
    }
    let decoded = DynamicImage::from_decoder(decoder)
        .map_err(|error| format!("failed to decode JPEG pixels: {error}"))?;
    canvas.draw_image_fit_contain(&decoded, 0, 0, width, height)?;
    canvas.into_frame()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ImageBuffer, Luma};

    const EXAMPLE_JPEG: &[u8] = include_bytes!("../assets/grayscale-example.jpg");

    fn black_jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = ImageBuffer::from_pixel(width, height, Luma([0_u8]));
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 100)
            .encode_image(&DynamicImage::ImageLuma8(image))
            .expect("encode test JPEG");
        jpeg
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
    fn fit_contain_preserves_aspect_ratio_and_centers_with_integer_rounding() {
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
        let frame = decode_jpeg_path_fit_contain(&fixture_path, 12, 16).expect("decode fixture");
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
