use std::io::{Cursor, Write};
use std::str::FromStr;

use anyhow::{Context, Result, bail, ensure};
use flate2::{Compression, Decompress, FlushDecompress, Status};
use paper_protocol::PixelFormat;
use paper_protocol::compression::{ZstdDecoder, ZstdEncoder};

use crate::corpus::Layout;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Codec {
    Raw,
    Rle,
    Lz4,
    Zstd(i32),
    Deflate(u32),
    Png,
}

impl Codec {
    pub const NAMES: [&str; 10] = [
        "raw",
        "rle",
        "lz4",
        "zstd-1",
        "zstd-3",
        "zstd-6",
        "deflate-fast",
        "deflate-default",
        "deflate-best",
        "png",
    ];

    const ALL: [Self; 10] = [
        Self::Raw,
        Self::Rle,
        Self::Lz4,
        Self::Zstd(1),
        Self::Zstd(3),
        Self::Zstd(6),
        Self::Deflate(1),
        Self::Deflate(6),
        Self::Deflate(9),
        Self::Png,
    ];

    pub fn id(self) -> u8 {
        Self::ALL
            .iter()
            .position(|&codec| codec == self)
            .expect("supported codec") as u8
    }

    pub fn from_id(id: u8) -> Result<Self> {
        Self::ALL
            .get(usize::from(id))
            .copied()
            .with_context(|| format!("unknown codec ID {id}"))
    }

    pub fn name(self) -> &'static str {
        Self::NAMES[usize::from(self.id())]
    }
}

impl FromStr for Codec {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::NAMES
            .iter()
            .position(|&name| name == value)
            .map(|index| Self::ALL[index])
            .with_context(|| {
                format!(
                    "unknown codec {value:?}, choose from {}",
                    Self::NAMES.join(", ")
                )
            })
    }
}

pub struct Encoder {
    codec: Codec,
    zstd: Option<ZstdEncoder>,
    scratch: Vec<u8>,
}

impl Encoder {
    pub fn new(codec: Codec) -> Result<Self> {
        let zstd = match codec {
            Codec::Zstd(level) => Some(ZstdEncoder::new(level)?),
            _ => None,
        };
        Ok(Self {
            codec,
            zstd,
            scratch: Vec::new(),
        })
    }

    pub fn release_buffers(&mut self) {
        self.scratch = Vec::new();
    }

    pub fn encode(&mut self, input: &[u8], output: &mut Vec<u8>, layout: Layout) -> Result<()> {
        output.clear();
        match self.codec {
            Codec::Raw => output.extend_from_slice(input),
            Codec::Rle => encode_rle(input, output),
            Codec::Lz4 => {
                output.resize(lz4_flex::block::get_maximum_output_size(input.len()), 0);
                let len = lz4_flex::block::compress_into(input, output)?;
                output.truncate(len);
            }
            Codec::Zstd(_) => self
                .zstd
                .as_mut()
                .context("missing Zstandard encoder")?
                .encode(input, output)?,
            Codec::Deflate(level) => {
                let mut encoder =
                    flate2::write::DeflateEncoder::new(output, Compression::new(level));
                encoder.write_all(input)?;
                encoder.finish()?;
            }
            Codec::Png => encode_png(input, output, layout, &mut self.scratch)?,
        }
        Ok(())
    }
}

pub struct Decoder {
    codec: Codec,
    zstd: Option<ZstdDecoder>,
}

impl Decoder {
    pub fn new(codec: Codec) -> Result<Self> {
        let zstd = if matches!(codec, Codec::Zstd(_)) {
            Some(ZstdDecoder::new()?)
        } else {
            None
        };
        Ok(Self { codec, zstd })
    }

    pub fn decode(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
        expected: usize,
        layout: Layout,
    ) -> Result<()> {
        output.clear();
        match self.codec {
            Codec::Raw => {
                ensure!(input.len() == expected, "incorrect raw length");
                output.extend_from_slice(input);
            }
            Codec::Rle => decode_rle(input, output, expected)?,
            Codec::Lz4 => {
                output.resize(expected, 0);
                let written = lz4_flex::block::decompress_into(input, output)?;
                ensure!(written == expected, "incorrect LZ4 decoded length");
            }
            Codec::Zstd(_) => self
                .zstd
                .as_mut()
                .context("missing Zstandard decoder")?
                .decode(input, expected, output)?,
            Codec::Deflate(_) => {
                output.resize(expected + 1, 0);
                let mut decoder = Decompress::new(false);
                let status = decoder.decompress(input, output, FlushDecompress::Finish)?;
                ensure!(
                    status == Status::StreamEnd && decoder.total_in() == input.len() as u64,
                    "invalid or incomplete Deflate stream"
                );
                ensure!(
                    decoder.total_out() == expected as u64,
                    "incorrect Deflate decoded length"
                );
                output.truncate(expected);
            }
            Codec::Png => {
                ensure!(
                    expected == layout.len,
                    "PNG requires a complete framebuffer"
                );
                decode_png(input, output, layout)?;
            }
        }
        Ok(())
    }
}

fn encode_rle(mut input: &[u8], output: &mut Vec<u8>) {
    while let Some(&value) = input.first() {
        let count = input
            .iter()
            .take(255)
            .take_while(|&&next| next == value)
            .count();
        output.extend_from_slice(&[count as u8, value]);
        input = &input[count..];
    }
}

fn decode_rle(input: &[u8], output: &mut Vec<u8>, expected: usize) -> Result<()> {
    ensure!(input.len().is_multiple_of(2), "incomplete RLE run");
    for group in input.chunks_exact(2) {
        let count = usize::from(group[0]);
        ensure!(
            count != 0 && count <= expected - output.len(),
            "invalid RLE run length"
        );
        output.resize(output.len() + count, group[1]);
    }
    ensure!(output.len() == expected, "incorrect RLE decoded length");
    Ok(())
}

fn png_depth(layout: Layout) -> png::BitDepth {
    match layout.format {
        PixelFormat::Mono1 => png::BitDepth::One,
        PixelFormat::Gray8 => png::BitDepth::Eight,
    }
}

fn encode_png(
    input: &[u8],
    output: &mut Vec<u8>,
    layout: Layout,
    scratch: &mut Vec<u8>,
) -> Result<()> {
    if input.len() != layout.len {
        bail!("PNG requires a complete framebuffer");
    }
    let input = if layout.format == PixelFormat::Mono1 {
        scratch.clear();
        scratch.extend(input.iter().map(|byte| !byte));
        scratch.as_slice()
    } else {
        input
    };
    let mut encoder = png::Encoder::new(output, u32::from(layout.width), u32::from(layout.height));
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png_depth(layout));
    let mut writer = encoder.write_header()?;
    writer.write_image_data(input)?;
    writer.finish()?;
    Ok(())
}

fn decode_png(input: &[u8], output: &mut Vec<u8>, layout: Layout) -> Result<()> {
    let decoder = png::Decoder::new_with_limits(
        Cursor::new(input),
        png::Limits {
            bytes: layout.len + 2 * 1024 * 1024,
        },
    );
    let mut reader = decoder.read_info()?;
    let info = reader.info();
    ensure!(
        info.width == u32::from(layout.width)
            && info.height == u32::from(layout.height)
            && info.color_type == png::ColorType::Grayscale
            && info.bit_depth == png_depth(layout)
            && info.animation_control.is_none(),
        "unexpected PNG geometry, format, or animation"
    );
    ensure!(
        reader.output_buffer_size() == Some(layout.len),
        "incorrect PNG decoded size"
    );
    output.resize(layout.len, 0);
    let frame = reader.next_frame(output)?;
    ensure!(
        frame.buffer_size() == layout.len,
        "incorrect PNG decoded length"
    );
    reader.finish()?;
    if layout.format == PixelFormat::Mono1 {
        for byte in output {
            *byte = !*byte;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_codecs_round_trip_packed_and_grayscale_pixels() {
        for (format, pixels) in [
            (PixelFormat::Mono1, vec![0x82, 0xc0, 0x5e, 0x40]),
            (
                PixelFormat::Gray8,
                (0..20).map(|pixel| pixel * 13).collect(),
            ),
        ] {
            let layout = Layout::new(format, 10, 2).unwrap();
            let mut encoded = Vec::new();
            let mut decoded = Vec::new();
            for codec in Codec::ALL {
                Encoder::new(codec)
                    .unwrap()
                    .encode(&pixels, &mut encoded, layout)
                    .unwrap();
                let mut decoder = Decoder::new(codec).unwrap();
                decoder
                    .decode(&encoded, &mut decoded, pixels.len(), layout)
                    .unwrap();
                assert_eq!(decoded, pixels, "{}", codec.name());
                assert!(
                    decoder
                        .decode(&encoded, &mut decoded, pixels.len() - 1, layout)
                        .is_err()
                );
                encoded.truncate(encoded.len() / 2);
                assert!(
                    decoder
                        .decode(&encoded, &mut decoded, pixels.len(), layout)
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn rle_splits_long_runs_and_rejects_zero_runs() {
        let mut output = Vec::new();
        encode_rle(&[b'A'; 256], &mut output);
        assert_eq!(output, [255, b'A', 1, b'A']);
        assert!(decode_rle(&[0, b'A'], &mut Vec::new(), 0).is_err());
    }
}
