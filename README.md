<p align="center"><img src="assets/logos/browsecraft.png" alt="BrowseCraft logo" width="160"></p>

<h1 align="center">BrowseCraft</h1>

<p align="center">Part of <a href="https://github.com/freewright-dev">Freewright</a></p>

<p align="center"><a href="https://discord.gg/SAy3t4BvJj"><img src="https://img.shields.io/badge/Discord-Join%20us-5865F2?logo=discord&logoColor=white" alt="Join the Freewright Discord"></a></p>

An experimental open-source desktop browser written in Rust, with tabs,
a built-in ad blocker and a custom HTML/CSS renderer.

## Current implementation

BrowseCraft currently uses a **hybrid rendering architecture**, not Servo:

- **BrowseCraft's engine** draws the browser's own pages: the new-tab page
  (`about:home`), `view-source:` and the `browsecraft://` pages (`browsecraft://flags`,
  `browsecraft://history`, `browsecraft://bookmarks`, `browsecraft://passwords`).
  It parses HTML and CSS, runs the cascade, lays out the
  page and rasterizes it in software. It also handles all headless screenshots
  and fallback browsing. It does not execute JavaScript.
- **Native web views**, embedded through `wry`, show regular websites using
  WebView2 on Windows and WKWebView on macOS. On Linux, or when the native web
  view cannot be initialized, pages use the custom engine with its limited
  standards support, or, in a Linux build with the experimental `servo`
  feature, Servo (see below).

Windows browsing therefore still depends on Edge's Chromium-based WebView2;
the current application is not yet independent of native browser engines.

The custom engine's pipeline is:

```
URL ─► net ─► html ─► DOM ─► css + style (cascade) ─► layout ─► display list ─► paint ─► window
```

The toolbar and tab sidebar are drawn with egui on the GPU when one is
available, and by the engine otherwise.

## Engine direction: Servo

[The architecture decision](https://github.com/freewright-dev/BrowseCraft/issues/65#issuecomment-6251509599)
selects **Servo** as the future independent website engine. The plan is to
retain Rust and egui/winit for the application and reuse Servo's page engine
rather than build a second general-purpose DOM, layout and JavaScript stack.

**First step, experimental and opt-in:** on Linux, a build with
`--features servo` renders web pages with Servo 0.7 instead of the custom
engine (see [docs/development.md](docs/development.md#servo-on-linux-experimental)).
It is off by default, not part of release builds, and not yet secure for
arbitrary browsing: Servo runs in the browser process, and the password
manager's page script and element hiding are not wired up yet. An earlier
[50-site compatibility run](docs/servo-compat.md) reported 38 rendered
properly, 7 partly and 5 not; the same document records the current
compatibility policies and native regression test.

Existing native web views stay in place until the integration passes explicit
compatibility, packaging and security gates. Windows renderer containment is
an unresolved release blocker; this decision is not a production-readiness
claim. Track the work in [the engine roadmap](https://github.com/freewright-dev/BrowseCraft/issues/75).

## Features

**Browser**

- Tabs in a vertical sidebar (left, right or hidden), with a right-click menu to duplicate a tab, close it or close the others
- Back, forward, reload/stop, home (Startpage) and an address bar that tells URLs from searches (searches also go to Startpage)
- Browsing history at `browsecraft://history`
- Bookmarks: a star in the address bar (`Ctrl+D`), a bookmarks bar under it, and a manager at `browsecraft://bookmarks`
- Password manager backed by a KeePass `.kdbx` vault (password or key file), with sign-up/login detection, autofill and a save prompt
- Real page fullscreen: a video's fullscreen button makes the whole window fullscreen, not just the viewport (`F11` for the window)
- Dark theme by default; light or automatic on request, toggled from the toolbar
- A modern settings page at `browsecraft://flags` with toggle switches, saved in the per-user configuration folder (see [Settings](#settings))
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
cargo run --release                  # opens Startpage
cargo run --release -- about:home     # opens the built-in new-tab page
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
| `Ctrl+D`                              | Bookmark / unbookmark the page |
| `Ctrl+Shift+B` / `Ctrl+Shift+O`       | Bookmarks bar / bookmarks page |
| `F11`                                 | Fullscreen window         |
| `Ctrl+Plus` / `Ctrl+Minus` / `Ctrl+0` | Zoom in / out / reset     |
| `Ctrl+U`                              | View source               |
| `Ctrl+Shift+S`                        | Save a screenshot to Pictures |
| `Shift+K`                             | Shortcut cheatsheet       |
| `Ctrl+Q`                              | Quit                      |

The full list is in [docs/usage.md](docs/usage.md).

## Settings

Open `browsecraft://flags` to change:

- **Theme:** system (follows the OS), light or dark (the default)
- **Tab sidebar:** left, right or hidden
- **Bookmarks bar:** shown or hidden (`Ctrl+Shift+B`)
- **Interface renderer:** auto, GPU (egui) or software (engine). This one takes effect after a restart.

Settings are saved atomically as `key=value` lines in the per-user configuration
folder: `%APPDATA%\BrowseCraft\flags.txt` on Windows,
`~/Library/Application Support/BrowseCraft/flags.txt` on macOS, or
`$XDG_CONFIG_HOME/BrowseCraft/flags.txt` on Linux (`~/.config` by default).
Old working-directory settings are no longer read. To retain them, close the
browser and copy your existing `flags.txt` to the new location once.

Bookmarks (`bookmarks.txt`) and the password manager's settings
(`passwords.cfg`: which vault, how it unlocks, never-save sites) live in the
same folder, which is also the suggested place for a new `.kdbx` vault.

## Ad-block filter lists

BrowseCraft builds its blocker from a built-in starter list plus every `filters/*.txt`
file it finds next to the executable or in the working directory. To block
more, add a list such as [EasyList](https://easylist.to/) to `filters/`.
At most 200,000 rules are loaded in all; lines past that are skipped.

Some uBlock Origin features aren't supported: regex filters, `$redirect`,
`$csp`, `$replace`, scriptlets and procedural filters. Rules that use them are
skipped rather than applied loosely.

## License

BrowseCraft is licensed under **GPL-3.0-or-later**. See [LICENSE](LICENSE)
and [third-party notices](THIRD_PARTY_NOTICES.md), including the uBlock Origin
code adapted by the ad blocker.

## Documentation

- [Usage](docs/usage.md): address bar rules, toolbar, tabs, bookmarks, fullscreen, the password manager, settings, shortcuts and forms
- [Architecture](docs/architecture.md): how the engine's stages fit together
- [egui chrome](docs/egui.md): the GPU toolbar and sidebar, and how they sit next to WebView2
- [Development](docs/development.md): building, tests, headless screenshots and common changes
- [Dependency maintenance](docs/dependencies.md): advisory tracking and license/source policy
- [Changelog](CHANGELOG.md)
- [Contributing](CONTRIBUTING.md)
- [Discord](https://discord.gg/SAy3t4BvJj): questions, ideas and chat with the Freewright community
