# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Replaced the system WebView (wry/tao) with Browsecraft's own Rust rendering engine: HTML parser, CSS parser and cascade, layout (block, inline, floats, flexbox, tables, positioning), and a software rasterizer, shown through winit and softbuffer.
- The toolbar is now drawn by the engine instead of an HTML toolbar webview.
- Searches use DuckDuckGo's HTML version, which works without JavaScript.
- The start page is the built-in `about:home`.

### Added

- Networking with ureq and rustls; `file://` directory listings, `data:` URLs and `view-source:`.
- Images (PNG, JPEG, GIF, WebP) that load in the background.
- Form fields and GET/POST form submission.
- Zoom (`Ctrl+Plus`/`Ctrl+Minus`/`Ctrl+0`), view source (`Ctrl+U`), keyboard scrolling, link previews on hover, clipboard support in the address bar.
- `--screenshot <out.png> <url> [width]` renders a page to PNG without a window.
- Unit tests for the HTML, CSS, style, network and text-editing modules.

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
