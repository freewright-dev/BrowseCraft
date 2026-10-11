# Architecture

Browsecraft does not render pages itself. It hosts the operating system's web engine through [wry](https://github.com/tauri-apps/wry), and gets its window and event loop from [tao](https://github.com/tauri-apps/tao).

| Platform | Engine                |
| -------- | --------------------- |
| Windows  | WebView2 (Edge)       |
| macOS    | WKWebView             |
| Linux    | WebKitGTK (X11 only)  |

## Source layout

| File               | Purpose                                                  |
| ------------------ | -------------------------------------------------------- |
| `src/main.rs`      | Window, both webviews, event loop, commands, layout      |
| `src/toolbar.html` | Toolbar UI, compiled into the binary with `include_str!` |
| `src/address.rs`   | Turns address bar input into a URL                       |

## Window layout

The window holds two child webviews:

```
┌──────────────────────────────────────┐
│ toolbar webview (44 logical px)      │
├──────────────────────────────────────┤
│                                      │
│ content webview (the web page)       │
│                                      │
└──────────────────────────────────────┘
```

`layout()` in `main.rs` computes both rectangles in physical pixels. It runs at startup and again on `Resized` and `ScaleFactorChanged`.

## Events

Webview callbacks run outside the main loop, so they send a `UserEvent` through an `EventLoopProxy`. The loop handles each event on the main thread:

| Event             | Sent by                                | Handling                                 |
| ----------------- | -------------------------------------- | ---------------------------------------- |
| `Command(String)` | Toolbar IPC, page shortcuts, pop-ups   | `run_command()`                          |
| `UrlChanged`      | Page load handler                      | Updates the address bar                  |
| `Loading(bool)`   | Page load handler (started / finished) | Toggles the loading bar and stop button  |
| `TitleChanged`    | Document title handler                 | Sets the window title                    |

## IPC commands

Both webviews send plain strings with `window.ipc.postMessage()`:

| Message       | Effect                              |
| ------------- | ----------------------------------- |
| `back`        | `history.back()` in the page        |
| `forward`     | `history.forward()` in the page     |
| `reload`      | Reloads the page                    |
| `stop`        | `window.stop()` in the page         |
| `home`        | Loads the home page                 |
| `focus-url`   | Focuses and selects the address bar |
| `nav:<input>` | Resolves `<input>` and loads it     |

The page webview gets an initialization script (`CONTENT_INIT_SCRIPT`) that turns browser shortcuts into these commands. A web page can send the same messages, but they only do things a page can already do itself, such as navigating.

## Rust → toolbar

Rust calls functions on `window.browsecraft` in the toolbar with `evaluate_script`:

- `setUrl(url)` shows the current address, unless the user is typing in the field.
- `setLoading(bool)` shows or hides the loading state.
- `focusUrl()` focuses the address bar.

URLs are escaped with `serde_json` before they go into the script.

## Binary size

The release profile in `Cargo.toml` trades build time for size: `opt-level = "z"`, LTO, one codegen unit, `panic = "abort"` and stripped symbols. The engine is part of the OS, so the binary is under 500 KB.
