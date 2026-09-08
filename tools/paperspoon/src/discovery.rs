//! PaperSpoon UDP discovery responder.
//!
//! Binds UDP `0.0.0.0:5580`; on a valid DISCOVER datagram, replies with a
//! single unicast HERE to the exact request source. No broadcast responses,
//! multicast, or acknowledgements.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

use paper_protocol::{DEFAULT_TCP_PORT, DISCOVERY_PORT, format_here, parse_discover};

/// Bind and run the discovery responder forever.
///
/// Returns only on socket failure; malformed requests are logged and skipped.
pub fn run_discovery_listener() -> std::io::Result<()> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)))?;
    println!("discovery listening address=0.0.0.0:{DISCOVERY_PORT}");
    let mut buffer = [0u8; 256];

    loop {
        let (count, source) = socket.recv_from(&mut buffer)?;
        let Some(nonce) = parse_discover(&buffer[..count]) else {
            eprintln!("discovery request invalid from={source}");
            continue;
        };
        eprintln!("discovery request from={source} nonce={nonce}");
        let response = format_here(&nonce, DEFAULT_TCP_PORT);
        socket.send_to(response.as_bytes(), source)?;
        eprintln!("discovery response sent to={source}");
    }
}
