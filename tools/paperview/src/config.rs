use std::ffi::OsString;
use std::path::PathBuf;

use paper_protocol::{DEFAULT_TCP_PORT, V2_FRAME_PREFIX_LEN, V2_MAX_PAYLOAD_LEN, v3};

pub(crate) const USAGE: &str = "Usage: paperview [OPTIONS]

Options:
  --host HOST             PaperSpoon host (default: 127.0.0.1)
  --port PORT             PaperSpoon TCP port (default: 5581)
  --viewport WIDTHxHEIGHT Negotiated PPFB viewport (default: 1272x1624)
  --save-dir DIR          Existing directory for source PNGs written with S
  --transport MODE         v2 (default), v3-raw, v3-auto, or v3-zstd
  --transport-diagnostics   Log receive statistics every five seconds
  --interactive           Send primary-button clicks as PPFB pointer input
  -h, --help              Print this help";

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_VIEWPORT_WIDTH: u16 = 1272;
const DEFAULT_VIEWPORT_HEIGHT: u16 = 1624;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Options {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) viewport_width: u16,
    pub(crate) viewport_height: u16,
    pub(crate) save_dir: Option<PathBuf>,
    pub(crate) interactive: bool,
    pub(crate) transport: v3::ClientMode,
    pub(crate) transport_diagnostics: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_TCP_PORT,
            viewport_width: DEFAULT_VIEWPORT_WIDTH,
            viewport_height: DEFAULT_VIEWPORT_HEIGHT,
            save_dir: None,
            interactive: false,
            transport: v3::ClientMode::Legacy,
            transport_diagnostics: false,
        }
    }
}

impl Options {
    pub(crate) fn target_label(&self) -> String {
        format!(
            "{}:{} — {}x{}",
            self.host, self.port, self.viewport_width, self.viewport_height
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ParseOutcome {
    Run(Options),
    Help,
}

pub(crate) fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<ParseOutcome, String> {
    let mut arguments = arguments.into_iter();
    let _program_name = arguments.next();
    let mut options = Options::default();
    let mut host_seen = false;
    let mut port_seen = false;
    let mut viewport_seen = false;
    let mut save_dir_seen = false;
    let mut interactive_seen = false;
    let mut transport_seen = false;

    while let Some(argument) = arguments.next() {
        let argument = argument
            .into_string()
            .map_err(|_| "command-line arguments must be valid Unicode".to_string())?;
        match argument.as_str() {
            "-h" | "--help" => return Ok(ParseOutcome::Help),
            "--host" => {
                reject_duplicate(&mut host_seen, "--host")?;
                let host = next_value(&mut arguments, "--host")?;
                if host.trim().is_empty() {
                    return Err("--host must not be empty or whitespace".to_string());
                }
                options.host = host;
            }
            "--port" => {
                reject_duplicate(&mut port_seen, "--port")?;
                let port = next_value(&mut arguments, "--port")?;
                options.port = parse_port(&port)?;
            }
            "--viewport" => {
                reject_duplicate(&mut viewport_seen, "--viewport")?;
                let viewport = next_value(&mut arguments, "--viewport")?;
                let (width, height) = parse_viewport(&viewport)?;
                options.viewport_width = width;
                options.viewport_height = height;
            }
            "--save-dir" => {
                reject_duplicate(&mut save_dir_seen, "--save-dir")?;
                let directory = next_os_value(&mut arguments, "--save-dir")?;
                if directory.is_empty() {
                    return Err("--save-dir must not be empty".to_string());
                }
                options.save_dir = Some(PathBuf::from(directory));
            }
            "--transport-diagnostics" => {
                reject_duplicate(
                    &mut options.transport_diagnostics,
                    "--transport-diagnostics",
                )?;
            }
            "--transport" => {
                reject_duplicate(&mut transport_seen, "--transport")?;
                let value = next_value(&mut arguments, "--transport")?;
                options.transport = value
                    .parse()
                    .map_err(|error: std::io::Error| format!("--transport: {error}"))?;
            }
            "--interactive" => {
                reject_duplicate(&mut interactive_seen, "--interactive")?;
                options.interactive = true;
            }
            _ => return Err(format!("unexpected argument {argument:?}")),
        }
    }

    Ok(ParseOutcome::Run(options))
}

fn reject_duplicate(seen: &mut bool, option: &str) -> Result<(), String> {
    if *seen {
        return Err(format!("{option} may only be specified once"));
    }
    *seen = true;
    Ok(())
}

fn next_os_value(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<OsString, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn next_value(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))?
        .into_string()
        .map_err(|_| format!("{option} value must be valid Unicode"))
}

fn parse_port(value: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| format!("--port must be an integer from 1 through 65535, got {value:?}"))
}

fn parse_viewport(value: &str) -> Result<(u16, u16), String> {
    let separator = value
        .find(['x', 'X'])
        .ok_or_else(|| viewport_syntax_error(value))?;
    let (width, height_with_separator) = value.split_at(separator);
    let height = &height_with_separator[1..];
    if height.contains(['x', 'X']) {
        return Err(viewport_syntax_error(value));
    }

    let width = parse_dimension(width, value)?;
    let height = parse_dimension(height, value)?;
    let gray8_pixels = u64::from(width) * u64::from(height);
    let maximum_gray8_pixels = (V2_MAX_PAYLOAD_LEN - V2_FRAME_PREFIX_LEN) as u64;
    if gray8_pixels > maximum_gray8_pixels {
        return Err(format!(
            "--viewport {value:?} needs {gray8_pixels} Gray8 bytes, exceeding the PPFB v2 frame limit of {maximum_gray8_pixels}"
        ));
    }
    Ok((width, height))
}

fn parse_dimension(dimension: &str, viewport: &str) -> Result<u16, String> {
    dimension
        .parse::<u16>()
        .ok()
        .filter(|dimension| *dimension != 0)
        .ok_or_else(|| viewport_syntax_error(viewport))
}

fn viewport_syntax_error(value: &str) -> String {
    format!(
        "--viewport must be nonzero WIDTHxHEIGHT with each dimension at most 65535, got {value:?}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strings(arguments: &[&str]) -> Result<ParseOutcome, String> {
        parse(
            std::iter::once(OsString::from("paperview"))
                .chain(arguments.iter().map(OsString::from)),
        )
    }

    #[test]
    fn defaults_match_local_paperspoon_and_paperwhite_viewport() {
        assert_eq!(
            parse_strings(&[]),
            Ok(ParseOutcome::Run(Options {
                host: "127.0.0.1".to_string(),
                port: 5581,
                viewport_width: 1272,
                viewport_height: 1624,
                save_dir: None,
                interactive: false,
                transport: v3::ClientMode::Legacy,
                transport_diagnostics: false,
            }))
        );
    }

    #[test]
    fn explicit_options_accept_dns_host_port_and_case_insensitive_separator() {
        assert_eq!(
            parse_strings(&[
                "--host",
                "paperspoon.local",
                "--port",
                "6000",
                "--viewport",
                "800X600",
                "--save-dir",
                "exports",
                "--interactive",
            ]),
            Ok(ParseOutcome::Run(Options {
                host: "paperspoon.local".to_string(),
                port: 6000,
                viewport_width: 800,
                viewport_height: 600,
                save_dir: Some(PathBuf::from("exports")),
                interactive: true,
                transport: v3::ClientMode::Legacy,
                transport_diagnostics: false,
            }))
        );
    }

    #[test]
    fn help_short_circuits_normal_startup() {
        assert_eq!(parse_strings(&["--help"]), Ok(ParseOutcome::Help));
        assert_eq!(parse_strings(&["-h"]), Ok(ParseOutcome::Help));
    }

    #[test]
    fn missing_option_values_fail_clearly() {
        for option in ["--host", "--port", "--viewport", "--save-dir"] {
            let error = parse_strings(&[option]).expect_err("missing value must fail");
            assert!(error.contains("requires a value"), "{error}");
        }
    }

    #[test]
    fn invalid_ports_are_rejected() {
        for port in ["0", "65536", "-1", "not-a-port"] {
            let error = parse_strings(&["--port", port]).expect_err("invalid port must fail");
            assert!(error.contains("1 through 65535"), "{error}");
        }
    }

    #[test]
    fn zero_malformed_and_out_of_range_viewports_are_rejected() {
        for viewport in ["", "800", "0x600", "800x0", "800x600x2", "65536x1"] {
            let error =
                parse_strings(&["--viewport", viewport]).expect_err("invalid viewport must fail");
            assert!(error.contains("nonzero WIDTHxHEIGHT"), "{error}");
        }
    }

    #[test]
    fn viewport_is_bounded_for_advertised_gray8_frames() {
        let accepted = parse_strings(&["--viewport", "4095x4096"]);
        assert!(
            accepted.is_ok(),
            "near-limit viewport must fit: {accepted:?}"
        );

        let error = parse_strings(&["--viewport", "4096x4096"])
            .expect_err("oversized Gray8 viewport must fail");
        assert!(
            error.contains("exceeding the PPFB v2 frame limit"),
            "{error}"
        );
    }

    #[test]
    fn now_playing_geometry_fixture_matches_the_default_viewport() {
        let fixture = include_str!("../tests/fixtures/now-playing-viewport.txt").trim();
        let ParseOutcome::Run(options) =
            parse_strings(&["--viewport", fixture]).expect("parse viewport fixture")
        else {
            panic!("viewport fixture unexpectedly requested help");
        };
        assert_eq!(
            (options.viewport_width, options.viewport_height),
            (DEFAULT_VIEWPORT_WIDTH, DEFAULT_VIEWPORT_HEIGHT)
        );
    }

    #[test]
    fn duplicate_options_are_rejected() {
        for (option, first, second) in [
            ("--host", "first", "second"),
            ("--port", "5581", "5582"),
            ("--viewport", "800x600", "600x800"),
            ("--save-dir", "first", "second"),
        ] {
            let error = parse_strings(&[option, first, option, second])
                .expect_err("duplicate option must fail");
            assert!(error.contains("only be specified once"), "{error}");
        }

        let error = parse_strings(&["--interactive", "--interactive"])
            .expect_err("duplicate interactive flag must fail");
        assert!(error.contains("only be specified once"), "{error}");
    }

    #[test]
    fn blank_hosts_and_unexpected_arguments_are_rejected() {
        assert!(
            parse_strings(&["--host", "  "])
                .expect_err("blank host must fail")
                .contains("must not be empty")
        );
        assert!(
            parse_strings(&["--save-dir", ""])
                .expect_err("empty save directory must fail")
                .contains("must not be empty")
        );
        assert!(
            parse_strings(&["positional"])
                .expect_err("positional argument must fail")
                .contains("unexpected argument")
        );
        assert!(
            parse_strings(&["--unknown"])
                .expect_err("unknown option must fail")
                .contains("unexpected argument")
        );
    }
}
