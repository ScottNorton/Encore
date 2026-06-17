//! Minimal mDNS responder for `.local` hostname + DNS-SD service discovery.
//!
//! Single socket on UDP 5353, joins 224.0.0.251 multicast. Handles:
//! - A-record queries for hostname (e.g. `encore.local`)
//! - PTR/SRV/TXT queries for registered services (e.g. `_spotify-connect._tcp`)
//!
//! Unlike libmdns, this responder tolerates the AD/CD bits that modern
//! clients (Spotify, Chrome, Android) set in their mDNS queries.

use anyhow::Result;
use std::net::Ipv4Addr;
use std::os::unix::io::FromRawFd;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tracing::{info, warn};

const MDNS_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;
const RECORD_TTL: u32 = 120;

// DNS record types.
const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_TXT: u16 = 16;
const TYPE_SRV: u16 = 33;
const TYPE_AAAA: u16 = 28;
const TYPE_ANY: u16 = 255;

/// A discovered peer from an mDNS response.
#[derive(Debug, Clone)]
pub struct MdnsDiscovery {
    pub peer_id: String,
    pub name: String,
    pub address: std::net::IpAddr,
    pub port: u16,
    pub group_name: String,
    pub channel: String,
}

/// A DNS-SD service to advertise via mDNS.
#[derive(Clone)]
pub struct MdnsService {
    /// e.g. `"_spotify-connect._tcp"`
    pub service_type: String,
    /// e.g. `"Invoke"`
    pub instance_name: String,
    /// TCP port the service listens on.
    pub port: u16,
    /// TXT record strings, e.g. `["VERSION=1.0", "CPath=/"]`.
    pub txt: Vec<String>,
}

/// Handle to the background mDNS responder task.
pub struct MdnsResponder {
    handle: JoinHandle<()>,
}

impl MdnsResponder {
    /// Start the mDNS responder.
    ///
    /// `hostname` should be like `"encore.local"`. `ip` is the address to
    /// advertise (wlan0 IP). `services` is a list of DNS-SD services to
    /// register (can be empty). `discovery_tx` receives discovered group peers.
    pub fn start(
        hostname: &str,
        ip: Ipv4Addr,
        services: Vec<MdnsService>,
        discovery_tx: Option<tokio::sync::mpsc::Sender<MdnsDiscovery>>,
        our_peer_id: String,
    ) -> Result<Self> {
        let socket = bind_mdns_socket(ip)?;

        // Pre-compute all the DNS wire-format data we'll need.
        let host_labels = encode_dns_name(hostname);
        let ip_bytes = ip.octets();
        let a_record = build_a_record(&host_labels, &ip_bytes);

        let svc_data: Vec<ServiceData> = services
            .iter()
            .map(|s| ServiceData::new(s, &host_labels, &ip_bytes))
            .collect();

        info!("mDNS: responding as {} -> {}", hostname, ip);
        for s in &services {
            info!(
                "mDNS: service {} ({}) port {}",
                s.service_type, s.instance_name, s.port
            );
        }

        // Build gratuitous announcement with all records.
        let announce = build_announcement(&a_record, &svc_data);

        // Build periodic PTR query for group discovery
        let group_query = build_ptr_query("_encore-group._tcp.local");

        let handle = tokio::spawn(async move {
            // Announce on startup.
            let _ = socket.send_to(&announce, (MDNS_ADDR, MDNS_PORT)).await;

            // Send initial group discovery query
            let _ = socket.send_to(&group_query, (MDNS_ADDR, MDNS_PORT)).await;

            let mut buf = [0u8; 1500];
            let mut query_interval = tokio::time::interval(std::time::Duration::from_secs(30));

            loop {
                tokio::select! {
                    result = socket.recv_from(&mut buf) => {
                        match result {
                            Ok((len, src)) => {
                                let pkt = &buf[..len];
                                if pkt.len() >= 12 && pkt[2] & 0x80 != 0 {
                                    // Response packet (QR=1) — parse for group discovery
                                    if let Some(ref tx) = discovery_tx {
                                        if let Some(discovery) = handle_response(pkt, &our_peer_id) {
                                            let _ = tx.try_send(discovery);
                                        }
                                    }
                                } else {
                                    // Query packet — respond
                                    if let Some(resp) = handle_query(
                                        pkt,
                                        &host_labels,
                                        &a_record,
                                        &svc_data,
                                    ) {
                                        let _ = socket.send_to(&resp, (MDNS_ADDR, MDNS_PORT)).await;
                                        if src.ip() != std::net::IpAddr::V4(MDNS_ADDR) {
                                            let _ = socket.send_to(&resp, src).await;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("mDNS: recv error: {}", e);
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            }
                        }
                    }
                    _ = query_interval.tick() => {
                        // Periodic group discovery query
                        let _ = socket.send_to(&group_query, (MDNS_ADDR, MDNS_PORT)).await;
                    }
                }
            }
        });

        Ok(Self { handle })
    }

    pub fn shutdown(self) {
        self.handle.abort();
    }
}

// ---------------------------------------------------------------------------
// Pre-computed service wire data
// ---------------------------------------------------------------------------

/// All the pre-built DNS record bytes for one service.
struct ServiceData {
    /// `_spotify-connect._tcp.local.` encoded
    svc_type_labels: Vec<u8>,
    /// `Invoke._spotify-connect._tcp.local.` encoded
    instance_labels: Vec<u8>,
    /// PTR record: svc_type → instance
    ptr_record: Vec<u8>,
    /// SRV record: instance → host:port
    srv_record: Vec<u8>,
    /// TXT record: instance → key=value pairs
    txt_record: Vec<u8>,
    /// A record for the hostname (same for all services)
    a_record: Vec<u8>,
}

impl ServiceData {
    fn new(svc: &MdnsService, host_labels: &[u8], ip_bytes: &[u8; 4]) -> Self {
        let svc_fqdn = format!("{}.local", svc.service_type);
        let instance_fqdn = format!("{}.{}.local", svc.instance_name, svc.service_type);

        let svc_type_labels = encode_dns_name(&svc_fqdn);
        let instance_labels = encode_dns_name(&instance_fqdn);

        let ptr_record = build_ptr_record(&svc_type_labels, &instance_labels);
        let srv_record = build_srv_record(&instance_labels, host_labels, svc.port);
        let txt_record = build_txt_record(&instance_labels, &svc.txt);
        let a_record = build_a_record(host_labels, ip_bytes);

        Self {
            svc_type_labels,
            instance_labels,
            ptr_record,
            srv_record,
            txt_record,
            a_record,
        }
    }
}

// ---------------------------------------------------------------------------
// Query handling
// ---------------------------------------------------------------------------

/// Parse an incoming mDNS query and return a response if any question matches.
fn handle_query(
    pkt: &[u8],
    host_labels: &[u8],
    a_record: &[u8],
    services: &[ServiceData],
) -> Option<Vec<u8>> {
    if pkt.len() < 12 {
        return None;
    }
    // Must be a query (QR bit = 0).
    if pkt[2] & 0x80 != 0 {
        return None;
    }
    let qdcount = u16::from_be_bytes([pkt[4], pkt[5]]);
    if qdcount == 0 {
        return None;
    }

    let mut answers: Vec<&[u8]> = Vec::new();
    let mut additional: Vec<&[u8]> = Vec::new();

    let mut pos = 12;
    for _ in 0..qdcount {
        let name_start = pos;
        // Skip past QNAME.
        loop {
            if pos >= pkt.len() {
                return None;
            }
            let label_len = pkt[pos] as usize;
            if label_len == 0 {
                pos += 1;
                break;
            }
            if label_len & 0xC0 != 0 {
                // Compressed name — skip 2 bytes.
                pos += 2;
                break;
            }
            pos += 1 + label_len;
        }
        if pos + 4 > pkt.len() {
            return None;
        }
        let qtype = u16::from_be_bytes([pkt[pos], pkt[pos + 1]]);
        let name_bytes = &pkt[name_start..pos];
        pos += 4; // skip QTYPE + QCLASS

        // Check hostname A record.
        if (qtype == TYPE_A || qtype == TYPE_ANY) && names_equal(name_bytes, host_labels) {
            answers.push(a_record);
        }

        // Check service records.
        for svc in services {
            // PTR query for the service type.
            if (qtype == TYPE_PTR || qtype == TYPE_ANY)
                && names_equal(name_bytes, &svc.svc_type_labels)
            {
                answers.push(&svc.ptr_record);
                additional.push(&svc.srv_record);
                additional.push(&svc.txt_record);
                additional.push(&svc.a_record);
            }

            // SRV query for the instance.
            if (qtype == TYPE_SRV || qtype == TYPE_ANY)
                && names_equal(name_bytes, &svc.instance_labels)
            {
                answers.push(&svc.srv_record);
                additional.push(&svc.a_record);
            }

            // TXT query for the instance.
            if (qtype == TYPE_TXT || qtype == TYPE_ANY)
                && names_equal(name_bytes, &svc.instance_labels)
            {
                answers.push(&svc.txt_record);
            }
        }
    }

    if answers.is_empty() {
        return None;
    }

    // De-duplicate additional records that are already in answers.
    additional.retain(|a| !answers.contains(a));
    // De-duplicate within additional.
    additional.dedup();

    Some(build_response(&answers, &additional))
}

/// Parse an mDNS response for `_encore-group._tcp` service advertisements.
/// Returns an MdnsDiscovery if a valid group peer is found that isn't us.
fn handle_response(pkt: &[u8], our_peer_id: &str) -> Option<MdnsDiscovery> {
    if pkt.len() < 12 {
        return None;
    }

    // Skip questions section (handle responses with 0 questions)
    let mut pos = 12;
    let qdcount = u16::from_be_bytes([pkt[4], pkt[5]]);
    let ancount = u16::from_be_bytes([pkt[6], pkt[7]]);
    let _nscount = u16::from_be_bytes([pkt[8], pkt[9]]);
    let arcount = u16::from_be_bytes([pkt[10], pkt[11]]);

    // Skip question section
    for _ in 0..qdcount {
        pos = skip_dns_name(pkt, pos)?;
        if pos + 4 > pkt.len() {
            return None;
        }
        pos += 4; // QTYPE + QCLASS
    }

    // State accumulated across answer + additional records
    let mut instance_name = String::new();
    let mut port: u16 = 0;
    let mut address: Option<std::net::IpAddr> = None;
    let mut txt_entries: Vec<String> = Vec::new();
    let mut found_group_service = false;

    // Parse answer + additional sections
    let total_rr = ancount as usize + arcount as usize;
    for _ in 0..total_rr {
        if pos >= pkt.len() {
            break;
        }

        let name = decode_dns_name_compressed(pkt, pos);
        pos = skip_dns_name(pkt, pos)?;
        if pos + 10 > pkt.len() {
            break;
        }

        let rtype = u16::from_be_bytes([pkt[pos], pkt[pos + 1]]);
        let _rclass = u16::from_be_bytes([pkt[pos + 2], pkt[pos + 3]]);
        let _ttl = u32::from_be_bytes([pkt[pos + 4], pkt[pos + 5], pkt[pos + 6], pkt[pos + 7]]);
        let rdlen = u16::from_be_bytes([pkt[pos + 8], pkt[pos + 9]]) as usize;
        pos += 10;

        if pos + rdlen > pkt.len() {
            break;
        }
        let rdata = &pkt[pos..pos + rdlen];

        match rtype {
            TYPE_PTR => {
                if let Some(ref n) = name {
                    if n.contains("_encore-group._tcp") {
                        found_group_service = true;
                        if let Some(inst) = decode_dns_name_compressed(pkt, pos) {
                            instance_name = inst;
                        }
                    }
                }
            }
            TYPE_SRV => {
                if rdlen >= 6 {
                    port = u16::from_be_bytes([rdata[4], rdata[5]]);
                }
            }
            TYPE_TXT => {
                let mut tpos = 0;
                while tpos < rdata.len() {
                    let tlen = rdata[tpos] as usize;
                    tpos += 1;
                    if tpos + tlen > rdata.len() {
                        break;
                    }
                    if let Ok(s) = std::str::from_utf8(&rdata[tpos..tpos + tlen]) {
                        txt_entries.push(s.to_string());
                    }
                    tpos += tlen;
                }
            }
            TYPE_A => {
                if rdata.len() == 4 {
                    address = Some(std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                        rdata[0], rdata[1], rdata[2], rdata[3],
                    )));
                }
            }
            TYPE_AAAA => {
                if rdata.len() == 16 {
                    let octets: [u8; 16] = rdata.try_into().unwrap();
                    address = Some(std::net::IpAddr::V6(std::net::Ipv6Addr::from(octets)));
                }
            }
            _ => {}
        }

        pos += rdlen;
    }

    if !found_group_service || address.is_none() || port == 0 {
        return None;
    }

    // Parse TXT records for peer metadata
    let mut peer_id = String::new();
    let mut group_name = String::new();
    let mut channel = String::new();
    for entry in &txt_entries {
        if let Some(val) = entry.strip_prefix("id=") {
            peer_id = val.to_string();
        } else if let Some(val) = entry.strip_prefix("group=") {
            group_name = val.to_string();
        } else if let Some(val) = entry.strip_prefix("ch=") {
            channel = val.to_string();
        } else if let Some(val) = entry.strip_prefix("channel=") {
            channel = val.to_string();
        }
    }

    // Skip our own announcements
    if peer_id == our_peer_id || peer_id.is_empty() {
        return None;
    }

    // Extract display name from instance name (before the service type)
    let display_name = instance_name
        .split("._encore-group")
        .next()
        .unwrap_or(&instance_name)
        .to_string();

    Some(MdnsDiscovery {
        peer_id,
        name: display_name,
        address: address.unwrap(),
        port,
        group_name,
        channel,
    })
}

/// Decode a DNS name with compression pointer support (0xC0 prefix).
/// Returns the decoded dotted name, or None if malformed.
fn decode_dns_name_compressed(pkt: &[u8], mut pos: usize) -> Option<String> {
    let mut parts = Vec::new();
    let mut jumps = 0;
    loop {
        if pos >= pkt.len() || jumps > 10 {
            return None;
        }
        let len = pkt[pos] as usize;
        if len == 0 {
            break;
        }
        if len & 0xC0 == 0xC0 {
            // Compression pointer
            if pos + 1 >= pkt.len() {
                return None;
            }
            let offset = ((len & 0x3F) << 8) | (pkt[pos + 1] as usize);
            pos = offset;
            jumps += 1;
            continue;
        }
        pos += 1;
        if pos + len > pkt.len() {
            return None;
        }
        if let Ok(s) = std::str::from_utf8(&pkt[pos..pos + len]) {
            parts.push(s.to_string());
        }
        pos += len;
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("."))
    }
}

/// Skip past a DNS name (handles both labels and compression pointers).
/// Returns the position after the name.
fn skip_dns_name(pkt: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        if pos >= pkt.len() {
            return None;
        }
        let len = pkt[pos] as usize;
        if len == 0 {
            return Some(pos + 1);
        }
        if len & 0xC0 == 0xC0 {
            // Compression pointer — 2 bytes total
            return Some(pos + 2);
        }
        pos += 1 + len;
    }
}

/// Build a PTR query for a given service type (e.g. "_encore-group._tcp.local").
fn build_ptr_query(service: &str) -> Vec<u8> {
    let labels = encode_dns_name(service);
    let mut pkt = Vec::with_capacity(12 + labels.len() + 4);
    pkt.extend_from_slice(&[0x00, 0x00]); // ID
    pkt.extend_from_slice(&[0x00, 0x00]); // Flags: query
    pkt.extend_from_slice(&[0x00, 0x01]); // QDCOUNT = 1
    pkt.extend_from_slice(&[0x00, 0x00]); // ANCOUNT
    pkt.extend_from_slice(&[0x00, 0x00]); // NSCOUNT
    pkt.extend_from_slice(&[0x00, 0x00]); // ARCOUNT
    pkt.extend_from_slice(&labels);
    pkt.extend_from_slice(&TYPE_PTR.to_be_bytes());
    pkt.extend_from_slice(&[0x00, 0x01]); // QCLASS = IN
    pkt
}

// ---------------------------------------------------------------------------
// Socket setup
// ---------------------------------------------------------------------------

fn bind_mdns_socket(iface_ip: Ipv4Addr) -> Result<UdpSocket> {
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if fd < 0 {
            return Err(anyhow::anyhow!(
                "socket() failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let one: libc::c_int = 1;
        setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, &one);
        setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT, &one);

        let ttl: libc::c_int = 255;
        setsockopt(fd, libc::IPPROTO_IP, libc::IP_MULTICAST_TTL, &ttl);

        let mc_if = libc::in_addr {
            s_addr: u32::from(iface_ip).to_be(),
        };
        setsockopt(fd, libc::IPPROTO_IP, libc::IP_MULTICAST_IF, &mc_if);

        let addr = libc::sockaddr_in {
            sin_family: libc::AF_INET as _,
            sin_port: MDNS_PORT.to_be(),
            sin_addr: libc::in_addr { s_addr: 0 },
            sin_zero: [0; 8],
        };
        if libc::bind(
            fd,
            &addr as *const _ as _,
            std::mem::size_of_val(&addr) as _,
        ) < 0
        {
            let err = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(anyhow::anyhow!("bind(:5353) failed: {}", err));
        }

        let mreq = libc::ip_mreq {
            imr_multiaddr: libc::in_addr {
                s_addr: u32::from(MDNS_ADDR).to_be(),
            },
            imr_interface: libc::in_addr {
                s_addr: u32::from(iface_ip).to_be(),
            },
        };
        if libc::setsockopt(
            fd,
            libc::IPPROTO_IP,
            libc::IP_ADD_MEMBERSHIP,
            &mreq as *const _ as _,
            std::mem::size_of_val(&mreq) as libc::socklen_t,
        ) < 0
        {
            let err = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(anyhow::anyhow!("IP_ADD_MEMBERSHIP failed: {}", err));
        }

        libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);

        let std_sock = std::net::UdpSocket::from_raw_fd(fd);
        Ok(UdpSocket::from_std(std_sock)?)
    }
}

unsafe fn setsockopt<T>(fd: libc::c_int, level: libc::c_int, name: libc::c_int, val: &T) {
    libc::setsockopt(
        fd,
        level,
        name,
        val as *const _ as _,
        std::mem::size_of_val(val) as libc::socklen_t,
    );
}

// ---------------------------------------------------------------------------
// DNS wire format helpers
// ---------------------------------------------------------------------------

/// Encode a name like `"encore.local"` or `"Invoke._spotify-connect._tcp.local"`
/// into DNS label format.
fn encode_dns_name(name: &str) -> Vec<u8> {
    let name = name.strip_suffix('.').unwrap_or(name);
    let mut labels = Vec::new();
    for part in name.split('.') {
        labels.push(part.len() as u8);
        labels.extend_from_slice(part.as_bytes());
    }
    labels.push(0);
    labels
}

/// Case-insensitive comparison of DNS-encoded names.
fn names_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        let len_a = a[i] as usize;
        let len_b = b[i] as usize;
        if len_a != len_b {
            return false;
        }
        if len_a == 0 {
            return true;
        }
        i += 1;
        if i + len_a > a.len() {
            return false;
        }
        for j in 0..len_a {
            if !a[i + j].eq_ignore_ascii_case(&b[i + j]) {
                return false;
            }
        }
        i += len_a;
    }
    true
}

// --- Record builders ---

fn build_a_record(name_labels: &[u8], ip: &[u8; 4]) -> Vec<u8> {
    let mut r = Vec::with_capacity(name_labels.len() + 14);
    r.extend_from_slice(name_labels);
    r.extend_from_slice(&TYPE_A.to_be_bytes());
    r.extend_from_slice(&[0x80, 0x01]); // CLASS IN | cache-flush
    r.extend_from_slice(&RECORD_TTL.to_be_bytes());
    r.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
    r.extend_from_slice(ip);
    r
}

fn build_ptr_record(svc_type_labels: &[u8], instance_labels: &[u8]) -> Vec<u8> {
    let mut r = Vec::new();
    r.extend_from_slice(svc_type_labels);
    r.extend_from_slice(&TYPE_PTR.to_be_bytes());
    r.extend_from_slice(&[0x00, 0x01]); // CLASS IN (no cache-flush for PTR)
    r.extend_from_slice(&RECORD_TTL.to_be_bytes());
    r.extend_from_slice(&(instance_labels.len() as u16).to_be_bytes());
    r.extend_from_slice(instance_labels);
    r
}

fn build_srv_record(instance_labels: &[u8], host_labels: &[u8], port: u16) -> Vec<u8> {
    // RDATA: priority(2) + weight(2) + port(2) + target
    let rdlen = 6 + host_labels.len();
    let mut r = Vec::new();
    r.extend_from_slice(instance_labels);
    r.extend_from_slice(&TYPE_SRV.to_be_bytes());
    r.extend_from_slice(&[0x80, 0x01]); // CLASS IN | cache-flush
    r.extend_from_slice(&RECORD_TTL.to_be_bytes());
    r.extend_from_slice(&(rdlen as u16).to_be_bytes());
    r.extend_from_slice(&0u16.to_be_bytes()); // priority
    r.extend_from_slice(&0u16.to_be_bytes()); // weight
    r.extend_from_slice(&port.to_be_bytes());
    r.extend_from_slice(host_labels);
    r
}

fn build_txt_record(instance_labels: &[u8], txt: &[String]) -> Vec<u8> {
    // TXT RDATA: sequence of length-prefixed strings.
    let mut rdata = Vec::new();
    for s in txt {
        rdata.push(s.len() as u8);
        rdata.extend_from_slice(s.as_bytes());
    }
    if rdata.is_empty() {
        rdata.push(0); // Empty TXT record must have a single zero byte.
    }
    let mut r = Vec::new();
    r.extend_from_slice(instance_labels);
    r.extend_from_slice(&TYPE_TXT.to_be_bytes());
    r.extend_from_slice(&[0x80, 0x01]); // CLASS IN | cache-flush
    r.extend_from_slice(&RECORD_TTL.to_be_bytes());
    r.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    r.extend_from_slice(&rdata);
    r
}

// --- Packet builders ---

fn build_response(answers: &[&[u8]], additional: &[&[u8]]) -> Vec<u8> {
    let an_count = answers.len() as u16;
    let ar_count = additional.len() as u16;
    let body_len: usize = answers.iter().map(|a| a.len()).sum::<usize>()
        + additional.iter().map(|a| a.len()).sum::<usize>();
    let mut pkt = Vec::with_capacity(12 + body_len);
    pkt.extend_from_slice(&[0x00, 0x00]); // ID
    pkt.extend_from_slice(&[0x84, 0x00]); // Flags: QR=1, AA=1
    pkt.extend_from_slice(&[0x00, 0x00]); // QDCOUNT
    pkt.extend_from_slice(&an_count.to_be_bytes());
    pkt.extend_from_slice(&[0x00, 0x00]); // NSCOUNT
    pkt.extend_from_slice(&ar_count.to_be_bytes());
    for a in answers {
        pkt.extend_from_slice(a);
    }
    for a in additional {
        pkt.extend_from_slice(a);
    }
    pkt
}

fn build_announcement(a_record: &[u8], services: &[ServiceData]) -> Vec<u8> {
    let mut answers: Vec<&[u8]> = vec![a_record];
    for svc in services {
        answers.push(&svc.ptr_record);
        answers.push(&svc.srv_record);
        answers.push(&svc.txt_record);
    }
    build_response(&answers, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_dns_name_basic() {
        let encoded = encode_dns_name("encore.local");
        assert_eq!(
            encoded,
            vec![6, b'e', b'n', b'c', b'o', b'r', b'e', 5, b'l', b'o', b'c', b'a', b'l', 0]
        );
    }

    #[test]
    fn encode_dns_name_trailing_dot() {
        assert_eq!(
            encode_dns_name("encore.local."),
            encode_dns_name("encore.local")
        );
    }

    #[test]
    fn encode_dns_name_service_instance() {
        let encoded = encode_dns_name("Invoke._spotify-connect._tcp.local");
        // "Invoke" = 6 bytes, "_spotify-connect" = 16, "_tcp" = 4, "local" = 5
        assert_eq!(encoded[0], 6);
        assert_eq!(&encoded[1..7], b"Invoke");
        assert_eq!(encoded[7], 16);
        assert_eq!(&encoded[8..24], b"_spotify-connect");
        assert_eq!(encoded[24], 4);
        assert_eq!(&encoded[25..29], b"_tcp");
    }

    #[test]
    fn names_equal_case_insensitive() {
        let a = encode_dns_name("Encore.Local");
        let b = encode_dns_name("encore.local");
        assert!(names_equal(&a, &b));
    }

    #[test]
    fn names_equal_different() {
        let a = encode_dns_name("encore.local");
        let b = encode_dns_name("other.local");
        assert!(!names_equal(&a, &b));
    }

    fn make_query(name: &str, qtype: u16) -> Vec<u8> {
        let labels = encode_dns_name(name);
        let mut pkt = vec![
            0x00, 0x00, // ID
            0x00, 0x00, // Flags (query)
            0x00, 0x01, // QDCOUNT = 1
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        pkt.extend_from_slice(&labels);
        pkt.extend_from_slice(&qtype.to_be_bytes());
        pkt.extend_from_slice(&[0x00, 0x01]); // QCLASS = IN
        pkt
    }

    fn make_query_with_ad_bit(name: &str, qtype: u16) -> Vec<u8> {
        let mut pkt = make_query(name, qtype);
        // Set AD bit (bit 5 of flags byte 3 = 0x20)
        pkt[3] |= 0x20;
        pkt
    }

    #[test]
    fn handle_a_query() {
        let host = encode_dns_name("encore.local");
        let ip = [192, 168, 7, 235];
        let a_rec = build_a_record(&host, &ip);
        let pkt = make_query("encore.local", TYPE_A);
        let resp = handle_query(&pkt, &host, &a_rec, &[]);
        assert!(resp.is_some());
        let resp = resp.unwrap();
        assert_eq!(resp[2], 0x84); // QR=1, AA=1
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 1); // 1 answer
                                                               // Last 4 bytes = IP
        assert_eq!(&resp[resp.len() - 4..], &ip);
    }

    #[test]
    fn handle_a_query_with_ad_bit() {
        // Queries with AD bit set must NOT be rejected.
        let host = encode_dns_name("encore.local");
        let ip = [192, 168, 7, 235];
        let a_rec = build_a_record(&host, &ip);
        let pkt = make_query_with_ad_bit("encore.local", TYPE_A);
        assert!(handle_query(&pkt, &host, &a_rec, &[]).is_some());
    }

    #[test]
    fn handle_ptr_query() {
        let host = encode_dns_name("encore.local");
        let ip = [192, 168, 7, 235];
        let a_rec = build_a_record(&host, &ip);
        let svc = MdnsService {
            service_type: "_spotify-connect._tcp".into(),
            instance_name: "Invoke".into(),
            port: 48144,
            txt: vec!["VERSION=1.0".into(), "CPath=/".into()],
        };
        let sd = ServiceData::new(&svc, &host, &ip);

        let pkt = make_query("_spotify-connect._tcp.local", TYPE_PTR);
        let resp = handle_query(&pkt, &host, &a_rec, &[sd]);
        assert!(resp.is_some());
        let resp = resp.unwrap();
        let an_count = u16::from_be_bytes([resp[6], resp[7]]);
        let ar_count = u16::from_be_bytes([resp[10], resp[11]]);
        assert_eq!(an_count, 1); // PTR answer
        assert_eq!(ar_count, 3); // SRV + TXT + A additional
    }

    #[test]
    fn handle_ptr_query_with_ad_bit() {
        let host = encode_dns_name("encore.local");
        let ip = [192, 168, 7, 235];
        let a_rec = build_a_record(&host, &ip);
        let svc = MdnsService {
            service_type: "_spotify-connect._tcp".into(),
            instance_name: "Invoke".into(),
            port: 48144,
            txt: vec!["VERSION=1.0".into(), "CPath=/".into()],
        };
        let sd = ServiceData::new(&svc, &host, &ip);

        let pkt = make_query_with_ad_bit("_spotify-connect._tcp.local", TYPE_PTR);
        assert!(handle_query(&pkt, &host, &a_rec, &[sd]).is_some());
    }

    #[test]
    fn handle_srv_query() {
        let host = encode_dns_name("encore.local");
        let ip = [192, 168, 7, 235];
        let a_rec = build_a_record(&host, &ip);
        let svc = MdnsService {
            service_type: "_spotify-connect._tcp".into(),
            instance_name: "Invoke".into(),
            port: 48144,
            txt: vec!["VERSION=1.0".into(), "CPath=/".into()],
        };
        let sd = ServiceData::new(&svc, &host, &ip);

        let pkt = make_query("Invoke._spotify-connect._tcp.local", TYPE_SRV);
        let resp = handle_query(&pkt, &host, &a_rec, &[sd]);
        assert!(resp.is_some());
    }

    #[test]
    fn unmatched_query_returns_none() {
        let host = encode_dns_name("encore.local");
        let ip = [192, 168, 7, 235];
        let a_rec = build_a_record(&host, &ip);
        let pkt = make_query("other.local", TYPE_A);
        assert!(handle_query(&pkt, &host, &a_rec, &[]).is_none());
    }

    #[test]
    fn response_ignored() {
        let host = encode_dns_name("encore.local");
        let a_rec = build_a_record(&host, &[192, 168, 7, 235]);
        let mut pkt = make_query("encore.local", TYPE_A);
        pkt[2] |= 0x80; // Set QR=1 (response)
        assert!(handle_query(&pkt, &host, &a_rec, &[]).is_none());
    }

    #[test]
    fn decode_dns_name_compressed_basic() {
        // Simple uncompressed name
        let name = encode_dns_name("encore.local");
        let result = decode_dns_name_compressed(&name, 0);
        assert_eq!(result, Some("encore.local".into()));
    }

    #[test]
    fn decode_dns_name_compressed_pointer() {
        // Build a packet with a name at offset 0, then a compression pointer at offset 14
        let mut pkt = encode_dns_name("encore.local"); // 14 bytes: 6,e,n,c,o,r,e,5,l,o,c,a,l,0
                                                       // Append a label "test" followed by pointer to offset 0
        pkt.push(4);
        pkt.extend_from_slice(b"test");
        pkt.push(0xC0); // compression pointer
        pkt.push(0x00); // offset 0
                        // Decode from offset 14 (the "test" label)
        let result = decode_dns_name_compressed(&pkt, 14);
        assert_eq!(result, Some("test.encore.local".into()));
    }

    #[test]
    fn skip_dns_name_works() {
        let name = encode_dns_name("encore.local");
        let pos = skip_dns_name(&name, 0);
        assert_eq!(pos, Some(name.len())); // should be at end (after null terminator)
    }

    #[test]
    fn skip_dns_name_compressed() {
        let pkt = vec![0xC0, 0x00]; // compression pointer
        let pos = skip_dns_name(&pkt, 0);
        assert_eq!(pos, Some(2)); // 2 bytes for pointer
    }

    /// Build a minimal mDNS response with PTR+SRV+TXT+A records for group discovery.
    fn build_group_response(
        peer_id: &str,
        name: &str,
        ip: [u8; 4],
        port: u16,
        group: &str,
    ) -> Vec<u8> {
        let svc_type = encode_dns_name("_encore-group._tcp.local");
        let instance = encode_dns_name(&format!("{}.{}", name, "_encore-group._tcp.local"));
        let host = encode_dns_name("test.local");

        let ptr_rec = build_ptr_record(&svc_type, &instance);
        let srv_rec = build_srv_record(&instance, &host, port);
        let txt_strs = vec![
            format!("id={}", peer_id),
            format!("group={}", group),
            "channel=stereo".into(),
        ];
        let txt_rec = build_txt_record(&instance, &txt_strs);
        let a_rec = build_a_record(&host, &ip);

        let answers = vec![ptr_rec.as_slice()];
        let additional = vec![srv_rec.as_slice(), txt_rec.as_slice(), a_rec.as_slice()];

        // Build response header
        let an_count = answers.len() as u16;
        let ar_count = additional.len() as u16;
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&[0x00, 0x00]); // ID
        pkt.extend_from_slice(&[0x84, 0x00]); // Flags: QR=1, AA=1
        pkt.extend_from_slice(&[0x00, 0x00]); // QDCOUNT
        pkt.extend_from_slice(&an_count.to_be_bytes());
        pkt.extend_from_slice(&[0x00, 0x00]); // NSCOUNT
        pkt.extend_from_slice(&ar_count.to_be_bytes());
        for a in &answers {
            pkt.extend_from_slice(a);
        }
        for a in &additional {
            pkt.extend_from_slice(a);
        }
        pkt
    }

    #[test]
    fn parse_mdns_response_discovers_peer() {
        let pkt = build_group_response("abc-123", "Kitchen", [192, 168, 1, 50], 48200, "home");
        let result = handle_response(&pkt, "other-peer");
        assert!(result.is_some(), "Should discover a peer");
        let disc = result.unwrap();
        assert_eq!(disc.peer_id, "abc-123");
        assert_eq!(
            disc.address,
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 50))
        );
        assert_eq!(disc.port, 48200);
        assert_eq!(disc.group_name, "home");
        assert_eq!(disc.channel, "stereo");
    }

    #[test]
    fn parse_mdns_response_self_filtering() {
        let pkt = build_group_response(
            "my-peer-id",
            "Living Room",
            [192, 168, 1, 100],
            48200,
            "home",
        );
        // When our_peer_id matches, should return None
        let result = handle_response(&pkt, "my-peer-id");
        assert!(result.is_none(), "Should filter out our own announcements");
    }

    #[test]
    fn parse_mdns_response_non_group_ignored() {
        // Build a response for a non-group service
        let svc_type = encode_dns_name("_spotify-connect._tcp.local");
        let instance = encode_dns_name("Invoke._spotify-connect._tcp.local");
        let host = encode_dns_name("encore.local");
        let ip = [192, 168, 7, 235];

        let ptr_rec = build_ptr_record(&svc_type, &instance);
        let a_rec = build_a_record(&host, &ip);

        let mut pkt = Vec::new();
        pkt.extend_from_slice(&[0x00, 0x00]); // ID
        pkt.extend_from_slice(&[0x84, 0x00]); // QR=1, AA=1
        pkt.extend_from_slice(&[0x00, 0x00]); // QDCOUNT
        pkt.extend_from_slice(&[0x00, 0x01]); // ANCOUNT = 1
        pkt.extend_from_slice(&[0x00, 0x00]); // NSCOUNT
        pkt.extend_from_slice(&[0x00, 0x01]); // ARCOUNT = 1
        pkt.extend_from_slice(&ptr_rec);
        pkt.extend_from_slice(&a_rec);

        let result = handle_response(&pkt, "other");
        assert!(
            result.is_none(),
            "Non-group services should not produce discovery"
        );
    }

    #[test]
    fn build_ptr_query_valid() {
        let query = build_ptr_query("_encore-group._tcp.local");
        assert!(query.len() >= 12);
        // QR bit should be 0 (query)
        assert_eq!(query[2] & 0x80, 0);
        // QDCOUNT should be 1
        assert_eq!(u16::from_be_bytes([query[4], query[5]]), 1);
    }
}
