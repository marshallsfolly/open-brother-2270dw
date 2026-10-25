//! Minimal ASN.1 BER (Basic Encoding Rules) codec — the subset used by SNMP.
//!
//! Implements just enough of X.690 to encode and decode SNMP v1/v2c messages:
//! INTEGER, OCTET STRING, NULL, OID, IpAddress, Counter32, Gauge32, TimeTicks,
//! Counter64, and constructed values (SEQUENCE and the application-defined
//! PDU tags `0xa0`–`0xa5`).

use crate::error::{Error, Result};

/// A decoded (or to-be-encoded) BER value, carrying its tag byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ber {
    pub tag: u8,
    pub content: BerContent,
}

/// The payload of a [`Ber`] value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BerContent {
    Boolean(bool),
    Integer(i64),
    OctetString(Vec<u8>),
    Null,
    Oid(Vec<u32>),
    IpAddress([u8; 4]),
    Counter32(u32),
    Gauge32(u32),
    TimeTicks(u32),
    Counter64(u64),
    Opaque(Vec<u8>),
    /// A constructed value: SEQUENCE (`0x30`) or an application/context tag.
    Sequence(Vec<Ber>),
    /// A primitive value of an unsupported tag, holding raw content bytes.
    Raw(Vec<u8>),
}

// Tag constants.
const TAG_BOOLEAN: u8 = 0x01;
const TAG_INTEGER: u8 = 0x02;
const TAG_OCTET_STRING: u8 = 0x04;
const TAG_NULL: u8 = 0x05;
const TAG_OID: u8 = 0x06;
const TAG_SEQUENCE: u8 = 0x30;
const TAG_IP_ADDRESS: u8 = 0x40;
const TAG_COUNTER32: u8 = 0x41;
const TAG_GAUGE32: u8 = 0x42;
const TAG_TIME_TICKS: u8 = 0x43;
const TAG_OPAQUE: u8 = 0x44;
const TAG_COUNTER64: u8 = 0x46;

impl Ber {
    pub fn boolean(v: bool) -> Self {
        Ber { tag: TAG_BOOLEAN, content: BerContent::Boolean(v) }
    }

    pub fn integer(v: i64) -> Self {
        Ber { tag: TAG_INTEGER, content: BerContent::Integer(v) }
    }

    pub fn octet_string(v: impl Into<Vec<u8>>) -> Self {
        Ber { tag: TAG_OCTET_STRING, content: BerContent::OctetString(v.into()) }
    }

    pub fn null() -> Self {
        Ber { tag: TAG_NULL, content: BerContent::Null }
    }

    pub fn oid(arcs: &[u32]) -> Self {
        Ber { tag: TAG_OID, content: BerContent::Oid(arcs.to_vec()) }
    }

    pub fn ip_address(v: [u8; 4]) -> Self {
        Ber { tag: TAG_IP_ADDRESS, content: BerContent::IpAddress(v) }
    }

    pub fn counter32(v: u32) -> Self {
        Ber { tag: TAG_COUNTER32, content: BerContent::Counter32(v) }
    }

    pub fn gauge32(v: u32) -> Self {
        Ber { tag: TAG_GAUGE32, content: BerContent::Gauge32(v) }
    }

    pub fn time_ticks(v: u32) -> Self {
        Ber { tag: TAG_TIME_TICKS, content: BerContent::TimeTicks(v) }
    }

    pub fn counter64(v: u64) -> Self {
        Ber { tag: TAG_COUNTER64, content: BerContent::Counter64(v) }
    }

    pub fn opaque(v: impl Into<Vec<u8>>) -> Self {
        Ber { tag: TAG_OPAQUE, content: BerContent::Opaque(v.into()) }
    }

    /// A SEQUENCE (`0x30`).
    pub fn sequence(items: Vec<Ber>) -> Self {
        Ber { tag: TAG_SEQUENCE, content: BerContent::Sequence(items) }
    }

    /// A constructed value with an explicit tag (e.g. `0xa0` for GetRequest).
    pub fn constructed(tag: u8, items: Vec<Ber>) -> Self {
        Ber { tag, content: BerContent::Sequence(items) }
    }

    pub fn is_constructed(&self) -> bool {
        self.tag & 0x20 != 0
    }
}

/// Decode a single BER value, returning it together with the number of bytes
/// consumed from the front of `data`.
pub fn decode(data: &[u8]) -> Result<(Ber, usize)> {
    decode_at(data, 0)
}

fn decode_at(data: &[u8], mut pos: usize) -> Result<(Ber, usize)> {
    let tag = *data.get(pos).ok_or_else(|| Error::Ber("truncated tag".into()))?;
    pos += 1;
    if tag & 0x1f == 0x1f {
        return Err(Error::Ber("long-form (multi-byte) tags are not supported".into()));
    }

    let first = *data.get(pos).ok_or_else(|| Error::Ber("truncated length".into()))?;
    pos += 1;
    let len = if first & 0x80 == 0 {
        first as usize
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 {
            return Err(Error::Ber("indefinite length is not supported".into()));
        }
        if n > 8 {
            return Err(Error::Ber("length field too large".into()));
        }
        let mut l: usize = 0;
        for _ in 0..n {
            let b = *data.get(pos).ok_or_else(|| Error::Ber("truncated length".into()))?;
            pos += 1;
            l = (l << 8) | b as usize;
        }
        l
    };

    let end = pos.checked_add(len).ok_or_else(|| Error::Ber("length overflow".into()))?;
    if end > data.len() {
        return Err(Error::Ber(format!(
            "truncated content: need {end} bytes, have {}",
            data.len()
        )));
    }
    let content_bytes = &data[pos..end];

    let content = if tag & 0x20 != 0 {
        // Constructed: recursively decode child values.
        let mut items = Vec::new();
        let mut q = 0;
        while q < content_bytes.len() {
            let (item, nq) = decode_at(content_bytes, q)?;
            q = nq;
            items.push(item);
        }
        BerContent::Sequence(items)
    } else {
        match tag {
            TAG_BOOLEAN => BerContent::Boolean(!content_bytes.is_empty() && content_bytes[0] != 0),
            TAG_INTEGER => BerContent::Integer(decode_signed(content_bytes)?),
            TAG_OCTET_STRING => BerContent::OctetString(content_bytes.to_vec()),
            TAG_NULL => BerContent::Null,
            TAG_OID => BerContent::Oid(decode_oid(content_bytes)?),
            TAG_IP_ADDRESS => {
                if content_bytes.len() != 4 {
                    return Err(Error::Ber("IpAddress must be 4 bytes".into()));
                }
                BerContent::IpAddress([content_bytes[0], content_bytes[1], content_bytes[2], content_bytes[3]])
            }
            TAG_COUNTER32 => BerContent::Counter32(decode_unsigned(content_bytes)? as u32),
            TAG_GAUGE32 => BerContent::Gauge32(decode_unsigned(content_bytes)? as u32),
            TAG_TIME_TICKS => BerContent::TimeTicks(decode_unsigned(content_bytes)? as u32),
            TAG_COUNTER64 => BerContent::Counter64(decode_unsigned(content_bytes)?),
            TAG_OPAQUE => BerContent::Opaque(content_bytes.to_vec()),
            _ => BerContent::Raw(content_bytes.to_vec()),
        }
    };

    Ok((Ber { tag, content }, end))
}

/// Encode a BER value into its byte representation.
pub fn encode(ber: &Ber) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(ber, &mut out);
    out
}

fn encode_into(ber: &Ber, out: &mut Vec<u8>) {
    let content = content_bytes(ber);
    out.push(ber.tag);
    encode_length(content.len(), out);
    out.extend_from_slice(&content);
}

fn content_bytes(ber: &Ber) -> Vec<u8> {
    match &ber.content {
        BerContent::Boolean(v) => vec![if *v { 0xff } else { 0x00 }],
        BerContent::Integer(v) => encode_signed(*v),
        BerContent::OctetString(b) => b.clone(),
        BerContent::Null => Vec::new(),
        BerContent::Oid(arcs) => encode_oid(arcs),
        BerContent::IpAddress(b) => b.to_vec(),
        BerContent::Counter32(v) | BerContent::Gauge32(v) | BerContent::TimeTicks(v) => {
            encode_unsigned(*v as u64)
        }
        BerContent::Counter64(v) => encode_unsigned(*v),
        BerContent::Opaque(b) => b.clone(),
        BerContent::Sequence(items) => {
            let mut out = Vec::new();
            for item in items {
                encode_into(item, &mut out);
            }
            out
        }
        BerContent::Raw(b) => b.clone(),
    }
}

fn encode_length(len: usize, out: &mut Vec<u8>) {
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let mut i = 0;
        while i < 7 && bytes[i] == 0 {
            i += 1;
        }
        let n = (8 - i) as u8;
        out.push(0x80 | n);
        out.extend_from_slice(&bytes[i..]);
    }
}

fn encode_unsigned(v: u64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let mut i = 0;
    while i < 7 && bytes[i] == 0 {
        i += 1;
    }
    bytes[i..].to_vec()
}

fn encode_signed(v: i64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let mut i = 0;
    if v >= 0 {
        while i < 7 && bytes[i] == 0 && (bytes[i + 1] & 0x80 == 0) {
            i += 1;
        }
    } else {
        while i < 7 && bytes[i] == 0xff && (bytes[i + 1] & 0x80 != 0) {
            i += 1;
        }
    }
    bytes[i..].to_vec()
}

fn decode_signed(bytes: &[u8]) -> Result<i64> {
    if bytes.len() > 8 {
        return Err(Error::Ber("integer does not fit in i64".into()));
    }
    let mut v: i64 = if bytes.first().map(|b| b & 0x80 != 0).unwrap_or(false) {
        -1
    } else {
        0
    };
    for &b in bytes {
        v = (v << 8) | b as i64;
    }
    Ok(v)
}

fn decode_unsigned(bytes: &[u8]) -> Result<u64> {
    if bytes.len() > 8 {
        return Err(Error::Ber("integer does not fit in u64".into()));
    }
    let mut v: u64 = 0;
    for &b in bytes {
        v = (v << 8) | b as u64;
    }
    Ok(v)
}

fn encode_oid(arcs: &[u32]) -> Vec<u8> {
    let mut out = Vec::new();
    if arcs.is_empty() {
        return out;
    }
    if arcs.len() == 1 {
        push_base128(&mut out, arcs[0]);
        return out;
    }
    // The first two arcs are packed into a single subidentifier.
    push_base128(&mut out, arcs[0] * 40 + arcs[1]);
    for &a in &arcs[2..] {
        push_base128(&mut out, a);
    }
    out
}

fn decode_oid(bytes: &[u8]) -> Result<Vec<u32>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut arcs = Vec::new();
    let (first, mut i) = read_base128(bytes, 0)?;
    if first < 40 {
        arcs.push(0);
        arcs.push(first);
    } else if first < 80 {
        arcs.push(1);
        arcs.push(first - 40);
    } else {
        arcs.push(2);
        arcs.push(first - 80);
    }
    while i < bytes.len() {
        let (arc, n) = read_base128(bytes, i)?;
        arcs.push(arc);
        i = n;
    }
    Ok(arcs)
}

fn read_base128(bytes: &[u8], mut i: usize) -> Result<(u32, usize)> {
    let mut v: u64 = 0;
    let mut count = 0;
    loop {
        let b = *bytes.get(i).ok_or_else(|| Error::Ber("truncated base-128 value".into()))?;
        i += 1;
        count += 1;
        if count > 5 {
            return Err(Error::Ber("base-128 value exceeds u32".into()));
        }
        v = (v << 7) | (b & 0x7f) as u64;
        if v > u32::MAX as u64 {
            return Err(Error::Ber("base-128 value exceeds u32".into()));
        }
        if b & 0x80 == 0 {
            break;
        }
    }
    Ok((v as u32, i))
}

fn push_base128(out: &mut Vec<u8>, v: u32) {
    let mut v = v as u64;
    let mut buf = [0u8; 5];
    let mut n = 0;
    loop {
        buf[n] = (v & 0x7f) as u8;
        v >>= 7;
        n += 1;
        if v == 0 {
            break;
        }
    }
    // buf[0..n] holds base-128 digits, least significant first; emit
    // most-significant first with the continuation bit on all but the last.
    for i in (0..n).rev() {
        let mut b = buf[i];
        if i != 0 {
            b |= 0x80;
        }
        out.push(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(v: &Ber) {
        let enc = encode(v);
        let (dec, n) = decode(&enc).expect("decode");
        assert_eq!(n, enc.len(), "consumed all bytes for {v:?}");
        assert_eq!(dec, *v);
    }

    #[test]
    fn roundtrip_primitives() {
        roundtrip(&Ber::integer(0));
        roundtrip(&Ber::integer(1));
        roundtrip(&Ber::integer(127));
        roundtrip(&Ber::integer(128));
        roundtrip(&Ber::integer(-1));
        roundtrip(&Ber::integer(-128));
        roundtrip(&Ber::integer(-129));
        roundtrip(&Ber::integer(65_535));
        roundtrip(&Ber::integer(-65_536));
        roundtrip(&Ber::octet_string(b"public"));
        roundtrip(&Ber::octet_string(Vec::<u8>::new()));
        roundtrip(&Ber::null());
        roundtrip(&Ber::oid(&[1, 3, 6, 1, 2, 1, 43]));
        roundtrip(&Ber::oid(&[0, 0]));
        roundtrip(&Ber::oid(&[2, 100, 3]));
        roundtrip(&Ber::ip_address([192, 168, 1, 20]));
        roundtrip(&Ber::counter32(u32::MAX));
        roundtrip(&Ber::gauge32(0));
        roundtrip(&Ber::time_ticks(0x1234));
        roundtrip(&Ber::counter64(u64::MAX));
        roundtrip(&Ber::boolean(true));
        roundtrip(&Ber::boolean(false));
    }

    #[test]
    fn roundtrip_sequence() {
        let msg = Ber::sequence(vec![
            Ber::integer(0),
            Ber::octet_string("public"),
            Ber::constructed(0xa0, vec![
                Ber::integer(42),
                Ber::integer(0),
                Ber::integer(0),
                Ber::sequence(vec![
                    Ber::sequence(vec![Ber::oid(&[1, 3, 6, 1, 2, 1, 1, 1, 0]), Ber::null()]),
                ]),
            ]),
        ]);
        roundtrip(&msg);
    }

    #[test]
    fn base128_known_values() {
        let mut b = Vec::new();
        push_base128(&mut b, 0);
        assert_eq!(b, vec![0x00]);
        b.clear();
        push_base128(&mut b, 127);
        assert_eq!(b, vec![0x7f]);
        b.clear();
        push_base128(&mut b, 128);
        assert_eq!(b, vec![0x81, 0x00]);
        b.clear();
        push_base128(&mut b, 16_383);
        assert_eq!(b, vec![0xff, 0x7f]);
    }

    #[test]
    fn oid_known_encoding() {
        // 1.3.6.1.2.1.43 -> 0x2b 0x06 0x01 0x02 0x01 0x2b
        let enc = encode(&Ber::oid(&[1, 3, 6, 1, 2, 1, 43]));
        assert_eq!(enc, vec![0x06, 0x06, 0x2b, 0x06, 0x01, 0x02, 0x01, 0x2b]);
    }

    #[test]
    fn length_encoding() {
        let mut out = Vec::new();
        encode_length(127, &mut out);
        assert_eq!(out, vec![0x7f]);
        out.clear();
        encode_length(128, &mut out);
        assert_eq!(out, vec![0x81, 0x80]);
        out.clear();
        encode_length(300, &mut out);
        assert_eq!(out, vec![0x82, 0x01, 0x2c]);
    }

    #[test]
    fn reject_truncated() {
        assert!(decode(&[0x30, 0x05, 0x02]).is_err());
        assert!(decode(&[0x30]).is_err());
    }
}
