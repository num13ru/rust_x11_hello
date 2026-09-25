//! Adapt wire `Mono1` pixels to the X11 server's native XYBitmap layout.
//!
//! X11 describes bitmap rows with three independent setup values: bit order
//! inside each scanline unit, byte order for serializing that unit, and row
//! padding. Keeping this conversion separate prevents the stable wire format
//! from accidentally depending on one X server's representation.

use anyhow::{Context, Result, ensure};
use paper_protocol::validate_mono1_pixels;
use std::fmt;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{ConnectionExt, Drawable, Gcontext, ImageFormat, ImageOrder};
use x11rb::rust_connection::RustConnection;

/// Bytes preceding image data in a core X11 `PutImage` request.
const PUT_IMAGE_REQUEST_HEADER_BYTES: usize = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BitmapFormat {
    image_byte_order: ImageOrder,
    bitmap_bit_order: ImageOrder,
    scanline_unit_bits: usize,
    scanline_pad_bits: usize,
}

impl BitmapFormat {
    fn new(
        image_byte_order: ImageOrder,
        bitmap_bit_order: ImageOrder,
        scanline_unit_bits: u8,
        scanline_pad_bits: u8,
    ) -> Result<Self> {
        ensure!(
            matches!(
                image_byte_order,
                ImageOrder::LSB_FIRST | ImageOrder::MSB_FIRST
            ),
            "unsupported X11 image byte order {image_byte_order:?}"
        );
        ensure!(
            matches!(
                bitmap_bit_order,
                ImageOrder::LSB_FIRST | ImageOrder::MSB_FIRST
            ),
            "unsupported X11 bitmap bit order {bitmap_bit_order:?}"
        );
        ensure!(
            matches!(scanline_unit_bits, 8 | 16 | 32),
            "unsupported X11 bitmap scanline unit {scanline_unit_bits} bits"
        );
        ensure!(
            matches!(scanline_pad_bits, 8 | 16 | 32),
            "unsupported X11 bitmap scanline padding {scanline_pad_bits} bits"
        );
        ensure!(
            scanline_unit_bits <= scanline_pad_bits,
            "X11 bitmap scanline unit {scanline_unit_bits} exceeds padding {scanline_pad_bits}"
        );

        Ok(Self {
            image_byte_order,
            bitmap_bit_order,
            scanline_unit_bits: usize::from(scanline_unit_bits),
            scanline_pad_bits: usize::from(scanline_pad_bits),
        })
    }

    fn row_stride(self, width: u16) -> usize {
        usize::from(width).div_ceil(self.scanline_pad_bits) * self.scanline_pad_bits / 8
    }
}

/// Converts complete Mono1 frames and precomputes safe `PutImage` row chunks.
pub(super) struct X11BitmapAdapter {
    format: BitmapFormat,
    maximum_request_bytes: usize,
}

impl X11BitmapAdapter {
    pub(super) fn from_connection(conn: &RustConnection) -> Result<Self> {
        let setup = conn.setup();
        Self::new(
            setup.image_byte_order,
            setup.bitmap_format_bit_order,
            setup.bitmap_format_scanline_unit,
            setup.bitmap_format_scanline_pad,
            conn.maximum_request_bytes(),
        )
    }

    fn new(
        image_byte_order: ImageOrder,
        bitmap_bit_order: ImageOrder,
        scanline_unit_bits: u8,
        scanline_pad_bits: u8,
        maximum_request_bytes: usize,
    ) -> Result<Self> {
        let format = BitmapFormat::new(
            image_byte_order,
            bitmap_bit_order,
            scanline_unit_bits,
            scanline_pad_bits,
        )?;
        ensure!(
            maximum_request_bytes > PUT_IMAGE_REQUEST_HEADER_BYTES,
            "X11 maximum request size {maximum_request_bytes} cannot hold PutImage header"
        );
        Ok(Self {
            format,
            maximum_request_bytes,
        })
    }

    pub(super) fn prepare(&self, width: u16, height: u16, pixels: &[u8]) -> Result<PreparedBitmap> {
        let source_stride = validate_mono1_pixels(width, height, pixels)?;
        let stride = self.format.row_stride(width);
        let maximum_data_bytes =
            (self.maximum_request_bytes - PUT_IMAGE_REQUEST_HEADER_BYTES) / 4 * 4;
        let rows_per_chunk = maximum_data_bytes / stride;
        ensure!(
            rows_per_chunk > 0,
            "X11 maximum request payload {maximum_data_bytes} cannot hold one {stride}-byte bitmap row"
        );
        let rows_per_chunk = rows_per_chunk.min(usize::from(u16::MAX));
        let mut encoded = vec![0; stride * usize::from(height)];
        let unit_bytes = self.format.scanline_unit_bits / 8;
        let units_per_row = stride / unit_bytes;

        for row in 0..usize::from(height) {
            let source_row = &pixels[row * source_stride..(row + 1) * source_stride];
            let encoded_row = &mut encoded[row * stride..(row + 1) * stride];
            for unit_index in 0..units_per_row {
                let first_pixel = unit_index * self.format.scanline_unit_bits;
                let unit = encode_unit(
                    source_row,
                    usize::from(width),
                    first_pixel,
                    self.format.scanline_unit_bits,
                    self.format.bitmap_bit_order,
                );
                write_unit(
                    unit,
                    &mut encoded_row[unit_index * unit_bytes..(unit_index + 1) * unit_bytes],
                    self.format.image_byte_order,
                );
            }
        }

        Ok(PreparedBitmap {
            width,
            height,
            stride,
            rows_per_chunk,
            encoded,
        })
    }

    /// Upload a prepared bitmap using checked, depth-1 XYBitmap requests.
    ///
    /// The target GC maps set bits to its foreground pixel and clear bits to
    /// its background pixel. PaperPad's window GC uses black and white,
    /// respectively. All issued cookies are checked even if one fails, keeping
    /// X11 errors from leaking into the event loop as unrelated later events.
    pub(super) fn blit(
        &self,
        conn: &RustConnection,
        drawable: Drawable,
        gc: Gcontext,
        destination: (i16, i16),
        bitmap: &PreparedBitmap,
    ) -> Result<usize> {
        let mut cookies = Vec::new();
        let mut first_error = None;

        for (y_offset, rows, data) in bitmap.chunks() {
            let destination_y = match destination_y(destination.1, y_offset) {
                Ok(destination_y) => destination_y,
                Err(error) => {
                    first_error = Some(error);
                    break;
                }
            };
            match conn.put_image(
                ImageFormat::XY_BITMAP,
                drawable,
                gc,
                bitmap.width,
                rows,
                destination.0,
                destination_y,
                0,
                1,
                data,
            ) {
                Ok(cookie) => cookies.push((y_offset, cookie)),
                Err(error) => {
                    first_error = Some(
                        anyhow::Error::new(error)
                            .context(format!("failed to send bitmap rows starting at {y_offset}")),
                    );
                    break;
                }
            }
        }

        let request_count = cookies.len();
        for (y_offset, cookie) in cookies {
            if let Err(error) = cookie
                .check()
                .with_context(|| format!("X11 server rejected bitmap rows starting at {y_offset}"))
            {
                record_upload_error(&mut first_error, error);
            }
        }
        if let Err(error) = conn.flush().context("failed to flush bitmap upload") {
            record_upload_error(&mut first_error, error);
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(request_count),
        }
    }
}

impl fmt::Display for X11BitmapAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "image_byte_order={:?} bitmap_bit_order={:?} scanline_unit={} scanline_pad={} max_request_bytes={}",
            self.format.image_byte_order,
            self.format.bitmap_bit_order,
            self.format.scanline_unit_bits,
            self.format.scanline_pad_bits,
            self.maximum_request_bytes
        )
    }
}

pub(super) struct PreparedBitmap {
    width: u16,
    height: u16,
    stride: usize,
    rows_per_chunk: usize,
    encoded: Vec<u8>,
}

fn destination_y(origin: i16, row_offset: u16) -> Result<i16> {
    let y = i32::from(origin) + i32::from(row_offset);
    i16::try_from(y).context("bitmap chunk destination exceeds X11 coordinate range")
}

fn record_upload_error(first_error: &mut Option<anyhow::Error>, error: anyhow::Error) {
    if first_error.is_none() {
        *first_error = Some(error);
    } else {
        eprintln!("additional framebuffer upload error: {error:#}");
    }
}

impl PreparedBitmap {
    pub(super) fn stride(&self) -> usize {
        self.stride
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.encoded
    }

    /// Yield `(y_offset, row_count, bytes)` chunks that each fit PutImage.
    pub(super) fn chunks(&self) -> impl Iterator<Item = (u16, u16, &[u8])> {
        let height = usize::from(self.height);
        (0..height)
            .step_by(self.rows_per_chunk)
            .map(move |first_row| {
                let rows = self.rows_per_chunk.min(height - first_row);
                let byte_start = first_row * self.stride;
                let byte_end = byte_start + rows * self.stride;
                (
                    first_row as u16,
                    rows as u16,
                    &self.encoded[byte_start..byte_end],
                )
            })
    }
}

fn encode_unit(
    source_row: &[u8],
    width: usize,
    first_pixel: usize,
    unit_bits: usize,
    bit_order: ImageOrder,
) -> u32 {
    let mut encoded = 0_u32;
    for offset in 0..unit_bits {
        let x = first_pixel + offset;
        if x >= width || source_row[x / 8] & (0x80 >> (x % 8)) == 0 {
            continue;
        }
        let destination_bit = if bit_order == ImageOrder::LSB_FIRST {
            offset
        } else {
            unit_bits - 1 - offset
        };
        encoded |= 1_u32 << destination_bit;
    }
    encoded
}

fn write_unit(unit: u32, destination: &mut [u8], byte_order: ImageOrder) {
    let byte_count = destination.len();
    for (index, byte) in destination.iter_mut().enumerate() {
        let significance_index = if byte_order == ImageOrder::LSB_FIRST {
            index
        } else {
            byte_count - 1 - index
        };
        *byte = (unit >> (significance_index * 8)) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(
        image_byte_order: ImageOrder,
        bitmap_bit_order: ImageOrder,
        unit: u8,
        pad: u8,
        maximum_request_bytes: usize,
    ) -> X11BitmapAdapter {
        X11BitmapAdapter::new(
            image_byte_order,
            bitmap_bit_order,
            unit,
            pad,
            maximum_request_bytes,
        )
        .expect("supported bitmap setup")
    }

    #[test]
    fn matching_msb_eight_bit_format_preserves_wire_bytes() {
        let adapter = adapter(ImageOrder::MSB_FIRST, ImageOrder::MSB_FIRST, 8, 8, 4096);
        let bitmap = adapter
            .prepare(9, 2, &[0xaa, 0x80, 0x55, 0x00])
            .expect("prepare");
        assert_eq!(bitmap.stride(), 2);
        assert_eq!(bitmap.bytes(), &[0xaa, 0x80, 0x55, 0x00]);
    }

    #[test]
    fn lsb_bit_order_reverses_bits_within_eight_bit_units() {
        let adapter = adapter(ImageOrder::LSB_FIRST, ImageOrder::LSB_FIRST, 8, 8, 4096);
        let bitmap = adapter.prepare(9, 1, &[0xaa, 0x80]).expect("prepare");
        assert_eq!(bitmap.bytes(), &[0x55, 0x01]);
    }

    #[test]
    fn sixteen_bit_units_apply_bit_and_byte_order_independently() {
        let cases = [
            (ImageOrder::MSB_FIRST, ImageOrder::MSB_FIRST, [0x40, 0x00]),
            (ImageOrder::LSB_FIRST, ImageOrder::MSB_FIRST, [0x00, 0x40]),
            (ImageOrder::MSB_FIRST, ImageOrder::LSB_FIRST, [0x00, 0x02]),
            (ImageOrder::LSB_FIRST, ImageOrder::LSB_FIRST, [0x02, 0x00]),
        ];
        for (image_order, bit_order, expected) in cases {
            let adapter = adapter(image_order, bit_order, 16, 16, 4096);
            let bitmap = adapter.prepare(16, 1, &[0x40, 0x00]).expect("prepare");
            assert_eq!(bitmap.bytes(), &expected);
        }
    }

    #[test]
    fn thirty_two_bit_units_place_pixels_across_all_bytes() {
        let adapter = adapter(ImageOrder::LSB_FIRST, ImageOrder::MSB_FIRST, 32, 32, 4096);
        let bitmap = adapter
            .prepare(25, 1, &[0x80, 0x01, 0x80, 0x80])
            .expect("prepare");
        assert_eq!(bitmap.bytes(), &[0x80, 0x80, 0x01, 0x80]);
    }

    #[test]
    fn rows_are_zero_padded_to_server_scanline_boundary() {
        let adapter = adapter(ImageOrder::MSB_FIRST, ImageOrder::MSB_FIRST, 16, 32, 4096);
        let bitmap = adapter
            .prepare(9, 2, &[0xaa, 0x80, 0x55, 0x00])
            .expect("prepare");
        assert_eq!(bitmap.stride(), 4);
        assert_eq!(
            bitmap.bytes(),
            &[0xaa, 0x80, 0x00, 0x00, 0x55, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn chunks_are_whole_rows_bounded_by_maximum_request_size() {
        let adapter = adapter(
            ImageOrder::MSB_FIRST,
            ImageOrder::MSB_FIRST,
            8,
            8,
            PUT_IMAGE_REQUEST_HEADER_BYTES + 8,
        );
        let bitmap = adapter.prepare(17, 5, &[0x00; 15]).expect("prepare");
        let chunks: Vec<_> = bitmap
            .chunks()
            .map(|(y, rows, bytes)| (y, rows, bytes.len()))
            .collect();
        assert_eq!(chunks, vec![(0, 2, 6), (2, 2, 6), (4, 1, 3)]);
        assert!(chunks.iter().all(|(_, _, bytes)| {
            PUT_IMAGE_REQUEST_HEADER_BYTES + (*bytes).div_ceil(4) * 4
                <= PUT_IMAGE_REQUEST_HEADER_BYTES + 8
        }));
    }

    #[test]
    fn invalid_setup_frame_and_request_limits_are_rejected() {
        for (unit, pad) in [(0, 8), (64, 64), (16, 8)] {
            assert!(
                X11BitmapAdapter::new(
                    ImageOrder::MSB_FIRST,
                    ImageOrder::MSB_FIRST,
                    unit,
                    pad,
                    4096,
                )
                .is_err()
            );
        }
        for order in [ImageOrder::from(2), ImageOrder::from(u8::MAX)] {
            assert!(X11BitmapAdapter::new(order, ImageOrder::MSB_FIRST, 8, 8, 4096).is_err());
            assert!(X11BitmapAdapter::new(ImageOrder::MSB_FIRST, order, 8, 8, 4096).is_err());
        }
        assert!(
            X11BitmapAdapter::new(
                ImageOrder::MSB_FIRST,
                ImageOrder::MSB_FIRST,
                8,
                8,
                PUT_IMAGE_REQUEST_HEADER_BYTES,
            )
            .is_err()
        );

        let narrow_adapter = adapter(
            ImageOrder::MSB_FIRST,
            ImageOrder::MSB_FIRST,
            8,
            32,
            PUT_IMAGE_REQUEST_HEADER_BYTES + 3,
        );
        assert!(narrow_adapter.prepare(9, 1, &[0x00; 2]).is_err());
        assert!(narrow_adapter.prepare(9, 2, &[0x00; 3]).is_err());
        assert!(narrow_adapter.prepare(9, 1, &[0x00, 0x01]).is_err());

        let unaligned_limit = adapter(
            ImageOrder::MSB_FIRST,
            ImageOrder::MSB_FIRST,
            8,
            8,
            PUT_IMAGE_REQUEST_HEADER_BYTES + 7,
        );
        let bitmap = unaligned_limit
            .prepare(17, 2, &[0x00; 6])
            .expect("one padded row fits");
        assert_eq!(bitmap.chunks().count(), 2);
    }

    #[test]
    fn chunk_destination_y_is_checked_instead_of_wrapping() {
        assert_eq!(destination_y(0, 0).expect("origin"), 0);
        assert_eq!(destination_y(-10, 10).expect("offset"), 0);
        assert_eq!(
            destination_y(i16::MAX - 1, 1).expect("maximum coordinate"),
            i16::MAX
        );
        assert!(destination_y(i16::MAX, 1).is_err());
    }
}
