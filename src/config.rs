//! Process configuration read once before transport startup.

use anyhow::{Context, Result, bail};

const COMPANION_HOST_ENV: &str = "RUST_X11_HELLO_COMPANION";
const COMPANION_PORT_ENV: &str = "RUST_X11_HELLO_COMPANION_PORT";
const DEFAULT_COMPANION_PORT: u16 = paper_protocol::DEFAULT_TCP_PORT;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PaperpadConfig {
    host: Option<String>,
    port: u16,
}

impl PaperpadConfig {
    pub(crate) fn from_env() -> Result<Self> {
        let host = read_optional_env(COMPANION_HOST_ENV)?;
        let port = read_optional_env(COMPANION_PORT_ENV)?;
        Self::from_values(host.as_deref(), port.as_deref())
    }

    pub(crate) fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    fn from_values(host: Option<&str>, port: Option<&str>) -> Result<Self> {
        let host = host.map(str::trim).filter(|value| !value.is_empty());
        if host.is_none() && port.is_some() {
            bail!("{COMPANION_PORT_ENV} requires an explicit {COMPANION_HOST_ENV}");
        }

        let port = match port {
            Some(value) => value
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .with_context(|| {
                    format!(
                        "{COMPANION_PORT_ENV} must be a decimal TCP port in 1..=65535, got {value:?}"
                    )
                })?,
            None => DEFAULT_COMPANION_PORT,
        };

        Ok(Self {
            host: host.map(str::to_owned),
            port,
        })
    }
}

fn read_optional_env(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("{name} must contain valid UTF-8")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_uses_protocol_default_as_inactive_port() {
        assert_eq!(
            PaperpadConfig::from_values(None, None).unwrap(),
            PaperpadConfig {
                host: None,
                port: DEFAULT_COMPANION_PORT,
            }
        );
    }

    #[test]
    fn explicit_host_uses_default_or_valid_nonzero_port() {
        assert_eq!(
            PaperpadConfig::from_values(Some("paperspoon.local"), None)
                .unwrap()
                .port(),
            DEFAULT_COMPANION_PORT
        );
        for port in [1, 6000, u16::MAX] {
            assert_eq!(
                PaperpadConfig::from_values(Some("paperspoon.local"), Some(&port.to_string()))
                    .unwrap()
                    .port(),
                port
            );
        }
    }

    #[test]
    fn explicit_host_is_trimmed_and_blank_host_selects_discovery() {
        assert_eq!(
            PaperpadConfig::from_values(Some(" 192.0.2.1 "), None)
                .unwrap()
                .host(),
            Some("192.0.2.1")
        );
        assert_eq!(
            PaperpadConfig::from_values(Some("  "), None)
                .unwrap()
                .host(),
            None
        );
    }

    #[test]
    fn invalid_explicit_ports_are_rejected() {
        for port in ["", "0", " 5581", "not-a-port", "65536"] {
            let error = PaperpadConfig::from_values(Some("paperspoon.local"), Some(port))
                .expect_err("invalid port should fail configuration");
            assert!(error.to_string().contains(COMPANION_PORT_ENV));
        }
    }

    #[test]
    fn port_without_explicit_host_is_rejected() {
        for host in [None, Some("  ")] {
            let error = PaperpadConfig::from_values(host, Some("5581"))
                .expect_err("discovery must use its advertised port");
            assert!(error.to_string().contains("requires an explicit"));
        }
    }
}
