use std::io::{Read, Write};

use anyhow::{Context, Result, ensure};
use paper_protocol::{
    Frame, PixelFormat, V2_FRAME_PREFIX_LEN, V2_MAX_PAYLOAD_LEN, gray8_stride, mono1_stride,
};

const MAGIC: &[u8; 4] = b"FBCP";
const VERSION: u8 = 1;
const MAX_SCENARIO_LEN: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    pub format: PixelFormat,
    pub width: u16,
    pub height: u16,
    pub stride: usize,
    pub len: usize,
}

impl Layout {
    pub fn new(format: PixelFormat, width: u16, height: u16) -> Result<Self> {
        ensure!(
            width != 0 && height != 0,
            "frame dimensions must be nonzero"
        );
        let stride = match format {
            PixelFormat::Mono1 => mono1_stride(width),
            PixelFormat::Gray8 => gray8_stride(width),
        };
        let len = stride
            .checked_mul(usize::from(height))
            .context("frame size overflow")?;
        ensure!(
            len <= V2_MAX_PAYLOAD_LEN - V2_FRAME_PREFIX_LEN,
            "frame exceeds PPFB pixel limit"
        );
        Ok(Self {
            format,
            width,
            height,
            stride,
            len,
        })
    }

    #[cfg(feature = "scenarios")]
    pub fn matches(self, frame: &Frame) -> bool {
        self.format == frame.pixel_format()
            && self.width == frame.width()
            && self.height == frame.height()
    }

    pub fn frame(self, pixels: Vec<u8>) -> Result<Frame> {
        Frame::new(self.width, self.height, self.format, pixels)
            .context("invalid framebuffer pixels")
    }
}

pub struct Header {
    pub scenario: String,
    pub layout: Layout,
    pub frame_count: u32,
}

impl Header {
    pub fn read(input: &mut impl Read, magic: &[u8; 4]) -> Result<Self> {
        let mut bytes = [0; 20];
        input.read_exact(&mut bytes).context("read corpus header")?;
        ensure!(&bytes[..4] == magic, "invalid framebuffer file magic");
        ensure!(
            bytes[4] == VERSION,
            "unsupported corpus version {}",
            bytes[4]
        );
        let format = match bytes[5] {
            1 => PixelFormat::Mono1,
            2 => PixelFormat::Gray8,
            format => anyhow::bail!("unknown corpus pixel format {format}"),
        };
        let scenario_len = usize::from(bytes[6]);
        ensure!(
            (1..=MAX_SCENARIO_LEN).contains(&scenario_len),
            "invalid scenario length"
        );
        ensure!(bytes[7] == 0, "invalid reserved corpus header byte");
        let width = u16::from_le_bytes([bytes[8], bytes[9]]);
        let height = u16::from_le_bytes([bytes[10], bytes[11]]);
        let stride = u32::from_le_bytes(bytes[12..16].try_into()?);
        let frame_count = u32::from_le_bytes(bytes[16..20].try_into()?);
        let layout = Layout::new(format, width, height)?;
        ensure!(
            stride as usize == layout.stride,
            "noncanonical corpus stride"
        );
        ensure!(frame_count > 0, "corpus contains no frames");
        let mut scenario = vec![0; scenario_len];
        input
            .read_exact(&mut scenario)
            .context("read corpus scenario")?;
        let scenario = String::from_utf8(scenario).context("invalid scenario name")?;
        validate_scenario(&scenario)?;
        Ok(Self {
            scenario,
            layout,
            frame_count,
        })
    }

    pub fn write(&self, output: &mut impl Write, magic: &[u8; 4]) -> Result<()> {
        validate_scenario(&self.scenario)?;
        ensure!(self.frame_count > 0, "corpus contains no frames");
        let format = match self.layout.format {
            PixelFormat::Mono1 => 1,
            PixelFormat::Gray8 => 2,
        };
        output.write_all(magic)?;
        output.write_all(&[VERSION, format, self.scenario.len() as u8, 0])?;
        output.write_all(&self.layout.width.to_le_bytes())?;
        output.write_all(&self.layout.height.to_le_bytes())?;
        output.write_all(&(self.layout.stride as u32).to_le_bytes())?;
        output.write_all(&self.frame_count.to_le_bytes())?;
        output.write_all(self.scenario.as_bytes())?;
        Ok(())
    }
}

pub struct Sample {
    pub timestamp_us: u64,
    pub frame: Frame,
}

pub struct CorpusReader<R> {
    input: R,
    pub header: Header,
    index: u32,
    last_timestamp_us: u64,
    finished: bool,
}

impl<R: Read> CorpusReader<R> {
    pub fn new(mut input: R) -> Result<Self> {
        let header = Header::read(&mut input, MAGIC)?;
        Ok(Self {
            input,
            header,
            index: 0,
            last_timestamp_us: 0,
            finished: false,
        })
    }

    pub fn next_sample(&mut self) -> Result<Option<Sample>> {
        if self.index == self.header.frame_count {
            if !self.finished {
                let mut extra = [0];
                ensure!(
                    self.input.read(&mut extra)? == 0,
                    "trailing bytes after corpus frames"
                );
                self.finished = true;
            }
            return Ok(None);
        }
        let index = self.index;
        let mut record = [0; 16];
        self.input
            .read_exact(&mut record)
            .with_context(|| format!("read frame {index} header"))?;
        let actual_index = u32::from_le_bytes(record[..4].try_into()?);
        let timestamp_us = u64::from_le_bytes(record[4..12].try_into()?);
        let len = u32::from_le_bytes(record[12..].try_into()?) as usize;
        ensure!(
            actual_index == index,
            "expected frame {index}, got {actual_index}"
        );
        validate_timestamp(index, self.last_timestamp_us, timestamp_us)?;
        ensure!(
            len == self.header.layout.len,
            "invalid raw length in frame {index}"
        );
        let mut pixels = vec![0; len];
        self.input
            .read_exact(&mut pixels)
            .with_context(|| format!("read frame {index} pixels"))?;
        let frame = self
            .header
            .layout
            .frame(pixels)
            .with_context(|| format!("frame {index}"))?;
        self.index += 1;
        self.last_timestamp_us = timestamp_us;
        Ok(Some(Sample {
            timestamp_us,
            frame,
        }))
    }
}

#[cfg(feature = "scenarios")]
pub fn write_corpus(
    mut output: impl Write,
    header: &Header,
    mut sample_at: impl FnMut(u32) -> Result<Sample>,
) -> Result<()> {
    header.write(&mut output, MAGIC)?;

    let mut previous = 0;
    for index in 0..header.frame_count {
        let Sample {
            timestamp_us,
            frame,
        } = sample_at(index).with_context(|| format!("generate frame {index}"))?;
        validate_timestamp(index, previous, timestamp_us)?;
        ensure!(
            header.layout.matches(&frame),
            "unexpected framebuffer layout in frame {index}"
        );
        output.write_all(&index.to_le_bytes())?;
        output.write_all(&timestamp_us.to_le_bytes())?;
        output.write_all(&(frame.pixels().len() as u32).to_le_bytes())?;
        output.write_all(frame.pixels())?;
        previous = timestamp_us;
    }
    output.flush()?;
    Ok(())
}

fn validate_scenario(scenario: &str) -> Result<()> {
    ensure!(
        (1..=MAX_SCENARIO_LEN).contains(&scenario.len())
            && scenario
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "invalid corpus scenario name {scenario:?}"
    );
    Ok(())
}

pub fn validate_timestamp(index: u32, previous: u64, current: u64) -> Result<()> {
    ensure!(
        (index != 0 || current == 0) && current >= previous,
        "invalid relative timestamp in frame {index}"
    );
    Ok(())
}

#[cfg(all(test, feature = "scenarios"))]
mod tests {
    use super::*;

    fn corpus() -> Vec<u8> {
        let header = Header {
            scenario: "odd-mono".into(),
            layout: Layout::new(PixelFormat::Mono1, 10, 2).unwrap(),
            frame_count: 2,
        };
        let mut bytes = Vec::new();
        write_corpus(&mut bytes, &header, |index| {
            Ok(Sample {
                timestamp_us: u64::from(index) * 1000,
                frame: header.layout.frame(vec![0xff, 0xc0, index as u8, 0])?,
            })
        })
        .unwrap();
        bytes
    }

    #[test]
    fn round_trips_packed_pixels_and_timestamps() {
        let bytes = corpus();
        let mut reader = CorpusReader::new(bytes.as_slice()).unwrap();
        assert_eq!(reader.header.layout.stride, 2);
        for index in 0..2 {
            let sample = reader.next_sample().unwrap().unwrap();
            assert_eq!(sample.timestamp_us, index * 1000);
            assert_eq!(sample.frame.pixels(), &[0xff, 0xc0, index as u8, 0]);
        }
        assert!(reader.next_sample().unwrap().is_none());
    }

    #[test]
    fn rejects_bad_lengths_order_padding_and_extra_bytes() {
        let bytes = corpus();
        let record = 20 + "odd-mono".len();
        for (offset, value) in [
            (record, 1),
            (record + 4, 1),
            (record + 12, 255),
            (record + 17, 1),
        ] {
            let mut invalid = bytes.clone();
            invalid[offset] = value;
            assert!(
                CorpusReader::new(invalid.as_slice())
                    .unwrap()
                    .next_sample()
                    .is_err()
            );
        }
        let mut truncated = CorpusReader::new(&bytes[..bytes.len() - 1]).unwrap();
        assert!(truncated.next_sample().unwrap().is_some());
        assert!(truncated.next_sample().is_err());
        let mut extra = bytes;
        extra.push(0);
        let mut reader = CorpusReader::new(extra.as_slice()).unwrap();
        assert!(reader.next_sample().unwrap().is_some());
        assert!(reader.next_sample().unwrap().is_some());
        assert!(reader.next_sample().is_err());
    }

    #[test]
    fn rejects_oversized_layout_before_allocating_pixels() {
        assert!(Layout::new(PixelFormat::Gray8, u16::MAX, u16::MAX).is_err());
        let mut bytes = corpus();
        bytes[12] += 1;
        assert!(CorpusReader::new(bytes.as_slice()).is_err());
    }
}
