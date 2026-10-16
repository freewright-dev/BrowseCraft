//! Fetching resources over http(s), from disk and from `data:` URLs.

use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

use ureq::ResponseExt;
use url::Url;

pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; BrowseCraft/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/pierruno-labs/browsecraft)"
);
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
            let body = read_http_body(resp.body_mut())?;
            Ok(Response {
                url: final_url,
                status,
                content_type,
                body,
            })
        }
        "file" => {
            // On Windows `file://host/share` maps to the UNC path
            // `\\host\share`, and opening it makes Windows authenticate to
            // that host over SMB with the user's credentials.
            if url.host_str().is_some_and(|h| !h.is_empty()) {
                return Err("file URLs on other computers are not supported".into());
            }
            let path = url
                .to_file_path()
                .map_err(|_| "invalid file path".to_string())?;
            let describe = |e: std::io::Error| format!("{}: {e}", path.display());
            let meta = std::fs::metadata(&path).map_err(describe)?;
            if meta.is_dir() {
                return Ok(Response {
                    url: url.clone(),
                    status: 200,
                    content_type: "text/html".into(),
                    body: directory_listing(&path, url).into_bytes(),
                });
            }
            // Devices such as `CON` or named pipes can block forever.
            if !meta.is_file() {
                return Err(format!("{}: not a regular file", path.display()));
            }
            let mut body = Vec::new();
            std::fs::File::open(&path)
                .and_then(|f| f.take(MAX_BODY + 1).read_to_end(&mut body))
                .map_err(describe)?;
            if body.len() as u64 > MAX_BODY {
                return Err("the file is too large".into());
            }
            let content_type = mime_from_extension(&path.to_string_lossy()).to_string();
            Ok(Response {
                url: url.clone(),
                status: 200,
                content_type,
                body,
            })
        }
        "data" => {
            let (content_type, body) = parse_data_url(url.as_str()).ok_or("malformed data: URL")?;
            Ok(Response {
                url: url.clone(),
                status: 200,
                content_type,
                body,
            })
        }
        scheme => Err(format!("the {scheme}: scheme is not supported")),
    }
}

fn read_http_body(body: &mut ureq::Body) -> Result<Vec<u8>, String> {
    // ureq's limit applies BEFORE content decoding. Keep a wire-size cap,
    // but also bound the decoded stream to prevent gzip decompression bombs.
    // Read one extra byte to distinguish an exact-limit body from truncation.
    let mut decoded = Vec::new();
    body.with_config()
        .limit(MAX_BODY + 1)
        .reader()
        .take(MAX_BODY + 1)
        .read_to_end(&mut decoded)
        .map_err(|e| describe_error(e.into()))?;
    if decoded.len() as u64 > MAX_BODY {
        return Err("the response is too large".into());
    }
    Ok(decoded)
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
                .map(|e| {
                    (
                        e.path().is_dir(),
                        e.file_name().to_string_lossy().into_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    let title = escape_html(&path.display().to_string());
    let mut html = format!("<title>{title}</title><h1>Index of {title}</h1><ul>");
    if url.path() != "/" && path.parent().is_some() {
        html.push_str("<li><a href=\"..\">..</a></li>");
    }
    let base = if url.path().ends_with('/') {
        String::new()
    } else {
        format!("{}/", url.path().rsplit('/').next().unwrap_or(""))
    };
    for (dir, name) in entries {
        let slash = if dir { "/" } else { "" };
        let href = format!("{base}{}{slash}", percent_encode_path(&name));
        html.push_str(&format!(
            "<li><a href=\"{}\">{}{slash}</a></li>",
            escape_html(&href),
            escape_html(&name)
        ));
    }
    html.push_str("</ul>");
    html
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Decodes `data:[<mime>][;base64],<payload>`.
pub fn parse_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let base64 = meta.ends_with(";base64");
    let mime = meta.trim_end_matches(";base64");
    let mime = if mime.is_empty() { "text/plain" } else { mime };
    let bytes = percent_decode(payload);
    let body = if base64 {
        decode_base64(&bytes)?
    } else {
        bytes
    };
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
    let header = content_type.to_ascii_lowercase().split(';').find_map(|p| {
        p.trim()
            .strip_prefix("charset=")
            .map(|c| c.trim_matches('"').to_string())
    });
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
        "iso-8859-1" | "latin1" | "windows-1252" | "cp1252" | "us-ascii" | "ascii"
        | "iso8859-1" | "l1" => {
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
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
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
    fn data_urls_decode_a_trailing_escape() {
        assert_eq!(parse_data_url("data:,a%41").unwrap().1, b"aA");
        assert_eq!(parse_data_url("data:,%4").unwrap().1, b"%4");
        assert_eq!(parse_data_url("data:,%").unwrap().1, b"%");
    }
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn file_urls_stay_on_this_computer() {
        for remote in [
            "file://attacker.example/share/x.png",
            "file://192.0.2.1/share/",
            "file://./pipe/x",
        ] {
            let err = fetch(&Url::parse(remote).unwrap(), None, "*/*").err();
            assert_eq!(
                err.as_deref(),
                Some("file URLs on other computers are not supported"),
                "{remote}"
            );
        }

        let dir = std::env::temp_dir().join(format!("browsecraft-net-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("page.html");
        std::fs::write(&file, "<p>hi").unwrap();
        let url = Url::from_file_path(&file).unwrap();
        assert_eq!(fetch(&url, None, "*/*").unwrap().body, b"<p>hi");
        // `localhost` means this computer.
        let local = Url::parse(&url.as_str().replacen("file://", "file://localhost", 1)).unwrap();
        assert_eq!(fetch(&local, None, "*/*").unwrap().body, b"<p>hi");
        let listing = fetch(&Url::from_directory_path(&dir).unwrap(), None, "*/*").unwrap();
        assert!(String::from_utf8_lossy(&listing.body).contains("page.html"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn file_urls_refuse_devices() {
        let err = fetch(&Url::parse("file:///C:/CON").unwrap(), None, "*/*").err();
        assert!(err.is_some_and(|e| !e.contains("too large")));
    }

    /// Exercise the real HTTP/content-decoding path without an external server.
    fn fetch_test_body(body: Vec<u8>, gzip: bool) -> Result<Response, String> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(&stream);
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            let encoding = if gzip { "Content-Encoding: gzip\r\n" } else { "" };
            write!(stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{encoding}Connection: close\r\n\r\n",
                body.len()).unwrap();
            // An oversized response may be rejected before the server finishes.
            let _ = stream.write_all(&body);
        });
        let result = fetch(&url, None, "*/*");
        server.join().unwrap();
        result
    }

    #[test]
    fn http_decoded_body_limit() {
        for gzip in [false, true] {
            for len in [0, 5, MAX_BODY, MAX_BODY + 1] {
                let body = if gzip {
                    let mut encoder = flate2::write::GzEncoder::new(
                        Vec::new(), flate2::Compression::fast());
                    std::io::copy(&mut std::io::repeat(b'x').take(len), &mut encoder).unwrap();
                    let compressed = encoder.finish().unwrap();
                    assert!((compressed.len() as u64) < MAX_BODY);
                    compressed
                } else {
                    vec![b'x'; len as usize]
                };
                let result = fetch_test_body(body, gzip);
                if len > MAX_BODY {
                    assert_eq!(result.err().as_deref(), Some("the response is too large"),
                        "gzip={gzip}, length={len}");
                } else {
                    let response = result.unwrap_or_else(|e| panic!("gzip={gzip}, length={len}: {e}"));
                    assert_eq!(response.body.len() as u64, len);
                    assert!(response.body.iter().all(|&b| b == b'x'));
                }
            }
        }
    }

    #[test]
    fn malformed_gzip_is_rejected() {
        assert!(fetch_test_body(b"not a gzip stream".to_vec(), true).is_err());
    }

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
        assert_eq!(
            decode_text(b"caf\xe9", "text/html; charset=ISO-8859-1"),
            "café"
        );
        assert_eq!(decode_text("café".as_bytes(), "text/html"), "café");
        assert_eq!(
            decode_text(b"<meta charset=windows-1252>\x93x\x94", ""),
            "<meta charset=windows-1252>“x”"
        );
    }
}
