//! The browser shell: window, toolbar, history, input handling and the
//! render loop. Pages are drawn by our own engine into a software buffer.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use softbuffer::{Context, Surface};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CursorIcon, Theme, Window, WindowId};

use crate::address;
use crate::css::Color;
use crate::dom::{Document, NodeId};
use crate::font::{FontKey, Fonts};
use crate::layout::{self, FormState, Layout, LayoutInput, Rect};
use crate::loader::{self, LoadEvent, Page, Request};
use crate::paint::{self, Canvas, Image};
use crate::style::{self, Style, StyleContext};
use crate::textfield::TextField;

pub const HOME_URL: &str = "about:home";
const TOOLBAR_HEIGHT: f32 = 44.0;
const ZOOM_STEPS: [f32; 13] = [0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 3.0];

pub struct UserEvent {
    generation: u64,
    event: LoadEvent,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Button {
    Back,
    Forward,
    Reload,
    Home,
}

const BUTTONS: [Button; 4] = [Button::Back, Button::Forward, Button::Reload, Button::Home];

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Page,
    Address,
    Input(NodeId),
}

struct HistoryEntry {
    request: Request,
    scroll: f32,
}

/// A loaded page plus everything derived from it.
struct Tab {
    page: Page,
    images: HashMap<String, Rc<Image>>,
    form: FormState,
    styles: Vec<Rc<Style>>,
    /// Media-query fingerprint and zoom the styles were computed for.
    style_key: Option<(Vec<bool>, f32)>,
    layout: Option<Layout>,
    /// Viewport width and zoom the layout was computed for.
    layout_key: (f32, f32),
    scroll_x: f32,
    scroll_y: f32,
    /// Scroll to apply once the next layout exists.
    pending_scroll: Option<PendingScroll>,
}

enum PendingScroll {
    To(f32),
    Fragment(String),
}

struct Gfx {
    window: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
}

pub struct Browser {
    gfx: Option<Gfx>,
    fonts: Fonts,
    proxy: EventLoopProxy<UserEvent>,
    generation: Arc<AtomicU64>,
    history: Vec<HistoryEntry>,
    index: usize,
    tab: Option<Tab>,
    loading: bool,
    start_url: String,

    address: TextField,
    address_scroll: f32,
    input: TextField,
    focus: Focus,
    modifiers: ModifiersState,
    mouse: (f32, f32),
    hover_button: Option<Button>,
    hover_link: Option<String>,
    user_zoom: f32,
    clipboard: Option<arboard::Clipboard>,
    last_click: (Instant, f32, f32),
    /// The frame is drawn here, then copied to the window surface.
    frame: Vec<u32>,
}

impl Browser {
    pub fn new(proxy: EventLoopProxy<UserEvent>, fonts: Fonts, start_url: String) -> Self {
        Browser {
            gfx: None,
            fonts,
            proxy,
            generation: Arc::new(AtomicU64::new(0)),
            history: Vec::new(),
            index: 0,
            tab: None,
            loading: false,
            start_url,
            address: TextField::default(),
            address_scroll: 0.0,
            input: TextField::default(),
            focus: Focus::Page,
            modifiers: ModifiersState::empty(),
            mouse: (0.0, 0.0),
            hover_button: None,
            hover_link: None,
            user_zoom: 1.0,
            clipboard: None,
            last_click: (Instant::now(), -1.0, -1.0),
            frame: Vec::new(),
        }
    }

    fn window(&self) -> Option<&Rc<Window>> {
        self.gfx.as_ref().map(|g| &g.window)
    }

    fn redraw(&self) {
        if let Some(w) = self.window() {
            w.request_redraw();
        }
    }

    fn scale(&self) -> f32 {
        self.window().map_or(1.0, |w| w.scale_factor() as f32)
    }

    fn zoom(&self) -> f32 {
        self.scale() * self.user_zoom
    }

    fn toolbar_height(&self) -> f32 {
        (TOOLBAR_HEIGHT * self.scale()).round()
    }

    fn window_size(&self) -> (f32, f32) {
        self.window().map_or((1.0, 1.0), |w| {
            let s = w.inner_size();
            (s.width as f32, s.height as f32)
        })
    }

    fn viewport(&self) -> (f32, f32) {
        let (w, h) = self.window_size();
        (w, (h - self.toolbar_height()).max(1.0))
    }

    fn dark(&self) -> bool {
        self.window().and_then(|w| w.theme()) == Some(Theme::Dark)
    }

    // ---- Navigation ----

    fn navigate(&mut self, request: Request) {
        // Same-document fragment links just scroll.
        if let (Request::Get(target), Some(tab)) = (&request, &mut self.tab)
            && let (Ok(to), Ok(from)) = (Url::parse(target), Url::parse(&tab.page.url))
                && let Some(fragment) = to.fragment() {
                    let mut a = to.clone();
                    let mut b = from.clone();
                    a.set_fragment(None);
                    b.set_fragment(None);
                    if a == b {
                        let fragment = fragment.to_string();
                        if let Some(entry) = self.history.get_mut(self.index) {
                            entry.scroll = tab.scroll_y;
                        }
                        self.history.truncate(self.index + 1);
                        self.history.push(HistoryEntry { request: request.clone(), scroll: 0.0 });
                        self.index = self.history.len() - 1;
                        tab.page.url = target.clone();
                        tab.pending_scroll = Some(PendingScroll::Fragment(fragment));
                        self.address.set(target);
                        self.redraw();
                        return;
                    }
                }
        if let (Some(entry), Some(tab)) = (self.history.get_mut(self.index), &self.tab) {
            entry.scroll = tab.scroll_y;
        }
        if !self.history.is_empty() {
            self.history.truncate(self.index + 1);
        }
        self.history.push(HistoryEntry { request: request.clone(), scroll: 0.0 });
        self.index = self.history.len() - 1;
        self.load(request);
    }

    fn load(&mut self, request: Request) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.loading = true;
        if self.focus != Focus::Address {
            self.address.set(request.url());
            self.address_scroll = 0.0;
        }
        let proxy = self.proxy.clone();
        loader::spawn(request, generation, self.generation.clone(), move |generation, event| {
            let _ = proxy.send_event(UserEvent { generation, event });
        });
        self.redraw();
    }

    fn go(&mut self, delta: isize) {
        let target = self.index as isize + delta;
        if target < 0 || target as usize >= self.history.len() {
            return;
        }
        if let Some(tab) = &self.tab {
            self.history[self.index].scroll = tab.scroll_y;
        }
        self.index = target as usize;
        let request = self.history[self.index].request.clone();
        self.load(request);
    }

    fn reload(&mut self) {
        if let Some(tab) = &self.tab
            && let Some(entry) = self.history.get_mut(self.index) {
                entry.scroll = tab.scroll_y;
            }
        if let Some(entry) = self.history.get(self.index) {
            let request = entry.request.clone();
            self.load(request);
        }
    }

    fn stop(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.loading = false;
        self.redraw();
    }

    fn on_load_event(&mut self, event: UserEvent) {
        if event.generation != self.generation.load(Ordering::SeqCst) {
            return;
        }
        match event.event {
            LoadEvent::Page(page) => {
                let restore = self.history.get(self.index).map_or(0.0, |e| e.scroll);
                let pending = match Url::parse(&page.url).ok().and_then(|u| u.fragment().map(str::to_string)) {
                    Some(f) if restore == 0.0 => PendingScroll::Fragment(f),
                    _ => PendingScroll::To(restore),
                };
                if let Some(entry) = self.history.get_mut(self.index)
                    && let Request::Get(url) = &mut entry.request
                        && !url.starts_with("view-source:") {
                            *url = page.url.clone();
                        }
                let title = page.doc.title();
                if let Some(w) = self.window() {
                    w.set_title(&if title.is_empty() { "Prowl".to_string() } else { format!("{title} - Prowl") });
                }
                if self.focus != Focus::Address {
                    self.address.set(&page.url);
                    self.address_scroll = 0.0;
                }
                let autofocus = page
                    .doc
                    .descendants(Document::ROOT)
                    .into_iter()
                    .find(|&n| page.doc.element(n).is_some_and(|e| e.attr("autofocus").is_some() && e.tag == "input"));
                self.tab = Some(Tab {
                    page,
                    images: HashMap::new(),
                    form: FormState::default(),
                    styles: Vec::new(),
                    style_key: None,
                    layout: None,
                    layout_key: (0.0, 0.0),
                    scroll_x: 0.0,
                    scroll_y: 0.0,
                    pending_scroll: Some(pending),
                });
                self.hover_link = None;
                self.focus = match (self.focus, autofocus) {
                    (Focus::Address, _) => Focus::Address,
                    (_, Some(node)) => {
                        self.input = TextField::new("");
                        Focus::Input(node)
                    }
                    _ => Focus::Page,
                };
            }
            LoadEvent::Image { src, image } => {
                if let Some(tab) = &mut self.tab {
                    tab.images.insert(src, Rc::new(image));
                    tab.layout = None;
                }
            }
            LoadEvent::Finished => self.loading = false,
        }
        self.redraw();
    }

    // ---- Styling and layout ----

    fn ensure_layout(&mut self) {
        let zoom = self.zoom();
        let (vw, vh) = self.viewport();
        let Some(tab) = &mut self.tab else { return };
        let signature = style::media_signature(&tab.page.sheets, vw / zoom);
        let key = (signature, zoom);
        if tab.style_key.as_ref() != Some(&key) {
            let ctx = StyleContext { zoom, viewport_width: vw, viewport_height: vh };
            tab.styles = style::compute_styles(&tab.page.doc, &tab.page.sheets, &ctx);
            tab.style_key = Some(key);
            tab.layout = None;
        }
        if tab.layout.is_some() && tab.layout_key == (vw, zoom) {
            return;
        }
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
        let new_layout = layout::layout(&input);
        // Keep the same content in view when a relayout changes heights.
        if let Some(old) = &tab.layout
            && tab.layout_key.0 != vw && old.height > 0.0 {
                tab.scroll_y *= new_layout.height / old.height;
            }
        tab.layout = Some(new_layout);
        tab.layout_key = (vw, zoom);

        if let Some(pending) = tab.pending_scroll.take() {
            let layout = tab.layout.as_ref().expect("layout");
            match pending {
                PendingScroll::To(y) => tab.scroll_y = y,
                PendingScroll::Fragment(name) => {
                    let decoded = percent_decode(&name);
                    if let Some(node) = tab.page.doc.find_anchor(&decoded) {
                        if let Some(&(_, y)) = layout.anchors.iter().find(|(n, _)| *n == node) {
                            tab.scroll_y = y;
                        }
                    } else if decoded.is_empty() || decoded == "top" {
                        tab.scroll_y = 0.0;
                    }
                }
            }
        }
        Self::clamp_scroll(tab, vw, vh);
    }

    fn clamp_scroll(tab: &mut Tab, vw: f32, vh: f32) {
        let (w, h) = tab.layout.as_ref().map_or((0.0, 0.0), |l| (l.width, l.height));
        tab.scroll_y = tab.scroll_y.clamp(0.0, (h - vh).max(0.0));
        tab.scroll_x = tab.scroll_x.clamp(0.0, (w - vw).max(0.0));
    }

    fn scroll_by(&mut self, dx: f32, dy: f32) {
        let (vw, vh) = self.viewport();
        if let Some(tab) = &mut self.tab {
            tab.scroll_x += dx;
            tab.scroll_y += dy;
            Self::clamp_scroll(tab, vw, vh);
            self.update_hover();
            self.redraw();
        }
    }

    fn invalidate_layout(&mut self) {
        if let Some(tab) = &mut self.tab {
            tab.layout = None;
        }
        self.redraw();
    }

    fn set_zoom(&mut self, zoom: f32) {
        self.user_zoom = zoom;
        if let Some(tab) = &mut self.tab {
            tab.style_key = None;
        }
        self.redraw();
    }

    fn zoom_step(&mut self, dir: i32) {
        let current = ZOOM_STEPS.iter().position(|z| (*z - self.user_zoom).abs() < 0.01).unwrap_or(6) as i32;
        let next = (current + dir).clamp(0, ZOOM_STEPS.len() as i32 - 1);
        self.set_zoom(ZOOM_STEPS[next as usize]);
    }

    // ---- Rendering ----

    fn render(&mut self) {
        self.ensure_layout();
        let (w, h) = self.window_size();
        let (wu, hu) = (w as u32, h as u32);
        let (Some(nw), Some(nh)) = (NonZeroU32::new(wu), NonZeroU32::new(hu)) else { return };
        let bar = self.toolbar_height();
        let scale = self.scale();
        let dark = self.dark();
        let mut frame = std::mem::take(&mut self.frame);
        frame.resize(wu as usize * hu as usize, 0);
        let mut canvas = Canvas::new(&mut frame, wu as usize, hu as usize);

        let page_rect = Rect { x: 0.0, y: bar, w, h: h - bar };
        canvas.push_clip(page_rect);
        match self.tab.as_ref().and_then(|t| t.layout.as_ref().map(|l| (t, l))) {
            Some((tab, layout)) => {
                canvas.fill_rect(page_rect, layout.background);
                paint::paint_items(&mut canvas, &layout.items, &self.fonts, &tab.images, -tab.scroll_x, bar - tab.scroll_y);
                if let Focus::Input(node) = self.focus {
                    draw_input_caret(&mut canvas, &self.fonts, layout, node, &self.input, -tab.scroll_x, bar - tab.scroll_y);
                }
                draw_scrollbar(&mut canvas, page_rect, tab.scroll_y, layout.height, scale);
            }
            None => canvas.fill_rect(page_rect, if dark { Color::rgb(0x20, 0x20, 0x20) } else { Color::WHITE }),
        }
        if let Some(link) = &self.hover_link {
            draw_status(&mut canvas, &self.fonts, link, page_rect, scale, dark);
        }
        canvas.pop_clip();
        self.draw_toolbar(&mut canvas, w);

        if let Some(gfx) = &mut self.gfx
            && gfx.surface.resize(nw, nh).is_ok()
                && let Ok(mut buffer) = gfx.surface.buffer_mut() {
                    buffer.copy_from_slice(&frame);
                    let _ = buffer.present();
                }
        self.frame = frame;
    }

    fn button_rect(&self, i: usize) -> Rect {
        let s = self.scale();
        Rect { x: (6.0 + i as f32 * 36.0) * s, y: 6.0 * s, w: 32.0 * s, h: 32.0 * s }
    }

    fn address_rect(&self) -> Rect {
        let s = self.scale();
        let (w, _) = self.window_size();
        let x = (6.0 + 4.0 * 36.0 + 6.0) * s;
        Rect { x, y: 6.0 * s, w: (w - x - 8.0 * s).max(10.0), h: 32.0 * s }
    }

    fn button_enabled(&self, b: Button) -> bool {
        match b {
            Button::Back => self.index > 0,
            Button::Forward => self.index + 1 < self.history.len(),
            _ => true,
        }
    }

    fn draw_toolbar(&mut self, canvas: &mut Canvas, w: f32) {
        let s = self.scale();
        let dark = self.dark();
        let bar = self.toolbar_height();
        let (bg, line, field_bg, text, muted, hover, accent) = if dark {
            (0x2b2b2b, 0x3c3c3c, 0x1e1e1e, 0xe8e8e8, 0x777777, 0x3d3d3d, 0x5b9cff)
        } else {
            (0xf3f3f3, 0xd6d6d6, 0xffffff, 0x1f1f1f, 0xb4b4b4, 0xe1e1e1, 0x2f6fde)
        };
        let c = |hex: u32| Color::rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        canvas.fill_rect(Rect { x: 0.0, y: 0.0, w, h: bar }, c(bg));
        canvas.fill_rect(Rect { x: 0.0, y: bar - s.max(1.0), w, h: s.max(1.0) }, c(line));

        for (i, &button) in BUTTONS.iter().enumerate() {
            let r = self.button_rect(i);
            let enabled = self.button_enabled(button);
            if enabled && self.hover_button == Some(button) {
                canvas.fill_rounded(r, 6.0 * s, c(hover), None);
            }
            let icon = match button {
                Button::Back => "←",
                Button::Forward => "→",
                Button::Reload if self.loading => "✕",
                Button::Reload => "↻",
                Button::Home => "⌂",
            };
            let size = 18.0 * s;
            let iw = self.fonts.measure(FontKey::UI, icon, size);
            let (asc, desc) = self.fonts.vertical_metrics(FontKey::UI, size);
            let baseline = r.y + (r.h - asc - desc) / 2.0 + asc;
            canvas.draw_text(&self.fonts, r.x + (r.w - iw) / 2.0, baseline, icon, FontKey::UI, size, if enabled { c(text) } else { c(muted) });
        }

        let field = self.address_rect();
        let focused = self.focus == Focus::Address;
        canvas.fill_rounded(field, 8.0 * s, c(field_bg), None);
        let border_w = if focused { 2.0 * s } else { s.max(1.0) };
        let inner = Rect { x: field.x + border_w, y: field.y + border_w, w: field.w - 2.0 * border_w, h: field.h - 2.0 * border_w };
        canvas.fill_rounded(field, 8.0 * s, if focused { c(accent) } else { c(line) }, Some((inner, 8.0 * s - border_w)));

        let size = 14.0 * s;
        let pad = 12.0 * s;
        let text_area = Rect { x: field.x + pad, y: field.y, w: field.w - 2.0 * pad, h: field.h };
        let caret_x = self.fonts.measure(FontKey::UI, &self.address.text[..self.address.cursor], size);
        if focused {
            if caret_x - self.address_scroll > text_area.w {
                self.address_scroll = caret_x - text_area.w;
            } else if caret_x < self.address_scroll {
                self.address_scroll = caret_x;
            }
        } else {
            self.address_scroll = 0.0;
        }
        let (asc, desc) = self.fonts.vertical_metrics(FontKey::UI, size);
        let baseline = (field.y + (field.h - asc - desc) / 2.0 + asc).round();
        let x0 = text_area.x - self.address_scroll;
        canvas.push_clip(text_area);
        let sel = self.address.selection();
        if focused && !sel.is_empty() {
            let sx = self.fonts.measure(FontKey::UI, &self.address.text[..sel.start], size);
            let sw = self.fonts.measure(FontKey::UI, &self.address.text[sel.clone()], size);
            let highlight = Color { a: 90, ..c(accent) };
            canvas.fill_rect(Rect { x: x0 + sx, y: baseline - asc, w: sw, h: asc + desc }, highlight);
        }
        if self.address.text.is_empty() && !focused {
            canvas.draw_text(&self.fonts, x0, baseline, "Search or enter address", FontKey::UI, size, c(muted));
        } else {
            canvas.draw_text(&self.fonts, x0, baseline, &self.address.text, FontKey::UI, size, c(text));
        }
        if focused && sel.is_empty() {
            canvas.fill_rect(Rect { x: (x0 + caret_x).round(), y: baseline - asc, w: s.max(1.0), h: asc + desc }, c(text));
        }
        canvas.pop_clip();

        if self.loading {
            let h = (2.0 * s).max(2.0);
            canvas.fill_rect(Rect { x: 0.0, y: bar - h, w: w * 0.35, h }, c(accent));
        }
    }

    // ---- Hit testing ----

    /// Converts a window position to document coordinates.
    fn to_doc(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let bar = self.toolbar_height();
        let tab = self.tab.as_ref()?;
        (y >= bar).then_some((x + tab.scroll_x, y - bar + tab.scroll_y))
    }

    fn link_at(&self, x: f32, y: f32) -> Option<String> {
        let (dx, dy) = self.to_doc(x, y)?;
        let tab = self.tab.as_ref()?;
        let layout = tab.layout.as_ref()?;
        let (_, href) = layout.links.iter().rev().find(|(r, _)| r.contains(dx, dy))?;
        resolve_href(&tab.page, href)
    }

    fn control_at(&self, x: f32, y: f32) -> Option<NodeId> {
        let (dx, dy) = self.to_doc(x, y)?;
        let layout = self.tab.as_ref()?.layout.as_ref()?;
        layout.controls.iter().rev().find(|c| c.rect.contains(dx, dy)).map(|c| c.node)
    }

    fn update_hover(&mut self) {
        let (x, y) = self.mouse;
        let hover_button = BUTTONS
            .iter()
            .enumerate()
            .find(|(i, _)| self.button_rect(*i).contains(x, y))
            .map(|(_, b)| *b);
        let link = self.link_at(x, y);
        let control = self.control_at(x, y);
        let cursor = if hover_button.is_some_and(|b| self.button_enabled(b)) || link.is_some() {
            CursorIcon::Pointer
        } else if self.address_rect().contains(x, y) {
            CursorIcon::Text
        } else if let Some(node) = control {
            if self.is_text_control(node) { CursorIcon::Text } else { CursorIcon::Pointer }
        } else {
            CursorIcon::Default
        };
        if let Some(w) = self.window() {
            w.set_cursor(cursor);
        }
        if hover_button != self.hover_button || link != self.hover_link {
            self.hover_button = hover_button;
            self.hover_link = link;
            self.redraw();
        }
    }

    fn is_text_control(&self, node: NodeId) -> bool {
        let Some(tab) = &self.tab else { return false };
        let Some(el) = tab.page.doc.element(node) else { return false };
        match el.tag.as_str() {
            "textarea" => true,
            "input" => matches!(
                el.attr("type").unwrap_or("text").to_ascii_lowercase().as_str(),
                "text" | "search" | "email" | "url" | "password" | "tel" | "number" | "" | "date" | "time"
            ),
            _ => false,
        }
    }

    fn click(&mut self, x: f32, y: f32) {
        let double = self.last_click.0.elapsed().as_millis() < 400
            && (self.last_click.1 - x).abs() < 4.0
            && (self.last_click.2 - y).abs() < 4.0;
        self.last_click = (Instant::now(), x, y);

        if y < self.toolbar_height() {
            for (i, &button) in BUTTONS.iter().enumerate() {
                if self.button_rect(i).contains(x, y) && self.button_enabled(button) {
                    match button {
                        Button::Back => self.go(-1),
                        Button::Forward => self.go(1),
                        Button::Reload if self.loading => self.stop(),
                        Button::Reload => self.reload(),
                        Button::Home => self.navigate(Request::Get(HOME_URL.into())),
                    }
                    return;
                }
            }
            if self.address_rect().contains(x, y) {
                if self.focus != Focus::Address || double {
                    self.focus = Focus::Address;
                    self.address.select_all();
                } else {
                    let rel = x - self.address_rect().x - 12.0 * self.scale() + self.address_scroll;
                    let size = 14.0 * self.scale();
                    let pos = caret_index(&self.fonts, &self.address.text, size, rel);
                    self.address.move_to(pos, self.modifiers.shift_key());
                }
                self.redraw();
            }
            return;
        }

        if let Some(node) = self.control_at(x, y) {
            self.activate_control(node, x);
            return;
        }
        if let Some(url) = self.link_at(x, y) {
            self.focus = Focus::Page;
            self.follow(url);
            return;
        }
        self.focus = Focus::Page;
        self.redraw();
    }

    fn follow(&mut self, url: String) {
        if url.starts_with("javascript:") || url.starts_with("mailto:") || url.starts_with("tel:") {
            return;
        }
        self.navigate(Request::Get(url));
    }

    fn activate_control(&mut self, node: NodeId, click_x: f32) {
        if self.is_text_control(node) {
            let tab = self.tab.as_ref().expect("tab");
            let value = tab.form.value(&tab.page.doc, node).to_string();
            let already = self.focus == Focus::Input(node);
            if !already {
                self.input = TextField::new(&value);
            }
            if let Some(control) = tab.layout.as_ref().and_then(|l| l.controls.iter().find(|c| c.node == node)) {
                let rel = click_x + tab.scroll_x - control.text_x;
                if already || !value.is_empty() {
                    let pos = caret_index(&self.fonts, &value, control.size, rel);
                    self.input.move_to(pos, already && self.modifiers.shift_key());
                }
            }
            self.focus = Focus::Input(node);
            self.redraw();
            return;
        }
        self.focus = Focus::Page;
        let Some(tab) = &mut self.tab else { return };
        let doc = &mut tab.page.doc;
        let Some(el) = doc.element(node) else { return };
        let tag = el.tag.clone();
        let input_type = el.attr("type").unwrap_or("").to_ascii_lowercase();
        match (tag.as_str(), input_type.as_str()) {
            ("input", "checkbox") => {
                let checked = tab.form.is_checked(doc, node);
                tab.form.checked.insert(node, !checked);
                self.invalidate_layout();
            }
            ("input", "radio") => {
                let name = el.attr("name").unwrap_or("").to_string();
                let form = form_of(doc, node);
                let group: Vec<NodeId> = doc
                    .descendants(form.unwrap_or(Document::ROOT))
                    .into_iter()
                    .filter(|&n| {
                        doc.element(n).is_some_and(|e| {
                            e.tag == "input"
                                && e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("radio"))
                                && e.attr("name").unwrap_or("") == name
                        })
                    })
                    .collect();
                for n in group {
                    tab.form.checked.insert(n, n == node);
                }
                self.invalidate_layout();
            }
            ("select", _) => {
                // No dropdown: each click selects the next option.
                let options: Vec<String> = doc
                    .descendants(node)
                    .into_iter()
                    .filter(|&n| doc.tag(n) == "option")
                    .map(|n| doc.text_content(n).trim().to_string())
                    .collect();
                if !options.is_empty() {
                    let current = tab.form.values.get(&node).cloned().unwrap_or_else(|| {
                        doc.descendants(node)
                            .into_iter()
                            .find(|&n| doc.tag(n) == "option" && doc.element(n).is_some_and(|e| e.attr("selected").is_some()))
                            .map(|n| doc.text_content(n).trim().to_string())
                            .unwrap_or_else(|| options[0].clone())
                    });
                    let i = options.iter().position(|o| *o == current).map_or(0, |i| (i + 1) % options.len());
                    tab.form.values.insert(node, options[i].clone());
                    self.invalidate_layout();
                }
            }
            ("summary", _) => {
                if let Some(details) = doc.parent_element(node).filter(|&p| doc.tag(p) == "details") {
                    if let Some(d) = doc.element_mut(details) {
                        if let Some(i) = d.attrs.iter().position(|(n, _)| n == "open") {
                            d.attrs.remove(i);
                        } else {
                            d.attrs.push(("open".into(), String::new()));
                        }
                    }
                    tab.style_key = None;
                    self.redraw();
                }
            }
            ("input", "submit" | "image") => self.submit(node, Some(node)),
            ("button", _) if !matches!(input_type.as_str(), "button" | "reset") => self.submit(node, Some(node)),
            _ => {}
        }
    }

    fn submit(&mut self, from: NodeId, submitter: Option<NodeId>) {
        let Some(tab) = &self.tab else { return };
        let doc = &tab.page.doc;
        let Some(form) = form_of(doc, from) else { return };
        let form_el = doc.element(form).expect("form");
        let mut pairs: Vec<(String, String)> = Vec::new();
        for n in doc.descendants(form) {
            let Some(el) = doc.element(n) else { continue };
            let Some(name) = el.attr("name").filter(|n| !n.is_empty()) else { continue };
            if el.attr("disabled").is_some() {
                continue;
            }
            let ty = el.attr("type").unwrap_or("text").to_ascii_lowercase();
            match el.tag.as_str() {
                "input" => match ty.as_str() {
                    "submit" | "button" | "reset" | "image" => {
                        if submitter == Some(n) {
                            if ty == "image" {
                                pairs.push((format!("{name}.x"), "0".into()));
                                pairs.push((format!("{name}.y"), "0".into()));
                            } else if ty == "submit" {
                                pairs.push((name.into(), el.attr("value").unwrap_or("Submit").into()));
                            }
                        }
                    }
                    "checkbox" | "radio" => {
                        if tab.form.is_checked(doc, n) {
                            pairs.push((name.into(), el.attr("value").unwrap_or("on").into()));
                        }
                    }
                    "file" => {}
                    _ => {
                        let value = if Focus::Input(n) == self.focus { self.input.text.clone() } else { tab.form.value(doc, n).to_string() };
                        pairs.push((name.into(), value));
                    }
                },
                "button" => {
                    if submitter == Some(n) {
                        pairs.push((name.into(), el.attr("value").unwrap_or("").into()));
                    }
                }
                "textarea" => pairs.push((name.into(), tab.form.value(doc, n).to_string())),
                "select" => {
                    let options: Vec<NodeId> = doc.descendants(n).into_iter().filter(|&o| doc.tag(o) == "option").collect();
                    let chosen = match tab.form.values.get(&n) {
                        Some(text) => options.iter().find(|&&o| doc.text_content(o).trim() == text),
                        None => options
                            .iter()
                            .find(|&&o| doc.element(o).is_some_and(|e| e.attr("selected").is_some()))
                            .or(options.first()),
                    };
                    if let Some(&o) = chosen {
                        let value = doc
                            .element(o)
                            .and_then(|e| e.attr("value"))
                            .map(str::to_string)
                            .unwrap_or_else(|| doc.text_content(o).trim().to_string());
                        pairs.push((name.into(), value));
                    }
                }
                _ => {}
            }
        }
        let encoded = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(&pairs).finish();
        let action = submitter
            .and_then(|s| doc.element(s))
            .and_then(|e| e.attr("formaction"))
            .or_else(|| form_el.attr("action"))
            .filter(|a| !a.trim().is_empty());
        let target = match action {
            Some(a) => tab.page.base.join(a.trim()).ok(),
            None => Url::parse(&tab.page.url).ok(),
        };
        let Some(mut target) = target else { return };
        let post = form_el.attr("method").is_some_and(|m| m.eq_ignore_ascii_case("post"));
        self.focus = Focus::Page;
        if post {
            self.navigate(Request::Post(target.to_string(), encoded));
        } else {
            target.set_query(Some(&encoded));
            target.set_fragment(None);
            self.navigate(Request::Get(target.to_string()));
        }
    }

    // ---- Keyboard ----

    fn clipboard_get(&mut self) -> Option<String> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()?.get_text().ok()
    }

    fn clipboard_set(&mut self, text: &str) {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        if let Some(c) = &mut self.clipboard {
            let _ = c.set_text(text.to_string());
        }
    }

    fn key(&mut self, event: winit::event::KeyEvent, event_loop: &ActiveEventLoop) {
        let ctrl = self.modifiers.control_key() || self.modifiers.super_key();
        let alt = self.modifiers.alt_key();
        let shift = self.modifiers.shift_key();
        let base = event.key_without_modifiers();
        let ch = match &base {
            Key::Character(c) => c.to_lowercase(),
            _ => String::new(),
        };
        let named = match &event.logical_key {
            Key::Named(n) => Some(*n),
            _ => None,
        };

        // Browser-wide shortcuts.
        match (ctrl, alt, named, ch.as_str()) {
            (_, _, Some(NamedKey::F5), _) | (true, false, _, "r") => return self.reload(),
            (true, false, _, "l") | (_, _, Some(NamedKey::F6), _) => {
                self.focus = Focus::Address;
                self.address.select_all();
                return self.redraw();
            }
            (false, true, Some(NamedKey::ArrowLeft), _) => return self.go(-1),
            (false, true, Some(NamedKey::ArrowRight), _) => return self.go(1),
            (false, true, Some(NamedKey::Home), _) => return self.navigate(Request::Get(HOME_URL.into())),
            (true, false, _, "=" | "+") => return self.zoom_step(1),
            (true, false, _, "-") => return self.zoom_step(-1),
            (true, false, _, "0") => return self.set_zoom(1.0),
            (true, false, _, "u") => {
                if let Some(tab) = &self.tab
                    && !tab.page.url.starts_with("view-source:") && !tab.page.url.starts_with("about:") {
                        let url = format!("view-source:{}", tab.page.url);
                        self.navigate(Request::Get(url));
                    }
                return;
            }
            (true, false, _, "w") | (true, false, _, "q") => return event_loop.exit(),
            _ => {}
        }

        match self.focus {
            Focus::Address => {
                match named {
                    Some(NamedKey::Enter) => {
                        let url = address::resolve(&self.address.text);
                        if url.is_empty() || self.address.text.trim().is_empty() {
                            return;
                        }
                        self.focus = Focus::Page;
                        self.address.set(&url);
                        self.navigate(Request::Get(url));
                        return;
                    }
                    Some(NamedKey::Escape) => {
                        let current = self.tab.as_ref().map(|t| t.page.url.clone()).unwrap_or_default();
                        self.address.set(&current);
                        self.focus = Focus::Page;
                    }
                    _ => {
                        let mut field = std::mem::take(&mut self.address);
                        self.edit(&mut field, &event, ctrl, shift, &ch);
                        self.address = field;
                    }
                }
                self.redraw();
            }
            Focus::Input(node) => {
                match named {
                    Some(NamedKey::Enter) => {
                        let multiline = self.tab.as_ref().is_some_and(|t| t.page.doc.tag(node) == "textarea");
                        if multiline {
                            self.input.insert(" ");
                        } else {
                            self.commit_input(node);
                            self.submit(node, None);
                            return;
                        }
                    }
                    Some(NamedKey::Escape) => self.focus = Focus::Page,
                    Some(NamedKey::Tab) => {
                        self.commit_input(node);
                        self.focus_next_input(node, shift);
                        return;
                    }
                    _ => {
                        let mut field = std::mem::take(&mut self.input);
                        self.edit(&mut field, &event, ctrl, shift, &ch);
                        self.input = field;
                    }
                }
                self.commit_input(node);
            }
            Focus::Page => {
                let (_, vh) = self.viewport();
                let line = 48.0 * self.zoom();
                match named {
                    Some(NamedKey::ArrowDown) => self.scroll_by(0.0, line),
                    Some(NamedKey::ArrowUp) => self.scroll_by(0.0, -line),
                    Some(NamedKey::ArrowRight) => self.scroll_by(line, 0.0),
                    Some(NamedKey::ArrowLeft) => self.scroll_by(-line, 0.0),
                    Some(NamedKey::PageDown) => self.scroll_by(0.0, vh * 0.9),
                    Some(NamedKey::PageUp) => self.scroll_by(0.0, -vh * 0.9),
                    Some(NamedKey::Space) => self.scroll_by(0.0, if shift { -vh * 0.9 } else { vh * 0.9 }),
                    Some(NamedKey::Home) => self.scroll_by(0.0, f32::NEG_INFINITY.max(-1e9)),
                    Some(NamedKey::End) => self.scroll_by(0.0, 1e9),
                    Some(NamedKey::Backspace) if !ctrl => self.go(-1),
                    _ => {}
                }
            }
        }
    }

    /// Applies an editing key to `field`.
    fn edit(&mut self, field: &mut TextField, event: &winit::event::KeyEvent, ctrl: bool, shift: bool, ch: &str) {
        match &event.logical_key {
            Key::Named(NamedKey::Backspace) => field.backspace(ctrl),
            Key::Named(NamedKey::Delete) => field.delete(ctrl),
            Key::Named(NamedKey::ArrowLeft) => field.left(shift, ctrl),
            Key::Named(NamedKey::ArrowRight) => field.right(shift, ctrl),
            Key::Named(NamedKey::Home) => field.move_to(0, shift),
            Key::Named(NamedKey::End) => field.move_to(field.text.len(), shift),
            _ if ctrl => match ch {
                "a" => field.select_all(),
                "c" => {
                    let text = field.selected_text().to_string();
                    if !text.is_empty() {
                        self.clipboard_set(&text);
                    }
                }
                "x" => {
                    let text = field.selected_text().to_string();
                    if !text.is_empty() {
                        self.clipboard_set(&text);
                        field.insert("");
                    }
                }
                "v" => {
                    if let Some(text) = self.clipboard_get() {
                        field.insert(&text.replace(['\r', '\n'], " "));
                    }
                }
                _ => {}
            },
            _ => {
                if let Some(text) = &event.text
                    && !text.chars().any(char::is_control) {
                        field.insert(text);
                    }
            }
        }
    }

    /// Stores the focused field's text in the form state.
    fn commit_input(&mut self, node: NodeId) {
        if let Some(tab) = &mut self.tab {
            let changed = tab.form.values.get(&node) != Some(&self.input.text);
            tab.form.values.insert(node, self.input.text.clone());
            if changed {
                tab.layout = None;
            }
        }
        self.redraw();
    }

    fn focus_next_input(&mut self, from: NodeId, backwards: bool) {
        let Some(tab) = &self.tab else { return };
        let Some(layout) = &tab.layout else { return };
        let nodes: Vec<NodeId> = layout.controls.iter().map(|c| c.node).filter(|&n| self.is_text_control(n)).collect();
        let Some(i) = nodes.iter().position(|&n| n == from) else { return };
        let next = if backwards { i.checked_sub(1) } else { Some(i + 1) }.and_then(|j| nodes.get(j)).copied();
        if let Some(next) = next {
            let value = tab.form.value(&tab.page.doc, next).to_string();
            self.input = TextField::new(&value);
            self.input.select_all();
            self.focus = Focus::Input(next);
        }
        self.redraw();
    }
}

impl ApplicationHandler<UserEvent> for Browser {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Prowl")
            .with_inner_size(LogicalSize::new(1200.0, 800.0))
            .with_min_inner_size(LogicalSize::new(400.0, 200.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("failed to create window: {e}");
                return event_loop.exit();
            }
        };
        let surface = Context::new(window.clone()).and_then(|ctx| Surface::new(&ctx, window.clone()));
        match surface {
            Ok(surface) => self.gfx = Some(Gfx { window, surface }),
            Err(e) => {
                eprintln!("failed to create drawing surface: {e}");
                return event_loop.exit();
            }
        }
        let start = std::mem::take(&mut self.start_url);
        self.navigate(Request::Get(start));
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        self.on_load_event(event);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.render(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } | WindowEvent::ThemeChanged(_) => self.redraw(),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse = (position.x as f32, position.y as f32);
                self.update_hover();
            }
            WindowEvent::CursorLeft { .. } => {
                self.mouse = (-1.0, -1.0);
                self.update_hover();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => match button {
                MouseButton::Left => self.click(self.mouse.0, self.mouse.1),
                MouseButton::Back => self.go(-1),
                MouseButton::Forward => self.go(1),
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        let line = 48.0 * self.zoom();
                        (-x * line, -y * line)
                    }
                    MouseScrollDelta::PixelDelta(p) => (-p.x as f32, -p.y as f32),
                };
                if self.modifiers.control_key() {
                    self.zoom_step(if dy < 0.0 { 1 } else { -1 });
                } else if self.modifiers.shift_key() {
                    self.scroll_by(dy, dx);
                } else {
                    self.scroll_by(dx, dy);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.key(event, event_loop);
            }
            _ => {}
        }
    }
}

/// The `<form>` that owns a control: its `form` attribute, or the nearest
/// ancestor form.
fn form_of(doc: &Document, node: NodeId) -> Option<NodeId> {
    if let Some(id) = doc.element(node).and_then(|e| e.attr("form"))
        && let Some(f) = doc.find_anchor(id).filter(|&f| doc.tag(f) == "form") {
            return Some(f);
        }
    doc.ancestors(node).find(|&a| doc.tag(a) == "form")
}

fn resolve_href(page: &Page, href: &str) -> Option<String> {
    let href = href.trim();
    if href.starts_with("javascript:") {
        return None;
    }
    if page.base.scheme() == "about" {
        return Url::parse(href).ok().map(|u| u.to_string());
    }
    page.base.join(href).ok().map(|u| u.to_string())
}

fn caret_index(fonts: &Fonts, text: &str, size: f32, x: f32) -> usize {
    let mut pos = 0.0;
    for (i, c) in text.char_indices() {
        let w = fonts.advance(FontKey::UI, c, size);
        if x < pos + w / 2.0 {
            return i;
        }
        pos += w;
    }
    text.len()
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| s.to_string())
}

fn draw_input_caret(canvas: &mut Canvas, fonts: &Fonts, layout: &Layout, node: NodeId, field: &TextField, dx: f32, dy: f32) {
    let Some(control) = layout.controls.iter().find(|c| c.node == node) else { return };
    let (asc, desc) = fonts.vertical_metrics(control.font, control.size);
    let x = control.text_x + fonts.measure(control.font, &field.text[..field.cursor], control.size) + dx;
    let y = control.baseline + dy;
    let sel = field.selection();
    if !sel.is_empty() {
        let sx = control.text_x + fonts.measure(control.font, &field.text[..sel.start], control.size) + dx;
        let sw = fonts.measure(control.font, &field.text[sel], control.size);
        canvas.fill_rect(Rect { x: sx, y: y - asc, w: sw, h: asc + desc }, Color { r: 0x2f, g: 0x6f, b: 0xde, a: 80 });
    }
    // Keep the caret inside the control.
    let r = control.rect;
    let x = x.min(r.x + r.w + dx - 2.0);
    canvas.fill_rect(Rect { x: x.round(), y: y - asc, w: 1.0_f32.max(control.size / 14.0), h: asc + desc }, Color::BLACK);
}

fn draw_scrollbar(canvas: &mut Canvas, view: Rect, scroll: f32, content_h: f32, scale: f32) {
    if content_h <= view.h + 1.0 {
        return;
    }
    let track_h = view.h - 4.0 * scale;
    let thumb_h = (view.h / content_h * track_h).max(24.0 * scale);
    let max_scroll = content_h - view.h;
    let y = view.y + 2.0 * scale + (scroll / max_scroll) * (track_h - thumb_h);
    let w = 6.0 * scale;
    let rect = Rect { x: view.x + view.w - w - 2.0 * scale, y, w, h: thumb_h };
    canvas.fill_rounded(rect, w / 2.0, Color { r: 0, g: 0, b: 0, a: 90 }, None);
}

fn draw_status(canvas: &mut Canvas, fonts: &Fonts, text: &str, view: Rect, scale: f32, dark: bool) {
    let size = 12.0 * scale;
    let pad = 8.0 * scale;
    let max_w = view.w * 0.6;
    let mut shown = text.to_string();
    if fonts.measure(FontKey::UI, &shown, size) > max_w {
        while !shown.is_empty() && fonts.measure(FontKey::UI, &shown, size) + fonts.measure(FontKey::UI, "…", size) > max_w {
            shown.pop();
        }
        shown.push('…');
    }
    let w = fonts.measure(FontKey::UI, &shown, size) + 2.0 * pad;
    let h = 22.0 * scale;
    let rect = Rect { x: view.x, y: view.y + view.h - h, w, h };
    let (bg, fg) = if dark { (Color::rgb(0x2b, 0x2b, 0x2b), Color::rgb(0xe8, 0xe8, 0xe8)) } else { (Color::rgb(0xf3, 0xf3, 0xf3), Color::rgb(0x1f, 0x1f, 0x1f)) };
    canvas.fill_rect(rect, bg);
    canvas.fill_rect(Rect { x: rect.x, y: rect.y, w: rect.w, h: scale.max(1.0) }, Color::rgb(0xc8, 0xc8, 0xc8));
    canvas.fill_rect(Rect { x: rect.x + rect.w - scale.max(1.0), y: rect.y, w: scale.max(1.0), h: rect.h }, Color::rgb(0xc8, 0xc8, 0xc8));
    let (asc, desc) = fonts.vertical_metrics(FontKey::UI, size);
    canvas.draw_text(fonts, rect.x + pad, rect.y + (h - asc - desc) / 2.0 + asc, &shown, FontKey::UI, size, fg);
}
