//! User-settable browser behavior ("flags"), persisted to `flags.txt` in the
//! working directory.
//!
//! BrowseCraft's own experiment switches (no Chromium involved): a tiny
//! `key=value` store. They load once at process start and are written back whenever they change,
//! as `key=value` lines. The internal `browsecraft://flags` page (the settings
//! page) edits them; a few (theme, tab side) also have toolbar/panel shortcuts.

use std::sync::RwLock;

use crate::internal::{self, FormToken};

const FILE_NAME: &str = "flags.txt";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabsPosition {
    Left,
    Right,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Auto,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Chrome {
    #[default]
    Auto,
    Egui,
    Engine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flags {
    pub tabs_position: TabsPosition,
    pub theme: Theme,
    pub chrome: Chrome,
    /// The bookmarks bar under the address bar.
    pub bookmarks_bar: bool,
}

impl Default for Flags {
    fn default() -> Self {
        DEFAULT_FLAGS
    }
}

const DEFAULT_FLAGS: Flags = Flags {
    tabs_position: TabsPosition::Left,
    theme: Theme::Dark,
    chrome: Chrome::Auto,
    bookmarks_bar: true,
};

static FLAGS: RwLock<Flags> = RwLock::new(DEFAULT_FLAGS);

fn file_path() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_default().join(FILE_NAME)
}

/// Loads the persisted flags from disk, merging into the defaults.
fn load_from_mut(flags: &mut Flags) {
    let Ok(text) = std::fs::read_to_string(file_path()) else {
        return;
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (key.trim(), value.trim()) {
            ("tabs-position", "left") => flags.tabs_position = TabsPosition::Left,
            ("tabs-position", "right") => flags.tabs_position = TabsPosition::Right,
            ("tabs-position", "hidden") => flags.tabs_position = TabsPosition::Hidden,
            ("theme", "auto") => flags.theme = Theme::Auto,
            ("theme", "light") => flags.theme = Theme::Light,
            ("theme", "dark") => flags.theme = Theme::Dark,
            ("chrome", "auto") => flags.chrome = Chrome::Auto,
            ("chrome", "egui") => flags.chrome = Chrome::Egui,
            ("chrome", "engine") => flags.chrome = Chrome::Engine,
            ("bookmarks-bar", "on") => flags.bookmarks_bar = true,
            ("bookmarks-bar", "off") => flags.bookmarks_bar = false,
            _ => {}
        }
    }
}

fn save(flags: &Flags) {
    let mut text = String::new();
    match flags.tabs_position {
        TabsPosition::Left => text += "tabs-position=left\n",
        TabsPosition::Right => text += "tabs-position=right\n",
        TabsPosition::Hidden => text += "tabs-position=hidden\n",
    }
    match flags.theme {
        Theme::Auto => text += "theme=auto\n",
        Theme::Light => text += "theme=light\n",
        Theme::Dark => text += "theme=dark\n",
    }
    match flags.chrome {
        Chrome::Auto => text += "chrome=auto\n",
        Chrome::Egui => text += "chrome=egui\n",
        Chrome::Engine => text += "chrome=engine\n",
    }
    text += if flags.bookmarks_bar {
        "bookmarks-bar=on\n"
    } else {
        "bookmarks-bar=off\n"
    };
    let _ = std::fs::write(file_path(), text);
}

/// Applies `load_from_mut`, persisting only the keys that changed. Call once
/// at startup.
pub fn load() {
    let mut flags = get();
    load_from_mut(&mut flags);
    *FLAGS.write().unwrap_or_else(|p| p.into_inner()) = flags;
}

/// The current flags (cheap: a copy).
pub fn get() -> Flags {
    *FLAGS.read().unwrap_or_else(|p| p.into_inner())
}

/// Replaces the flags and writes them to `flags.txt`.
pub fn set(flags: Flags) {
    *FLAGS.write().unwrap_or_else(|p| p.into_inner()) = flags;
    save(&flags);
}

// ---- browsecraft://flags page ---------------------------------------------------

/// The effective theme for the browser chrome: the flag, or the OS theme.
pub fn effective_dark(window_dark: bool) -> bool {
    match get().theme {
        Theme::Light => false,
        Theme::Dark => true,
        Theme::Auto => window_dark,
    }
}

/// The one-time token the settings page's links must send back. Only that
/// page can change the flags: a link, a pop-up, another page's form or a
/// back/forward replay of an earlier submission cannot know it.
static FORM_TOKEN: FormToken = FormToken::new();

/// Applies a submission of the settings page (the URL query, without the
/// `?`). Returns whether any flag changed. Without the current token it does
/// nothing; with it, the token is used up.
pub fn apply_form(query: &str) -> bool {
    match FORM_TOKEN.take(query) {
        Some(pairs) => apply_query(pairs.into_iter()),
        None => false,
    }
}

fn apply_query(pairs: impl Iterator<Item = (String, String)>) -> bool {
    let mut flags = get();
    let mut changed = false;
    for (key, value) in pairs {
        match (key.as_str(), value.as_str()) {
            ("tabs-position", "left") => {
                changed |= flags.tabs_position != TabsPosition::Left;
                flags.tabs_position = TabsPosition::Left;
            }
            ("tabs-position", "right") => {
                changed |= flags.tabs_position != TabsPosition::Right;
                flags.tabs_position = TabsPosition::Right;
            }
            ("tabs-position", "hidden") => {
                changed |= flags.tabs_position != TabsPosition::Hidden;
                flags.tabs_position = TabsPosition::Hidden;
            }
            ("theme", "auto") => {
                changed |= flags.theme != Theme::Auto;
                flags.theme = Theme::Auto;
            }
            ("theme", "light") => {
                changed |= flags.theme != Theme::Light;
                flags.theme = Theme::Light;
            }
            ("theme", "dark") => {
                changed |= flags.theme != Theme::Dark;
                flags.theme = Theme::Dark;
            }
            ("chrome", "auto") => {
                changed |= flags.chrome != Chrome::Auto;
                flags.chrome = Chrome::Auto;
            }
            ("chrome", "egui") => {
                changed |= flags.chrome != Chrome::Egui;
                flags.chrome = Chrome::Egui;
            }
            ("chrome", "engine") => {
                changed |= flags.chrome != Chrome::Engine;
                flags.chrome = Chrome::Engine;
            }
            ("bookmarks-bar", "on") => {
                changed |= !flags.bookmarks_bar;
                flags.bookmarks_bar = true;
            }
            ("bookmarks-bar", "off") => {
                changed |= flags.bookmarks_bar;
                flags.bookmarks_bar = false;
            }
            _ => {}
        }
    }
    if changed {
        set(flags);
    }
    changed
}

/// The HTML source of the `browsecraft://flags` (settings) page, reflecting
/// the current flags. Each call issues a new form token.
pub fn page_html() -> String {
    let flags = get();
    let token = FORM_TOKEN.issue();
    // Every option is a link back to the page carrying the token.
    let to = |key: &str, value: &str| format!("browsecraft://flags?token={token}&{key}={value}");

    let theme = |label: &'static str, value: &str, on: bool| (label, to("theme", value), on);
    let side = |label: &'static str, value: &str, on: bool| (label, to("tabs-position", value), on);
    let render = |label: &'static str, value: &str, on: bool| (label, to("chrome", value), on);

    let appearance = internal::segmented_row(
        true,
        "Theme",
        "Dark is easiest on the eyes. System follows your OS.",
        &[
            theme("System", "auto", flags.theme == Theme::Auto),
            theme("Light", "light", flags.theme == Theme::Light),
            theme("Dark", "dark", flags.theme == Theme::Dark),
        ],
    );
    let layout = internal::segmented_row(
        true,
        "Tab sidebar",
        "Where your open tabs are listed.",
        &[
            side("Left", "left", flags.tabs_position == TabsPosition::Left),
            side("Right", "right", flags.tabs_position == TabsPosition::Right),
            side("Hidden", "hidden", flags.tabs_position == TabsPosition::Hidden),
        ],
    );
    let bar = internal::switch_row(
        false,
        "Bookmarks bar",
        "Show your bookmarks under the address bar.",
        flags.bookmarks_bar,
        &to("bookmarks-bar", if flags.bookmarks_bar { "off" } else { "on" }),
    );
    let renderer = internal::segmented_row(
        true,
        "Interface renderer",
        "Auto uses the GPU when there is one. Applies after a restart.",
        &[
            render("Auto", "auto", flags.chrome == Chrome::Auto),
            render("GPU", "egui", flags.chrome == Chrome::Egui),
            render("Software", "engine", flags.chrome == Chrome::Engine),
        ],
    );
    let body = format!(
        "<h1>Settings</h1><p class=\"sub\">Changes apply immediately.</p>\
         <div class=\"card\"><h2>Appearance</h2>{appearance}</div>\
         <div class=\"card\"><h2>Layout</h2>{layout}{bar}</div>\
         <div class=\"card\"><h2>Advanced</h2>{renderer}</div>\
         <div class=\"card\"><h2>Shortcuts to your data</h2>\
         <div class=\"row first\"><div class=\"grow\"><div class=\"t\">Bookmarks</div><div class=\"d\">Pages you saved.</div></div>\
         <a class=\"btn ghost\" href=\"browsecraft://bookmarks\">Open</a></div>\
         <div class=\"row\"><div class=\"grow\"><div class=\"t\">History</div><div class=\"d\">Pages you visited.</div></div>\
         <a class=\"btn ghost\" href=\"browsecraft://history\">Open</a></div>\
         <div class=\"row\"><div class=\"grow\"><div class=\"t\">Passwords</div><div class=\"d\">Your KeePass (.kdbx) vault.</div></div>\
         <a class=\"btn ghost\" href=\"browsecraft://passwords\">Open</a></div></div>"
    );
    internal::page("Settings", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flags are a process-global and persist to a file; tests that touch
    /// them must not run at the same time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn query_sets_flags() {
        let _guard = SERIAL.lock().unwrap();
        let before = get();
        set(Flags::default());
        let pairs = vec![
            ("tabs-position".into(), "right".into()),
            ("theme".into(), "light".into()),
            ("chrome".into(), "egui".into()),
            ("bookmarks-bar".into(), "off".into()),
        ];
        assert!(apply_query(pairs.into_iter()));
        let after = get();
        assert_eq!(after.tabs_position, TabsPosition::Right);
        assert_eq!(after.theme, Theme::Light);
        assert_eq!(after.chrome, Chrome::Egui);
        assert!(!after.bookmarks_bar);
        assert!(!apply_query(vec![].into_iter()));
        assert!(!effective_dark(false));
        set(before);
    }

    #[test]
    fn dark_is_the_default_theme() {
        assert_eq!(Flags::default().theme, Theme::Dark);
        assert!(Flags::default().bookmarks_bar);
    }

    /// The token from a freshly rendered settings page.
    fn rendered_token() -> String {
        let html = page_html();
        let marker = "token=";
        let start = html.find(marker).unwrap() + marker.len();
        html[start..start + 32].to_string()
    }

    #[test]
    fn only_the_settings_page_can_change_flags() {
        let _guard = SERIAL.lock().unwrap();
        let before = get();
        set(Flags::default());
        let query = |token: &str| format!("token={token}&theme=light");

        // No token, a guessed token, or a link with no page rendered yet.
        FORM_TOKEN.clear();
        assert!(!apply_form("theme=light"));
        assert!(!apply_form("token=&theme=light"));
        let token = rendered_token();
        assert!(!apply_form("theme=light"));
        assert!(!apply_form(&query(&"0".repeat(32))));
        assert_eq!(get().theme, Theme::Dark);

        // The real link works once; replaying it (back/forward) does not.
        assert!(apply_form(&query(&token)));
        assert_eq!(get().theme, Theme::Light);
        set(Flags::default());
        assert!(!apply_form(&query(&token)));
        assert_eq!(get().theme, Theme::Dark);

        // Rendering again issues a different token.
        assert_ne!(rendered_token(), token);
        set(before);
    }

    #[test]
    fn unknown_keys_ignored() {
        let _guard = SERIAL.lock().unwrap();
        let before = get();
        set(Flags::default());
        assert!(!apply_query(vec![("bogus".into(), "1".into())].into_iter()));
        assert_eq!(get().tabs_position, TabsPosition::Left);
        set(before);
    }

    #[test]
    fn page_marks_the_current_values() {
        let _guard = SERIAL.lock().unwrap();
        let before = get();
        set(Flags {
            tabs_position: TabsPosition::Right,
            theme: Theme::Light,
            chrome: Chrome::Egui,
            bookmarks_bar: false,
        });
        let html = page_html();
        assert!(html.contains("tabs-position=right\">Right"), "unselected links have no class");
        assert!(html.contains("class=\"on\" href=\"browsecraft://flags?token="));
        assert_eq!(html.matches("class=\"on\"").count(), 3, "one selected option per control");
        // The bookmarks-bar switch is off and offers to turn it on.
        assert!(html.contains("bookmarks-bar=on"));
        assert!(!html.contains("class=\"sw on\""));
        set(Flags { bookmarks_bar: true, ..Flags::default() });
        assert!(page_html().contains("class=\"sw on\""));
        set(before);
    }

    #[test]
    fn roundtrip_via_file() {
        let _guard = SERIAL.lock().unwrap();
        let before = get();
        set(Flags {
            tabs_position: TabsPosition::Right,
            theme: Theme::Light,
            chrome: Chrome::Egui,
            bookmarks_bar: false,
        });
        let mut flags = Flags::default();
        load_from_mut(&mut flags);
        assert_eq!(flags.tabs_position, TabsPosition::Right);
        assert_eq!(flags.theme, Theme::Light);
        assert_eq!(flags.chrome, Chrome::Egui);
        assert!(!flags.bookmarks_bar);
        set(before);
    }
}
