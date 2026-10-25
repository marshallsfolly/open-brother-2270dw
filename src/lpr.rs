//! LPR/LPD client (RFC 1179) for printing on port 515.
//!
//! Useful as a fallback when raw port 9100 is blocked. The HL-2270DW ships
//! with an LPD daemon; its "BINARY_P1" queue accepts raw PCL/PS data.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use crate::error::{Error, Result};

pub const LPR_PORT: u16 = 515;
pub const DEFAULT_QUEUE: &str = "BINARY_P1";

/// Submit `data` to `queue` on the printer as a single LPR job.
pub fn print(
    addr: IpAddr,
    queue: &str,
    job_name: &str,
    data: &[u8],
    timeout: Duration,
) -> Result<()> {
    let mut stream = TcpStream::connect_timeout(&SocketAddr::new(addr, LPR_PORT), timeout)?;
    stream.set_write_timeout(Some(timeout))?;
    stream.set_read_timeout(Some(timeout))?;

    let host = hostname();

    // 1. "Receive a printer job" command.
    write_cmd(&mut stream, &format!("\x02{queue}\n"))?;
    expect_ack(&mut stream)?;

    // 2. Send control file.
    let df = format!("dfA000{host}");
    let control = build_control_file(&host, &df, job_name);
    write_cmd(&mut stream, &format!("\x02{} cfA000{host}\n", control.len()))?;
    stream.write_all(&control)?;
    stream.write_all(&[0x00])?;
    stream.flush()?;
    expect_ack(&mut stream)?;

    // 3. Send data file.
    write_cmd(&mut stream, &format!("\x03{} {df}\n", data.len()))?;
    stream.write_all(data)?;
    stream.write_all(&[0x00])?;
    stream.flush()?;
    expect_ack(&mut stream)?;

    Ok(())
}

fn write_cmd(stream: &mut TcpStream, cmd: &str) -> Result<()> {
    stream.write_all(cmd.as_bytes())?;
    Ok(())
}

fn expect_ack(stream: &mut TcpStream) -> Result<()> {
    let mut b = [0u8; 1];
    stream.read_exact(&mut b)?;
    if b[0] != 0x00 {
        return Err(Error::Printer(format!("LPR server returned error code {}", b[0])));
    }
    Ok(())
}

/// Build the LPD control file (a set of subcommands, one per line).
fn build_control_file(host: &str, df: &str, job_name: &str) -> Vec<u8> {
    let mut c = Vec::new();
    c.extend_from_slice(format!("H{host}\n").as_bytes());
    c.extend_from_slice(format!("P{}\n", username()).as_bytes());
    c.extend_from_slice(format!("J{}\n", sanitize(job_name)).as_bytes());
    // 'l' = print the data file leaving control characters intact (raw).
    c.extend_from_slice(format!("l{df}\n").as_bytes());
    c.extend_from_slice(format!("U{df}\n").as_bytes());
    c.extend_from_slice(format!("N{}\n", sanitize(job_name)).as_bytes());
    c
}

fn sanitize(s: &str) -> String {
    s.replace('"', "'").replace(['\r', '\n'], " ")
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "localhost".into())
}

fn username() -> String {
    std::env::var("USER").unwrap_or_else(|_| "user".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_file_contains_data_file_refs() {
        let c = build_control_file("host", "dfA000host", "my job");
        let s = String::from_utf8(c).unwrap();
        assert!(s.contains("Hhost\n"));
        assert!(s.contains("ldfA000host\n"));
        assert!(s.contains("UdfA000host\n"));
        assert!(s.contains("Jmy job\n"));
    }
}
