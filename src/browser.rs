//! The browser shell: window, toolbar, tab sidebar, history, input, forms and
//! the render loop.
//!
//! Two kinds of pages live side by side. `about:`, `view-source:` and `browsecraft:`
//! URLs go through BrowseCraft's own engine (loader -> html/css -> style ->
//! layout -> paint); everything else renders in an embedded WebView2 pane
//! (`wry`) exposed below the toolbar and beside the tab sidebar. The chrome
//! (toolbar and tab sidebar) is drawn either by the engine into a softbuffer
//! frame, or by egui on wgpu when a GPU is available.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use softbuffer::{Context, Surface};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CursorIcon, Fullscreen, Theme, Window, WindowId};
use wry::dpi::{
    PhysicalPosition as WryPhysicalPosition, PhysicalSize as WryPhysicalSize, Position as WryPosition,
    Size as WrySize,
};
use wry::{NewWindowResponse, PageLoadEvent, WebView, WebViewBuilder};

use crate::adblock;
use crate::address;
use crate::bookmarks;
use crate::css::Color;
use crate::dom::{Document, NodeId};
use crate::egui_chrome::{self, EguiChrome};
use crate::flags::{self, Chrome, TabsPosition};
use crate::font::{FontKey, Fonts};
use crate::history;
use crate::icons;
use crate::internal;
use crate::layout::{self, FormState, Layout, LayoutInput, Rect};
use crate::loader::{self, LoadEvent, Page, Request};
use crate::paint::{self, Canvas, Image};
use crate::passwords::{self, FormKind, PageMessage};
use crate::style::{self, Style, StyleContext};
use crate::textfield::TextField;
use crate::vault;

pub const HOME_URL: &str = "https://www.startpage.com/";
/// What a new tab opens: the built-in start page (`src/home.html`).
const NEW_TAB_URL: &str = "about:home";
const TOOLBAR_HEIGHT: f32 = 44.0;
const SIDEBAR_WIDTH: f32 = 220.0;
/// Height of the status strip that shows a notice under the page.
const NOTICE_HEIGHT: f32 = 30.0;
const BOOKMARKS_BAR_HEIGHT: f32 = 34.0;
const SAVE_BAR_HEIGHT: f32 = 42.0;
const NOTICE_LIFETIME: Duration = Duration::from_secs(3);
/// Pixels scrolled per mouse-wheel line or arrow-key press, in CSS px.
const SCROLL_STEP: f32 = 48.0;
const ZOOM_STEPS: [f32; 13] = [
    0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 3.0,
];

/// Every keyboard shortcut, shown by the Shift+K cheatsheet.
const SHORTCUTS: &[(&str, &str)] = &[
    ("Shift + K", "Show or hide this cheatsheet"),
    ("Ctrl + T", "New tab"),
    ("Ctrl + W", "Close the current tab"),
    ("Ctrl + Tab", "Switch to the next tab"),
    ("Ctrl + L", "Focus the address bar"),
    ("Ctrl + S", "Select the address bar to type"),
    ("Ctrl + Left", "Go to the previous page"),
    ("Ctrl + Right", "Go to the next page"),
    ("Ctrl + H", "Open the history page"),
    ("Ctrl + D", "Bookmark this page, or remove its bookmark"),
    ("Ctrl + Shift + B", "Show or hide the bookmarks bar"),
    ("Ctrl + Shift + O", "Open the bookmarks page"),
    ("F11", "Fullscreen"),
    ("Ctrl + R", "Reload the page"),
    ("Ctrl + Shift + S", "Save a screenshot to Pictures"),
    ("Ctrl + + / \u{2212} / 0", "Zoom in / out / reset"),
    ("Ctrl + U", "View the page source"),
    ("Ctrl + Q", "Quit BrowseCraft"),
];

#[derive(Clone, Debug)]
pub enum UserMessage {
    /// A background engine load made progress (generation, event).
    Load(u64, LoadEvent),
    /// The WebView2 page changed its title.
    WebTitle(String),
    /// The WebView2 page started loading this URL.
    WebStart(String),
    /// The WebView2 page finished loading this URL.
    WebLoad(String),
    /// The WebView2 pane wants to open this URL (pop-ups, blocked pages).
    WebOpen(String),
    /// A shortcut pressed inside the WebView2 pane (`AcceleratorKeyPressed`).
    #[cfg_attr(not(windows), allow(dead_code))]
    WebKey(Shortcut),
    /// A page inside the WebView2 pane entered or left element fullscreen
    /// (a video's fullscreen button, `requestFullscreen()`).
    #[cfg_attr(not(windows), allow(dead_code))]
    WebFullscreen(bool),
    /// The page script reported a form or a submitted login.
    Page(PageMessage),
    /// Stop the current load.
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Focus {
    Page,
    Address,
    Input(NodeId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Button {
    Back,
    Forward,
    Reload,
    Home,
    Theme,
    Flags,
}

impl Button {
    const ALL: [Button; 6] = [
        Button::Back,
        Button::Forward,
        Button::Reload,
        Button::Home,
        Button::Theme,
        Button::Flags,
    ];
}

/// What a mouse drag is currently selecting.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    None,
    Address,
    Input,
}

/// What is under the pointer on an engine page.
enum Hit {
    None,
    Link(Rc<str>),
    Control(NodeId),
}

/// Chrome actions collected while the egui frame is built and applied after.
enum Action {
    Back,
    Forward,
    Reload,
    Stop,
    Home,
    Theme,
    Flags,
    Navigate(String),
    RestoreAddress,
    NewTab,
    SwitchTab(usize),
    CloseTab(usize),
    DuplicateTab(usize),
    CloseOtherTabs(usize),
    ToggleBookmark,
    OpenBookmark(String),
    OpenInNewTab(String),
    RemoveBookmark(String),
    BookmarksPage,
    FillLogin(String),
    SuggestPassword,
    SaveLogin,
    DismissOffer,
    NeverSave,
    LockVault,
    PasswordsPage,
}

struct HistoryEntry {
    request: Request,
    scroll: f32,
}

/// The saved state of a tab that is not the active one.
#[derive(Default)]
struct TabState {
    history: Vec<HistoryEntry>,
    index: usize,
    tab: Option<Tab>,
    web_page: bool,
    web_url: String,
    title: String,
}

struct Tab {
    page: Page,
    images: HashMap<String, Rc<Image>>,
    form: FormState,
    styles: Vec<Rc<Style>>,
    style_key: Option<(Vec<bool>, f32)>,
    /// The viewport width and zoom `style_key` was last checked for, so the
    /// media queries are not re-evaluated on every frame.
    styled_at: (f32, f32),
    layout: Option<Layout>,
    layout_key: (f32, f32),
    scroll_x: f32,
    scroll_y: f32,
    pending_scroll: Option<PendingScroll>,
}

enum PendingScroll {
    To(f32),
    Fragment(String),
}

struct Gfx {
    window: Arc<Window>,
    /// `None` while egui owns the window surface.
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    /// Kept alive for as long as the softbuffer surface.
    _context: Option<Context<Arc<Window>>>,
}

pub struct Browser {
    gfx: Option<Gfx>,
    fonts: Fonts,
    proxy: EventLoopProxy<UserMessage>,
    generation: Arc<AtomicU64>,
    tabs: Vec<TabState>,
    active: usize,
    history: Vec<HistoryEntry>,
    index: usize,
    tab: Option<Tab>,
    loading: bool,
    start_url: String,
    webview: Option<WebView>,
    web_page: bool,
    web_url: String,
    title: String,
    address: TextField,
    address_scroll: f32,
    input: TextField,
    focus: Focus,
    modifiers: ModifiersState,
    mouse: (f32, f32),
    hover_button: Option<Button>,
    hover_tab: Option<usize>,
    tabs_scroll: usize,
    hover_link: Option<String>,
    user_zoom: f32,
    clipboard: Option<arboard::Clipboard>,
    last_click: (Instant, f32, f32),
    frame: Vec<u32>,
    egui: Option<EguiChrome>,
    page_tex: Option<egui::TextureHandle>,
    egui_page: Option<Rect>,
    cheatsheet: bool,
    notice: Option<(String, Instant)>,
    // ---- state added when rebuilding the shell ----
    drag: Drag,
    quit: bool,
    epoch: Instant,
    last_dark: Option<bool>,
    /// A WebView2 navigation we started ourselves is in flight, so its final
    /// URL replaces the history entry instead of adding one.
    web_expect: bool,
    /// The URL the WebView was showing when we started that navigation.
    /// Until it commits, late events for this URL belong to the old page.
    web_leaving: String,
    /// Our navigation has started in the WebView (so titles are its own).
    web_committed: bool,
    web_visible: bool,
    /// When the last screenshot was saved, to allow at most one a second.
    last_screenshot: Option<Instant>,
    web_bounds: Option<(i32, i32, u32, u32)>,
    /// Scroll offset to restore when the next engine page arrives.
    pending_restore: Option<f32>,
    /// Where the page sits inside `frame`: x, y, width, height, and the
    /// frame's row stride (all in pixels).
    frame_page: (usize, usize, usize, usize, usize),
    egui_focus_address: bool,
    egui_select_all: bool,
    egui_address_focused: bool,
    /// A web page is fullscreen: the window fills the screen and only the
    /// page is shown.
    web_fullscreen: bool,
    /// The kind of password form the web page has right now, if any.
    page_forms: Option<FormKind>,
    /// An egui menu or popup is open. The WebView2 pane is a native window
    /// that paints over anything egui draws, so while a menu is open the pane
    /// is hidden and a snapshot of the page is shown in its place.
    egui_popup_open: bool,
    /// The page as it looked when the menu opened (see `egui_popup_open`).
    popup_snapshot: Option<egui::TextureHandle>,
    /// The window was already fullscreen (F11 / the OS) when the page asked,
    /// so leaving page fullscreen must keep it that way.
    fullscreen_restore: bool,
}

// ---------------------------------------------------------------------------
// Pure helpers (no window needed, so they can be unit-tested)
// ---------------------------------------------------------------------------

/// `true` when `url` should render in the WebView2 pane. The engine keeps
/// `about:`, `view-source:` and `browsecraft:` pages for itself.
pub fn request_in_webview(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    !(lower.starts_with("about:") || lower.starts_with("view-source:") || lower.starts_with("browsecraft:"))
}

/// Whether a WebView event for `url` belongs to the page being left
/// (`leaving`) rather than to the navigation to `target` we are expecting.
/// A reload of the same URL is ours.
fn stale_web_event(expecting: bool, leaving: &str, target: &str, url: &str) -> bool {
    expecting && !leaving.is_empty() && url == leaving && url != target
}

/// The height, in physical pixels, of the bars between the toolbar and the
/// page: the bookmarks bar and the save-login bar. Zero on the engine chrome
/// (`egui_chrome` false), which has neither.
fn extra_bars_h(scale: f32, egui_chrome: bool, bookmarks_bar: bool, save_bar: bool) -> f32 {
    if !egui_chrome {
        return 0.0;
    }
    let mut h = 0.0;
    if bookmarks_bar {
        h += (BOOKMARKS_BAR_HEIGHT * scale).round();
    }
    if save_bar {
        h += (SAVE_BAR_HEIGHT * scale).round();
    }
    h
}

/// The request a history entry keeps. A form post to the passwords page
/// carries typed passwords, so the entry keeps a plain visit to the page
/// instead: Back, Forward and Reload never resubmit or retain them.
fn history_request(request: &Request) -> Request {
    match request {
        Request::Post(url, _) if url.starts_with("browsecraft://") => {
            Request::Get(url.split('?').next().unwrap_or(url).to_string())
        }
        other => other.clone(),
    }
}

/// Whether both URLs are the same `browsecraft://` page (ignoring the query).
fn same_internal_page(a: &str, b: &str) -> bool {
    let base = |u: &str| u.split('?').next().unwrap_or(u).trim_end_matches('/').to_string();
    a.starts_with("browsecraft://") && b.starts_with("browsecraft://") && base(a) == base(b)
}

/// Whether `url` is an http(s) URL, in any letter case (`HTTP://` too).
fn is_web_url(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Whether a web page's pop-up may open `url`.
fn popup_allowed(url: &str) -> bool {
    is_web_url(url)
}

/// Whether the web view may navigate its main frame to `url`. The browser
/// never loads its internal pages (`browsecraft:`, `about:`, `view-source:`)
/// in the web view, only the `about:blank` it uses to unload a page, so a
/// page script setting `location` to one of them is refused.
fn navigation_allowed(url: &str) -> bool {
    request_in_webview(url) || url.trim().eq_ignore_ascii_case("about:blank")
}

/// A page title fit for the window title and history: control and
/// bidirectional-formatting characters (which can disguise the rest) are
/// dropped, whitespace is collapsed, and it is at most 300 characters.
fn clean_title(title: &str) -> String {
    let words: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .filter(|c| !matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
        .collect();
    words
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(300)
        .collect()
}

/// The next (or previous) zoom level from [`ZOOM_STEPS`].
fn zoom_step(current: f32, up: bool) -> f32 {
    if up {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|&z| z > current + 0.001)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|&z| z < current - 0.001)
            .unwrap_or(ZOOM_STEPS[0])
    }
}

/// `browsecraft-YYYYMMDD-HHMMSS.png` for a Unix time (UTC).
fn screenshot_name(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("browsecraft-{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}.png")
}

/// Where screenshots go: the user's Pictures folder, else the working
/// directory.
/// Where screenshots go: the Pictures folder Windows reports (which follows
/// OneDrive and folder redirection), else `~/Pictures`.
fn pictures_dir() -> Result<PathBuf, String> {
    let dir = crate::capture::pictures_dir().or_else(|| {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(|home| PathBuf::from(home).join("Pictures"))
    });
    match dir {
        Some(dir) if std::fs::create_dir_all(&dir).is_ok() => Ok(dir),
        _ => Err("no Pictures folder to save to".to_string()),
    }
}

/// Creates a new screenshot file in `dir`, never replacing one: a second
/// screenshot in the same second gets `-2`, then `-3`, and so on.
fn create_screenshot_file(dir: &Path, secs: u64) -> std::io::Result<(PathBuf, std::fs::File)> {
    let name = screenshot_name(secs);
    let stem = name.trim_end_matches(".png");
    for n in 1..1000 {
        let path = if n == 1 {
            dir.join(&name)
        } else {
            dir.join(format!("{stem}-{n}.png"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many screenshots this second",
    ))
}

/// Shortens `text` with an ellipsis so it fits in `max_w` pixels.
fn ellipsize(fonts: &Fonts, key: FontKey, text: &str, size: f32, max_w: f32) -> String {
    if fonts.measure(key, text, size) <= max_w {
        return text.to_string();
    }
    let dots = fonts.measure(key, "\u{2026}", size);
    let mut out = String::new();
    let mut width = dots;
    for ch in text.chars() {
        width += fonts.advance(key, ch, size);
        if width > max_w {
            break;
        }
        out.push(ch);
    }
    out.push('\u{2026}');
    out
}

/// Byte index in `text` for a click at `x` pixels from the text's left edge.
fn caret_index(
    fonts: &Fonts,
    key: FontKey,
    size: f32,
    text: &str,
    password: bool,
    x: f32,
) -> usize {
    let mut acc = 0.0;
    for (i, ch) in text.char_indices() {
        let shown = if password { '\u{2022}' } else { ch };
        let w = fonts.advance(key, shown, size);
        if x < acc + w / 2.0 {
            return i;
        }
        acc += w;
    }
    text.len()
}

/// Pixel offset of byte index `at` from the text's left edge.
fn caret_x(fonts: &Fonts, key: FontKey, size: f32, text: &str, password: bool, at: usize) -> f32 {
    text[..at.min(text.len())]
        .chars()
        .map(|ch| fonts.advance(key, if password { '\u{2022}' } else { ch }, size))
        .sum()
}

fn is_char(key: &Key, c: &str) -> bool {
    matches!(key, Key::Character(s) if s.as_str().eq_ignore_ascii_case(c))
}

/// A browser-wide keyboard shortcut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shortcut {
    CycleTab(isize),
    Go(isize),
    Home,
    NewTab,
    CloseTab,
    FocusAddress,
    Screenshot,
    Reload,
    ViewSource,
    History,
    Bookmark,
    BookmarksBar,
    BookmarksPage,
    Fullscreen,
    Quit,
    Zoom(Option<bool>),
}

/// The browser-wide shortcut for `key` with these modifiers, if any. `typing`
/// is true while a text field has the keyboard, which keeps `Ctrl+Left` and
/// `Ctrl+Right` for moving by word.
fn shortcut_for(key: &Key, ctrl: bool, shift: bool, alt: bool, typing: bool) -> Option<Shortcut> {
    if ctrl && !alt {
        let found = match key {
            Key::Named(NamedKey::Tab) => Some(Shortcut::CycleTab(if shift { -1 } else { 1 })),
            Key::Named(NamedKey::ArrowLeft) if !typing => Some(Shortcut::Go(-1)),
            Key::Named(NamedKey::ArrowRight) if !typing => Some(Shortcut::Go(1)),
            Key::Character(c) => match c.to_ascii_lowercase().as_str() {
                "t" => Some(Shortcut::NewTab),
                "w" => Some(Shortcut::CloseTab),
                "l" => Some(Shortcut::FocusAddress),
                "s" if shift => Some(Shortcut::Screenshot),
                "s" => Some(Shortcut::FocusAddress),
                "r" => Some(Shortcut::Reload),
                "u" => Some(Shortcut::ViewSource),
                "h" => Some(Shortcut::History),
                "d" => Some(Shortcut::Bookmark),
                "b" if shift => Some(Shortcut::BookmarksBar),
                "o" if shift => Some(Shortcut::BookmarksPage),
                "q" => Some(Shortcut::Quit),
                "=" | "+" => Some(Shortcut::Zoom(Some(true))),
                "-" | "_" => Some(Shortcut::Zoom(Some(false))),
                "0" => Some(Shortcut::Zoom(None)),
                _ => return None,
            },
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    if alt && !ctrl {
        match key {
            Key::Named(NamedKey::ArrowLeft) => return Some(Shortcut::Go(-1)),
            Key::Named(NamedKey::ArrowRight) => return Some(Shortcut::Go(1)),
            Key::Named(NamedKey::Home) => return Some(Shortcut::Home),
            _ => {}
        }
    }
    match key {
        Key::Named(NamedKey::F5) => Some(Shortcut::Reload),
        Key::Named(NamedKey::F6) => Some(Shortcut::FocusAddress),
        Key::Named(NamedKey::F11) => Some(Shortcut::Fullscreen),
        _ => None,
    }
}

/// Maps a Win32 virtual-key code (`VK_*`) from WebView2's
/// `AcceleratorKeyPressed` event to the `winit` [`Key`] that `shortcut_for`
/// expects.
#[cfg_attr(not(windows), allow(dead_code))]
fn vk_to_key(vk: u32) -> Option<Key> {
    match vk {
        0x09 => Some(Key::Named(NamedKey::Tab)),        // VK_TAB
        0x24 => Some(Key::Named(NamedKey::Home)),       // VK_HOME
        0x25 => Some(Key::Named(NamedKey::ArrowLeft)),  // VK_LEFT
        0x27 => Some(Key::Named(NamedKey::ArrowRight)), // VK_RIGHT
        0x74 => Some(Key::Named(NamedKey::F5)),         // VK_F5
        0x75 => Some(Key::Named(NamedKey::F6)),         // VK_F6
        0x7A => Some(Key::Named(NamedKey::F11)),        // VK_F11
        0x30 | 0x60 => Some(Key::Character("0".into())), // '0', VK_NUMPAD0
        0x6B | 0xBB => Some(Key::Character("+".into())), // VK_ADD, VK_OEM_PLUS ('=' / '+')
        0x6D | 0xBD => Some(Key::Character("-".into())), // VK_SUBTRACT, VK_OEM_MINUS ('-' / '_')
        0x41..=0x5A => {
            let ch = char::from_u32(vk)?.to_ascii_lowercase();
            Some(Key::Character(ch.to_string().into()))
        }
        _ => None,
    }
}

/// Maps a Win32 virtual-key code and modifier state from WebView2's native
/// `AcceleratorKeyPressed` event to a browser [`Shortcut`].
#[cfg_attr(not(windows), allow(dead_code))]
fn web_shortcut_for(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<Shortcut> {
    let key = vk_to_key(vk)?;
    shortcut_for(&key, ctrl, shift, alt, false)
}

#[cfg(windows)]
fn attach_webview_shortcuts(wv: &WebView, proxy: EventLoopProxy<UserMessage>) {
    use webview2_com::AcceleratorKeyPressedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
        COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
    };
    use wry::WebViewExtWindows;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetKeyState(vk: i32) -> i16;
    }

    const VK_SHIFT: i32 = 0x10;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;

    let controller = wv.controller();
    let mut token = 0i64;
    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        unsafe {
            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
            args.KeyEventKind(&mut kind)?;
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
            {
                return Ok(());
            }
            let mut vk = 0u32;
            args.VirtualKey(&mut vk)?;
            let ctrl = GetKeyState(VK_CONTROL) < 0;
            let shift = GetKeyState(VK_SHIFT) < 0;
            let alt = GetKeyState(VK_MENU) < 0;
            if let Some(shortcut) = web_shortcut_for(vk, ctrl, shift, alt) {
                args.SetHandled(true)?;
                let _ = proxy.send_event(UserMessage::WebKey(shortcut));
            }
        }
        Ok(())
    }));
    let _ = unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token) };
}

/// Reports element fullscreen (a video's fullscreen button) from the WebView2
/// pane. This is WebView2's own `ContainsFullScreenElementChanged` event, so a
/// page cannot fake it, and it covers fullscreen requested from iframes too.
#[cfg(windows)]
fn attach_webview_fullscreen(wv: &WebView, proxy: EventLoopProxy<UserMessage>) {
    use webview2_com::ContainsFullScreenElementChangedEventHandler;
    use wry::WebViewExtWindows;

    let webview = wv.webview();
    let mut token = 0i64;
    let handler = ContainsFullScreenElementChangedEventHandler::create(Box::new(move |sender, _| {
        let Some(sender) = sender else {
            return Ok(());
        };
        let mut flag = Default::default();
        unsafe { sender.ContainsFullScreenElement(&mut flag)? };
        let _ = proxy.send_event(UserMessage::WebFullscreen(flag.as_bool()));
        Ok(())
    }));
    let _ = unsafe { webview.add_ContainsFullScreenElementChanged(&handler, &mut token) };
}

/// Applies an editing key to a text field. Returns `true` when the key was an
/// editing key (so the caller should redraw).
/// `secret` fields (passwords) never copy or cut to the clipboard.
fn edit_field(
    field: &mut TextField,
    key: &Key,
    text: Option<&str>,
    mods: ModifiersState,
    clipboard: &mut Option<arboard::Clipboard>,
    secret: bool,
) -> bool {
    let ctrl = mods.control_key() || (cfg!(target_os = "macos") && mods.super_key());
    let shift = mods.shift_key();
    match key {
        Key::Named(NamedKey::Backspace) => field.backspace(ctrl),
        Key::Named(NamedKey::Delete) => field.delete(ctrl),
        Key::Named(NamedKey::ArrowLeft) => field.left(shift, ctrl),
        Key::Named(NamedKey::ArrowRight) => field.right(shift, ctrl),
        Key::Named(NamedKey::Home) => field.move_to(0, shift),
        Key::Named(NamedKey::End) => field.move_to(field.text.len(), shift),
        Key::Character(c) if ctrl && !mods.alt_key() => match c.to_ascii_lowercase().as_str() {
            "a" => field.select_all(),
            "c" | "x" if secret => {}
            "c" => {
                if let Some(cb) = clipboard {
                    let _ = cb.set_text(field.selected_text().to_string());
                }
            }
            "x" => {
                if let Some(cb) = clipboard {
                    let _ = cb.set_text(field.selected_text().to_string());
                }
                field.backspace(false);
            }
            "v" => {
                if let Some(Ok(pasted)) = clipboard.as_mut().map(|cb| cb.get_text()) {
                    field.insert(&pasted.replace(['\r', '\n'], " "));
                }
            }
            _ => return false,
        },
        _ => match text {
            Some(t) if !t.is_empty() && (!ctrl || mods.alt_key()) => field.insert(t),
            _ => return false,
        },
    }
    true
}

// ---- forms ----

fn form_ancestor(doc: &Document, node: NodeId) -> Option<NodeId> {
    doc.ancestors(node).find(|&n| doc.tag(n) == "form")
}

fn input_type(doc: &Document, node: NodeId) -> String {
    doc.element(node)
        .and_then(|e| e.attr("type"))
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Single-line or multi-line text controls that take typing.
fn is_text_control(doc: &Document, node: NodeId) -> bool {
    match doc.tag(node) {
        "textarea" => true,
        "input" => matches!(
            input_type(doc, node).as_str(),
            "" | "text" | "search" | "email" | "url" | "tel" | "number" | "password"
        ),
        _ => false,
    }
}

fn option_nodes(doc: &Document, select: NodeId) -> Vec<NodeId> {
    doc.descendants(select)
        .into_iter()
        .filter(|&n| doc.tag(n) == "option")
        .collect()
}

fn option_label(doc: &Document, option: NodeId) -> String {
    doc.text_content(option).trim().to_string()
}

fn option_value(doc: &Document, option: NodeId) -> String {
    doc.element(option)
        .and_then(|e| e.attr("value"))
        .map(str::to_string)
        .unwrap_or_else(|| option_label(doc, option))
}

/// The selected `<option>` of a `<select>`: what the user picked, else the
/// `selected` attribute, else the first one. (The form state stores the
/// picked option's label.)
fn selected_option(doc: &Document, state: &FormState, select: NodeId) -> Option<NodeId> {
    let options = option_nodes(doc, select);
    if let Some(label) = state.values.get(&select) {
        return options
            .iter()
            .copied()
            .find(|&o| option_label(doc, o) == *label);
    }
    options
        .iter()
        .copied()
        .find(|&o| doc.element(o).is_some_and(|e| e.attr("selected").is_some()))
        .or(options.first().copied())
}

/// The name/value pairs a form submits, in document order.
fn form_pairs(
    doc: &Document,
    state: &FormState,
    form: NodeId,
    submitter: Option<NodeId>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for node in doc.descendants(form) {
        let Some(el) = doc.element(node) else {
            continue;
        };
        let Some(name) = el.attr("name").filter(|n| !n.is_empty()) else {
            continue;
        };
        if el.attr("disabled").is_some() {
            continue;
        }
        let name = name.to_string();
        let ty = input_type(doc, node);
        match el.tag.as_str() {
            "input" => match ty.as_str() {
                "checkbox" | "radio" => {
                    if state.is_checked(doc, node) {
                        out.push((name, el.attr("value").unwrap_or("on").to_string()));
                    }
                }
                "submit" | "image" | "button" => {
                    if submitter == Some(node) {
                        out.push((name, el.attr("value").unwrap_or("").to_string()));
                    }
                }
                "reset" | "file" => {}
                _ => out.push((name, state.value(doc, node).to_string())),
            },
            "textarea" => out.push((name, state.value(doc, node).to_string())),
            "select" => {
                if let Some(option) = selected_option(doc, state, node) {
                    out.push((name, option_value(doc, option)));
                }
            }
            "button" => {
                if submitter == Some(node) && ty != "reset" && ty != "button" {
                    out.push((name, el.attr("value").unwrap_or("").to_string()));
                }
            }
            _ => {}
        }
    }
    out
}

fn rgb(hex: u32) -> Color {
    Color::rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn with_alpha(c: Color, a: u8) -> Color {
    Color { a, ..c }
}

/// The engine chrome's colours (the same palette as `egui_chrome::Colors`).
#[derive(Clone, Copy)]
struct Palette {
    bar_bg: Color,
    sidebar_bg: Color,
    line: Color,
    field_bg: Color,
    text: Color,
    muted: Color,
    hover: Color,
    accent: Color,
    page_bg: Color,
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bar_bg: rgb(0x2b2b2b),
            sidebar_bg: rgb(0x202020),
            line: rgb(0x3c3c3c),
            field_bg: rgb(0x1e1e1e),
            text: rgb(0xe8e8e8),
            muted: rgb(0x8a8a8a),
            hover: rgb(0x3d3d3d),
            accent: rgb(0x5b9cff),
            page_bg: rgb(0x1e1e1e),
        }
    } else {
        Palette {
            bar_bg: rgb(0xf3f3f3),
            sidebar_bg: rgb(0xececec),
            line: rgb(0xd6d6d6),
            field_bg: rgb(0xffffff),
            text: rgb(0x1f1f1f),
            muted: rgb(0x8a8a8a),
            hover: rgb(0xe1e1e1),
            accent: rgb(0x2f6fde),
            page_bg: rgb(0xffffff),
        }
    }
}

fn inset(r: Rect, by: f32) -> Rect {
    Rect {
        x: r.x + by,
        y: r.y + by,
        w: (r.w - 2.0 * by).max(0.0),
        h: (r.h - 2.0 * by).max(0.0),
    }
}

// ---------------------------------------------------------------------------
// Construction, geometry, tabs and navigation
// ---------------------------------------------------------------------------

impl Browser {
    pub fn new(proxy: EventLoopProxy<UserMessage>, fonts: Fonts, start_url: String) -> Self {
        Browser {
            gfx: None,
            fonts,
            proxy,
            generation: Arc::new(AtomicU64::new(0)),
            tabs: vec![TabState::default()],
            active: 0,
            history: Vec::new(),
            index: 0,
            tab: None,
            loading: false,
            start_url,
            webview: None,
            web_page: false,
            web_url: String::new(),
            title: String::new(),
            address: TextField::new(""),
            address_scroll: 0.0,
            input: TextField::default(),
            focus: Focus::Page,
            modifiers: ModifiersState::empty(),
            mouse: (0.0, 0.0),
            hover_button: None,
            hover_tab: None,
            tabs_scroll: 0,
            hover_link: None,
            user_zoom: 1.0,
            clipboard: arboard::Clipboard::new().ok(),
            last_click: (Instant::now(), -1.0, -1.0),
            frame: Vec::new(),
            egui: None,
            page_tex: None,
            egui_page: None,
            cheatsheet: false,
            notice: None,
            drag: Drag::None,
            quit: false,
            epoch: Instant::now(),
            last_dark: None,
            web_expect: false,
            web_leaving: String::new(),
            web_committed: false,
            web_visible: false,
            last_screenshot: None,
            web_bounds: None,
            pending_restore: None,
            frame_page: (0, 0, 0, 0, 0),
            egui_focus_address: false,
            egui_select_all: false,
            egui_address_focused: false,
            web_fullscreen: false,
            page_forms: None,
            egui_popup_open: false,
            popup_snapshot: None,
            fullscreen_restore: false,
        }
    }

    // ---- geometry (all in physical pixels) ----

    fn scale(&self) -> f32 {
        self.gfx
            .as_ref()
            .map_or(1.0, |g| g.window.scale_factor() as f32)
    }

    fn window_size(&self) -> (f32, f32) {
        self.gfx.as_ref().map_or((1.0, 1.0), |g| {
            let size = g.window.inner_size();
            (size.width as f32, size.height as f32)
        })
    }

    fn toolbar_h(&self) -> f32 {
        if self.web_fullscreen {
            return 0.0;
        }
        (TOOLBAR_HEIGHT * self.scale()).round()
    }

    /// Everything above the page: the toolbar plus, on the GPU chrome, the
    /// bookmarks bar and the "save this login?" bar. The engine lays out,
    /// paints and hit-tests the page from this, so it must match the egui
    /// panels exactly or clicks land on the wrong spot.
    fn top_h(&self) -> f32 {
        let scale = self.scale();
        let save_bar = self.egui.is_some() && self.web_page && passwords::has_offer();
        self.toolbar_h()
            + extra_bars_h(
                scale,
                self.egui.is_some() && !self.web_fullscreen,
                flags::get().bookmarks_bar,
                save_bar && !self.web_fullscreen,
            )
    }

    fn sidebar_w(&self) -> f32 {
        if self.web_fullscreen {
            return 0.0;
        }
        match flags::get().tabs_position {
            TabsPosition::Hidden => 0.0,
            _ => (SIDEBAR_WIDTH * self.scale()).round(),
        }
    }

    fn notice_active(&self) -> bool {
        self.notice
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() < NOTICE_LIFETIME)
    }

    fn notice_h(&self) -> f32 {
        if self.notice_active() && !self.web_fullscreen {
            (NOTICE_HEIGHT * self.scale()).round()
        } else {
            0.0
        }
    }

    fn set_notice(&mut self, message: impl Into<String>) {
        self.notice = Some((message.into(), Instant::now()));
        self.request_redraw();
    }

    /// The area the page (engine raster or WebView2 pane) occupies.
    fn page_rect(&self) -> Rect {
        let (w, h) = self.window_size();
        let (th, sw) = (self.top_h(), self.sidebar_w());
        let left = flags::get().tabs_position == TabsPosition::Left;
        Rect {
            x: if left { sw } else { 0.0 },
            y: th,
            w: (w - sw).max(1.0),
            h: (h - th - self.notice_h()).max(1.0),
        }
    }

    fn sidebar_rect(&self) -> Option<Rect> {
        let pos = flags::get().tabs_position;
        if pos == TabsPosition::Hidden || self.web_fullscreen {
            return None;
        }
        let (w, h) = self.window_size();
        let (th, sw) = (self.top_h(), self.sidebar_w());
        Some(Rect {
            x: if pos == TabsPosition::Left { 0.0 } else { w - sw },
            y: th,
            w: sw,
            h: (h - th).max(1.0),
        })
    }

    /// Where the WebView2 pane goes: exactly over the egui central panel on
    /// the GPU path, the engine layout's page rectangle otherwise.
    fn webview_rect(&self) -> Rect {
        match (&self.egui, self.egui_page) {
            (Some(_), Some(r)) => r,
            _ => self.page_rect(),
        }
    }

    fn request_redraw(&self) {
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }

    fn dark(&self) -> bool {
        let window_dark = self
            .gfx
            .as_ref()
            .and_then(|g| g.window.theme())
            .is_some_and(|t| t == Theme::Dark);
        flags::effective_dark(window_dark)
    }

    fn address_focused(&self) -> bool {
        self.focus == Focus::Address || self.egui_address_focused
    }

    // ---- tabs ----

    /// The URL of the page this tab shows.
    fn current_url(&self) -> String {
        if self.web_page {
            self.web_url.clone()
        } else if let Some(tab) = &self.tab {
            tab.page.url.clone()
        } else {
            self.history
                .get(self.index)
                .map(|e| e.request.url().to_string())
                .unwrap_or_default()
        }
    }

    fn live_title(&self) -> String {
        let title = self.title.trim();
        if !title.is_empty() {
            return title.to_string();
        }
        let url = self.current_url();
        if url.is_empty() || url == NEW_TAB_URL {
            "New Tab".to_string()
        } else {
            url
        }
    }

    fn tab_title(&self, i: usize) -> String {
        if i == self.active {
            return self.live_title();
        }
        let Some(state) = self.tabs.get(i) else {
            return String::new();
        };
        if !state.title.trim().is_empty() {
            return state.title.clone();
        }
        state
            .history
            .get(state.index)
            .map(|e| e.request.url().to_string())
            .filter(|u| u != NEW_TAB_URL)
            .unwrap_or_else(|| "New Tab".to_string())
    }

    fn update_window_title(&self) {
        if let Some(g) = &self.gfx {
            g.window
                .set_title(&format!("{} - BrowseCraft", self.live_title()));
        }
    }

    /// Moves the live tab fields into the active tab's pantry slot.
    fn park_active_tab(&mut self) {
        // Every tab shares the one WebView; a parked web tab reloads its URL
        // when it becomes active again.
        self.unload_webview();
        self.web_leaving = self.web_url.clone();
        let title = self.live_title();
        let state = TabState {
            history: std::mem::take(&mut self.history),
            index: std::mem::take(&mut self.index),
            tab: self.tab.take(),
            web_page: std::mem::take(&mut self.web_page),
            web_url: std::mem::take(&mut self.web_url),
            title,
        };
        // Any load still running belongs to the tab we are leaving.
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.loading = false;
        self.title.clear();
        self.hover_link = None;
        self.focus = Focus::Page;
        self.drag = Drag::None;
        if let Some(slot) = self.tabs.get_mut(self.active) {
            *slot = state;
        }
    }

    /// Makes tab `i` the live tab, reloading whatever it was showing if its
    /// page was not kept in memory.
    fn activate_pantry(&mut self, i: usize) {
        self.active = i;
        let state = std::mem::take(&mut self.tabs[i]);
        self.history = state.history;
        self.index = state.index;
        self.tab = state.tab;
        self.web_page = state.web_page;
        self.web_url = state.web_url;
        self.title = if state.title == "New Tab" {
            String::new()
        } else {
            state.title
        };
        self.loading = false;
        self.focus = Focus::Page;
        self.address.set(&self.current_url());
        if self.web_page {
            let url = self.web_url.clone();
            self.web_expect = true;
            self.web_committed = false;
            self.loading = true;
            if let Some(wv) = &self.webview {
                let _ = wv.load_url(&url);
                let _ = wv.zoom(self.user_zoom as f64);
            }
        } else if self.tab.is_none()
            && let Some(entry) = self.history.get(self.index)
        {
            let (request, scroll) = (entry.request.clone(), entry.scroll);
            self.start_load(request, false, Some(scroll));
        }
        self.update_window_title();
        self.request_redraw();
    }

    fn switch_tab(&mut self, i: usize) {
        if i == self.active || i >= self.tabs.len() {
            return;
        }
        self.park_active_tab();
        self.activate_pantry(i);
    }

    fn cycle_tab(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        if n < 2 {
            return;
        }
        let next = (self.active as isize + delta).rem_euclid(n) as usize;
        self.switch_tab(next);
    }

    fn new_tab(&mut self) {
        self.park_active_tab();
        self.tabs.push(TabState::default());
        self.active = self.tabs.len() - 1;
        self.load(Request::Get(NEW_TAB_URL.to_string()), true);
        // A fresh tab starts with an empty, focused address bar.
        self.address.set("");
        self.focus_address();
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            self.quit = true;
            return;
        }
        if i == self.active {
            self.park_active_tab();
            self.tabs.remove(i);
            let next = i.min(self.tabs.len() - 1);
            self.activate_pantry(next);
        } else {
            self.tabs.remove(i);
            if i < self.active {
                self.active -= 1;
            }
        }
        self.request_redraw();
    }

    // ---- navigation ----

    fn focus_address(&mut self) {
        self.focus = Focus::Address;
        self.address.select_all();
        self.egui_focus_address = true;
        if let Some(wv) = &self.webview {
            let _ = wv.focus_parent();
        }
        self.request_redraw();
    }

    /// Opens what the user typed in the address bar.
    fn navigate_input(&mut self, text: &str) {
        let url = address::resolve(text);
        self.focus = Focus::Page;
        self.load(Request::Get(url), true);
    }

    fn live_scroll(&self) -> f32 {
        self.tab.as_ref().map_or(0.0, |t| t.scroll_y)
    }

    /// Loads `request` as a new history entry (when `push`) in the active tab.
    fn load(&mut self, request: Request, push: bool) {
        self.start_load(request, push, None);
    }

    fn start_load(&mut self, mut request: Request, push: bool, restore: Option<f32>) {
        let url = request.url().to_string();
        // Main-frame ad filtering: blocked pages show an engine notice but
        // keep the original URL in the address bar.
        if is_web_url(&url) && adblock::blocked_page(&url, "") {
            request = Request::Get(format!("about:blocked:{url}"));
        }
        if push {
            let scroll = self.live_scroll();
            if let Some(entry) = self.history.get_mut(self.index) {
                entry.scroll = scroll;
            }
            if !self.history.is_empty() {
                self.history.truncate(self.index + 1);
            }
            // Toggling a setting reloads the same internal page: replace the
            // entry instead of stacking one Back step per click.
            let same_page = self.history.get(self.index).is_some_and(|e| {
                same_internal_page(e.request.url(), request.url())
            });
            let entry = HistoryEntry {
                request: history_request(&request),
                scroll: 0.0,
            };
            if same_page {
                self.history[self.index] = entry;
            } else {
                self.history.push(entry);
                self.index = self.history.len() - 1;
            }
        }
        self.focus = Focus::Page;
        self.drag = Drag::None;
        self.hover_link = None;
        let target = request.url().to_string();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.loading = true;

        let use_webview = self.webview.is_some()
            && matches!(request, Request::Get(_))
            && request_in_webview(&target);
        if use_webview {
            self.tab = None;
            self.web_page = true;
            if !self.web_url.is_empty() {
                self.web_leaving = std::mem::take(&mut self.web_url);
            }
            self.web_url = target.clone();
            self.web_expect = true;
            self.web_committed = false;
            self.title.clear();
            self.address.set(&target);
            if let Some(wv) = &self.webview {
                let _ = wv.load_url(&target);
                let _ = wv.zoom(self.user_zoom as f64);
            }
            self.update_window_title();
            self.request_redraw();
            return;
        }

        self.unload_webview();
        self.web_page = false;
        self.web_url.clear();
        self.web_expect = false;
        self.pending_restore = restore;
        let shown = loader::blocked_notice_target(&target)
            .unwrap_or(&target)
            .to_string();
        self.address.set(&shown);
        let proxy = self.proxy.clone();
        loader::spawn(
            request,
            generation,
            self.generation.clone(),
            move |g, event| {
                let _ = proxy.send_event(UserMessage::Load(g, event));
            },
        );
        self.update_window_title();
        self.request_redraw();
    }

    /// Back (`-1`) or forward (`+1`) through this tab's history.
    fn go(&mut self, delta: isize) {
        let target = self.index as isize + delta;
        if target < 0 || target as usize >= self.history.len() {
            return;
        }
        let scroll = self.live_scroll();
        if let Some(entry) = self.history.get_mut(self.index) {
            entry.scroll = scroll;
        }
        self.index = target as usize;
        let entry = &self.history[self.index];
        let (request, scroll) = (entry.request.clone(), entry.scroll);
        self.start_load(request, false, Some(scroll));
    }

    fn reload(&mut self) {
        if self.web_page {
            self.web_expect = true;
            self.web_committed = false;
            self.loading = true;
            if let Some(wv) = &self.webview {
                let _ = wv.reload();
            }
            self.request_redraw();
            return;
        }
        if let Some(entry) = self.history.get(self.index) {
            let request = entry.request.clone();
            let scroll = self.live_scroll();
            self.start_load(request, false, Some(scroll));
        }
    }

    fn stop(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.loading = false;
        if self.web_page
            && let Some(wv) = &self.webview
        {
            let _ = wv.evaluate_script("window.stop()");
        }
        self.request_redraw();
    }

    fn home(&mut self) {
        self.load(Request::Get(HOME_URL.to_string()), true);
    }

    fn open_history_page(&mut self) {
        self.load(Request::Get("browsecraft://history".to_string()), true);
    }

    fn view_source(&mut self) {
        let url = self.current_url();
        if is_web_url(&url) || url.to_ascii_lowercase().starts_with("file:") {
            self.load(Request::Get(format!("view-source:{url}")), true);
        } else {
            self.set_notice("There is no source to show for this page");
        }
    }

    fn toggle_theme(&mut self) {
        let mut f = flags::get();
        f.theme = if self.dark() {
            flags::Theme::Light
        } else {
            flags::Theme::Dark
        };
        if let Err(error) = flags::set(f) { self.set_notice(&error); }
        self.request_redraw();
    }

    /// A report from the password script of the page in the web view.
    fn page_message(&mut self, message: PageMessage) {
        if !self.web_page {
            return;
        }
        match message {
            PageMessage::Forms(kind) => {
                self.page_forms = kind;
                self.request_redraw();
            }
            PageMessage::Login {
                url,
                username,
                password,
                kind,
            } => {
                // Judge by the page the browser is really showing, not by
                // the address the page claims.
                let page = self.current_url();
                if vault::same_site(&url, &page)
                    && passwords::offer_login(&page, &username, &password, kind)
                {
                    self.request_redraw();
                }
            }
        }
    }

    /// Runs `script` in the page of the web view.
    fn run_in_page(&self, script: &str) -> bool {
        self.web_page
            && self
                .webview
                .as_ref()
                .is_some_and(|wv| wv.evaluate_script(script).is_ok())
    }

    fn fill_login(&mut self, id: &str) {
        let page = self.current_url();
        match passwords::fill_script(id, &page) {
            Some(script) if self.run_in_page(&script) => self.set_notice("Filled the login"),
            _ => self.set_notice("Could not fill this login on this page"),
        }
    }

    fn suggest_password(&mut self) {
        if self.run_in_page(&passwords::suggest_script()) {
            self.set_notice("Filled a new password. Submit the form and BrowseCraft will offer to save it");
        } else {
            self.set_notice("Could not fill the password fields");
        }
    }

    /// Bookmarks the page, or removes its bookmark (the star, Ctrl+D).
    fn toggle_bookmark(&mut self) {
        let url = self.current_url();
        if !bookmarks::bookmarkable(&url) {
            self.set_notice("Only web pages can be bookmarked");
            return;
        }
        let title = self.live_title();
        let message = if bookmarks::toggle(&title, &url) {
            "Bookmark added"
        } else {
            "Bookmark removed"
        };
        self.set_notice(message);
        // The bookmarks page and the new-tab links are built from the store.
        self.refresh_if_internal("browsecraft://bookmarks");
    }

    fn toggle_bookmarks_bar(&mut self) {
        let mut f = flags::get();
        f.bookmarks_bar = !f.bookmarks_bar;
        if let Err(error) = flags::set(f) { self.set_notice(&error); }
        self.request_redraw();
    }

    /// F11: the whole window fullscreen, independent of page fullscreen.
    fn toggle_window_fullscreen(&mut self) {
        if self.web_fullscreen {
            return;
        }
        if let Some(g) = &self.gfx {
            let on = g.window.fullscreen().is_some();
            g.window
                .set_fullscreen(if on { None } else { Some(Fullscreen::Borderless(None)) });
        }
    }

    /// Opens `url` in a new tab.
    fn open_in_new_tab(&mut self, url: String) {
        self.new_tab();
        self.load(Request::Get(url), true);
        self.focus = Focus::Page;
    }

    /// The URL tab `i` is showing.
    fn tab_url(&self, i: usize) -> String {
        if i == self.active {
            return self.current_url();
        }
        self.tabs
            .get(i)
            .and_then(|s| s.history.get(s.index))
            .map(|e| e.request.url().to_string())
            .unwrap_or_default()
    }

    /// Keeps only tab `keep`.
    fn close_other_tabs(&mut self, keep: usize) {
        if keep >= self.tabs.len() {
            return;
        }
        self.switch_tab(keep);
        while self.tabs.len() > 1 {
            let Some(other) = (0..self.tabs.len()).rev().find(|&j| j != self.active) else {
                break;
            };
            self.close_tab(other);
        }
    }

    /// Reloads the active page when it is the engine page `prefix`, so it
    /// reflects data that just changed.
    fn refresh_if_internal(&mut self, prefix: &str) {
        if !self.web_page && self.current_url().starts_with(prefix) {
            self.reload();
        }
    }

    fn zoom(&mut self, change: Option<bool>) {
        self.user_zoom = match change {
            Some(up) => zoom_step(self.user_zoom, up),
            None => 1.0,
        };
        if let Some(tab) = self.tab.as_mut() {
            tab.style_key = None;
            tab.layout = None;
        }
        if let Some(wv) = &self.webview {
            let _ = wv.zoom(self.user_zoom as f64);
        }
        self.set_notice(format!("Zoom {}%", (self.user_zoom * 100.0).round()));
    }

    // ---- load events from the background loader and the WebView2 pane ----

    fn handle_load(&mut self, event: LoadEvent) {
        match event {
            LoadEvent::Page(page) => {
                let title = clean_title(&page.doc.title());
                let url = page.url.clone();
                if !self.address_focused() {
                    self.address.set(&url);
                }
                let fragment = Url::parse(&url)
                    .ok()
                    .and_then(|u| u.fragment().map(str::to_string));
                let pending = match (self.pending_restore.take(), fragment) {
                    (Some(y), _) if y > 0.0 => Some(PendingScroll::To(y)),
                    (_, Some(f)) => Some(PendingScroll::Fragment(f)),
                    _ => None,
                };
                let blocked = self
                    .history
                    .get(self.index)
                    .is_some_and(|e| e.request.url().starts_with("about:blocked:"));
                if !blocked && !url.starts_with("about:") {
                    history::record(&url, &title);
                }
                self.title = title;
                self.tab = Some(Tab {
                    page,
                    images: HashMap::new(),
                    form: FormState::default(),
                    styles: Vec::new(),
                    style_key: None,
                    styled_at: (0.0, 0.0),
                    layout: None,
                    layout_key: (0.0, 0.0),
                    scroll_x: 0.0,
                    scroll_y: 0.0,
                    pending_scroll: pending,
                });
                if matches!(self.focus, Focus::Input(_)) {
                    self.focus = Focus::Page;
                }
                self.update_window_title();
            }
            LoadEvent::Image { src, image } => {
                if let Some(tab) = self.tab.as_mut() {
                    tab.images.insert(src, Rc::new(image));
                    tab.layout = None;
                }
            }
            LoadEvent::Finished => self.loading = false,
        }
    }

    /// Whether a WebView event for `url` is a late one from the page we
    /// navigated away from, rather than from our navigation.
    fn stale_web_event(&self, url: &str) -> bool {
        stale_web_event(self.web_expect, &self.web_leaving, &self.web_url, url)
    }

    fn web_started(&mut self, url: String) {
        if !self.web_page || self.stale_web_event(&url) {
            return;
        }
        if url != "about:blank" {
            self.web_committed = true;
        }
        self.loading = true;
        // A new page: forget the form kind (the script reports it again) and
        // a save offer that was for another site.
        self.page_forms = None;
        passwords::drop_offer_unless_site(&url);
        if !self.address_focused() && url != "about:blank" {
            self.address.set(&url);
        }
    }

    fn web_loaded(&mut self, url: String) {
        if !self.web_page || self.stale_web_event(&url) {
            return;
        }
        self.loading = false;
        if url.is_empty() || url == "about:blank" {
            return;
        }
        self.web_committed = true;
        if self.web_expect {
            // Our own navigation finished; the final URL (after redirects)
            // replaces the entry we made for it.
            self.web_expect = false;
            if url != self.web_url
                && let Some(entry) = self.history.get_mut(self.index)
            {
                entry.request = Request::Get(url.clone());
            }
        } else if url != self.web_url {
            // The user followed a link inside the page.
            if !self.history.is_empty() {
                self.history.truncate(self.index + 1);
            }
            self.history.push(HistoryEntry {
                request: Request::Get(url.clone()),
                scroll: 0.0,
            });
            self.index = self.history.len() - 1;
        }
        self.web_url = url.clone();
        if !self.address_focused() {
            self.address.set(&url);
        }
        history::record(&url, &self.title);
    }

    fn web_titled(&mut self, title: String) {
        // Before our navigation starts, a title is still the old page's.
        if !self.web_page || self.webview_blank() || (self.web_expect && !self.web_committed) {
            return;
        }
        self.title = clean_title(&title);
        self.update_window_title();
        if !self.loading && !self.web_url.is_empty() {
            history::record(&self.web_url, &self.title);
        }
    }

    /// A pop-up or a blocked main-frame navigation asked to open `url`.
    fn web_open(&mut self, url: String) {
        if !self.web_page {
            return; // from a page that is no longer shown
        }
        if is_web_url(&url) && adblock::blocked_page(&url, "") {
            self.set_notice("Blocked a pop-up (ad filter)");
            return;
        }
        self.load(Request::Get(url), true);
    }

    // ---- the WebView2 pane ----

    /// Navigates the WebView to `about:blank` so that a page that is no longer
    /// shown stops running scripts, playing media and making requests, which
    /// merely hiding the WebView does not do.
    fn unload_webview(&self) {
        if let Some(wv) = &self.webview
            && wv.url().is_ok_and(|u| !u.is_empty() && u != "about:blank")
        {
            let _ = wv.load_url("about:blank");
        }
    }

    fn webview_blank(&self) -> bool {
        self.webview
            .as_ref()
            .is_some_and(|wv| wv.url().is_ok_and(|u| u == "about:blank"))
    }

    fn create_webview(&mut self, window: &Window) {
        // The embedded web view is a WebView2 child window on Windows (and a
        // WKWebView on macOS). Elsewhere everything renders in the engine.
        if !(cfg!(windows) || cfg!(target_os = "macos")) {
            return;
        }
        let nav_proxy = self.proxy.clone();
        let popup_proxy = self.proxy.clone();
        let title_proxy = self.proxy.clone();
        let load_proxy = self.proxy.clone();
        let ipc_proxy = self.proxy.clone();
        let size = window.inner_size();
        let bounds = wry::Rect {
            position: WryPosition::Physical(WryPhysicalPosition::new(0, 0)),
            size: WrySize::Physical(WryPhysicalSize::new(size.width.max(1), size.height.max(1))),
        };
        let builder = WebViewBuilder::new()
            .with_visible(false)
            .with_bounds(bounds)
            .with_initialization_script(adblock::instance().page_script_with_network(!cfg!(windows)))
            .with_initialization_script(passwords::PAGE_SCRIPT)
            .with_ipc_handler(move |request| {
                // Reports from the password script. The page is untrusted:
                // messages are parsed defensively and only ever lead to an
                // offer the user can decline.
                if let Some(message) = passwords::parse_page_message(request.body()) {
                    let _ = ipc_proxy.send_event(UserMessage::Page(message));
                }
            })
            .with_navigation_handler(move |url| {
                // Web content cannot open the engine's internal pages.
                if !navigation_allowed(&url) {
                    return false;
                }
                // Main-frame navigations the ad filter blocks become an engine
                // notice page instead.
                if is_web_url(&url) && adblock::blocked_page(&url, "") {
                    let _ =
                        nav_proxy.send_event(UserMessage::WebOpen(format!("about:blocked:{url}")));
                    return false;
                }
                true
            })
            .with_new_window_req_handler(move |url, _features| {
                // Pop-ups open as a new navigation in the browser, but only
                // to web pages: never to internal, file or view-source URLs.
                if popup_allowed(&url) {
                    let _ = popup_proxy.send_event(UserMessage::WebOpen(url));
                }
                NewWindowResponse::Deny
            })
            .with_document_title_changed_handler(move |title| {
                let _ = title_proxy.send_event(UserMessage::WebTitle(title));
            })
            .with_on_page_load_handler(move |event, url| {
                let message = match event {
                    PageLoadEvent::Started => UserMessage::WebStart(url),
                    PageLoadEvent::Finished => UserMessage::WebLoad(url),
                };
                let _ = load_proxy.send_event(message);
            });
        #[cfg(windows)]
        let builder = {
            use wry::WebViewBuilderExtWindows;
            builder.with_theme(match flags::get().theme {
                flags::Theme::Light => wry::Theme::Light,
                flags::Theme::Dark => wry::Theme::Dark,
                flags::Theme::Auto => wry::Theme::Auto,
            })
        };
        match builder.build_as_child(window) {
            Ok(wv) => {
                #[cfg(windows)]
                {
                    if let Err(error) = crate::webview_network::install(&wv) {
                        eprintln!("browsecraft: could not install native network filtering ({error}); using the engine");
                        return;
                    }
                    attach_webview_shortcuts(&wv, self.proxy.clone());
                    attach_webview_fullscreen(&wv, self.proxy.clone());
                }
                self.webview = Some(wv);
            }
            Err(e) => eprintln!("browsecraft: WebView2 is unavailable ({e}); using the engine for every page"),
        }
    }

    /// Captures the web view's area into a texture shown while a menu is
    /// open. Without a capture (not Windows, or it failed) the area is blank
    /// for as long as the menu is open.
    fn snapshot_page(&mut self) {
        let r = self.webview_rect();
        let (x, y, w, h) = (r.x.round() as i32, r.y.round() as i32, r.w.round() as u32, r.h.round() as u32);
        let max_side = self.egui.as_ref().map_or(0, |e| e.max_texture_side()) as u32;
        let Some(window) = self.gfx.as_ref().map(|g| g.window.clone()) else {
            return;
        };
        if w == 0 || h == 0 || w > max_side || h > max_side {
            return;
        }
        let Some(rgba) = crate::capture::window_region(&window, x, y, w, h) else {
            return;
        };
        if rgba.len() != (w * h * 4) as usize {
            return;
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
        if let Some(egui) = &self.egui {
            self.popup_snapshot = Some(egui.ctx().load_texture(
                "browsecraft-popup-snapshot",
                image,
                egui::TextureOptions::LINEAR,
            ));
            // Draw it now, before the page disappears for good.
            self.request_redraw();
        }
    }

    /// Puts the whole window into (or out of) page fullscreen. The chrome is
    /// hidden while it lasts, so the page, and not just the viewport, fills
    /// the screen.
    fn set_web_fullscreen(&mut self, on: bool) {
        if on == self.web_fullscreen {
            return;
        }
        self.web_fullscreen = on;
        if let Some(g) = &self.gfx {
            if on {
                self.fullscreen_restore = g.window.fullscreen().is_some();
                g.window.set_fullscreen(Some(Fullscreen::Borderless(None)));
            } else if !self.fullscreen_restore {
                g.window.set_fullscreen(None);
            }
        }
        // The page area changed size: force the pane to follow.
        self.web_bounds = None;
        self.request_redraw();
    }

    /// Shows the WebView2 pane over the page area when a web page is the
    /// active one, and keeps its bounds in step with the layout.
    fn sync_webview(&mut self) {
        if self.web_fullscreen && !self.web_page {
            // The fullscreen page is gone (another tab, an engine page).
            self.set_web_fullscreen(false);
        }
        let menu_over_page = self.egui_popup_open && self.web_page && !self.cheatsheet;
        if menu_over_page && self.web_visible && self.popup_snapshot.is_none() {
            // The menu just opened: grab the page while it is still showing.
            self.snapshot_page();
        }
        if !menu_over_page {
            self.popup_snapshot = None;
        }
        let visible = self.web_page && !self.cheatsheet && !self.egui_popup_open;
        let r = self.webview_rect();
        let bounds = (
            r.x.round() as i32,
            r.y.round() as i32,
            r.w.round().max(1.0) as u32,
            r.h.round().max(1.0) as u32,
        );
        let Some(wv) = &self.webview else {
            return;
        };
        if visible && self.web_bounds != Some(bounds) {
            let _ = wv.set_bounds(wry::Rect {
                position: WryPosition::Physical(WryPhysicalPosition::new(bounds.0, bounds.1)),
                size: WrySize::Physical(WryPhysicalSize::new(bounds.2, bounds.3)),
            });
            self.web_bounds = Some(bounds);
        }
        if visible != self.web_visible {
            let _ = wv.set_visible(visible);
            self.web_visible = visible;
        }
    }

    /// Pushes the effective theme to the window and the web view.
    fn apply_theme(&mut self, dark: bool) {
        self.last_dark = Some(dark);
        // Internal pages are generated for one theme: rebuild the open one.
        if internal::set_dark(dark) && !self.web_page {
            let url = self.current_url();
            if url.starts_with("browsecraft://") || url == NEW_TAB_URL {
                self.reload();
            }
        }
        let setting = flags::get().theme;
        if let Some(g) = &self.gfx {
            g.window.set_theme(match setting {
                flags::Theme::Auto => None,
                flags::Theme::Light => Some(Theme::Light),
                flags::Theme::Dark => Some(Theme::Dark),
            });
        }
        #[cfg(windows)]
        if let Some(wv) = &self.webview {
            use wry::WebViewExtWindows;
            let _ = wv.set_theme(match setting {
                flags::Theme::Auto => wry::Theme::Auto,
                flags::Theme::Light => wry::Theme::Light,
                flags::Theme::Dark => wry::Theme::Dark,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Layout, page interaction, forms, keyboard and mouse
// ---------------------------------------------------------------------------

impl Browser {
    /// Restyles and re-lays-out the engine page when the viewport, zoom, form
    /// state or images changed, then applies any pending scroll.
    fn ensure_layout(&mut self, vw: f32, vh: f32) {
        let zoom = self.scale() * self.user_zoom;
        let Some(tab) = self.tab.as_mut() else {
            return;
        };
        if tab.style_key.is_none() || tab.styled_at != (vw, zoom) {
            tab.styled_at = (vw, zoom);
            let key = (style::media_signature(&tab.page.sheets, vw / zoom), zoom);
            if tab.style_key.as_ref() != Some(&key) {
                let ctx = StyleContext {
                    zoom,
                    viewport_width: vw,
                    viewport_height: vh,
                };
                tab.styles = style::compute_styles(&tab.page.doc, &tab.page.sheets, &ctx);
                tab.style_key = Some(key);
                tab.layout = None;
            }
        }
        if tab.layout.is_none() || tab.layout_key != (vw, zoom) {
            let input = LayoutInput {
                doc: &tab.page.doc,
                styles: &tab.styles,
                fonts: &self.fonts,
                images: &tab.images,
                form: &tab.form,
                zoom,
                viewport_width: vw,
                viewport_height: vh,
            };
            let laid_out = layout::layout(&input);
            tab.layout = Some(laid_out);
            tab.layout_key = (vw, zoom);
        }
        let Some(layout) = tab.layout.as_ref() else {
            return;
        };
        let (lw, lh) = (layout.width, layout.height);
        match tab.pending_scroll.take() {
            Some(PendingScroll::To(y)) => tab.scroll_y = y,
            Some(PendingScroll::Fragment(name)) => {
                if name.is_empty() {
                    tab.scroll_y = 0.0;
                } else if let Some(node) = tab.page.doc.find_anchor(&name)
                    && let Some(&(_, y)) = layout.anchors.iter().find(|(n, _)| *n == node)
                {
                    tab.scroll_y = y;
                }
            }
            None => {}
        }
        tab.scroll_x = tab.scroll_x.clamp(0.0, (lw - vw).max(0.0));
        tab.scroll_y = tab.scroll_y.clamp(0.0, (lh - vh).max(0.0));
    }

    fn scroll_by(&mut self, dx: f32, dy: f32) {
        let page = self.page_rect();
        let Some(tab) = self.tab.as_mut() else {
            return;
        };
        let Some((lw, lh)) = tab.layout.as_ref().map(|l| (l.width, l.height)) else {
            return;
        };
        tab.scroll_x = (tab.scroll_x + dx).clamp(0.0, (lw - page.w).max(0.0));
        tab.scroll_y = (tab.scroll_y + dy).clamp(0.0, (lh - page.h).max(0.0));
        self.request_redraw();
    }

    fn scroll_to_edge(&mut self, end: bool) {
        self.scroll_by(0.0, if end { 1.0e9 } else { -1.0e9 });
    }

    // ---- hit testing (engine pages) ----

    /// Window position -> layout position, when the point is over the page.
    fn page_point(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let page = self.page_rect();
        let tab = self.tab.as_ref()?;
        if self.web_page || !page.contains(x, y) {
            return None;
        }
        Some((x - page.x + tab.scroll_x, y - page.y + tab.scroll_y))
    }

    fn hit_at(&self, px: f32, py: f32) -> Hit {
        let Some(layout) = self.tab.as_ref().and_then(|t| t.layout.as_ref()) else {
            return Hit::None;
        };
        if let Some(c) = layout.controls.iter().rev().find(|c| c.rect.contains(px, py)) {
            return Hit::Control(c.node);
        }
        if let Some((_, href)) = layout.links.iter().rev().find(|(r, _)| r.contains(px, py)) {
            return Hit::Link(href.clone());
        }
        Hit::None
    }

    fn resolve_href(&self, href: &str) -> Option<String> {
        let tab = self.tab.as_ref()?;
        let href = href.trim();
        tab.page
            .base
            .join(href)
            .or_else(|_| Url::parse(href))
            .ok()
            .map(|u| u.to_string())
    }

    fn is_double_click(&mut self, x: f32, y: f32) -> bool {
        let (at, lx, ly) = self.last_click;
        let double = at.elapsed() < Duration::from_millis(400)
            && (x - lx).abs() < 4.0
            && (y - ly).abs() < 4.0;
        self.last_click = (Instant::now(), x, y);
        double
    }

    // ---- clicks on the page ----

    fn click_page(&mut self, x: f32, y: f32) {
        let Some((px, py)) = self.page_point(x, y) else {
            return;
        };
        match self.hit_at(px, py) {
            Hit::Control(node) => self.click_control(node, px),
            Hit::Link(href) => {
                self.focus = Focus::Page;
                self.open_link(&href);
            }
            Hit::None => self.focus = Focus::Page,
        }
        self.request_redraw();
    }

    fn open_link(&mut self, href: &str) {
        let trimmed = href.trim();
        let lower = trimmed.to_ascii_lowercase();
        if trimmed.is_empty()
            || lower.starts_with("javascript:")
            || lower.starts_with("mailto:")
            || lower.starts_with("tel:")
        {
            return;
        }
        if let Some(fragment) = trimmed.strip_prefix('#') {
            if let Some(tab) = self.tab.as_mut() {
                tab.pending_scroll = Some(PendingScroll::Fragment(fragment.to_string()));
            }
            return;
        }
        if let Some(url) = self.resolve_href(trimmed) {
            self.load(Request::Get(url), true);
        }
    }

    fn click_control(&mut self, node: NodeId, px: f32) {
        let Some(tab) = self.tab.as_ref() else {
            return;
        };
        let doc = &tab.page.doc;
        let tag = doc.tag(node).to_string();
        let ty = input_type(doc, node);
        let form = form_ancestor(doc, node);
        let text_like = is_text_control(doc, node);
        match (tag.as_str(), ty.as_str()) {
            ("input", "checkbox") => {
                if let Some(tab) = self.tab.as_mut() {
                    let now = tab.form.is_checked(&tab.page.doc, node);
                    tab.form.checked.insert(node, !now);
                    tab.layout = None;
                }
                self.focus = Focus::Page;
            }
            ("input", "radio") => {
                self.select_radio(node);
                self.focus = Focus::Page;
            }
            ("select", _) => {
                self.cycle_select(node);
                self.focus = Focus::Page;
            }
            ("input", "submit" | "image") | ("button", "" | "submit") => {
                self.focus = Focus::Page;
                if let Some(f) = form {
                    self.submit_form(f, Some(node));
                }
            }
            ("input", "reset") | ("button", "reset") => {
                self.focus = Focus::Page;
                if let Some(f) = form {
                    self.reset_form(f);
                }
            }
            _ if text_like => self.focus_input(node, Some(px)),
            _ => self.focus = Focus::Page,
        }
    }

    fn select_radio(&mut self, node: NodeId) {
        let Some(tab) = self.tab.as_mut() else {
            return;
        };
        let doc = &tab.page.doc;
        let name = doc
            .element(node)
            .and_then(|e| e.attr("name"))
            .unwrap_or("")
            .to_string();
        if !name.is_empty() {
            let scope = form_ancestor(doc, node).unwrap_or(Document::ROOT);
            for n in doc.descendants(scope) {
                if doc.tag(n) == "input"
                    && input_type(doc, n) == "radio"
                    && doc.element(n).and_then(|e| e.attr("name")) == Some(name.as_str())
                {
                    tab.form.checked.insert(n, false);
                }
            }
        }
        tab.form.checked.insert(node, true);
        tab.layout = None;
    }

    /// There is no drop-down: each click moves to the next option.
    fn cycle_select(&mut self, node: NodeId) {
        let Some(tab) = self.tab.as_mut() else {
            return;
        };
        let doc = &tab.page.doc;
        let options = option_nodes(doc, node);
        if options.len() < 2 {
            return;
        }
        let current = selected_option(doc, &tab.form, node);
        let at = options.iter().position(|&o| Some(o) == current).unwrap_or(0);
        let next = options[(at + 1) % options.len()];
        tab.form.values.insert(node, option_label(doc, next));
        tab.layout = None;
    }

    fn reset_form(&mut self, form: NodeId) {
        let Some(tab) = self.tab.as_mut() else {
            return;
        };
        for n in tab.page.doc.descendants(form) {
            tab.form.values.remove(&n);
            tab.form.checked.remove(&n);
        }
        tab.layout = None;
    }

    fn submit_form(&mut self, form: NodeId, submitter: Option<NodeId>) {
        let Some(tab) = self.tab.as_ref() else {
            return;
        };
        let doc = &tab.page.doc;
        let Some(el) = doc.element(form) else {
            return;
        };
        let pairs = form_pairs(doc, &tab.form, form, submitter);
        let post = el
            .attr("method")
            .is_some_and(|m| m.eq_ignore_ascii_case("post"));
        let action = el.attr("action").map(str::trim).unwrap_or("");
        // An empty action submits to the page itself (how `browsecraft://flags`
        // applies its settings).
        let target = if action.is_empty() {
            tab.page.url.clone()
        } else {
            tab.page
                .base
                .join(action)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| action.to_string())
        };
        let Ok(mut url) = Url::parse(&target) else {
            return;
        };
        url.set_fragment(None);
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        let request = if post {
            Request::Post(url.to_string(), body)
        } else {
            url.set_query(if body.is_empty() { None } else { Some(&body) });
            Request::Get(url.to_string())
        };
        self.load(request, true);
    }

    // ---- text fields on engine pages ----

    /// Layout facts needed to place a caret in a text control.
    fn input_geometry(&self, node: NodeId) -> Option<(Rect, f32, FontKey, f32, bool)> {
        let tab = self.tab.as_ref()?;
        let layout = tab.layout.as_ref()?;
        let c = layout.controls.iter().find(|c| c.node == node)?;
        let password = input_type(&tab.page.doc, node) == "password";
        Some((c.rect, c.text_x, c.font, c.size, password))
    }

    fn focus_input(&mut self, node: NodeId, click_x: Option<f32>) {
        let Some(tab) = self.tab.as_ref() else {
            return;
        };
        let value = tab.form.value(&tab.page.doc, node).to_string();
        let geometry = self.input_geometry(node);
        self.input.set(&value);
        if let (Some(px), Some((_, text_x, font, size, password))) = (click_x, geometry) {
            let at = caret_index(&self.fonts, font, size, &value, password, px - text_x);
            self.input.move_to(at, false);
        }
        self.focus = Focus::Input(node);
        self.drag = Drag::Input;
        self.surrender_egui_focus();
    }

    /// Writes the edited text back into the form state.
    fn commit_input(&mut self, node: NodeId) {
        if let Some(tab) = self.tab.as_mut() {
            tab.form.values.insert(node, self.input.text.clone());
            tab.layout = None;
        }
    }

    fn next_field(&mut self, node: NodeId, backwards: bool) {
        let Some(tab) = self.tab.as_ref() else {
            return;
        };
        let Some(layout) = tab.layout.as_ref() else {
            return;
        };
        let fields: Vec<NodeId> = layout
            .controls
            .iter()
            .map(|c| c.node)
            .filter(|&n| is_text_control(&tab.page.doc, n))
            .collect();
        let Some(at) = fields.iter().position(|&n| n == node) else {
            return;
        };
        let n = fields.len();
        let next = if backwards { (at + n - 1) % n } else { (at + 1) % n };
        self.focus_input(fields[next], None);
        self.drag = Drag::None;
    }

    fn surrender_egui_focus(&mut self) {
        if let Some(egui) = &self.egui {
            egui.ctx()
                .memory_mut(|m| m.surrender_focus(egui_chrome::address_id()));
        }
        self.egui_address_focused = false;
    }

    // ---- keyboard ----

    /// Keys that act as browser shortcuts even while an egui text box has
    /// the keyboard.
    fn is_shortcut(&self, event: &KeyEvent) -> bool {
        let m = self.modifiers;
        m.control_key()
            || m.alt_key()
            || (cfg!(target_os = "macos") && m.super_key())
            || matches!(&event.logical_key, Key::Named(NamedKey::F5 | NamedKey::F6))
    }

    fn key(&mut self, event: &KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        let mods = self.modifiers;
        let ctrl = mods.control_key() || (cfg!(target_os = "macos") && mods.super_key());
        let (shift, alt) = (mods.shift_key(), mods.alt_key());
        let base = event.key_without_modifiers();
        let egui_typing = self
            .egui
            .as_ref()
            .is_some_and(|e| e.ctx().egui_wants_keyboard_input());
        let typing = egui_typing || matches!(self.focus, Focus::Address | Focus::Input(_));

        if self.cheatsheet {
            if !ctrl {
                let close = matches!(
                    base,
                    Key::Named(NamedKey::Escape | NamedKey::Enter | NamedKey::Space)
                ) || is_char(&base, "k");
                if close {
                    self.cheatsheet = false;
                    self.request_redraw();
                }
                return;
            }
        }

        if let Some(shortcut) = shortcut_for(&base, ctrl, shift, alt, typing) {
            self.run_shortcut(shortcut);
            return;
        }
        self.key_in_focus(event, &base, typing);
    }

    fn run_shortcut(&mut self, shortcut: Shortcut) {
        match shortcut {
            Shortcut::CycleTab(delta) => self.cycle_tab(delta),
            Shortcut::Go(delta) => self.go(delta),
            Shortcut::Home => self.home(),
            Shortcut::NewTab => self.new_tab(),
            Shortcut::CloseTab => self.close_tab(self.active),
            Shortcut::FocusAddress => self.focus_address(),
            Shortcut::Screenshot => self.save_screenshot(),
            Shortcut::Reload => self.reload(),
            Shortcut::ViewSource => self.view_source(),
            Shortcut::History => self.open_history_page(),
            Shortcut::Bookmark => self.toggle_bookmark(),
            Shortcut::BookmarksBar => self.toggle_bookmarks_bar(),
            Shortcut::BookmarksPage => self.load(Request::Get("browsecraft://bookmarks".into()), true),
            Shortcut::Fullscreen => self.toggle_window_fullscreen(),
            Shortcut::Quit => self.quit = true,
            Shortcut::Zoom(change) => self.zoom(change),
        }
        self.request_redraw();
    }

    /// A shortcut pressed while a WebView2 page had the keyboard, reported by
    /// the controller's `AcceleratorKeyPressed` handler.
    fn web_key(&mut self, shortcut: Shortcut) {
        if self.web_page && self.web_visible {
            self.run_shortcut(shortcut);
        }
    }

    /// Keys handled by whatever has focus: the address bar, a form field or
    /// the page itself.
    fn key_in_focus(&mut self, event: &KeyEvent, base: &Key, typing: bool) {
        let mods = self.modifiers;
        let shift = mods.shift_key();
        let egui_typing = typing && !matches!(self.focus, Focus::Address | Focus::Input(_));
        if egui_typing {
            return; // egui's own text box has the keyboard
        }
        match self.focus {
            Focus::Address => match base {
                Key::Named(NamedKey::Enter) => {
                    let text = self.address.text.clone();
                    self.navigate_input(&text);
                }
                Key::Named(NamedKey::Escape) => {
                    let url = self.current_url();
                    self.address.set(&url);
                    self.focus = Focus::Page;
                }
                // With the egui chrome, egui's text box owns the editing;
                // editing here as well would paste twice (issue #14).
                _ if self.egui.is_some() => {}
                _ => {
                    let text = event.text.as_deref();
                    edit_field(&mut self.address, base, text, mods, &mut self.clipboard, false);
                }
            },
            Focus::Input(node) => match base {
                Key::Named(NamedKey::Enter) => {
                    let multiline = self
                        .tab
                        .as_ref()
                        .is_some_and(|t| t.page.doc.tag(node) == "textarea");
                    if multiline {
                        self.input.newline();
                        self.commit_input(node);
                    } else if let Some(form) = self
                        .tab
                        .as_ref()
                        .and_then(|t| form_ancestor(&t.page.doc, node))
                    {
                        self.submit_form(form, None);
                    }
                }
                Key::Named(NamedKey::Tab) => self.next_field(node, shift),
                Key::Named(NamedKey::Escape) => self.focus = Focus::Page,
                _ => {
                    let text = event.text.as_deref();
                    let secret = self
                        .tab
                        .as_ref()
                        .is_some_and(|t| input_type(&t.page.doc, node) == "password");
                    if edit_field(
                        &mut self.input,
                        base,
                        text,
                        mods,
                        &mut self.clipboard,
                        secret,
                    ) {
                        self.commit_input(node);
                    }
                }
            },
            Focus::Page => self.key_page(base, shift),
        }
        self.request_redraw();
    }

    fn key_page(&mut self, base: &Key, shift: bool) {
        let s = self.scale();
        let page_h = self.page_rect().h;
        match base {
            Key::Character(c) if shift && c.as_str().eq_ignore_ascii_case("k") => {
                self.cheatsheet = !self.cheatsheet;
            }
            Key::Named(NamedKey::Escape) if self.loading => self.stop(),
            Key::Named(NamedKey::Backspace) => self.go(if shift { 1 } else { -1 }),
            Key::Named(NamedKey::Space) => {
                self.scroll_by(0.0, if shift { -page_h * 0.9 } else { page_h * 0.9 })
            }
            Key::Named(NamedKey::PageDown) => self.scroll_by(0.0, page_h * 0.9),
            Key::Named(NamedKey::PageUp) => self.scroll_by(0.0, -page_h * 0.9),
            Key::Named(NamedKey::ArrowDown) => self.scroll_by(0.0, SCROLL_STEP * s),
            Key::Named(NamedKey::ArrowUp) => self.scroll_by(0.0, -SCROLL_STEP * s),
            Key::Named(NamedKey::ArrowLeft) => self.scroll_by(-SCROLL_STEP * s, 0.0),
            Key::Named(NamedKey::ArrowRight) => self.scroll_by(SCROLL_STEP * s, 0.0),
            Key::Named(NamedKey::Home) => self.scroll_to_edge(false),
            Key::Named(NamedKey::End) => self.scroll_to_edge(true),
            _ => {}
        }
    }

    // ---- mouse ----

    fn mouse_down(&mut self, button: MouseButton) {
        let (x, y) = self.mouse;
        match button {
            MouseButton::Back => self.go(-1),
            MouseButton::Forward => self.go(1),
            MouseButton::Middle => {
                if self.egui.is_none()
                    && let Some(i) = self.tab_at(x, y)
                {
                    self.close_tab(i);
                }
            }
            MouseButton::Left => self.left_click(x, y),
            _ => {}
        }
        self.request_redraw();
    }

    fn left_click(&mut self, x: f32, y: f32) {
        if self.cheatsheet {
            self.cheatsheet = false;
            return;
        }
        let double = self.is_double_click(x, y);
        if self.page_rect().contains(x, y) {
            // Page clicks work on both chrome paths; the engine page is just
            // a texture under egui, so it is hit-tested here.
            self.surrender_egui_focus();
            self.click_page(x, y);
            if double && let Focus::Input(_) = self.focus {
                self.input.select_all();
            }
            return;
        }
        if self.egui.is_some() {
            return; // egui handles its own toolbar and sidebar clicks
        }
        if y < self.toolbar_h() {
            self.click_toolbar(x, y, double);
        } else if let Some(r) = self.sidebar_rect()
            && r.contains(x, y)
        {
            self.click_sidebar(x, y);
        }
    }

    fn click_toolbar(&mut self, x: f32, y: f32, double: bool) {
        if let Some(b) = Button::ALL
            .into_iter()
            .find(|&b| self.button_rect(b).contains(x, y))
        {
            if let Some(wv) = &self.webview {
                let _ = wv.focus_parent();
            }
            match b {
                Button::Back => self.go(-1),
                Button::Forward => self.go(1),
                Button::Reload => {
                    if self.loading {
                        let _ = self.proxy.send_event(UserMessage::Stop);
                    } else {
                        self.reload();
                    }
                }
                Button::Home => self.home(),
                Button::Theme => self.toggle_theme(),
                Button::Flags => self.load(Request::Get("browsecraft://flags".into()), true),
            }
            return;
        }
        let a = self.address_rect();
        if a.contains(x, y) {
            if self.focus != Focus::Address {
                self.focus_address();
                return;
            }
            if double {
                self.address.select_all();
            } else {
                let at = self.address_index_at(x);
                self.address.move_to(at, false);
                self.drag = Drag::Address;
            }
        } else {
            self.focus = Focus::Page;
        }
    }

    fn click_sidebar(&mut self, x: f32, y: f32) {
        if self.new_tab_rect().contains(x, y) {
            self.new_tab();
            return;
        }
        if let Some(i) = self.tab_at(x, y) {
            if self.close_rect(i).contains(x, y) {
                self.close_tab(i);
            } else {
                self.switch_tab(i);
            }
        }
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        let (x, y) = self.mouse;
        let s = self.scale();
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(lx, ly) => (-lx * SCROLL_STEP * s, -ly * SCROLL_STEP * s),
            MouseScrollDelta::PixelDelta(p) => (-p.x as f32, -p.y as f32),
        };
        let ctrl = self.modifiers.control_key();
        if ctrl {
            if dy != 0.0 {
                self.zoom(Some(dy < 0.0));
            }
            return;
        }
        if self.egui.is_none()
            && let Some(r) = self.sidebar_rect()
            && r.contains(x, y)
        {
            let max = self.tabs.len().saturating_sub(1);
            self.tabs_scroll = if dy > 0.0 {
                (self.tabs_scroll + 1).min(max)
            } else {
                self.tabs_scroll.saturating_sub(1)
            };
            self.request_redraw();
            return;
        }
        if self.page_rect().contains(x, y) {
            if self.modifiers.shift_key() {
                self.scroll_by(dy, 0.0);
            } else {
                self.scroll_by(dx, dy);
            }
        }
    }

    /// Updates hover state, drag selections and the mouse cursor.
    fn mouse_moved(&mut self) {
        let (x, y) = self.mouse;
        let before = (
            self.hover_button,
            self.hover_tab,
            self.hover_link.clone(),
            self.drag,
        );
        let mut cursor = CursorIcon::Default;
        if self.egui.is_none() {
            self.hover_button = Button::ALL
                .into_iter()
                .find(|&b| self.button_rect(b).contains(x, y));
            self.hover_tab = self.tab_at(x, y);
            if self.hover_button.is_some()
                || self.hover_tab.is_some()
                || self.new_tab_rect().contains(x, y)
            {
                cursor = CursorIcon::Pointer;
            } else if self.address_rect().contains(x, y) {
                cursor = CursorIcon::Text;
            }
        }
        let mut link = None;
        if let Some((px, py)) = self.page_point(x, y) {
            match self.hit_at(px, py) {
                Hit::Link(href) => {
                    cursor = CursorIcon::Pointer;
                    link = self.resolve_href(&href).or(Some(href.to_string()));
                }
                Hit::Control(node) => {
                    let text = self
                        .tab
                        .as_ref()
                        .is_some_and(|t| is_text_control(&t.page.doc, node));
                    cursor = if text {
                        CursorIcon::Text
                    } else {
                        CursorIcon::Pointer
                    };
                }
                Hit::None => {}
            }
        }
        self.hover_link = link;
        match (self.drag, self.focus) {
            (Drag::Address, _) => {
                let at = self.address_index_at(x);
                self.address.move_to(at, true);
            }
            (Drag::Input, Focus::Input(node)) => {
                if let Some((_, text_x, font, size, password)) = self.input_geometry(node)
                    && let Some(tab) = self.tab.as_ref()
                {
                    let px = x - self.page_rect().x + tab.scroll_x;
                    let at = caret_index(
                        &self.fonts,
                        font,
                        size,
                        &self.input.text,
                        password,
                        px - text_x,
                    );
                    self.input.move_to(at, true);
                }
            }
            _ => {}
        }
        if self.egui.is_none()
            && let Some(g) = &self.gfx
        {
            g.window.set_cursor(cursor);
        }
        let after = (
            self.hover_button,
            self.hover_tab,
            self.hover_link.clone(),
            self.drag,
        );
        if before != after || self.drag != Drag::None {
            self.request_redraw();
        }
    }
}

// ---------------------------------------------------------------------------
// Engine-drawn chrome, page overlays, cheatsheet, screenshots
// ---------------------------------------------------------------------------

const TAB_ROW_H: f32 = 34.0;
const TAB_LIST_TOP: f32 = 48.0;

impl Browser {
    // ---- chrome geometry ----

    fn button_rect(&self, b: Button) -> Rect {
        let s = self.scale();
        let (w, _) = self.window_size();
        let bw = (32.0 * s).round();
        let gap = (2.0 * s).round();
        let margin = (8.0 * s).round();
        let y = ((self.toolbar_h() - bw) / 2.0).round();
        let x = match b {
            Button::Back => margin,
            Button::Forward => margin + (bw + gap),
            Button::Reload => margin + 2.0 * (bw + gap),
            Button::Home => margin + 3.0 * (bw + gap),
            Button::Flags => w - margin - bw,
            Button::Theme => w - margin - 2.0 * bw - gap,
        };
        Rect { x, y, w: bw, h: bw }
    }

    fn address_rect(&self) -> Rect {
        let s = self.scale();
        let home = self.button_rect(Button::Home);
        let theme = self.button_rect(Button::Theme);
        let x0 = home.x + home.w + (6.0 * s).round();
        let x1 = theme.x - (6.0 * s).round();
        let h = (30.0 * s).round();
        Rect {
            x: x0,
            y: ((self.toolbar_h() - h) / 2.0).round(),
            w: (x1 - x0).max(40.0),
            h,
        }
    }

    fn address_text_rect(&self) -> Rect {
        let s = self.scale();
        let a = self.address_rect();
        let left = (32.0 * s).round();
        Rect {
            x: a.x + left,
            y: a.y,
            w: (a.w - left - (10.0 * s).round()).max(10.0),
            h: a.h,
        }
    }

    fn address_font_size(&self) -> f32 {
        14.0 * self.scale()
    }

    fn address_index_at(&self, x: f32) -> usize {
        let t = self.address_text_rect();
        caret_index(
            &self.fonts,
            FontKey::UI,
            self.address_font_size(),
            &self.address.text,
            false,
            x - t.x + self.address_scroll,
        )
    }

    /// Keeps the caret inside the address field's visible text area.
    fn update_address_scroll(&mut self) {
        if self.focus != Focus::Address {
            self.address_scroll = 0.0;
            return;
        }
        let width = self.address_text_rect().w;
        let size = self.address_font_size();
        let cx = caret_x(
            &self.fonts,
            FontKey::UI,
            size,
            &self.address.text,
            false,
            self.address.cursor(),
        );
        let total = self.fonts.measure(FontKey::UI, &self.address.text, size);
        if cx - self.address_scroll > width - 4.0 {
            self.address_scroll = cx - width + 4.0;
        }
        if cx - self.address_scroll < 0.0 {
            self.address_scroll = cx;
        }
        self.address_scroll = self.address_scroll.clamp(0.0, (total - width + 4.0).max(0.0));
    }

    fn new_tab_rect(&self) -> Rect {
        let Some(sr) = self.sidebar_rect() else {
            return Rect::default();
        };
        let s = self.scale();
        let pad = (8.0 * s).round();
        Rect {
            x: sr.x + pad,
            y: sr.y + pad,
            w: sr.w - 2.0 * pad,
            h: (32.0 * s).round(),
        }
    }

    fn visible_tab_slots(&self) -> usize {
        let Some(sr) = self.sidebar_rect() else {
            return 0;
        };
        let s = self.scale();
        (((sr.h - TAB_LIST_TOP * s) / (TAB_ROW_H * s)).floor() as usize).max(1)
    }

    fn tab_row_rect(&self, slot: usize) -> Rect {
        let Some(sr) = self.sidebar_rect() else {
            return Rect::default();
        };
        let s = self.scale();
        let pad = (8.0 * s).round();
        let row = (TAB_ROW_H * s).round();
        Rect {
            x: sr.x + pad,
            y: sr.y + (TAB_LIST_TOP * s).round() + slot as f32 * row,
            w: sr.w - 2.0 * pad,
            h: row - (2.0 * s).round(),
        }
    }

    fn close_rect(&self, tab: usize) -> Rect {
        let slot = tab.saturating_sub(self.tabs_scroll);
        let r = self.tab_row_rect(slot);
        let s = self.scale();
        let size = (22.0 * s).round();
        Rect {
            x: r.x + r.w - size - (4.0 * s).round(),
            y: r.y + (r.h - size) / 2.0,
            w: size,
            h: size,
        }
    }

    fn tab_at(&self, x: f32, y: f32) -> Option<usize> {
        let sr = self.sidebar_rect()?;
        if !sr.contains(x, y) {
            return None;
        }
        (0..self.visible_tab_slots()).find_map(|slot| {
            let i = self.tabs_scroll + slot;
            (i < self.tabs.len() && self.tab_row_rect(slot).contains(x, y)).then_some(i)
        })
    }

    fn fix_tabs_scroll(&mut self) {
        let visible = self.visible_tab_slots().max(1);
        if self.active < self.tabs_scroll {
            self.tabs_scroll = self.active;
        } else if self.active >= self.tabs_scroll + visible {
            self.tabs_scroll = self.active + 1 - visible;
        }
        self.tabs_scroll = self.tabs_scroll.min(self.tabs.len().saturating_sub(1));
    }

    fn button_enabled(&self, b: Button) -> bool {
        match b {
            Button::Back => self.index > 0,
            Button::Forward => self.index + 1 < self.history.len(),
            _ => true,
        }
    }

    // ---- text helpers ----

    fn baseline_in(&self, key: FontKey, size: f32, y: f32, h: f32) -> f32 {
        let (ascent, descent) = self.fonts.vertical_metrics(key, size);
        y + (h - (ascent + descent)) / 2.0 + ascent
    }

    #[allow(clippy::too_many_arguments)]
    fn text_in(
        &self,
        canvas: &mut Canvas,
        x: f32,
        row: Rect,
        text: &str,
        key: FontKey,
        size: f32,
        color: Color,
    ) {
        let baseline = self.baseline_in(key, size, row.y, row.h);
        canvas.draw_text(&self.fonts, x, baseline, text, key, size, color);
    }

    fn icon_in(&self, canvas: &mut Canvas, r: Rect, icon: &str, size: f32, color: Color) {
        let w = self.fonts.measure(FontKey::ICON, icon, size);
        self.text_in(canvas, r.x + (r.w - w) / 2.0, r, icon, FontKey::ICON, size, color);
    }

    // ---- drawing ----

    fn draw_toolbar(&self, canvas: &mut Canvas, pal: &Palette) {
        let s = self.scale();
        let (w, _) = self.window_size();
        let th = self.toolbar_h();
        canvas.fill_rect(Rect { x: 0.0, y: 0.0, w, h: th }, pal.bar_bg);
        canvas.fill_rect(Rect { x: 0.0, y: th - 1.0, w, h: 1.0 }, pal.line);
        let dark = self.dark();
        for b in Button::ALL {
            let r = self.button_rect(b);
            let enabled = self.button_enabled(b);
            if enabled && self.hover_button == Some(b) {
                canvas.fill_rounded(r, 7.0 * s, pal.hover, None);
            }
            let icon = match b {
                Button::Back => icons::BACK,
                Button::Forward => icons::FORWARD,
                Button::Reload if self.loading => icons::CLOSE,
                Button::Reload => icons::RELOAD,
                Button::Home => icons::HOME,
                Button::Theme if dark => icons::SUN,
                Button::Theme => icons::MOON,
                Button::Flags => icons::GEAR,
            };
            let color = if enabled { pal.text } else { with_alpha(pal.muted, 110) };
            self.icon_in(canvas, r, icon, 16.0 * s, color);
        }

        // The address field.
        let a = self.address_rect();
        let focused = self.focus == Focus::Address;
        let radius = 8.0 * s;
        let border = if focused { pal.accent } else { pal.line };
        canvas.fill_rounded(a, radius, border, None);
        canvas.fill_rounded(inset(a, 1.0), radius - 1.0, pal.field_bg, None);
        let https = self.address.text.starts_with("https://");
        let lead = Rect { x: a.x + 8.0 * s, y: a.y, w: 20.0 * s, h: a.h };
        self.icon_in(
            canvas,
            lead,
            if https { icons::LOCK } else { icons::GLOBE },
            13.0 * s,
            pal.muted,
        );
        let t = self.address_text_rect();
        let size = self.address_font_size();
        canvas.push_clip(t);
        let x0 = t.x - self.address_scroll;
        if self.address.text.is_empty() && !focused {
            self.text_in(canvas, t.x, t, "Search or enter address", FontKey::UI, size, pal.muted);
        } else {
            if focused {
                let sel = self.address.selection();
                if !sel.is_empty() {
                    let sx = x0 + caret_x(&self.fonts, FontKey::UI, size, &self.address.text, false, sel.start);
                    let ex = x0 + caret_x(&self.fonts, FontKey::UI, size, &self.address.text, false, sel.end);
                    let selection = Rect { x: sx, y: t.y + 4.0 * s, w: ex - sx, h: t.h - 8.0 * s };
                    canvas.fill_rect(selection, with_alpha(pal.accent, 90));
                }
            }
            self.text_in(canvas, x0, t, &self.address.text, FontKey::UI, size, pal.text);
            if focused {
                let cx = x0 + caret_x(&self.fonts, FontKey::UI, size, &self.address.text, false, self.address.cursor());
                canvas.fill_rect(
                    Rect { x: cx, y: t.y + 5.0 * s, w: (1.5 * s).max(1.0), h: t.h - 10.0 * s },
                    pal.text,
                );
            }
        }
        canvas.pop_clip();

        // Loading bar under the address field.
        if self.loading {
            let seg = a.w * 0.3;
            let phase = (self.epoch.elapsed().as_secs_f32() * 0.9) % 1.0;
            let x = a.x + (a.w + seg) * phase - seg;
            let left = x.max(a.x);
            let right = (x + seg).min(a.x + a.w);
            if right > left {
                canvas.fill_rect(
                    Rect { x: left, y: th - 3.0 * s, w: right - left, h: 2.0 * s },
                    pal.accent,
                );
            }
        }
    }

    fn draw_sidebar(&self, canvas: &mut Canvas, pal: &Palette) {
        let Some(sr) = self.sidebar_rect() else {
            return;
        };
        let s = self.scale();
        canvas.fill_rect(sr, pal.sidebar_bg);
        let edge_x = if flags::get().tabs_position == TabsPosition::Left {
            sr.x + sr.w - 1.0
        } else {
            sr.x
        };
        canvas.fill_rect(Rect { x: edge_x, y: sr.y, w: 1.0, h: sr.h }, pal.line);
        canvas.push_clip(sr);

        let nt = self.new_tab_rect();
        if nt.contains(self.mouse.0, self.mouse.1) {
            canvas.fill_rounded(nt, 7.0 * s, pal.hover, None);
        }
        self.icon_in(
            canvas,
            Rect { x: nt.x + 4.0 * s, y: nt.y, w: 26.0 * s, h: nt.h },
            icons::PLUS,
            15.0 * s,
            pal.text,
        );
        self.text_in(canvas, nt.x + 34.0 * s, nt, "New tab", FontKey::UI, 13.0 * s, pal.text);

        for slot in 0..self.visible_tab_slots() {
            let i = self.tabs_scroll + slot;
            if i >= self.tabs.len() {
                break;
            }
            let r = self.tab_row_rect(slot);
            let active = i == self.active;
            let hovered = self.hover_tab == Some(i);
            if active || hovered {
                canvas.fill_rounded(r, 7.0 * s, pal.hover, None);
            }
            if active {
                canvas.fill_rect(
                    Rect { x: r.x, y: r.y + 6.0 * s, w: 3.0 * s, h: r.h - 12.0 * s },
                    pal.accent,
                );
            }
            self.icon_in(
                canvas,
                Rect { x: r.x + 6.0 * s, y: r.y, w: 22.0 * s, h: r.h },
                icons::GLOBE,
                14.0 * s,
                pal.muted,
            );
            let show_close = active || hovered;
            let text_w = r.w - 36.0 * s - if show_close { 26.0 * s } else { 0.0 };
            let size = 13.0 * s;
            let title = ellipsize(&self.fonts, FontKey::UI, &self.tab_title(i), size, text_w);
            self.text_in(canvas, r.x + 34.0 * s, r, &title, FontKey::UI, size, pal.text);
            if show_close {
                let c = self.close_rect(i);
                if c.contains(self.mouse.0, self.mouse.1) {
                    canvas.fill_rounded(c, 5.0 * s, pal.line, None);
                }
                self.icon_in(canvas, c, icons::CLOSE, 12.0 * s, pal.muted);
            }
        }
        canvas.pop_clip();
    }

    /// The notice strip under the page.
    fn draw_notice(&self, canvas: &mut Canvas, pal: &Palette) {
        let Some((message, _)) = self.notice.as_ref().filter(|_| self.notice_active()) else {
            return;
        };
        let page = self.page_rect();
        let s = self.scale();
        let strip = Rect { x: page.x, y: page.y + page.h, w: page.w, h: self.notice_h() };
        canvas.fill_rect(strip, pal.hover);
        canvas.fill_rect(Rect { x: strip.x, y: strip.y, w: strip.w, h: 1.0 }, pal.line);
        canvas.push_clip(strip);
        let size = 13.0 * s;
        let text = ellipsize(&self.fonts, FontKey::UI, message, size, strip.w - 20.0 * s);
        self.text_in(canvas, strip.x + 10.0 * s, strip, &text, FontKey::UI, size, pal.text);
        canvas.pop_clip();
    }

    /// Paints the engine page (or a placeholder) into `page`, plus the link
    /// preview and the cheatsheet. Used by both chrome paths.
    fn paint_page(&self, canvas: &mut Canvas, page: Rect, pal: &Palette) {
        let s = self.scale();
        canvas.push_clip(page);
        let laid_out = self.tab.as_ref().filter(|_| !self.web_page);
        let bg = laid_out
            .and_then(|t| t.layout.as_ref())
            .map_or(pal.page_bg, |l| l.background);
        canvas.fill_rect(page, bg);
        if let Some(tab) = laid_out
            && let Some(layout) = &tab.layout
        {
            let (dx, dy) = (page.x - tab.scroll_x, page.y - tab.scroll_y);
            paint::paint_items(canvas, &layout.items, &self.fonts, &tab.images, dx, dy);
            self.paint_input_caret(canvas, tab, layout, dx, dy, pal);
            if layout.height > page.h {
                let track = page.h;
                let thumb_h = (track * page.h / layout.height).max(24.0 * s);
                let max_scroll = (layout.height - page.h).max(1.0);
                let thumb_y = page.y + (track - thumb_h) * (tab.scroll_y / max_scroll);
                canvas.fill_rounded(
                    Rect { x: page.x + page.w - 8.0 * s, y: thumb_y, w: 5.0 * s, h: thumb_h },
                    2.5 * s,
                    with_alpha(pal.muted, 150),
                    None,
                );
            }
        } else if self.loading && !self.web_page {
            let size = 15.0 * s;
            let msg = "Loading\u{2026}";
            let w = self.fonts.measure(FontKey::UI, msg, size);
            self.text_in(canvas, page.x + (page.w - w) / 2.0, page, msg, FontKey::UI, size, pal.muted);
        }
        canvas.pop_clip();

        if let Some(url) = &self.hover_link {
            canvas.push_clip(page);
            let size = 12.0 * s;
            let max_w = page.w * 0.8;
            let text = ellipsize(&self.fonts, FontKey::UI, url, size, max_w);
            let w = self.fonts.measure(FontKey::UI, &text, size) + 16.0 * s;
            let h = 22.0 * s;
            let r = Rect { x: page.x, y: page.y + page.h - h, w, h };
            canvas.fill_rect(r, pal.bar_bg);
            canvas.fill_rect(Rect { x: r.x, y: r.y, w: r.w, h: 1.0 }, pal.line);
            self.text_in(canvas, r.x + 8.0 * s, r, &text, FontKey::UI, size, pal.text);
            canvas.pop_clip();
        }
        if self.cheatsheet {
            self.draw_cheatsheet(canvas, page, pal);
        }
    }

    /// The caret and selection of the focused form field.
    fn paint_input_caret(
        &self,
        canvas: &mut Canvas,
        tab: &Tab,
        layout: &Layout,
        dx: f32,
        dy: f32,
        pal: &Palette,
    ) {
        let Focus::Input(node) = self.focus else {
            return;
        };
        let Some(c) = layout.controls.iter().find(|c| c.node == node) else {
            return;
        };
        let password = input_type(&tab.page.doc, node) == "password";
        let text = &self.input.text;
        let field = Rect { x: c.rect.x + dx, y: c.rect.y + dy, w: c.rect.w, h: c.rect.h };
        canvas.push_clip(field);
        let x0 = c.text_x + dx;
        let (top, height) = (c.baseline + dy - c.size, c.size * 1.3);
        let sel = self.input.selection();
        if !sel.is_empty() {
            let sx = x0 + caret_x(&self.fonts, c.font, c.size, text, password, sel.start);
            let ex = x0 + caret_x(&self.fonts, c.font, c.size, text, password, sel.end);
            canvas.fill_rect(Rect { x: sx, y: top, w: ex - sx, h: height }, with_alpha(pal.accent, 90));
        }
        let cx = x0 + caret_x(&self.fonts, c.font, c.size, text, password, self.input.cursor());
        let thickness = (1.5 * self.scale()).max(1.0);
        canvas.fill_rect(Rect { x: cx, y: top, w: thickness, h: height }, Color::rgb(0x20, 0x20, 0x20));
        canvas.pop_clip();
    }

    /// The Shift+K overlay listing every shortcut.
    fn draw_cheatsheet(&self, canvas: &mut Canvas, page: Rect, pal: &Palette) {
        let s = self.scale();
        canvas.push_clip(page);
        canvas.fill_rect(page, Color { r: 0, g: 0, b: 0, a: 150 });
        let row_h = 28.0 * s;
        let title_h = 52.0 * s;
        let footer_h = 40.0 * s;
        let key_w = 190.0 * s;
        let card_w = (key_w + 330.0 * s).min(page.w - 24.0 * s).max(200.0);
        let card_h = (title_h + row_h * SHORTCUTS.len() as f32 + footer_h).min(page.h - 24.0 * s);
        let card = Rect {
            x: page.x + (page.w - card_w) / 2.0,
            y: page.y + (page.h - card_h) / 2.0,
            w: card_w,
            h: card_h,
        };
        canvas.fill_rounded(card, 12.0 * s, pal.line, None);
        canvas.fill_rounded(inset(card, 1.0), 11.0 * s, pal.bar_bg, None);
        canvas.push_clip(card);
        let bold = FontKey { bold: true, ..FontKey::UI };
        let title_row = Rect { x: card.x, y: card.y, w: card.w, h: title_h };
        self.text_in(canvas, card.x + 22.0 * s, title_row, "Keyboard shortcuts", bold, 18.0 * s, pal.text);
        let size = 14.0 * s;
        for (i, (keys, what)) in SHORTCUTS.iter().enumerate() {
            let row = Rect { x: card.x, y: card.y + title_h + i as f32 * row_h, w: card.w, h: row_h };
            if i % 2 == 1 {
                canvas.fill_rect(Rect { x: row.x + 10.0 * s, y: row.y, w: row.w - 20.0 * s, h: row.h }, with_alpha(pal.hover, 140));
            }
            self.text_in(canvas, row.x + 22.0 * s, row, keys, bold, size, pal.accent);
            self.text_in(canvas, row.x + 22.0 * s + key_w, row, what, FontKey::UI, size, pal.text);
        }
        let footer = Rect { x: card.x, y: card.y + card.h - footer_h, w: card.w, h: footer_h };
        self.text_in(
            canvas,
            card.x + 22.0 * s,
            footer,
            "Press Esc or Shift + K to close",
            FontKey::UI,
            12.0 * s,
            pal.muted,
        );
        canvas.pop_clip();
        canvas.pop_clip();
    }

    // ---- engine render path ----

    fn render_engine(&mut self) {
        let (w, h) = {
            let (w, h) = self.window_size();
            (w as usize, h as usize)
        };
        if w == 0 || h == 0 {
            return;
        }
        let page = self.page_rect();
        self.fix_tabs_scroll();
        self.update_address_scroll();
        self.ensure_layout(page.w, page.h);
        let pal = palette(self.dark());

        let mut frame = std::mem::take(&mut self.frame);
        frame.clear();
        frame.resize(w * h, 0);
        {
            let mut canvas = Canvas::new(&mut frame, w, h);
            canvas.fill_rect(Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 }, pal.bar_bg);
            self.paint_page(&mut canvas, page, &pal);
            self.draw_sidebar(&mut canvas, &pal);
            self.draw_toolbar(&mut canvas, &pal);
            self.draw_notice(&mut canvas, &pal);
        }
        self.frame_page = (
            page.x as usize,
            page.y as usize,
            page.w as usize,
            page.h as usize,
            w,
        );
        if let Some(surface) = self.gfx.as_mut().and_then(|g| g.surface.as_mut())
            && let (Some(nw), Some(nh)) = (NonZeroU32::new(w as u32), NonZeroU32::new(h as u32))
            && surface.resize(nw, nh).is_ok()
            && let Ok(mut buffer) = surface.buffer_mut()
        {
            if buffer.len() == frame.len() {
                buffer.copy_from_slice(&frame);
            }
            let _ = buffer.present();
        }
        self.frame = frame;
    }

    // ---- screenshots ----

    /// Ctrl+Shift+S: saves the visible page area as a PNG in Pictures.
    fn save_screenshot(&mut self) {
        if self
            .last_screenshot
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        self.last_screenshot = Some(Instant::now());
        match self.capture_png() {
            Ok(path) => self.set_notice(format!("Screenshot saved to {}", path.display())),
            Err(e) => self.set_notice(format!("Screenshot failed: {e}")),
        }
    }

    fn capture_png(&self) -> Result<PathBuf, String> {
        use image::ImageEncoder;
        let (w, h, rgba) = self.capture_page_rgba()?;
        let dir = pictures_dir()?;
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let (path, file) = create_screenshot_file(&dir, secs).map_err(|e| e.to_string())?;
        let written = image::codecs::png::PngEncoder::new(std::io::BufWriter::new(file))
            .write_image(&rgba, w, h, image::ExtendedColorType::Rgba8);
        if let Err(e) = written {
            let _ = std::fs::remove_file(&path);
            return Err(e.to_string());
        }
        Ok(path)
    }

    /// The page area as RGBA: rendered from the window for a WebView2 page
    /// (the native pane can't be read back from our frame) and cropped from
    /// the engine frame otherwise.
    fn capture_page_rgba(&self) -> Result<(u32, u32, Vec<u8>), String> {
        if self.web_page && self.web_visible {
            let r = self.webview_rect();
            let (w, h) = (r.w.round() as u32, r.h.round() as u32);
            let rgba = self.gfx.as_ref().and_then(|g| {
                crate::capture::window_region(
                    &g.window,
                    r.x.round() as i32,
                    r.y.round() as i32,
                    w,
                    h,
                )
            });
            return match rgba {
                Some(rgba) => Ok((w, h, rgba)),
                None => Err("this platform can't capture web pages".to_string()),
            };
        }
        let (fx, fy, fw, fh, stride) = self.frame_page;
        if fw == 0 || fh == 0 || self.frame.len() < stride * (fy + fh) {
            return Err("nothing has been drawn yet".to_string());
        }
        let mut rgba = Vec::with_capacity(fw * fh * 4);
        for row in fy..fy + fh {
            for &px in &self.frame[row * stride + fx..row * stride + fx + fw] {
                rgba.extend_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, px as u8, 255]);
            }
        }
        Ok((fw as u32, fh as u32, rgba))
    }
}

// ---------------------------------------------------------------------------
// egui chrome path and the winit application handler
// ---------------------------------------------------------------------------

impl Browser {
    fn render(&mut self) {
        let dark = self.dark();
        if self.last_dark != Some(dark) {
            self.apply_theme(dark);
        }
        if self.egui.is_some() {
            self.render_egui();
        } else {
            self.render_engine();
        }
        self.sync_webview();
    }

    fn render_egui(&mut self) {
        let page = self.page_rect();
        // On a window bigger than the GPU's largest texture (several
        // monitors), rasterise only the part of the page that fits.
        let max_side = self.egui.as_ref().map_or(usize::MAX, |e| e.max_texture_side());
        let (fw, fh) = ((page.w as usize).min(max_side), (page.h as usize).min(max_side));
        if fw == 0 || fh == 0 {
            return;
        }
        self.ensure_layout(page.w, page.h);
        let pal = palette(self.dark());

        // The page alone is rasterised; egui draws everything around it.
        let mut frame = std::mem::take(&mut self.frame);
        frame.clear();
        frame.resize(fw * fh, 0);
        {
            let mut canvas = Canvas::new(&mut frame, fw, fh);
            let local = Rect { x: 0.0, y: 0.0, w: fw as f32, h: fh as f32 };
            self.paint_page(&mut canvas, local, &pal);
        }
        let mut rgb = Vec::with_capacity(fw * fh * 3);
        for &px in &frame {
            rgb.extend_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, px as u8]);
        }
        self.frame = frame;
        self.frame_page = (0, 0, fw, fh, fw);

        let Some(window) = self.gfx.as_ref().map(|g| g.window.clone()) else {
            return;
        };
        let Some(mut egui) = self.egui.take() else {
            return;
        };
        let image = egui::ColorImage::from_rgb([fw, fh], &rgb);
        match self.page_tex.as_mut() {
            Some(tex) => tex.set(image, egui::TextureOptions::NEAREST),
            None => {
                self.page_tex = Some(egui.ctx().load_texture(
                    "browsecraft-page",
                    image,
                    egui::TextureOptions::NEAREST,
                ));
            }
        }
        let tex = self.page_tex.as_ref().map(|t| (t.id(), t.size()));
        egui_chrome::apply_style(egui.ctx(), self.dark());
        let raw = egui.take_raw_input(&window);
        egui.frame(&window, raw, |ui| self.build_egui_chrome(ui, tex));
        self.egui = Some(egui);
    }

    fn build_egui_chrome(&mut self, ui: &mut egui::Ui, tex: Option<(egui::TextureId, [usize; 2])>) {
        use egui_chrome::{ICON_BACK, ICON_FORWARD, ICON_GEAR, ICON_GLOBE, ICON_HOME, ICON_KEY, ICON_LOCK};
        let dark = self.dark();
        let c = egui_chrome::Colors::for_dark(dark);
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        let mut actions: Vec<Action> = Vec::new();
        let (can_back, can_forward) = (self.button_enabled(Button::Back), self.button_enabled(Button::Forward));
        let loading = self.loading;
        let leading = if self.address.text.starts_with("https://") { ICON_LOCK } else { ICON_GLOBE };
        let theme_icon = if dark { egui_chrome::ICON_SUN } else { egui_chrome::ICON_MOON };
        let reload_icon = if loading { egui_chrome::ICON_STOP } else { egui_chrome::ICON_RELOAD };

        let fullscreen = self.web_fullscreen;
        let page_url = self.current_url();
        let star = bookmarks::bookmarkable(&page_url).then(|| bookmarks::contains(&page_url));
        let bar_visible = flags::get().bookmarks_bar && !fullscreen;
        // Password manager state for the key menu and the save bar.
        let pw_status = passwords::status();
        let pw_unlocked = matches!(pw_status, passwords::Status::Unlocked { .. });
        let pw_offer = passwords::current_offer().filter(|_| self.web_page);
        let pw_forms = self.page_forms;
        let on_web_page = self.web_page;
        let page_host = vault::host_key(&page_url);

        // Toolbar.
        if !fullscreen {
            egui::Panel::top("toolbar")
                .exact_size(TOOLBAR_HEIGHT)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(c.bar_bg))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.add_space(8.0);
                        if egui_chrome::icon_button(ui, can_back, ICON_BACK, "Back", 16.0).clicked() {
                            actions.push(Action::Back);
                        }
                        if egui_chrome::icon_button(ui, can_forward, ICON_FORWARD, "Forward", 16.0).clicked() {
                            actions.push(Action::Forward);
                        }
                        let tip = if loading { "Stop" } else { "Reload" };
                        if egui_chrome::icon_button(ui, true, reload_icon, tip, 16.0).clicked() {
                            actions.push(if loading { Action::Stop } else { Action::Reload });
                        }
                        if egui_chrome::icon_button(ui, true, ICON_HOME, "Home", 16.0).clicked() {
                            actions.push(Action::Home);
                        }
                        // The buttons on the right are placed from the right edge, then
                        // the address bar takes whatever room is left. Nothing can be
                        // pushed past the window edge, at any window width.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(8.0);
                            let key_tip = match &pw_status {
                                passwords::Status::Unlocked { .. } => "Passwords (unlocked)",
                                passwords::Status::Locked => "Passwords (locked)",
                                passwords::Status::NoVault => "Passwords",
                            };
                            let key = egui_chrome::icon_button(ui, true, ICON_KEY, key_tip, 16.0);
                            egui::Popup::menu(&key).align(egui::RectAlign::BOTTOM_END).width(300.0).show(|ui| {
                                // Only while the menu is open: the vault is not queried every frame.
                                let pw_logins = if pw_unlocked && on_web_page {
                                    passwords::logins_for(&page_url)
                                } else {
                                    Vec::new()
                                };
                                egui_chrome::password_menu(
                                    ui,
                                    &c,
                                    &egui_chrome::PasswordMenu {
                                        status: &pw_status,
                                        logins: &pw_logins,
                                        forms: pw_forms,
                                        offer: pw_offer.as_ref(),
                                        on_web_page,
                                        host: &page_host,
                                    },
                                    &mut |event| {
                                        actions.push(match event {
                                            egui_chrome::PasswordEvent::Fill(id) => Action::FillLogin(id),
                                            egui_chrome::PasswordEvent::Suggest => Action::SuggestPassword,
                                            egui_chrome::PasswordEvent::Save => Action::SaveLogin,
                                            egui_chrome::PasswordEvent::Manage => Action::PasswordsPage,
                                            egui_chrome::PasswordEvent::Lock => Action::LockVault,
                                        });
                                    },
                                );
                            });
                            if egui_chrome::icon_button(ui, true, ICON_GEAR, "Settings", 16.0).clicked() {
                                actions.push(Action::Flags);
                            }
                            if egui_chrome::icon_button(ui, true, theme_icon, "Toggle light / dark", 16.0).clicked() {
                                actions.push(Action::Theme);
                            }
                            let width = (ui.available_width() - 4.0).max(60.0);
                            if self.egui_focus_address {
                                ctx.memory_mut(|m| m.request_focus(egui_chrome::address_id()));
                                self.egui_focus_address = false;
                                self.egui_select_all = true;
                            }
                            let (resp, star_clicked) =
                                egui_chrome::address_bar(ui, &mut self.address.text, width, &c, leading, star);
                            if star_clicked {
                                actions.push(Action::ToggleBookmark);
                            }
                            self.egui_address_focused = resp.has_focus();
                            if self.egui_select_all && resp.has_focus() {
                                let chars = self.address.text.chars().count();
                                egui_chrome::select_all(&ctx, egui_chrome::address_id(), chars);
                                self.egui_select_all = false;
                            }
                            if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                actions.push(Action::RestoreAddress);
                            } else if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                actions.push(Action::Navigate(self.address.text.clone()));
                            }
                        });
                    });
                    if loading {
                        // A thin sliding bar along the bottom edge of the toolbar.
                        let r = ui.max_rect();
                        let phase = (self.epoch.elapsed().as_secs_f32() * 0.9) % 1.0;
                        let seg = r.width() * 0.25;
                        let x = r.min.x + (r.width() + seg) * phase - seg;
                        let bar = egui::Rect::from_min_max(
                            egui::pos2(x.max(r.min.x), r.max.y - 3.0),
                            egui::pos2((x + seg).min(r.max.x), r.max.y - 1.0),
                        );
                        if bar.width() > 0.0 {
                            ui.painter().rect_filled(bar, 0.0, c.accent);
                        }
                    }
                    if !bar_visible {
                        let r = ui.max_rect();
                        ui.painter().hline(r.x_range(), r.max.y - 0.5, egui::Stroke::new(1.0, c.line));
                    }
                });
        }

        // Bookmarks bar, under the address bar.
        if bar_visible {
            let items: Vec<(String, String)> = bookmarks::all().into_iter().map(|b| (b.title, b.url)).collect();
            egui::Panel::top("bookmarks")
                .exact_size(BOOKMARKS_BAR_HEIGHT)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(c.bar_bg))
                .show(ui, |ui| {
                    for event in egui_chrome::bookmarks_bar(ui, &c, &items) {
                        actions.push(match event {
                            egui_chrome::BarEvent::Open(url) => Action::OpenBookmark(url),
                            egui_chrome::BarEvent::OpenInNewTab(url) => Action::OpenInNewTab(url),
                            egui_chrome::BarEvent::Remove(url) => Action::RemoveBookmark(url),
                            egui_chrome::BarEvent::Manage => Action::BookmarksPage,
                        });
                    }
                    let r = ui.max_rect();
                    ui.painter().hline(r.x_range(), r.max.y - 0.5, egui::Stroke::new(1.0, c.line));
                });
        }

        // "Save this login?" bar, under the bookmarks bar.
        if let Some(offer) = &pw_offer
            && !fullscreen
        {
            egui::Panel::top("save_login")
                .exact_size(SAVE_BAR_HEIGHT)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(c.hover))
                .show(ui, |ui| {
                    egui_chrome::save_login_bar(ui, &c, offer, &pw_status, &mut |event| {
                        actions.push(match event {
                            egui_chrome::SaveBarEvent::Save => Action::SaveLogin,
                            egui_chrome::SaveBarEvent::OpenPasswords => Action::PasswordsPage,
                            egui_chrome::SaveBarEvent::NotNow => Action::DismissOffer,
                            egui_chrome::SaveBarEvent::Never => Action::NeverSave,
                        });
                    });
                    let r = ui.max_rect();
                    ui.painter().hline(r.x_range(), r.max.y - 0.5, egui::Stroke::new(1.0, c.line));
                });
        }

        // Tab sidebar.
        let position = flags::get().tabs_position;
        if position != TabsPosition::Hidden && !fullscreen {
            let titles: Vec<String> = (0..self.tabs.len()).map(|i| self.tab_title(i)).collect();
            let active = self.active;
            let panel = if position == TabsPosition::Left {
                egui::Panel::left("tabs")
            } else {
                egui::Panel::right("tabs")
            };
            panel
                .exact_size(SIDEBAR_WIDTH)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(c.sidebar_bg))
                .show(ui, |ui| {
                    egui::Frame::NONE.inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                        let label = egui::RichText::new(format!("{}  New tab", egui_chrome::ICON_PLUS))
                            .size(13.0)
                            .color(c.text);
                        let button = egui::Button::new(label)
                            .fill(c.hover)
                            .corner_radius(egui::CornerRadius::same(10));
                        if ui.add_sized([ui.available_width(), 34.0], button).clicked() {
                            actions.push(Action::NewTab);
                        }
                        ui.add_space(8.0);
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            for (i, title) in titles.iter().enumerate() {
                                let row = egui_chrome::tab_row(ui, &c, title, i == active);
                                if row.close {
                                    actions.push(Action::CloseTab(i));
                                } else if row.duplicate {
                                    actions.push(Action::DuplicateTab(i));
                                } else if row.close_others {
                                    actions.push(Action::CloseOtherTabs(i));
                                } else if row.clicked {
                                    actions.push(Action::SwitchTab(i));
                                }
                            }
                        });
                    });
                });
        }

        // Notice strip.
        if self.notice_active()
            && !fullscreen
            && let Some((message, _)) = self.notice.clone()
        {
            egui::Panel::bottom("notice")
                .exact_size(NOTICE_HEIGHT)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(c.hover))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new(message).size(13.0).color(c.text));
                    });
                });
        }

        let popup_now = egui::Popup::is_any_open(&ctx);
        if popup_now != self.egui_popup_open {
            self.egui_popup_open = popup_now;
            // Show or hide the web view, and draw the snapshot, on the next frame.
            ctx.request_repaint();
        }
        let snapshot_id = self.popup_snapshot.as_ref().map(|t| t.id());

        // The page: the engine raster as a texture; the WebView2 pane sits
        // exactly over this rectangle.
        let central = egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let rect = ui.max_rect();
                if let Some((id, [w, h])) = tex {
                    let size = egui::vec2(w as f32 / ppp, h as f32 / ppp);
                    let at = egui::Rect::from_min_size(rect.min, size);
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    ui.painter().image(id, at, uv, egui::Color32::WHITE);
                }
                if let Some(id) = snapshot_id {
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    ui.painter().image(id, rect, uv, egui::Color32::WHITE);
                }
                rect
            });
        let r = central.inner;
        self.egui_page = Some(Rect {
            x: (r.min.x * ppp).round(),
            y: (r.min.y * ppp).round(),
            w: (r.width() * ppp).round().max(1.0),
            h: (r.height() * ppp).round().max(1.0),
        });

        let changed = !actions.is_empty();
        for action in actions {
            match action {
                Action::Back => self.go(-1),
                Action::Forward => self.go(1),
                Action::Reload => self.reload(),
                Action::Stop => self.stop(),
                Action::Home => self.home(),
                Action::Theme => self.toggle_theme(),
                Action::Flags => self.load(Request::Get("browsecraft://flags".into()), true),
                Action::Navigate(text) => {
                    ctx.memory_mut(|m| m.surrender_focus(egui_chrome::address_id()));
                    self.navigate_input(&text);
                }
                Action::RestoreAddress => {
                    ctx.memory_mut(|m| m.surrender_focus(egui_chrome::address_id()));
                    let url = self.current_url();
                    self.address.set(&url);
                }
                Action::NewTab => self.new_tab(),
                Action::SwitchTab(i) => self.switch_tab(i),
                Action::CloseTab(i) => self.close_tab(i),
                Action::DuplicateTab(i) => {
                    let url = self.tab_url(i);
                    if url.is_empty() {
                        self.new_tab();
                    } else {
                        self.open_in_new_tab(url);
                    }
                }
                Action::CloseOtherTabs(i) => self.close_other_tabs(i),
                Action::ToggleBookmark => self.toggle_bookmark(),
                Action::OpenBookmark(url) => self.load(Request::Get(url), true),
                Action::OpenInNewTab(url) => self.open_in_new_tab(url),
                Action::RemoveBookmark(url) => {
                    bookmarks::remove(&url);
                    self.set_notice("Bookmark removed");
                    self.refresh_if_internal("browsecraft://bookmarks");
                }
                Action::BookmarksPage => self.load(Request::Get("browsecraft://bookmarks".into()), true),
                Action::FillLogin(id) => self.fill_login(&id),
                Action::SuggestPassword => self.suggest_password(),
                Action::SaveLogin => match passwords::save_offer() {
                    Ok(message) => self.set_notice(&message),
                    Err(message) => self.set_notice(&message),
                },
                Action::DismissOffer => passwords::dismiss_offer(),
                Action::NeverSave => {
                    passwords::never_offer_for_site();
                    self.set_notice("BrowseCraft will not offer to save logins on this site");
                }
                Action::LockVault => {
                    passwords::lock();
                    self.set_notice("Vault locked");
                }
                Action::PasswordsPage => self.load(Request::Get("browsecraft://passwords".into()), true),
            }
        }
        if changed {
            self.request_redraw();
        }
    }

    fn handle_message(&mut self, message: UserMessage) {
        match message {
            UserMessage::Load(generation, event) => {
                if generation == self.generation.load(Ordering::SeqCst) {
                    self.handle_load(event);
                }
            }
            UserMessage::WebStart(url) => self.web_started(url),
            UserMessage::WebLoad(url) => self.web_loaded(url),
            UserMessage::WebTitle(title) => self.web_titled(title),
            UserMessage::WebOpen(url) => self.web_open(url),
            UserMessage::WebKey(shortcut) => self.web_key(shortcut),
            UserMessage::WebFullscreen(on) => self.set_web_fullscreen(on),
            UserMessage::Page(message) => self.page_message(message),
            UserMessage::Stop => self.stop(),
        }
        self.request_redraw();
    }
}

impl ApplicationHandler<UserMessage> for Browser {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let window_icon = image::load_from_memory(icons::LOGO_PNG)
            .map_err(|e| eprintln!("browsecraft: could not decode the window logo: {e}"))
            .ok()
            .and_then(|image| {
                let rgba = image
                    .resize(64, 64, image::imageops::FilterType::Lanczos3)
                    .into_rgba8();
                let (width, height) = rgba.dimensions();
                winit::window::Icon::from_rgba(rgba.into_raw(), width, height).ok()
            });
        let attrs = Window::default_attributes()
            .with_title("BrowseCraft")
            .with_window_icon(window_icon)
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(LogicalSize::new(480.0, 320.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("browsecraft: could not create a window: {e}");
                event_loop.exit();
                return;
            }
        };
        self.create_webview(&window);

        // Chrome: egui on the GPU when asked for and available, otherwise the
        // engine draws it into a softbuffer frame.
        let egui = if flags::get().chrome == Chrome::Engine {
            None
        } else {
            EguiChrome::new(window.clone())
        };
        let (mut surface, mut context) = (None, None);
        if egui_chrome::resolve(egui.is_some()) == egui_chrome::Choice::Egui {
            self.egui = egui;
        } else if let Ok(ctx) = Context::new(window.clone()) {
            surface = Surface::new(&ctx, window.clone()).ok();
            context = Some(ctx);
        }
        self.gfx = Some(Gfx {
            window: window.clone(),
            surface,
            _context: context,
        });
        self.apply_theme(self.dark());
        let start = self.start_url.clone();
        self.load(Request::Get(start), true);
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let consumed = match (&mut self.egui, &self.gfx) {
            (Some(egui), Some(g)) => egui.on_window_event(&g.window, &event),
            _ => false,
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.render(),
            WindowEvent::Resized(size) => {
                if let Some(egui) = &mut self.egui {
                    egui.on_resized(size.width, size.height);
                }
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } | WindowEvent::ThemeChanged(_) => {
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if !consumed || self.is_shortcut(&event) {
                    self.key(&event);
                }
                self.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse = (position.x as f32, position.y as f32);
                self.mouse_moved();
            }
            WindowEvent::CursorLeft { .. } => {
                self.hover_button = None;
                self.hover_tab = None;
                self.hover_link = None;
                self.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => match state {
                ElementState::Pressed => {
                    // A click egui took over the page area belongs to a menu
                    // or popup drawn on top of it: not to the page below.
                    if !(consumed && self.page_rect().contains(self.mouse.0, self.mouse.1)) {
                        self.mouse_down(button);
                    }
                }
                ElementState::Released => {
                    if button == MouseButton::Left {
                        self.drag = Drag::None;
                    }
                }
            },
            WindowEvent::MouseWheel { delta, .. } => {
                if !consumed || self.page_rect().contains(self.mouse.0, self.mouse.1) {
                    self.wheel(delta);
                }
            }
            _ => {}
        }
        if self.quit {
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, message: UserMessage) {
        self.handle_message(message);
        if self.quit {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.quit {
            event_loop.exit();
            return;
        }
        let now = Instant::now();
        let mut wake: Option<Instant> = None;
        let mut sooner = |t: Instant| wake = Some(wake.map_or(t, |w| w.min(t)));
        if let Some((_, at)) = &self.notice {
            let until = *at + NOTICE_LIFETIME;
            if now >= until {
                self.notice = None;
                self.request_redraw();
            } else {
                sooner(until);
            }
        }
        if self.loading {
            // Animate the loading bar.
            self.request_redraw();
            sooner(now + Duration::from_millis(50));
        }
        event_loop.set_control_flow(match wake {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html;

    #[test]
    fn shortcuts() {
        let ch = |c: &str| Key::Character(c.into());
        assert_eq!(shortcut_for(&ch("t"), true, false, false, false), Some(Shortcut::NewTab));
        assert_eq!(shortcut_for(&ch("S"), true, true, false, false), Some(Shortcut::Screenshot));
        assert_eq!(shortcut_for(&ch("s"), true, false, false, false), Some(Shortcut::FocusAddress));
        assert_eq!(shortcut_for(&ch("c"), true, false, false, false), None);
        assert_eq!(shortcut_for(&ch("t"), false, false, false, false), None);
        let left = Key::Named(NamedKey::ArrowLeft);
        assert_eq!(shortcut_for(&left, true, false, false, false), Some(Shortcut::Go(-1)));
        assert_eq!(shortcut_for(&left, true, false, false, true), None);
        assert_eq!(shortcut_for(&left, false, false, true, true), Some(Shortcut::Go(-1)));
        let tab = Key::Named(NamedKey::Tab);
        assert_eq!(shortcut_for(&tab, true, true, false, false), Some(Shortcut::CycleTab(-1)));
        let f5 = Key::Named(NamedKey::F5);
        assert_eq!(shortcut_for(&f5, false, false, false, false), Some(Shortcut::Reload));
        let f11 = Key::Named(NamedKey::F11);
        assert_eq!(shortcut_for(&f11, false, false, false, false), Some(Shortcut::Fullscreen));
        assert_eq!(shortcut_for(&ch("d"), true, false, false, false), Some(Shortcut::Bookmark));
        assert_eq!(shortcut_for(&ch("B"), true, true, false, false), Some(Shortcut::BookmarksBar));
        assert_eq!(shortcut_for(&ch("b"), true, false, false, false), None);
        assert_eq!(shortcut_for(&ch("O"), true, true, false, false), Some(Shortcut::BookmarksPage));
    }

    #[test]
    fn bars_between_toolbar_and_page_are_counted() {
        // The page area (layout, painting and click mapping) must start where
        // egui's central panel starts, below every bar.
        assert_eq!(extra_bars_h(1.0, true, true, false), 34.0);
        assert_eq!(extra_bars_h(1.0, true, true, true), 76.0);
        assert_eq!(extra_bars_h(1.0, true, false, true), 42.0);
        assert_eq!(extra_bars_h(1.5, true, true, false), 51.0);
        assert_eq!(extra_bars_h(1.0, true, false, false), 0.0);
        assert_eq!(extra_bars_h(2.0, false, true, true), 0.0, "the engine chrome has neither bar");
    }

    #[test]
    fn same_internal_page_ignores_the_query() {
        assert!(same_internal_page("browsecraft://flags", "browsecraft://flags?token=x&theme=dark"));
        assert!(same_internal_page("browsecraft://flags/?a=1", "browsecraft://flags"));
        assert!(!same_internal_page("browsecraft://flags", "browsecraft://history"));
        assert!(!same_internal_page("https://a.test/?x", "https://a.test/?y"));
        assert!(!same_internal_page("about:home", "about:home"));
    }

    #[test]
    fn web_key_messages() {
        assert_eq!(web_shortcut_for(0x54, true, false, false), Some(Shortcut::NewTab));
        assert_eq!(web_shortcut_for(0x57, true, false, false), Some(Shortcut::CloseTab));
        assert_eq!(web_shortcut_for(0x51, true, false, false), Some(Shortcut::Quit));
        assert_eq!(web_shortcut_for(0x53, true, true, false), Some(Shortcut::Screenshot));
        assert_eq!(web_shortcut_for(0x53, true, false, false), Some(Shortcut::FocusAddress));
        assert_eq!(web_shortcut_for(0x09, true, true, false), Some(Shortcut::CycleTab(-1)));
        assert_eq!(web_shortcut_for(0x25, false, false, true), Some(Shortcut::Go(-1)));
        assert_eq!(web_shortcut_for(0x24, false, false, true), Some(Shortcut::Home));
        assert_eq!(web_shortcut_for(0x74, false, false, false), Some(Shortcut::Reload));
        assert_eq!(web_shortcut_for(0x75, false, false, false), Some(Shortcut::FocusAddress));
        assert_eq!(web_shortcut_for(0x7A, false, false, false), Some(Shortcut::Fullscreen));
        assert_eq!(web_shortcut_for(0x44, true, false, false), Some(Shortcut::Bookmark));
        assert_eq!(web_shortcut_for(0xBB, true, false, false), Some(Shortcut::Zoom(Some(true))));
        assert_eq!(web_shortcut_for(0xBD, true, false, false), Some(Shortcut::Zoom(Some(false))));
        assert_eq!(web_shortcut_for(0x30, true, false, false), Some(Shortcut::Zoom(None)));
        assert_eq!(web_shortcut_for(0x43, true, false, false), None); // Ctrl+C stays with WebView2
        assert_eq!(web_shortcut_for(0x0D, true, false, false), None); // Ctrl+Enter
    }

    #[test]
    fn webview_routing() {
        assert!(request_in_webview("https://example.com/"));
        assert!(request_in_webview("file:///C:/index.html"));
        assert!(!request_in_webview("about:home"));
        assert!(!request_in_webview("about:blocked:https://ads.example"));
        assert!(!request_in_webview("view-source:https://example.com"));
        assert!(!request_in_webview("browsecraft://history"));
        let (a, b) = ("https://slow.example/", "https://b.example/");
        assert!(stale_web_event(true, a, b, a));
        assert!(!stale_web_event(true, a, b, b));
        assert!(!stale_web_event(true, a, b, "https://b.example/redirected"));
        assert!(!stale_web_event(false, a, b, a), "a link followed later");
        assert!(!stale_web_event(true, a, a, a), "a reload");
        assert!(!stale_web_event(true, "", b, a));
        assert!(is_web_url("HTTP://example.com/") && is_web_url("https://x.example/"));
        assert!(!is_web_url("file:///C:/") && !is_web_url("about:blank"));
        assert_eq!(
            clean_title("  A\u{202E}gnp.exe \n\t B\u{0007} "),
            "Agnp.exe B"
        );
        assert_eq!(clean_title(&"x".repeat(1000)).len(), 300);
        assert!(popup_allowed("https://example.com/"));
        assert!(popup_allowed(" HTTP://example.com/"));
        for url in [
            "browsecraft://flags?theme=dark",
            "file:///C:/",
            "view-source:file://host/x",
            "about:blocked:https://bank.example",
            "javascript:alert(1)",
        ] {
            assert!(!popup_allowed(url), "{url}");
        }
        assert!(!request_in_webview("BROWSECRAFT://flags"));
        for url in [
            "https://example.com/",
            "file:///C:/page.html",
            "data:text/html,hi",
            "about:blank",
            " About:Blank ",
        ] {
            assert!(navigation_allowed(url), "{url}");
        }
        for url in [
            "browsecraft://flags",
            "BrowseCraft://passwords?action=lock",
            "about:home",
            "about:blocked:https://bank.example",
            "view-source:https://attacker.example/",
        ] {
            assert!(!navigation_allowed(url), "{url}");
        }
    }

    #[test]
    fn zoom_steps_walk_the_table() {
        assert_eq!(zoom_step(1.0, true), 1.1);
        assert_eq!(zoom_step(1.0, false), 0.9);
        assert_eq!(zoom_step(3.0, true), 3.0);
        assert_eq!(zoom_step(0.33, false), 0.33);
    }

    #[test]
    fn screenshots_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("browsecraft-shots-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let names: Vec<_> = (0..3)
            .map(|_| {
                let (path, _) = create_screenshot_file(&dir, 0).unwrap();
                path.file_name().unwrap().to_string_lossy().into_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "browsecraft-19700101-000000.png",
                "browsecraft-19700101-000000-2.png",
                "browsecraft-19700101-000000-3.png"
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn screenshot_names_are_utc_stamps() {
        assert_eq!(screenshot_name(0), "browsecraft-19700101-000000.png");
        assert_eq!(screenshot_name(1_700_000_000), "browsecraft-20231114-221320.png");
        assert_eq!(screenshot_name(1_709_164_800), "browsecraft-20240229-000000.png");
    }

    #[test]
    fn every_shortcut_is_listed_once() {
        for needed in [
            "Shift + K", "Ctrl + H", "Ctrl + Shift + S", "Ctrl + Left", "Ctrl + Right", "Ctrl + S",
            "Ctrl + D", "Ctrl + Shift + B", "Ctrl + Shift + O", "F11",
        ] {
            assert_eq!(SHORTCUTS.iter().filter(|(k, _)| *k == needed).count(), 1, "{needed}");
        }
    }

    #[test]
    fn form_submission_collects_the_right_fields() {
        let doc = html::parse(
            r#"<form action="/go" method="get">
                <input name="q" value="hello world">
                <input type="hidden" name="h" value="1">
                <input type="checkbox" name="on" checked>
                <input type="checkbox" name="off">
                <input type="radio" name="r" value="a">
                <input type="radio" name="r" value="b" checked>
                <select name="s"><option value="x">First</option><option value="y" selected>Second</option></select>
                <textarea name="t">notes</textarea>
                <input type="submit" name="go" value="Search">
                <input type="submit" name="other" value="No">
            </form>"#,
        );
        let form = doc
            .descendants(Document::ROOT)
            .into_iter()
            .find(|&n| doc.tag(n) == "form")
            .unwrap();
        let submit = doc
            .descendants(form)
            .into_iter()
            .find(|&n| doc.element(n).and_then(|e| e.attr("name")) == Some("go"))
            .unwrap();
        let pairs = form_pairs(&doc, &FormState::default(), form, Some(submit));
        let names: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["q", "h", "on", "r", "s", "t", "go"]);
        let get = |k: &str| pairs.iter().find(|(n, _)| n == k).unwrap().1.clone();
        assert_eq!(get("q"), "hello world");
        assert_eq!(get("on"), "on");
        assert_eq!(get("r"), "b");
        assert_eq!(get("s"), "y");
        assert_eq!(get("go"), "Search");
    }
}
