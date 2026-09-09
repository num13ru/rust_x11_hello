//! PaperSpoon UDP discovery responder.
//!
//! Binds UDP `0.0.0.0:5580`; on a valid DISCOVER datagram, replies with a
//! single unicast HERE to the exact request source. No broadcast responses,
//! multicast, or acknowledgements.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

use paper_protocol::{DISCOVERY_PORT, format_here, parse_discover};

/// Bind and run the discovery responder forever.
///
/// Returns only on socket failure; malformed requests are logged and skipped.
pub fn run_discovery_listener(tcp_port: u16) -> std::io::Result<()> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)))?;
    println!("discovery listening address=0.0.0.0:{DISCOVERY_PORT}");

    loop {
        respond_once(&socket, tcp_port)?;
    }
}

fn respond_once(socket: &UdpSocket, tcp_port: u16) -> std::io::Result<()> {
    if tcp_port == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "discovery cannot advertise TCP port 0",
        ));
    }
    let mut buffer = [0u8; 256];
    let (count, source) = socket.recv_from(&mut buffer)?;
    let Some(nonce) = parse_discover(&buffer[..count]) else {
        eprintln!("discovery request invalid from={source}");
        return Ok(());
    };
    eprintln!("discovery request from={source} nonce={nonce}");
    let response = format_here(&nonce, tcp_port);
    socket.send_to(response.as_bytes(), source)?;
    eprintln!("discovery response sent to={source} tcp_port={tcp_port}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use paper_protocol::{format_discover, parse_here};
    use std::time::Duration;

    #[test]
    fn responder_advertises_injected_tcp_port() {
        let responder = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind responder");
        responder
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set responder timeout");
        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind client");
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set client timeout");
        let nonce = "actual-port-test";
        client
            .send_to(
                format_discover(nonce).as_bytes(),
                responder.local_addr().expect("responder address"),
            )
            .expect("send discovery request");

        respond_once(&responder, 42_424).expect("respond");

        let mut buffer = [0u8; 256];
        let (count, _) = client.recv_from(&mut buffer).expect("receive response");
        assert_eq!(
            parse_here(&buffer[..count]),
            Some((nonce.to_string(), 42_424))
        );
    }

    #[test]
    fn responder_rejects_zero_tcp_port() {
        let responder = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind responder");
        let error = respond_once(&responder, 0).expect_err("zero port must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn malformed_request_is_ignored_without_response() {
        let responder = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind responder");
        responder
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set responder timeout");
        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind client");
        client
            .send_to(
                b"not a discovery request\n",
                responder.local_addr().expect("responder address"),
            )
            .expect("send malformed request");

        respond_once(&responder, 42_424).expect("ignore malformed request");

        client
            .set_nonblocking(true)
            .expect("set client nonblocking");
        let mut buffer = [0u8; 256];
        let error = client
            .recv_from(&mut buffer)
            .expect_err("malformed request must not receive response");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }
}
