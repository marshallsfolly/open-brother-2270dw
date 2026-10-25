//! Zero-configuration LAN discovery of the printer.
//!
//! Combines mDNS/DNS-SD (primary) with an SNMP broadcast sweep (fallback), and
//! merges the two into a de-duplicated list of candidates.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use serde::Serialize;

use crate::error::Result;
use crate::mdns::{self, DnsRecord};
use crate::raw::RAW_PORT;
use crate::snmp::{oids, SnmpClient, Varbind};

/// A printer candidate found on the network.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredPrinter {
    pub ip: IpAddr,
    pub hostname: Option<String>,
    pub model: Option<String>,
    pub note: Option<String>,
    /// Print-service port when known (e.g. 9100, 515, or 631).
    pub port: Option<u16>,
    pub is_brother: bool,
}

const MDNS_SERVICES: [&str; 3] = [
    "_pdl-datastream._tcp.local",
    "_printer._tcp.local",
    "_ipp._tcp.local",
];

/// Discover printers on the local network within `timeout`.
pub fn discover(timeout: Duration, community: &str) -> Vec<DiscoveredPrinter> {
    let mut merged: HashMap<IpAddr, DiscoveredPrinter> = HashMap::new();

    if let Ok(printers) = discover_mdns(timeout) {
        for p in printers {
            merge_into(&mut merged, p);
        }
    }
    if let Ok(printers) = discover_snmp_broadcast(timeout, community) {
        for p in printers {
            merge_into(&mut merged, p);
        }
    }

    // Fallback: scan the local /24 for the raw print port, then identify any
    // responder over unicast SNMP. This finds printers that ignore broadcast
    // and mDNS.
    for ip in scan_local_subnet(timeout) {
        merge_into(&mut merged, identify_host(ip, timeout, community));
    }

    let mut out: Vec<DiscoveredPrinter> = merged.into_values().collect();
    out.sort_by_key(|p| match p.ip {
        IpAddr::V4(a) => u128::from(u32::from_be_bytes(a.octets())),
        IpAddr::V6(a) => u128::from(a),
    });
    out
}

fn merge_into(map: &mut HashMap<IpAddr, DiscoveredPrinter>, p: DiscoveredPrinter) {
    map.entry(p.ip)
        .and_modify(|existing| {
            if existing.hostname.is_none() {
                existing.hostname = p.hostname.clone();
            }
            if existing.model.is_none() {
                existing.model = p.model.clone();
            }
            if existing.note.is_none() {
                existing.note = p.note.clone();
            }
            if existing.port.is_none() {
                existing.port = p.port;
            }
            existing.is_brother |= p.is_brother;
        })
        .or_insert(p);
}

fn discover_mdns(timeout: Duration) -> Result<Vec<DiscoveredPrinter>> {
    let mut records: Vec<DnsRecord> = Vec::new();
    for service in MDNS_SERVICES {
        if let Ok(mut recs) = mdns::query_records(service, mdns::TYPE_PTR, timeout) {
            records.append(&mut recs);
        }
    }

    let mut a: HashMap<String, Vec<IpAddr>> = HashMap::new();
    let mut srv: HashMap<String, (u16, String)> = HashMap::new();
    let mut txt: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut instances: Vec<String> = Vec::new();

    for rec in &records {
        match rec {
            DnsRecord::A { name, addr, .. } => a.entry(norm(name)).or_default().push(IpAddr::V4(*addr)),
            DnsRecord::Aaaa { name, addr, .. } => a.entry(norm(name)).or_default().push(IpAddr::V6(*addr)),
            DnsRecord::Srv { name, port, target, .. } => {
                srv.insert(norm(name), (*port, norm(target)));
            }
            DnsRecord::Txt { name, .. } => {
                txt.insert(norm(name), rec.txt_pairs().unwrap_or_default());
            }
            DnsRecord::Ptr { target, .. } => instances.push(norm(target)),
            _ => {}
        }
    }

    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for inst in instances {
        if !seen.insert(inst.clone()) {
            continue;
        }
        let (port, host) = srv
            .get(&inst)
            .cloned()
            .unwrap_or((RAW_PORT, inst.clone()));
        let addrs = a
            .get(&host)
            .cloned()
            .or_else(|| a.get(&inst).cloned())
            .unwrap_or_default();
        let props = txt.get(&inst).cloned().unwrap_or_default();

        let model = props
            .iter()
            .find(|(k, _)| k == "ty" || k == "model")
            .map(|(_, v)| v.clone())
            .or_else(|| {
                props
                    .iter()
                    .find(|(k, _)| k == "note")
                    .map(|(_, v)| v.clone())
            });
        let note = props.iter().find(|(k, _)| k == "note").map(|(_, v)| v.clone());
        let is_brother = inst.contains("brother")
            || model
                .as_deref()
                .map(|m| m.to_ascii_lowercase().contains("brother"))
                .unwrap_or(false);

        for ip in addrs {
            out.push(DiscoveredPrinter {
                ip,
                hostname: Some(host.clone()),
                model: model.clone(),
                note: note.clone(),
                port: Some(port),
                is_brother,
            });
        }
    }
    Ok(out)
}

fn discover_snmp_broadcast(timeout: Duration, community: &str) -> Result<Vec<DiscoveredPrinter>> {
    let client = SnmpClient::new(community).with_timeout(timeout);
    let responses = client.broadcast_get(&[
        oids::sys_descr(),
        oids::sys_name(),
        oids::sys_object_id(),
        oids::hr_device_descr(),
    ])?;

    let mut out = Vec::new();
    for (src, vbs) in responses {
        let (hostname, model, is_brother) = identity_from_varbinds(&vbs);
        out.push(DiscoveredPrinter {
            ip: src.ip(),
            hostname,
            model,
            note: None,
            port: Some(RAW_PORT),
            is_brother,
        });
    }
    Ok(out)
}

/// Extract `(hostname, model, is_brother)` from SNMP identity varbinds.
fn identity_from_varbinds(vbs: &[Varbind]) -> (Option<String>, Option<String>, bool) {
    let mut hostname = None;
    let mut model = None;
    let mut is_brother = false;
    let mut sys_descr: Option<String> = None;
    for vb in vbs {
        if vb.oid == oids::sys_descr() {
            if let Some(s) = vb.value.as_str() {
                sys_descr = Some(s.to_string());
                if s.to_ascii_lowercase().contains("brother") {
                    is_brother = true;
                }
            }
        } else if vb.oid == oids::hr_device_descr() {
            if let Some(s) = vb.value.as_str() {
                let s = s.trim();
                if !s.is_empty() {
                    model = Some(s.to_string());
                }
            }
        } else if vb.oid == oids::sys_name() {
            hostname = vb.value.as_str().map(|s| s.to_string());
        } else if vb.oid == oids::sys_object_id() {
            if let Some(arcs) = vb.value.as_oid() {
                if arcs.starts_with(&[1, 3, 6, 1, 4, 1, 2435]) {
                    is_brother = true;
                }
            }
        }
    }
    if model.is_none() {
        model = sys_descr.map(|s| s.split(',').next().unwrap_or(&s).trim().to_string());
    }
    (hostname, model, is_brother)
}

/// Query a host's identity over unicast SNMP.
fn identify_host(ip: Ipv4Addr, timeout: Duration, community: &str) -> DiscoveredPrinter {
    let addr = SocketAddr::new(IpAddr::V4(ip), 161);
    let client = SnmpClient::new(community)
        .with_timeout(timeout.min(Duration::from_secs(1)))
        .with_retries(0);
    let (hostname, model, is_brother) = client
        .get(
            addr,
            &[
                oids::sys_descr(),
                oids::sys_name(),
                oids::sys_object_id(),
                oids::hr_device_descr(),
            ],
        )
        .map(|vbs| identity_from_varbinds(&vbs))
        .unwrap_or((None, None, false));
    DiscoveredPrinter {
        ip: IpAddr::V4(ip),
        hostname,
        model,
        note: None,
        port: Some(RAW_PORT),
        is_brother,
    }
}

/// Scan the local /24 for hosts with the raw print port open.
fn scan_local_subnet(timeout: Duration) -> Vec<Ipv4Addr> {
    let Some(local) = mdns::local_ipv4() else {
        return Vec::new();
    };
    let base = u32::from_be_bytes(local.octets()) & 0xffff_ff00;
    let connect_timeout = timeout.clamp(Duration::from_millis(50), Duration::from_millis(500));
    let results = std::sync::Mutex::new(Vec::new());

    let hosts: Vec<Ipv4Addr> = (1..255)
        .map(|i| Ipv4Addr::from(base + i))
        .filter(|ip| *ip != local)
        .collect();

    for chunk in hosts.chunks(64) {
        std::thread::scope(|s| {
            for &ip in chunk {
                let results = &results;
                s.spawn(move || {
                    let addr = SocketAddr::new(IpAddr::V4(ip), RAW_PORT);
                    if TcpStream::connect_timeout(&addr, connect_timeout).is_ok() {
                        results.lock().unwrap().push(ip);
                    }
                });
            }
        });
    }

    results.into_inner().unwrap()
}

fn norm(s: &str) -> String {
    s.trim_end_matches('.').to_ascii_lowercase()
}

/// Resolve a hostname or IP string to an `IpAddr` for direct use.
pub fn resolve_host(host: &str) -> Result<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ip);
    }
    use std::net::ToSocketAddrs;
    let mut addrs = (host, 0).to_socket_addrs()?;
    let first = addrs.next().ok_or(crate::error::Error::NotFound)?;
    Ok(first.ip())
}

/// Convenience: a `SocketAddr` for a host + port.
pub fn socket_addr(host: &str, port: u16) -> Result<SocketAddr> {
    Ok(SocketAddr::new(resolve_host(host)?, port))
}
