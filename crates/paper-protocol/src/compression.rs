//! Independent Zstandard frames with bounded reconstruction.

use std::io::{self, ErrorKind};

use zstd::bulk::{Compressor, Decompressor};
use zstd::zstd_safe::{self, FrameHeaderStatus};

/// Largest accepted Zstandard back-reference window, in bytes.
pub const MAX_ZSTD_WINDOW: u64 = 16 * 1024 * 1024;
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

pub struct ZstdEncoder(Compressor<'static>);

impl ZstdEncoder {
    pub fn new(level: i32) -> io::Result<Self> {
        if !matches!(level, 1 | 3 | 6) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "supported Zstandard levels are 1, 3, and 6",
            ));
        }
        Compressor::new(level).map(Self)
    }

    /// Compress a complete byte buffer into one independent frame.
    pub fn encode(&mut self, input: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
        output.clear();
        output
            .try_reserve(zstd_safe::compress_bound(input.len()))
            .map_err(io::Error::other)?;
        self.0.compress_to_buffer(input, output)?;
        Ok(())
    }

    /// Return compressed bytes when the complete message is strictly smaller.
    /// A caller can send its raw bytes on `None` or after handling a codec error.
    pub fn encode_if_smaller<'a>(
        &mut self,
        raw: &[u8],
        raw_header_bytes: usize,
        compressed_header_bytes: usize,
        scratch: &'a mut Vec<u8>,
    ) -> io::Result<Option<&'a [u8]>> {
        self.encode(raw, scratch)?;
        let raw_len = raw
            .len()
            .checked_add(raw_header_bytes)
            .ok_or(io::Error::new(
                ErrorKind::InvalidInput,
                "raw message size overflow",
            ))?;
        Ok(scratch
            .len()
            .checked_add(compressed_header_bytes)
            .filter(|&len| len < raw_len)
            .map(|_| scratch.as_slice()))
    }
}

pub struct ZstdDecoder(Decompressor<'static>);

impl ZstdDecoder {
    pub fn new() -> io::Result<Self> {
        Decompressor::new().map(Self)
    }

    /// `expected` must come from a validated layout. Use scratch output that
    /// can be discarded after an error. Validate pixel format and padding
    /// before handing the decoded buffer to presentation.
    pub fn decode(
        &mut self,
        input: &[u8],
        expected: usize,
        output: &mut Vec<u8>,
    ) -> io::Result<()> {
        validate_frame(input, expected)?;
        output.clear();
        output
            .try_reserve_exact(expected)
            .map_err(io::Error::other)?;
        output.resize(expected, 0);
        let written = self.0.decompress_to_buffer(input, output.as_mut_slice())?;
        if written != expected {
            return Err(invalid(
                "Zstandard decompressed length does not match frame layout",
            ));
        }
        Ok(())
    }
}

fn validate_frame(input: &[u8], expected: usize) -> io::Result<()> {
    if !input.starts_with(&ZSTD_MAGIC) {
        return Err(invalid("expected a standard Zstandard frame"));
    }
    let FrameHeaderStatus::Complete(header) =
        zstd_safe::get_frame_header(input).map_err(codec_error)?
    else {
        return Err(invalid("truncated Zstandard header"));
    };
    if header.windowSize > MAX_ZSTD_WINDOW {
        return Err(invalid("Zstandard window exceeds the 16 MiB limit"));
    }
    if header.dictID != 0 {
        return Err(invalid("Zstandard frame requires a dictionary"));
    }
    if header.frameContentSize != expected as u64 {
        return Err(invalid(
            "Zstandard content size does not match frame layout",
        ));
    }
    if zstd_safe::find_frame_compressed_size(input).map_err(codec_error)? != input.len() {
        return Err(invalid("trailing bytes after Zstandard frame"));
    }
    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}

fn codec_error(code: zstd_safe::ErrorCode) -> io::Error {
    invalid(zstd_safe::get_error_name(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reused_contexts_preserve_independent_frames_at_all_levels() {
        for level in [1, 3, 6] {
            let mut encoder = ZstdEncoder::new(level).unwrap();
            let mut decoder = ZstdDecoder::new().unwrap();
            let mut first = Vec::new();
            let mut second = Vec::new();
            let mut output = Vec::new();
            encoder.encode(&[5; 2048], &mut first).unwrap();
            encoder.encode(&[10; 4096], &mut second).unwrap();
            decoder.decode(&second, 4096, &mut output).unwrap();
            assert_eq!(output, [10; 4096]);
            decoder.decode(&first, 2048, &mut output).unwrap();
            assert_eq!(output, [5; 2048]);
        }
    }

    #[test]
    fn selection_includes_framing_and_avoids_expansion() {
        let mut encoder = ZstdEncoder::new(1).unwrap();
        let mut scratch = Vec::new();
        let solid = [15; 1024];
        let compressed = encoder
            .encode_if_smaller(&solid, 16, 20, &mut scratch)
            .unwrap()
            .unwrap();
        assert!(compressed.len() + 20 < solid.len() + 16);
        let tie = 16 + solid.len() - compressed.len();
        assert!(
            encoder
                .encode_if_smaller(&solid, 16, tie, &mut scratch)
                .unwrap()
                .is_none()
        );
        let mut noise = [0; 4096];
        let mut state = 0x5eed_u32;
        for byte in &mut noise {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state as u8;
        }
        assert!(
            encoder
                .encode_if_smaller(&noise, 16, 20, &mut scratch)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_invalid_metadata_and_recovers_with_a_complete_frame() {
        let mut encoder = ZstdEncoder::new(1).unwrap();
        let mut decoder = ZstdDecoder::new().unwrap();
        let mut payload = Vec::new();
        let mut output = vec![3, 2, 1];
        encoder.encode(&[7; 256], &mut payload).unwrap();
        let concat = payload.repeat(2);
        let mut trailing = payload.clone();
        trailing.push(0);
        let mut big_window = ZSTD_MAGIC.to_vec();
        big_window.extend([0x00, 0x78, 0x01, 0x00, 0x00]);
        let mut dictionary = ZSTD_MAGIC.to_vec();
        dictionary.extend([0x21, 0x01, 0x01, 0x09, 0x00, 0x00, 0x00]);
        for (bytes, expected, reason) in [
            (&payload[..payload.len() - 1], 256, "Src size"),
            (payload.as_slice(), 255, "content size"),
            (&concat, 256, "trailing bytes"),
            (&trailing, 256, "trailing bytes"),
            (&big_window, 256, "window exceeds"),
            (&dictionary, 256, "requires a dictionary"),
            (
                &[0x50, 0x2a, 0x4d, 0x18, 0, 0, 0, 0],
                0,
                "standard Zstandard frame",
            ),
        ] {
            let error = decoder.decode(bytes, expected, &mut output).unwrap_err();
            assert!(
                error
                    .to_string()
                    .to_lowercase()
                    .contains(&reason.to_lowercase()),
                "{error}"
            );
            assert_eq!(output, [3, 2, 1]);
        }
        decoder.decode(&payload, 256, &mut output).unwrap();
        assert_eq!(output, [7; 256]);
    }
}
