//! Raw TCP printing on port 9100 ("HP JetDirect" compatible).
//!
//! This is the primary, most reliable transport for the HL-2270DW.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use crate::error::Result;

pub const RAW_PORT: u16 = 9100;

/// A connection to the printer's raw (port 9100) listener.
pub struct RawConnection {
    stream: TcpStream,
}

impl RawConnection {
    pub fn connect(addr: IpAddr, port: u16, timeout: Duration) -> Result<Self> {
        let stream = TcpStream::connect_timeout(&SocketAddr::new(addr, port), timeout)?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        Ok(RawConnection { stream })
    }

    /// Write a complete job without waiting for a reply.
    pub fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.stream.write_all(bytes)?;
        self.stream.flush()?;
        Ok(())
    }

    /// Write a request and collect the synchronous response (used for PJL
    /// `INFO`/`STATUS` queries). Reading stops on connection close or a read
    /// timeout.
    pub fn send_and_read(&mut self, bytes: &[u8], read_timeout: Duration) -> Result<Vec<u8>> {
        self.stream.set_read_timeout(Some(read_timeout))?;
        self.stream.write_all(bytes)?;
        self.stream.flush()?;

        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
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
}
