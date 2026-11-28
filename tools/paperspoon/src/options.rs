//! Command-line options for PaperSpoon.

use std::time::Duration;

use paper_protocol::v3;

use crate::live_now_playing::ProviderKind;

/// Default TCP port. Must match `rust_x11_hello`'s `COMPANION_PORT`.
pub(crate) const DEFAULT_PORT: u16 = paper_protocol::DEFAULT_TCP_PORT;
/// Default log file name for received activation lines.
pub(crate) const DEFAULT_LOG_FILE: &str = "paperspoon.log";
pub(crate) const DEFAULT_LATENCY_LOG_FILE: &str = "paperspoon.latency.jsonl";
/// Whether to forward actions to Hammerspoon via `open -g hammerspoon://`.
pub(crate) const FORWARD_TO_HAMMERSPOON: bool = true;

/// Parsed command line.
#[derive(Debug)]
pub(crate) struct Options {
    pub(crate) port: u16,
    pub(crate) log_path: String,
    pub(crate) latency_log_path: String,
    /// None means URL forwarding is disabled.
    pub(crate) forward_url: bool,
    pub(crate) live_provider: ProviderKind,
    pub(crate) progress_interval: Duration,
    pub(crate) compression: HostCompression,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum HostCompression {
    Raw,
    #[default]
    Auto,
    Zstd,
}

impl HostCompression {
    pub(crate) fn negotiate(
        self,
        offered: Option<v3::Encodings>,
    ) -> Result<Option<v3::Encodings>, &'static str> {
        let supported = if self == Self::Raw {
            v3::Encodings::RAW
        } else {
            v3::Encodings::RAW_ZSTD
        };
        let agreed = offered.map(|offered| offered.intersection(supported));
        if self == Self::Zstd && !agreed.is_some_and(|agreed| agreed.supports(v3::Encoding::Zstd)) {
            return Err(
                "Zstandard is required. Enable PPFB v3 Zstandard on the client or select --compression=auto",
            );
        }
        Ok(agreed)
    }
}

pub(crate) fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut port = DEFAULT_PORT;
    let mut log_path = DEFAULT_LOG_FILE.to_string();
    let mut latency_log_path = DEFAULT_LATENCY_LOG_FILE.to_string();
    let mut forward_url = FORWARD_TO_HAMMERSPOON;
    let mut live_provider = ProviderKind::default();
    let mut progress_interval = Duration::from_secs(1);
    let mut compression = HostCompression::default();
    let positional = args.iter().skip(1);
    for arg in positional {
        match arg.as_str() {
            "--compression=raw" => compression = HostCompression::Raw,
            "--compression=auto" => compression = HostCompression::Auto,
            "--compression=zstd" => compression = HostCompression::Zstd,
            value if value.starts_with("--compression=") => {
                return Err("--compression requires raw, auto, or zstd".into());
            }
            "--no-forward-url" => {
                forward_url = false;
            }
            "--now-playing-fake" => live_provider = ProviderKind::Fake,
            value if value.starts_with("--now-playing-progress-ms=") => {
                let interval = value["--now-playing-progress-ms=".len()..]
                    .parse::<u64>()
                    .ok()
                    .filter(|interval| matches!(interval, 100 | 250 | 500 | 1000 | 2000))
                    .ok_or("--now-playing-progress-ms expects 100, 250, 500, 1000, or 2000")?;
                progress_interval = Duration::from_millis(interval);
            }
            value if value.starts_with("--latency-log=") => {
                if let Some(path) = value.strip_prefix("--latency-log=")
                    && !path.is_empty()
                {
                    latency_log_path = path.to_string();
                }
            }
            value if value.starts_with("--") => return Err(format!("unknown option {value}")),
            _ => {
                // Positional: try port first, then log file.
                if port == DEFAULT_PORT
                    && let Ok(parsed) = arg.parse()
                {
                    port = parsed;
                    continue;
                }
                if log_path == DEFAULT_LOG_FILE {
                    log_path = arg.clone();
                    continue;
                }
            }
        }
    }
    Ok(Options {
        port,
        log_path,
        latency_log_path,
        forward_url,
        live_provider,
        progress_interval,
        compression,
    })
}
