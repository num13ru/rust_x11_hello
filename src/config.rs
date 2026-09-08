//! Process configuration read once before transport startup.

const COMPANION_HOST_ENV: &str = "RUST_X11_HELLO_COMPANION";
const COMPANION_PORT_ENV: &str = "RUST_X11_HELLO_COMPANION_PORT";
const DEFAULT_COMPANION_PORT: u16 = paper_protocol::DEFAULT_TCP_PORT;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PaperpadConfig {
    host: Option<String>,
    port: u16,
}

impl PaperpadConfig {
    pub(crate) fn from_env() -> Self {
        let host = std::env::var(COMPANION_HOST_ENV).ok();
        let port = std::env::var(COMPANION_PORT_ENV).ok();
        Self::from_values(host.as_deref(), port.as_deref())
    }

    pub(crate) fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    fn from_values(host: Option<&str>, port: Option<&str>) -> Self {
        Self {
            host: host.map(str::to_owned),
            port: port
                .and_then(|value| value.parse().ok())
                .unwrap_or(DEFAULT_COMPANION_PORT),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_invalid_port_uses_protocol_default() {
        assert_eq!(
            PaperpadConfig::from_values(None, None),
            PaperpadConfig {
                host: None,
                port: DEFAULT_COMPANION_PORT,
            }
        );
        assert_eq!(
            PaperpadConfig::from_values(None, Some("not-a-port")).port(),
            DEFAULT_COMPANION_PORT
        );
        assert_eq!(
            PaperpadConfig::from_values(None, Some("65536")).port(),
            DEFAULT_COMPANION_PORT
        );
    }

    #[test]
    fn valid_port_including_zero_is_preserved() {
        assert_eq!(PaperpadConfig::from_values(None, Some("0")).port(), 0);
        assert_eq!(PaperpadConfig::from_values(None, Some("6000")).port(), 6000);
    }

    #[test]
    fn host_value_is_preserved_for_transport_resolution() {
        assert_eq!(
            PaperpadConfig::from_values(Some("192.0.2.1"), None).host(),
            Some("192.0.2.1")
        );
        assert_eq!(
            PaperpadConfig::from_values(Some("  "), None).host(),
            Some("  ")
        );
    }
}
