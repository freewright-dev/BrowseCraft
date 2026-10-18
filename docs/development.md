# Development

## Requirements

- Rust 1.95 or newer with the MSVC toolchain (the crate uses the 2024 edition,
  and the egui 0.36 dependency needs Rust 1.95)
- Visual Studio Build Tools with the "Desktop development with C++" workload
  (for the MSVC linker)
- The WebView2 runtime at run time (already part of Windows 11 and current
  Windows 10)
- System fonts: Segoe UI or Arial on Windows, Arial on macOS, DejaVu or Liberation fonts on Linux

BrowseCraft is developed and tested on Windows; CI runs only on `windows-latest`.
`about:`, `view-source:` and `browsecraft:` pages render in BrowseCraft's own engine;
everything else renders in an embedded WebView2 pane via the `wry` crate.

## Build and run

```sh
cargo run                       # debug build
cargo run -- example.com        # open a URL
cargo build --release           # optimized binary in target/release/
```

Release builds on Windows use the GUI subsystem, so no console window opens
(debug builds keep one, so `eprintln!` output is visible).
Layout is much faster in release builds; use them to judge performance.
The release profile optimises for size (`opt-level = "z"`) with LTO, one
codegen unit, `panic = "abort"` and stripped symbols, so the first release
build takes a few minutes.

### build.bat

On Windows, `build.bat` wraps the same steps:

```bat
build.bat              :: optimised release build -> dist\browsecraft.exe
build.bat debug        :: faster build with a console window for eprintln!
build.bat test         :: run the unit tests instead of building
build.bat clean        :: remove target\ and dist\
build.bat run          :: start the browser after a successful build
```

It checks that `rustc` is 1.95 or newer (and runs `rustup update stable` if
not), builds, then copies the executable to `dist\browsecraft.exe` and the
`filters\` folder to `dist\filters\`. Set `NOPAUSE=1` to skip the closing
`pause`.

## Ad-block filter lists

A small starter list is built into `src/adblock.rs`. On top of that, every
`*.txt` file in a `filters/` folder in the working directory or next to the
executable is merged in at startup (EasyList/uBlock Origin syntax; regex,
`$redirect`, `$csp`, `$replace`, scriptlet and procedural rules are dropped).
To add a list, drop it into `filters/` (the repo ships `filters/basic.txt`)
and restart; `build.bat` copies the folder into `dist\`.

## Flags

Settings from `browsecraft://flags` are saved as `key=value` lines in `flags.txt` in
the working directory (ignored by git).

## Headless screenshots

```sh
cargo run --release -- --screenshot out.png https://news.ycombinator.com/ 1200
```

This runs without a window (see `src/headless.rs`); the width is optional and
defaults to 1200. It always uses BrowseCraft's own engine, never WebView2. It loads
the page and all its images, then renders the full page (up to
6000 px tall) at the given width to a PNG. It runs the same style, layout and
paint code as the window, so it's the quickest way to check a rendering change.

## Tests

```sh
cargo test
```

CI (`.github/workflows/ci.yml`) runs `cargo build --locked` and
`cargo test --locked` on `windows-latest` for every pull request and every
push to `main`.

Unit tests live next to the code: HTML parsing (`html.rs`), sibling metadata
and cache invalidation (`dom.rs`), selectors, values
and media queries (`css.rs`), the cascade (`style.rs`), data URLs and charsets
(`net.rs`), text editing (`textfield.rs`), address-bar rules (`address.rs`),
ad blocking (`adblock.rs`), flags and the `flags.txt` store (`flags.rs`),
glyph coverage (`font.rs`), the history store (`history.rs`), and WebView2
routing, zoom steps, shortcuts and form submission (`browser.rs`). The
`flags.rs` tests share a global store and file, so they serialise on a mutex.

### Parser and selector regressions

Run the focused engine suites with:

```sh
cargo test --locked css::tests
cargo test --locked html::tests
cargo test --locked dom::tests
cargo test --locked style::tests
```

Coverage for the fixes in [PR #58](https://github.com/pierruno-labs/BrowseCraft/pull/58)
and [PR #59](https://github.com/pierruno-labs/BrowseCraft/pull/59) includes:

- Nested `:is()`/`:not()` matching: bounded operation counts, distinct cache
  keys and budget exhaustion that cannot be inverted into a match by `:not()`.
- Long selector chains: a 50,000-part input is rejected in a subprocess with
  a 15-second deadline and a 2 MiB worker stack. Accepted flat/nested chains
  and directly constructed selectors are checked too.
- Cascade allocation: a rule with 10,000 matching selectors and 10,000
  declarations contributes 10,000 author cascade entries rather than 100 million.
  Tests also cover specificity, source order, `!important` and aggregate limits.
- At-rule parsing: 25,000, 50,000 and 100,000 sibling blocks/statements with
  counted prelude visits bounded by input size, plus quoted and escaped delimiters.
- Attribute merging: 25,000 repeated root/body tags preserve distinct
  attributes, first values and insertion order within a five-second guard.
- Sibling matching: nine selector passes over 40,000 elements share one
  two-pass metadata build. Tests include text nodes, distinct tag names,
  parent boundaries, appends, tag changes and document clones.

Prefer deterministic operation or allocation counts for scaling regressions;
the broad time guards catch gross slowdowns rather than serve as benchmarks.
Use `Document::append` for structural additions and `Document::element_mut`
for tag changes so cached sibling metadata is invalidated. The limits and
cache design are described in [architecture](architecture.md#parsing-and-sibling-metadata).

## Common changes

- **Default styles:** `src/ua.css`.
- **A new CSS property:** add it to `Style` and `apply()` in `style.rs` (and to `expand_shorthand()` if it is a shorthand), then use it in `layout.rs`.
- **Home page:** `src/home.html`. **Search engine:** `SEARCH_URL` in `src/address.rs`.
- **Fonts:** the file lists in `src/font.rs`.
- **Toolbar / sidebar:** engine chrome in `draw_toolbar()`, `draw_sidebar()`, `button_rect()`, `click_toolbar()` and `click_sidebar()` in `src/browser.rs`; egui chrome in `build_egui_chrome()` in `src/browser.rs` and the widgets in `src/egui_chrome.rs`. Change both.
- **Which pages use WebView2:** `request_in_webview()` in `src/browser.rs`.
- **Ad blocking:** `src/adblock.rs` (`load_default()`, built-in `START_LIST`); extra lists in `filters/`.
- **Tabs:** `TabState`, `park_active_tab`/`activate_pantry`/`switch_tab`/`new_tab`/`close_tab` in `src/browser.rs`.
- **Flags / settings page:** `src/flags.rs` (`browsecraft://flags` HTML in `page_html()`); routing in `src/loader.rs`. **History page:** `src/history.rs`.
- **Icons:** Phosphor codepoints in `src/icons.rs` (font embedded via the `egui-phosphor` crate), loaded as `Family::Icon` in `src/font.rs`.
- **Shortcuts:** `Browser::key()` in `src/browser.rs`.

See [architecture.md](architecture.md) for how the stages fit together.
