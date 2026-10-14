# Usage

## Starting the browser

```sh
prowl                 # opens the start page
prowl example.com     # opens a URL
prowl "rust browser"  # runs a search
prowl --screenshot out.png example.com 1200   # renders to a PNG
```

The command-line argument is read the same way as text typed into the address bar.

## Address bar

Press `Enter` to go. Prowl decides what the text means using these rules, in order:

| Input                                                  | Result                  |
| ------------------------------------------------------ | ----------------------- |
| Has a scheme (`https://…`, `ftp://…`)                  | Opened unchanged        |
| Starts with `about:`, `data:`, `file:`, `view-source:` | Opened unchanged        |
| `localhost` or an IPv4 address, with an optional port  | `http://` is added      |
| A domain such as `docs.rs/wry` or `example.com:8443`   | `https://` is added     |
| Anything else, including text with spaces              | Startpage search        |

A domain needs a dot and a top-level part of at least two letters. So `hello` and `1.5` are searched, not opened. Searches go to Startpage, the default search engine.

The address bar supports selection with the mouse or `Shift` + arrows, `Ctrl` + arrows to move by word, and `Ctrl+A`, `Ctrl+C`, `Ctrl+X` and `Ctrl+V`. Press `Esc` while editing to put back the current address.

Built-in pages: `about:home` (the start page), `about:blank`, and `prowl://flags` (settings). `file://` URLs that point at a folder show a directory listing.

## Toolbar

| Button  | Action                                  |
| ------- | --------------------------------------- |
| ◀       | Back                                    |
| ▶       | Forward                                 |
| ↻ / ✕   | Reload, or stop while a page is loading |
| ⌂       | Start page                              |
| ☀ / ☾   | Toggle light / dark theme               |

A bar under the address field shows that a page is loading. Hovering a link shows its address in the bottom-left corner.

## Tabs

The vertical sidebar on the left lists the open tabs; the active one is
highlighted. Click a tab to switch to it, click its ✕ to close it, and click the
`+` at the top to open a new one. The sidebar position (left, right or hidden)
is set on `prowl://flags`.

## Settings (`prowl://flags`)

`prowl://flags` is an engine-rendered settings page. It controls the tab
sidebar position, the theme (auto, light or dark) and the chrome backend
(`auto`, `egui` or `engine`). Choices are stored in `flags.txt` and applied
immediately; `auto` follows the OS for the theme, and for chrome it uses the
GPU (egui) path when one is available, falling back to the engine otherwise.
The chrome choice is made at startup, so restart to apply a change to it.

## Keyboard and mouse

| Shortcut                              | Action                    |
| ------------------------------------- | ------------------------- |
| `Ctrl+L` / `F6`                       | Focus address bar         |
| `Ctrl+T`                              | New tab                   |
| `Ctrl+W`                              | Close tab                 |
| `Ctrl+Tab` / `Ctrl+Shift+Tab`         | Next / previous tab       |
| `F5` / `Ctrl+R`                       | Reload                    |
| `Alt+Left` / `Alt+Right`, mouse back/forward buttons | Back / forward |
| `Backspace` (page focused)            | Back                      |
| `Alt+Home`                            | Start page                |
| `Ctrl+Plus` / `Ctrl+Minus` / `Ctrl+0`, `Ctrl`+wheel | Zoom in / out / reset |
| `Ctrl+U`                              | View page source          |
| `Shift+K` (page focused)              | Show / hide the shortcut cheatsheet (`Esc` closes it) |
| `Ctrl+Left` / `Ctrl+Right`            | Back / forward (when not typing in a field) |
| `Ctrl+H`                              | Open the history page (`prowl://history`) |
| `Ctrl+S`                              | Select the address bar to type |
| `Ctrl+Shift+S`                        | Save a PNG of the visible page to your Pictures folder (UTC-stamped `prowl-YYYYMMDD-HHMMSS.png`); the result shows in a strip under the page |
| `Space` / `Shift+Space`, `Page Up/Down`, arrows, `Home`/`End` | Scroll |
| `Shift`+wheel                         | Scroll sideways           |
| `Ctrl+Q`                              | Quit                      |

## Forms

Click a text field to type in it. `Enter` submits the form and `Tab` moves to the next field. Checkboxes and radio buttons toggle on click. A `<select>` moves to its next option on each click, since there is no dropdown. Forms submit with GET or POST.

## Links and new windows

Links with `target="_blank"` open in the current tab. `javascript:` and `mailto:` links are ignored. Prowl has a single window; new pages open in tabs.
