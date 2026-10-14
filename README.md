<p align="center"><img src="assets/logo.png" alt="" width="160"></p>

<h1 align="center">Prowl</h1>

<p align="center">Part of <a href="https://github.com/pierruno-labs">Pierruno Labs</a></p>

A small web browser written in Rust with **its own rendering engine**. It
doesn't use a WebView or any system web engine: Prowl parses HTML and CSS,
runs the cascade, lays out the page and rasterizes it in software.

```
URL ─► net ─► html ─► DOM ─► css + style (cascade) ─► layout ─► display list ─► paint ─► window
```

There is no JavaScript. Pages that render on the server, such as Hacker News,
Wikipedia, Lobsters, blogs, docs and DuckDuckGo's HTML search, work well. Pages
built entirely in JavaScript show whatever their HTML contains.

## Features

- HTML parser with implied tags, auto-closing `<p>`/`<li>`/table parts, entities and quirks-mode detection
- CSS parser: selectors with combinators, attributes and pseudo-classes (`:not`, `:is`, `:nth-child`, …), `@media`, `@import`, `@supports`, custom properties and `var()`, `calc()` basics
- Cascade with specificity, `!important`, inline styles, inheritance and legacy HTML attributes (`bgcolor`, `align`, `cellpadding`, …)
- Layout: block flow with margin collapsing, inline text with line breaking, inline-blocks, floats, flexbox (row/column, wrap, grow/shrink, justify/align, gap), tables (auto column widths, colspan), absolute/fixed/relative positioning, lists, `overflow: hidden`
- Painting: backgrounds, borders, rounded corners, anti-aliased text with system fonts and glyph fallback, PNG/JPEG/GIF/WebP images
- Forms: text inputs, checkboxes, radios, selects and buttons; GET and POST submission
- Browser UI drawn by the same engine: back, forward, reload/stop, home, an address bar with selection and clipboard, link previews, zoom, HiDPI, light and dark toolbar
- `http(s)`, `file://` (with directory listings), `data:`, `view-source:` and a built-in start page

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

## Documentation

- [Usage](docs/usage.md): address bar rules, toolbar, shortcuts and forms
- [Architecture](docs/architecture.md): how the engine's stages fit together
- [Development](docs/development.md): building, tests, headless screenshots and common changes
- [Changelog](CHANGELOG.md)
