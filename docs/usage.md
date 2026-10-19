# Usage

## Starting the browser

```sh
browsecraft                 # opens Startpage (the home page)
browsecraft example.com     # opens a URL
browsecraft "rust browser"  # runs a search
browsecraft --screenshot out.png example.com 1200   # renders to a PNG
```

The command-line argument is read the same way as text typed into the address bar. `--screenshot <out.png> <url> [width]` renders the page with BrowseCraft's own engine (never WebView2) and exits; the width defaults to 1200 pixels.

## Address bar

Press `Enter` to go. BrowseCraft decides what the text means using these rules, in order:

| Input                                                  | Result                  |
| ------------------------------------------------------ | ----------------------- |
| Has a scheme (`https://…`, `ftp://…`, `browsecraft://…`)     | Opened unchanged        |
| Starts with `about:`, `data:`, `file:`, `view-source:` | Opened unchanged        |
| `localhost` or an IPv4 address, with an optional port  | `http://` is added      |
| A domain such as `docs.rs/wry` or `example.com:8443`   | `https://` is added     |
| Anything else, including text with spaces              | Startpage search        |

A domain needs a dot and a top-level part of at least two letters. So `hello` and `1.5` are searched, not opened. Searches go to Startpage, the default search engine.

The address bar supports selection with the mouse or `Shift` + arrows, `Ctrl` + arrows to move by word, and `Ctrl+A`, `Ctrl+C`, `Ctrl+X` and `Ctrl+V`. Press `Esc` while editing to put back the current address.

## Where pages render

On Windows, web pages (`http:`, `https:`, `file:` and `data:` addresses) are shown in an embedded WebView2 pane (WKWebView on macOS). BrowseCraft's own engine renders the built-in pages (`about:`, `browsecraft://` and `view-source:`), the result of a POST form submitted from an engine page, and every page when WebView2 is unavailable or on other platforms.

This matters for input. BrowseCraft's shortcuts below also work while a WebView2 page has the keyboard (handled on Windows through the WebView2 controller's `AcceleratorKeyPressed` event). Forms, link hover, `Ctrl`+wheel and the scrolling keys described here apply to engine pages; WebView2 pages handle these themselves.

## Built-in pages

| Page                  | What it shows                                                     |
| --------------------- | ----------------------------------------------------------------- |
| `about:home`          | The start page that new tabs open                                 |
| `about:blank`         | An empty page                                                     |
| `browsecraft://flags`       | Settings (see below)                                              |
| `browsecraft://history`     | Pages visited this session, newest first                          |
| `browsecraft://bookmarks`   | Your bookmarks, with a Remove button each                         |
| `browsecraft://passwords`   | The password vault (see Password manager below)                   |
| `view-source:<url>`   | The raw source of an `http:`, `https:` or `file:` page            |

History is kept in memory only (up to 500 entries) and is lost when BrowseCraft quits. Built-in pages are not recorded. When the engine renders a `file://` URL that points at a folder, it shows a directory listing.

## Toolbar

| Button  | Action                                  |
| ------- | --------------------------------------- |
| ◀       | Back                                    |
| ▶       | Forward                                 |
| ↻ / ✕   | Reload, or stop while a page is loading |
| ⌂       | Home (Startpage)                        |
| ☀ / ☾   | Toggle light / dark theme               |
| ⚙       | Open settings (`browsecraft://flags`)         |
| 🔑      | Password manager menu (top right)       |

The star inside the address bar bookmarks the page you are on (click it again to remove the bookmark); it is also `Ctrl+D`.

A bar along the bottom of the toolbar shows that a page is loading. On engine pages, hovering a link shows its address in the bottom-left corner.

## Tabs

The vertical sidebar on the left lists the open tabs; the active one is
highlighted. Click a tab to switch to it, click its ✕ to close it, and click the
`+` (New tab) button at the top to open a new one. New tabs open `about:home`
with the address bar focused. With the GPU interface, right-clicking a tab
offers *Duplicate tab*, *Close tab* and *Close other tabs*. With the engine
chrome, middle-clicking a tab also closes it. Closing the last tab quits BrowseCraft. The sidebar position (left,
right or hidden) is set on `browsecraft://flags`.

## Bookmarks

Click the star in the address bar, or press `Ctrl+D`, to bookmark the current page; do it again to remove it. Bookmarks show on the bookmarks bar under the address bar (turn it off in Settings or with `Ctrl+Shift+B`). Click one to open it, middle-click to open it in a new tab, right-click for *Open in new tab* and *Remove bookmark*. Bookmarks that do not fit go in the `»` menu. **All bookmarks** (`Ctrl+Shift+O`) opens `browsecraft://bookmarks`. They are saved in `bookmarks.txt` in the [per-user settings folder](../README.md#settings) (one `url<TAB>title` per line). Only `http(s)` pages can be bookmarked. The bar is drawn by the GPU interface; with the software interface (`chrome=engine`) bookmarks still work from `Ctrl+D` and the manager page.

The key menu, the bookmarks bar and the save-login bar belong to the GPU interface (the default). When a menu is open over a web page, the page is shown as a still picture until the menu closes (the live page is a native window that cannot sit under a menu).

## Fullscreen

When a web page goes fullscreen, for example a video's fullscreen button on YouTube, the whole window goes fullscreen and the toolbar, bookmarks bar and tabs are hidden, so the page fills the screen. For the first few seconds a strip at the bottom, drawn by BrowseCraft rather than the page, names the site and how to leave. Press `Esc`, `F11` or the page's own button to leave; `F11` always works, even if the page handles `Esc`. The window returns to how it was. Outside page fullscreen, `F11` makes the window fullscreen on its own.

## Password manager

Click the key icon at the top right. The first time, choose how to protect your vault:

- **A password**, or
- **A key file**: a small file with random bytes that BrowseCraft creates for you. The vault cannot be opened without it. Keep it apart from the vault (a USB stick, say). If you lose it, the vault is gone for good.

The vault is a standard KeePass 4 (`.kdbx`) file, so KeePassXC and other KeePass apps open it too. You can also open an existing vault. BrowseCraft locks the vault again after 15 minutes without use, and keeps which vault to use (never a password) in `passwords.cfg` in the [per-user settings folder](../README.md#settings), which is also where it suggests creating a new vault.

On a page with a login form the key menu lists the logins saved for that site: click one to fill the page. A login counts for its own host and for hosts above or below it in the same registered domain (a login for `example.com` is offered on `login.example.com`), but not for sibling subdomains or across a public suffix such as `github.io`. On a sign-up or change-password form, *Suggest a strong password* fills a generated one. When you submit a login, a bar offers to **Save** it (or **Update** a changed password), or to never ask for that site. `browsecraft://passwords` lists, shows, copies (cleared from the clipboard after 30 seconds), adds and deletes logins.

## Settings (`browsecraft://flags`)

`browsecraft://flags` is an engine-rendered settings page with switches and segmented controls. It sets the theme (system, light or dark; dark by default), the tab sidebar position, whether the bookmarks bar shows, and the interface renderer (auto, GPU or software). Changes are stored in the [per-user settings folder](../README.md#settings) and apply immediately after a successful save; save errors are shown on the page or in the status notice. System follows the OS for the theme; Auto uses the GPU path when one is available. The renderer choice is made at startup, so restart to apply a change to it.

## Ad blocking

BrowseCraft blocks ads and trackers with a small built-in filter list plus every
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
| `Ctrl+H`                              | Open the history page (`browsecraft://history`) |
| `Ctrl+D`                              | Bookmark the page, or remove its bookmark |
| `Ctrl+Shift+B`                        | Show / hide the bookmarks bar |
| `Ctrl+Shift+O`                        | Open the bookmarks page (`browsecraft://bookmarks`) |
| `F11`                                 | Make the window fullscreen, or leave fullscreen |
| `Esc` (page in fullscreen)            | Leave page fullscreen     |
| `Ctrl+Shift+S`                        | Save a PNG of the visible page to your Pictures folder (UTC-stamped `browsecraft-YYYYMMDD-HHMMSS.png`); the result shows in a strip under the page |
| `Shift+K` (page focused)              | Show / hide the shortcut cheatsheet (`Esc`, `Enter`, `Space` or `K` closes it) |
| `Space` / `Shift+Space`, `Page Up/Down`, arrows, `Home`/`End` | Scroll |
| `Shift`+wheel                         | Scroll sideways           |
| `Ctrl+Q`                              | Quit                      |

Screenshots of WebView2 pages are grabbed from the screen, so this works on Windows only and captures anything covering the window.

## Forms

These apply to forms on engine pages. Click a text field to type in it. `Enter` submits the form (or adds a line in a `<textarea>`), `Tab` / `Shift+Tab` move between fields and `Esc` leaves the field. Checkboxes and radio buttons toggle on click. A `<select>` moves to its next option on each click, since there is no dropdown. Reset buttons clear the form. Forms submit with GET or POST.

## Links and new windows

Links with `target="_blank"` and pop-ups from WebView2 pages open in the current tab, but only when you opened them (a click, not a script running on its own), only to web pages, and at most one a second. A tab keeps its last 100 Back and Forward entries. On engine pages, `javascript:`, `mailto:` and `tel:` links are ignored. BrowseCraft has a single window; new pages open in tabs.

BrowseCraft does not hand links to other apps: a page (or a frame in it) that opens a `mailto:` link or an app's own scheme gets a "Blocked a link that opens another app" notice instead of the system prompt. A page cannot close its tab with `window.close()`.
