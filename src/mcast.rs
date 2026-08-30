//! UDP and multicast reachability diagnostics for the Kindle Wi-Fi path.
//!
//! The probe binds the same UDP port used by every test packet, joins the
//! mDNS multicast group on a named interface, and distinguishes five paths:
//! local loopback unicast, local interface unicast, local multicast loopback,
//! external unicast, and external multicast. A neutral port can be selected to
//! avoid ownership or filtering specific to mDNS port 5353.

use if_addrs::IfAddr;
use socket2::{Domain, Protocol, Socket, Type};
use std::fs;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::process::Command;
use std::time::{Duration, Instant};

/// mDNS multicast group (RFC 6762).
pub const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
/// mDNS UDP port (RFC 6762).
pub const MDNS_PORT: u16 = 5353;
/// Default Kindle Wi-Fi interface.
pub const DEFAULT_INTERFACE: &str = "wlan0";
/// Marker an external sender uses for the multicast path.
pub const PROBE_MARKER: &[u8] = b"rust-x11-hello-external-multicast";
/// Marker an external sender uses for the unicast control path.
pub const EXTERNAL_UNICAST_MARKER: &[u8] = b"rust-x11-hello-external-unicast";
const LOCAL_LOOPBACK_UNICAST_MARKER: &[u8] = b"rust-x11-hello-local-loopback-unicast";
const LOCAL_INTERFACE_UNICAST_MARKER: &[u8] = b"rust-x11-hello-local-interface-unicast";
const LOCAL_MULTICAST_MARKER: &[u8] = b"rust-x11-hello-local-multicast";
const FIREWALL_CHAIN: &str = "RXH_UDP_TEST";
const PROMISCUOUS_MARKER: &str = "/tmp/rust_x11_hello.promisc-owned";
const IFF_PROMISC: u32 = 0x100;
/// Bound on how long the probe listens.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PacketKind {
    LocalLoopbackUnicast,
    LocalInterfaceUnicast,
    LocalMulticast,
    ExternalUnicast,
    ExternalMulticast,
    ExternalOther,
}

impl PacketKind {
    const fn label(self) -> &'static str {
        match self {
            Self::LocalLoopbackUnicast => "local-loopback-unicast",
            Self::LocalInterfaceUnicast => "local-interface-unicast",
            Self::LocalMulticast => "local-multicast",
            Self::ExternalUnicast => "external-unicast",
            Self::ExternalMulticast => "external-multicast",
            Self::ExternalOther => "external-other",
        }
    }
}

#[derive(Default)]
pub struct ProbeResult {
    local_loopback_unicast: bool,
    local_interface_unicast: bool,
    local_multicast: bool,
    external_unicast: bool,
    external_multicast: bool,
    external_other: u64,
}

impl ProbeResult {
    pub const fn received_external_multicast(&self) -> bool {
        self.external_multicast
    }

    fn record(&mut self, kind: PacketKind) -> bool {
        match kind {
            PacketKind::LocalLoopbackUnicast => {
                let first = !self.local_loopback_unicast;
                self.local_loopback_unicast = true;
                first
            }
            PacketKind::LocalInterfaceUnicast => {
                let first = !self.local_interface_unicast;
                self.local_interface_unicast = true;
                first
            }
            PacketKind::LocalMulticast => {
                let first = !self.local_multicast;
                self.local_multicast = true;
                first
            }
            PacketKind::ExternalUnicast => {
                let first = !self.external_unicast;
                self.external_unicast = true;
                first
            }
            PacketKind::ExternalMulticast => {
                let first = !self.external_multicast;
                self.external_multicast = true;
                first
            }
            PacketKind::ExternalOther => {
                self.external_other += 1;
                self.external_other == 1
            }
        }
    }
}

/// Run one bounded UDP path matrix on the mDNS multicast group.
pub fn probe_multicast(
    interface_name: &str,
    port: u16,
    timeout: Duration,
    temporary_firewall_source: Option<Ipv4Addr>,
    temporary_promiscuous: bool,
) -> anyhow::Result<ProbeResult> {
    if port == 0 {
        return Err(anyhow::anyhow!("multicast probe port must be non-zero"));
    }
    if timeout.is_zero() {
        return Err(anyhow::anyhow!("multicast probe timeout must be non-zero"));
    }

    let interface_ip = interface_ipv4(interface_name)?;
    let bind_addr = probe_bind_addr(port);
    let socket = bind_probe_socket(bind_addr, interface_ip, interface_name)?;
    socket
        .set_multicast_loop_v4(true)
        .map_err(|error| anyhow::anyhow!("multicast probe enable loopback failed: {error}"))?;
    eprintln!(
        "multicast probe start interface={interface_name} address={interface_ip} bind={bind_addr} group={MDNS_GROUP} port={port} timeout_ms={}",
        timeout.as_millis()
    );

    socket
        .join_multicast_v4(&MDNS_GROUP, &interface_ip)
        .map_err(|error| {
            anyhow::anyhow!("multicast probe join {MDNS_GROUP} on {interface_name}: {error}")
        })?;
    eprintln!("multicast probe join interface={interface_name} address={interface_ip} ok");

    let mut promiscuous = temporary_promiscuous
        .then(|| TemporaryPromiscuous::install(interface_name))
        .transpose()?;
    let mut firewall = temporary_firewall_source
        .map(|source| TemporaryFirewall::install(interface_name, source, interface_ip, port))
        .transpose()?;
    log_kernel_network_state();

    let unicast_sender = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0)))
        .map_err(|error| anyhow::anyhow!("multicast probe unicast sender bind failed: {error}"))?;
    send_marker(
        &unicast_sender,
        LOCAL_LOOPBACK_UNICAST_MARKER,
        SocketAddr::from(([127, 0, 0, 1], port)),
        "local-loopback-unicast",
    )?;
    send_marker(
        &unicast_sender,
        LOCAL_INTERFACE_UNICAST_MARKER,
        SocketAddr::from((interface_ip, port)),
        "local-interface-unicast",
    )?;
    send_marker(
        &socket,
        LOCAL_MULTICAST_MARKER,
        SocketAddr::from((MDNS_GROUP, port)),
        "local-multicast",
    )?;

    let mut buffer = [0u8; 512];
    let mut result = ProbeResult::default();
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| anyhow::anyhow!("multicast probe timeout is too large"))?;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        socket
            .set_read_timeout(Some(remaining))
            .map_err(|error| anyhow::anyhow!("multicast probe read timeout failed: {error}"))?;

        match socket.recv_from(&mut buffer) {
            Ok((count, source)) => {
                let packet = &buffer[..count];
                let kind = classify_packet(packet);
                if result.record(kind) {
                    let prefix = escaped_prefix(packet, 48);
                    eprintln!(
                        "multicast probe received-first kind={} from={source} bytes={count} prefix={prefix}",
                        kind.label()
                    );
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => {
                return Err(anyhow::anyhow!("multicast probe receive failed: {error}"));
            }
        }
    }

    eprintln!(
        "multicast probe complete local_loopback_unicast={} local_interface_unicast={} local_multicast={} external_unicast={} external_multicast={} external_other={}",
        result.local_loopback_unicast,
        result.local_interface_unicast,
        result.local_multicast,
        result.external_unicast,
        result.external_multicast,
        result.external_other
    );

    if let Some(firewall) = firewall.as_mut() {
        firewall.log_counters()?;
        firewall.remove()?;
    }
    if let Some(promiscuous) = promiscuous.as_mut() {
        promiscuous.remove()?;
    }
    Ok(result)
}

struct TemporaryPromiscuous {
    interface_name: String,
    changed: bool,
}

impl TemporaryPromiscuous {
    fn install(interface_name: &str) -> anyhow::Result<Self> {
        Self::remove_stale(interface_name)?;
        let flags = interface_flags(interface_name)?;
        if flags & IFF_PROMISC != 0 {
            eprintln!(
                "multicast probe temporary-promiscuous preserved-preexisting interface={interface_name} flags=0x{flags:x}"
            );
            return Ok(Self {
                interface_name: interface_name.to_string(),
                changed: false,
            });
        }

        fs::write(PROMISCUOUS_MARKER, interface_name).map_err(|error| {
            anyhow::anyhow!("write promiscuous ownership marker {PROMISCUOUS_MARKER}: {error}")
        })?;
        let guard = Self {
            interface_name: interface_name.to_string(),
            changed: true,
        };
        run_ip(&["link", "set", "dev", interface_name, "promisc", "on"])?;
        let flags = interface_flags(interface_name)?;
        if flags & IFF_PROMISC == 0 {
            return Err(anyhow::anyhow!(
                "promiscuous mode did not enable on {interface_name}; flags=0x{flags:x}"
            ));
        }
        eprintln!(
            "multicast probe temporary-promiscuous enabled interface={interface_name} flags=0x{flags:x}"
        );
        Ok(guard)
    }

    fn remove(&mut self) -> anyhow::Result<()> {
        if !self.changed {
            return Ok(());
        }
        run_ip(&["link", "set", "dev", &self.interface_name, "promisc", "off"])?;
        let flags = interface_flags(&self.interface_name)?;
        if flags & IFF_PROMISC != 0 {
            return Err(anyhow::anyhow!(
                "promiscuous mode remained enabled on {}; flags=0x{flags:x}",
                self.interface_name
            ));
        }
        fs::remove_file(PROMISCUOUS_MARKER).map_err(|error| {
            anyhow::anyhow!("remove promiscuous ownership marker {PROMISCUOUS_MARKER}: {error}")
        })?;
        self.changed = false;
        eprintln!(
            "multicast probe temporary-promiscuous cleanup-verified interface={} flags=0x{flags:x}",
            self.interface_name
        );
        Ok(())
    }

    fn remove_stale(interface_name: &str) -> anyhow::Result<()> {
        if fs::metadata(PROMISCUOUS_MARKER).is_err() {
            return Ok(());
        }
        run_ip(&["link", "set", "dev", interface_name, "promisc", "off"])?;
        fs::remove_file(PROMISCUOUS_MARKER).map_err(|error| {
            anyhow::anyhow!("remove stale promiscuous marker {PROMISCUOUS_MARKER}: {error}")
        })?;
        let flags = interface_flags(interface_name)?;
        if flags & IFF_PROMISC != 0 {
            return Err(anyhow::anyhow!(
                "stale promiscuous mode remained enabled on {interface_name}; flags=0x{flags:x}"
            ));
        }
        eprintln!(
            "multicast probe temporary-promiscuous stale-cleanup interface={interface_name} flags=0x{flags:x}"
        );
        Ok(())
    }
}

impl Drop for TemporaryPromiscuous {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            eprintln!(
                "multicast probe temporary-promiscuous cleanup-failed interface={} error={error}",
                self.interface_name
            );
        }
    }
}

fn interface_flags(interface_name: &str) -> anyhow::Result<u32> {
    let path = format!("/sys/class/net/{interface_name}/flags");
    let value =
        fs::read_to_string(&path).map_err(|error| anyhow::anyhow!("read {path}: {error}"))?;
    let value = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    u32::from_str_radix(value, 16)
        .map_err(|error| anyhow::anyhow!("parse {path} value={value}: {error}"))
}

fn run_ip(arguments: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("ip")
        .args(arguments)
        .output()
        .map_err(|error| anyhow::anyhow!("run ip {arguments:?} failed: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "ip {arguments:?} failed status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

struct TemporaryFirewall {
    active: bool,
}

impl TemporaryFirewall {
    fn install(
        interface_name: &str,
        external_source: Ipv4Addr,
        local_interface_ip: Ipv4Addr,
        port: u16,
    ) -> anyhow::Result<Self> {
        Self::remove_stale();
        run_iptables(&["-N", FIREWALL_CHAIN])?;
        let guard = Self { active: true };
        let external_source = format!("{external_source}/32");
        let local_interface_ip = format!("{local_interface_ip}/32");
        let multicast_group = format!("{MDNS_GROUP}/32");
        let port = port.to_string();
        run_iptables(&[
            "-A",
            FIREWALL_CHAIN,
            "-i",
            interface_name,
            "-s",
            &external_source,
            "-d",
            &local_interface_ip,
            "-p",
            "udp",
            "--dport",
            &port,
            "-j",
            "ACCEPT",
        ])?;
        run_iptables(&[
            "-A",
            FIREWALL_CHAIN,
            "-i",
            interface_name,
            "-s",
            &external_source,
            "-d",
            &multicast_group,
            "-p",
            "udp",
            "--dport",
            &port,
            "-j",
            "ACCEPT",
        ])?;
        run_iptables(&[
            "-A",
            FIREWALL_CHAIN,
            "-i",
            "lo",
            "-s",
            &local_interface_ip,
            "-d",
            &local_interface_ip,
            "-p",
            "udp",
            "--dport",
            &port,
            "-j",
            "ACCEPT",
        ])?;
        run_iptables(&[
            "-A",
            FIREWALL_CHAIN,
            "-s",
            &local_interface_ip,
            "-d",
            &multicast_group,
            "-p",
            "udp",
            "--dport",
            &port,
            "-j",
            "ACCEPT",
        ])?;
        run_iptables(&["-I", "INPUT", "1", "-j", FIREWALL_CHAIN])?;
        eprintln!(
            "multicast probe temporary-firewall installed chain={FIREWALL_CHAIN} interface={interface_name} source={external_source} local_source={local_interface_ip} group={multicast_group} port={port} rule_order=external-unicast,external-multicast,local-interface-unicast,local-multicast"
        );
        Ok(guard)
    }

    fn log_counters(&self) -> anyhow::Result<()> {
        let output = Command::new("iptables")
            .args(["-L", FIREWALL_CHAIN, "-n", "-v", "--line-numbers"])
            .output()
            .map_err(|error| anyhow::anyhow!("read {FIREWALL_CHAIN} counters failed: {error}"))?;
        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "read {FIREWALL_CHAIN} counters failed status={} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        eprintln!("multicast probe temporary-firewall counters begin");
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            eprintln!("multicast probe temporary-firewall counters {line}");
        }
        eprintln!("multicast probe temporary-firewall counters end");
        Ok(())
    }

    fn remove(&mut self) -> anyhow::Result<()> {
        if !self.active {
            return Ok(());
        }
        while try_iptables(&["-D", "INPUT", "-j", FIREWALL_CHAIN])? {}
        run_iptables(&["-F", FIREWALL_CHAIN])?;
        run_iptables(&["-X", FIREWALL_CHAIN])?;
        self.active = false;
        eprintln!("multicast probe temporary-firewall removed chain={FIREWALL_CHAIN}");
        if try_iptables(&["-L", FIREWALL_CHAIN, "-n"])? {
            return Err(anyhow::anyhow!(
                "temporary firewall chain {FIREWALL_CHAIN} still exists after removal"
            ));
        }
        eprintln!(
            "multicast probe temporary-firewall cleanup-verified chain={FIREWALL_CHAIN} absent"
        );
        Ok(())
    }

    fn remove_stale() {
        for _ in 0..16 {
            match try_iptables(&["-D", "INPUT", "-j", FIREWALL_CHAIN]) {
                Ok(true) => continue,
                Ok(false) | Err(_) => break,
            }
        }
        let _ = try_iptables(&["-F", FIREWALL_CHAIN]);
        let _ = try_iptables(&["-X", FIREWALL_CHAIN]);
    }
}

impl Drop for TemporaryFirewall {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            eprintln!(
                "multicast probe temporary-firewall cleanup-failed chain={FIREWALL_CHAIN} error={error}"
            );
        }
    }
}

fn run_iptables(arguments: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("iptables")
        .args(arguments)
        .output()
        .map_err(|error| anyhow::anyhow!("run iptables {arguments:?} failed: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "iptables {arguments:?} failed status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn try_iptables(arguments: &[&str]) -> anyhow::Result<bool> {
    Command::new("iptables")
        .args(arguments)
        .output()
        .map(|output| output.status.success())
        .map_err(|error| anyhow::anyhow!("run iptables {arguments:?} failed: {error}"))
}

fn classify_packet(packet: &[u8]) -> PacketKind {
    match packet {
        LOCAL_LOOPBACK_UNICAST_MARKER => PacketKind::LocalLoopbackUnicast,
        LOCAL_INTERFACE_UNICAST_MARKER => PacketKind::LocalInterfaceUnicast,
        LOCAL_MULTICAST_MARKER => PacketKind::LocalMulticast,
        EXTERNAL_UNICAST_MARKER => PacketKind::ExternalUnicast,
        PROBE_MARKER => PacketKind::ExternalMulticast,
        _ => PacketKind::ExternalOther,
    }
}

fn send_marker(
    socket: &UdpSocket,
    marker: &[u8],
    destination: SocketAddr,
    kind: &str,
) -> anyhow::Result<()> {
    let sent = socket.send_to(marker, destination).map_err(|error| {
        anyhow::anyhow!("multicast probe send {kind} to {destination} failed: {error}")
    })?;
    eprintln!("multicast probe sent kind={kind} destination={destination} bytes={sent}");
    Ok(())
}

fn probe_bind_addr(port: u16) -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], port))
}

fn escaped_prefix(packet: &[u8], limit: usize) -> String {
    packet
        .iter()
        .take(limit)
        .flat_map(|byte| std::ascii::escape_default(*byte))
        .map(char::from)
        .collect()
}

fn bind_probe_socket(
    bind_addr: SocketAddr,
    interface_ip: Ipv4Addr,
    interface_name: &str,
) -> anyhow::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|error| anyhow::anyhow!("multicast probe socket failed: {error}"))?;
    socket
        .set_reuse_address(true)
        .map_err(|error| anyhow::anyhow!("multicast probe SO_REUSEADDR failed: {error}"))?;
    #[cfg(target_vendor = "apple")]
    socket
        .set_reuse_port(true)
        .map_err(|error| anyhow::anyhow!("multicast probe SO_REUSEPORT failed: {error}"))?;
    socket
        .set_multicast_if_v4(&interface_ip)
        .map_err(|error| anyhow::anyhow!("multicast probe select {interface_name}: {error}"))?;
    socket
        .bind(&bind_addr.into())
        .map_err(|error| anyhow::anyhow!("multicast probe bind {bind_addr} failed: {error}"))?;
    Ok(socket.into())
}

fn interface_ipv4(interface_name: &str) -> anyhow::Result<Ipv4Addr> {
    let mut addresses: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .map_err(|error| anyhow::anyhow!("multicast probe list interfaces failed: {error}"))?
        .into_iter()
        .filter_map(|interface| {
            if interface.name != interface_name || !interface.is_oper_up() {
                return None;
            }
            match interface.addr {
                IfAddr::V4(address) if !address.ip.is_loopback() => Some(address.ip),
                _ => None,
            }
        })
        .collect();
    addresses.sort_by_key(|address| (address.is_link_local(), *address));
    addresses.first().copied().ok_or_else(|| {
        anyhow::anyhow!("multicast probe interface {interface_name} has no active IPv4 address")
    })
}

fn log_kernel_network_state() {
    for path in [
        "/proc/net/igmp",
        "/proc/net/dev_mcast",
        "/proc/net/route",
        "/proc/net/udp",
        "/proc/net/ip_tables_names",
        "/proc/sys/net/ipv4/conf/all/rp_filter",
        "/proc/sys/net/ipv4/conf/wlan0/rp_filter",
        "/proc/sys/net/ipv4/conf/wlan0/accept_local",
        "/sys/class/net/wlan0/flags",
    ] {
        match fs::read_to_string(path) {
            Ok(contents) => {
                eprintln!("multicast probe kernel-state path={path} begin");
                for line in contents.lines() {
                    eprintln!("multicast probe kernel-state {line}");
                }
                eprintln!("multicast probe kernel-state path={path} end");
            }
            Err(error) => {
                eprintln!("multicast probe kernel-state path={path} unavailable error={error}");
            }
        }
    }

    for (label, programs, arguments) in [
        (
            "ip-rule",
            &["ip", "/sbin/ip", "/usr/sbin/ip"][..],
            &["rule", "show"][..],
        ),
        (
            "ip-route-all",
            &["ip", "/sbin/ip", "/usr/sbin/ip"][..],
            &["route", "show", "table", "all"][..],
        ),
        (
            "iptables-filter",
            &["iptables", "/sbin/iptables", "/usr/sbin/iptables"][..],
            &["-t", "filter", "-L", "-n", "-v", "--line-numbers"][..],
        ),
        (
            "iptables-raw",
            &["iptables", "/sbin/iptables", "/usr/sbin/iptables"][..],
            &["-t", "raw", "-L", "-n", "-v", "--line-numbers"][..],
        ),
        (
            "iptables-mangle",
            &["iptables", "/sbin/iptables", "/usr/sbin/iptables"][..],
            &["-t", "mangle", "-L", "-n", "-v", "--line-numbers"][..],
        ),
        (
            "netstat-udp",
            &["netstat", "/bin/netstat", "/usr/bin/netstat"][..],
            &["-a", "-n", "-u"][..],
        ),
    ] {
        log_first_available_command(label, programs, arguments);
    }
}

fn log_first_available_command(label: &str, programs: &[&str], arguments: &[&str]) {
    for program in programs {
        match Command::new(program).args(arguments).output() {
            Ok(output) => {
                eprintln!(
                    "multicast probe command label={label} program={program} status={} begin",
                    output.status
                );
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    eprintln!("multicast probe command stdout {line}");
                }
                for line in String::from_utf8_lossy(&output.stderr).lines() {
                    eprintln!("multicast probe command stderr {line}");
                }
                eprintln!("multicast probe command label={label} end");
                return;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                eprintln!(
                    "multicast probe command label={label} program={program} unavailable error={error}"
                );
                return;
            }
        }
    }
    eprintln!("multicast probe command label={label} unavailable no-program-found");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receiver_binds_the_packet_destination_port() {
        assert_eq!(
            probe_bind_addr(5582),
            SocketAddr::from(([0, 0, 0, 0], 5582))
        );
    }

    #[test]
    fn every_matrix_marker_has_a_distinct_classification() {
        assert_eq!(
            classify_packet(LOCAL_LOOPBACK_UNICAST_MARKER),
            PacketKind::LocalLoopbackUnicast
        );
        assert_eq!(
            classify_packet(LOCAL_INTERFACE_UNICAST_MARKER),
            PacketKind::LocalInterfaceUnicast
        );
        assert_eq!(
            classify_packet(LOCAL_MULTICAST_MARKER),
            PacketKind::LocalMulticast
        );
        assert_eq!(
            classify_packet(EXTERNAL_UNICAST_MARKER),
            PacketKind::ExternalUnicast
        );
        assert_eq!(classify_packet(PROBE_MARKER), PacketKind::ExternalMulticast);
        assert_eq!(classify_packet(b"other"), PacketKind::ExternalOther);
    }

    #[test]
    fn binary_packet_prefix_is_log_safe() {
        assert_eq!(escaped_prefix(&[0, b'A', b'\n', 0xff], 4), "\\x00A\\n\\xff");
    }
}
