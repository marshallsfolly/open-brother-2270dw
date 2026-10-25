//! Health assessment, alerting, and Prometheus metrics for printer status.

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::status::{DeviceState, DeviceStatus, SupplyKind};

/// Severity of the printer's current condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum AlertLevel {
    Healthy,
    Warning,
    Critical,
}

impl AlertLevel {
    pub fn label(self) -> &'static str {
        match self {
            AlertLevel::Healthy => "healthy",
            AlertLevel::Warning => "warning",
            AlertLevel::Critical => "critical",
        }
    }

    /// Process exit code convention: 0=healthy, 1=warning, 2=critical.
    pub fn exit_code(self) -> i32 {
        match self {
            AlertLevel::Healthy => 0,
            AlertLevel::Warning => 1,
            AlertLevel::Critical => 2,
        }
    }
}

/// Assess overall health from a status snapshot.
pub fn assess(status: &DeviceStatus) -> AlertLevel {
    if status.state == DeviceState::Down {
        return AlertLevel::Critical;
    }
    for a in &status.alerts {
        if a.severity == "critical" {
            return AlertLevel::Critical;
        }
    }
    for s in &status.supplies {
        match s.kind {
            SupplyKind::Toner
                if (s.qualitative == Some("Low")
                    || s.remaining_percent.map(|p| p < 10.0).unwrap_or(false))
                => {
                    return AlertLevel::Warning;
                }
            SupplyKind::Drum
                if s.remaining_percent.map(|p| p < 10.0).unwrap_or(false) => {
                    return AlertLevel::Warning;
                }
            _ => {}
        }
    }
    if status.state == DeviceState::Warning {
        return AlertLevel::Warning;
    }
    AlertLevel::Healthy
}

/// Render a Prometheus text-exposition format snapshot.
pub fn prometheus_metrics(status: &DeviceStatus) -> String {
    let mut out = String::new();
    let state = match status.state {
        DeviceState::Running => 0,
        DeviceState::Warning => 1,
        DeviceState::Testing => 2,
        DeviceState::Down => 3,
        DeviceState::Unknown => 4,
    };
    out.push_str("# TYPE brother_printer_state gauge\n");
    out.push_str(&format!("brother_printer_state {state}\n"));
    if let Some(pc) = status.page_count {
        out.push_str("# TYPE brother_page_count counter\n");
        out.push_str(&format!("brother_page_count {pc}\n"));
    }
    for s in &status.supplies {
        if let Some(pct) = s.remaining_percent {
            let name = sanitize_label(&s.description);
            out.push_str("# TYPE brother_supply_remaining_percent gauge\n");
            out.push_str(&format!(
                "brother_supply_remaining_percent{{supply=\"{name}\",kind=\"{}\"}} {pct:.1}\n",
                s.kind.label()
            ));
        }
    }
    out.push_str("# TYPE brother_active_alerts gauge\n");
    out.push_str(&format!("brother_active_alerts {}\n", status.alerts.len()));
    out
}

/// POST a status snapshot as JSON to a webhook URL.
pub fn webhook_notify(url: &str, status: &DeviceStatus) -> Result<()> {
    let body = serde_json::to_string(status)
        .map_err(|e| Error::Config(format!("serialize status: {e}")))?;
    ureq::post(url)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(10))
        .send_string(&body)
        .map_err(|e| Error::Printer(format!("webhook {url}: {e}")))?;
    Ok(())
}

/// Run a shell command, piping the JSON status to its stdin.
pub fn command_notify(cmd: &str, status: &DeviceStatus) -> Result<()> {
    let body = serde_json::to_string(status)
        .map_err(|e| Error::Config(format!("serialize status: {e}")))?;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Printer(format!("spawn command: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.as_bytes());
    }
    let status = child
        .wait()
        .map_err(|e| Error::Printer(format!("wait command: {e}")))?;
    if !status.success() {
        return Err(Error::Printer(format!("command exited with {status}")));
    }
    Ok(())
}

fn sanitize_label(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{Alert, Supply, SupplyKind};

    fn status_with(toner_low: bool, drum_pct: Option<f32>) -> DeviceStatus {
        DeviceStatus {
            model: None,
            firmware: None,
            serial: None,
            hostname: None,
            page_count: Some(100),
            state: DeviceState::Running,
            supplies: vec![
                Supply {
                    description: "Black Toner Cartridge".into(),
                    kind: SupplyKind::Toner,
                    max: None,
                    level: None,
                    qualitative: if toner_low { Some("Low") } else { None },
                    unit: "units",
                    remaining_percent: None,
                },
                Supply {
                    description: "Drum Unit".into(),
                    kind: SupplyKind::Drum,
                    max: Some(12000),
                    level: Some(1200),
                    qualitative: None,
                    unit: "impressions",
                    remaining_percent: drum_pct,
                },
            ],
            alerts: Vec::new(),
        }
    }

    #[test]
    fn assess_levels() {
        assert_eq!(assess(&status_with(false, Some(72.0))), AlertLevel::Healthy);
        assert_eq!(assess(&status_with(true, Some(72.0))), AlertLevel::Warning);
        assert_eq!(assess(&status_with(false, Some(5.0))), AlertLevel::Warning);
    }

    #[test]
    fn assess_critical_alert() {
        let mut s = status_with(false, Some(72.0));
        s.alerts.push(Alert {
            severity: "critical",
            description: "PAPER JAM".into(),
            code: None,
        });
        assert_eq!(assess(&s), AlertLevel::Critical);
    }

    #[test]
    fn metrics_contains_key_series() {
        let m = prometheus_metrics(&status_with(true, Some(72.0)));
        assert!(m.contains("brother_printer_state"));
        assert!(m.contains("brother_page_count 100"));
        assert!(m.contains("brother_supply_remaining_percent"));
    }
}
