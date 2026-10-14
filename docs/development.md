# Development

## Requirements

- Rust 1.85 or newer (the crate uses the 2024 edition)
- System fonts: Segoe UI or Arial on Windows, Arial on macOS, DejaVu or Liberation fonts on Linux
- Linux: the usual X11/Wayland development libraries that winit needs

There is no web engine dependency.

## Build and run

```sh
cargo run                       # debug build
cargo run -- example.com        # open a URL
cargo build --release           # optimized binary in target/release/
```

Release builds on Windows use the GUI subsystem, so no console window opens.
Layout is much faster in release builds; use them to judge performance.

## Headless screenshots

```sh
cargo run --release -- --screenshot out.png https://news.ycombinator.com/ 1200
```

This loads the page and all its images, then renders the full page (up to
6000 px tall) at the given width to a PNG. It runs the same style, layout and
paint code as the window, so it's the quickest way to check a rendering change.

## Tests

```sh
cargo test
```

Unit tests live next to the code: HTML parsing (`html.rs`), selectors, values
and media queries (`css.rs`), the cascade (`style.rs`), data URLs and charsets
(`net.rs`), text editing (`textfield.rs`), address-bar rules (`address.rs`),
ad blocking (`adblock.rs`), flags and the `flags.txt` store (`flags.rs`) and
glyph coverage (`font.rs`). The `flags.rs` tests share a global store and file,
so they serialise on a mutex.

## Common changes

- **Default styles:** `src/ua.css`.
- **A new CSS property:** add it to `Style` and `apply()` in `style.rs` (and to `expand_shorthand()` if it is a shorthand), then use it in `layout.rs`.
- **Home page:** `src/home.html`. **Search engine:** `SEARCH_URL` in `src/address.rs`.
- **Fonts:** the file lists in `src/font.rs`.
- **Toolbar / sidebar:** `draw_toolbar()`, `draw_sidebar()`, `button_rect()` and `click()` in `src/browser.rs`.
- **Tabs:** `TabState`, `park_active_tab`/`activate_pantry`/`switch_tab`/`new_tab`/`close_tab` in `src/browser.rs`.
- **Flags / settings page:** `src/flags.rs` (`prowl://flags` HTML in `page_html()`); routing in `src/loader.rs`.
- **Icons:** Phosphor codepoints in `src/icons.rs` (font embedded via the `egui-phosphor` crate), loaded as `Family::Icon` in `src/font.rs`.
- **Shortcuts:** `Browser::key()` in `src/browser.rs`.

See [architecture.md](architecture.md) for how the stages fit together.
