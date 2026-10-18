<p align="center"><img src="assets/logo.png" alt="" width="160"></p>

<h1 align="center">BrowseCraft</h1>

<p align="center">Part of <a href="https://github.com/pierruno-labs">Pierruno Labs</a></p>

A small web browser written in Rust, with tabs, a built-in ad blocker and
**its own rendering engine**.

BrowseCraft uses two renderers side by side:

- **BrowseCraft's engine** draws the browser's own pages: the new-tab page
  (`about:home`), `view-source:` and the `browsecraft://` pages (`browsecraft://flags`,
  `browsecraft://history`). It parses HTML and CSS, runs the cascade, lays out the
  page and rasterizes it in software.
- **WebView2** (through `wry`) shows regular websites, so JavaScript-heavy
  sites work as they would in Edge.

```
URL ─► net ─► html ─► DOM ─► css + style (cascade) ─► layout ─► display list ─► paint ─► window
```

The toolbar and tab sidebar are drawn with egui on the GPU when one is
available, and by the engine otherwise.

## Features

**Browser**

- Tabs in a vertical sidebar (left, right or hidden)
- Back, forward, reload/stop, home (Startpage) and an address bar that tells URLs from searches (searches also go to Startpage)
- Browsing history at `browsecraft://history`
- Light, dark or automatic theme, toggled from the toolbar
- Settings page at `browsecraft://flags`, saved to `flags.txt`
- Zoom, view source, link previews on hover and HiDPI support
- Screenshots of the visible page (`Ctrl+Shift+S`), plus a headless `--screenshot` mode
- A shortcut cheatsheet on `Shift+K`

**Ad blocking**

- A network and element-hiding filter engine ported from uBlock Origin's core, reading EasyList/uBO filter syntax
- Blocks navigations and popups; checks engine redirect destinations and native Windows WebView2 subresources before requests reach the network
- Hides matching elements on both renderers; macOS JavaScript network hooks remain best effort
- Ships with a starter list; drop more lists into `filters/`

**Rendering engine**

- HTML parser with implied tags, auto-closing `<p>`/`<li>`/table parts, entities and quirks-mode detection
- CSS parser: selectors with combinators, attributes and pseudo-classes (`:not`, `:is`, `:nth-child`, …), `@media`, `@import`, `@supports`, custom properties and `var()`, `calc()` basics
- Cascade with specificity, `!important`, inline styles, inheritance and legacy HTML attributes (`bgcolor`, `align`, `cellpadding`, …)
- Layout: block flow with margin collapsing, inline text with line breaking, inline-blocks, floats, flexbox, tables, absolute/fixed/relative positioning, lists, `overflow: hidden`
- Painting: backgrounds, borders, rounded corners, anti-aliased text with system fonts and glyph fallback, PNG/JPEG/GIF/WebP images
- Forms with GET and POST submission; `http(s)`, `file://` (with directory listings) and `data:` URLs

The custom engine also handles headless screenshots and fallback browsing when
an embedded web view is unavailable. It bounds selector complexity, matching
work and cascade size, and caches sibling positions to avoid repeated scans.
Selectors exceeding 64 compound parts (including nested pseudo-selector lists)
are rejected; matching stops after 100,000 work steps per element, and the
cascade keeps at most 65,536 declarations per element. Pages exceeding these
limits may render with reduced styling. See [architecture](docs/architecture.md#style)
for the limits and [the changelog](CHANGELOG.md#unreleased) for recent fixes.

## Requirements

BrowseCraft is developed and tested on Windows. Building it needs:

- Rust 1.95 or newer (`rustup update stable`)
- The Visual Studio Build Tools with the "Desktop development with C++" workload
- The WebView2 runtime at run time (included with Windows 11 and current Windows 10)

## Getting started

On Windows, `build.bat` checks your toolchain, builds a release binary and
copies it into `dist\` together with the filter lists:

```bat
build.bat            :: release build into dist\browsecraft.exe
build.bat run        :: build, then start BrowseCraft
build.bat debug      :: faster build that keeps a console window for logs
build.bat test       :: run the unit tests
build.bat clean      :: remove target\ and dist\
```

Or use Cargo directly:

```sh
cargo run --release                  # opens the start page
cargo run --release -- example.com   # opens a URL or search
cargo run --release -- --screenshot page.png news.ycombinator.com 1200
cargo test
```

## Shortcuts

| Shortcut                              | Action                    |
| ------------------------------------- | ------------------------- |
| `Ctrl+L` / `F6` / `Ctrl+S`            | Focus address bar         |
| `Ctrl+T` / `Ctrl+W`                   | New tab / close tab       |
| `Ctrl+Tab` / `Ctrl+Shift+Tab`         | Next / previous tab       |
| `Alt+Left` / `Alt+Right`              | Back / forward            |
| `F5` / `Ctrl+R`                       | Reload                    |
| `Alt+Home`                            | Home page (Startpage)     |
| `Ctrl+H`                              | History                   |
| `Ctrl+Plus` / `Ctrl+Minus` / `Ctrl+0` | Zoom in / out / reset     |
| `Ctrl+U`                              | View source               |
| `Ctrl+Shift+S`                        | Save a screenshot to Pictures |
| `Shift+K`                             | Shortcut cheatsheet       |
| `Ctrl+Q`                              | Quit                      |

The full list is in [docs/usage.md](docs/usage.md).

## Settings

Open `browsecraft://flags` to change:

- **Tab sidebar:** left, right or hidden
- **Theme:** auto (follows the OS), light or dark
- **Chrome backend:** `auto`, `egui` (GPU) or `engine` (software). This one takes effect after a restart.

Settings are saved atomically as `key=value` lines in the per-user configuration
folder: `%APPDATA%\BrowseCraft\flags.txt` on Windows,
`~/Library/Application Support/BrowseCraft/flags.txt` on macOS, or
`$XDG_CONFIG_HOME/BrowseCraft/flags.txt` on Linux (`~/.config` by default).
Old working-directory settings are no longer read. To retain them, close the
browser and copy your existing `flags.txt` to the new location once.

## Ad-block filter lists

BrowseCraft builds its blocker from a built-in starter list plus every `filters/*.txt`
file it finds next to the executable or in the working directory. To block
more, add a list such as [EasyList](https://easylist.to/) to `filters/`.

Some uBlock Origin features aren't supported: regex filters, `$redirect`,
`$csp`, `$replace`, scriptlets and procedural filters. Rules that use them are
skipped rather than applied loosely.

## Documentation

- [Usage](docs/usage.md): address bar rules, toolbar, tabs, settings, shortcuts and forms
- [Architecture](docs/architecture.md): how the engine's stages fit together
- [egui chrome](docs/egui.md): the GPU toolbar and sidebar, and how they sit next to WebView2
- [Development](docs/development.md): building, tests, headless screenshots and common changes
- [Changelog](CHANGELOG.md)
- [Contributing](CONTRIBUTING.md)
