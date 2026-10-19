//! Browsing history: every page that finishes loading is recorded here, and
//! `browsecraft://history` renders it as an internal page.
//!
//! The store is global (all tabs share it) and is read from the loader thread
//! when the history page is built, so it lives behind a `Mutex` in a
//! `OnceLock` like the flags.

use std::sync::{Mutex, OnceLock};

use crate::internal;

/// Most recent entries kept; older ones are dropped.
const MAX_ENTRIES: usize = 500;

/// Longer URLs are not recorded: a page could otherwise fill the history
/// with megabyte-long addresses (500 of them), and the history page with
/// twice that.
const MAX_URL_LEN: usize = 8 * 1024;

#[derive(Clone)]
struct Entry {
    url: String,
    title: String,
}

fn store() -> &'static Mutex<Vec<Entry>> {
    static STORE: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Records a visit. Consecutive visits to the same URL update the title
/// instead of adding a duplicate row. Internal-only pages and very long URLs
/// are ignored, and a user name and password in the URL are never kept.
pub fn record(url: &str, title: &str) {
    let clean = crate::net::without_userinfo(url);
    let url = clean.as_deref().unwrap_or(url);
    if url.is_empty() || url.len() > MAX_URL_LEN || is_internal(url) {
        return;
    }
    let Ok(mut entries) = store().lock() else {
        return;
    };
    if let Some(last) = entries.last_mut()
        && last.url == url
    {
        if !title.is_empty() {
            last.title = title.to_string();
        }
        return;
    }
    entries.push(Entry {
        url: url.to_string(),
        title: title.to_string(),
    });
    if entries.len() > MAX_ENTRIES {
        entries.remove(0);
    }
}

/// Pages that shouldn't clutter the history (the history page itself, blank
/// tabs, view-source of another page).
fn is_internal(url: &str) -> bool {
    url.starts_with("browsecraft://") || url == "about:blank" || url.starts_with("view-source:")
}

/// The HTML for the `browsecraft://history` page, newest visits first.
pub fn page_html() -> String {
    let entries = store().lock().map(|e| e.clone()).unwrap_or_default();
    let mut rows = String::new();
    for (i, entry) in entries.iter().rev().enumerate() {
        let title = if entry.title.is_empty() {
            entry.url.clone()
        } else {
            entry.title.clone()
        };
        let letter: String = title
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_else(|| "?".into());
        rows.push_str(&format!(
            "<div class=\"item{first}\"><div class=\"ico\">{letter}</div>\
             <div class=\"main\"><a class=\"name\" href=\"{url}\">{title}</a>\
             <div class=\"url\">{url}</div></div></div>",
            first = if i == 0 { " first" } else { "" },
            letter = internal::escape(&letter),
            url = internal::escape(&entry.url),
            title = internal::escape(&title),
        ));
    }
    let list = if rows.is_empty() {
        "<div class=\"empty\">No history yet. Pages you visit will show up here.</div>".to_string()
    } else {
        rows
    };
    internal::page(
        "History",
        &format!(
            "<h1>History</h1><p class=\"sub\">Pages you have loaded, newest first.</p>\
             <div class=\"card\">{list}</div>"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The store is global, so these tests must not run concurrently.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn records_and_dedups_consecutive() {
        let _guard = SERIAL.lock().unwrap();
        record("https://example.com/", "Example");
        record("https://example.com/", "Example Domain");
        let html = page_html();
        assert!(html.contains("https://example.com/"));
        assert!(html.contains("Example Domain"));
        // Each row shows the URL twice (link + caption), so count the links.
        assert_eq!(html.matches("href=\"https://example.com/\"").count(), 1);
    }

    #[test]
    fn long_urls_and_credentials_are_not_kept() {
        let _guard = SERIAL.lock().unwrap();
        let long = format!("https://long.test/?{}", "x".repeat(MAX_URL_LEN));
        record(&long, "Long");
        record("https://alice:secret@creds.test/", "Creds");
        let html = page_html();
        assert!(!html.contains("long.test"));
        assert!(!html.contains("secret"));
        assert!(html.contains("https://creds.test/"));
    }

    #[test]
    fn internal_pages_are_ignored() {
        let _guard = SERIAL.lock().unwrap();
        record("browsecraft://history", "History");
        record("about:blank", "");
        record("view-source:https://example.com/", "src");
        let html = page_html();
        assert!(!html.contains("browsecraft://history"));
        assert!(!html.contains("view-source:"));
    }

    #[test]
    fn escapes_markup() {
        let _guard = SERIAL.lock().unwrap();
        record("https://x.test/a?b=<q>", "<b>bold</b>");
        let html = page_html();
        assert!(html.contains("&lt;b&gt;bold&lt;/b&gt;"));
    }
}
