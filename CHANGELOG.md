# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Browser window with a toolbar (back, forward, reload/stop, home), a loading indicator and the page title in the window title.
- Browsecraft's own Rust rendering engine: HTML parser, CSS parser and cascade, layout (block, inline, floats, flexbox, tables, positioning) and a software rasterizer, shown through winit and softbuffer. It renders the internal pages (`about:`, `view-source:`, `browsecraft://`) and is used for everything on platforms without an embedded web view.
- Regular websites render in an embedded WebView2 pane (wry) on Windows (WKWebView on macOS), so they run JavaScript; the engine-rendered internal pages do not.
- Address bar that accepts full URLs, bare domains, `localhost` and IPv4 addresses and otherwise searches Startpage; clipboard support in the address bar.
- Optional URL or search as a command-line argument.
- Real tabs with a vertical tab sidebar: `Ctrl+T` opens a new tab on the built-in `about:home` start page, `Ctrl+W` closes one, `Ctrl+Tab`/`Ctrl+Shift+Tab` cycle them.
- Pop-ups and `target="_blank"` links open in the current tab.
- Ad and tracker blocking ported from uBlock Origin's core, using EasyList/uBO filter syntax: network rules (`||host^`, `@@` exceptions, `$` options such as `third-party`, `domain=`, `important` and request types) and `##` element-hiding rules. Blocked pages show a "This page was blocked" notice, blocked pop-ups show a notice in the status strip, and WebView2 pages get an injected script that cancels matching requests and hides matching elements. Ships with a starter list (`filters/basic.txt`); any extra `filters/*.txt` lists next to the executable or in the working directory are merged at startup. Regex, `$redirect`, `$csp`, scriptlet and procedural filters are not supported (those rules are skipped).
- Every page that finishes loading is recorded in the session history shown at `browsecraft://history` (`Ctrl+H`, last 500 entries).
- User-settable options ("flags") persisted to `flags.txt` and edited on the `browsecraft://flags` settings page (toolbar gear button): tab sidebar position (left, right or hidden), theme and chrome backend.
- Light/dark/auto theme with a toolbar sun/moon toggle; `auto` follows the system setting and the theme is applied to WebView2 pages too.
- Optional GPU chrome (`chrome = auto | egui | engine`): with an available adapter, `egui` on `wgpu` draws the toolbar and tab sidebar and shows engine pages as a texture; otherwise the engine/softbuffer chrome is kept. The choice is made at startup. The egui chrome has a rounded address pill and a hoverable tab list.
- Toolbar and tab-sidebar icons from Phosphor (the `egui-phosphor` crate), embedded in the binary as an icon font (`src/icons.rs`), so they render regardless of the working directory.
- Keyboard shortcuts: `Ctrl+L`/`F6` and `Ctrl+S` focus the address bar, `F5`/`Ctrl+R` reload, `Alt+Left`/`Alt+Right` and `Ctrl+Left`/`Ctrl+Right` go back/forward, `Alt+Home` goes home, `Ctrl+Q` quits, and `Shift+K` shows a keyboard cheatsheet overlay.
- `Ctrl+Shift+S` saves a screenshot of the visible page to the Pictures folder (on Windows by capturing the screen, so WebView2 pages are included), with a status strip under the page confirming it.
- Engine features: networking with ureq and rustls, `file://` directory listings, `data:` URLs, `view-source:` (`Ctrl+U`), images (PNG, JPEG, GIF, WebP) that load in the background, form fields and GET/POST form submission, link previews on hover and keyboard scrolling.
- Zoom (`Ctrl+Plus`/`Ctrl+Minus`/`Ctrl+0`), for both engine and WebView2 pages.
- `--screenshot <out.png> <url> [width]` renders a page with the engine to PNG without a window.
- `build.bat`: Windows build script (`release`, `debug`, `test`, `clean`, `run`) that checks for Rust 1.95 or newer, builds, and collects `browsecraft.exe` plus the `filters/` folder in `dist\`.
- Size-optimized release profile (`opt-level = "z"`, LTO, stripped).
- Unit tests for the HTML, CSS, style, font, network, text-editing, address, browser, history, flags and ad-blocking modules.
- Documentation in `docs/`.

### Changed

- The internal pages moved from `craft://` to `browsecraft://` (`browsecraft://flags`, `browsecraft://history`), and the repository moved to `pierruno-labs/prowl`.
- Replaced the original wry/tao-only browser with a hybrid design: Browsecraft's own engine and winit window draw the chrome and internal pages, while websites render in an embedded WebView2 pane. The HTML toolbar webview is gone; the toolbar is drawn by the engine or by egui.
- Searches use Startpage instead of DuckDuckGo.
- The home page (`Alt+Home`, and the page shown at startup without an argument) is Startpage; new tabs open the built-in `about:home` start page.

### Fixed

- Browsecraft's keyboard shortcuts (new/close tab, address bar, reload, back/forward, zoom, history and the rest) now work while a web page has the keyboard, instead of only after clicking the toolbar or sidebar.
