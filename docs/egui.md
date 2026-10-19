# Chrome: egui + wgpu (GPU) with softbuffer fallback (CPU)

Status: **implemented (MVP).** `src/egui_chrome.rs` draws the toolbar and tab
sidebar with egui on wgpu; the engine/softbuffer chrome in `browser.rs` is the
fallback. Engine-page input under egui is forwarded but has not been run on a
GPU yet (see "Known limitations").

## Goal

Draw the browser chrome (toolbar, address bar, tab sidebar) with egui instead
of the software `Canvas`. When a GPU is available, egui renders through
**wgpu**; when there is no usable adapter, keep today's engine/softbuffer
chrome. The engine keeps producing the page raster; on the GPU path it is
uploaded as an egui texture and shown in the central panel.

## Why two paths

egui only ships GPU backends (`egui-wgpu`, `egui-glow`). Writing a full egui
mesh rasterizer for the software path is a large amount of work for little
benefit, so the CPU path stays as the current engine-drawn chrome. Selection is
automatic:

- **GPU present** -> egui chrome, wgpu surface, engine page as a texture.
- **No GPU** -> the existing engine-drawn chrome and softbuffer presentation.

A flag (`chrome = auto | egui | engine`, on `browsecraft://flags`) forces a path for
testing. `auto` and `egui` attempt the GPU path and fall back to the engine when
no adapter/device can be created; `engine` always uses the software path.

## Crate choices

| Crate        | Use                                                        |
| ------------ | ---------------------------------------------------------- |
| `egui`       | Widgets, layout, tessellation, theme                        |
| `egui-winit` | `State`: feeds winit events to egui, scale factor, clipboard |
| `egui-wgpu`  | wgpu surface + painter; re-exports the matching `wgpu`      |

`wgpu` is **not** added directly: use `egui_wgpu::wgpu` so there is only one
wgpu version. The egui release used is the one whose `egui-winit` targets winit
0.30 (egui 0.36.x). `egui-wgpu` needs its `winit` feature for
`egui_wgpu::winit::Painter`.

## GPU detection

There is no separate probe module: `EguiChrome::new(window)` is the probe. It
creates the egui context, `egui_winit::State` and `egui_wgpu::winit::Painter`,
then `Painter::set_window(...)` and checks `render_state()`; any failure (no
adapter, no surface, no device) returns `None`. `resumed()` skips the probe
when `chrome = engine`, then `egui_chrome::resolve(gpu_available)` returns
`Choice::Egui` or `Choice::Engine`. On `Engine`, `browser.rs` creates the
softbuffer `Context`/`Surface` and keeps the engine chrome. Detection happens
once, in `resumed()`, and the choice is fixed for the run.

## Render loop changes (`browser.rs`)

Engine work is unchanged: load/style/layout/paint into the `Canvas`. Only how
the frame is presented differs; `Gfx::surface` is `Option`, and `None` means
the egui path owns the window.

**GPU path** (`render` -> `render_egui` -> `build_egui_chrome`)
1. Render only the page into `frame` (no engine toolbar/sidebar), sized from
   `page_rect()`.
2. Upload `frame` as an egui texture (`ColorImage` + `load_texture`/`set`).
3. `egui_winit::State::take_egui_input`, build the UI with
   `egui::Panel::top("toolbar")`, `egui::Panel::top("bookmarks")` (when the
   bookmarks bar is on), `egui::Panel::top("save_login")` (while a login can
   be saved), `egui::Panel::left("tabs")` (or `right`,
   none when `tabs-position = hidden`), `egui::Panel::bottom("notice")` while
   a notice is shown, and `egui::CentralPanel`, and run it via
   `Context::run_ui` (egui 0.36 panels take the root `Ui`, not the `Context`).
   Button clicks are collected as `Action`s and applied after the panels are
   built.
4. Record the central panel's rect (physical px) so the WebView2 pane is
   positioned exactly over it (`egui_page` -> `webview_rect`).
5. `EguiChrome::frame` tessellates and presents with
   `Painter::paint_and_update_textures`.

**CPU path** is the current code: draw toolbar + sidebar with `Canvas`,
`softbuffer` present.

## WebView2 interaction

The webview is a child HWND with explicit bounds, independent of the
presentation path. On the GPU path its bounds come from the egui central panel
(`egui_page`), refreshed each frame by `sync_webview`; on the CPU path they
come from the engine toolbar/sidebar geometry (`page_rect`).

The pane is a native window, so it always paints over egui. While a menu that
can hang over the page is open (the key menu, a bookmark's menu, the `»`
overflow menu), `snapshot_page` captures the page as a texture, the live pane
is hidden and the snapshot is drawn in the central panel; the pane comes back
when the menu closes. In page fullscreen the toolbar, bars and sidebar
collapse to zero size and the pane fills the window.

## Chrome look

`egui_chrome.rs` registers the Phosphor face (embedded in the binary by the
`egui-phosphor` crate, codepoints in `src/icons.rs`) as the `phosphor` egui
family and draws the toolbar as an icon row (back/forward/reload-stop/home, a
rounded address pill with a leading lock/globe and a trailing bookmark star,
theme and flags buttons, the password key menu at the right edge, and a
sliding loading bar) plus a "New tab" button and a hoverable tab list (globe,
truncated title, close button on the active or hovered row, accent bar on the
active row, and a right-click menu to duplicate, close or close other tabs).
Under the toolbar sit the bookmarks bar (overflow in a `»` menu, right-click
and middle-click actions) and the "save this login?" bar. Visuals are themed from the same palette as the engine chrome
(`Colors::for_dark`), and `apply_style` sets the widget spacing/rounding once
per frame.

Because the icon font is compiled into the binary, icons render in every
working directory; there is no loose asset to miss (the previous Font Awesome
file could be absent, which made epaint panic on the unbound `fa-solid`
family).

## Known limitations (MVP)

- **Engine-page input under egui is untested on real hardware.** The page is
  drawn as a texture, but `window_event` hit-tests clicks, the wheel and typing
  against the page rectangle and forwards them to the engine DOM, so
  `about:`/`browsecraft:` pages should be clickable. This path compiles and is shared
  with the engine chrome (which was run end to end), but has not been run on a
  GPU. Mouse clicks and hover use the engine geometry (`page_rect`), not the
  egui central panel rect; `top_h` adds the bookmarks and save-login bars so the
  two match. Keys reach the browser when egui did not consume
  them, or when they are shortcuts (Ctrl/Alt, F5, F6).
- Middle-click to close a tab only works in the engine chrome.
- The chrome is not re-created when the `chrome` flag changes at runtime; the
  choice is made once in `resumed()` (restart to apply).
- egui has its own fonts, so the chrome looks different from the engine-drawn
  chrome.

## Flags wiring

`browsecraft://flags` has an "Interface renderer" control (Auto, GPU, Software,
stored as `chrome = auto | egui | engine`) persisted like the others. The gear
button in either chrome opens `browsecraft://flags`; you can also edit
`flags.txt` in the per-user configuration folder by hand while the browser is
closed.
