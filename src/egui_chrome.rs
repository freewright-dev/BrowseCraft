//! GPU-rendered browser chrome (toolbar + tab sidebar) using `egui` on
//! `wgpu`, with the engine's software raster uploaded as a texture.
//!
//! This is the Phase 2 path. It is only constructed when the `chrome` flag
//! asks for it and a wgpu adapter/device can actually be created; otherwise
//! the browser falls back to the engine/softbuffer chrome in `browser.rs`.
//!
//! The engine keeps painting every frame into `Browser::frame`; here that
//! buffer is handed to egui as a texture and drawn inside the central panel,
//! while the toolbar and sidebar become real egui widgets.

use std::sync::Arc;

use winit::event::WindowEvent;
use winit::window::Window;

use crate::flags::{self, Chrome};

/// The GPU-rendered chrome. Holds the egui context, the winit bridge and the
/// wgpu painter. `Browser` owns one of these when the egui path is active.
pub struct EguiChrome {
    ctx: egui::Context,
    state: egui_winit::State,
    painter: egui_wgpu::winit::Painter,
    viewport_id: egui::ViewportId,
}

impl EguiChrome {
    /// Tries to create the GPU chrome for `window`. Returns `None` when no
    /// adapter/device is available so the caller can keep the engine chrome.
    pub fn new(window: Arc<Window>) -> Option<Self> {
        let ctx = egui::Context::default();
        install_icon_font(&ctx);
        let viewport_id = egui::ViewportId::ROOT;
        let mut state = egui_winit::State::new(
            ctx.clone(),
            viewport_id,
            &*window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        let mut painter = pollster::block_on(egui_wgpu::winit::Painter::new(
            ctx.clone(),
            egui_wgpu::WgpuConfiguration::default(),
            false,
            egui_wgpu::RendererOptions::default(),
        ));
        pollster::block_on(painter.set_window(viewport_id, Some(window.clone()))).ok()?;
        painter.render_state()?;
        if let Some(max) = painter.max_texture_side() {
            state.set_max_texture_side(max);
        }
        Some(EguiChrome {
            ctx,
            state,
            painter,
            viewport_id,
        })
    }

    /// Feeds a winit event into egui. Returns `true` when egui consumed it.
    pub fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        let response = self.state.on_window_event(window, event);
        if response.repaint {
            window.request_redraw();
        }
        response.consumed
    }

    /// The largest texture side the GPU accepts. A bigger texture fails wgpu
    /// validation, which panics.
    pub fn max_texture_side(&self) -> usize {
        self.painter.max_texture_side().unwrap_or(2048)
    }

    pub fn on_resized(&mut self, w: u32, h: u32) {
        if let (Some(w), Some(h)) = (std::num::NonZeroU32::new(w), std::num::NonZeroU32::new(h)) {
            self.painter.on_window_resized(self.viewport_id, w, h);
        }
    }

    /// Runs one egui frame and presents it. `build` draws the chrome; it may
    /// mutate browser state directly. The engine raster `frame`
    /// (`w × h` physical pixels, `0x00RRGGBB`) is uploaded as the page texture
    /// shown in the central panel (when `show_page` is true).
    pub fn frame<R>(&mut self, window: &Arc<Window>, raw_input: egui::RawInput, build: R)
    where
        R: FnMut(&mut egui::Ui),
    {
        let mut full_output = self.ctx.run_ui(raw_input, build);
        let ppp = self.ctx.pixels_per_point();
        self.state
            .handle_platform_output(window, full_output.platform_output.clone());
        let paint_jobs = self
            .ctx
            .tessellate(std::mem::take(&mut full_output.shapes), ppp);
        let _ = self.painter.paint_and_update_textures(
            self.viewport_id,
            ppp,
            [0.0, 0.0, 0.0, 0.0],
            &paint_jobs,
            &mut full_output.textures_delta,
            Vec::new(),
            window,
        );
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    pub fn take_raw_input(&mut self, window: &Window) -> egui::RawInput {
        self.state.take_egui_input(window)
    }
}

/// The central panel is only a painted page, but egui's `consumed` flag
/// includes its background. Inside that page, only overlays or a drag that
/// started on a chrome widget should keep pointer input from the page engine.
pub fn blocks_page_pointer(ctx: &egui::Context, position: egui::Pos2) -> bool {
    egui::Popup::is_any_open(ctx)
        || ctx.egui_is_using_pointer()
        || ctx.layer_id_at(position) != Some(egui::LayerId::background())
}

/// Which chrome the flags ask for, given whether the GPU path came up.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Engine,
    Egui,
}

/// Resolves the `chrome` flag: `engine` always uses the engine path, while
/// `auto`/`egui` use the GPU path when it successfully initialised.
pub fn resolve(gpu_available: bool) -> Choice {
    match flags::get().chrome {
        Chrome::Engine => Choice::Engine,
        Chrome::Egui | Chrome::Auto if gpu_available => Choice::Egui,
        Chrome::Egui | Chrome::Auto => Choice::Engine,
    }
}

// ---- Icons and theming -----------------------------------------------------

// Phosphor codepoints used by the chrome (see `crate::icons`). Re-exported
// under the historical `ICON_*` names for the call sites in `browser.rs`.
pub use crate::icons::{
    BACK as ICON_BACK, BOOKMARKS as ICON_BOOKMARKS, CLOSE as ICON_STOP, CLOSE as ICON_XMARK,
    FORWARD as ICON_FORWARD, GEAR as ICON_GEAR, GLOBE as ICON_GLOBE, HOME as ICON_HOME,
    KEY as ICON_KEY, LOCK as ICON_LOCK, MOON as ICON_MOON, PLUS as ICON_PLUS,
    RELOAD as ICON_RELOAD, STAR as ICON_STAR, SUN as ICON_SUN,
};

/// Icon text at `size` points, in the Phosphor family.
pub fn icon(code: &str, size: f32) -> egui::RichText {
    egui::RichText::new(code)
        .family(crate::icons::family())
        .size(size)
}

/// A compact square icon button that shows its label on hover. Uses the theme
/// visuals for hover/press feedback.
pub fn icon_button(
    ui: &mut egui::Ui,
    enabled: bool,
    code: &str,
    tooltip: &str,
    size: f32,
) -> egui::Response {
    let button = egui::Button::new(icon(code, size))
        .min_size(egui::vec2(32.0, 32.0))
        .corner_radius(egui::CornerRadius::same(10));
    let response = ui.add_enabled(enabled, button);
    if enabled {
        response.on_hover_text(tooltip)
    } else {
        response
    }
}

/// Stable id of the address-bar text field, so focus can be requested from
/// keyboard shortcuts elsewhere in the frame.
pub fn address_id() -> egui::Id {
    egui::Id::new("bc-address")
}

/// The rounded address field: a leading scheme icon, the text, and (for pages
/// that can be bookmarked) a star on the right. `star` is `Some(is_starred)`
/// or `None` when the page cannot be bookmarked. Returns the text edit's
/// response and whether the star was clicked.
pub fn address_bar(
    ui: &mut egui::Ui,
    text: &mut String,
    width: f32,
    c: &Colors,
    leading: &str,
    star: Option<bool>,
) -> (egui::Response, bool) {
    let focused = ui.memory(|m| m.has_focus(address_id()));
    let stroke = if focused {
        egui::Stroke::new(1.5, c.accent)
    } else {
        egui::Stroke::new(1.0, c.line)
    };
    let mut star_clicked = false;
    let response = egui::Frame::NONE
        .fill(c.field_bg)
        .stroke(stroke)
        .corner_radius(egui::CornerRadius::same(16))
        .inner_margin(egui::Margin::symmetric(12, 3))
        .show(ui, |ui| {
            ui.set_width((width - 24.0).max(40.0));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(icon(leading, 13.0).color(c.muted));
                let star_w = if star.is_some() { 26.0 } else { 0.0 };
                let response = ui.add_sized(
                    [(ui.available_width() - star_w).max(20.0), 24.0],
                    egui::TextEdit::singleline(text)
                        .id(address_id())
                        .frame(egui::Frame::NONE)
                        .text_color(c.text)
                        .hint_text(egui::RichText::new("Search or enter address").color(c.muted)),
                );
                if let Some(on) = star {
                    let color = if on { c.accent } else { c.muted };
                    let tip = if on { "Remove bookmark (Ctrl+D)" } else { "Bookmark this page (Ctrl+D)" };
                    let button = egui::Button::new(icon(ICON_STAR, 15.0).color(color))
                        .frame(false)
                        .min_size(egui::vec2(24.0, 24.0));
                    star_clicked = ui.add(button).on_hover_text(tip).clicked();
                }
                response
            })
            .inner
        })
        .inner;
    (response, star_clicked)
}

/// What the user did in the bookmarks bar.
pub enum BarEvent {
    Open(String),
    OpenInNewTab(String),
    Remove(String),
    Manage,
}

/// A small icon-and-label entry for popup menus.
pub fn menu_item(ui: &mut egui::Ui, c: &Colors, code: &str, label: &str) -> egui::Response {
    let text = egui::RichText::new(format!("{code}  {label}")).size(13.0).color(c.text);
    ui.add(egui::Button::new(text).frame(false).min_size(egui::vec2(180.0, 26.0)))
}

/// The bookmarks bar contents: one chip per bookmark that fits, an overflow
/// menu for the rest, and an "All bookmarks" button on the right.
pub fn bookmarks_bar(ui: &mut egui::Ui, c: &Colors, items: &[(String, String)]) -> Vec<BarEvent> {
    const CHIP_MAX_TEXT: f32 = 150.0;
    const CHIP_PAD: f32 = 10.0;
    const ICON_W: f32 = 14.0;
    const GAP: f32 = 4.0;
    let mut events = Vec::new();
    let font = egui::FontId::proportional(13.0);
    let reserve = 128.0; // the "All bookmarks" button
    let budget = (ui.available_width() - reserve - 8.0).max(0.0);

    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        ui.add_space(8.0);
        let mut used = 0.0;
        let mut shown = 0;
        let mut overflow: Vec<&(String, String)> = Vec::new();
        for item in items {
            let text_w = ui
                .painter()
                .layout_no_wrap(item.0.clone(), font.clone(), c.text)
                .size()
                .x
                .min(CHIP_MAX_TEXT);
            let w = CHIP_PAD * 2.0 + ICON_W + 6.0 + text_w;
            if overflow.is_empty() && used + w + GAP <= budget - 30.0 {
                used += w + GAP;
                shown += 1;
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 26.0), egui::Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(13), c.hover);
                }
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink2(egui::vec2(CHIP_PAD, 0.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                child.spacing_mut().item_spacing.x = 6.0;
                child.label(icon(crate::icons::GLOBE, 12.0).color(c.accent));
                child.add(
                    egui::Label::new(egui::RichText::new(&item.0).size(13.0).color(c.text))
                        .truncate()
                        .selectable(false),
                );
                let resp = resp.on_hover_text(&item.1);
                if resp.clicked() {
                    events.push(BarEvent::Open(item.1.clone()));
                } else if resp.middle_clicked() {
                    events.push(BarEvent::OpenInNewTab(item.1.clone()));
                }
                resp.context_menu(|ui| {
                    if menu_item(ui, c, crate::icons::GLOBE, "Open").clicked() {
                        events.push(BarEvent::Open(item.1.clone()));
                        ui.close();
                    }
                    if menu_item(ui, c, crate::icons::PLUS, "Open in new tab").clicked() {
                        events.push(BarEvent::OpenInNewTab(item.1.clone()));
                        ui.close();
                    }
                    if menu_item(ui, c, crate::icons::TRASH, "Remove bookmark").clicked() {
                        events.push(BarEvent::Remove(item.1.clone()));
                        ui.close();
                    }
                });
            } else {
                overflow.push(item);
            }
        }
        if items.is_empty() {
            ui.label(
                egui::RichText::new("Bookmarks you add show up here. Click the star in the address bar.")
                    .size(12.5)
                    .color(c.muted),
            );
        }
        let _ = shown;
        if !overflow.is_empty() {
            ui.menu_button(egui::RichText::new("\u{bb}").size(15.0).color(c.text), |ui| {
                for item in overflow {
                    let label = egui::RichText::new(&item.0).size(13.0).color(c.text);
                    if ui
                        .add(egui::Button::new(label).frame(false).min_size(egui::vec2(220.0, 26.0)))
                        .on_hover_text(&item.1)
                        .clicked()
                    {
                        events.push(BarEvent::Open(item.1.clone()));
                        ui.close();
                    }
                }
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = egui::RichText::new(format!("{}  All bookmarks", ICON_BOOKMARKS))
                .size(12.5)
                .color(c.muted);
            let button = egui::Button::new(label)
                .frame(false)
                .corner_radius(egui::CornerRadius::same(13))
                .min_size(egui::vec2(0.0, 26.0));
            if ui.add(button).on_hover_text("Open the bookmarks page (Ctrl+Shift+O)").clicked() {
                events.push(BarEvent::Manage);
            }
            ui.add_space(6.0);
        });
    });
    events
}

/// Selects the whole address field (used when the address bar is focused by
/// shortcut, matching the engine chrome's select-all behaviour).
pub fn select_all(ctx: &egui::Context, id: egui::Id, chars: usize) {
    if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) {
        let range = egui::text::CCursorRange::two(
            egui::text::CCursor::new(0usize),
            egui::text::CCursor::new(chars),
        );
        state.cursor.set_char_range(Some(range));
        state.store(ctx, id);
    }
}

/// Outcome of drawing one tab row.
pub struct TabRow {
    pub clicked: bool,
    pub close: bool,
    /// The user picked "Duplicate tab" from the context menu.
    pub duplicate: bool,
    /// The user picked "Close other tabs" from the context menu.
    pub close_others: bool,
}

/// One row in the tab sidebar: a globe, the (truncated) title and a close
/// button that appears on the active/hovered row. The whole row is clickable.
pub fn tab_row(ui: &mut egui::Ui, c: &Colors, title: &str, active: bool) -> TabRow {
    let (rect, row) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click());
    let hovered = row.hovered();
    let painter = ui.painter();
    if active {
        painter.rect_filled(rect, egui::CornerRadius::same(7), c.hover);
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.min.x, rect.min.y + 6.0),
            egui::vec2(3.0, rect.height() - 12.0),
        );
        painter.rect_filled(bar, egui::CornerRadius::same(2), c.accent);
    } else if hovered {
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(7),
            c.hover.gamma_multiply(0.6),
        );
    }

    let inner = rect.shrink2(egui::vec2(10.0, 0.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 7.0;
    let icon_color = if active { c.accent } else { c.muted };
    child.label(icon(ICON_GLOBE, 12.0).color(icon_color));

    let show_close = active || hovered;
    let label_w = (child.available_width() - if show_close { 26.0 } else { 0.0 }).max(10.0);
    let label = egui::Label::new(egui::RichText::new(title).size(13.0).color(c.text))
        .truncate()
        .selectable(false)
        .sense(egui::Sense::click());
    let label_resp = child.add_sized([label_w, 20.0], label);

    let mut close = false;
    if show_close {
        child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let resp = ui.add(
                egui::Button::new(icon(ICON_XMARK, 11.0).color(c.muted))
                    .frame(false)
                    .min_size(egui::vec2(20.0, 20.0)),
            );
            if resp.on_hover_text("Close tab").clicked() {
                close = true;
            }
        });
    }
    let (mut duplicate, mut close_others) = (false, false);
    row.context_menu(|ui| {
        if menu_item(ui, c, crate::icons::COPY, "Duplicate tab").clicked() {
            duplicate = true;
            ui.close();
        }
        if menu_item(ui, c, crate::icons::CLOSE, "Close tab").clicked() {
            close = true;
            ui.close();
        }
        if menu_item(ui, c, crate::icons::TRASH, "Close other tabs").clicked() {
            close_others = true;
            ui.close();
        }
    });
    TabRow {
        clicked: row.clicked() || label_resp.clicked(),
        close,
        duplicate,
        close_others,
    }
}

/// Registers the embedded Phosphor face with egui. The font ships inside the
/// binary, so no asset lookup can fail.
fn install_icon_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    crate::icons::add_fonts(&mut fonts);
    ctx.set_fonts(fonts);
}

/// The chrome palette, matching the engine-drawn toolbar/sidebar.
pub struct Colors {
    pub bar_bg: egui::Color32,
    pub sidebar_bg: egui::Color32,
    pub line: egui::Color32,
    pub field_bg: egui::Color32,
    pub text: egui::Color32,
    pub muted: egui::Color32,
    pub hover: egui::Color32,
    pub accent: egui::Color32,
}

impl Colors {
    pub fn for_dark(dark: bool) -> Self {
        if dark {
            Self {
                bar_bg: rgb(0x1b1e24),
                sidebar_bg: rgb(0x16181d),
                line: rgb(0x2c313a),
                field_bg: rgb(0x14161a),
                text: rgb(0xe8eaee),
                muted: rgb(0x8b93a1),
                hover: rgb(0x2a2f38),
                accent: rgb(0x5b8def),
            }
        } else {
            Self {
                bar_bg: rgb(0xf6f7f9),
                sidebar_bg: rgb(0xeceef2),
                line: rgb(0xdde1e7),
                field_bg: rgb(0xffffff),
                text: rgb(0x1c1f24),
                muted: rgb(0x667085),
                hover: rgb(0xe3e6ec),
                accent: rgb(0x2f6fde),
            }
        }
    }
}

pub fn rgb(hex: u32) -> egui::Color32 {
    egui::Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Installs the chrome visuals and spacing for the current theme. Called once
/// per frame before the panels are built.
pub fn apply_style(ctx: &egui::Context, dark: bool) {
    let c = Colors::for_dark(dark);
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = c.bar_bg;
    v.window_fill = c.bar_bg;
    v.extreme_bg_color = c.field_bg;
    v.faint_bg_color = c.sidebar_bg;
    v.hyperlink_color = c.accent;
    v.selection.bg_fill = c.accent.gamma_multiply(0.40);
    v.selection.stroke = egui::Stroke::new(1.0, c.text);

    let radius = egui::CornerRadius::same(9);
    v.widgets.noninteractive.corner_radius = radius;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, c.text);

    v.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    v.widgets.inactive.corner_radius = radius;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, c.text);

    v.widgets.hovered.weak_bg_fill = c.hover;
    v.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    v.widgets.hovered.corner_radius = radius;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, c.text);

    v.widgets.active.weak_bg_fill = c.accent;
    v.widgets.active.bg_stroke = egui::Stroke::NONE;
    v.widgets.active.corner_radius = radius;
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);

    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(4.0, 4.0);
        s.spacing.button_padding = egui::vec2(7.0, 3.0);
        s.spacing.interact_size = egui::vec2(28.0, 28.0);
    });
}

// ---- password manager ----

/// What the key menu shows.
pub struct PasswordMenu<'a> {
    pub status: &'a crate::passwords::Status,
    /// Logins saved for the current site (empty while locked).
    pub logins: &'a [crate::vault::Credential],
    /// The kind of password form the page has, if any.
    pub forms: Option<crate::passwords::FormKind>,
    /// A login waiting to be saved.
    pub offer: Option<&'a crate::passwords::Offer>,
    pub on_web_page: bool,
    pub host: &'a str,
}

pub enum PasswordEvent {
    Fill(String),
    Suggest,
    Save,
    Manage,
    Lock,
}

/// A full-width flat button for the key menu.
fn menu_button(ui: &mut egui::Ui, c: &Colors, text: &str) -> egui::Response {
    ui.add_sized(
        [ui.available_width(), 30.0],
        egui::Button::new(egui::RichText::new(text).size(13.0).color(c.text))
            .fill(c.hover)
            .corner_radius(egui::CornerRadius::same(9)),
    )
}

/// The contents of the password menu under the key icon.
pub fn password_menu(
    ui: &mut egui::Ui,
    c: &Colors,
    menu: &PasswordMenu,
    emit: &mut dyn FnMut(PasswordEvent),
) {
    use crate::passwords::{FormKind, Status};
    ui.set_min_width(280.0);
    ui.spacing_mut().item_spacing.y = 6.0;
    ui.horizontal(|ui| {
        ui.label(icon(ICON_KEY, 16.0).color(c.accent));
        ui.label(egui::RichText::new("Passwords").size(15.0).strong().color(c.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (label, color) = match menu.status {
                Status::Unlocked { .. } => ("Unlocked", c.accent),
                Status::Locked => ("Locked", c.muted),
                Status::NoVault => ("Not set up", c.muted),
            };
            ui.label(egui::RichText::new(label).size(12.0).color(color));
        });
    });
    ui.separator();
    match menu.status {
        Status::NoVault => {
            ui.label(
                egui::RichText::new("Keep your logins in a KeePass (.kdbx) vault, protected by a password or a key file.")
                    .size(12.5)
                    .color(c.muted),
            );
            if menu_button(ui, c, "Set up the password manager").clicked() {
                emit(PasswordEvent::Manage);
                ui.close();
            }
        }
        Status::Locked => {
            ui.label(egui::RichText::new("Your vault is locked.").size(12.5).color(c.muted));
            if menu_button(ui, c, &format!("{}  Unlock vault", ICON_KEY)).clicked() {
                emit(PasswordEvent::Manage);
                ui.close();
            }
        }
        Status::Unlocked { logins } => {
            if menu.on_web_page {
                if menu.logins.is_empty() {
                    let text = if menu.host.is_empty() {
                        "Nothing to fill on this page.".to_string()
                    } else {
                        format!("No saved logins for {}.", menu.host)
                    };
                    ui.label(egui::RichText::new(text).size(12.5).color(c.muted));
                }
                for login in menu.logins {
                    let name = if login.username.is_empty() { &login.title } else { &login.username };
                    let label = format!("{}  {}", crate::icons::USER, name);
                    if menu_button(ui, c, &label)
                        .on_hover_text(format!("Fill {} on this page", login.title))
                        .clicked()
                    {
                        emit(PasswordEvent::Fill(login.id.clone()));
                        ui.close();
                    }
                }
                if matches!(menu.forms, Some(FormKind::SignUp | FormKind::ChangePassword))
                    && menu_button(ui, c, &format!("{}  Suggest a strong password", crate::icons::LOCK_KEY)).clicked()
                {
                    emit(PasswordEvent::Suggest);
                    ui.close();
                }
                if menu.offer.is_some() && menu_button(ui, c, &format!("{}  Save this login", crate::icons::CHECK)).clicked() {
                    emit(PasswordEvent::Save);
                    ui.close();
                }
            } else {
                ui.label(egui::RichText::new("Open a web page to fill a login.").size(12.5).color(c.muted));
            }
            ui.separator();
            if menu_button(ui, c, &format!("{}  Manage passwords ({logins})", crate::icons::FOLDER)).clicked() {
                emit(PasswordEvent::Manage);
                ui.close();
            }
            if menu_button(ui, c, &format!("{}  Lock vault", crate::icons::LOCK_KEY)).clicked() {
                emit(PasswordEvent::Lock);
                ui.close();
            }
        }
    }
}

pub enum SaveBarEvent {
    Save,
    OpenPasswords,
    NotNow,
    Never,
}

/// The bar offering to save (or update) a login the page just submitted.
pub fn save_login_bar(
    ui: &mut egui::Ui,
    c: &Colors,
    offer: &crate::passwords::Offer,
    status: &crate::passwords::Status,
    emit: &mut dyn FnMut(SaveBarEvent),
) {
    use crate::passwords::Status;
    // The page chose the user name: show it cleaned and short.
    let name = crate::browser::display_text(&offer.username, 64);
    let who = if name.is_empty() {
        String::new()
    } else if name.chars().count() < offer.username.chars().count() && name.chars().count() == 64 {
        format!(" ({name}…)")
    } else {
        format!(" ({name})")
    };
    let (text, action_label, action) = match status {
        Status::Unlocked { .. } if offer.update_id.is_some() => (
            format!("Update the saved password for {}{who}?", offer.host),
            "Update",
            SaveBarEvent::Save,
        ),
        Status::Unlocked { .. } => (
            if offer.kind == crate::passwords::FormKind::SignUp {
                format!("Save the new account for {}{who}?", offer.host)
            } else {
                format!("Save the login for {}{who}?", offer.host)
            },
            "Save",
            SaveBarEvent::Save,
        ),
        Status::Locked => (
            format!("Unlock your vault to save the login for {}{who}.", offer.host),
            "Unlock",
            SaveBarEvent::OpenPasswords,
        ),
        Status::NoVault => (
            format!("Set up a password vault to save the login for {}{who}.", offer.host),
            "Set up",
            SaveBarEvent::OpenPasswords,
        ),
    };
    ui.horizontal_centered(|ui| {
        ui.add_space(12.0);
        ui.label(icon(ICON_KEY, 16.0).color(c.accent));
        ui.label(egui::RichText::new(text).size(13.0).color(c.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("Never for this site").size(12.5).color(c.muted))
                        .frame(false),
                )
                .clicked()
            {
                emit(SaveBarEvent::Never);
            }
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("Not now").size(12.5).color(c.text))
                        .fill(c.bar_bg)
                        .corner_radius(egui::CornerRadius::same(8)),
                )
                .clicked()
            {
                emit(SaveBarEvent::NotNow);
            }
            let primary = egui::Button::new(egui::RichText::new(action_label).size(12.5).color(egui::Color32::WHITE))
                .fill(c.accent)
                .corner_radius(egui::CornerRadius::same(8));
            if ui.add(primary).clicked() {
                match action {
                    SaveBarEvent::Save => emit(SaveBarEvent::Save),
                    _ => emit(SaveBarEvent::OpenPasswords),
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painted_page_accepts_pointer_but_chrome_drags_and_popups_do_not() {
        use egui::{Event, PointerButton, Pos2, Rect, Vec2};
        let ctx = egui::Context::default();
        let page_pos = egui::pos2(400.0, 300.0);
        let mut button = Rect::NOTHING;
        let mut frame = |events: Vec<Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                egui::Panel::top("toolbar").exact_size(44.0).show(ui, |ui| {
                    let response = ui.add(egui::Button::new("Menu").sense(egui::Sense::click_and_drag()));
                    button = response.rect;
                    egui::Popup::menu(&response).show(|ui| {
                        ui.label("Menu above the page");
                    });
                });
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.painter().rect_filled(ui.max_rect(), 0.0, egui::Color32::WHITE);
                });
            });
            output.textures_delta.clear();
            button.center()
        };
        let button_pos = frame(vec![]);
        frame(vec![Event::PointerMoved(page_pos)]);
        // This is why forwarding only !consumed drops legitimate page clicks.
        assert!(ctx.egui_wants_pointer_input());
        assert!(!blocks_page_pointer(&ctx, page_pos));

        let press = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![Event::PointerMoved(button_pos)]);
        frame(vec![press(button_pos, true)]);
        frame(vec![Event::PointerMoved(page_pos)]);
        assert!(blocks_page_pointer(&ctx, page_pos), "chrome owns this drag");
        frame(vec![press(page_pos, false)]);
        frame(vec![]);
        assert!(!blocks_page_pointer(&ctx, page_pos));

        frame(vec![Event::PointerMoved(button_pos)]);
        frame(vec![press(button_pos, true)]);
        frame(vec![press(button_pos, false)]);
        frame(vec![Event::PointerMoved(page_pos)]);
        assert!(egui::Popup::is_any_open(&ctx));
        assert!(blocks_page_pointer(&ctx, page_pos), "dismissal must not click through");
    }

    /// Builds the toolbar the way `browser.rs` does (buttons on the left,
    /// then the right-hand buttons anchored to the right edge with the
    /// address bar taking the rest) and returns the rect of every widget.
    fn toolbar_rects(screen_w: f32) -> (Vec<egui::Rect>, egui::Rect) {
        let ctx = egui::Context::default();
        install_icon_font(&ctx);
        let c = Colors::for_dark(true);
        let mut text = String::from("https://example.com/");
        let mut buttons = Vec::new();
        let mut address = egui::Rect::NOTHING;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(screen_w, 400.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            egui::Panel::top("toolbar").exact_size(44.0).show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(8.0);
                    for code in [ICON_BACK, ICON_FORWARD, ICON_RELOAD, ICON_HOME] {
                        buttons.push(icon_button(ui, true, code, "x", 16.0).rect);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        for code in [ICON_KEY, ICON_GEAR, ICON_MOON] {
                            buttons.push(icon_button(ui, true, code, "x", 16.0).rect);
                        }
                        let width = (ui.available_width() - 4.0).max(60.0);
                        let (resp, _) = address_bar(ui, &mut text, width, &c, ICON_LOCK, Some(false));
                        address = resp.rect;
                    });
                });
            });
        });
        output.textures_delta.clear();
        (buttons, address)
    }

    #[test]
    fn toolbar_buttons_stay_inside_the_window_at_every_width() {
        for width in [1600.0, 1280.0, 900.0, 640.0, 520.0] {
            let (buttons, address) = toolbar_rects(width);
            assert_eq!(buttons.len(), 7);
            for (i, r) in buttons.iter().enumerate() {
                assert!(r.max.x <= width + 0.5, "button {i} ends at {} in a {width}px window", r.max.x);
                assert!(r.min.x >= 0.0, "button {i} starts left of the window at {width}px");
            }
            // The right-hand buttons are ordered theme | settings | key, key last.
            let (key, gear, theme) = (buttons[4], buttons[5], buttons[6]);
            assert!(key.min.x > gear.min.x && gear.min.x > theme.min.x || key.max.x >= width - 20.0,
                "key is the rightmost button at {width}px");
            assert!(key.max.x >= width - 20.0, "key hugs the right edge at {width}px: {}", key.max.x);
            // The address bar never overlaps the buttons beside it.
            if width >= 640.0 {
                assert!(address.max.x <= theme.min.x.min(gear.min.x).min(key.min.x) + 0.5, "address overlaps buttons at {width}px");
                assert!(address.width() > 150.0, "address bar too small at {width}px");
            }
        }
    }

    /// The width the bookmarks bar's content really takes in a window of
    /// `screen_w` with `count` bookmarks.
    fn bookmarks_bar_used(screen_w: f32, count: usize) -> f32 {
        let ctx = egui::Context::default();
        install_icon_font(&ctx);
        let c = Colors::for_dark(true);
        let items: Vec<(String, String)> = (0..count)
            .map(|i| (format!("A fairly long bookmark title number {i}"), format!("https://site{i}.test/")))
            .collect();
        let mut used = 0.0;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(screen_w, 400.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            egui::Panel::top("bookmarks").exact_size(34.0).show(ui, |ui| {
                let _ = bookmarks_bar(ui, &c, &items);
                used = ui.min_rect().max.x;
            });
        });
        output.textures_delta.clear();
        used
    }

    #[test]
    fn bookmarks_bar_fits_the_window_with_few_or_many_bookmarks() {
        for width in [1600.0, 1280.0, 800.0, 500.0] {
            for count in [0, 3, 40] {
                let used = bookmarks_bar_used(width, count);
                assert!(used <= width + 0.5, "{count} bookmarks overflow a {width}px window: ends at {used}");
            }
        }
    }
}
