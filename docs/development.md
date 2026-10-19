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

### Servo on Linux (experimental)

On Linux, BrowseCraft has no system web view, so web pages render in its own
engine (no JavaScript). The `servo` feature renders them with
[Servo](https://servo.org) 0.6 LTS instead
([issue #65](https://github.com/freewright-dev/BrowseCraft/issues/65)). It is
off by default and ignored on other platforms.

Servo needs clang 19 and its native dependencies. On Ubuntu 24.04:

```sh
sudo apt install build-essential pkg-config clang-19 libclang-19-dev llvm-19-dev   cmake gperf m4 python3 libssl-dev libdbus-1-dev libudev-dev libunwind-dev liblzma-dev   libegl1-mesa-dev libgles2-mesa-dev libgl1-mesa-dri libvulkan1 libfreetype6-dev   libharfbuzz-dev libglib2.0-dev libx11-dev libxcb-render0-dev libxcb-shape0-dev   libxcb-xfixes0-dev libxkbcommon-dev libxkbcommon-x11-0 libxmu-dev xorg-dev   libgtk-3-dev libwebkit2gtk-4.1-dev libxdo-dev
CC=clang-19 CXX=clang++-19 LIBCLANG_PATH=/usr/lib/llvm-19/lib cargo run --features servo
```

`src/servo_pane.rs` holds the integration: Servo renders offscreen
(`SoftwareRenderingContext`), the browser draws each frame into the page area
and forwards mouse, wheel and keyboard input, and Servo's notifications become
the same `UserMessage`s the WebView2 pane sends. Navigation policy
(`browser::web_navigation`) and the ad filter apply as they do on Windows;
pop-ups are refused. The password manager's page script is not injected yet.
The `Servo (experimental)` workflow builds this configuration; it is not a
required check.

For evaluation, `BROWSECRAFT_SERVO_TRACE=1` prints load progress, titles and
crashes, and `BROWSECRAFT_SERVO_FRAME=<file.png>` keeps Servo's newest frame.
`tools/servo-compat/run.sh` uses both to open every site in
`tools/servo-compat/sites.txt`; the first run's results are in
[servo-compat.md](servo-compat.md).

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
not), builds, then copies the executable to `dist\browsecraft.exe`. Set
`NOPAUSE=1` to skip the closing `pause`.

## Flags

Settings from `browsecraft://flags` are saved in the per-user configuration
folder listed in [the README](../README.md#settings). A same-directory,
exclusively created temporary file is synced and atomically replaces
`flags.txt`; destination symlinks/hard links do not redirect writes to their
targets. Tests use isolated temporary directories rather than user settings.
Save failures leave in-memory flags unchanged and are shown by the settings
page or the toolbar notice. Working-directory settings are not migrated
automatically; users can copy a trusted legacy file while the browser is closed.
Bookmarks (`bookmarks.txt`) and the password manager's settings (`passwords.cfg`:
which vault, how it unlocks, never-save sites) live in the same folder and are
written the same way. Both helpers are in `src/storage.rs`; use
`storage::config_file` and `storage::write_private` for any new file BrowseCraft
keeps, never the working directory. In tests `storage::config_dir` is a
temporary directory per test thread.

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

The fetch-input regression also requires Node.js 18 or newer (`node` on PATH).
Native Windows network interception can be checked separately on a desktop
with WebView2 installed:

```sh
cargo test --locked webview_network::tests -- --ignored --nocapture
```

This loopback-only integration test checks static resources, fetch input
types (including a cross-frame Request) and redirected resources. It is
ignored in the ordinary unit suite because it creates a native WebView2 window.

```sh
cargo test
```

CI (`.github/workflows/ci.yml`) runs `cargo build --locked` and
`cargo test --locked` on `windows-latest` for every pull request and every
push to `main`, then `cargo clippy --locked --all-targets`. Clippy denies
`unwrap` and `expect` in production code (`[lints.clippy]` in `Cargo.toml`;
`clippy.toml` allows them in tests), because release builds abort on panic.
Handle the `None`/`Err` case instead; where an invariant really guarantees
the value, put `#[allow(clippy::expect_used, reason = "...")]` on the
function and say why. `.github/workflows/deny.yml` runs `cargo deny check` on
the same events and once a week.

Unit tests live next to the code: HTML parsing (`html.rs`), sibling metadata
and cache invalidation (`dom.rs`), selectors, values
and media queries (`css.rs`), the cascade (`style.rs`), data URLs and charsets
(`net.rs`), text editing (`textfield.rs`), address-bar rules (`address.rs`),
flags and the `flags.txt` store (`flags.rs`), atomic writes (`storage.rs`),
one-time tokens and the internal-page look (`internal.rs`), the bookmarks store
and page (`bookmarks.rs`), the KeePass vault (`vault.rs`) and the password
manager's forms, offers and page (`passwords.rs`),
glyph coverage (`font.rs`), the history store (`history.rs`), and WebView2
routing, zoom steps, shortcuts and form submission (`browser.rs`). The
`flags.rs` and `bookmarks.rs` tests share a global store, so they serialise on
a mutex.

### Parser and selector regressions

Run the focused engine suites with:

```sh
cargo test --locked css::tests
cargo test --locked html::tests
cargo test --locked dom::tests
cargo test --locked style::tests
```

Coverage for the fixes in [PR #58](https://github.com/freewright-dev/BrowseCraft/pull/58)
and [PR #59](https://github.com/freewright-dev/BrowseCraft/pull/59) includes:

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

For dependency upgrades, run `cargo audit` and `cargo deny check` as well as
the build/tests. CI also runs `cargo deny check` on every pull request and
weekly. See [dependency maintenance](dependencies.md) for the current
target-specific advisories and license/source policy.

- **Default styles:** `src/ua.css`.
- **A new CSS property:** add it to `Style` and `apply()` in `style.rs` (and to `expand_shorthand()` if it is a shorthand), then use it in `layout.rs`.
- **Home page:** `src/home.html`. **Search engine:** `SEARCH_URL` in `src/address.rs`.
- **Fonts:** the file lists in `src/font.rs`.
- **Toolbar / sidebar:** engine chrome in `draw_toolbar()`, `draw_sidebar()`, `button_rect()`, `click_toolbar()` and `click_sidebar()` in `src/browser.rs`; egui chrome in `build_egui_chrome()` in `src/browser.rs` and the widgets in `src/egui_chrome.rs`. Change both.
- **Which pages use WebView2:** `request_in_webview()` in `src/browser.rs`.
- **Tabs:** `TabState`, `park_active_tab`/`activate_pantry`/`switch_tab`/`new_tab`/`close_tab` in `src/browser.rs`.
- **Flags / settings page:** `src/flags.rs` (`browsecraft://flags` HTML in `page_html()`); routing in `src/loader.rs`. **History page:** `src/history.rs`. **Bookmarks:** `src/bookmarks.rs`. **Password manager:** `src/passwords.rs`, `src/passwords.js` and `src/vault.rs`.
- **Look of the internal pages:** the palette and shared CSS in `src/internal.rs` (also `switch_row()` and `segmented_row()` for settings controls); the new-tab page is `src/home.html`, filled in by `home_html()` in `src/loader.rs`.
- **Icons:** Phosphor codepoints in `src/icons.rs` (font embedded via the `egui-phosphor` crate), loaded as `Family::Icon` in `src/font.rs`.
- **Shortcuts:** `Browser::key()` in `src/browser.rs`.

See [architecture.md](architecture.md) for how the stages fit together.
