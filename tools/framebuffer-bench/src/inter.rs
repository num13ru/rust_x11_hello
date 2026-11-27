use std::str::FromStr;

use anyhow::{Context, Result, bail, ensure};
use paper_protocol::PixelFormat;

use crate::codec::{Codec, Decoder, Encoder};
use crate::corpus::Layout;

pub const KEYFRAME: u32 = u32::MAX;
pub const MODE_NAMES: &str = "xor, tiles, tiles-each, tiles-xor, rect, rects";
pub const TILE_SIZES: [u16; 4] = [16, 32, 64, 128];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Mode {
    Full,
    Xor,
    Tiles,
    TilesEach,
    TilesXor,
    Rect,
    Rects,
}

impl Mode {
    const TAGS: [&str; 7] = [
        "",
        "xor-",
        "tiles-",
        "tiles-each-",
        "tiles-xor-",
        "rect-",
        "rects-",
    ];
    const MODES: [Self; 7] = [
        Self::Full,
        Self::Xor,
        Self::Tiles,
        Self::TilesEach,
        Self::TilesXor,
        Self::Rect,
        Self::Rects,
    ];

    pub fn from_id(id: u8) -> Result<Self> {
        Self::MODES
            .get(usize::from(id))
            .copied()
            .with_context(|| format!("unknown encoding mode {id}"))
    }

    pub const fn tiled(self) -> bool {
        matches!(self, Self::Tiles | Self::TilesEach | Self::TilesXor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Encoding {
    pub mode: Mode,
    pub codec: Codec,
}

impl Encoding {
    pub fn new(mode: Mode, codec: Codec) -> Result<Self> {
        ensure!(
            mode == Mode::Full || codec != Codec::Png,
            "PNG requires an independent framebuffer"
        );
        Ok(Self { mode, codec })
    }

    pub fn name(self) -> String {
        format!("{}{}", Mode::TAGS[self.mode as usize], self.codec.name())
    }

    pub fn tile_width(self, layout: Layout, size: u16) -> usize {
        usize::from(size)
            / if layout.format == PixelFormat::Mono1 {
                8
            } else {
                1
            }
    }

    pub fn decoded_limit(self, layout: Layout, size: u16) -> usize {
        let patch_count = match self.mode {
            Mode::Full | Mode::Xor => 0,
            Mode::Rect => 1,
            Mode::Rects => usize::from(layout.height),
            _ => {
                layout.stride.div_ceil(self.tile_width(layout, size))
                    * usize::from(layout.height).div_ceil(usize::from(size))
            }
        };
        layout.len + 8 * patch_count
    }
}

impl FromStr for Encoding {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        for (index, tag) in Mode::TAGS.iter().enumerate().rev() {
            if let Some(codec) = value.strip_prefix(tag) {
                return Self::new(Mode::MODES[index], codec.parse()?);
            }
        }
        bail!("unknown encoding {value:?}")
    }
}

pub struct SequenceEncoder {
    encoding: Encoding,
    codec: Encoder,
    layout: Layout,
    tile_size: u16,
    patches: Vec<Patch>,
    plain: Vec<u8>,
    chunk: Vec<u8>,
    compressed: Vec<u8>,
}

impl SequenceEncoder {
    pub fn new(encoding: Encoding, layout: Layout, tile_size: u16) -> Result<Self> {
        Ok(Self {
            encoding,
            layout,
            tile_size,
            codec: Encoder::new(encoding.codec)?,
            patches: Vec::new(),
            plain: Vec::new(),
            chunk: Vec::new(),
            compressed: Vec::new(),
        })
    }

    pub fn release_buffers(&mut self) {
        self.codec.release_buffers();
        self.patches = Vec::new();
        self.plain = Vec::new();
        self.chunk = Vec::new();
        self.compressed = Vec::new();
    }

    pub fn encode(
        &mut self,
        pixels: &[u8],
        base: Option<&[u8]>,
        output: &mut Vec<u8>,
    ) -> Result<usize> {
        let Some(base) = base else {
            self.codec.encode(pixels, output, self.layout)?;
            return Ok(pixels.len());
        };
        self.plain.clear();
        match self.encoding.mode {
            Mode::Full => bail!("independent encoding received a base frame"),
            Mode::Xor => self
                .plain
                .extend(pixels.iter().zip(base).map(|(a, b)| a ^ b)),
            mode => {
                self.patches.clear();
                match mode {
                    Mode::Rect | Mode::Rects => dirty_rects(
                        self.layout,
                        base,
                        pixels,
                        mode == Mode::Rect,
                        &mut self.patches,
                    ),
                    _ => dirty_tiles(
                        self.layout,
                        base,
                        pixels,
                        self.encoding.tile_width(self.layout, self.tile_size),
                        usize::from(self.tile_size),
                        &mut self.patches,
                    ),
                }
                if mode == Mode::TilesEach {
                    output.clear();
                }
                let mut plain_len = 0;
                for patch in &self.patches {
                    if mode == Mode::TilesEach {
                        patch.write(output);
                        self.chunk.clear();
                        patch.extract(self.layout, pixels, None, &mut self.chunk);
                        self.codec
                            .encode(&self.chunk, &mut self.compressed, self.layout)?;
                        output.extend_from_slice(
                            &u32::try_from(self.compressed.len())?.to_le_bytes(),
                        );
                        output.extend_from_slice(&self.compressed);
                        plain_len += 8 + self.chunk.len();
                    } else {
                        patch.write(&mut self.plain);
                        let xor_base = (mode == Mode::TilesXor).then_some(base);
                        patch.extract(self.layout, pixels, xor_base, &mut self.plain);
                    }
                }
                if mode == Mode::TilesEach {
                    return Ok(plain_len);
                }
            }
        }
        if self.plain.is_empty() {
            output.clear();
        } else {
            self.codec.encode(&self.plain, output, self.layout)?;
        }
        Ok(self.plain.len())
    }
}

pub struct SequenceDecoder {
    encoding: Encoding,
    codec: Decoder,
    layout: Layout,
    tile_size: u16,
    scratch: Vec<u8>,
    tile: Vec<u8>,
}

impl SequenceDecoder {
    pub fn new(encoding: Encoding, layout: Layout, tile_size: u16) -> Result<Self> {
        Ok(Self {
            encoding,
            layout,
            tile_size,
            codec: Decoder::new(encoding.codec)?,
            scratch: Vec::new(),
            tile: Vec::new(),
        })
    }

    pub fn release_buffers(&mut self) {
        self.scratch = Vec::new();
        self.tile = Vec::new();
    }

    pub fn decode(
        &mut self,
        payload: &[u8],
        plain_len: usize,
        base: Option<&[u8]>,
        output: &mut Vec<u8>,
    ) -> Result<()> {
        let layout = self.layout;
        let Some(base) = base else {
            ensure!(plain_len == layout.len, "invalid keyframe decoded length");
            return self.codec.decode(payload, output, plain_len, layout);
        };
        let mode = self.encoding.mode;
        ensure!(mode != Mode::Full, "independent encoding has a delta");
        ensure!(
            plain_len <= self.encoding.decoded_limit(layout, self.tile_size),
            "oversized delta decoded length"
        );
        if mode == Mode::Xor {
            ensure!(plain_len == layout.len, "incorrect XOR length");
            self.codec.decode(payload, output, plain_len, layout)?;
            for (pixel, base) in output.iter_mut().zip(base) {
                *pixel ^= base;
            }
            return Ok(());
        }

        output.clear();
        output.extend_from_slice(base);
        if plain_len == 0 {
            ensure!(payload.is_empty(), "nonempty payload for empty patch set");
            return Ok(());
        }
        let separate = mode == Mode::TilesEach;
        let mut patches = if separate {
            payload
        } else {
            self.codec
                .decode(payload, &mut self.scratch, plain_len, layout)?;
            self.scratch.as_slice()
        };
        let mut previous: Option<Patch> = None;
        let mut consumed = 0;
        while !patches.is_empty() {
            let patch = Patch::read(&mut patches, layout)?;
            patch.validate(previous, self.encoding, layout, self.tile_size)?;
            let len = patch.width * patch.height;
            consumed += 8 + len;
            ensure!(
                consumed <= plain_len,
                "patch bytes exceed declared decoded length"
            );
            if separate {
                let len_bytes: [u8; 4] = take(&mut patches, 4)?.try_into()?;
                let encoded = take(&mut patches, u32::from_le_bytes(len_bytes) as usize)?;
                self.codec.decode(encoded, &mut self.tile, len, layout)?;
                patch.apply(layout, &self.tile, output, false);
            } else {
                patch.apply(
                    layout,
                    take(&mut patches, len)?,
                    output,
                    mode == Mode::TilesXor,
                );
            }
            previous = Some(patch);
        }
        ensure!(consumed == plain_len, "incorrect patch decoded length");
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Patch {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl Patch {
    fn range(self, row: usize, layout: Layout) -> std::ops::Range<usize> {
        let start = (self.y + row) * layout.stride + self.x;
        start..start + self.width
    }

    fn write(self, output: &mut Vec<u8>) {
        for value in [self.x, self.y, self.width, self.height] {
            output.extend_from_slice(&(value as u16).to_le_bytes());
        }
    }

    fn read(input: &mut &[u8], layout: Layout) -> Result<Self> {
        let bytes = take(input, 8)?;
        let field =
            |offset: usize| usize::from(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]));
        let patch = Self {
            x: field(0),
            y: field(2),
            width: field(4),
            height: field(6),
        };
        ensure!(
            patch.width != 0
                && patch.height != 0
                && patch.x + patch.width <= layout.stride
                && patch.y + patch.height <= usize::from(layout.height),
            "patch lies outside framebuffer"
        );
        Ok(patch)
    }

    fn validate(
        self,
        previous: Option<Self>,
        encoding: Encoding,
        layout: Layout,
        size: u16,
    ) -> Result<()> {
        if encoding.mode.tiled() {
            let width = encoding.tile_width(layout, size);
            let height = usize::from(size);
            ensure!(
                self.x.is_multiple_of(width)
                    && self.y.is_multiple_of(height)
                    && self.width == width.min(layout.stride - self.x)
                    && self.height == height.min(usize::from(layout.height) - self.y),
                "invalid tile geometry"
            );
            ensure!(
                previous.is_none_or(|p| (self.y, self.x) > (p.y, p.x)),
                "duplicate or unordered tile"
            );
        } else {
            ensure!(
                previous.is_none_or(|p| encoding.mode == Mode::Rects && self.y >= p.y + p.height),
                "duplicate, overlapping, or unordered rectangle"
            );
        }
        Ok(())
    }

    fn extract(self, layout: Layout, pixels: &[u8], xor_base: Option<&[u8]>, output: &mut Vec<u8>) {
        for row in 0..self.height {
            let span = self.range(row, layout);
            if let Some(base) = xor_base {
                output.extend(
                    pixels[span.clone()]
                        .iter()
                        .zip(&base[span])
                        .map(|(a, b)| a ^ b),
                );
            } else {
                output.extend_from_slice(&pixels[span]);
            }
        }
    }

    fn apply(self, layout: Layout, pixels: &[u8], output: &mut [u8], xor: bool) {
        for (row, bytes) in pixels.chunks_exact(self.width).enumerate() {
            let target = &mut output[self.range(row, layout)];
            if xor {
                for (a, b) in target.iter_mut().zip(bytes) {
                    *a ^= b;
                }
            } else {
                target.copy_from_slice(bytes);
            }
        }
    }
}

fn take<'a>(input: &mut &'a [u8], len: usize) -> Result<&'a [u8]> {
    let (head, tail) = input
        .split_at_checked(len)
        .context("truncated patch data")?;
    *input = tail;
    Ok(head)
}

fn dirty_tiles(
    layout: Layout,
    base: &[u8],
    pixels: &[u8],
    tile_width: usize,
    tile_height: usize,
    output: &mut Vec<Patch>,
) {
    let height = usize::from(layout.height);
    for y in (0..height).step_by(tile_height) {
        for x in (0..layout.stride).step_by(tile_width) {
            let patch = Patch {
                x,
                y,
                width: tile_width.min(layout.stride - x),
                height: tile_height.min(height - y),
            };
            if (0..patch.height).any(|row| {
                let region = patch.range(row, layout);
                pixels[region.clone()] != base[region]
            }) {
                output.push(patch);
            }
        }
    }
}

fn dirty_rects(
    layout: Layout,
    base: &[u8],
    pixels: &[u8],
    bounding: bool,
    output: &mut Vec<Patch>,
) {
    for (y, (old, new)) in base
        .chunks_exact(layout.stride)
        .zip(pixels.chunks_exact(layout.stride))
        .enumerate()
    {
        let mut changes = old.iter().zip(new).map(|(a, b)| a != b);
        let Some(x) = changes.position(|different| different) else {
            continue;
        };
        let width = changes
            .rposition(|different| different)
            .map_or(1, |end| end + 2);
        if let Some(last) = output.last_mut() {
            if bounding {
                let right = (last.x + last.width).max(x + width);
                last.x = last.x.min(x);
                last.width = right - last.x;
                last.height = y + 1 - last.y;
                continue;
            }
            if (last.x, last.width, last.y + last.height) == (x, width, y) {
                last.height += 1;
                continue;
            }
        }
        output.push(Patch {
            x,
            y,
            width,
            height: 1,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_delta_modes_reconstruct_edges_and_keyframe_resets() {
        let layout = Layout::new(PixelFormat::Mono1, 67, 37).unwrap();
        let white = vec![0; layout.len];
        let mut marked = white.clone();
        marked[0] = 0x80;
        marked[layout.len - 1] = 0x20;
        let mut replacement = vec![0xff; layout.len];
        for row in replacement.chunks_exact_mut(layout.stride) {
            row[layout.stride - 1] = 0xe0;
        }
        for size in TILE_SIZES {
            for mode in Mode::MODES[1..].iter().copied() {
                for codec in [
                    Codec::Raw,
                    Codec::Rle,
                    Codec::Lz4,
                    Codec::Zstd(1),
                    Codec::Deflate(1),
                ] {
                    let encoding = Encoding::new(mode, codec).unwrap();
                    let mut encoder = SequenceEncoder::new(encoding, layout, size).unwrap();
                    let mut decoder = SequenceDecoder::new(encoding, layout, size).unwrap();
                    let mut payload = Vec::new();
                    let mut output = Vec::new();
                    for (pixels, base) in [
                        (white.as_slice(), None),
                        (&white, Some(white.as_slice())),
                        (&marked, Some(white.as_slice())),
                        (&replacement, Some(marked.as_slice())),
                        (&marked, None),
                    ] {
                        let len = encoder.encode(pixels, base, &mut payload).unwrap();
                        if mode != Mode::Xor && base == Some(pixels) {
                            assert!(payload.is_empty());
                        }
                        decoder.decode(&payload, len, base, &mut output).unwrap();
                        assert_eq!(output, pixels, "{mode:?} {codec:?} {size}");
                    }
                }
            }
        }
    }

    #[test]
    fn malformed_tiles_do_not_modify_the_base_and_keyframe_recovers() {
        let layout = Layout::new(PixelFormat::Mono1, 32, 2).unwrap();
        let encoding = Encoding::new(Mode::Tiles, Codec::Raw).unwrap();
        let mut decoder = SequenceDecoder::new(encoding, layout, 16).unwrap();
        let base = [0; 8];
        let mut output = vec![];
        let patch = [0, 0, 0, 0, 2, 0, 2, 0, 0x80, 0, 0, 0];
        let mut bad = patch;
        bad[0] = 3;
        assert!(
            decoder
                .decode(&bad, bad.len(), Some(&base), &mut output)
                .is_err()
        );
        let mut duplicates = patch.repeat(2);
        assert!(
            decoder
                .decode(&duplicates, duplicates.len(), Some(&base), &mut output)
                .is_err()
        );
        duplicates.truncate(10);
        assert!(
            decoder
                .decode(&duplicates, duplicates.len(), Some(&base), &mut output)
                .is_err()
        );
        assert_eq!(base, [0; 8]);
        decoder
            .decode(&base, base.len(), None, &mut output)
            .unwrap();
        assert_eq!(output, base);
    }
}
