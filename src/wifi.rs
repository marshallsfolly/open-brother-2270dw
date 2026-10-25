//! Wi-Fi configuration via the printer's embedded network web UI.
//!
//! The HL-2270DW's network card serves a config UI under `/bio/`. Wireless
//! settings live at `/bio/wlan.html` and are committed by POSTing a form to
//! `/bio/post/wlan.html?tab=1` with HTTP Basic auth (`admin` / the configured
//! admin password, default `access`).

use std::time::Duration;

use crate::error::{Error, Result};

/// Auth-mode values accepted by the web UI.
pub const AUTH_OPEN: &str = "1";
pub const AUTH_SHARED_WEP: &str = "2";
pub const AUTH_WPA_WPA2: &str = "3";

/// Encryption-mode values accepted by the web UI.
pub const ENC_TKIP: &str = "3";
pub const ENC_AES: &str = "4";

/// Current wireless configuration.
#[derive(Debug, Clone)]
pub struct WifiStatus {
    pub ssid: String,
    pub mode: String,
    pub auth: String,
    pub encryption: String,
}

/// Read the current wireless configuration.
pub fn status(host: &str, password: &str) -> Result<WifiStatus> {
    let body = get(host, password, "/bio/wlan.html")?;
    Ok(parse_wlan_page(&body))
}

/// Write the wireless configuration (Infrastructure mode).
pub fn configure(
    host: &str,
    password: &str,
    ssid: &str,
    auth: &str,
    encryption: &str,
    network_key: Option<&str>,
) -> Result<()> {
    let mut params: Vec<(&str, &str)> = vec![
        ("Com_Mode", "1"),
        ("WLan_SSID", ssid),
        ("auth", auth),
        ("WLan_Encryption", encryption),
    ];
    if let Some(k) = network_key {
        params.push(("WLan_NetworkKey", k));
    }
    let code = post_form(host, password, "/bio/post/wlan.html?tab=1", &params)?;
    if code != 200 {
        return Err(Error::Printer(format!(
            "wireless config POST returned HTTP {code}"
        )));
    }
    Ok(())
}

fn get(host: &str, password: &str, path: &str) -> Result<String> {
    let url = format!("http://{host}{path}");
    let resp = ureq::get(&url)
        .set("Authorization", &basic_auth(password))
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| Error::Printer(format!("GET {url}: {e}")))?;
    if resp.status() != 200 {
        return Err(Error::Printer(format!("GET {url}: HTTP {}", resp.status())));
    }
    resp.into_string()
        .map_err(|e| Error::Printer(format!("read {url}: {e}")))
}

fn post_form(host: &str, password: &str, path: &str, form: &[(&str, &str)]) -> Result<u16> {
    let url = format!("http://{host}{path}");
    let resp = ureq::post(&url)
        .set("Authorization", &basic_auth(password))
        .timeout(Duration::from_secs(15))
        .send_form(form)
        .map_err(|e| Error::Printer(format!("POST {url}: {e}")))?;
    Ok(resp.status())
}

fn basic_auth(password: &str) -> String {
    format!("Basic {}", base64(format!("admin:{password}").as_bytes()))
}

fn base64(input: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Parse the `/bio/wlan.html` page into a [`WifiStatus`].
fn parse_wlan_page(html: &str) -> WifiStatus {
    let ssid = input_value(html, "WLan_SSID").unwrap_or_default();
    let mode = match selected_option(html, "Com_Mode").as_deref() {
        Some("2") => "ad-hoc".to_string(),
        _ => "infrastructure".to_string(),
    };
    let auth = match selected_option(html, "auth").as_deref() {
        Some("1") => "open".to_string(),
        Some("2") => "shared-key (WEP)".to_string(),
        Some("3") => "WPA/WPA2-PSK".to_string(),
        _ => "unknown".to_string(),
    };
    let encryption = match selected_option(html, "WLan_Encryption").as_deref() {
        Some("3") => "TKIP".to_string(),
        Some("4") => "AES".to_string(),
        _ => "none".to_string(),
    };
    WifiStatus {
        ssid,
        mode,
        auth,
        encryption,
    }
}

/// Extract the `VALUE="…"` of the `<input NAME="…">` control named `name`.
fn input_value(html: &str, name: &str) -> Option<String> {
    let needle = format!("NAME=\"{name}\"");
    let start = html.find(&needle)?;
    let tail = &html[start..];
    let value_marker = tail.find("VALUE=\"")?;
    let after = &tail[value_marker + 7..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

/// Extract the `VALUE` of the selected `<OPTION>` in `<SELECT NAME="name">`.
fn selected_option(html: &str, name: &str) -> Option<String> {
    let needle = format!("NAME=\"{name}\"");
    let start = html.find(&needle)?;
    let block = &html[start..];
    let block_end = block.find("</SELECT>")?;
    let block = &block[..block_end];
    let selected = block.find("SELECTED")?;
    let before = &block[..selected];
    let value_at = before.rfind("VALUE=")?;
    let value = &before[value_at + 6..];
    let end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    Some(value[..end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<SELECT NAME="Com_Mode" SIZE=1>
<OPTION VALUE=1 SELECTED>Infrastructure</OPTION>
<OPTION VALUE=2 >Ad-hoc</OPTION>
</SELECT>
<INPUT TYPE=TEXT NAME="WLan_SSID" VALUE="HomeNet">
<SELECT NAME="auth" SIZE=1>
<OPTION VALUE=1 >Open</OPTION>
<OPTION VALUE=2 >Shared</OPTION>
<OPTION VALUE=3 SELECTED>WPA/WPA2-PSK</OPTION>
</SELECT>
<SELECT NAME="WLan_Encryption" SIZE=1>
<OPTION VALUE=3 >TKIP</OPTION>
<OPTION VALUE=4 SELECTED>AES</OPTION>
</SELECT>"#;

    #[test]
    fn parses_wlan_page() {
        let s = parse_wlan_page(PAGE);
        assert_eq!(s.ssid, "HomeNet");
        assert_eq!(s.mode, "infrastructure");
        assert_eq!(s.auth, "WPA/WPA2-PSK");
        assert_eq!(s.encryption, "AES");
    }

    #[test]
    fn base64_roundtrip() {
        assert_eq!(base64(b"admin:access"), "YWRtaW46YWNjZXNz");
    }
}
