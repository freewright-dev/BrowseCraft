# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Page fullscreen: when a web page (a video, `requestFullscreen()`) goes fullscreen, the whole window goes fullscreen and the chrome is hidden, instead of the page only filling the viewport. Driven by WebView2's `ContainsFullScreenElementChanged`; the window returns to its previous state when the page leaves fullscreen. `F11` toggles window fullscreen.
- Bookmarks: a star in the address bar, `Ctrl+D`, a bookmarks bar under the address bar (`Ctrl+Shift+B` to show or hide, overflow menu, middle-click and right-click menus), the `browsecraft://bookmarks` page (`Ctrl+Shift+O`), bookmarks as quick links on the new-tab page, and `bookmarks.txt` persistence in the per-user configuration folder.
- Password manager: a KeePass 4 (`.kdbx`) vault protected by a password or a key file (created by BrowseCraft), a key menu at the top right, login/sign-up/change-password form detection, a "Save this login?" bar (save, update, never for this site), autofill, strong password suggestion, auto-lock after 15 minutes, and the `browsecraft://passwords` page. Its settings (`passwords.cfg`) live in the per-user configuration folder, which is also the suggested vault location.
- Tab context menu: duplicate tab, close tab, close other tabs.
- Browser window with a toolbar (back, forward, reload/stop, home), a loading indicator and the page title in the window title.
- BrowseCraft's own Rust rendering engine: HTML parser, CSS parser and cascade, layout (block, inline, floats, flexbox, tables, positioning) and a software rasterizer, shown through winit and softbuffer. It renders the internal pages (`about:`, `view-source:`, `browsecraft://`) and is used for everything on platforms without an embedded web view.
- Regular websites render in an embedded WebView2 pane (wry) on Windows (WKWebView on macOS), so they run JavaScript; the engine-rendered internal pages do not.
- Address bar that accepts full URLs, bare domains, `localhost` and IPv4 addresses and otherwise searches Startpage; clipboard support in the address bar.
- Optional URL or search as a command-line argument.
- Real tabs with a vertical tab sidebar: `Ctrl+T` opens a new tab on the built-in `about:home` start page, `Ctrl+W` closes one, `Ctrl+Tab`/`Ctrl+Shift+Tab` cycle them.
- Pop-ups and `target="_blank"` links open in the current tab.
- Every page that finishes loading is recorded in the session history shown at `browsecraft://history` (`Ctrl+H`, last 500 entries).
- User-settable options ("flags") persisted to `flags.txt` and edited on the `browsecraft://flags` settings page (toolbar gear button): tab sidebar position (left, right or hidden), theme and chrome backend.
- Light/dark/auto theme with a toolbar sun/moon toggle; `auto` follows the system setting and the theme is applied to WebView2 pages too.
- Optional GPU chrome (`chrome = auto | egui | engine`): with an available adapter, `egui` on `wgpu` draws the toolbar and tab sidebar and shows engine pages as a texture; otherwise the engine/softbuffer chrome is kept. The choice is made at startup. The egui chrome has a rounded address pill and a hoverable tab list.
- Toolbar and tab-sidebar icons from Phosphor (the `egui-phosphor` crate), embedded in the binary as an icon font (`src/icons.rs`), so they render regardless of the working directory.
- Keyboard shortcuts: `Ctrl+L`/`F6` and `Ctrl+S` focus the address bar, `F5`/`Ctrl+R` reload, `Alt+Left`/`Alt+Right` and `Ctrl+Left`/`Ctrl+Right` go back/forward, `Alt+Home` goes home, `Ctrl+Q` quits, and `Shift+K` shows a keyboard cheatsheet overlay.
- `Ctrl+Shift+S` saves a screenshot of the visible page to the Pictures folder (on Windows by capturing the screen, so WebView2 pages are included), with a status strip under the page confirming it.
- Engine features: networking with ureq and rustls, `file://` directory listings, `data:` URLs, `view-source:` (`Ctrl+U`), images (PNG, JPEG, GIF, WebP) that load in the background, form fields and GET/POST form submission, link previews on hover and keyboard scrolling.
- Zoom (`Ctrl+Plus`/`Ctrl+Minus`/`Ctrl+0`), for both engine and WebView2 pages.
- `--screenshot <out.png> <url> [width]` renders a page with the engine to PNG without a window.
- `build.bat`: Windows build script (`release`, `debug`, `test`, `clean`, `run`) that checks for Rust 1.95 or newer, builds, and collects `browsecraft.exe` in `dist\`.
- Size-optimized release profile (`opt-level = "z"`, LTO, stripped).
- Unit tests for the HTML, CSS, style, font, network, text-editing, address, browser, history, flags, storage, bookmarks, vault and password-manager modules.
- Documentation in `docs/`.

### Changed

- Modern look: a refreshed dark and light palette for the toolbar, tabs and address bar (rounded, with a focus ring), and a shared stylesheet for every internal page (new tab, settings, history, bookmarks, passwords, error pages). Internal pages follow the theme and reload when it changes.
- The theme now defaults to dark.
- The settings page is redesigned with toggle switches and segmented controls; changing a setting no longer adds a Back step each time.
- Declared GPL-3.0-or-later licensing with the full license text and uBlock Origin attribution; added an explicit dependency license/source policy.
- CI runs `cargo deny check` (advisories, bans, licenses, sources) on every pull request, on pushes to `main` and weekly. The two tracked unmaintained advisories (`proc-macro-error`, `ttf-parser`) are ignored by ID with a reason; any other advisory fails the check (#76).
- The internal pages moved from `craft://` to `browsecraft://` (`browsecraft://flags`, `browsecraft://history`), and the repository moved to `freewright-dev/BrowseCraft`.
- Replaced the original wry/tao-only browser with a hybrid design: BrowseCraft's own engine and winit window draw the chrome and internal pages, while websites render in an embedded WebView2 pane. The HTML toolbar webview is gone; the toolbar is drawn by the engine or by egui.
- Searches use Startpage instead of DuckDuckGo.
- The home page (`Alt+Home`, and the page shown at startup without an argument) is Startpage; new tabs open the built-in `about:home` start page.

### Fixed

- The password key icon was pushed past the right edge of the window: the toolbar reserved room for two right-hand buttons and there are three. The buttons on the right are now laid out from the window's edge and the address bar takes the room left over, so nothing can leave the window at any width (covered by a headless layout test).
- Clicks and scrolling on engine-rendered pages (the new-tab page and its search box, settings, bookmarks, history, passwords) landed about 34 px off once the bookmarks bar was added: the page area used for layout, painting and hit-testing did not include the bookmarks bar or the "save this login?" bar. It now matches the egui panels exactly.
- A click on a menu that hangs over the page (the key menu, a bookmark's right-click menu, the `»` overflow menu) was also delivered to the page underneath.
- Menus drawn over a web page were hidden behind the WebView2 pane (a native window that always paints on top of egui). While a menu is open the page is now shown as a snapshot and the live pane is hidden, then restored when the menu closes.
- The vault was queried (copied and sorted) on every frame to show the key icon's state; it now counts logins cheaply and loads site logins only while the menu is open.
- Updated rustls to 0.23.46 for four TLS advisories and wry to 0.55.1 to remove the affected fxhash/rand dependency chains. Remaining GTK/font maintenance advisories are tracked in `docs/dependencies.md` (#54).
- Settings now use atomic replacement in a per-user configuration directory, preventing planted working-directory symlinks from redirecting writes. Save errors are displayed and preserve the in-memory settings; trusted legacy files can be copied manually to the new location (#56).
- Settings forms in multiple retained tabs keep independent OS-random one-time tokens, with bounded storage, replay rejection and an explanation when a form has expired (#57).
- Bookmarks and the password manager's settings use the same atomic, owner-only storage in the per-user configuration folder as the settings (`src/storage.rs`), and the bookmarks and passwords pages use the same bounded one-time tokens. Vault saves no longer go through a predictable `<vault>.tmp` file that a planted symlink could redirect.
- Password copies made while filling a login, offering to save one, revealing it or checking the clipboard are wiped from memory when dropped (`zeroize`), instead of lingering after the vault locks (#77).
- Saved logins are matched to sites with the Public Suffix List (`psl`): a login stays within one registrable domain, so one saved for a public suffix such as `github.io` or `co.uk` is no longer offered on every site under it (#78).
- The WebView2 navigation handler refuses main-frame navigations to internal pages (`browsecraft:`, `about:` other than `about:blank`, `view-source:`), so a web page cannot replace itself with an internal page by setting `location` (#79).
- Ad blocking loads at most 200,000 filter rules; the Windows page script no longer embeds the network rules it never uses (WebView2 filters natively); and the macOS JavaScript hooks take the host from the parsed URL, so a host name in the user info or path cannot shift where host rules match (#80).
- Copying passwords no longer starts a 30-second thread per copy: one clipboard clearer serves every copy, and a new copy replaces the pending clear (#81).
- A login added on `browsecraft://passwords` must have an `http(s)` address with a host (or none); `javascript:`, `data:` and other URLs are refused (#81).
- An SVG file opened in the engine (`file://` or `image/svg+xml`) shows its source instead of a "can't be displayed" page, since the engine does not draw SVG (#81).
- Windows screenshot capture rejects overflowing or excessive dimensions before allocation, validates bitmap selection/restoration, and releases native resources on error paths (#55).
- Ad blocking now checks every engine HTTP redirect destination and uses native WebView2 interception for parser-initiated resources and redirects. JavaScript fallback fetch hooks handle URL and Request objects with one native conversion (#52).
- Cancelled engine loads stop scheduling document/stylesheet/import/image work and publishing stale events. Background loads use a bounded two-worker queue and whole-load deadlines (#53).
- Nested `:is()`/`:not()` selector matching now shares cached results and a work budget, preventing the reported CPU-exhaustion path in the custom engine ([#48](https://github.com/freewright-dev/BrowseCraft/issues/48), [PR #58](https://github.com/freewright-dev/BrowseCraft/pull/58)).
- Matching selector lists apply each rule's declarations once using the greatest matching specificity and rule source order, removing multiplicative cascade allocations. An aggregate per-element declaration limit bounds cascade size ([#49](https://github.com/freewright-dev/BrowseCraft/issues/49), [PR #58](https://github.com/freewright-dev/BrowseCraft/pull/58)).
- Long sibling-selector chains are rejected by a shared 64-part selector-tree limit; runtime recursion is also bounded to prevent stack overflow, including combinations with nested pseudo-selectors ([#50](https://github.com/freewright-dev/BrowseCraft/issues/50), [PR #58](https://github.com/freewright-dev/BrowseCraft/pull/58)).
- Removed quadratic at-rule prelude scanning and repeated root/body attribute merging. Cached sibling links and indices make positional selector checks constant-time after a linear metadata build ([#51](https://github.com/freewright-dev/BrowseCraft/issues/51), [PR #59](https://github.com/freewright-dev/BrowseCraft/pull/59)).
- BrowseCraft's keyboard shortcuts (new/close tab, address bar, reload, back/forward, zoom, history and the rest) now work while a web page has the keyboard, instead of only after clicking the toolbar or sidebar.
- Keyboard shortcuts in the WebView2 pane now use the controller's native `AcceleratorKeyPressed` event instead of an injected script over `window.ipc`, preventing page JavaScript from triggering browser shortcuts.
