<p align="center"><img src="assets/logo.png" alt="" width="160"></p>

<h1 align="center">Prowl</h1>

<p align="center">Part of <a href="https://github.com/pierruno-labs">Pierruno Labs</a></p>

A small web browser written in Rust with **its own rendering engine**. Prowl
parses HTML and CSS, runs the cascade, lays out the page and rasterizes it in
software:

```
URL ─► net ─► html ─► DOM ─► css + style (cascade) ─► layout ─► display list ─► paint ─► window
```

The engine renders Prowl's own pages (`about:`, `view-source:` and `prowl://`
pages such as `prowl://flags` and `prowl://history`). Regular web pages open in
an embedded WebView2 pane beside the tab sidebar, with a built-in ad blocker.
The toolbar and tabs are drawn with egui on the GPU when one is available, and
by the engine otherwise.

## Features

- HTML parser with implied tags, auto-closing `<p>`/`<li>`/table parts, entities and quirks-mode detection
- CSS parser: selectors with combinators, attributes and pseudo-classes (`:not`, `:is`, `:nth-child`, …), `@media`, `@import`, `@supports`, custom properties and `var()`, `calc()` basics
- Cascade with specificity, `!important`, inline styles, inheritance and legacy HTML attributes (`bgcolor`, `align`, `cellpadding`, …)
- Layout: block flow with margin collapsing, inline text with line breaking, inline-blocks, floats, flexbox (row/column, wrap, grow/shrink, justify/align, gap), tables (auto column widths, colspan), absolute/fixed/relative positioning, lists, `overflow: hidden`
- Painting: backgrounds, borders, rounded corners, anti-aliased text with system fonts and glyph fallback, PNG/JPEG/GIF/WebP images
- Forms: text inputs, checkboxes, radios, selects and buttons; GET and POST submission
- Browser UI drawn by the same engine: back, forward, reload/stop, home, an address bar with selection and clipboard, link previews, zoom, HiDPI, light and dark toolbar
- `http(s)`, `file://` (with directory listings), `data:`, `view-source:` and a built-in start page
- Tabs in a vertical sidebar, browsing history, light/dark/auto theme and a `prowl://flags` settings page
- Ad blocking with uBlock-style filter lists loaded from `filters/`

## Usage

```sh
cargo run --release                  # opens the start page
cargo run --release -- example.com   # opens a URL or search
cargo run --release -- --screenshot page.png news.ycombinator.com 1200
```

| Shortcut                  | Action                |
| ------------------------- | --------------------- |
| `Ctrl+L` / `F6`           | Focus address bar     |
| `F5` / `Ctrl+R`           | Reload                |
| `Alt+Left` / `Alt+Right`  | Back / forward        |
| `Alt+Home`                | Home                  |
| `Ctrl+Plus` / `Ctrl+Minus` / `Ctrl+0` | Zoom      |
| `Ctrl+U`                  | View source           |
| `Space`, `Page Up/Down`, arrows | Scroll          |
| `Ctrl+T` / `Ctrl+W`       | New tab / close tab   |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `Ctrl+H`                  | History               |
| `Shift+K`                 | Shortcut cheatsheet   |

On Windows, `build.bat` builds a release binary into `dist\` along with the filter lists.

## Documentation

- [Usage](docs/usage.md): address bar rules, toolbar, shortcuts and forms
- [Architecture](docs/architecture.md): how the engine's stages fit together
- [Development](docs/development.md): building, tests, headless screenshots and common changes
- [Changelog](CHANGELOG.md)
