//! User-settable browser behavior ("flags"), persisted to `flags.txt` in the
//! working directory.
//!
//! BrowseCraft's own experiment switches (no Chromium involved): a tiny
//! `key=value` store. They load once at process start and are written back whenever they change,
//! as `key=value` lines. The internal `browsecraft://flags` page edits them; a few
//! (theme, tab side) also have toolbar/panel shortcuts.

use std::sync::RwLock;

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
}

impl Default for Flags {
    fn default() -> Self {
        Flags {
            tabs_position: TabsPosition::Left,
            theme: Theme::Auto,
            chrome: Chrome::default(),
        }
    }
}

const DEFAULT_FLAGS: Flags = Flags {
    tabs_position: TabsPosition::Left,
    theme: Theme::Auto,
    chrome: Chrome::Auto,
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

/// Applies form-submitted query pairs to the flags (unknown keys ignored).
/// Returns whether anything changed.
pub fn apply_query(pairs: impl Iterator<Item = (String, String)>) -> bool {
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
            _ => {}
        }
    }
    if changed {
        set(flags);
    }
    changed
}

/// The HTML source of the `browsecraft://flags` page, reflecting the current flags.
pub fn page_html() -> String {
    let flags = get();
    let checked = |name: &str, value: &str| -> &'static str {
        let on = match name {
            "tabs-position" => matches!(
                (flags.tabs_position, value),
                (TabsPosition::Left, "left")
                    | (TabsPosition::Right, "right")
                    | (TabsPosition::Hidden, "hidden")
            ),
            "theme" => matches!(
                (flags.theme, value),
                (Theme::Auto, "auto") | (Theme::Light, "light") | (Theme::Dark, "dark")
            ),
            "chrome" => matches!(
                (flags.chrome, value),
                (Chrome::Auto, "auto") | (Chrome::Egui, "egui") | (Chrome::Engine, "engine")
            ),
            _ => false,
        };
        if on { " checked" } else { "" }
    };
    format!(
        "<!doctype html><title>Flags</title>\
         <style>body{{font-family:sans-serif;margin:0;background:#f6f7f9;color:#1f2328;font-size:14px}}\
         main{{max-width:640px;margin:40px auto;padding:0 24px}}h1{{font-size:22px}}\
         p{{color:#57606a}}fieldset{{border:1px solid #d6d8dc;border-radius:8px;margin:16px 0;padding:12px 16px}}\
         legend{{padding:0 6px;font-weight:600}}label{{display:block;margin:6px 0}}input{{accent-color:#2f6fde}}\
         button{{margin-top:4px;padding:8px 20px;border:0;border-radius:6px;background:#2f6fde;color:white;font-size:14px}}\
         button:focus{{outline:none}}</style>\
         <main><h1>Flags</h1><p>Experiments for BrowseCraft. Changes apply immediately.</p>\
         <form><fieldset><legend>Tab sidebar</legend>\
         <label><input type=\"radio\" name=\"tabs-position\" value=\"left\"{0}> Show tabs on the left</label>\
         <label><input type=\"radio\" name=\"tabs-position\" value=\"right\"{1}> Show tabs on the right</label>\
         <label><input type=\"radio\" name=\"tabs-position\" value=\"hidden\"{2}> Hide the tab sidebar</label>\
         </fieldset><fieldset><legend>Theme</legend>\
         <label><input type=\"radio\" name=\"theme\" value=\"auto\"{3}> Auto (follow the system)</label>\
         <label><input type=\"radio\" name=\"theme\" value=\"light\"{4}> Light</label>\
         <label><input type=\"radio\" name=\"theme\" value=\"dark\"{5}> Dark</label>\
         </fieldset><fieldset><legend>Chrome</legend>\
         <label><input type=\"radio\" name=\"chrome\" value=\"auto\"{6}> Auto (use GPU if available)</label>\
         <label><input type=\"radio\" name=\"chrome\" value=\"egui\"{7}> Egui (GPU-rendered chrome)</label>\
         <label><input type=\"radio\" name=\"chrome\" value=\"engine\"{8}> Engine (software chrome)</label>\
         </fieldset><button type=\"submit\">Apply</button></form></main>",
        checked("tabs-position", "left"),
        checked("tabs-position", "right"),
        checked("tabs-position", "hidden"),
        checked("theme", "auto"),
        checked("theme", "light"),
        checked("theme", "dark"),
        checked("chrome", "auto"),
        checked("chrome", "egui"),
        checked("chrome", "engine"),
    )
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
        let pairs = vec![
            ("tabs-position".into(), "right".into()),
            ("theme".into(), "dark".into()),
            ("chrome".into(), "egui".into()),
        ];
        assert!(apply_query(pairs.into_iter()));
        let after = get();
        assert_eq!(after.tabs_position, TabsPosition::Right);
        assert_eq!(after.theme, Theme::Dark);
        assert_eq!(after.chrome, Chrome::Egui);
        assert!(!apply_query(vec![].into_iter()));
        assert_eq!(effective_dark(true), true);
        set(before);
    }

    #[test]
    fn unknown_keys_ignored() {
        let _guard = SERIAL.lock().unwrap();
        set(Flags::default());
        assert!(!apply_query(vec![("bogus".into(), "1".into())].into_iter()));
        let f = get();
        assert_eq!(f.tabs_position, TabsPosition::Left);
    }

    #[test]
    fn page_has_current_values_marked() {
        let _guard = SERIAL.lock().unwrap();
        set(Flags {
            tabs_position: TabsPosition::Right,
            theme: Theme::Dark,
            chrome: Chrome::Egui,
        });
        let html = page_html();
        assert!(html.contains("name=\"tabs-position\" value=\"right\" checked"));
        assert!(html.contains("name=\"theme\" value=\"dark\" checked"));
        assert!(html.contains("name=\"chrome\" value=\"egui\" checked"));
        assert!(!html.contains("name=\"tabs-position\" value=\"left\" checked"));
        set(Flags::default());
    }

    #[test]
    fn roundtrip_via_file() {
        let _guard = SERIAL.lock().unwrap();
        set(Flags {
            tabs_position: TabsPosition::Right,
            theme: Theme::Dark,
            chrome: Chrome::Egui,
        });
        let mut flags = Flags::default();
        load_from_mut(&mut flags);
        assert_eq!(flags.tabs_position, TabsPosition::Right);
        assert_eq!(flags.theme, Theme::Dark);
        assert_eq!(flags.chrome, Chrome::Egui);
        set(Flags::default());
    }
}
