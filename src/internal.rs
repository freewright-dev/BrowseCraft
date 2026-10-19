//! The shared look of the internal pages (settings, bookmarks, history,
//! passwords, the new-tab page and error pages), plus the small helpers they
//! have in common: HTML escaping and one-time form tokens.
//!
//! The pages are drawn by BrowseCraft's own engine, which does not know the
//! `prefers-color-scheme` media feature for the browser's dark mode, so each
//! page is generated for the current theme (`set_dark` is called whenever the
//! chrome theme changes) and reloaded when it flips.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

static DARK: AtomicBool = AtomicBool::new(true);

/// Records the effective theme. Returns `true` when it changed.
pub fn set_dark(dark: bool) -> bool {
    DARK.swap(dark, Ordering::SeqCst) != dark
}

pub fn dark() -> bool {
    DARK.load(Ordering::SeqCst)
}

/// Escapes text for HTML element content and double-quoted attributes.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// The palette of the internal pages as `(token, light, dark)`.
const PALETTE: &[(&str, &str, &str)] = &[
    ("@bg@", "#f5f6f8", "#14161a"),
    ("@surface@", "#ffffff", "#1d2026"),
    ("@surface2@", "#eef0f3", "#262a32"),
    ("@line@", "#dde1e7", "#2c313a"),
    ("@text@", "#1c1f24", "#e8eaee"),
    ("@muted@", "#667085", "#8b93a1"),
    ("@accent@", "#2f6fde", "#5b8def"),
    ("@off@", "#b8bec9", "#3a404b"),
    ("@danger@", "#d92d20", "#f0584d"),
];

const BASE_CSS: &str = r#"
body { margin: 0; background: @bg@; color: @text@; font-family: system-ui, "Segoe UI", sans-serif; font-size: 14px; line-height: 1.5 }
main { max-width: 720px; margin: 0 auto; padding: 44px 24px 72px }
h1 { font-size: 28px; font-weight: 700; margin: 0 0 4px }
.sub { color: @muted@; margin: 0 0 28px }
a { color: @accent@ }
.card { background: @surface@; border: 1px solid @line@; border-radius: 14px; margin: 0 0 18px; padding: 4px 20px 6px }
.card h2 { font-size: 12px; color: @muted@; margin: 16px 0 6px; font-weight: 600; text-transform: uppercase; letter-spacing: 1px }
.row { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 14px 0; border-top: 1px solid @line@ }
.row.first { border-top: 0 }
.row .grow { flex: 1 }
.t { font-weight: 600; font-size: 14px }
.d { color: @muted@; font-size: 13px; margin-top: 1px }
.seg { display: flex; gap: 2px; background: @bg@; border: 1px solid @line@; border-radius: 10px; padding: 3px }
.seg a { display: block; padding: 6px 14px; border-radius: 7px; color: @muted@; text-decoration: none; font-size: 13px }
.seg a.on { background: @accent@; color: #ffffff; font-weight: 600 }
.sw { display: flex; width: 46px; height: 26px; border-radius: 13px; padding: 3px; background: @off@; justify-content: flex-start; text-decoration: none }
.sw.on { background: @accent@; justify-content: flex-end }
.sw span { display: block; width: 20px; height: 20px; border-radius: 10px; background: #ffffff }
.btn { display: inline-block; padding: 8px 16px; border: 0; border-radius: 9px; background: @accent@; color: #ffffff; text-decoration: none; font-size: 13px; font-weight: 600 }
.btn.ghost { background: @surface2@; color: @text@ }
.btn.danger { background: @danger@; color: #ffffff }
.item { display: flex; align-items: center; gap: 14px; padding: 12px 0; border-top: 1px solid @line@ }
.item.first { border-top: 0 }
.item .ico { width: 34px; height: 34px; border-radius: 9px; background: @surface2@; color: @accent@; text-align: center; font-weight: 700; line-height: 34px }
.item .main { flex: 1 }
.item .main a.name { color: @text@; text-decoration: none; font-size: 15px; font-weight: 600 }
.item .url { color: @muted@; font-size: 12px; word-break: break-all }
.empty { color: @muted@; text-align: center; padding: 40px 0 36px }
.field { display: block; box-sizing: border-box; width: 100%; padding: 9px 12px; border: 1px solid @line@; border-radius: 9px; background: @bg@; color: @text@; font-size: 14px; margin: 4px 0 12px }
label.l { display: block; color: @muted@; font-size: 12px; font-weight: 600; margin-top: 8px }
.note { color: @muted@; font-size: 13px; padding: 0 0 14px }
.ok { color: #12a150 }
.bad { color: @danger@ }
"#;

/// The shared stylesheet for the current theme.
pub fn css() -> String {
    css_for(dark())
}

pub fn css_for(dark: bool) -> String {
    themed(BASE_CSS, dark)
}

/// Replaces the `@token@` colours in `text` with the palette of one theme.
pub fn themed(text: &str, dark: bool) -> String {
    let mut out = text.to_string();
    for (token, light, dark_value) in PALETTE {
        out = out.replace(token, if dark { dark_value } else { light });
    }
    out
}

/// A complete internal page: title, the shared stylesheet and `body`.
pub fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><title>{}</title><style>{}</style><main>{}</main>",
        escape(title),
        css(),
        body
    )
}

/// A one-time token a page's forms must send back. Only a page that was just
/// rendered can know it, so a link, a pop-up or a back/forward replay of an
/// earlier submission cannot change anything.
pub struct FormToken {
    slot: Mutex<Option<String>>,
}

impl FormToken {
    pub const fn new() -> Self {
        FormToken {
            slot: Mutex::new(None),
        }
    }

    /// Makes and remembers a new token, replacing the previous one.
    pub fn issue(&self) -> String {
        use std::hash::{BuildHasher, Hasher};
        // Each `RandomState` is seeded from the OS's random number generator.
        let token: String = (0..2)
            .map(|_| {
                let mut h = std::collections::hash_map::RandomState::new().build_hasher();
                h.write_u8(0);
                format!("{:016x}", h.finish())
            })
            .collect();
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(token.clone());
        token
    }

    /// Forgets the current token.
    #[cfg(test)]
    pub fn clear(&self) {
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Parses a submitted query (or form body) and returns its pairs if it
    /// carries the current token, which is used up. `None` otherwise.
    pub fn take(&self, query: &str) -> Option<Vec<(String, String)>> {
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let sent = pairs.iter().find(|(k, _)| k == "token").map(|(_, v)| v);
        if slot.is_none() || sent != slot.as_ref() {
            return None;
        }
        *slot = None;
        Some(pairs)
    }
}

/// Percent-encodes a value for use inside a query string.
pub fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// A settings row with a pill switch. `href` is where the switch leads.
pub fn switch_row(first: bool, title: &str, detail: &str, on: bool, href: &str) -> String {
    format!(
        "<div class=\"row{}\"><div class=\"grow\"><div class=\"t\">{}</div><div class=\"d\">{}</div></div>\
         <a class=\"sw{}\" href=\"{}\"><span></span></a></div>",
        if first { " first" } else { "" },
        escape(title),
        escape(detail),
        if on { " on" } else { "" },
        escape(href)
    )
}

/// A settings row with a segmented control. `options` are
/// `(label, href, selected)`.
pub fn segmented_row(
    first: bool,
    title: &str,
    detail: &str,
    options: &[(&str, String, bool)],
) -> String {
    let mut links = String::new();
    for (label, href, selected) in options {
        links.push_str(&format!(
            "<a{} href=\"{}\">{}</a>",
            if *selected { " class=\"on\"" } else { "" },
            escape(href),
            escape(label)
        ));
    }
    format!(
        "<div class=\"row{}\"><div class=\"grow\"><div class=\"t\">{}</div><div class=\"d\">{}</div></div>\
         <div class=\"seg\">{}</div></div>",
        if first { " first" } else { "" },
        escape(title),
        escape(detail),
        links
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup_and_quotes() {
        assert_eq!(escape("<a href=\"x\">&'"), "&lt;a href=&quot;x&quot;&gt;&amp;&#39;");
    }

    #[test]
    fn css_replaces_every_token_for_both_themes() {
        for dark in [false, true] {
            let css = css_for(dark);
            assert!(!css.contains('@'), "unreplaced token in {dark}: {css}");
        }
        assert!(css_for(true).contains("#14161a"));
        assert!(css_for(false).contains("#f5f6f8"));
    }

    #[test]
    fn tokens_are_single_use() {
        let tokens = FormToken::new();
        assert!(tokens.take("token=abc").is_none());
        let t = tokens.issue();
        assert!(tokens.take("token=wrong&x=1").is_none());
        assert!(tokens.take("x=1").is_none());
        let pairs = tokens.take(&format!("x=1&token={t}")).unwrap();
        assert!(pairs.contains(&("x".into(), "1".into())));
        assert!(tokens.take(&format!("token={t}")).is_none(), "used up");
    }

    #[test]
    fn switch_and_segmented_rows_carry_state() {
        let on = switch_row(true, "Bar", "Show it", true, "browsecraft://flags?x=1&y=2");
        assert!(on.contains("class=\"sw on\"") && on.contains("&amp;y=2"));
        assert!(switch_row(false, "Bar", "", false, "x").contains("class=\"sw\""));
        let seg = segmented_row(false, "T", "", &[("A", "a".into(), false), ("B", "b".into(), true)]);
        assert!(seg.contains("<a class=\"on\" href=\"b\">B</a>"));
    }
}
