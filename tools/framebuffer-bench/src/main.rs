mod benchmark;
mod codec;
mod corpus;
mod inter;
#[cfg(feature = "scenarios")]
mod scenarios;

use std::env;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use corpus::CorpusReader;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("framebuffer-bench: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    match arguments.as_slice() {
        [command] if command == "--help" || command == "-h" => {
            println!("framebuffer-bench verify CORPUS");
            println!("framebuffer-bench verify CORPUS ENCODED");
            println!("framebuffer-bench encode CORPUS ENCODING ENCODED [OPTIONS]");
            println!("framebuffer-bench decode ENCODED [OPTIONS]");
            println!("\nOptions: --warmup N, --iterations N, --fresh-buffers, --csv");
            println!("Encode options: --tile-size N, --keyframe N (0 means initial keyframe only)");
            println!("Delta prefixes: {}", inter::MODE_NAMES);
            println!("Codecs: {}", codec::Codec::NAMES.join(", "));
            #[cfg(feature = "scenarios")]
            println!(
                "framebuffer-bench generate SCENARIO CORPUS [JPEG ...]\n\nScenarios: {}",
                scenarios::NAMES.join(", ")
            );
            Ok(())
        }
        [command, path] if command == "verify" => verify(Path::new(path)),
        [command, corpus, path] if command == "verify" => benchmark::decode(
            Path::new(path),
            Some(Path::new(corpus)),
            benchmark::Options::parse(&[])?,
        ),
        [command, corpus, name, output, options @ ..] if command == "encode" => {
            let encoding = name
                .to_str()
                .context("encoding name must be UTF-8")?
                .parse()?;
            benchmark::encode(
                Path::new(corpus),
                encoding,
                Path::new(output),
                benchmark::Options::parse(options)?,
            )
        }
        [command, path, options @ ..] if command == "decode" => {
            benchmark::decode(Path::new(path), None, benchmark::Options::parse(options)?)
        }
        #[cfg(feature = "scenarios")]
        [command, name, path, artwork @ ..] if command == "generate" => {
            let name = name.to_str().context("scenario name must be UTF-8")?;
            scenarios::generate(name, Path::new(path), artwork)
        }
        _ => bail!("invalid arguments, see --help"),
    }
}

fn stage_output(path: &Path) -> Result<tempfile::NamedTempFile> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("stage output in {}", parent.display()))
}

fn save_output(output: tempfile::NamedTempFile, path: &Path) -> Result<()> {
    output
        .persist_noclobber(path)
        .map_err(|error| error.error)
        .with_context(|| format!("save new file {}", path.display()))?;
    Ok(())
}

fn verify(path: &Path) -> Result<()> {
    let input = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut corpus = CorpusReader::new(BufReader::new(input))?;
    let mut end_us = 0;
    let mut raw_bytes = 0;
    while let Some(sample) = corpus.next_sample()? {
        end_us = sample.timestamp_us;
        raw_bytes += sample.frame.pixels().len() as u64;
    }
    let header = &corpus.header;
    let layout = header.layout;
    println!(
        "verified {}: {} frames, {:?} {}x{}, stride {}, {} bytes/frame, {raw_bytes} total bytes, {:.3} s",
        header.scenario,
        header.frame_count,
        layout.format,
        layout.width,
        layout.height,
        layout.stride,
        layout.len,
        end_us as f64 / 1_000_000.0,
    );
    Ok(())
}
