//! Turns whatever the user typed into the address bar into a URL.

/// Startpage's search endpoint (the browser's default search engine).
const SEARCH_URL: &str = "https://www.startpage.com/sp/search?query=";

/// Resolves address-bar input to a URL: full URLs pass through, bare hosts
/// get a scheme, and anything else becomes a web search.
pub fn resolve(input: &str) -> String {
    let input = input.trim();

    if has_scheme(input) {
        return input.to_string();
    }
    if !input.contains(char::is_whitespace) {
        let host = input.split(['/', '?', '#']).next().unwrap_or_default();
        let hostname = host.rsplit_once(':').map_or(host, |(name, port)| {
            if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
                name
            } else {
                host
            }
        });

        if hostname == "localhost" || is_ipv4(hostname) {
            return format!("http://{input}");
        }
        if looks_like_domain(hostname) {
            return format!("https://{input}");
        }
    }

    format!("{SEARCH_URL}{}", encode_query(input))
}

fn has_scheme(input: &str) -> bool {
    // Checked first: `view-source:https://…` also contains `://`, but
    // `view-source:https` is not a scheme.
    if ["about:", "data:", "file:", "view-source:"]
        .iter()
        .any(|prefix| input.to_ascii_lowercase().starts_with(prefix))
    {
        return true;
    }
    if input.contains("://") {
        return input.split("://").next().is_some_and(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
        });
    }
    false
}

fn is_ipv4(host: &str) -> bool {
    let parts: Vec<_> = host.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok())
}

fn looks_like_domain(host: &str) -> bool {
    let Some((name, tld)) = host.rsplit_once('.') else {
        return false;
    };
    !name.is_empty()
        && tld.len() >= 2
        && tld.chars().all(char::is_alphabetic)
        && host.split('.').all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
}

fn encode_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for byte in query.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn keeps_full_urls() {
        assert_eq!(
            resolve("https://example.com/a?b=c"),
            "https://example.com/a?b=c"
        );
        assert_eq!(resolve("  http://example.com  "), "http://example.com");
        assert_eq!(resolve("about:blank"), "about:blank");
        assert_eq!(resolve("file:///C:/index.html"), "file:///C:/index.html");
        assert_eq!(
            resolve("view-source:https://example.com/"),
            "view-source:https://example.com/"
        );
        assert_eq!(
            resolve("about:blocked:https://bank.example/"),
            "about:blocked:https://bank.example/"
        );
    }

    #[test]
    fn adds_https_to_domains() {
        assert_eq!(resolve("example.com"), "https://example.com");
        assert_eq!(resolve("docs.rs/wry"), "https://docs.rs/wry");
        assert_eq!(resolve("example.com:8443/x"), "https://example.com:8443/x");
    }

    #[test]
    fn adds_http_to_local_hosts() {
        assert_eq!(resolve("localhost:3000"), "http://localhost:3000");
        assert_eq!(resolve("192.168.1.1"), "http://192.168.1.1");
    }

    #[test]
    fn searches_everything_else() {
        assert_eq!(
            resolve("rust webview"),
            "https://www.startpage.com/sp/search?query=rust+webview"
        );
        assert_eq!(
            resolve("c++"),
            "https://www.startpage.com/sp/search?query=c%2B%2B"
        );
        assert_eq!(
            resolve("hello"),
            "https://www.startpage.com/sp/search?query=hello"
        );
        assert_eq!(
            resolve("1.5"),
            "https://www.startpage.com/sp/search?query=1.5"
        );
        assert_eq!(
            resolve("what is example.com"),
            "https://www.startpage.com/sp/search?query=what+is+example.com"
        );
    }
}
