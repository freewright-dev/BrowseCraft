# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Renamed the project from Browsecraft to Prowl. The binary is now `prowl`, the internal pages moved from `craft://` to `prowl://` (`prowl://flags`, `prowl://history`), and the repository moved to `pierruno-labs/prowl`.

- Replaced the system WebView (wry/tao) with Prowl's own Rust rendering engine: HTML parser, CSS parser and cascade, layout (block, inline, floats, flexbox, tables, positioning), and a software rasterizer, shown through winit and softbuffer.
- The toolbar is now drawn by the engine instead of an HTML toolbar webview.
- Searches use Startpage, the browser's default search engine.
- The default home page is Startpage; the built-in `about:home` start page is still available.
- Toolbar and tab-sidebar icons are Phosphor (the `egui-phosphor` crate), embedded in the binary and loaded as an icon font (`src/icons.rs`), so they render regardless of the working directory.

### Added

- `Shift+K` keyboard cheatsheet overlay, `Ctrl+H` history page, `Ctrl+Left`/`Ctrl+Right` back/forward, `Ctrl+S` to select the address bar, and `Ctrl+Shift+S` to save a screenshot to Pictures (with a status strip under the page confirming it).
- Every page that finishes loading is recorded in the history shown at `prowl://history`.
- Networking with ureq and rustls; `file://` directory listings, `data:` URLs and `view-source:`.
- Images (PNG, JPEG, GIF, WebP) that load in the background.
- Form fields and GET/POST form submission.
- Zoom (`Ctrl+Plus`/`Ctrl+Minus`/`Ctrl+0`), view source (`Ctrl+U`), keyboard scrolling, link previews on hover, clipboard support in the address bar.
- `--screenshot <out.png> <url> [width]` renders a page to PNG without a window.
- Unit tests for the HTML, CSS, style, network and text-editing modules.
- Real tabs with a vertical tab sidebar, openable with `Ctrl+T`, closable with `Ctrl+W`, cycled with `Ctrl+Tab`/`Ctrl+Shift+Tab`.
- `flags.rs`: user-settable options persisted to `flags.txt`, and the `prowl://flags` settings page (sidebar position, theme and chrome backend).
- Light/dark/auto theme with a toolbar sun/moon toggle; the current theme is applied to WebView2 pages too.
- Optional GPU chrome (`chrome = auto | egui | engine`): with an available adapter, `egui` on `wgpu` draws the toolbar and tab sidebar and shows the engine page as a texture; otherwise the engine/softbuffer chrome is kept. The choice is made at startup. The egui chrome uses embedded Phosphor icons, a rounded address pill and a hoverable tab list.

### Removed

- JavaScript support. The new engine does not run scripts.

### Previously added

- Browser window that uses the system web engine through wry and tao.
- Toolbar with back, forward, reload/stop and home buttons, plus a loading indicator.
- Address bar that accepts full URLs, bare domains, `localhost` and IPv4 addresses, and falls back to a DuckDuckGo search.
- Optional URL or search as a command-line argument.
- Keyboard shortcuts: `Ctrl+L`, `F5`/`Ctrl+R`, `Alt+Left`/`Alt+Right` and `Alt+Home`.
- Page title shown in the window title.
- Pop-ups and `target="_blank"` links open in the current view.
- Light and dark toolbar theme that follows the system setting.
- Size-optimized release profile (binary under 500 KB).
- Documentation in `docs/`.
