//! USB access to the printer.
//!
//! Brother's Linux tooling configures the HL-2270DW's Wi-Fi over **Ethernet**
//! (via its `/bio/` web UI — see [`crate::wifi`]), not over USB; the USB
//! wireless "wizard" is Windows/macOS-only proprietary software. Over USB we
//! can detect the printer and read its IEEE 1284 device ID, which is what this
//! module provides.

use std::time::Duration;

use rusb::{Context, DeviceHandle, UsbContext};

use crate::error::{Error, Result};

/// Brother vendor ID.
pub const VID: u16 = 0x04f9;
/// HL-2270DW product ID.
pub const PID: u16 = 0x0042;

/// A handle to the printer's USB interface.
pub struct UsbPrinter {
    handle: DeviceHandle<Context>,
}

impl UsbPrinter {
    /// Open the printer by VID/PID and claim its interface.
    pub fn open() -> Result<Self> {
        let ctx = Context::new().map_err(|e| Error::Printer(format!("usb context: {e}")))?;
        let device = ctx
            .devices()
            .map_err(|e| Error::Printer(format!("usb devices: {e}")))?
            .iter()
            .find(|d| {
                d.device_descriptor()
                    .map(|dd| dd.vendor_id() == VID && dd.product_id() == PID)
                    .unwrap_or(false)
            })
            .ok_or(Error::NotFound)?;

        let handle = device
            .open()
            .map_err(|e| Error::Printer(format!("open usb device: {e}")))?;
        handle
            .claim_interface(0)
            .map_err(|e| Error::Printer(format!("claim usb interface: {e}")))?;
        Ok(UsbPrinter { handle })
    }

    /// USB Printer-class `GET_DEVICE_ID` (IEEE 1284 device ID string). The
    /// response is prefixed by a two-byte big-endian length, which is stripped.
    pub fn device_id(&self) -> Result<String> {
        let mut buf = [0u8; 256];
        let n = self
            .handle
            .read_control(0xA1, 0x00, 0, 0, &mut buf, Duration::from_secs(3))
            .map_err(|e| Error::Printer(format!("GET_DEVICE_ID: {e}")))?;
        if n >= 2 {
            let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
            let end = (len + 2).min(n);
            return Ok(String::from_utf8_lossy(&buf[2..end]).into_owned());
        }
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }
}
