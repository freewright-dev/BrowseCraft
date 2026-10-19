//! The password manager: the unlocked vault, its settings, the
//! `browsecraft://passwords` page, the "save this login?" offer and the
//! script that finds login and sign-up forms on web pages.
//!
//! The vault itself (the `.kdbx` file) lives in `vault.rs`. This module keeps
//! the one unlocked [`Vault`] in memory, locks it again after a quiet spell,
//! and never shows a saved password except where the user asked for it
//! (Fill, Show, Copy). Password copies made on the way (fill scripts, save
//! offers, clipboard checks) are `Zeroizing`, so they are wiped once used.
//!
//! Page and browser talk like this:
//!
//! * The page script (`passwords.js`) reports forms (`bc:forms?kind=…`) and
//!   submitted logins (`bc:cred?…`) over WebView IPC. Any page can send those,
//!   so they only ever *offer* to save what that page itself just handled;
//!   the user decides.
//! * Filling goes the other way: the browser runs `__bcFill(…)` in the page,
//!   only when the user picked a login for that very site.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use crate::internal::{self, FormToken};
use crate::vault::{self, Credential, NewProtection, Unlock, Vault};

/// The script injected into every web page.
pub const PAGE_SCRIPT: &str = include_str!("passwords.js");

const FILE_NAME: &str = "passwords.cfg";
/// The vault locks itself after this long without use.
const AUTO_LOCK: Duration = Duration::from_secs(15 * 60);
/// A save offer is forgotten after this long.
const OFFER_TTL: Duration = Duration::from_secs(10 * 60);
/// How long a copied password stays on the clipboard.
const CLIPBOARD_CLEAR: Duration = Duration::from_secs(30);
const MAX_NEVER: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Password,
    KeyFile,
}

/// What kind of form the page script found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormKind {
    Login,
    SignUp,
    ChangePassword,
}

impl FormKind {
    pub fn parse(text: &str) -> Option<FormKind> {
        match text {
            "login" => Some(FormKind::Login),
            "signup" => Some(FormKind::SignUp),
            "change" => Some(FormKind::ChangePassword),
            _ => None,
        }
    }
}

/// A login the page just submitted, waiting for the user's answer. The
/// password is kept private and is not shown in `Debug` output.
#[derive(Clone)]
pub struct Offer {
    pub host: String,
    /// What the login is saved under: the site's address.
    pub url: String,
    pub username: String,
    password: Zeroizing<String>,
    pub kind: FormKind,
    /// Set when the vault already has this user on this site with another
    /// password: the offer is then to update that login.
    pub update_id: Option<String>,
    made: Instant,
}

impl std::fmt::Debug for Offer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Offer")
            .field("host", &self.host)
            .field("username", &self.username)
            .field("password", &"<hidden>")
            .finish()
    }
}

/// What the key menu and the save bar need to know.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Status {
    /// No vault is set up yet.
    NoVault,
    Locked,
    Unlocked { logins: usize },
}

struct State {
    vault_path: Option<PathBuf>,
    mode: Mode,
    keyfile_path: Option<PathBuf>,
    /// Sites the user said never to offer saving for.
    never: Vec<String>,
    vault: Option<Vault>,
    last_used: Option<Instant>,
    /// One-shot message for the page: (ok, text).
    message: Option<(bool, String)>,
    revealed: Option<String>,
    confirm_delete: Option<String>,
    offer: Option<Offer>,
}

impl State {
    const fn new() -> State {
        State {
            vault_path: None,
            mode: Mode::Password,
            keyfile_path: None,
            never: Vec::new(),
            vault: None,
            last_used: None,
            message: None,
            revealed: None,
            confirm_delete: None,
            offer: None,
        }
    }

    /// Locks an idle vault and forgets a stale offer.
    fn expire(&mut self) {
        if self.vault.is_some() && self.last_used.is_none_or(|t| t.elapsed() > AUTO_LOCK) {
            self.lock_now();
            self.message = Some((true, "The vault locked itself after 15 minutes without use.".into()));
        }
        if self.offer.as_ref().is_some_and(|o| o.made.elapsed() > OFFER_TTL) {
            self.offer = None;
        }
    }

    fn lock_now(&mut self) {
        self.vault = None;
        self.revealed = None;
        self.confirm_delete = None;
        self.last_used = None;
    }

    fn touch(&mut self) {
        self.last_used = Some(Instant::now());
    }

    fn fail(&mut self, text: impl Into<String>) {
        self.message = Some((false, text.into()));
    }

    fn ok(&mut self, text: impl Into<String>) {
        self.message = Some((true, text.into()));
    }

    fn unlocked(&mut self, vault: Vault, path: &Path, mode: Mode, keyfile: Option<PathBuf>) {
        self.vault = Some(vault);
        self.vault_path = Some(path.to_path_buf());
        self.mode = mode;
        self.keyfile_path = keyfile;
        self.touch();
        save_config(self);
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());
static FORM_TOKEN: FormToken = FormToken::new();

fn state() -> MutexGuard<'static, State> {
    let mut s = STATE.lock().unwrap_or_else(|p| p.into_inner());
    s.expire();
    s
}

// ---- settings file ----

fn config_path() -> Result<PathBuf, String> {
    crate::storage::config_file(FILE_NAME)
}

fn save_config(s: &State) {
    let mut text = String::new();
    if let Some(p) = &s.vault_path {
        text += &format!("vault={}\n", p.display());
    }
    text += match s.mode {
        Mode::Password => "mode=password\n",
        Mode::KeyFile => "mode=keyfile\n",
    };
    if let Some(p) = &s.keyfile_path {
        text += &format!("keyfile={}\n", p.display());
    }
    for host in &s.never {
        text += &format!("never={host}\n");
    }
    if let Ok(path) = config_path() {
        let _ = crate::storage::write_private(&path, text.as_bytes());
    }
}

fn parse_config(s: &mut State, text: &str) {
    s.never.clear();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "vault" if !value.is_empty() => s.vault_path = Some(PathBuf::from(value)),
            "mode" => {
                s.mode = if value == "keyfile" { Mode::KeyFile } else { Mode::Password };
            }
            "keyfile" if !value.is_empty() => s.keyfile_path = Some(PathBuf::from(value)),
            "never" if !value.is_empty() && s.never.len() < MAX_NEVER => s.never.push(value.to_string()),
            _ => {}
        }
    }
}

/// Reads `passwords.cfg` (which vault, how it unlocks, the never-save list).
/// The vault stays locked until the user unlocks it.
pub fn load() {
    if let Ok(text) = config_path().and_then(|path| std::fs::read_to_string(path).map_err(|e| e.to_string())) {
        parse_config(&mut state(), &text);
    }
}

// ---- status ----

pub fn status() -> Status {
    let s = state();
    match (&s.vault, &s.vault_path) {
        (Some(v), _) => Status::Unlocked { logins: v.count() },
        (None, Some(_)) => Status::Locked,
        (None, None) => Status::NoVault,
    }
}

pub fn lock() {
    let mut s = state();
    s.lock_now();
}

/// The logins for the site of `url`. Empty while the vault is locked.
pub fn logins_for(url: &str) -> Vec<Credential> {
    let mut s = state();
    s.touch_if_unlocked();
    s.vault.as_ref().map(|v| v.for_site(url)).unwrap_or_default()
}

impl State {
    fn touch_if_unlocked(&mut self) {
        if self.vault.is_some() {
            self.touch();
        }
    }
}

// ---- filling ----

/// A JavaScript string literal for `text`, safe to embed in a script.
pub fn js_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Keep `</script>` and HTML comments inert, and the line
            // separators that old engines reject inside literals.
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The script that fills login `id` into the page at `page_url`. `None`
/// unless the vault is unlocked and the login belongs to that site. The
/// script holds the password, so it is wiped when dropped.
pub fn fill_script(id: &str, page_url: &str) -> Option<Zeroizing<String>> {
    let mut s = state();
    s.touch_if_unlocked();
    let vault = s.vault.as_ref()?;
    let login = vault.credentials().into_iter().find(|c| c.id == id)?;
    if !vault::same_site(&login.url, page_url) {
        return None;
    }
    let password = vault.password(id)?;
    let password = Zeroizing::new(js_string(&password));
    let username = js_string(&login.username);
    // Sized up front so the buffer never reallocates and leaves a copy behind.
    let (head, tail) = ("window.__bcFill&&window.__bcFill(", ");");
    let mut script = Zeroizing::new(String::with_capacity(
        head.len() + username.len() + 1 + password.len() + tail.len(),
    ));
    for part in [head, &username, ",", &password, tail] {
        script.push_str(part);
    }
    Some(script)
}

/// A script that puts a new strong password in the page's password fields
/// (for sign-up forms). The page then offers to save it when submitted.
pub fn suggest_script() -> String {
    format!(
        "window.__bcFill&&window.__bcFill(null,{});",
        js_string(&vault::generate_password(20))
    )
}

// ---- the save offer ----

fn origin_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .filter(|u| u.host_str().is_some())
        .map(|u| u.origin().ascii_serialization() + "/")
        .unwrap_or_else(|| url.to_string())
}

/// Called when the page at `page_url` submitted a login. Returns whether
/// there is now an offer to show (not for sites on the never list, and not
/// when the vault already has exactly this login).
pub fn offer_login(page_url: &str, username: &str, password: &str, kind: FormKind) -> bool {
    let host = vault::host_key(page_url);
    if host.is_empty() || password.is_empty() {
        return false;
    }
    let mut s = state();
    if s.never.contains(&host) {
        return false;
    }
    let mut update_id = None;
    if let Some(vault) = &s.vault {
        for login in vault.for_site(page_url).into_iter().filter(|c| c.username == username) {
            if vault.password(&login.id).is_some_and(|saved| saved.as_str() == password) {
                return false;
            }
            update_id = Some(login.id);
        }
    }
    s.offer = Some(Offer {
        host,
        url: origin_of(page_url),
        username: username.to_string(),
        password: Zeroizing::new(password.to_string()),
        kind,
        update_id,
        made: Instant::now(),
    });
    true
}

/// Whether a save offer is waiting (cheap: nothing is copied).
pub fn has_offer() -> bool {
    state().offer.is_some()
}

pub fn current_offer() -> Option<Offer> {
    state().offer.clone()
}

/// Forgets the offer unless it is for the site of `page_url` (called when
/// the user navigates somewhere else).
pub fn drop_offer_unless_site(page_url: &str) {
    let mut s = state();
    if s.offer.as_ref().is_some_and(|o| !vault::same_site(&o.url, page_url)) {
        s.offer = None;
    }
}

pub fn dismiss_offer() {
    state().offer = None;
}

/// "Never for this site": drops the offer and remembers the site.
pub fn never_offer_for_site() {
    let mut s = state();
    if let Some(offer) = s.offer.take()
        && !s.never.contains(&offer.host)
        && s.never.len() < MAX_NEVER
    {
        s.never.push(offer.host);
        save_config(&s);
    }
}

/// Saves (or updates) the offered login. Needs an unlocked vault.
pub fn save_offer() -> Result<String, String> {
    let mut s = state();
    let offer = s.offer.clone().ok_or("There is nothing to save.")?;
    let vault = s.vault.as_mut().ok_or("Unlock your vault first.")?;
    let message = match &offer.update_id {
        Some(id) => {
            vault.set_password(id, &offer.password)?;
            format!("Updated the password for {}.", offer.host)
        }
        None => {
            vault.add(&offer.host, &offer.url, &offer.username, &offer.password)?;
            format!("Saved the login for {}.", offer.host)
        }
    };
    s.offer = None;
    s.touch();
    Ok(message)
}

// ---- messages from the page script ----

/// What the page script reported over IPC.
#[derive(Clone)]
pub enum PageMessage {
    /// The kind of password form the page now has (`None`: none).
    Forms(Option<FormKind>),
    /// A login the page just submitted.
    Login {
        url: String,
        username: String,
        password: Zeroizing<String>,
        kind: FormKind,
    },
}

impl std::fmt::Debug for PageMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PageMessage::Forms(kind) => write!(f, "Forms({kind:?})"),
            PageMessage::Login { username, kind, .. } => {
                write!(f, "Login {{ username: {username:?}, password: <hidden>, kind: {kind:?} }}")
            }
        }
    }
}

/// Parses a `bc:` IPC message from the page script. Anything malformed or
/// oversized is ignored: the page is untrusted.
pub fn parse_page_message(body: &str) -> Option<PageMessage> {
    if body.len() > 4096 {
        return None;
    }
    let (path, query) = body.strip_prefix("bc:")?.split_once('?')?;
    let get = |name: &str| -> String {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    };
    match path {
        "forms" => Some(PageMessage::Forms(FormKind::parse(&get("k")).or_else(|| FormKind::parse(&get("kind"))))),
        "cred" => {
            let kind = FormKind::parse(&get("k"))?;
            let password = Zeroizing::new(get("p"));
            if password.is_empty() {
                return None;
            }
            Some(PageMessage::Login {
                url: get("url"),
                username: get("u"),
                password,
                kind,
            })
        }
        _ => None,
    }
}

// ---- the page ----

fn field<'a>(pairs: &'a [(String, String)], name: &str) -> &'a str {
    pairs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

/// A path typed in a form: trimmed, without the quotes Windows' "Copy as
/// path" adds.
fn path_field(text: &str) -> Option<PathBuf> {
    let t = text.trim().trim_matches('"').trim();
    (!t.is_empty()).then(|| PathBuf::from(t))
}

/// Suggested vault and key-file locations: the per-user config folder (made
/// here, so creating a vault there works on a fresh install), never the
/// working directory.
fn default_data_path(name: &str) -> String {
    let Ok(dir) = crate::storage::config_dir() else {
        return String::new();
    };
    let _ = crate::storage::private_dir(&dir);
    dir.join(name).display().to_string()
}

fn default_vault_path() -> String {
    default_data_path("browsecraft-passwords.kdbx")
}

fn default_keyfile_path() -> String {
    default_data_path("browsecraft-passwords.keyx")
}

fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("No clipboard: {e}"))?;
    clipboard.set_text(text).map_err(|e| format!("Could not copy: {e}"))?;
    let copied = Zeroizing::new(text.to_string());
    // Do not leave the password on the clipboard: clear it if it is still ours.
    std::thread::spawn(move || {
        std::thread::sleep(CLIPBOARD_CLEAR);
        if let Ok(mut clipboard) = arboard::Clipboard::new()
            && clipboard.get_text().is_ok_and(|now| *Zeroizing::new(now) == *copied)
        {
            let _ = clipboard.clear();
        }
    });
    Ok(())
}

/// Applies a submission of the passwords page: the query of a link, or the
/// body of a form. Only the page's own, freshly rendered forms and links
/// carry the token, so nothing else can unlock, add or delete.
pub fn handle(query: &str, post_body: Option<&str>) {
    let Some(pairs) = FORM_TOKEN.take(post_body.unwrap_or(query)) else {
        return;
    };
    let action = field(&pairs, "action").to_string();
    let mut s = state();
    s.message = None;
    match action.as_str() {
        "create-password" => {
            let password = field(&pairs, "password");
            if password != field(&pairs, "confirm") {
                return s.fail("The two passwords do not match.");
            }
            let Some(path) = path_field(field(&pairs, "path")) else {
                return s.fail("Choose where to save the vault.");
            };
            match Vault::create(&path, NewProtection::Password(password.to_string())) {
                Ok(vault) => {
                    s.unlocked(vault, &path, Mode::Password, None);
                    s.ok("Vault created and unlocked. It is a standard KeePass file you can also open in KeePassXC.");
                }
                Err(e) => s.fail(e),
            }
        }
        "create-keyfile" => {
            let (Some(path), Some(key)) = (path_field(field(&pairs, "path")), path_field(field(&pairs, "keyfile")))
            else {
                return s.fail("Choose where to save the vault and the key file.");
            };
            match Vault::create(&path, NewProtection::KeyFile(key.clone())) {
                Ok(vault) => {
                    s.unlocked(vault, &path, Mode::KeyFile, Some(key));
                    s.ok("Vault created and unlocked. Keep the key file safe and apart from the vault: without it nothing can open the vault.");
                }
                Err(e) => s.fail(e),
            }
        }
        "open" | "unlock" => {
            let path = if action == "open" {
                path_field(field(&pairs, "path"))
            } else {
                s.vault_path.clone()
            };
            let Some(path) = path else {
                return s.fail("Choose the vault file to open.");
            };
            let keyfile = path_field(field(&pairs, "keyfile"));
            let password = field(&pairs, "password");
            let (unlock, mode) = match (&keyfile, password.is_empty()) {
                (Some(k), _) => (Unlock::KeyFile(k.clone()), Mode::KeyFile),
                (None, false) => (Unlock::Password(password.to_string()), Mode::Password),
                (None, true) => {
                    return s.fail("Enter the password, or the path of the key file.");
                }
            };
            match Vault::open(&path, &unlock) {
                Ok(vault) => {
                    s.unlocked(vault, &path, mode, keyfile);
                    s.ok("Vault unlocked.");
                }
                Err(e) => s.fail(e),
            }
        }
        "lock" => {
            s.lock_now();
            s.ok("Vault locked.");
        }
        "forget" => {
            s.lock_now();
            s.vault_path = None;
            s.keyfile_path = None;
            save_config(&s);
            s.ok("The vault was disconnected. Its file was not touched.");
        }
        _ => {
            let Some(vault) = s.vault.as_mut() else {
                return s.fail("Unlock the vault first.");
            };
            let id = field(&pairs, "id").to_string();
            match action.as_str() {
                "add" => {
                    let mut password = Zeroizing::new(field(&pairs, "password").to_string());
                    if password.is_empty() && !field(&pairs, "generate").is_empty() {
                        password = Zeroizing::new(vault::generate_password(20));
                    }
                    let url = field(&pairs, "url").trim();
                    let url = if url.is_empty() || url.contains("://") {
                        url.to_string()
                    } else {
                        format!("https://{url}")
                    };
                    match vault.add(field(&pairs, "title"), &url, field(&pairs, "username"), &password) {
                        Ok(_) => s.ok("Login saved."),
                        Err(e) => s.fail(e),
                    }
                }
                "reveal" => s.revealed = Some(id),
                "hide" => s.revealed = None,
                "ask-delete" => s.confirm_delete = Some(id),
                "cancel" => s.confirm_delete = None,
                "delete" => {
                    match vault.delete(&id) {
                        Ok(()) => s.ok("Login deleted."),
                        Err(e) => s.fail(e),
                    }
                    s.confirm_delete = None;
                }
                "copy" => match vault.password(&id) {
                    Some(password) => match copy_to_clipboard(&password) {
                        Ok(()) => s.ok("Password copied. It leaves the clipboard in 30 seconds."),
                        Err(e) => s.fail(e),
                    },
                    None => s.fail("That login no longer exists."),
                },
                _ => {}
            }
            s.touch();
        }
    }
}

fn form(token: &str, action: &str, inner: &str) -> String {
    format!(
        "<form method=\"post\" action=\"browsecraft://passwords\">\
         <input type=\"hidden\" name=\"token\" value=\"{token}\">\
         <input type=\"hidden\" name=\"action\" value=\"{action}\">{inner}</form>"
    )
}

fn input(label: &str, name: &str, kind: &str, value: &str, placeholder: &str) -> String {
    format!(
        "<label class=\"l\">{}</label><input class=\"field\" type=\"{kind}\" name=\"{name}\" value=\"{}\" placeholder=\"{}\">",
        internal::escape(label),
        internal::escape(value),
        internal::escape(placeholder)
    )
}

fn card(heading: &str, body: &str) -> String {
    format!(
        "<div class=\"card\"><h2>{}</h2><div style=\"padding:4px 0 10px\">{body}</div></div>",
        internal::escape(heading)
    )
}

fn button(label: &str) -> String {
    format!("<button class=\"btn\" type=\"submit\">{}</button>", internal::escape(label))
}

/// The `browsecraft://passwords` page for the current state. Each call
/// issues a new form token.
pub fn page_html() -> String {
    let token = match FORM_TOKEN.issue() {
        Ok(token) => token,
        Err(error) => return internal::unavailable_page("Passwords", &error),
    };
    let mut s = state();
    let message = s.message.take();
    let mut body = String::from("<h1>Passwords</h1>");

    let (lead, lead_class) = match (&s.vault, &s.vault_path) {
        (Some(_), _) => ("Unlocked. It locks itself after 15 minutes without use.", "sub"),
        (None, Some(_)) => ("Locked. Unlock your vault to fill and save logins.", "sub"),
        (None, None) => (
            "Save logins in a KeePass (.kdbx) vault that also opens in KeePassXC.",
            "sub",
        ),
    };
    body += &format!("<p class=\"{lead_class}\">{}</p>", internal::escape(lead));
    if let Some((ok, text)) = message {
        body += &format!(
            "<div class=\"card\"><div class=\"row first\"><div class=\"{}\">{}</div></div></div>",
            if ok { "ok" } else { "bad" },
            internal::escape(&text)
        );
    }

    let link = |action: &str, id: &str| {
        format!(
            "browsecraft://passwords?token={token}&amp;action={action}&amp;id={}",
            internal::escape(&internal::encode(id))
        )
    };

    if let Some(vault) = &s.vault {
        let path = vault.path().display().to_string();
        body += &format!(
            "<div class=\"card\"><div class=\"row first\"><div class=\"grow\"><div class=\"t\">Vault</div>\
             <div class=\"d\" style=\"word-break:break-all\">{}</div></div>{}</div></div>",
            internal::escape(&path),
            form(&token, "lock", &button("Lock now"))
        );
        let logins = vault.credentials();
        let mut list = String::new();
        if logins.is_empty() {
            list.push_str("<div class=\"empty\">No saved logins yet. Sign in to a site and BrowseCraft will offer to save it.</div>");
        }
        for (i, login) in logins.iter().enumerate() {
            let letter: String = login
                .title
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_else(|| "?".into());
            let shown = if s.revealed.as_deref() == Some(login.id.as_str()) {
                vault.password(&login.id).map(|p| {
                    format!(
                        "<div class=\"url\">Password: <b>{}</b> &nbsp; <a href=\"{}\">Hide</a></div>",
                        internal::escape(&p),
                        link("hide", &login.id)
                    )
                })
            } else {
                None
            }
            .unwrap_or_default();
            let actions = if s.confirm_delete.as_deref() == Some(login.id.as_str()) {
                format!(
                    "<a class=\"btn danger\" href=\"{}\">Delete</a> <a class=\"btn ghost\" href=\"{}\">Cancel</a>",
                    link("delete", &login.id),
                    link("cancel", &login.id)
                )
            } else {
                format!(
                    "<a class=\"btn ghost\" href=\"{}\">Show</a> <a class=\"btn ghost\" href=\"{}\">Copy</a> \
                     <a class=\"btn ghost\" href=\"{}\">Delete</a>",
                    link("reveal", &login.id),
                    link("copy", &login.id),
                    link("ask-delete", &login.id)
                )
            };
            list.push_str(&format!(
                "<div class=\"item{first}\"><div class=\"ico\">{letter}</div>\
                 <div class=\"main\"><span class=\"t\">{title}</span>\
                 <div class=\"url\">{user}</div><div class=\"url\">{url}</div>{shown}</div>\
                 <div>{actions}</div></div>",
                first = if i == 0 { " first" } else { "" },
                letter = internal::escape(&letter),
                title = internal::escape(&login.title),
                user = internal::escape(&login.username),
                url = internal::escape(&login.url),
            ));
        }
        body += &format!("<div class=\"card\">{list}</div>");
        body += &card(
            "Add a login",
            &form(
                &token,
                "add",
                &format!(
                    "{}{}{}{}<label class=\"l\"><input type=\"checkbox\" name=\"generate\" value=\"1\"> \
                     Generate a strong password if the field is empty</label><div style=\"height:10px\"></div>{}",
                    input("Website", "url", "text", "", "example.com"),
                    input("Title (optional)", "title", "text", "", ""),
                    input("User name", "username", "text", "", ""),
                    input("Password", "password", "password", "", ""),
                    button("Save login")
                ),
            ),
        );
    } else if let Some(path) = &s.vault_path {
        let file = path.display().to_string();
        let (unlock_fields, hint) = match s.mode {
            Mode::Password => (input("Password", "password", "password", "", ""), "This vault opens with a password."),
            Mode::KeyFile => (
                input(
                    "Key file path",
                    "keyfile",
                    "text",
                    &s.keyfile_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                    "",
                ),
                "This vault opens with a key file.",
            ),
        };
        body += &card(
            "Unlock your vault",
            &format!(
                "<div class=\"note\" style=\"word-break:break-all\">{}<br>{}</div>{}",
                internal::escape(&file),
                hint,
                form(&token, "unlock", &format!("{unlock_fields}{}", button("Unlock")))
            ),
        );
        body += &card(
            "Use a different vault",
            &format!(
                "<div class=\"note\">Disconnects this vault from BrowseCraft. The file is not deleted.</div>{}",
                form(&token, "forget", &button("Disconnect vault"))
            ),
        );
    } else {
        body += &card(
            "Create a vault with a password",
            &form(
                &token,
                "create-password",
                &format!(
                    "{}{}{}{}",
                    input("Vault file", "path", "text", &default_vault_path(), ""),
                    input("Password (at least 8 characters)", "password", "password", "", ""),
                    input("Repeat the password", "confirm", "password", "", ""),
                    button("Create vault")
                ),
            ),
        );
        body += &card(
            "Create a vault with a key file",
            &format!(
                "<div class=\"note\">A key file is a small file only you have. The vault cannot be opened without it, \
                 so keep it on a USB stick or another safe place, apart from the vault. If you lose it, the vault \
                 is gone for good.</div>{}",
                form(
                    &token,
                    "create-keyfile",
                    &format!(
                        "{}{}{}",
                        input("Vault file", "path", "text", &default_vault_path(), ""),
                        input("Key file to create", "keyfile", "text", &default_keyfile_path(), ""),
                        button("Create vault and key file")
                    ),
                )
            ),
        );
        body += &card(
            "Open an existing KeePass vault",
            &format!(
                "<div class=\"note\">Use the password, or the key file, whichever protects it. Leave the other empty.</div>{}",
                form(
                    &token,
                    "open",
                    &format!(
                        "{}{}{}{}",
                        input("Vault file (.kdbx)", "path", "text", "", ""),
                        input("Password", "password", "password", "", ""),
                        input("Key file path", "keyfile", "text", "", ""),
                        button("Open vault")
                    ),
                )
            ),
        );
    }

    if !s.never.is_empty() {
        body += &format!(
            "<p class=\"sub\">BrowseCraft will not offer to save logins on: {}.</p>",
            internal::escape(&s.never.join(", "))
        );
    }
    internal::page("Passwords", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state is a process global backed by a file.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn reset() {
        *STATE.lock().unwrap_or_else(|p| p.into_inner()) = State::new();
        let _ = std::fs::remove_file(config_path().unwrap());
    }

    /// The token of a freshly rendered page.
    fn token() -> String {
        let html = page_html();
        let marker = "name=\"token\" value=\"";
        let start = html.find(marker).or_else(|| html.find("token=")).unwrap();
        let after = &html[start + if html[start..].starts_with("token=") { 6 } else { marker.len() }..];
        after[..32].to_string()
    }

    fn submit(action: &str, fields: &[(&str, &str)]) {
        let mut body = url::form_urlencoded::Serializer::new(String::new());
        body.append_pair("token", &token());
        body.append_pair("action", action);
        for (k, v) in fields {
            body.append_pair(k, v);
        }
        handle("", Some(&body.finish()));
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("browsecraft-pw-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn js_strings_are_safe_to_embed() {
        assert_eq!(js_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        assert_eq!(js_string("</script><!--"), "\"\\u003c/script\\u003e\\u003c!--\"");
        assert_eq!(js_string("\u{2028}"), "\"\\u2028\"");
        assert_eq!(js_string("tab\there"), "\"tab\\there\"");
    }

    #[test]
    fn create_lock_unlock_with_a_password() {
        let _g = SERIAL.lock().unwrap();
        reset();
        let dir = temp("pw");
        let vault = dir.join("v.kdbx");
        let vault_s = vault.display().to_string();
        assert_eq!(status(), Status::NoVault);

        // A mismatched confirmation changes nothing.
        submit("create-password", &[("path", &vault_s), ("password", "long password 1"), ("confirm", "different one")]);
        assert_eq!(status(), Status::NoVault);
        assert!(page_html().contains("do not match"));

        submit("create-password", &[("path", &vault_s), ("password", "long password 1"), ("confirm", "long password 1")]);
        assert_eq!(status(), Status::Unlocked { logins: 0 });
        assert!(vault.is_file());

        submit("lock", &[]);
        assert_eq!(status(), Status::Locked);
        submit("unlock", &[("password", "wrong wrong wrong")]);
        assert_eq!(status(), Status::Locked);
        assert!(page_html().contains("Wrong password."));
        submit("unlock", &[("password", "long password 1")]);
        assert_eq!(status(), Status::Unlocked { logins: 0 });
        let _ = std::fs::remove_dir_all(dir);
        reset();
    }

    #[test]
    fn create_and_unlock_with_a_keyfile() {
        let _g = SERIAL.lock().unwrap();
        reset();
        let dir = temp("kf");
        let (vault, key) = (dir.join("v.kdbx"), dir.join("v.keyx"));
        submit("create-keyfile", &[("path", &vault.display().to_string()), ("keyfile", &key.display().to_string())]);
        assert_eq!(status(), Status::Unlocked { logins: 0 });
        assert!(key.is_file());
        submit("lock", &[]);
        // The page asks for the key file now, prefilled from the settings.
        let html = page_html();
        assert!(html.contains("key file") && html.contains(&key.display().to_string()));
        submit("unlock", &[("password", "anything")]);
        assert_eq!(status(), Status::Locked, "a password does not open a key-file vault");
        submit("unlock", &[("keyfile", &format!("\"{}\"", key.display()))]);
        assert_eq!(status(), Status::Unlocked { logins: 0 });
        let _ = std::fs::remove_dir_all(dir);
        reset();
    }

    #[test]
    fn settings_survive_a_restart_but_the_vault_stays_locked() {
        let _g = SERIAL.lock().unwrap();
        reset();
        let dir = temp("cfg");
        let vault = dir.join("v.kdbx");
        submit("create-password", &[("path", &vault.display().to_string()), ("password", "long password 1"), ("confirm", "long password 1")]);
        offer_login("https://never.test/", "u", "p", FormKind::Login);
        never_offer_for_site();
        // A new process: empty state, then load the settings file.
        *STATE.lock().unwrap() = State::new();
        load();
        assert_eq!(status(), Status::Locked);
        assert!(!offer_login("https://never.test/login", "u", "p", FormKind::Login), "never list persists");
        let _ = std::fs::remove_dir_all(dir);
        reset();
    }

    #[test]
    fn only_the_rendered_page_can_unlock_add_or_delete() {
        let _g = SERIAL.lock().unwrap();
        reset();
        let dir = temp("tok");
        let vault_path = dir.join("v.kdbx");
        submit("create-password", &[("path", &vault_path.display().to_string()), ("password", "long password 1"), ("confirm", "long password 1")]);
        // Forged submissions without or with a wrong token do nothing.
        handle("", Some("action=add&url=evil.test&username=x&password=y"));
        handle("action=lock", None);
        handle("", Some(&format!("token={}&action=lock", "0".repeat(32))));
        assert_eq!(status(), Status::Unlocked { logins: 0 });
        // A used token cannot be replayed.
        let t = token();
        handle("", Some(&format!("token={t}&action=lock")));
        assert_eq!(status(), Status::Locked);
        handle("", Some(&format!("token={t}&action=unlock&password=long password 1")));
        assert_eq!(status(), Status::Locked);
        let _ = std::fs::remove_dir_all(dir);
        reset();
    }

    #[test]
    fn add_show_copy_fill_and_delete_a_login() {
        let _g = SERIAL.lock().unwrap();
        reset();
        let dir = temp("crud");
        submit("create-password", &[("path", &dir.join("v.kdbx").display().to_string()), ("password", "long password 1"), ("confirm", "long password 1")]);
        submit("add", &[("url", "example.com"), ("title", "<b>Ex</b>"), ("username", "alice"), ("password", "pw\"quote")]);
        assert_eq!(status(), Status::Unlocked { logins: 1 });
        let html = page_html();
        assert!(html.contains("&lt;b&gt;Ex&lt;/b&gt;") && !html.contains("<b>Ex</b>"), "titles are escaped");
        assert!(!html.contains("pw&quot;quote"), "passwords are not shown by default");

        let id = logins_for("https://www.example.com/login")[0].id.clone();
        submit("reveal", &[("id", &id)]);
        assert!(page_html().contains("pw&quot;quote"));
        submit("hide", &[("id", &id)]);
        assert!(!page_html().contains("pw&quot;quote"));

        // Filling only works for the login's own site.
        let script = fill_script(&id, "https://login.example.com/").unwrap();
        assert!(script.contains("\"alice\"") && script.contains("\"pw\\\"quote\""));
        assert_eq!(script.capacity(), script.len(), "built without reallocating");
        assert!(fill_script(&id, "https://evil.test/").is_none());
        assert!(fill_script(&id, "https://example.com.evil.test/").is_none());
        assert!(fill_script("no-such-id", "https://example.com/").is_none());

        submit("ask-delete", &[("id", &id)]);
        assert!(page_html().contains("class=\"btn danger\""));
        submit("delete", &[("id", &id)]);
        assert_eq!(status(), Status::Unlocked { logins: 0 });
        let _ = std::fs::remove_dir_all(dir);
        reset();
    }

    #[test]
    fn generated_password_on_add() {
        let _g = SERIAL.lock().unwrap();
        reset();
        let dir = temp("gen");
        submit("create-password", &[("path", &dir.join("v.kdbx").display().to_string()), ("password", "long password 1"), ("confirm", "long password 1")]);
        submit("add", &[("url", "https://gen.test/"), ("username", "g"), ("generate", "1")]);
        let id = logins_for("https://gen.test/")[0].id.clone();
        let script = fill_script(&id, "https://gen.test/").unwrap();
        assert!(script.len() > 40, "a generated password was saved: {}", script.as_str());
        submit("add", &[("url", "https://none.test/"), ("username", "g")]);
        assert_eq!(status(), Status::Unlocked { logins: 1 }, "no password and no generate: nothing saved");
        let _ = std::fs::remove_dir_all(dir);
        reset();
    }

    #[test]
    fn offers_cover_new_same_changed_and_never() {
        let _g = SERIAL.lock().unwrap();
        reset();
        // Locked: the offer waits.
        assert!(offer_login("https://shop.test/signup", "me@x.test", "pw-12345", FormKind::SignUp));
        let offer = current_offer().unwrap();
        assert_eq!((offer.host.as_str(), offer.url.as_str(), offer.kind), ("shop.test", "https://shop.test/", FormKind::SignUp));
        assert_eq!(save_offer().err().as_deref(), Some("Unlock your vault first."));
        assert!(!format!("{offer:?}").contains("pw-12345"), "Debug hides the password");

        let dir = temp("offer");
        submit("create-password", &[("path", &dir.join("v.kdbx").display().to_string()), ("password", "long password 1"), ("confirm", "long password 1")]);
        // Now saving works, and the login is stored for the whole site.
        assert_eq!(save_offer().unwrap(), "Saved the login for shop.test.");
        assert!(current_offer().is_none());
        assert_eq!(logins_for("https://www.shop.test/cart").len(), 1);
        // Same login again: no offer. Changed password: an update offer.
        assert!(!offer_login("https://shop.test/login", "me@x.test", "pw-12345", FormKind::Login));
        assert!(offer_login("https://shop.test/login", "me@x.test", "new-pw-999", FormKind::Login));
        assert!(current_offer().unwrap().update_id.is_some());
        assert_eq!(save_offer().unwrap(), "Updated the password for shop.test.");
        assert_eq!(logins_for("https://shop.test/").len(), 1);
        // A different user on the same site is a new login.
        assert!(offer_login("https://shop.test/login", "other@x.test", "pw-0000000", FormKind::Login));
        assert!(current_offer().unwrap().update_id.is_none());
        // Never for this site.
        never_offer_for_site();
        assert!(current_offer().is_none());
        assert!(!offer_login("https://shop.test/login", "third@x.test", "pw-1111111", FormKind::Login));
        // Empty passwords and non-sites never make an offer.
        assert!(!offer_login("https://ok.test/", "u", "", FormKind::Login));
        assert!(!offer_login("about:blank", "u", "pw", FormKind::Login));
        let _ = std::fs::remove_dir_all(dir);
        reset();
    }

    #[test]
    fn offers_belong_to_their_site_and_expire() {
        let _g = SERIAL.lock().unwrap();
        reset();
        assert!(offer_login("https://a.test/", "u", "pw-1234567", FormKind::Login));
        drop_offer_unless_site("https://a.test/other");
        assert!(current_offer().is_some());
        drop_offer_unless_site("https://b.test/");
        assert!(current_offer().is_none());
        assert!(offer_login("https://a.test/", "u", "pw-1234567", FormKind::Login));
        dismiss_offer();
        assert!(current_offer().is_none());
        reset();
    }

    #[test]
    fn an_idle_vault_locks_itself() {
        let _g = SERIAL.lock().unwrap();
        reset();
        let dir = temp("idle");
        submit("create-password", &[("path", &dir.join("v.kdbx").display().to_string()), ("password", "long password 1"), ("confirm", "long password 1")]);
        assert_eq!(status(), Status::Unlocked { logins: 0 });
        STATE.lock().unwrap().last_used = Some(Instant::now() - AUTO_LOCK - Duration::from_secs(1));
        assert_eq!(status(), Status::Locked);
        assert!(page_html().contains("locked itself"));
        let _ = std::fs::remove_dir_all(dir);
        reset();
    }

    #[test]
    fn forgetting_a_vault_keeps_the_file() {
        let _g = SERIAL.lock().unwrap();
        reset();
        let dir = temp("forget");
        let vault = dir.join("v.kdbx");
        submit("create-password", &[("path", &vault.display().to_string()), ("password", "long password 1"), ("confirm", "long password 1")]);
        submit("forget", &[]);
        assert_eq!(status(), Status::NoVault);
        assert!(vault.is_file());
        // And it can be opened again from the page.
        submit("open", &[("path", &vault.display().to_string()), ("password", "long password 1")]);
        assert_eq!(status(), Status::Unlocked { logins: 0 });
        let _ = std::fs::remove_dir_all(dir);
        reset();
    }

    #[test]
    fn page_messages_are_parsed_defensively() {
        assert!(matches!(parse_page_message("bc:forms?kind=signup"), Some(PageMessage::Forms(Some(FormKind::SignUp)))));
        assert!(matches!(parse_page_message("bc:forms?kind=none"), Some(PageMessage::Forms(None))));
        let Some(PageMessage::Login { url, username, password, kind }) =
            parse_page_message("bc:cred?k=login&u=a%40b.test&p=p%26w%3D1&url=https%3A%2F%2Fx.test%2F")
        else {
            panic!("login not parsed");
        };
        assert_eq!((url.as_str(), username.as_str(), password.as_str(), kind), ("https://x.test/", "a@b.test", "p&w=1", FormKind::Login));
        assert!(parse_page_message("bc:cred?k=login&u=a&p=").is_none(), "empty password");
        assert!(parse_page_message("bc:cred?k=bogus&u=a&p=x").is_none(), "unknown kind");
        assert!(parse_page_message("bc:other?x=1").is_none());
        assert!(parse_page_message("hello").is_none());
        assert!(parse_page_message(&format!("bc:cred?k=login&p={}", "a".repeat(5000))).is_none(), "oversized");
        let dbg = format!("{:?}", parse_page_message("bc:cred?k=login&u=a&p=secret1").unwrap());
        assert!(!dbg.contains("secret1"));
    }

    #[test]
    fn the_page_script_is_bundled() {
        assert!(PAGE_SCRIPT.contains("__bcFill") && PAGE_SCRIPT.contains("'bc:'"));
    }
}
