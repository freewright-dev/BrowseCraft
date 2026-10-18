//! User-settable behavior, atomically persisted in the per-user config folder.
//!
//! BrowseCraft's own experiment switches (no Chromium involved): a tiny
//! `key=value` store. They load once at process start and are written back whenever they change,
//! as `key=value` lines. The internal `browsecraft://flags` page edits them; a few
//! (theme, tab side) also have toolbar/panel shortcuts.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

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
static SAVE_ERROR: Mutex<Option<String>> = Mutex::new(None);

#[cfg(not(test))]
fn config_dir() -> Result<PathBuf, String> {
    let env_path = |key| {
        std::env::var_os(key)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    #[cfg(windows)]
    let base = env_path("APPDATA");
    #[cfg(target_os = "macos")]
    let base = env_path("HOME").map(|p| p.join("Library/Application Support"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = env_path("XDG_CONFIG_HOME").or_else(|| env_path("HOME").map(|p| p.join(".config")));
    base.map(|p| p.join("BrowseCraft"))
        .ok_or_else(|| "The per-user configuration folder is unavailable.".into())
}

#[cfg(test)]
fn config_dir() -> Result<PathBuf, String> {
    // Tests never touch the user's real settings or the working directory.
    std::thread_local! { static DIR: tempfile::TempDir = tempfile::tempdir().unwrap(); }
    Ok(DIR.with(|dir| dir.path().join("BrowseCraft")))
}

fn file_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join(FILE_NAME))
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
            _ => {}
        }
    }
}

fn save_to(path: &Path, flags: &Flags) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing settings directory"))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(std::io::Error::other(
            "Settings directory must be a real directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
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
    // Exclusive temporary creation + atomic replacement replaces a planted
    // destination symlink itself, never opens/truncates its target. tempfile
    // uses rename on Unix and MoveFileExW(REPLACE_EXISTING) on Windows.
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(text.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
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

/// The one-time tokens each flags page's form must send back. Only that form
/// can change the flags: a link, a pop-up, another page's form or a
/// back/forward replay of an earlier submission cannot know it.
const MAX_FORM_TOKENS: usize = 128;
static FORM_TOKENS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Each retained form owns a one-time OS-random token. Opening another form
/// preserves older tokens until the bounded store evicts its oldest entry.
fn new_form_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| format!("Could not create a settings form token: {e}"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut tokens = FORM_TOKENS.lock().unwrap_or_else(|e| e.into_inner());
    if tokens.len() == MAX_FORM_TOKENS {
        tokens.pop_front();
    }
    tokens.push_back(token.clone());
    Ok(token)
}

/// Applies a submission of the flags page's form (the URL query, without
/// the `?`). Returns whether any flag changed. Without an outstanding token it
/// does nothing; with it, the token is used up.
pub fn apply_form(query: &str) -> bool {
    if query.is_empty() {
        return false;
    }
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    {
        let mut tokens = FORM_TOKENS.lock().unwrap_or_else(|e| e.into_inner());
        let sent = pairs.iter().find(|(k, _)| k == "token").map(|(_, v)| v);
        let index = sent.and_then(|sent| tokens.iter().position(|token| token == sent));
        let Some(index) = index else {
            *SAVE_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = Some("This settings form has expired or was already submitted. Review the values and apply again.".into());
            return false;
        };
        tokens.remove(index);
    }
    *SAVE_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = None;
    apply_query(pairs.into_iter())
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
            _ => {}
        }
    }
    if changed {
        return set(flags).is_ok();
    }
    changed
}

/// The HTML source of the `browsecraft://flags` page, reflecting the current
/// flags. Each call issues a new form token.
pub fn page_html() -> String {
    let flags = get();
    let token = match new_form_token() {
        Ok(token) => token,
        Err(error) => {
            return format!(
                "<title>Settings unavailable</title><p>{}</p>",
                crate::net::escape_html(&error)
            );
        }
    };
    let error = SAVE_ERROR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_deref()
        .map(|error| format!("<p role=alert>{}</p>", crate::net::escape_html(error)))
        .unwrap_or_default();
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
          <main><h1>Flags</h1><p>Experiments for BrowseCraft. Changes apply immediately.</p>{error}\
         <form><input type=\"hidden\" name=\"token\" value=\"{9}\"><fieldset><legend>Tab sidebar</legend>\
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
        token,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flags are a process-global and persist to a file; tests that touch
    /// them must not run at the same time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn retained_forms_have_independent_single_use_tokens() {
        let _guard = SERIAL.lock().unwrap();
        set(Flags::default()).unwrap();
        FORM_TOKENS.lock().unwrap().clear();
        let first = rendered_token();
        let second = rendered_token();
        assert_ne!(first, second);
        assert!(apply_form(&format!("token={first}&theme=dark")));
        assert!(apply_form(&format!("token={second}&theme=light")));
        assert!(!apply_form(&format!("token={first}&theme=auto")));
        assert!(!apply_form(&format!("token={second}&theme=auto")));
        assert_eq!(get().theme, Theme::Light);
        assert!(page_html().contains("already submitted"));
        set(Flags::default()).unwrap();
    }

    #[test]
    fn outstanding_forms_are_bounded_and_expiration_is_explained() {
        let _guard = SERIAL.lock().unwrap();
        set(Flags::default()).unwrap();
        FORM_TOKENS.lock().unwrap().clear();
        let tokens: Vec<_> = (0..=MAX_FORM_TOKENS).map(|_| rendered_token()).collect();
        assert_eq!(FORM_TOKENS.lock().unwrap().len(), MAX_FORM_TOKENS);
        assert!(!apply_form(&format!("token={}&theme=dark", tokens[0])));
        assert!(apply_form(&format!("token={}&theme=dark", tokens[1])));
        assert!(!FORM_TOKENS.lock().unwrap().contains(&tokens[1]));
        assert!(FORM_TOKENS.lock().unwrap().contains(tokens.last().unwrap()));
        set(Flags::default()).unwrap();
    }

    #[test]
    fn atomic_replacement_does_not_truncate_alias_targets() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&victim, "keep me").unwrap();
        std::fs::hard_link(&victim, &path).unwrap();
        save_to(&path, &Flags::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .starts_with("tabs-position=")
        );
        std::fs::remove_file(&path).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&victim, &path).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        save_to(&path, &Flags::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert!(
            !std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn failed_persistence_preserves_flags_and_displays_the_error() {
        let _guard = SERIAL.lock().unwrap();
        let before = get();
        let path = file_path().unwrap();
        std::fs::create_dir_all(&path).unwrap(); // a directory cannot be atomically replaced by a file
        let mut next = before;
        next.theme = Theme::Dark;
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
        set(before).unwrap();
    }

    /// The token from the hidden field of a freshly rendered flags page.
    fn rendered_token() -> String {
        let html = page_html();
        let marker = "name=\"token\" value=\"";
        let start = html.find(marker).unwrap() + marker.len();
        html[start..start + 32].to_string()
    }

    #[test]
    fn only_the_flags_form_can_change_flags() {
        let _guard = SERIAL.lock().unwrap();
        let before = get();
        set(Flags::default()).unwrap();
        let query = |token: &str| format!("token={token}&theme=dark");

        // No token, a guessed token, or a link with no form rendered yet.
        FORM_TOKENS.lock().unwrap().clear();
        assert!(!apply_form("theme=dark"));
        assert!(!apply_form("token=&theme=dark"));
        let token = rendered_token();
        assert!(!apply_form("theme=dark"));
        assert!(!apply_form(&query(&"0".repeat(32))));
        assert_eq!(get().theme, Theme::Auto);

        // The real form works once; replaying it (back/forward) does not.
        assert!(apply_form(&query(&token)));
        assert_eq!(get().theme, Theme::Dark);
        set(Flags::default()).unwrap();
        assert!(!apply_form(&query(&token)));
        assert_eq!(get().theme, Theme::Auto);

        // Rendering again issues a different token.
        assert_ne!(rendered_token(), token);
        set(before).unwrap();
    }

    #[test]
    fn unknown_keys_ignored() {
        let _guard = SERIAL.lock().unwrap();
        set(Flags::default()).unwrap();
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
        })
        .unwrap();
        let html = page_html();
        assert!(html.contains("name=\"tabs-position\" value=\"right\" checked"));
        assert!(html.contains("name=\"theme\" value=\"dark\" checked"));
        assert!(html.contains("name=\"chrome\" value=\"egui\" checked"));
        assert!(!html.contains("name=\"tabs-position\" value=\"left\" checked"));
        set(Flags::default()).unwrap();
    }

    #[test]
    fn roundtrip_via_file() {
        let _guard = SERIAL.lock().unwrap();
        set(Flags {
            tabs_position: TabsPosition::Right,
            theme: Theme::Dark,
            chrome: Chrome::Egui,
        })
        .unwrap();
        let mut flags = Flags::default();
        load_from_mut(&mut flags);
        assert_eq!(flags.tabs_position, TabsPosition::Right);
        assert_eq!(flags.theme, Theme::Dark);
        assert_eq!(flags.chrome, Chrome::Egui);
        set(Flags::default()).unwrap();
    }
}
