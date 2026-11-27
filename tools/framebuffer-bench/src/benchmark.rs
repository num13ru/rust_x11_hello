use std::fs::File;
use std::hint::black_box;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use paper_protocol::{PixelFormat, validate_mono1_pixels};

use crate::codec::Codec;
use crate::corpus::{CorpusReader, Header, validate_timestamp};
use crate::inter::{Encoding, KEYFRAME, Mode, SequenceDecoder, SequenceEncoder, TILE_SIZES};

const MAGIC: &[u8; 4] = b"FBCE";
const RECORD_BYTES: usize = 28;
const MAX_SAMPLES: u64 = 1_000_000;

#[derive(Clone, Copy)]
pub struct Options {
    warmup: u32,
    iterations: u32,
    csv: bool,
    fresh_buffers: bool,
    tile_size: Option<u16>,
    keyframe: Option<u32>,
}

impl Options {
    pub fn parse(args: &[std::ffi::OsString]) -> Result<Self> {
        let mut result = Self {
            warmup: 0,
            iterations: 1,
            csv: false,
            fresh_buffers: false,
            tile_size: None,
            keyframe: None,
        };
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            let count = |argument: Option<&std::ffi::OsString>| -> Result<u32> {
                argument
                    .and_then(|argument| argument.to_str())
                    .and_then(|argument| argument.parse().ok())
                    .with_context(|| format!("{flag:?} requires a nonnegative integer"))
            };
            match flag.to_str() {
                Some("--warmup") => result.warmup = count(args.next())?,
                Some("--iterations") => result.iterations = count(args.next())?,
                Some("--csv") => result.csv = true,
                Some("--fresh-buffers") => result.fresh_buffers = true,
                Some("--tile-size") => result.tile_size = Some(count(args.next())?.try_into()?),
                Some("--keyframe") => result.keyframe = Some(count(args.next())?),
                _ => bail!("unknown benchmark option {flag:?}"),
            }
        }
        ensure!(
            result.iterations > 0,
            "at least one measurement iteration is required"
        );
        Ok(result)
    }

    fn passes(self, frames: u32) -> Result<u32> {
        let passes = self
            .warmup
            .checked_add(self.iterations)
            .context("too many iterations")?;
        ensure!(
            u64::from(passes) * u64::from(frames) <= MAX_SAMPLES,
            "too many frame samples"
        );
        Ok(passes)
    }
}

pub fn encode(
    corpus_path: &Path,
    encoding: Encoding,
    output: &Path,
    options: Options,
) -> Result<()> {
    ensure!(
        encoding.mode.tiled() || options.tile_size.is_none(),
        "--tile-size requires a tile encoding"
    );
    ensure!(
        encoding.mode != Mode::Full || options.keyframe.is_none(),
        "--keyframe requires a delta encoding"
    );
    let tile_size = options.tile_size.unwrap_or(32);
    ensure!(
        TILE_SIZES.contains(&tile_size),
        "tile size must be 16, 32, 64, or 128"
    );
    let keyframe = options.keyframe.unwrap_or(0);
    let source = open(corpus_path)?;
    let mut corpus = CorpusReader::new(BufReader::new(&source))?;
    let header = &corpus.header;
    let layout = header.layout;
    let frames = header.frame_count;
    let scenario = header.scenario.clone();
    let passes = options.passes(frames)?;
    let mut output_file = crate::stage_output(output)?;
    let mut writer = BufWriter::new(output_file.as_file_mut());
    header.write(&mut writer, MAGIC)?;
    writer.write_all(&[encoding.codec.id(), encoding.mode as u8])?;
    writer.write_all(&tile_size.to_le_bytes())?;

    let start = Instant::now();
    let mut encoder = SequenceEncoder::new(encoding, layout, tile_size)?;
    let init = start.elapsed();
    let mut decoder = SequenceDecoder::new(encoding, layout, tile_size)?;
    let mut stats = Stats::new(init);
    let mut encoded = Vec::new();
    let mut decoded = Vec::new();
    for pass in 0..passes {
        if pass != 0 {
            (&source).seek(SeekFrom::Start(0))?;
            corpus = CorpusReader::new(BufReader::new(&source))?;
            ensure!(
                corpus.header.layout == layout
                    && corpus.header.frame_count == frames
                    && corpus.header.scenario == scenario,
                "source corpus header changed"
            );
        }
        let mut previous = None::<paper_protocol::Frame>;
        for index in 0..frames {
            let start = Instant::now();
            let sample = corpus.next_sample()?.context("missing source frame")?;
            let load = start.elapsed();
            let pixels = sample.frame.pixels();
            let base = if encoding.mode == Mode::Full
                || (keyframe != 0 && index.is_multiple_of(keyframe))
            {
                None
            } else {
                previous.as_ref().map(paper_protocol::Frame::pixels)
            };
            if options.fresh_buffers {
                encoder.release_buffers();
                encoded = Vec::new();
            }
            let start = Instant::now();
            let plain_len = encoder
                .encode(black_box(pixels), base, &mut encoded)
                .with_context(|| format!("encode frame {index}"))?;
            let time = start.elapsed();
            decoder.decode(&encoded, plain_len, base, &mut decoded)?;
            ensure!(pixels == decoded, "decoded pixels differ in frame {index}");
            if pass >= options.warmup {
                stats.record(time, load, encoded.len());
            }
            if pass == options.warmup {
                writer.write_all(&index.to_le_bytes())?;
                writer.write_all(&sample.timestamp_us.to_le_bytes())?;
                writer.write_all(&u32::try_from(encoded.len())?.to_le_bytes())?;
                writer.write_all(&crc32fast::hash(pixels).to_le_bytes())?;
                writer.write_all(&u32::try_from(plain_len)?.to_le_bytes())?;
                let base_id = base.map_or(KEYFRAME, |_| index - 1);
                writer.write_all(&base_id.to_le_bytes())?;
                writer.write_all(&encoded)?;
            }
            previous = Some(sample.frame);
        }
        ensure!(corpus.next_sample()?.is_none(), "extra source frames");
    }
    writer.flush()?;
    drop(writer);
    crate::save_output(output_file, output)?;
    stats.report(
        "encode", &scenario, encoding, tile_size, layout.len, options,
    );
    Ok(())
}

pub fn decode(encoded_path: &Path, original_path: Option<&Path>, options: Options) -> Result<()> {
    let input = open(encoded_path)?;
    let mut stream = BufReader::new(&input);
    let header = Header::read(&mut stream, MAGIC)?;
    ensure!(
        options.tile_size.is_none() && options.keyframe.is_none(),
        "encoding options are only valid with encode"
    );
    let mut parameters = [0; 4];
    stream
        .read_exact(&mut parameters)
        .context("read encoding parameters")?;
    let encoding = Encoding::new(
        Mode::from_id(parameters[1])?,
        Codec::from_id(parameters[0])?,
    )?;
    let tile_size = u16::from_le_bytes([parameters[2], parameters[3]]);
    ensure!(
        TILE_SIZES.contains(&tile_size),
        "invalid tile size in encoded header"
    );
    let records_start = stream.stream_position()?;
    let layout = header.layout;
    let passes = options.passes(header.frame_count)?;
    let decoded_limit = encoding.decoded_limit(layout, tile_size);
    let encoded_limit = 2 * decoded_limit + 8 * (decoded_limit - layout.len) + 128 * 1024;
    let source = original_path.map(open).transpose()?;
    let start = Instant::now();
    let mut decoder = SequenceDecoder::new(encoding, layout, tile_size)?;
    let mut stats = Stats::new(start.elapsed());
    let mut encoded = Vec::new();
    let mut decoded = Vec::new();
    let mut previous = Vec::new();
    for pass in 0..passes {
        previous.clear();
        if pass != 0 {
            stream.seek(SeekFrom::Start(records_start))?;
        }
        let mut original = if let Some(source) = &source {
            let mut file = BufReader::new(source);
            file.seek(SeekFrom::Start(0))?;
            let reader = CorpusReader::new(file)?;
            ensure!(
                reader.header.scenario == header.scenario
                    && reader.header.layout == layout
                    && reader.header.frame_count == header.frame_count,
                "original corpus header differs"
            );
            Some(reader)
        } else {
            None
        };
        let mut previous_timestamp = 0;
        for index in 0..header.frame_count {
            let load_start = Instant::now();
            let mut record = [0; RECORD_BYTES];
            stream
                .read_exact(&mut record)
                .with_context(|| format!("read encoded frame {index} header"))?;
            let actual_index = u32::from_le_bytes(record[..4].try_into()?);
            let timestamp = u64::from_le_bytes(record[4..12].try_into()?);
            let len = u32::from_le_bytes(record[12..16].try_into()?) as usize;
            let checksum = u32::from_le_bytes(record[16..20].try_into()?);
            let plain_len = u32::from_le_bytes(record[20..24].try_into()?) as usize;
            let base_id = u32::from_le_bytes(record[24..28].try_into()?);
            ensure!(
                actual_index == index,
                "expected encoded frame {index}, got {actual_index}"
            );
            validate_timestamp(index, previous_timestamp, timestamp)?;
            ensure!(len <= encoded_limit, "oversized encoded frame {index}");
            encoded.resize(len, 0);
            stream
                .read_exact(&mut encoded)
                .with_context(|| format!("read frame {index} payload"))?;
            let load = load_start.elapsed();
            if options.fresh_buffers {
                decoder.release_buffers();
                decoded = Vec::new();
            }
            let start = Instant::now();
            let base = if base_id == KEYFRAME {
                None
            } else {
                ensure!(
                    index.checked_sub(1) == Some(base_id) && previous.len() == layout.len,
                    "invalid base for frame {index}"
                );
                Some(previous.as_slice())
            };
            decoder
                .decode(black_box(&encoded), plain_len, base, &mut decoded)
                .with_context(|| format!("decode frame {index}"))?;
            let time = start.elapsed();
            ensure!(
                crc32fast::hash(&decoded) == checksum,
                "checksum differs in frame {index}"
            );
            if layout.format == PixelFormat::Mono1 {
                validate_mono1_pixels(layout.width, layout.height, &decoded)?;
            }
            if let Some(original) = &mut original {
                let sample = original.next_sample()?.context("missing original frame")?;
                ensure!(
                    sample.timestamp_us == timestamp && sample.frame.pixels() == decoded,
                    "frame {index} differs from the original"
                );
            }
            if pass >= options.warmup {
                stats.record(time, load, len);
            }
            previous_timestamp = timestamp;
            if encoding.mode != Mode::Full {
                std::mem::swap(&mut previous, &mut decoded);
            }
        }
        ensure!(
            stream.read(&mut [0])? == 0,
            "trailing bytes after encoded frames"
        );
        if let Some(original) = &mut original {
            ensure!(original.next_sample()?.is_none(), "extra original frames");
        }
    }
    stats.report(
        "decode",
        &header.scenario,
        encoding,
        tile_size,
        layout.len,
        options,
    );
    Ok(())
}

fn open(path: &Path) -> Result<File> {
    File::open(path).with_context(|| format!("open {}", path.display()))
}

struct Stats {
    elapsed: Vec<Duration>,
    sizes: Vec<usize>,
    init: Duration,
    load: Duration,
    initial_usage: Option<(f64, u64)>,
}

impl Stats {
    fn new(init: Duration) -> Self {
        Self {
            elapsed: Vec::new(),
            sizes: Vec::new(),
            init,
            load: Duration::ZERO,
            initial_usage: process_usage(),
        }
    }

    fn record(&mut self, time: Duration, load: Duration, len: usize) {
        self.elapsed.push(time);
        self.sizes.push(len + RECORD_BYTES);
        self.load += load;
    }

    fn report(
        mut self,
        command: &str,
        scenario: &str,
        encoding: Encoding,
        tile_size: u16,
        raw_len: usize,
        options: Options,
    ) {
        let usage = self
            .initial_usage
            .zip(process_usage())
            .map(|((start, _), (end, rss))| (end - start, rss));
        self.elapsed.sort_unstable();
        self.sizes.sort_unstable();
        let count = self.sizes.len();
        let sum: u64 = self.sizes.iter().map(|&size| size as u64).sum();
        let seconds = self.elapsed.iter().sum::<Duration>().as_secs_f64();
        let avg = sum as f64 / count as f64;
        let percent = 100.0 * avg / raw_len as f64;
        let mib = (raw_len as f64 * count as f64) / 1_048_576.0;
        let mib_per_sec = mib / seconds.max(1e-9);
        let quantile = |percent: usize| (count * percent).div_ceil(100) - 1;
        let micros = |time: Duration| time.as_secs_f64() * 1_000_000.0;
        let [p50, p95, p99, max] = [50, 95, 99, 100].map(|p| micros(self.elapsed[quantile(p)]));
        let [bytes_p50, bytes_p95, bytes_p99, bytes_max] =
            [50, 95, 99, 100].map(|p| self.sizes[quantile(p)]);
        let bytes_min = self.sizes[0];
        let init_us = micros(self.init);
        let load_us = micros(self.load);
        let codec = encoding.name();
        let tile_size = if encoding.mode.tiled() { tile_size } else { 0 };
        let buffers = if options.fresh_buffers {
            "fresh"
        } else {
            "reused"
        };
        if options.csv {
            let (cpu_s, rss_bytes) = usage.map_or_else(
                || (String::new(), String::new()),
                |(cpu, rss)| (format!("{cpu:.6}"), rss.to_string()),
            );
            println!(
                "operation,scenario,codec,tile_size,buffers,samples,raw_bytes_per_frame,record_overhead_bytes,avg_bytes,min_bytes,p50_bytes,p95_bytes,p99_bytes,max_bytes,percent_of_raw,p50_us,p95_us,p99_us,max_us,raw_mib_per_sec,init_us,load_us,process_cpu_s,peak_rss_bytes"
            );
            println!(
                "{command},{scenario},{codec},{tile_size},{buffers},{count},{raw_len},{RECORD_BYTES},{avg:.2},{bytes_min},{bytes_p50},{bytes_p95},{bytes_p99},{bytes_max},{percent:.3},{p50:.2},{p95:.2},{p99:.2},{max:.2},{mib_per_sec:.2},{init_us:.2},{load_us:.2},{cpu_s},{rss_bytes}"
            );
        } else {
            println!("{scenario} {codec} {command}: {count} verified samples, {buffers} buffers");
            println!(
                "{raw_len} raw bytes/frame, {avg:.1} mean bytes/frame ({percent:.2}% of raw, includes {RECORD_BYTES} framing bytes)"
            );
            println!(
                "p50 {p50:.2} µs, p95 {p95:.2} µs, p99 {p99:.2} µs, max {max:.2} µs, {mib_per_sec:.2} raw MiB/s"
            );
            println!("codec initialization {init_us:.2} µs, total input loading {load_us:.2} µs");
            if let Some((cpu, rss)) = usage {
                println!(
                    "process CPU {cpu:.3} s, peak RSS {:.2} MiB",
                    rss as f64 / 1_048_576.0
                );
            }
        }
    }
}

fn process_usage() -> Option<(f64, u64)> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // A successful getrusage initializes the supplied structure.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
            return None;
        }
        let usage = unsafe { usage.assume_init() };
        let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1_000_000.0;
        let bytes = u64::try_from(usage.ru_maxrss).ok()?;
        let bytes = if cfg!(target_os = "linux") {
            bytes * 1024
        } else {
            bytes
        };
        Some((seconds(usage.ru_utime) + seconds(usage.ru_stime), bytes))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    None
}
