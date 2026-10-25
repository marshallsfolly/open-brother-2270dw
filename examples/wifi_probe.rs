//! Development probe: walk the Brother private MIB to discover wireless OIDs.
//!
//! Usage: `cargo run --example wifi_probe -- <ip> [community]`

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use brother_2270dw::snmp::{Oid, SnmpClient};

fn main() {
    let ip = std::env::args().nth(1).expect("usage: wifi_probe <ip> [community]");
    let community = std::env::args().nth(2).unwrap_or_else(|| "public".into());
    let addr: SocketAddr = (ip.as_str(), 161)
        .to_socket_addrs()
        .expect("resolve")
        .next()
        .expect("no addr");

    let client = SnmpClient::new(&community)
        .with_timeout(Duration::from_secs(3))
        .with_retries(1);

    // Walk the Brother enterprise MIB root and print OIDs containing
    // wireless-ish keywords, plus the raw structure.
    let base = Oid::parse("1.3.6.1.4.1.2435").unwrap();
    println!("== walking {} (this may take a moment) ==", base);
    match client.walk_bulk(addr, &base, 128) {
        Ok(vbs) => {
            println!("{} entries total", vbs.len());
            for vb in &vbs {
                let s = format!("{}", vb.oid);
                let v = render(&vb.value);
                // Print everything: wireless keywords may be buried.
                println!("{s} = {v}");
            }
        }
        Err(e) => println!("walk failed: {e}"),
    }
}

fn render(v: &brother_2270dw::ber::Ber) -> String {
    use brother_2270dw::ber::BerContent;
    match &v.content {
        BerContent::Integer(i) => format!("{i}"),
        BerContent::OctetString(b) => format!("{:?}", String::from_utf8_lossy(b)),
        BerContent::Oid(a) => format!("OID({})", Oid(a.clone())),
        BerContent::Counter32(u) => format!("{u}"),
        BerContent::Gauge32(u) => format!("{u}"),
        BerContent::TimeTicks(u) => format!("{u}"),
        BerContent::Null => "null".into(),
        _ => "?".into(),
    }
}
