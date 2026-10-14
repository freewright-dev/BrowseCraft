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
| Anything else, including text with spaces              | DuckDuckGo search       |

A domain needs a dot and a top-level part of at least two letters. So `hello` and `1.5` are searched, not opened. Searches use DuckDuckGo's HTML version, which works without JavaScript.

The address bar supports selection with the mouse or `Shift` + arrows, `Ctrl` + arrows to move by word, and `Ctrl+A`, `Ctrl+C`, `Ctrl+X` and `Ctrl+V`. Press `Esc` while editing to put back the current address.

Built-in pages: `about:home` (the start page) and `about:blank`. `file://` URLs that point at a folder show a directory listing.

## Toolbar

| Button | Action                                  |
| ------ | --------------------------------------- |
| ←      | Back                                    |
| →      | Forward                                 |
| ↻ / ✕  | Reload, or stop while a page is loading |
| ⌂      | Start page                              |

A bar under the address field shows that a page is loading. Hovering a link shows its address in the bottom-left corner.

## Keyboard and mouse

| Shortcut                              | Action                    |
| ------------------------------------- | ------------------------- |
| `Ctrl+L` / `F6`                       | Focus address bar         |
| `F5` / `Ctrl+R`                       | Reload                    |
| `Alt+Left` / `Alt+Right`, mouse back/forward buttons | Back / forward |
| `Backspace` (page focused)            | Back                      |
| `Alt+Home`                            | Start page                |
| `Ctrl+Plus` / `Ctrl+Minus` / `Ctrl+0`, `Ctrl`+wheel | Zoom in / out / reset |
| `Ctrl+U`                              | View page source          |
| `Space` / `Shift+Space`, `Page Up/Down`, arrows, `Home`/`End` | Scroll |
| `Shift`+wheel                         | Scroll sideways           |
| `Ctrl+W` / `Ctrl+Q`                   | Quit                      |

## Forms

Click a text field to type in it. `Enter` submits the form and `Tab` moves to the next field. Checkboxes and radio buttons toggle on click. A `<select>` moves to its next option on each click, since there is no dropdown. Forms submit with GET or POST.

## Links and new windows

Links with `target="_blank"` open in the current view. `javascript:` and `mailto:` links are ignored. Prowl has a single window and no tabs.
