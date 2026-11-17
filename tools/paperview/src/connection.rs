use std::fmt;
use std::io::{self, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use paper_protocol::{
    V2_FRAME_PREFIX_LEN, V2_MAX_PAYLOAD_LEN, V2EncodeError, V2Hello, V2PixelFormat, V2PixelFormats,
};

pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
pub const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub enum ConnectionError {
    InvalidInput(String),
    EncodeHello(V2EncodeError),
    Resolve {
        host: String,
        port: u16,
        source: io::Error,
    },
    NoAddresses {
        host: String,
        port: u16,
    },
    Connect {
        host: String,
        port: u16,
        source: io::Error,
    },
    Configure {
        operation: &'static str,
        source: io::Error,
    },
    SendHello(io::Error),
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::EncodeHello(error) => {
                write!(formatter, "could not encode PPFB v2 Hello: {error}")
            }
            Self::Resolve { host, port, source } => {
                write!(formatter, "could not resolve {host}:{port}: {source}")
            }
            Self::NoAddresses { host, port } => {
                write!(formatter, "{host}:{port} resolved to no socket addresses")
            }
            Self::Connect { host, port, source } => {
                write!(formatter, "could not connect to {host}:{port}: {source}")
            }
            Self::Configure { operation, source } => {
                write!(
                    formatter,
                    "could not {operation} on PaperSpoon socket: {source}"
                )
            }
            Self::SendHello(error) => write!(formatter, "could not send PPFB v2 Hello: {error}"),
        }
    }
}

impl std::error::Error for ConnectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidInput(_) | Self::NoAddresses { .. } => None,
            Self::EncodeHello(error) => Some(error),
            Self::Resolve { source, .. }
            | Self::Connect { source, .. }
            | Self::Configure { source, .. }
            | Self::SendHello(source) => Some(source),
        }
    }
}

pub fn connect_and_send_hello(
    host: &str,
    port: u16,
    viewport_width: u16,
    viewport_height: u16,
    connect_timeout: Duration,
    write_timeout: Duration,
) -> Result<TcpStream, ConnectionError> {
    validate_inputs(
        host,
        port,
        viewport_width,
        viewport_height,
        connect_timeout,
        write_timeout,
    )?;

    let formats = V2PixelFormats::MONO1.with(V2PixelFormat::Gray8);
    let hello = V2Hello::with_pixel_formats(viewport_width, viewport_height, formats);
    let encoded = hello
        .encode_message()
        .map_err(ConnectionError::EncodeHello)?;

    let started = Instant::now();
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|source| ConnectionError::Resolve {
            host: host.to_string(),
            port,
            source,
        })?;
    let mut resolved_any = false;
    let mut last_error = None;
    let mut connected = None;
    for address in addresses {
        resolved_any = true;
        let Some(remaining) = connect_timeout
            .checked_sub(started.elapsed())
            .filter(|remaining| !remaining.is_zero())
        else {
            last_error = Some(io::Error::new(
                io::ErrorKind::TimedOut,
                "overall connection deadline elapsed",
            ));
            break;
        };
        match TcpStream::connect_timeout(&address, remaining) {
            Ok(stream) => {
                connected = Some(stream);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }

    if !resolved_any {
        return Err(ConnectionError::NoAddresses {
            host: host.to_string(),
            port,
        });
    }
    let mut stream = connected.ok_or_else(|| ConnectionError::Connect {
        host: host.to_string(),
        port,
        source: last_error.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "no connection attempt succeeded",
            )
        }),
    })?;

    stream
        .set_nodelay(true)
        .map_err(|source| ConnectionError::Configure {
            operation: "enable TCP_NODELAY",
            source,
        })?;
    stream
        .set_write_timeout(Some(write_timeout))
        .map_err(|source| ConnectionError::Configure {
            operation: "set write timeout",
            source,
        })?;
    if let Err(error) = stream.write_all(&encoded) {
        let _ = stream.shutdown(Shutdown::Both);
        return Err(ConnectionError::SendHello(error));
    }
    Ok(stream)
}

fn validate_inputs(
    host: &str,
    port: u16,
    viewport_width: u16,
    viewport_height: u16,
    connect_timeout: Duration,
    write_timeout: Duration,
) -> Result<(), ConnectionError> {
    if host.trim().is_empty() {
        return Err(ConnectionError::InvalidInput(
            "PaperSpoon host must not be empty or whitespace".to_string(),
        ));
    }
    if port == 0 {
        return Err(ConnectionError::InvalidInput(
            "PaperSpoon port must be nonzero".to_string(),
        ));
    }
    if viewport_width == 0 || viewport_height == 0 {
        return Err(ConnectionError::InvalidInput(format!(
            "PPFB viewport must be nonzero, got {viewport_width}x{viewport_height}"
        )));
    }
    let gray8_pixels = u64::from(viewport_width) * u64::from(viewport_height);
    let maximum_gray8_pixels = (V2_MAX_PAYLOAD_LEN - V2_FRAME_PREFIX_LEN) as u64;
    if gray8_pixels > maximum_gray8_pixels {
        return Err(ConnectionError::InvalidInput(format!(
            "PPFB viewport {viewport_width}x{viewport_height} needs {gray8_pixels} Gray8 bytes, exceeding the frame limit of {maximum_gray8_pixels}"
        )));
    }
    if connect_timeout.is_zero() {
        return Err(ConnectionError::InvalidInput(
            "connect timeout must be nonzero".to_string(),
        ));
    }
    if write_timeout.is_zero() {
        return Err(ConnectionError::InvalidInput(
            "write timeout must be nonzero".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::TcpListener;
    use std::thread;

    use paper_protocol::{
        V2_HEADER_LEN, V2DecodeResult, V2Payload, decode_v2_message, decode_v2_payload,
    };

    use super::*;

    #[test]
    fn sends_exactly_one_dual_format_hello_and_keeps_stream_open() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback listener");
        let address = listener.local_addr().expect("listener address");
        let server = thread::spawn(move || {
            let (mut peer, _) = listener.accept().expect("accept PaperView");
            let mut encoded = vec![0; V2_HEADER_LEN];
            peer.read_exact(&mut encoded).expect("read Hello header");
            let additional = match decode_v2_message(&encoded).expect("validate Hello header") {
                V2DecodeResult::Incomplete { additional } => additional,
                V2DecodeResult::Complete { .. } => panic!("header-only Hello must be incomplete"),
            };
            encoded.resize(encoded.len() + additional, 0);
            peer.read_exact(&mut encoded[V2_HEADER_LEN..])
                .expect("read Hello payload");
            let message = match decode_v2_message(&encoded).expect("decode Hello") {
                V2DecodeResult::Complete { message, consumed } => {
                    assert_eq!(consumed, encoded.len());
                    message
                }
                V2DecodeResult::Incomplete { .. } => panic!("complete Hello expected"),
            };
            let V2Payload::Hello(hello) = decode_v2_payload(message).expect("typed Hello") else {
                panic!("Hello payload expected");
            };

            peer.set_read_timeout(Some(Duration::from_millis(100)))
                .expect("set extra-message timeout");
            let mut extra = [0; 1];
            let no_extra_message = match peer.read(&mut extra) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    true
                }
                Ok(0) => false,
                Ok(_) => false,
                Err(error) => panic!("unexpected extra-message read error: {error}"),
            };
            (hello, no_extra_message)
        });

        let stream = connect_and_send_hello(
            "127.0.0.1",
            address.port(),
            1272,
            1624,
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("connect and send Hello");
        assert_eq!(stream.peer_addr().expect("peer address"), address);

        let (hello, no_extra_message) = server.join().expect("join server");
        assert_eq!(hello.viewport_width(), 1272);
        assert_eq!(hello.viewport_height(), 1624);
        assert_eq!(
            hello.pixel_formats(),
            V2PixelFormats::MONO1.with(V2PixelFormat::Gray8)
        );
        assert!(no_extra_message, "only one Hello should be sent");
    }

    #[test]
    fn rejects_invalid_inputs_before_connecting() {
        for error in [
            connect_and_send_hello(
                " ",
                5581,
                1272,
                1624,
                Duration::from_secs(1),
                Duration::from_secs(1),
            ),
            connect_and_send_hello(
                "127.0.0.1",
                0,
                1272,
                1624,
                Duration::from_secs(1),
                Duration::from_secs(1),
            ),
            connect_and_send_hello(
                "127.0.0.1",
                5581,
                0,
                1624,
                Duration::from_secs(1),
                Duration::from_secs(1),
            ),
            connect_and_send_hello(
                "127.0.0.1",
                5581,
                4096,
                4096,
                Duration::from_secs(1),
                Duration::from_secs(1),
            ),
            connect_and_send_hello(
                "127.0.0.1",
                5581,
                1272,
                1624,
                Duration::ZERO,
                Duration::from_secs(1),
            ),
            connect_and_send_hello(
                "127.0.0.1",
                5581,
                1272,
                1624,
                Duration::from_secs(1),
                Duration::ZERO,
            ),
        ] {
            assert!(
                matches!(error, Err(ConnectionError::InvalidInput(_))),
                "unexpected result: {error:?}"
            );
        }
    }

    #[test]
    fn reports_refused_connections_without_sending_hello() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve loopback port");
        let port = listener.local_addr().expect("listener address").port();
        drop(listener);

        let error = connect_and_send_hello(
            "127.0.0.1",
            port,
            1272,
            1624,
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect_err("closed loopback port must refuse connection");
        assert!(matches!(error, ConnectionError::Connect { .. }), "{error}");
    }
}
