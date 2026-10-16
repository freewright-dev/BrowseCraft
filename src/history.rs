//! Browsing history: every page that finishes loading is recorded here, and
//! `browsecraft://history` renders it as an internal page.
//!
//! The store is global (all tabs share it) and is read from the loader thread
//! when the history page is built, so it lives behind a `Mutex` in a
//! `OnceLock` like the ad filter and flags.

use std::sync::{Mutex, OnceLock};

use crate::net::escape_html;

/// Most recent entries kept; older ones are dropped.
const MAX_ENTRIES: usize = 500;

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
/// instead of adding a duplicate row. Internal-only pages are ignored.
pub fn record(url: &str, title: &str) {
    if url.is_empty() || is_internal(url) {
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
        rows.push_str(&format!(
            "<li><span class=\"n\">{n}</span>\
             <div class=\"meta\"><a href=\"{url}\">{title}</a>\
             <span class=\"u\">{url}</span></div></li>",
            n = entries.len() - i,
            url = escape_html(&entry.url),
            title = escape_html(&title),
        ));
    }
    let body = if rows.is_empty() {
        "<p class=\"empty\">No history yet. Pages you visit will show up here.</p>".to_string()
    } else {
        format!("<ol class=\"h\">{rows}</ol>")
    };
    format!(
        "<!doctype html><title>History</title><style>\
         body {{ margin: 0; background: #f6f7f9; color: #1f2328; \
         font-family: system-ui, sans-serif }}\
         main {{ max-width: 760px; margin: 0 auto; padding: 40px 24px 64px }}\
         h1 {{ font-size: 26px; font-weight: 600; margin: 0 0 4px }}\
         .sub {{ color: #57606a; margin: 0 0 24px }}\
         ol.h {{ list-style: none; margin: 0; padding: 0 }}\
         ol.h li {{ display: flex; gap: 14px; padding: 12px 14px; background: #fff; \
         border: 1px solid #e3e6ea; border-radius: 10px; margin-bottom: 8px }}\
         .n {{ color: #8c959f; font-variant-numeric: tabular-nums; min-width: 32px; \
         text-align: right }}\
         .meta {{ min-width: 0 }}\
         .meta a {{ color: #1f2328; text-decoration: none; font-size: 15px }}\
         .meta a:hover {{ text-decoration: underline }}\
         .u {{ display: block; color: #8c959f; font-size: 12px; word-break: break-all; margin-top: 2px }}\
         .empty {{ color: #57606a }}\
         </style><main><h1>History</h1>\
         <p class=\"sub\">Pages Browsecraft has loaded, newest first.</p>{body}</main>"
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
