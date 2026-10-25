//! Printer status, supplies, counters and alerts, read over SNMP.
//!
//! Uses the standard Host Resources MIB and Printer MIB (RFC 1759/3805) layout
//! that the Brother HL-2270DW exposes, verified against a real device.

use std::net::SocketAddr;
use std::time::Duration;

use serde::Serialize;

use crate::error::Result;
use crate::snmp::{oids, Oid, SnmpClient, Varbind};

/// Overall device state, from `hrDeviceStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DeviceState {
    Unknown,
    Running,
    Warning,
    Testing,
    Down,
}

impl DeviceState {
    fn from_code(v: i64) -> Self {
        match v {
            2 => DeviceState::Running,
            3 => DeviceState::Warning,
            4 => DeviceState::Testing,
            5 => DeviceState::Down,
            _ => DeviceState::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DeviceState::Unknown => "unknown",
            DeviceState::Running => "ready",
            DeviceState::Warning => "warning",
            DeviceState::Testing => "testing",
            DeviceState::Down => "down",
        }
    }
}

/// A consumable (toner or drum) with its current level.
#[derive(Debug, Clone, Serialize)]
pub struct Supply {
    pub description: String,
    pub kind: SupplyKind,
    /// Maximum capacity in `unit`s (`None` when the printer reports unknown).
    pub max: Option<u32>,
    /// Current level, if reported as a quantity.
    pub level: Option<u32>,
    /// Qualitative level status (e.g. "Low" when the printer only reports -3).
    pub qualitative: Option<&'static str>,
    pub unit: &'static str,
    /// Remaining percentage (0-100), when both level and max are known.
    pub remaining_percent: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SupplyKind {
    Toner,
    Drum,
    Other,
}

impl SupplyKind {
    pub fn label(self) -> &'static str {
        match self {
            SupplyKind::Toner => "toner",
            SupplyKind::Drum => "drum",
            SupplyKind::Other => "supply",
        }
    }
}

/// An active printer alert (from `prtAlertTable`).
#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    pub severity: &'static str,
    pub description: String,
    pub code: Option<u32>,
}

/// A snapshot of printer identity and status.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceStatus {
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub serial: Option<String>,
    pub hostname: Option<String>,
    pub page_count: Option<u32>,
    pub state: DeviceState,
    pub supplies: Vec<Supply>,
    pub alerts: Vec<Alert>,
}

// Printer MIB object identifiers (relative to `1.3.6.1.2.1.43`).
const PRINTER_MIB: [u32; 7] = [1, 3, 6, 1, 2, 1, 43];
fn mib_oid(tail: &[u32]) -> Oid {
    let mut a = PRINTER_MIB.to_vec();
    a.extend_from_slice(tail);
    Oid(a)
}
fn serial_oid() -> Oid {
    mib_oid(&[5, 1, 1, 17, 1])
}
fn life_count_oid() -> Oid {
    mib_oid(&[10, 2, 1, 4, 1, 1])
}
fn supplies_table() -> Oid {
    mib_oid(&[11, 1, 1])
}
fn alerts_table() -> Oid {
    mib_oid(&[18, 1, 1])
}

/// Query device identity, supplies and alerts from the printer.
pub fn query_status(addr: SocketAddr, community: &str, timeout: Duration) -> Result<DeviceStatus> {
    let client = SnmpClient::new(community)
        .with_timeout(timeout)
        .with_retries(1);

    let scalars = client.get(
        addr,
        &[
            oids::sys_descr(),
            oids::sys_name(),
            serial_oid(),
            life_count_oid(),
            oids::hr_device_status(),
            oids::hr_device_descr(),
        ],
    )?;

    let mut status = DeviceStatus {
        model: None,
        firmware: None,
        serial: None,
        hostname: None,
        page_count: None,
        state: DeviceState::Unknown,
        supplies: Vec::new(),
        alerts: Vec::new(),
    };

    let mut sys_descr: Option<String> = None;
    for vb in &scalars {
        if vb.oid == oids::sys_descr() {
            if let Some(s) = vb.value.as_str() {
                sys_descr = Some(s.to_string());
                // "Brother NC-8200h, Firmware Ver.1.03  (10.05.20),MID 00ZZ99"
                status.firmware = s
                    .split("Firmware Ver.")
                    .nth(1)
                    .map(|f| f.split(',').next().unwrap_or(f).trim().to_string());
            }
        } else if vb.oid == oids::hr_device_descr() {
            if let Some(s) = vb.value.as_str() {
                let s = s.trim();
                if !s.is_empty() {
                    status.model = Some(s.to_string());
                }
            }
        } else if vb.oid == oids::sys_name() {
            status.hostname = vb.value.as_str().map(|s| s.to_string());
        } else if vb.oid == serial_oid() {
            status.serial = vb.value.as_str().map(|s| s.to_string());
        } else if vb.oid == life_count_oid() {
            status.page_count = vb.value.as_u64().map(|v| v as u32);
        } else if vb.oid == oids::hr_device_status() {
            status.state = vb
                .value
                .as_i64()
                .map(DeviceState::from_code)
                .unwrap_or(DeviceState::Unknown);
        }
    }

    if status.model.is_none() {
        status.model =
            sys_descr.map(|s| s.split(',').next().unwrap_or(&s).trim().to_string());
    }

    // Supplies.
    if let Ok(vbs) = client.walk_bulk(addr, &supplies_table(), 64) {
        status.supplies = parse_supplies(&vbs);
    }

    // Alerts.
    if let Ok(vbs) = client.walk_bulk(addr, &alerts_table(), 64) {
        status.alerts = parse_alerts(&vbs);
    }

    Ok(status)
}

fn parse_supplies(vbs: &[Varbind]) -> Vec<Supply> {
    // Group columns by supply index (the last arc; the arc before it is `.1`).
    let mut columns: std::collections::BTreeMap<u32, Vec<(u32, Varbind)>> =
        std::collections::BTreeMap::new();
    for vb in vbs {
        let arcs = vb.oid.as_slice();
        if arcs.len() < 3 {
            continue;
        }
        let (col, marker, idx) = (arcs[arcs.len() - 3], arcs[arcs.len() - 2], arcs[arcs.len() - 1]);
        if marker != 1 {
            continue;
        }
        columns.entry(idx).or_default().push((col, vb.clone()));
    }

    let mut supplies = Vec::new();
    for (_idx, cols) in columns {
        let mut s = Supply {
            description: String::new(),
            kind: SupplyKind::Other,
            max: None,
            level: None,
            qualitative: None,
            unit: "units",
            remaining_percent: None,
        };
        for (col, vb) in cols {
            match col {
                5 => s.kind = match vb.value.as_u64() {
                    Some(3) => SupplyKind::Toner,
                    Some(9) => SupplyKind::Drum,
                    _ => SupplyKind::Other,
                },
                6 => s.description = vb.value.as_str().unwrap_or("").to_string(),
                7 => s.unit = match vb.value.as_u64() {
                    Some(7) => "impressions",
                    Some(8) => "sheets",
                    Some(17) => "percent",
                    _ => "units",
                },
                8 => s.max = vb.value.as_u64().filter(|&v| v != u64::from(u32::MAX)).map(|v| v as u32),
                9 => match vb.value.as_i64() {
                    Some(-3) => s.qualitative = Some("Low"),
                    Some(-2) | None => {}
                    Some(-1) => s.qualitative = Some("Other"),
                    Some(v) if v >= 0 => s.level = Some(v as u32),
                    Some(_) => {}
                },
                _ => {}
            }
        }
        if s.description.is_empty() {
            s.description = s.kind.label().to_string();
        }
        if let (Some(level), Some(max)) = (s.level, s.max) {
            if max > 0 {
                s.remaining_percent = Some((level as f32 / max as f32) * 100.0);
            }
        }
        supplies.push(s);
    }
    supplies
}

fn parse_alerts(vbs: &[Varbind]) -> Vec<Alert> {
    let mut columns: std::collections::BTreeMap<u32, Vec<(u32, Varbind)>> =
        std::collections::BTreeMap::new();
    for vb in vbs {
        let arcs = vb.oid.as_slice();
        if arcs.len() < 3 {
            continue;
        }
        let (col, marker, idx) = (arcs[arcs.len() - 3], arcs[arcs.len() - 2], arcs[arcs.len() - 1]);
        if marker != 1 {
            continue;
        }
        columns.entry(idx).or_default().push((col, vb.clone()));
    }

    let mut alerts = Vec::new();
    for (_idx, cols) in columns {
        let mut a = Alert { severity: "unknown", description: String::new(), code: None };
        for (col, vb) in cols {
            match col {
                2 => {
                    a.severity = match vb.value.as_u64() {
                        Some(3) => "critical",
                        Some(4) => "warning",
                        Some(5) => "indeterminate",
                        _ => "unknown",
                    };
                }
                7 => a.code = vb.value.as_u64().map(|v| v as u32),
                8 => a.description = vb.value.as_str().unwrap_or("").to_string(),
                _ => {}
            }
        }
        if !a.description.is_empty() {
            alerts.push(a);
        }
    }
    alerts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ber::{Ber, BerContent};

    fn vb(oid: Oid, v: Ber) -> Varbind {
        Varbind { oid, value: v }
    }

    #[test]
    fn parses_toner_and_drum() {
        let vbs = vec![
            vb(Oid::parse("1.3.6.1.2.1.43.11.1.1.6.1.1").unwrap(), Ber::octet_string("Black Toner Cartridge")),
            vb(Oid::parse("1.3.6.1.2.1.43.11.1.1.9.1.1").unwrap(), Ber::integer(-3)),
            vb(Oid::parse("1.3.6.1.2.1.43.11.1.1.6.1.2").unwrap(), Ber::octet_string("Drum Unit")),
            vb(Oid::parse("1.3.6.1.2.1.43.11.1.1.8.1.2").unwrap(), Ber::integer(12000)),
            vb(Oid::parse("1.3.6.1.2.1.43.11.1.1.9.1.2").unwrap(), Ber::integer(8741)),
        ];
        let supplies = parse_supplies(&vbs);
        assert_eq!(supplies.len(), 2);
        let toner = &supplies[0];
        assert_eq!(toner.description, "Black Toner Cartridge");
        assert_eq!(toner.qualitative, Some("Low"));
        let drum = &supplies[1];
        assert_eq!(drum.description, "Drum Unit");
        assert_eq!(drum.max, Some(12000));
        assert_eq!(drum.level, Some(8741));
        assert_eq!(drum.unit, "units");
        assert!(drum.remaining_percent.is_some());
    }

    #[test]
    fn parses_toner_low_alert() {
        let vbs = vec![
            vb(Oid::parse("1.3.6.1.2.1.43.18.1.1.2.1.1").unwrap(), Ber::integer(4)),
            vb(Oid::parse("1.3.6.1.2.1.43.18.1.1.8.1.1").unwrap(), Ber::octet_string("TONER LOW")),
        ];
        let alerts = parse_alerts(&vbs);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, "warning");
        assert_eq!(alerts[0].description, "TONER LOW");
    }

    #[test]
    fn device_state_mapping() {
        assert_eq!(DeviceState::from_code(2), DeviceState::Running);
        assert_eq!(DeviceState::from_code(3), DeviceState::Warning);
        assert_eq!(DeviceState::from_code(5), DeviceState::Down);
        assert_eq!(DeviceState::from_code(99), DeviceState::Unknown);
    }

    #[test]
    fn ber_content_is_public() {
        // Guard that the BerContent type is reachable from status tests.
        let _ = BerContent::Integer(3);
    }
}
