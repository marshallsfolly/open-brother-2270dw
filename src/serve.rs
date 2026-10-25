//! A minimal HTTP status/print server.
//!
//! Endpoints:
//!
//! * `GET  /`        — a small status page with an upload form
//! * `GET  /health`  — `ok`
//! * `GET  /status`  — JSON status
//! * `GET  /metrics` — Prometheus text exposition
//! * `POST /print`   — print a file (multipart upload, or raw body with a
//!   `?name=<filename>` query parameter; a bare body is treated as text)

use tiny_http::{Header, Request, Response, StatusCode};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::monitor;
use crate::printer::Printer;

/// Run the HTTP server until interrupted.
pub fn run(printer: &Printer, cfg: &Config, listen: &str) -> Result<()> {
    let server = tiny_http::Server::http(listen)
        .map_err(|e| Error::Printer(format!("bind {listen}: {e}")))?;
    eprintln!("serving on http://{listen}");

    for request in server.incoming_requests() {
        let method = request.method().as_str().to_string();
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("/").to_string();

        if let Err(e) = route(printer, cfg, &method, &path, &url, request) {
            eprintln!("request error: {e}");
        }
    }
    Ok(())
}

fn route(
    printer: &Printer,
    cfg: &Config,
    method: &str,
    path: &str,
    url: &str,
    req: Request,
) -> Result<()> {
    match (method, path) {
        (_, "/") | (_, "/index.html") => {
            let html = html_page(printer)?;
            respond(req, StatusCode(200), "text/html", &html)
        }
        (_, "/health") => respond(req, StatusCode(200), "text/plain", "ok"),
        (_, "/status") => {
            let status = printer.status()?;
            respond(req, StatusCode(200), "application/json", &serde_json::to_string_pretty(&status)?)
        }
        (_, "/metrics") => {
            let status = printer.status()?;
            respond(req, StatusCode(200), "text/plain", &monitor::prometheus_metrics(&status))
        }
        ("POST", "/print") => handle_print(printer, cfg, url, req),
        _ => respond(req, StatusCode(404), "text/plain", "not found"),
    }
}

fn handle_print(printer: &Printer, cfg: &Config, url: &str, mut req: Request) -> Result<()> {
    let mut body = Vec::new();
    req.as_reader().read_to_end(&mut body)?;
    let content_type = header_value(req.headers(), "content-type").unwrap_or_default();

    if let Some((filename, content)) = parse_multipart(&body, &content_type) {
        print_bytes(printer, cfg, &filename, &content)?;
    } else if let Some(name) = query_param(url, "name") {
        print_bytes(printer, cfg, &name, &body)?;
    } else {
        let opts = cfg.job_options();
        let style = cfg.text_style();
        printer.print_text(&String::from_utf8_lossy(&body), &opts, &style)?;
    }
    respond(req, StatusCode(200), "text/plain", "printed")
}

fn print_bytes(printer: &Printer, cfg: &Config, name: &str, bytes: &[u8]) -> Result<()> {
    let dir = std::env::temp_dir().join(format!(
        "brother-2270dw-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    let result = (|| -> Result<()> {
        std::fs::write(&path, bytes)?;
        let opts = cfg.job_options();
        let style = cfg.text_style();
        printer.print_file(&path, &opts, &style)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn html_page(printer: &Printer) -> Result<String> {
    let status = printer.status();
    let status_html = match &status {
        Ok(s) => format!("<pre>state: {}\npages: {:?}\n</pre>", s.state.label(), s.page_count),
        Err(e) => format!("<pre>status unavailable: {e}</pre>"),
    };
    Ok(format!(
        "<!doctype html><html><head><title>Brother HL-2270DW</title></head><body>\
         <h1>Brother HL-2270DW</h1>{status_html}\
         <form action=\"/print\" method=\"post\" enctype=\"multipart/form-data\">\
         <input type=\"file\" name=\"file\"> <button type=\"submit\">Print file</button></form>\
         <p><a href=\"/status\">status (JSON)</a> · <a href=\"/metrics\">metrics</a></p>\
         <p><code>curl -F file=@doc.pdf http://&lt;host&gt;/print</code></p>\
         </body></html>"
    ))
}

fn respond(req: Request, code: StatusCode, content_type: &str, body: &str) -> Result<()> {
    let header = Header::from_bytes("Content-Type", content_type.as_bytes())
        .map_err(|_| Error::Printer("invalid Content-Type".into()))?;
    let response = Response::from_string(body.to_string())
        .with_status_code(code)
        .with_header(header);
    req.respond(response)?;
    Ok(())
}

fn header_value(headers: &[Header], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split('?').nth(1)?;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(percent_decode(v));
            }
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Minimal single-file `multipart/form-data` parser. Returns
/// `(filename, content)`.
fn parse_multipart(body: &[u8], content_type: &str) -> Option<(String, Vec<u8>)> {
    let boundary = content_type.split("boundary=").nth(1)?;
    let boundary = boundary.trim().trim_matches('"');
    let marker = format!("--{boundary}");
    let marker_bytes = marker.as_bytes();

    let start = find_subsequence(body, marker_bytes)? + marker_bytes.len();
    // Skip optional CRLF, then read headers until the blank line.
    let headers_start = start + if body[start..].starts_with(b"\r\n") { 2 } else { 0 };
    let header_end = find_subsequence(&body[headers_start..], b"\r\n\r\n")? + headers_start + 4;
    let headers = String::from_utf8_lossy(&body[headers_start..header_end.saturating_sub(4)]);
    let filename = headers.split("filename=\"").nth(1)?.split('"').next()?.to_string();

    let content_start = header_end;
    let content_end = find_subsequence(&body[content_start..], marker_bytes)? + content_start;
    let mut content = body[content_start..content_end].to_vec();
    // Strip the CRLF that precedes the closing boundary.
    while content.ends_with(b"\r\n") {
        content.truncate(content.len() - 2);
    }
    if filename.is_empty() {
        return None;
    }
    Some((filename, content))
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_multipart() {
        let boundary = "----XYZ";
        let body = "------XYZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n------XYZ--\r\n";
        let ct = format!("multipart/form-data; boundary={boundary}");
        let (name, content) = parse_multipart(body.as_bytes(), &ct).unwrap();
        assert_eq!(name, "a.txt");
        assert_eq!(content, b"hello");
    }

    #[test]
    fn query_param_decode() {
        assert_eq!(query_param("/print?name=a%20b.txt", "name").as_deref(), Some("a b.txt"));
        assert_eq!(query_param("/print", "name"), None);
    }
}
