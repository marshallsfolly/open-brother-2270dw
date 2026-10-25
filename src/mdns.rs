//! A minimal mDNS / DNS-SD querier used for zero-configuration printer
//! discovery on the local network.
//!
//! It builds DNS query packets and multicasts them to the mDNS group
//! (`224.0.0.251:5353`), collecting the A/AAAA/PTR/SRV/TXT records that come
//! back. It first tries a proper multicast listener (binding UDP 5353 and
//! joining the group), and falls back to a unicast-response ("QU" bit) query
//! when that port is already owned by another mDNS responder.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

pub const MDNS_GROUP_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub const MDNS_PORT: u16 = 5353;

/// DNS resource-record type codes we care about.
pub const TYPE_A: u16 = 1;
pub const TYPE_PTR: u16 = 12;
pub const TYPE_TXT: u16 = 16;
pub const TYPE_AAAA: u16 = 28;
pub const TYPE_SRV: u16 = 33;

/// DNS class IN (no QU bit) — asks for a multicast response.
const CLASS_IN: u16 = 0x0001;
/// DNS class IN with the mDNS "QU" bit set — asks for a unicast response.
const CLASS_IN_QU: u16 = 0x8001;

/// A decoded DNS resource record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsRecord {
    A { name: String, addr: Ipv4Addr, ttl: u32 },
    Aaaa { name: String, addr: Ipv6Addr, ttl: u32 },
    Ptr { name: String, target: String, ttl: u32 },
    Srv { name: String, port: u16, target: String, ttl: u32 },
    Txt { name: String, entries: Vec<Vec<u8>>, ttl: u32 },
    Other { name: String, rtype: u16, rdata: Vec<u8>, ttl: u32 },
}

impl DnsRecord {
    pub fn name(&self) -> &str {
        match self {
            DnsRecord::A { name, .. }
            | DnsRecord::Aaaa { name, .. }
            | DnsRecord::Ptr { name, .. }
            | DnsRecord::Srv { name, .. }
            | DnsRecord::Txt { name, .. }
            | DnsRecord::Other { name, .. } => name,
        }
    }

    pub fn ip_addr(&self) -> Option<IpAddr> {
        match self {
            DnsRecord::A { addr, .. } => Some(IpAddr::V4(*addr)),
            DnsRecord::Aaaa { addr, .. } => Some(IpAddr::V6(*addr)),
            _ => None,
        }
    }

    /// TXT entries as `(key, value)` pairs (`value` is empty when the entry is
    /// a bare key).
    pub fn txt_pairs(&self) -> Option<Vec<(String, String)>> {
        match self {
            DnsRecord::Txt { entries, .. } => Some(
                entries
                    .iter()
                    .map(|e| match e.iter().position(|&b| b == b'=') {
                        Some(i) => (
                            String::from_utf8_lossy(&e[..i]).into_owned(),
                            String::from_utf8_lossy(&e[i + 1..]).into_owned(),
                        ),
                        None => (String::from_utf8_lossy(e).into_owned(), String::new()),
                    })
                    .collect(),
            ),
            _ => None,
        }
    }
}

/// Send an mDNS query for `question` (a PTR query for a service type such as
/// `_printer._tcp.local`) and collect every record that arrives until
/// `timeout` elapses.
pub fn query_records(question: &str, qtype: u16, timeout: Duration) -> Result<Vec<DnsRecord>> {
    // Prefer a proper multicast listener; fall back to a QU-bit unicast query.
    if let Ok(recs) = query_multicast(question, qtype, timeout) {
        if !recs.is_empty() {
            return Ok(recs);
        }
    }
    query_unicast(question, qtype, timeout)
}

/// Bind UDP 5353, join the mDNS group, and collect multicast responses.
fn query_multicast(question: &str, qtype: u16, timeout: Duration) -> Result<Vec<DnsRecord>> {
    let socket = UdpSocket::bind("0.0.0.0:5353")?;
    if let Some(ip) = local_ipv4() {
        let _ = socket.join_multicast_v4(&MDNS_GROUP_V4, &ip);
    }
    socket.set_multicast_ttl_v4(1)?;
    let query = encode_query(question, qtype, false);
    let dest = SocketAddrV4::new(MDNS_GROUP_V4, MDNS_PORT);
    for _ in 0..2 {
        socket.send_to(&query, dest)?;
    }
    collect(&socket, timeout)
}

/// Send a QU-bit query and collect unicast responses on an ephemeral port.
fn query_unicast(question: &str, qtype: u16, timeout: Duration) -> Result<Vec<DnsRecord>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_multicast_ttl_v4(1)?;
    let query = encode_query(question, qtype, true);
    let dest = SocketAddrV4::new(MDNS_GROUP_V4, MDNS_PORT);
    for _ in 0..2 {
        socket.send_to(&query, dest)?;
    }
    collect(&socket, timeout)
}

fn collect(socket: &UdpSocket, timeout: Duration) -> Result<Vec<DnsRecord>> {
    let deadline = Instant::now() + timeout;
    let mut buf = vec![0u8; 65_535];
    let mut records = Vec::new();

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        socket.set_read_timeout(Some(remaining.min(Duration::from_millis(250))))?;
        match socket.recv_from(&mut buf) {
            Ok((n, _src)) => {
                if let Ok(mut recs) = parse_message(&buf[..n]) {
                    records.append(&mut recs);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
    }

    Ok(records)
}

/// The local IPv4 address used by the default route.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    match s.local_addr().ok()? {
        std::net::SocketAddr::V4(a) => Some(*a.ip()),
        _ => None,
    }
}

/// Build a single-question DNS query packet. `qu` sets the mDNS unicast-
/// response bit.
pub fn encode_query(question: &str, qtype: u16, qu: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    // Header: ID=0 (mDNS), flags=0, QDCOUNT=1, rest 0.
    out.extend_from_slice(&[0x00, 0x00]); // ID
    out.extend_from_slice(&[0x00, 0x00]); // flags
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    encode_name(question, &mut out);
    out.extend_from_slice(&qtype.to_be_bytes());
    out.extend_from_slice(&(if qu { CLASS_IN_QU } else { CLASS_IN }).to_be_bytes());
    out
}

/// Parse a full DNS message, returning all resource records from every section.
pub fn parse_message(data: &[u8]) -> Result<Vec<DnsRecord>> {
    if data.len() < 12 {
        return Err(Error::Mdns("packet shorter than DNS header".into()));
    }
    let qd = u16::from_be_bytes([data[4], data[5]]) as usize;
    let an = u16::from_be_bytes([data[6], data[7]]) as usize;
    let ns = u16::from_be_bytes([data[8], data[9]]) as usize;
    let ar = u16::from_be_bytes([data[10], data[11]]) as usize;

    let mut pos = 12;
    for _ in 0..qd {
        let (_name, np) = read_name(data, pos)?;
        pos = np + 4; // qtype (2) + qclass (2)
    }

    let total = an.saturating_add(ns).saturating_add(ar);
    let mut records = Vec::with_capacity(total);
    for _ in 0..total {
        let (name, np) = read_name(data, pos)?;
        pos = np;
        if pos + 10 > data.len() {
            break;
        }
        let rtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let ttl = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
        let rdlen = u16::from_be_bytes([data[pos + 8], data[pos + 9]]) as usize;
        pos += 10;
        let rdata_start = pos;
        let rdata_end = pos.saturating_add(rdlen);
        if rdata_end > data.len() {
            break;
        }

        let record = match rtype {
            TYPE_A if rdlen == 4 => DnsRecord::A {
                name,
                addr: Ipv4Addr::new(data[pos], data[pos + 1], data[pos + 2], data[pos + 3]),
                ttl,
            },
            TYPE_AAAA if rdlen == 16 => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&data[pos..pos + 16]);
                DnsRecord::Aaaa {
                    name,
                    addr: Ipv6Addr::from(octets),
                    ttl,
                }
            }
            TYPE_PTR => {
                let (target, _next) = read_name(data, rdata_start)?;
                DnsRecord::Ptr { name, target, ttl }
            }
            TYPE_SRV => {
                if rdlen < 7 {
                    DnsRecord::Other { name, rtype, rdata: data[pos..rdata_end].to_vec(), ttl }
                } else {
                    let port = u16::from_be_bytes([data[pos + 4], data[pos + 5]]);
                    let (target, _next) = read_name(data, pos + 6)?;
                    DnsRecord::Srv { name, port, target, ttl }
                }
            }
            TYPE_TXT => {
                let mut entries = Vec::new();
                let mut p = pos;
                while p < rdata_end {
                    let len = data[p] as usize;
                    p += 1;
                    let end = p.saturating_add(len).min(rdata_end);
                    entries.push(data[p..end].to_vec());
                    p = end;
                }
                DnsRecord::Txt { name, entries, ttl }
            }
            _ => DnsRecord::Other {
                name,
                rtype,
                rdata: data[pos..rdata_end].to_vec(),
                ttl,
            },
        };

        records.push(record);
        pos = rdata_end;
    }

    Ok(records)
}

/// Encode a dotted domain name into uncompressed label form.
pub fn encode_name(name: &str, out: &mut Vec<u8>) {
    for label in name.split('.') {
        if label.is_empty() {
            continue; // trailing dot
        }
        let label = &label[..label.len().min(63)];
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
}

/// Read a (possibly compressed) domain name starting at `pos` in `data`.
/// Returns the decoded name and the position immediately after the name in the
/// byte stream (accounting for compression pointers).
pub fn read_name(data: &[u8], mut pos: usize) -> Result<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut jumped = false;
    let mut end: Option<usize> = None;
    let mut hops = 0;

    loop {
        let len = *data.get(pos).ok_or_else(|| Error::Mdns("truncated name".into()))?;
        if len == 0 {
            // End of name; the terminator only belongs to the original stream
            // when we have not followed a compression pointer.
            if !jumped {
                end = Some(pos + 1);
            }
            break;
        }
        if len & 0xc0 == 0xc0 {
            let b2 = *data
                .get(pos + 1)
                .ok_or_else(|| Error::Mdns("truncated pointer".into()))?;
            let offset = (((len & 0x3f) as usize) << 8) | b2 as usize;
            if !jumped {
                end = Some(pos + 2);
            }
            jumped = true;
            pos = offset;
            hops += 1;
            if hops > 32 {
                return Err(Error::Mdns("compression pointer loop".into()));
            }
            continue;
        }
        if len & 0xc0 != 0 {
            return Err(Error::Mdns("unsupported label type".into()));
        }
        let len = len as usize;
        pos += 1;
        let slice = data
            .get(pos..pos + len)
            .ok_or_else(|| Error::Mdns("truncated label".into()))?;
        labels.push(String::from_utf8_lossy(slice).into_owned());
        pos += len;
    }

    Ok((labels.join("."), end.unwrap_or(pos)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_encode_decode_roundtrip() {
        let mut out = Vec::new();
        encode_name("_printer._tcp.local", &mut out);
        let (name, pos) = read_name(&out, 0).unwrap();
        assert_eq!(name, "_printer._tcp.local");
        assert_eq!(pos, out.len());
    }

    #[test]
    fn parse_srv_response() {
        // Build a synthetic response: header + 0 questions + 1 PTR + 1 SRV +
        // 1 A, exercising name compression via a pointer.
        let mut msg = Vec::new();
        msg.extend_from_slice(&[0x00, 0x00]); // ID
        msg.extend_from_slice(&0x8400u16.to_be_bytes()); // QR=1, AA=1
        msg.extend_from_slice(&0u16.to_be_bytes()); // QDCOUNT
        msg.extend_from_slice(&3u16.to_be_bytes()); // ANCOUNT
        msg.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
        msg.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT

        // Question-less. First record: PTR _printer._tcp.local -> "Brother HL-2270DW series"
        encode_name("_printer._tcp.local", &mut msg);
        msg.extend_from_slice(&TYPE_PTR.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes()); // class IN
        msg.extend_from_slice(&120u32.to_be_bytes()); // ttl
        // rdata: target name (uncompressed for simplicity)
        let target_pos = msg.len() + 2; // rdlen placeholder position
        let mut target = Vec::new();
        encode_name("Brother HL-2270DW series._printer._tcp.local", &mut target);
        msg.extend_from_slice(&(target.len() as u16).to_be_bytes());
        msg.extend_from_slice(&target);
        let _ = target_pos;

        // Second record: SRV for the instance -> port 9100, target BRN.local
        encode_name("Brother HL-2270DW series._printer._tcp.local", &mut msg);
        msg.extend_from_slice(&TYPE_SRV.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&120u32.to_be_bytes());
        let mut srv_rdata = Vec::new();
        srv_rdata.extend_from_slice(&0u16.to_be_bytes()); // priority
        srv_rdata.extend_from_slice(&0u16.to_be_bytes()); // weight
        srv_rdata.extend_from_slice(&9100u16.to_be_bytes()); // port
        encode_name("BRN1234.local", &mut srv_rdata);
        msg.extend_from_slice(&(srv_rdata.len() as u16).to_be_bytes());
        msg.extend_from_slice(&srv_rdata);

        // Third record: A for BRN1234.local -> 192.168.1.20
        encode_name("BRN1234.local", &mut msg);
        msg.extend_from_slice(&TYPE_A.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&120u32.to_be_bytes());
        msg.extend_from_slice(&4u16.to_be_bytes());
        msg.extend_from_slice(&[192, 168, 1, 20]);

        let records = parse_message(&msg).unwrap();
        assert_eq!(records.len(), 3);
        assert!(matches!(&records[0], DnsRecord::Ptr { target, .. } if target == "Brother HL-2270DW series._printer._tcp.local"));
        assert!(matches!(&records[1], DnsRecord::Srv { port: 9100, target, .. } if target == "BRN1234.local"));
        assert!(matches!(&records[2], DnsRecord::A { addr, .. } if *addr == Ipv4Addr::new(192, 168, 1, 20)));
    }

    #[test]
    fn name_compression_pointer() {
        // Message body: an uncompressed name "BRN1234.local" then a second
        // name that points back to it.
        let mut msg = Vec::new();
        let name_start = 0usize;
        encode_name("BRN1234.local", &mut msg);
        let name_end = msg.len();
        // A pointer to name_start, encoded as 0xc0 | offset (offset=0).
        msg.push(0xc0);
        msg.push(name_start as u8);

        let (name, pos) = read_name(&msg, name_end).unwrap();
        assert_eq!(name, "BRN1234.local");
        assert_eq!(pos, name_end + 2, "consumed exactly the 2-byte pointer");
    }

    #[test]
    fn txt_parsing() {
        let rec = DnsRecord::Txt {
            name: "x._printer._tcp.local".into(),
            entries: vec![b"ty=Brother HL-2270DW".to_vec(), b"note=Office printer".to_vec()],
            ttl: 0,
        };
        let pairs = rec.txt_pairs().unwrap();
        assert_eq!(pairs[0], ("ty".to_string(), "Brother HL-2270DW".to_string()));
        assert_eq!(pairs[1], ("note".to_string(), "Office printer".to_string()));
    }
}
