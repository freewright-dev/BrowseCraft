//! Fetching resources over http(s), from disk and from `data:` URLs.

use std::sync::OnceLock;
use std::time::Duration;

use ureq::ResponseExt;
use url::Url;

pub const USER_AGENT: &str =
    concat!("Mozilla/5.0 (compatible; Prowl/", env!("CARGO_PKG_VERSION"), "; +https://github.com/pierruno-labs/prowl)");
const MAX_BODY: u64 = 32 * 1024 * 1024;

pub struct Response {
    /// The URL after redirects.
    pub url: Url,
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(USER_AGENT)
            .max_redirects(10)
            .build()
            .into()
    })
}

/// Fetches `url`, sending `form` as an urlencoded POST body when given.
pub fn fetch(url: &Url, form: Option<&str>, accept: &str) -> Result<Response, String> {
    match url.scheme() {
        "http" | "https" => {
            let result = match form {
                Some(body) => agent()
                    .post(url.as_str())
                    .header("Accept", accept)
                    .content_type("application/x-www-form-urlencoded")
                    .send(body.as_bytes()),
                None => agent().get(url.as_str()).header("Accept", accept).call(),
            };
            let mut resp = result.map_err(describe_error)?;
            let final_url = Url::parse(&resp.get_uri().to_string()).unwrap_or_else(|_| url.clone());
            let status = resp.status().as_u16();
            let content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let body = resp
                .body_mut()
                .with_config()
                .limit(MAX_BODY)
                .read_to_vec()
                .map_err(describe_error)?;
            Ok(Response { url: final_url, status, content_type, body })
        }
        "file" => {
            let path = url.to_file_path().map_err(|_| "invalid file path".to_string())?;
            if path.is_dir() {
                return Ok(Response {
                    url: url.clone(),
                    status: 200,
                    content_type: "text/html".into(),
                    body: directory_listing(&path, url).into_bytes(),
                });
            }
            let body = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let content_type = mime_from_extension(&path.to_string_lossy()).to_string();
            Ok(Response { url: url.clone(), status: 200, content_type, body })
        }
        "data" => {
            let (content_type, body) = parse_data_url(url.as_str()).ok_or("malformed data: URL")?;
            Ok(Response { url: url.clone(), status: 200, content_type, body })
        }
        scheme => Err(format!("the {scheme}: scheme is not supported")),
    }
}

fn describe_error(err: ureq::Error) -> String {
    match err {
        ureq::Error::HostNotFound => "the server's address could not be found".into(),
        ureq::Error::Timeout(_) => "the connection timed out".into(),
        ureq::Error::Io(e) => format!("connection failed: {e}"),
        ureq::Error::BodyExceedsLimit(_) => "the response is too large".into(),
        other => other.to_string(),
    }
}

fn mime_from_extension(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" | "xhtml" => "text/html",
        "css" => "text/css",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "zip" | "exe" | "gz" | "7z" => "application/octet-stream",
        _ => "text/plain",
    }
}

fn directory_listing(path: &std::path::Path, url: &Url) -> String {
    let mut entries: Vec<(bool, String)> = std::fs::read_dir(path)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| (e.path().is_dir(), e.file_name().to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let title = escape_html(&path.display().to_string());
    let mut html = format!("<title>{title}</title><h1>Index of {title}</h1><ul>");
    if url.path() != "/" && path.parent().is_some() {
        html.push_str("<li><a href=\"..\">..</a></li>");
    }
    let base = if url.path().ends_with('/') { String::new() } else { format!("{}/", url.path().rsplit('/').next().unwrap_or("")) };
    for (dir, name) in entries {
        let slash = if dir { "/" } else { "" };
        let href = format!("{base}{}{slash}", percent_encode_path(&name));
        html.push_str(&format!("<li><a href=\"{}\">{}{slash}</a></li>", escape_html(&href), escape_html(&name)));
    }
    html.push_str("</ul>");
    html
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Decodes `data:[<mime>][;base64],<payload>`.
pub fn parse_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let base64 = meta.ends_with(";base64");
    let mime = meta.trim_end_matches(";base64");
    let mime = if mime.is_empty() { "text/plain" } else { mime };
    let bytes = percent_decode(payload);
    let body = if base64 { decode_base64(&bytes)? } else { bytes };
    Some((mime.to_string(), body))
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in input {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | value(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Decodes a text body using the charset from the Content-Type header or a
/// `<meta charset>` in the first bytes. Supports UTF-8 and Windows-1252
/// (which covers ISO-8859-1 and ASCII).
pub fn decode_text(body: &[u8], content_type: &str) -> String {
    let body = body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body);
    let header = content_type
        .to_ascii_lowercase()
        .split(';')
        .find_map(|p| p.trim().strip_prefix("charset=").map(|c| c.trim_matches('"').to_string()));
    let sniffed = || {
        let head = String::from_utf8_lossy(&body[..body.len().min(4096)]).to_ascii_lowercase();
        let i = head.find("charset=")? + 8;
        let label: String = head[i..]
            .trim_start_matches(['"', '\''])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        Some(label)
    };
    let label = header.or_else(sniffed).unwrap_or_default();
    match label.as_str() {
        "iso-8859-1" | "latin1" | "windows-1252" | "cp1252" | "us-ascii" | "ascii" | "iso8859-1" | "l1" => {
            if std::str::from_utf8(body).is_ok() && label.contains("ascii") {
                return String::from_utf8_lossy(body).into_owned();
            }
            body.iter().map(|&b| windows_1252(b)).collect()
        }
        _ => String::from_utf8_lossy(body).into_owned(),
    }
}

fn windows_1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9f => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_urls() {
        let (mime, body) = parse_data_url("data:text/html,%3Cb%3Ehi%3C/b%3E").unwrap();
        assert_eq!(mime, "text/html");
        assert_eq!(body, b"<b>hi</b>");
        let (mime, body) = parse_data_url("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(body, b"hello");
    }

    #[test]
    fn charsets() {
        assert_eq!(decode_text(b"caf\xe9", "text/html; charset=ISO-8859-1"), "café");
        assert_eq!(decode_text("café".as_bytes(), "text/html"), "café");
        assert_eq!(decode_text(b"<meta charset=windows-1252>\x93x\x94", ""), "<meta charset=windows-1252>“x”");
    }
}
