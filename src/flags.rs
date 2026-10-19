//! User-settable behavior, atomically persisted in the per-user config folder.
//!
//! BrowseCraft's own experiment switches (no Chromium involved): a tiny
//! `key=value` store. They load once at process start and are written back whenever they change,
//! as `key=value` lines. The internal `browsecraft://flags` page (the settings
//! page) edits them; a few (theme, tab side) also have toolbar/panel shortcuts.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

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
static SAVE_ERROR: Mutex<Option<String>> = Mutex::new(None);

fn file_path() -> Result<PathBuf, String> {
    crate::storage::config_file(FILE_NAME)
}

/// Loads the persisted flags from disk, merging into the defaults.
fn load_from_mut(flags: &mut Flags) {
    let Ok(path) = file_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
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

fn save_to(path: &Path, flags: &Flags) -> std::io::Result<()> {
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
    crate::storage::write_private(path, text.as_bytes())
}

/// Loads persisted settings once at startup without writing a new file.
pub fn load() {
    let mut flags = get();
    load_from_mut(&mut flags);
    *FLAGS.write().unwrap_or_else(|p| p.into_inner()) = flags;
}

/// The current flags (cheap: a copy).
pub fn get() -> Flags {
    *FLAGS.read().unwrap_or_else(|p| p.into_inner())
}

/// Persist atomically before changing in-memory flags; report failures to UI.
pub fn set(flags: Flags) -> Result<(), String> {
    let mut current = FLAGS.write().unwrap_or_else(|p| p.into_inner());
    let result = file_path().and_then(|path| {
        save_to(&path, &flags)
            .map_err(|e| format!("Could not save settings to {}: {e}", path.display()))
    });
    *SAVE_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = result.as_ref().err().cloned();
    if result.is_ok() {
        *current = flags;
    }
    result
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
/// `?`). Returns whether any flag changed. Without an outstanding token it
/// does nothing; with it, the token is used up.
pub fn apply_form(query: &str) -> bool {
    if query.is_empty() {
        return false;
    }
    let Some(pairs) = FORM_TOKEN.take(query) else {
        *SAVE_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = Some(
            "This settings link has expired or was already used. Review the values and choose again."
                .into(),
        );
        return false;
    };
    *SAVE_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = None;
    apply_query(pairs.into_iter().map(|(k, v)| (k, v.to_string())))
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
        return set(flags).is_ok();
    }
    changed
}

/// The HTML source of the `browsecraft://flags` (settings) page, reflecting
/// the current flags. Each call issues a new form token.
pub fn page_html() -> String {
    let flags = get();
    let token = match FORM_TOKEN.issue() {
        Ok(token) => token,
        Err(error) => return internal::unavailable_page("Settings", &error),
    };
    let error = SAVE_ERROR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_deref()
        .map(|error| {
            format!(
                "<div class=\"card\"><div class=\"row first\"><div class=\"bad\" role=alert>{}</div></div></div>",
                internal::escape(error)
            )
        })
        .unwrap_or_default();
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
        "<h1>Settings</h1><p class=\"sub\">Changes apply immediately.</p>{error}\
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
        set(Flags::default()).unwrap();
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
        set(before).unwrap();
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
        set(Flags::default()).unwrap();
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
        set(Flags::default()).unwrap();
        assert!(!apply_form(&query(&token)));
        assert_eq!(get().theme, Theme::Dark);

        // Rendering again issues a different token.
        assert_ne!(rendered_token(), token);
        set(before).unwrap();
    }

    #[test]
    fn unknown_keys_ignored() {
        let _guard = SERIAL.lock().unwrap();
        let before = get();
        set(Flags::default()).unwrap();
        assert!(!apply_query(vec![("bogus".into(), "1".into())].into_iter()));
        assert_eq!(get().tabs_position, TabsPosition::Left);
        set(before).unwrap();
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
        }).unwrap();
        let html = page_html();
        assert!(html.contains("tabs-position=right\">Right"), "unselected links have no class");
        assert!(html.contains("class=\"on\" href=\"browsecraft://flags?token="));
        assert_eq!(html.matches("class=\"on\"").count(), 3, "one selected option per control");
        // The bookmarks-bar switch is off and offers to turn it on.
        assert!(html.contains("bookmarks-bar=on"));
        assert!(!html.contains("class=\"sw on\""));
        set(Flags { bookmarks_bar: true, ..Flags::default() }).unwrap();
        assert!(page_html().contains("class=\"sw on\""));
        set(before).unwrap();
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
        }).unwrap();
        let mut flags = Flags::default();
        load_from_mut(&mut flags);
        assert_eq!(flags.tabs_position, TabsPosition::Right);
        assert_eq!(flags.theme, Theme::Light);
        assert_eq!(flags.chrome, Chrome::Egui);
        assert!(!flags.bookmarks_bar);
        set(before).unwrap();
    }

    #[test]
    fn retained_pages_have_independent_single_use_tokens() {
        let _guard = SERIAL.lock().unwrap();
        set(Flags::default()).unwrap();
        FORM_TOKEN.clear();
        let first = rendered_token();
        let second = rendered_token();
        assert_ne!(first, second);
        assert!(apply_form(&format!("token={first}&theme=light")));
        assert!(apply_form(&format!("token={second}&theme=auto")));
        assert!(!apply_form(&format!("token={first}&theme=dark")));
        assert!(!apply_form(&format!("token={second}&theme=dark")));
        assert_eq!(get().theme, Theme::Auto);
        assert!(page_html().contains("already used"));
        set(Flags::default()).unwrap();
    }

    #[test]
    fn failed_persistence_preserves_flags_and_displays_the_error() {
        let _guard = SERIAL.lock().unwrap();
        let before = get();
        let path = file_path().unwrap();
        std::fs::create_dir_all(&path).unwrap(); // a directory cannot be atomically replaced by a file
        let mut next = before;
        next.theme = Theme::Light;
        assert!(set(next).is_err());
        assert_eq!(get(), before);
        assert!(page_html().contains("Could not save settings"));
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        std::fs::remove_dir(path).unwrap();
        set(before).unwrap();
    }
}
