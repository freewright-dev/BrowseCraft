# Browsecraft

A lightweight web browser written in Rust. It uses the operating system's own
web engine (WebView2 on Windows, WebKit on macOS, WebKitGTK on Linux) through
[wry](https://github.com/tauri-apps/wry) and [tao](https://github.com/tauri-apps/tao),
so the release binary is under 500 KB.

## Features

- Address bar that accepts URLs, bare domains (`docs.rs`), `localhost:3000`, or search terms (DuckDuckGo)
- Back, forward, reload/stop, home
- Loading indicator and page title in the window title
- Popups and `target="_blank"` links open in the same view
- Light and dark toolbar that follows the system theme

## Usage

```sh
cargo run --release                  # opens the home page
cargo run --release -- example.com   # opens a URL or search
```

| Shortcut              | Action             |
| --------------------- | ------------------ |
| `Ctrl+L`              | Focus address bar  |
| `F5` / `Ctrl+R`       | Reload             |
| `Alt+Left` / `Alt+Right` | Back / forward  |
| `Alt+Home`            | Home               |
| `Esc` (in address bar) | Revert the address |

## Documentation

- [Usage](docs/usage.md): address bar rules, toolbar and shortcuts
- [Architecture](docs/architecture.md): how the window, webviews and IPC fit together
- [Development](docs/development.md): requirements, building, tests and common changes
- [Changelog](CHANGELOG.md)

On Linux, child webviews require X11.
