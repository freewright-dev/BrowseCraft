# Development

## Requirements

- Rust 1.85 or newer (the crate uses the 2024 edition)
- Windows: the WebView2 runtime, which ships with Windows 10 and 11
- macOS: nothing extra
- Linux: WebKitGTK and GTK development packages, for example on Debian/Ubuntu:

  ```sh
  sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev
  ```

## Build and run

```sh
cargo run                       # debug build
cargo run -- example.com        # open a URL
cargo build --release           # optimized binary in target/release/
```

Release builds on Windows use the GUI subsystem, so no console window opens. Debug builds keep the console, so errors from `run_command()` are visible.

## Tests

```sh
cargo test
```

The tests in `src/address.rs` cover address bar input. Add a case there when you change the rules in [usage.md](usage.md#address-bar).

## Common changes

- **Home page or search engine:** `HOME_URL` in `src/main.rs`, `SEARCH_URL` in `src/address.rs`.
- **New toolbar button:** add the button and a `send("...")` call in `src/toolbar.html`, then a match arm in `run_command()`.
- **New shortcut:** add it to `CONTENT_INIT_SCRIPT` so it works when the page has focus, and to the `keydown` handler in `toolbar.html` so it works in the toolbar.
- **Toolbar height:** `TOOLBAR_HEIGHT` in `src/main.rs`, in logical pixels.

See [architecture.md](architecture.md) for how the pieces fit together.
