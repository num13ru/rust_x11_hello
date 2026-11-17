use std::borrow::Cow;
use std::collections::TryReserveError;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use paper_protocol::Frame;

#[derive(Debug)]
pub enum ExportError {
    PixelCountOverflow {
        width: u16,
        height: u16,
    },
    Allocation {
        pixel_count: usize,
        source: TryReserveError,
    },
    DestinationExists {
        path: PathBuf,
    },
    Open {
        path: PathBuf,
        source: io::Error,
    },
    Encode {
        path: PathBuf,
        source: png::EncodingError,
    },
    Flush {
        path: PathBuf,
        source: io::Error,
    },
    Cleanup {
        path: PathBuf,
        write_error: String,
        source: io::Error,
    },
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PixelCountOverflow { width, height } => {
                write!(
                    formatter,
                    "source pixel count overflows for {width}x{height}"
                )
            }
            Self::Allocation {
                pixel_count,
                source,
            } => write!(
                formatter,
                "could not allocate {pixel_count} grayscale pixels for PNG export: {source}"
            ),
            Self::DestinationExists { path } => {
                write!(formatter, "refusing to overwrite {}", path.display())
            }
            Self::Open { path, source } => {
                write!(formatter, "could not create {}: {source}", path.display())
            }
            Self::Encode { path, source } => {
                write!(formatter, "could not encode {}: {source}", path.display())
            }
            Self::Flush { path, source } => {
                write!(formatter, "could not finish {}: {source}", path.display())
            }
            Self::Cleanup {
                path,
                write_error,
                source,
            } => write!(
                formatter,
                "{write_error}; also could not remove incomplete {}: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Allocation { source, .. } => Some(source),
            Self::Open { source, .. }
            | Self::Flush { source, .. }
            | Self::Cleanup { source, .. } => Some(source),
            Self::Encode { source, .. } => Some(source),
            Self::PixelCountOverflow { .. } | Self::DestinationExists { .. } => None,
        }
    }
}

pub fn source_png_filename(frame_id: u64, frame: &Frame) -> String {
    let format = match frame {
        Frame::Mono1(_) => "mono1",
        Frame::Gray8(_) => "gray8",
    };
    format!(
        "paperview-frame-{frame_id:020}-{format}-{}x{}.png",
        frame.width(),
        frame.height()
    )
}

pub fn export_source_png(frame: &Frame, path: &Path) -> Result<(), ExportError> {
    let pixels = source_luma(frame)?;
    let file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            return Err(ExportError::DestinationExists {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(ExportError::Open {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let mut output = BufWriter::new(file);
    let result = encode_png(frame, pixels.as_ref(), path, &mut output);
    if let Err(error) = result {
        let write_error = error.to_string();
        drop(output);
        return match fs::remove_file(path) {
            Ok(()) => Err(error),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Err(error),
            Err(source) => Err(ExportError::Cleanup {
                path: path.to_path_buf(),
                write_error,
                source,
            }),
        };
    }
    Ok(())
}

fn source_luma(frame: &Frame) -> Result<Cow<'_, [u8]>, ExportError> {
    match frame {
        Frame::Gray8(frame) => Ok(Cow::Borrowed(frame.pixels())),
        Frame::Mono1(frame) => {
            let width = frame.width();
            let height = frame.height();
            let pixel_count = usize::from(width)
                .checked_mul(usize::from(height))
                .ok_or(ExportError::PixelCountOverflow { width, height })?;
            let mut pixels = Vec::new();
            pixels
                .try_reserve_exact(pixel_count)
                .map_err(|source| ExportError::Allocation {
                    pixel_count,
                    source,
                })?;
            for row in frame.pixels().chunks_exact(frame.stride()) {
                for x in 0..usize::from(width) {
                    let byte = row[x / 8];
                    let mask = 0x80 >> (x % 8);
                    pixels.push(if byte & mask == 0 { 255 } else { 0 });
                }
            }
            debug_assert_eq!(pixels.len(), pixel_count);
            Ok(Cow::Owned(pixels))
        }
    }
}

fn encode_png(
    frame: &Frame,
    pixels: &[u8],
    path: &Path,
    output: &mut BufWriter<File>,
) -> Result<(), ExportError> {
    let mut encoder = png::Encoder::new(
        &mut *output,
        u32::from(frame.width()),
        u32::from(frame.height()),
    );
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|source| ExportError::Encode {
            path: path.to_path_buf(),
            source,
        })?;
    writer
        .write_image_data(pixels)
        .map_err(|source| ExportError::Encode {
            path: path.to_path_buf(),
            source,
        })?;
    writer.finish().map_err(|source| ExportError::Encode {
        path: path.to_path_buf(),
        source,
    })?;
    output.flush().map_err(|source| ExportError::Flush {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::io::BufReader;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use paper_protocol::{Gray8Frame, Mono1Frame};

    use super::*;

    static NEXT_PATH_ID: AtomicUsize = AtomicUsize::new(0);

    fn temporary_png_path(label: &str) -> PathBuf {
        let path_id = NEXT_PATH_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "paperview-{label}-{}-{path_id}.png",
            std::process::id()
        ))
    }

    fn decode_png(path: &Path) -> (png::OutputInfo, Vec<u8>) {
        let file = File::open(path).expect("open exported PNG");
        let decoder = png::Decoder::new(BufReader::new(file));
        let mut reader = decoder.read_info().expect("read PNG header");
        let mut pixels = vec![0; reader.output_buffer_size().expect("bounded PNG output")];
        let info = reader.next_frame(&mut pixels).expect("decode PNG pixels");
        pixels.truncate(info.buffer_size());
        (info, pixels)
    }

    #[test]
    fn gray8_export_preserves_dimensions_and_luminance_bytes() {
        let path = temporary_png_path("gray8");
        let values = vec![0, 1, 127, 128, 254, 255];
        let frame = Frame::Gray8(Gray8Frame::new(3, 2, values.clone()).expect("valid Gray8"));

        export_source_png(&frame, &path).expect("export Gray8 PNG");
        let (info, pixels) = decode_png(&path);
        fs::remove_file(&path).expect("remove test PNG");

        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!(info.color_type, png::ColorType::Grayscale);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        assert_eq!(pixels, values);
    }

    #[test]
    fn mono1_export_preserves_polarity_and_excludes_row_padding() {
        let path = temporary_png_path("mono1");
        let frame = Frame::Mono1(
            Mono1Frame::new(10, 2, vec![0x80, 0x00, 0x55, 0x40]).expect("valid odd-width Mono1"),
        );

        export_source_png(&frame, &path).expect("export Mono1 PNG");
        let (info, pixels) = decode_png(&path);
        fs::remove_file(&path).expect("remove test PNG");

        assert_eq!((info.width, info.height), (10, 2));
        assert_eq!(
            pixels,
            vec![
                0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 255, 0, 255, 0, 255, 0,
                255, 0,
            ]
        );
    }

    #[test]
    fn existing_destination_is_never_overwritten() {
        let path = temporary_png_path("no-overwrite");
        let black = Frame::Gray8(Gray8Frame::new(1, 1, vec![0]).expect("valid black frame"));
        let white = Frame::Gray8(Gray8Frame::new(1, 1, vec![255]).expect("valid white frame"));
        export_source_png(&black, &path).expect("export first PNG");

        let error = export_source_png(&white, &path).expect_err("overwrite must fail");
        assert!(matches!(error, ExportError::DestinationExists { .. }));
        let (_, pixels) = decode_png(&path);
        fs::remove_file(&path).expect("remove test PNG");
        assert_eq!(pixels, vec![0]);
    }

    #[test]
    fn nonexistent_destination_directory_fails_without_creating_parents() {
        let path_id = NEXT_PATH_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "paperview-missing-directory-{}-{path_id}",
            std::process::id()
        ));
        let path = directory.join("frame.png");
        let frame = Frame::Gray8(Gray8Frame::new(1, 1, vec![0]).expect("valid Gray8"));

        let error = export_source_png(&frame, &path).expect_err("missing directory must fail");

        assert!(matches!(
            error,
            ExportError::Open { source, .. } if source.kind() == io::ErrorKind::NotFound
        ));
        assert!(!directory.exists());
    }

    #[test]
    fn filename_is_deterministic_and_includes_source_identity() {
        let frame = Frame::Mono1(Mono1Frame::new(9, 7, vec![0; 2 * 7]).expect("valid Mono1"));
        assert_eq!(
            source_png_filename(42, &frame),
            "paperview-frame-00000000000000000042-mono1-9x7.png"
        );
    }
}
