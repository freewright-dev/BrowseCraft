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
        let state = egui_winit::State::new(
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
    BACK as ICON_BACK, CLOSE as ICON_STOP, CLOSE as ICON_XMARK, FORWARD as ICON_FORWARD,
    GEAR as ICON_GEAR, GLOBE as ICON_GLOBE, HOME as ICON_HOME, LOCK as ICON_LOCK,
    MOON as ICON_MOON, PLUS as ICON_PLUS, RELOAD as ICON_RELOAD, SUN as ICON_SUN,
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
        .min_size(egui::vec2(30.0, 30.0))
        .corner_radius(egui::CornerRadius::same(7));
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

/// The rounded address field with a leading scheme icon. Returns the text
/// edit's response so the caller can react to focus/submit.
pub fn address_bar(
    ui: &mut egui::Ui,
    text: &mut String,
    width: f32,
    c: &Colors,
    leading: &str,
) -> egui::Response {
    egui::Frame::NONE
        .fill(c.field_bg)
        .stroke(egui::Stroke::new(1.0, c.line))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.set_width((width - 18.0).max(40.0));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(icon(leading, 13.0).color(c.muted));
                ui.add_sized(
                    [ui.available_width(), 22.0],
                    egui::TextEdit::singleline(text)
                        .id(address_id())
                        .frame(egui::Frame::NONE)
                        .text_color(c.text)
                        .hint_text(egui::RichText::new("Search or enter address").color(c.muted)),
                )
            })
            .inner
        })
        .inner
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
    TabRow {
        clicked: row.clicked() || label_resp.clicked(),
        close,
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
                bar_bg: rgb(0x2b2b2b),
                sidebar_bg: rgb(0x202020),
                line: rgb(0x3c3c3c),
                field_bg: rgb(0x1e1e1e),
                text: rgb(0xe8e8e8),
                muted: rgb(0x8a8a8a),
                hover: rgb(0x3d3d3d),
                accent: rgb(0x5b9cff),
            }
        } else {
            Self {
                bar_bg: rgb(0xf3f3f3),
                sidebar_bg: rgb(0xececec),
                line: rgb(0xd6d6d6),
                field_bg: rgb(0xffffff),
                text: rgb(0x1f1f1f),
                muted: rgb(0x8a8a8a),
                hover: rgb(0xe1e1e1),
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

    let radius = egui::CornerRadius::same(7);
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
