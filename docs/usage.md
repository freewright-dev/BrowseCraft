# Usage

## Starting the browser

```sh
prowl                 # opens Startpage (the home page)
prowl example.com     # opens a URL
prowl "rust browser"  # runs a search
prowl --screenshot out.png example.com 1200   # renders to a PNG
```

The command-line argument is read the same way as text typed into the address bar. `--screenshot <out.png> <url> [width]` renders the page with Prowl's own engine (never WebView2) and exits; the width defaults to 1200 pixels.

## Address bar

Press `Enter` to go. Prowl decides what the text means using these rules, in order:

| Input                                                  | Result                  |
| ------------------------------------------------------ | ----------------------- |
| Has a scheme (`https://…`, `ftp://…`, `prowl://…`)     | Opened unchanged        |
| Starts with `about:`, `data:`, `file:`, `view-source:` | Opened unchanged        |
| `localhost` or an IPv4 address, with an optional port  | `http://` is added      |
| A domain such as `docs.rs/wry` or `example.com:8443`   | `https://` is added     |
| Anything else, including text with spaces              | Startpage search        |

A domain needs a dot and a top-level part of at least two letters. So `hello` and `1.5` are searched, not opened. Searches go to Startpage, the default search engine.

The address bar supports selection with the mouse or `Shift` + arrows, `Ctrl` + arrows to move by word, and `Ctrl+A`, `Ctrl+C`, `Ctrl+X` and `Ctrl+V`. Press `Esc` while editing to put back the current address.

## Where pages render

On Windows, web pages (`http:`, `https:`, `file:` and `data:` addresses) are shown in an embedded WebView2 pane (WKWebView on macOS). Prowl's own engine renders the built-in pages (`about:`, `prowl://` and `view-source:`), the result of a POST form submitted from an engine page, and every page when WebView2 is unavailable or on other platforms.

This matters for input. While a WebView2 page has the keyboard, keys go to that page, so Prowl's shortcuts below do not fire until you click the toolbar, the address bar or the tab sidebar. Forms, link hover, `Ctrl`+wheel and the scrolling keys described here apply to engine pages; WebView2 pages handle these themselves.

## Built-in pages

| Page                  | What it shows                                                     |
| --------------------- | ----------------------------------------------------------------- |
| `about:home`          | The start page that new tabs open                                 |
| `about:blank`         | An empty page                                                     |
| `prowl://flags`       | Settings (see below)                                              |
| `prowl://history`     | Pages visited this session, newest first                          |
| `view-source:<url>`   | The raw source of an `http:`, `https:` or `file:` page            |

History is kept in memory only (up to 500 entries) and is lost when Prowl quits. Built-in pages are not recorded. When the engine renders a `file://` URL that points at a folder, it shows a directory listing.

## Toolbar

| Button  | Action                                  |
| ------- | --------------------------------------- |
| ◀       | Back                                    |
| ▶       | Forward                                 |
| ↻ / ✕   | Reload, or stop while a page is loading |
| ⌂       | Home (Startpage)                        |
| ☀ / ☾   | Toggle light / dark theme               |
| ⚙       | Open settings (`prowl://flags`)         |

A bar along the bottom of the toolbar shows that a page is loading. On engine pages, hovering a link shows its address in the bottom-left corner.

## Tabs

The vertical sidebar on the left lists the open tabs; the active one is
highlighted. Click a tab to switch to it, click its ✕ to close it, and click the
`+` (New tab) button at the top to open a new one. New tabs open `about:home`
with the address bar focused. With the engine chrome, middle-clicking a tab
also closes it. Closing the last tab quits Prowl. The sidebar position (left,
right or hidden) is set on `prowl://flags`.

## Settings (`prowl://flags`)

`prowl://flags` is an engine-rendered settings page. It controls the tab
sidebar position, the theme (auto, light or dark) and the chrome backend
(`auto`, `egui` or `engine`). Pick the options and press **Apply**; choices are
stored in `flags.txt` in the working directory and take effect immediately.
`auto` follows the OS for the theme, and for chrome it uses the GPU (egui) path
when one is available, falling back to the engine otherwise. The chrome choice
is made at startup, so restart to apply a change to it.

## Ad blocking

Prowl blocks ads and trackers with a small built-in filter list plus every
`.txt` file in a `filters` folder in the working directory or next to the
executable (EasyList / uBlock Origin syntax). A blocked page shows a "This page
was blocked" notice while the address bar keeps the original address, and a
blocked pop-up shows a short notice under the page.

## Keyboard and mouse

| Shortcut                              | Action                    |
| ------------------------------------- | ------------------------- |
| `Ctrl+L` / `F6` / `Ctrl+S`            | Focus address bar and select its text |
| `Ctrl+T`                              | New tab                   |
| `Ctrl+W`                              | Close tab                 |
| `Ctrl+Tab` / `Ctrl+Shift+Tab`         | Next / previous tab       |
| `F5` / `Ctrl+R`                       | Reload                    |
| `Esc` (page focused, while loading)   | Stop loading              |
| `Alt+Left` / `Alt+Right`, mouse back/forward buttons | Back / forward |
| `Ctrl+Left` / `Ctrl+Right`            | Back / forward (when not typing in a field) |
| `Backspace` / `Shift+Backspace` (page focused) | Back / forward   |
| `Alt+Home`                            | Home (Startpage)          |
| `Ctrl+Plus` / `Ctrl+Minus` / `Ctrl+0`, `Ctrl`+wheel | Zoom in / out / reset |
| `Ctrl+U`                              | View page source          |
| `Ctrl+H`                              | Open the history page (`prowl://history`) |
| `Ctrl+Shift+S`                        | Save a PNG of the visible page to your Pictures folder (UTC-stamped `prowl-YYYYMMDD-HHMMSS.png`); the result shows in a strip under the page |
| `Shift+K` (page focused)              | Show / hide the shortcut cheatsheet (`Esc`, `Enter`, `Space` or `K` closes it) |
| `Space` / `Shift+Space`, `Page Up/Down`, arrows, `Home`/`End` | Scroll |
| `Shift`+wheel                         | Scroll sideways           |
| `Ctrl+Q`                              | Quit                      |

Screenshots of WebView2 pages are grabbed from the screen, so this works on Windows only and captures anything covering the window.

## Forms

These apply to forms on engine pages. Click a text field to type in it. `Enter` submits the form (or adds a line in a `<textarea>`), `Tab` / `Shift+Tab` move between fields and `Esc` leaves the field. Checkboxes and radio buttons toggle on click. A `<select>` moves to its next option on each click, since there is no dropdown. Reset buttons clear the form. Forms submit with GET or POST.

## Links and new windows

Links with `target="_blank"` and pop-ups from WebView2 pages open in the current tab. On engine pages, `javascript:`, `mailto:` and `tel:` links are ignored. Prowl has a single window; new pages open in tabs.
