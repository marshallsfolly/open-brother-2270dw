//! CUPS backend: forward a print job from stdin to the printer's raw port.
//!
//! Register this binary as a CUPS backend (symlink under
//! `/usr/lib/cups/backend/`) and use a device URI of the form
//! `brother2270://<ip>[:9100]`. CUPS sets `$DEVICE_URI`; the job arrives on
//! stdin already converted to the printer's language (PostScript via the PPD's
//! filters).

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use crate::error::{Error, Result};

/// Run the backend (reads stdin, forwards to the raw port).
pub fn backend_main() -> Result<()> {
    let uri = std::env::var("DEVICE_URI").or_else(|_| {
        std::env::args()
            .nth(1)
            .ok_or_else(|| Error::Printer("no device URI (set $DEVICE_URI or pass it as an arg)".into()))
    })?;
    let addr = parse_uri(&uri)?;

    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(30))?;
    stream.set_nodelay(true)?;
    let mut data = Vec::new();
    std::io::stdin().read_to_end(&mut data)?;
    stream.write_all(&data)?;
    stream.flush()?;
    Ok(())
}

/// Parse a `scheme://host[:port]` URI into a socket address.
pub fn parse_uri(uri: &str) -> Result<SocketAddr> {
    let rest = uri.split("://").nth(1).unwrap_or(uri);
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => {
            let port: u16 = p
                .parse()
                .map_err(|_| Error::Printer(format!("bad port in device URI: {p}")))?;
            (h, port)
        }
        _ => (rest, 9100),
    };
    let ip: IpAddr = host
        .parse()
        .map_err(|_| Error::Printer(format!("bad host in device URI: {host}")))?;
    Ok(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uris() {
        assert_eq!(
            parse_uri("brother2270://192.168.1.50:9100").unwrap(),
            "192.168.1.50:9100".parse().unwrap()
        );
        assert_eq!(
            parse_uri("brother2270://192.168.1.50").unwrap(),
            "192.168.1.50:9100".parse().unwrap()
        );
        assert!(parse_uri("brother2270://not-an-ip").is_err());
    }
}
