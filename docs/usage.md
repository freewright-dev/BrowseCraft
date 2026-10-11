# Usage

## Starting the browser

```sh
browsecraft                 # opens the home page (DuckDuckGo)
browsecraft example.com     # opens a URL
browsecraft "rust webview"  # runs a search
```

The command-line argument is read the same way as text typed into the address bar.

## Address bar

Press `Enter` to go. Browsecraft decides what the text means using these rules, in order:

| Input                                                  | Result                  |
| ------------------------------------------------------ | ----------------------- |
| Has a scheme (`https://…`, `ftp://…`)                  | Opened unchanged        |
| Starts with `about:`, `data:`, `file:`, `view-source:` | Opened unchanged        |
| `localhost` or an IPv4 address, with an optional port  | `http://` is added      |
| A domain such as `docs.rs/wry` or `example.com:8443`   | `https://` is added     |
| Anything else, including text with spaces              | DuckDuckGo search       |

A domain needs a dot and a top-level part of at least two letters. So `hello` and `1.5` are searched, not opened.

Press `Esc` while editing to put back the current address.

## Toolbar

| Button | Action                                  |
| ------ | --------------------------------------- |
| ←      | Back                                    |
| →      | Forward                                 |
| ↻ / ✕  | Reload, or stop while a page is loading |
| ⌂      | Home page                               |

A thin bar under the address field shows that a page is loading.

## Keyboard shortcuts

| Shortcut                 | Action            | Works from      |
| ------------------------ | ----------------- | --------------- |
| `Ctrl+L`                 | Focus address bar | Page            |
| `F5` / `Ctrl+R`          | Reload            | Page, toolbar   |
| `Alt+Left` / `Alt+Right` | Back / forward    | Page, toolbar   |
| `Alt+Home`               | Home page         | Page            |
| `Esc`                    | Revert address    | Address bar     |

## Pop-ups and new windows

Links with `target="_blank"` and `window.open()` calls open in the current view. Browsecraft has a single window and no tabs.
