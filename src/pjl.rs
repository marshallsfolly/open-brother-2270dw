//! PJL (Printer Job Language) command builders and response parsing.
//!
//! PJL is the language used to wrap and control print jobs on Brother (and
//! most other) printers: job boundaries, paper, duplex, resolution, copies,
//! and device `INFO`/`STATUS` queries all travel through PJL.

use crate::job::{Duplex, JobOptions, Language, Orientation};

/// Universal Exit Language sequence (`ESC %-12345X`).
pub const UEL: &[u8] = b"\x1b%-12345X";

/// Build a single `@PJL <command><CR><LF>` line.
pub fn cmd_line(cmd: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(cmd.len() + 8);
    v.extend_from_slice(b"@PJL ");
    v.extend_from_slice(cmd.as_bytes());
    v.extend_from_slice(b"\r\n");
    v
}

/// The `@PJL JOB NAME ...` header plus all `SET` commands that configure the
/// page for this job.
pub fn job_header(opts: &JobOptions) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&cmd_line(&format!(
        "JOB NAME = \"{}\"",
        sanitize(&opts.job_name)
    )));
    out.extend_from_slice(&cmd_line(&format!(
        "SET PAPER = {}",
        opts.paper.pjl_name()
    )));
    out.extend_from_slice(&cmd_line(&format!(
        "SET ORIENTATION = {}",
        match opts.orientation {
            Orientation::Portrait => "PORTRAIT",
            Orientation::Landscape => "LANDSCAPE",
        }
    )));
    out.extend_from_slice(&cmd_line(&format!(
        "SET RESOLUTION = {}",
        opts.resolution
    )));
    out.extend_from_slice(&cmd_line(&format!("SET COPIES = {}", opts.copies.max(1))));
    match opts.duplex {
        Duplex::Off => out.extend_from_slice(&cmd_line("SET DUPLEX = OFF")),
        Duplex::LongEdge => {
            out.extend_from_slice(&cmd_line("SET DUPLEX = ON"));
            out.extend_from_slice(&cmd_line("SET BINDING = LONGEDGE"));
        }
        Duplex::ShortEdge => {
            out.extend_from_slice(&cmd_line("SET DUPLEX = ON"));
            out.extend_from_slice(&cmd_line("SET BINDING = SHORTEDGE"));
        }
    }
    out.extend_from_slice(&cmd_line(&format!(
        "SET TONERSAVE = {}",
        if opts.toner_save { "ON" } else { "OFF" }
    )));
    out
}

/// `@PJL ENTER LANGUAGE = ...`
pub fn enter_language(lang: Language) -> Vec<u8> {
    let name = match lang {
        Language::Pcl => "PCL",
        Language::Pclxl => "PCLXL",
        Language::Postscript => "POSTSCRIPT",
    };
    cmd_line(&format!("ENTER LANGUAGE = {name}"))
}

/// End-of-job sequence: `UEL` (exit the page language) + `@PJL EOJ` + `UEL`.
pub fn end_job() -> Vec<u8> {
    let mut out = UEL.to_vec();
    out.extend_from_slice(&cmd_line("EOJ"));
    out.extend_from_slice(UEL);
    out
}

/// Reset the printer to a clean state (UEL alone).
pub fn reset() -> Vec<u8> {
    UEL.to_vec()
}

/// A full `@PJL INFO <kind>` request packet: `UEL` + `INFO` + `UEL`.
pub fn info_request(kind: &str) -> Vec<u8> {
    let mut out = UEL.to_vec();
    out.extend_from_slice(&cmd_line(&format!("INFO {kind}")));
    out.extend_from_slice(UEL);
    out
}

/// Parse a PJL `INFO` response into `(key, value)` pairs. Lines are typically
/// of the form `"Model Name" = "Brother HL-2270DW series"`, terminated by a
/// form feed.
pub fn parse_info_response(bytes: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_end_matches('\u{0c}').trim();
        if line.is_empty() || line.starts_with('@') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim().trim_matches('"').trim();
            let v = v.trim().trim_matches('"').trim();
            out.push((k.to_string(), v.to_string()));
        }
    }
    out
}

/// Parse the `@PJL INFO ID` response, which is a single quoted string of the
/// form `"<model>:<machine-id>:<firmware>"`. Also tolerates `key = value`
/// lines for other INFO kinds.
pub fn parse_info_id(bytes: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_end_matches('\u{0c}').trim();
        if line.is_empty() || line.starts_with('@') {
            continue;
        }
        if let Some(inner) = line.strip_prefix('"').and_then(|l| l.strip_suffix('"')) {
            let parts: Vec<&str> = inner.split(':').collect();
            if let Some(model) = parts.first().filter(|s| !s.is_empty()) {
                out.push(("Model".to_string(), model.to_string()));
            }
            if parts.len() >= 2 {
                out.push(("Machine ID".to_string(), parts[1].to_string()));
            }
            if parts.len() >= 3 {
                out.push(("Firmware".to_string(), parts[2].to_string()));
            }
        } else if let Some((k, v)) = line.split_once('=') {
            out.push((
                k.trim().trim_matches('"').to_string(),
                v.trim().trim_matches('"').to_string(),
            ));
        }
    }
    out
}

/// Strip characters that would break a PJL quoted string.
fn sanitize(s: &str) -> String {
    s.replace('"', "'").replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_line_format() {
        assert_eq!(cmd_line("SET COPIES = 2"), b"@PJL SET COPIES = 2\r\n");
    }

    #[test]
    fn uel_is_exit_language() {
        assert_eq!(UEL, b"\x1b%-12345X");
    }

    #[test]
    fn end_job_exits_language_before_eoj() {
        let end = end_job();
        // Must start with UEL (exit PCL/PS back to PJL) *before* the EOJ line.
        assert!(end.starts_with(UEL));
        let s = String::from_utf8(end.clone()).unwrap();
        let eoj = s.find("@PJL EOJ").expect("contains EOJ");
        assert!(eoj > 0, "EOJ must come after a UEL");
        assert!(end.ends_with(UEL));
    }

    #[test]
    fn parse_info_extracts_pairs() {
        let resp = b"@PJL INFO ID\r\n\"Model Name\" = \"Brother HL-2270DW series\"\r\n\"Serial Number\" = \"U1234567890ABCDE\"\r\n\x0c";
        let pairs = parse_info_response(resp);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("Model Name".to_string(), "Brother HL-2270DW series".to_string()));
        assert_eq!(pairs[1], ("Serial Number".to_string(), "U1234567890ABCDE".to_string()));
    }

    #[test]
    fn parse_info_id_single_string() {
        let resp = b"@PJL INFO ID\r\n\"Brother HL-2270DW series:00ZZ99:Ver1.02\"\r\n\x0c";
        let pairs = parse_info_id(resp);
        assert_eq!(pairs[0], ("Model".to_string(), "Brother HL-2270DW series".to_string()));
        assert_eq!(pairs[1], ("Machine ID".to_string(), "00ZZ99".to_string()));
        assert_eq!(pairs[2], ("Firmware".to_string(), "Ver1.02".to_string()));
    }

    #[test]
    fn header_contains_duplex_binding() {
        let opts = JobOptions {
            duplex: Duplex::LongEdge,
            ..JobOptions::default()
        };
        let header = job_header(&opts);
        let s = String::from_utf8_lossy(&header);
        assert!(s.contains("SET DUPLEX = ON"));
        assert!(s.contains("SET BINDING = LONGEDGE"));
    }
}
