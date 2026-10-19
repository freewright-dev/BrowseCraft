//! Bookmarks: a global, persisted list shared by every tab.
//!
//! Stored in `bookmarks.txt` next to `flags.txt` in the per-user config
//! folder, as one `url<TAB>title` line per bookmark, in the order the user
//! added them. The bookmarks bar, the
//! star button, the new-tab page and the `browsecraft://bookmarks` page all
//! read this store.

use std::sync::RwLock;

use crate::internal::{self, FormToken};

const FILE_NAME: &str = "bookmarks.txt";
const MAX_TITLE_CHARS: usize = 120;
const MAX_BOOKMARKS: usize = 2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
}

static STORE: RwLock<Vec<Bookmark>> = RwLock::new(Vec::new());
static FORM_TOKEN: FormToken = FormToken::new();

fn file_path() -> Result<std::path::PathBuf, String> {
    crate::storage::config_file(FILE_NAME)
}

/// Only web pages are bookmarkable.
pub fn bookmarkable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The URL without a fragment, so `page#top` and `page` are one bookmark.
fn key(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

fn clean_title(title: &str, url: &str) -> String {
    let collapsed: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let title = if collapsed.is_empty() {
        host_of(url)
    } else {
        collapsed
    };
    title.chars().take(MAX_TITLE_CHARS).collect()
}

/// `example.com` for `https://www.example.com/a`.
pub fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
        .unwrap_or_else(|| url.to_string())
}

fn parse(text: &str) -> Vec<Bookmark> {
    let mut out: Vec<Bookmark> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        if out.len() == MAX_BOOKMARKS {
            break;
        }
        let Some((url, title)) = line.split_once('\t') else {
            continue;
        };
        if bookmarkable(url) && seen.insert(key(url)) {
            out.push(Bookmark {
                title: clean_title(title, url),
                url: url.to_string(),
            });
        }
    }
    out
}

fn serialize(list: &[Bookmark]) -> String {
    let mut text = String::new();
    for b in list {
        text.push_str(&b.url);
        text.push('\t');
        text.push_str(&b.title);
        text.push('\n');
    }
    text
}

fn save(list: &[Bookmark]) {
    if let Ok(path) = file_path() {
        let _ = crate::storage::write_private(&path, serialize(list).as_bytes());
    }
}

/// Reads `bookmarks.txt`. Call once at startup.
pub fn load() {
    if let Ok(text) = file_path().and_then(|path| {
        crate::storage::read_text_limited(&path, crate::storage::MAX_SETTINGS_FILE).map_err(|e| e.to_string())
    }) {
        *STORE.write().unwrap_or_else(|p| p.into_inner()) = parse(&text);
    }
}

pub fn all() -> Vec<Bookmark> {
    STORE.read().unwrap_or_else(|p| p.into_inner()).clone()
}

pub fn contains(url: &str) -> bool {
    STORE
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .any(|b| key(&b.url) == key(url))
}

/// Adds a bookmark. Returns `false` for a non-web URL, a duplicate or a full
/// list.
pub fn add(title: &str, url: &str) -> bool {
    if !bookmarkable(url) {
        return false;
    }
    let mut list = STORE.write().unwrap_or_else(|p| p.into_inner());
    if list.len() >= MAX_BOOKMARKS || list.iter().any(|b| key(&b.url) == key(url)) {
        return false;
    }
    list.push(Bookmark {
        title: clean_title(title, url),
        url: url.to_string(),
    });
    save(&list);
    true
}

/// Removes the bookmark for `url`. Returns whether there was one.
pub fn remove(url: &str) -> bool {
    let mut list = STORE.write().unwrap_or_else(|p| p.into_inner());
    let before = list.len();
    list.retain(|b| key(&b.url) != key(url));
    let removed = list.len() != before;
    if removed {
        save(&list);
    }
    removed
}

/// Adds the page if it is not bookmarked, removes it if it is. Returns
/// `true` when it is bookmarked afterwards.
pub fn toggle(title: &str, url: &str) -> bool {
    if contains(url) {
        remove(url);
        false
    } else {
        add(title, url)
    }
}

/// Applies a submission of the bookmarks page (`remove=<url>`). Only the
/// page's own, freshly rendered links carry the token.
pub fn apply_form(query: &str) -> bool {
    let Some(pairs) = FORM_TOKEN.take(query) else {
        return false;
    };
    let mut changed = false;
    for (k, v) in pairs {
        if k == "remove" {
            changed |= remove(&v);
        }
    }
    changed
}

/// The `browsecraft://bookmarks` page. Each call issues a new form token.
pub fn page_html() -> String {
    let token = match FORM_TOKEN.issue() {
        Ok(token) => token,
        Err(error) => return internal::unavailable_page("Bookmarks", &error),
    };
    let list = all();
    let mut body = String::from(
        "<h1>Bookmarks</h1><p class=\"sub\">Pages you saved. Star a page in the address bar, \
         or press Ctrl + D, to add it.</p><div class=\"card\">",
    );
    if list.is_empty() {
        body.push_str("<div class=\"empty\">No bookmarks yet.</div>");
    }
    for (i, b) in list.iter().enumerate() {
        let letter: String = b
            .title
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_else(|| "?".into());
        body.push_str(&format!(
            "<div class=\"item{first}\"><div class=\"ico\">{letter}</div>\
             <div class=\"main\"><a class=\"name\" href=\"{url}\">{title}</a><div class=\"url\">{url}</div></div>\
             <a class=\"btn ghost\" href=\"browsecraft://bookmarks?token={token}&amp;remove={enc}\">Remove</a></div>",
            first = if i == 0 { " first" } else { "" },
            letter = internal::escape(&letter),
            url = internal::escape(&b.url),
            title = internal::escape(&b.title),
            token = token,
            enc = internal::escape(&internal::encode(&b.url)),
        ));
    }
    body.push_str("</div>");
    internal::page("Bookmarks", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store is a process global and persists to a file.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset() {
        *STORE.write().unwrap() = Vec::new();
    }

    #[test]
    fn add_remove_toggle_and_dedup() {
        let _g = SERIAL.lock().unwrap();
        reset();
        assert!(add("Example", "https://example.com/"));
        assert!(!add("Again", "https://example.com/#top"), "fragment is the same page");
        assert!(contains("https://example.com/#x"));
        assert!(!toggle("Example", "https://example.com/"));
        assert!(!contains("https://example.com/"));
        assert!(toggle("", "https://www.rust-lang.org/learn"));
        assert_eq!(all()[0].title, "rust-lang.org", "empty titles fall back to the host");
        assert!(remove("https://www.rust-lang.org/learn"));
        assert!(!remove("https://www.rust-lang.org/learn"));
    }

    #[test]
    fn parsing_dedups_and_stops_at_the_cap() {
        // A crafted file of a million lines parses in linear time and keeps
        // at most MAX_BOOKMARKS entries (audit B-25).
        let text: String = (0..1_000_000).map(|i| format!("https://b{}.test/\tt\n", i % 500_000)).collect();
        let start = std::time::Instant::now();
        let list = parse(&text);
        assert_eq!(list.len(), MAX_BOOKMARKS);
        assert_eq!(list[1].url, "https://b1.test/");
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
        let dup = parse("https://a.test/\tA\nhttps://a.test/#x\tB\nhttps://c.test/\tC\n");
        assert_eq!(dup.iter().map(|b| b.title.as_str()).collect::<Vec<_>>(), ["A", "C"]);
    }

    #[test]
    fn only_web_pages_can_be_bookmarked() {
        let _g = SERIAL.lock().unwrap();
        reset();
        for url in ["about:home", "browsecraft://flags", "javascript:alert(1)", "file:///etc/passwd"] {
            assert!(!add("x", url), "{url}");
        }
        assert!(all().is_empty());
    }

    #[test]
    fn titles_are_cleaned() {
        assert_eq!(clean_title("  a\tb\nc  ", "https://x.test/"), "a b c");
        assert_eq!(clean_title(&"x".repeat(500), "https://x.test/").chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn file_roundtrip_ignores_junk_lines() {
        let list = parse("https://a.test/\tA\nnot a bookmark\njavascript:x\tbad\nhttps://a.test/#f\tdup\nhttps://b.test/\tB\n");
        assert_eq!(
            list,
            vec![
                Bookmark { title: "A".into(), url: "https://a.test/".into() },
                Bookmark { title: "B".into(), url: "https://b.test/".into() },
            ]
        );
        assert_eq!(parse(&serialize(&list)), list);
    }

    #[test]
    fn persists_to_disk() {
        let _g = SERIAL.lock().unwrap();
        reset();
        assert!(add("Disk", "https://disk.test/"));
        reset();
        load();
        assert!(contains("https://disk.test/"));
        remove("https://disk.test/");
        let _ = std::fs::remove_file(file_path().unwrap());
    }

    #[test]
    fn only_the_rendered_page_can_remove() {
        let _g = SERIAL.lock().unwrap();
        reset();
        assert!(add("Keep", "https://keep.test/?a=1&b=2"));
        let enc = internal::encode("https://keep.test/?a=1&b=2");
        assert!(!apply_form(&format!("remove={enc}")), "no token");
        let html = page_html();
        let marker = "token=";
        let start = html.find(marker).unwrap() + marker.len();
        let token = html[start..start + 32].to_string();
        assert!(!apply_form(&format!("token={}&remove={enc}", "0".repeat(32))));
        assert!(contains("https://keep.test/?a=1&b=2"));
        assert!(apply_form(&format!("token={token}&remove={enc}")));
        assert!(!contains("https://keep.test/?a=1&b=2"));
        assert!(!apply_form(&format!("token={token}&remove={enc}")), "token used up");
        let _ = std::fs::remove_file(file_path().unwrap());
    }

    #[test]
    fn page_escapes_markup() {
        let _g = SERIAL.lock().unwrap();
        reset();
        assert!(add("<script>x</script>", "https://x.test/?q=\"a\""));
        let html = page_html();
        assert!(!html.contains("<script>x"));
        assert!(html.contains("&lt;script&gt;"));
        reset();
        let _ = std::fs::remove_file(file_path().unwrap());
    }
}
