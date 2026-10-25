//! Development probe: exercise the SNMP client against a real printer.
//!
//! Usage: `cargo run --example snmp_probe -- <ip> [community]`

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use brother_2270dw::ber::{Ber, BerContent};
use brother_2270dw::snmp::{oids, Oid, SnmpClient};

fn main() {
    let ip = std::env::args().nth(1).expect("usage: snmp_probe <ip> [community]");
    let community = std::env::args().nth(2).unwrap_or_else(|| "public".into());

    let addr: SocketAddr = (ip.as_str(), 161)
        .to_socket_addrs()
        .expect("resolve")
        .next()
        .expect("no addr");

    let client = SnmpClient::new(&community)
        .with_timeout(Duration::from_secs(3))
        .with_retries(1);

    let scalar_oids = [
        ("sysDescr", oids::sys_descr()),
        ("sysObjectID", oids::sys_object_id()),
        ("sysUpTime", oids::sys_up_time()),
        ("sysContact", oids::sys_contact()),
        ("sysName", oids::sys_name()),
        ("sysLocation", oids::sys_location()),
        ("hrDeviceStatus", oids::hr_device_status()),
    ];
    let oids: Vec<Oid> = scalar_oids.iter().map(|(_, o)| o.clone()).collect();

    println!("== GET scalars from {addr} (community={community}) ==");
    match client.get(addr, &oids) {
        Ok(vbs) => {
            for vb in vbs {
                println!("{:<14} {} => {}", label_for(&vb.oid, &scalar_oids), vb.oid, render(&vb.value));
            }
        }
        Err(e) => println!("GET failed: {e}"),
    }

    println!("\n== WALK printer MIB (1.3.6.1.2.1.43) ==");
    match client.walk_bulk(addr, &Oid::parse("1.3.6.1.2.1.43").unwrap(), 64) {
        Ok(vbs) => {
            for vb in &vbs {
                println!("{} = {}", vb.oid, render(&vb.value));
            }
            println!("({} entries)", vbs.len());
        }
        Err(e) => println!("WALK failed: {e}"),
    }
}

fn label_for<'a>(oid: &Oid, table: &'a [(&'a str, Oid)]) -> &'a str {
    table
        .iter()
        .find(|(_, o)| o == oid)
        .map(|(n, _)| *n)
        .unwrap_or("")
}

fn render(v: &Ber) -> String {
    match &v.content {
        BerContent::Integer(i) => format!("{i} (integer)"),
        BerContent::OctetString(b) => format!("{:?} (string)", String::from_utf8_lossy(b)),
        BerContent::Null => "null".into(),
        BerContent::Oid(a) => format!("OID({})", Oid(a.clone())),
        BerContent::IpAddress(a) => format!("{}.{}.{}.{} (ip)", a[0], a[1], a[2], a[3]),
        BerContent::Counter32(u) => format!("{u} (counter32)"),
        BerContent::Gauge32(u) => format!("{u} (gauge32)"),
        BerContent::TimeTicks(u) => format!("{u} (timeticks)"),
        BerContent::Counter64(u) => format!("{u} (counter64)"),
        BerContent::Opaque(b) => format!("opaque({} bytes)", b.len()),
        BerContent::Sequence(_) => "sequence".into(),
        BerContent::Raw(b) => format!("raw({} bytes)", b.len()),
        BerContent::Boolean(b) => format!("{b} (bool)"),
    }
}
