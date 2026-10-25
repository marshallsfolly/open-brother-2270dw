//! A small SNMP v1/v2c client — enough to read the Printer MIB (RFC 1759/3805)
//! and the Host Resources MIB, and to perform broadcast discovery.
//!
//! Only `GET`, `GETNEXT`, `GETBULK`, `WALK` and `SET` are implemented, over
//! UDP. No encryption (SNMPv3) is supported; Brother printers ship with
//! community string `public` by default.

use std::fmt;
use std::net::{SocketAddr, UdpSocket};
use std::str::FromStr;
use std::time::{Duration, Instant};

use crate::ber::{self, Ber, BerContent};
use crate::error::{Error, Result};

/// Default SNMP community string used by Brother printers.
pub const DEFAULT_COMMUNITY: &str = "public";
/// Well-known SNMP agent port.
pub const SNMP_PORT: u16 = 161;

/// An object identifier: a sequence of integer arcs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Oid(pub Vec<u32>);

impl Oid {
    pub fn new(arcs: Vec<u32>) -> Self {
        Oid(arcs)
    }

    /// Parse a dotted OID string such as `1.3.6.1.2.1.1.1.0`.
    pub fn parse(s: &str) -> Result<Self> {
        let mut arcs = Vec::new();
        for part in s.split('.') {
            if part.is_empty() {
                return Err(Error::InvalidArgument(format!("bad OID: {s}")));
            }
            let arc: u32 = part
                .parse()
                .map_err(|_| Error::InvalidArgument(format!("bad OID arc in {s}: {part}")))?;
            arcs.push(arc);
        }
        if arcs.is_empty() {
            return Err(Error::InvalidArgument(format!("empty OID: {s}")));
        }
        Ok(Oid(arcs))
    }

    pub fn as_slice(&self) -> &[u32] {
        &self.0
    }

    pub fn is_prefix_of(&self, other: &Oid) -> bool {
        self.0.len() <= other.0.len() && other.0.starts_with(&self.0)
    }

    pub fn to_ber(&self) -> Ber {
        Ber::oid(&self.0)
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return Ok(());
        }
        write!(f, "{}", self.0[0])?;
        for a in &self.0[1..] {
            write!(f, ".{a}")?;
        }
        Ok(())
    }
}

impl FromStr for Oid {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Oid::parse(s)
    }
}

impl From<&[u32]> for Oid {
    fn from(v: &[u32]) -> Self {
        Oid(v.to_vec())
    }
}

impl From<Vec<u32>> for Oid {
    fn from(v: Vec<u32>) -> Self {
        Oid(v)
    }
}

/// A single SNMP variable binding: an OID paired with its value.
#[derive(Clone, Debug, PartialEq)]
pub struct Varbind {
    pub oid: Oid,
    pub value: Ber,
}

/// A decoded SNMP response.
#[derive(Debug)]
struct Response {
    request_id: i64,
    varbinds: Vec<Varbind>,
}

/// Convenience methods to read typed values out of a decoded [`Ber`].
impl Ber {
    /// Signed integer value (only for the INTEGER type).
    pub fn as_i64(&self) -> Option<i64> {
        match &self.content {
            BerContent::Integer(v) => Some(*v),
            _ => None,
        }
    }

    /// Unsigned integer value (INTEGER >= 0, Counter32, Gauge32, TimeTicks,
    /// Counter64).
    pub fn as_u64(&self) -> Option<u64> {
        match &self.content {
            BerContent::Integer(v) if *v >= 0 => Some(*v as u64),
            BerContent::Counter32(v)
            | BerContent::Gauge32(v)
            | BerContent::TimeTicks(v) => Some(*v as u64),
            BerContent::Counter64(v) => Some(*v),
            _ => None,
        }
    }

    /// UTF-8 string value (for OCTET STRING).
    pub fn as_str(&self) -> Option<&str> {
        match &self.content {
            BerContent::OctetString(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    /// Raw octet-string bytes.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match &self.content {
            BerContent::OctetString(b) | BerContent::Opaque(b) | BerContent::Raw(b) => Some(b),
            _ => None,
        }
    }

    /// OID value.
    pub fn as_oid(&self) -> Option<&[u32]> {
        match &self.content {
            BerContent::Oid(a) => Some(a),
            _ => None,
        }
    }
}

/// An SNMP agent client bound to a community string.
#[derive(Debug, Clone)]
pub struct SnmpClient {
    community: Vec<u8>,
    timeout: Duration,
    retries: u32,
}

impl Default for SnmpClient {
    fn default() -> Self {
        Self::new(DEFAULT_COMMUNITY)
    }
}

impl SnmpClient {
    pub fn new(community: &str) -> Self {
        SnmpClient {
            community: community.as_bytes().to_vec(),
            timeout: Duration::from_secs(2),
            retries: 2,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// GET one or more OIDs from `addr`.
    pub fn get(&self, addr: SocketAddr, oids: &[Oid]) -> Result<Vec<Varbind>> {
        let varbinds: Vec<(Oid, Ber)> = oids
            .iter()
            .map(|o| (o.clone(), Ber::null()))
            .collect();
        let (request_id, pdu) = build_get_pdu(0xa0, &varbinds);
        let resp = self.exchange(addr, request_id, pdu)?;
        Ok(resp.varbinds)
    }

    /// GETNEXT a single OID from `addr`.
    pub fn get_next(&self, addr: SocketAddr, oid: &Oid) -> Result<Option<Varbind>> {
        let varbinds = vec![(oid.clone(), Ber::null())];
        let (request_id, pdu) = build_get_pdu(0xa1, &varbinds);
        let resp = self.exchange(addr, request_id, pdu)?;
        Ok(resp.varbinds.into_iter().next())
    }

    /// SET a single OID on `addr`.
    pub fn set(&self, addr: SocketAddr, oid: &Oid, value: Ber) -> Result<()> {
        let varbinds = vec![(oid.clone(), value)];
        let (request_id, pdu) = build_get_pdu(0xa3, &varbinds);
        self.exchange(addr, request_id, pdu)?;
        Ok(())
    }

    /// Walk a MIB subtree rooted at `base`, using an iterative GETNEXT loop
    /// (works for both SNMPv1 and v2c agents).
    pub fn walk(&self, addr: SocketAddr, base: &Oid) -> Result<Vec<Varbind>> {
        let mut out = Vec::new();
        let mut next = base.clone();
        while let Some(vb) = self.get_next(addr, &next)? {
            if !base.is_prefix_of(&vb.oid) {
                break;
            }
            if vb.oid == next {
                // Agent did not advance (defensive: avoid infinite loop).
                out.push(vb);
                break;
            }
            next = vb.oid.clone();
            out.push(vb);
        }
        Ok(out)
    }

    /// Walk a MIB subtree using v2c GETBULK — a single round-trip fetches up
    /// to `max_repetitions` consecutive OIDs at a time.
    pub fn walk_bulk(&self, addr: SocketAddr, base: &Oid, max_repetitions: u32) -> Result<Vec<Varbind>> {
        let mut out = Vec::new();
        let mut next = base.clone();
        let max = max_repetitions.max(1);
        loop {
            let (request_id, pdu) = build_get_bulk_pdu(&next, max);
            let resp = self.exchange(addr, request_id, pdu)?;
            if resp.varbinds.is_empty() {
                break;
            }
            let count = resp.varbinds.len();
            let mut last: Option<Oid> = None;
            let mut done = false;
            for vb in resp.varbinds {
                if !base.is_prefix_of(&vb.oid) {
                    done = true;
                    break;
                }
                last = Some(vb.oid.clone());
                out.push(vb);
            }
            if done {
                break;
            }
            // Fewer results than requested means we hit the end of the MIB.
            if count < max as usize {
                break;
            }
            match last {
                Some(l) if l != next => next = l,
                _ => break,
            }
        }
        Ok(out)
    }

    /// Send a GET to the IPv4 limited-broadcast address and collect every
    /// response, returning `(source address, varbinds)` pairs. Used for LAN
    /// discovery when mDNS is unavailable.
    pub fn broadcast_get(&self, oids: &[Oid]) -> Result<Vec<(SocketAddr, Vec<Varbind>)>> {
        let varbinds: Vec<(Oid, Ber)> = oids
            .iter()
            .map(|o| (o.clone(), Ber::null()))
            .collect();
        let (request_id, pdu) = build_get_pdu(0xa0, &varbinds);
        let message = Ber::sequence(vec![
            Ber::integer(1),
            Ber::octet_string(self.community.clone()),
            pdu,
        ]);
        let request = ber::encode(&message);

        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_broadcast(true)?;
        let dest = SocketAddr::from(([255, 255, 255, 255], SNMP_PORT));
        socket.send_to(&request, dest)?;

        let deadline = Instant::now() + self.timeout;
        let mut buf = vec![0u8; 65_535];
        let mut out = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            socket.set_read_timeout(Some(remaining))?;
            match socket.recv_from(&mut buf) {
                Ok((n, src)) => {
                    if let Ok(resp) = parse_response(&buf[..n]) {
                        if resp.request_id == request_id {
                            out.push((src, resp.varbinds));
                        }
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(out)
    }

    /// Send a request and wait for the matching response.
    fn exchange(&self, addr: SocketAddr, request_id: i64, pdu: Ber) -> Result<Response> {        let message = Ber::sequence(vec![
            Ber::integer(1), // SNMP version 2c
            Ber::octet_string(self.community.clone()),
            pdu,
        ]);
        let request = ber::encode(&message);

        let bind = if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
        let socket = UdpSocket::bind(bind)?;
        socket.set_read_timeout(Some(self.timeout))?;
        socket.connect(addr)?;

        let mut buf = vec![0u8; 65_535];
        let mut last_err = Error::Timeout("no response".into());
        for _ in 0..=self.retries {
            socket.send(&request)?;
            loop {
                match socket.recv(&mut buf) {
                    Ok(n) => {
                        let resp = parse_response(&buf[..n])?;
                        if resp.request_id == request_id {
                            return Ok(resp);
                        }
                        // Stale datagram (shouldn't happen on a fresh socket);
                        // keep waiting for our response.
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        last_err = Error::Timeout(format!("{addr}"));
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Err(last_err)
    }
}

fn build_get_pdu(tag: u8, varbinds: &[(Oid, Ber)]) -> (i64, Ber) {
    let request_id = next_request_id();
    let pdu = Ber::constructed(
        tag,
        vec![
            Ber::integer(request_id),
            Ber::integer(0), // error-status
            Ber::integer(0), // error-index
            Ber::sequence(
                varbinds
                    .iter()
                    .map(|(o, v)| Ber::sequence(vec![o.to_ber(), v.clone()]))
                    .collect(),
            ),
        ],
    );
    (request_id, pdu)
}

fn build_get_bulk_pdu(base: &Oid, max_repetitions: u32) -> (i64, Ber) {
    let request_id = next_request_id();
    let pdu = Ber::constructed(
        0xa5,
        vec![
            Ber::integer(request_id),
            Ber::integer(0), // non-repeaters
            Ber::integer(max_repetitions as i64),
            Ber::sequence(vec![Ber::sequence(vec![base.to_ber(), Ber::null()])]),
        ],
    );
    (request_id, pdu)
}

fn parse_response(data: &[u8]) -> Result<Response> {
    let (msg, _) = ber::decode(data)?;
    let items = match &msg.content {
        BerContent::Sequence(items) => items,
        _ => return Err(Error::Snmp("message is not a SEQUENCE".into())),
    };
    if items.len() < 3 {
        return Err(Error::Snmp("message too short".into()));
    }
    let version = match &items[0].content {
        BerContent::Integer(v) => *v,
        _ => return Err(Error::Snmp("missing version".into())),
    };
    let pdu = &items[2];
    let pdu_items = match &pdu.content {
        BerContent::Sequence(items) => items,
        _ => return Err(Error::Snmp("PDU is not constructed".into())),
    };
    if pdu_items.len() < 4 {
        return Err(Error::Snmp("PDU too short".into()));
    }
    let request_id = pdu_items[0]
        .as_i64()
        .ok_or_else(|| Error::Snmp("bad request-id".into()))?;
    let error_status = pdu_items[1]
        .as_i64()
        .ok_or_else(|| Error::Snmp("bad error-status".into()))?;
    let error_index = pdu_items[2]
        .as_i64()
        .ok_or_else(|| Error::Snmp("bad error-index".into()))?;

    let varbinds = match &pdu_items[3].content {
        BerContent::Sequence(items) => items
            .iter()
            .map(|vb| match &vb.content {
                BerContent::Sequence(fields) if fields.len() == 2 => {
                    let oid = fields[0]
                        .as_oid()
                        .ok_or_else(|| Error::Snmp("varbind name is not an OID".into()))?;
                    Ok(Varbind {
                        oid: Oid(oid.to_vec()),
                        value: fields[1].clone(),
                    })
                }
                _ => Err(Error::Snmp("malformed varbind".into())),
            })
            .collect::<Result<Vec<_>>>()?,
        _ => return Err(Error::Snmp("varbind list is not a SEQUENCE".into())),
    };

    if error_status != 0 {
        return Err(Error::Snmp(format!(
            "agent error-status {error_status} at varbind index {error_index} (version {version})"
        )));
    }

    Ok(Response { request_id, varbinds })
}

fn next_request_id() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    static COUNTER: AtomicI64 = AtomicI64::new(0);
    // Seed once with the low bits of the clock so restarts don't always start
    // at zero (which can confuse agents that de-duplicate request ids).
    let mut id = COUNTER.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(1);
        id = seed & 0x0fff_ffff;
        COUNTER.store(id, Ordering::Relaxed);
    }
    id
}

/// Commonly used standard-MIB object identifiers.
pub mod oids {
    use super::Oid;

    pub fn sys_descr() -> Oid {
        Oid(vec![1, 3, 6, 1, 2, 1, 1, 1, 0])
    }
    pub fn sys_object_id() -> Oid {
        Oid(vec![1, 3, 6, 1, 2, 1, 1, 2, 0])
    }
    pub fn sys_up_time() -> Oid {
        Oid(vec![1, 3, 6, 1, 2, 1, 1, 3, 0])
    }
    pub fn sys_contact() -> Oid {
        Oid(vec![1, 3, 6, 1, 2, 1, 1, 4, 0])
    }
    pub fn sys_name() -> Oid {
        Oid(vec![1, 3, 6, 1, 2, 1, 1, 5, 0])
    }
    pub fn sys_location() -> Oid {
        Oid(vec![1, 3, 6, 1, 2, 1, 1, 6, 0])
    }
    /// Host Resources MIB device status for the first device
    /// (1 = unknown, 2 = running, 3 = warning, 4 = testing, 5 = down).
    pub fn hr_device_status() -> Oid {
        Oid(vec![1, 3, 6, 1, 2, 1, 25, 3, 2, 1, 5, 1])
    }
    /// Host Resources MIB device description (the human-readable model name).
    pub fn hr_device_descr() -> Oid {
        Oid(vec![1, 3, 6, 1, 2, 1, 25, 3, 2, 1, 3, 1])
    }
    /// Printer MIB root.
    pub fn printer_mib() -> Oid {
        Oid(vec![1, 3, 6, 1, 2, 1, 43])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oid_parse_and_display() {
        let oid = Oid::parse("1.3.6.1.2.1.43.11.1.1.9.1.1").unwrap();
        assert_eq!(oid.to_string(), "1.3.6.1.2.1.43.11.1.1.9.1.1");
        assert_eq!(oid.as_slice(), &[1, 3, 6, 1, 2, 1, 43, 11, 1, 1, 9, 1, 1]);
    }

    #[test]
    fn oid_parse_rejects_junk() {
        assert!(Oid::parse("1.3.x").is_err());
        assert!(Oid::parse("").is_err());
    }

    #[test]
    fn prefix_check() {
        let base = Oid::parse("1.3.6.1.2.1.43").unwrap();
        let under = Oid::parse("1.3.6.1.2.1.43.11.1.1.9.1.1").unwrap();
        let outside = Oid::parse("1.3.6.1.2.1.44").unwrap();
        assert!(base.is_prefix_of(&under));
        assert!(!base.is_prefix_of(&outside));
    }

    #[test]
    fn value_accessors() {
        assert_eq!(Ber::integer(42).as_i64(), Some(42));
        assert_eq!(Ber::integer(-1).as_u64(), None);
        assert_eq!(Ber::counter32(7).as_u64(), Some(7));
        assert_eq!(Ber::octet_string(b"abc").as_str(), Some("abc"));
        assert_eq!(Ber::octet_string([0xff, 0xfe]).as_str(), None);
        assert_eq!(Ber::oid(&[1, 3, 6]).as_oid(), Some(&[1, 3, 6][..]));
    }

    #[test]
    fn parse_response_rejects_bad_status() {
        // A well-formed response with error-status = 5 (genErr).
        let msg = Ber::sequence(vec![
            Ber::integer(1),
            Ber::octet_string(b"public"),
            Ber::constructed(
                0xa2,
                vec![
                    Ber::integer(1),
                    Ber::integer(5),
                    Ber::integer(1),
                    Ber::sequence(vec![Ber::sequence(vec![
                        Ber::oid(&[1, 3, 6, 1, 2, 1, 1, 1, 0]),
                        Ber::null(),
                    ])]),
                ],
            ),
        ]);
        let enc = ber::encode(&msg);
        let err = parse_response(&enc).unwrap_err();
        assert!(err.to_string().contains("error-status 5"));
    }
}
