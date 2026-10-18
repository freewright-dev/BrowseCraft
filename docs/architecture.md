# Architecture

BrowseCraft renders internal pages with its own engine and ordinary web pages in an
embedded WebView2 pane (see "Engine pages and the WebView2 pane"). Each engine
stage is a module in `src/`:

```
loader ──► net ──► html ──► dom
              │
              └──► css ──► style ──► layout ──► paint ──► softbuffer window
                                                       └─► egui texture (GPU chrome)
```

| Module           | Purpose                                                                 |
| ---------------- | ----------------------------------------------------------------------- |
| `main.rs`        | Entry point; window mode or `--screenshot`                              |
| `browser.rs`     | Window, toolbar, tabs, history, input, forms, WebView2 pane, render loop |
| `flags.rs`       | User-settable flags persisted to `flags.txt`; the `browsecraft://flags` page  |
| `egui_chrome.rs` | GPU chrome: egui toolbar/sidebar on wgpu, page raster as a texture      |
| `icons.rs`       | Chrome icon codepoints (Phosphor) and the embedded icon font            |
| `adblock.rs`     | Ad blocker ported from uBlock Origin: network and cosmetic filters      |
| `history.rs`     | In-memory browsing history; the `browsecraft://history` page                  |
| `capture.rs`     | Screen-region capture (GDI `BitBlt`) for screenshots of WebView2 pages  |
| `loader.rs`      | Background page loads: document, stylesheets, then images; internal pages |
| `net.rs`         | `http(s)` (ureq + rustls), `file:`, `data:`, charset decoding           |
| `html.rs`        | Tokenizer and tree builder                                              |
| `dom.rs`         | Arena-based document tree                                               |
| `css.rs`         | Stylesheet, selector and value parsing; selector matching; media queries |
| `style.rs`       | Cascade and computed styles                                             |
| `ua.css`         | The default (user-agent) stylesheet, embedded by `style.rs`             |
| `layout.rs`      | Box layout; produces the display list                                   |
| `paint.rs`       | Software rasterizer                                                     |
| `font.rs`        | System fonts through fontdue, glyph fallback, caches; Phosphor chrome icons |
| `textfield.rs`   | Single-line text editing for the address bar and form fields            |
| `headless.rs`    | Renders a page to PNG without a window                                  |
| `address.rs`     | Turns address-bar input into a URL (or a Startpage search)              |
| `home.html`      | The `about:home` page that new tabs open, embedded by `loader.rs`       |

## Engine pages and the WebView2 pane

Two kinds of pages live side by side. `browser::request_in_webview` decides:
`about:`, `view-source:` and `browsecraft:` URLs (and every form `POST`) go through
the engine; everything else is loaded into a `wry` WebView2 child window placed
over the page area. If no web view exists (WebView2 failed to start, or the
platform is neither Windows nor macOS, where `wry` uses WKWebView), every page
goes through the engine.

The engine handles these internal URLs in `loader::load_page`:
`about:home`, `about:blank`, `view-source:<url>`, `browsecraft://flags`,
`browsecraft://history`, and `about:blocked:<url>` (the notice shown when the ad
filter blocks a page). WebView2 events (`WebStart`, `WebLoad`, `WebTitle`,
`WebOpen`, `WebKey`) reach the UI thread as `UserMessage`s. Pop-ups are denied and
reopened in the current tab.

A WebView2 page has the keyboard to itself, so winit never sees its keys.
On Windows, `attach_webview_shortcuts` hooks the WebView2 controller's native
`AcceleratorKeyPressed` event, maps the virtual key and modifiers through
`vk_to_key` and `shortcut_for` (the same table `key()` uses), marks the key
handled and sends `UserMessage::WebKey` to the UI thread. Because this runs on
the controller rather than in page JavaScript, pages cannot forge shortcuts
over IPC and keys pressed inside iframes are covered too.

## Loading

`loader::spawn` runs on a background thread. It fetches the document, parses
it, and fetches every `<link rel=stylesheet>` in parallel, along with nested
`@import`s. It then sends a `LoadEvent::Page` to the UI thread through winit's
`EventLoopProxy`. Images follow from a small worker pool (`LoadEvent::Image`),
and each one triggers a relayout. Every load has a generation number, so
events from an outdated load are dropped.

## Parsing and sibling metadata

CSS at-rule preludes use one forward scan to the first top-level `{` or `;`,
respecting quotes, escapes and function/bracket nesting. A run of sibling
at-rules therefore does not repeatedly scan the remaining stylesheet for a
delimiter that is absent.

The HTML builder retains separate attribute-name sets for the root and body
throughout parsing. Repeated `<html>`/`<body>` tags add only unseen attributes,
preserving the first value and insertion order without rescanning old names.

`Document` lazily caches element sibling links, one-based element and same-tag
indices, and last-of-type flags. Building this metadata visits each parent's
children twice; subsequent first/last/nth-child and first/last-of-type checks
use constant-time lookups. Sibling iterators skip text nodes via cached links.
`append` and `element_mut` invalidate the cache, so structural additions and
tag changes must use those methods. The cache is shared across matching calls
and restyles of an unchanged document.

## Style

`style::compute_styles` gives every node an `Rc<Style>`. Rules are bucketed by
the subject's id, class or tag, so each element only tests likely selectors.
Each rule contributes its declarations once, using the greatest specificity
among its matching selectors and the rule's source order.
Declarations are sorted by importance, origin (UA, HTML hints, author, inline),
specificity and order. Custom properties resolve first so `var()` can be
substituted, then shorthands expand into longhands. Lengths come out in
**device pixels** (CSS px × scale factor × zoom); percentages stay for layout
to resolve.

Selector parsing permits at most 64 compound parts across an entire selector
tree, including nested pseudo-selector lists. Matching also caps recursion at
64 parts and shares memoized successes/failures across `:is()`/`:not()` calls,
keyed by selector suffix identity and DOM node. A 100,000-step budget covers
state visits and class/attribute/pseudo checks, shared by all candidate
selectors for one element (or one standalone test-helper match).
Exhaustion propagates as an error, so `:not()` cannot invert it into a match;
styling stops evaluating further candidates for that element. The cascade
retains at most 65,536 declarations per element, gathered in rule source order,
then HTML hints and inline styles; excess declarations are ignored before
sorting and value expansion. These limits apply to the custom engine.

Styles depend on the viewport only through media queries and viewport units,
so the browser restyles only when the set of matching `@media` queries or the
zoom changes.

## Layout

`layout::layout` walks the DOM with the computed styles and appends paint
items (`Fill`, `Border`, `Text`, `Image`, clip push/pop) to one list. It also
records link rectangles, form controls and anchor positions for hit testing.

Boxes are laid out where they are first seen. When a parent learns a child's
final position later, as with centering, flex alignment, table cell alignment
or inline-blocks on a line, it moves the child by translating the range of
items the child produced. Backgrounds are emitted as placeholders before the
children and filled in once the box's height is known, so they paint
underneath.

- **Block flow:** vertical stacking with sibling margin collapsing and auto-margin centering.
- **Inline flow:** text is split into words and spaces according to `white-space`. Lines are filled greedily and placed one at a time, so floats placed earlier narrow later lines.
- **Shrink-to-fit:** inline-blocks, floats, tables and flex items use cached min/max-content widths.
- **Flexbox:** single-pass hypothetical sizes, wrapping, grow/shrink with minimums, auto margins, `justify-content` and `align-items`.
- **Tables:** the CSS auto table layout algorithm, simplified, with colspan; cells stretch to the row height.
- **Positioning:** absolute boxes wait for their containing block's size, then are placed using `top/left/right/bottom`.

## Painting

`paint::paint_items` draws only the items that intersect the viewport into a
`u32` buffer, using alpha blending, anti-aliased rounded rectangles and cached
glyph bitmaps. On the engine chrome path the browser draws its toolbar and
sidebar with the same `Canvas`, then copies the frame to the window with
softbuffer. On the egui path only the page is rasterized and uploaded as a
texture.

## Browser chrome and tabs

On the engine chrome path the toolbar, address bar and tab sidebar are drawn
by the engine itself (the same `Canvas` as page content), not the system.
Icons are Phosphor (embedded in the binary by the `egui-phosphor` crate,
codepoints in `src/icons.rs`), loaded as `Family::Icon` in `font.rs`.

Tabs are real: `browser.rs` keeps a `TabState` per open tab (history, history
index, engine page as an optional `Tab`, whether it shows a web page, webview
URL, title). Exactly one tab is active; its fields are the live `Browser`
fields. `park_active_tab` / `activate_pantry` swap between the two, so
switching tabs never reloads engine pages. WebView2 tabs re-issue `load_url`
on activation (there is one shared web view). Ctrl+T/W/Tab and the sidebar
open, close and cycle tabs; closing the last tab quits. New tabs open
`about:home`; the Home button opens `HOME_URL` (Startpage). The sidebar sits
on the left or right, or is hidden (`tabs-position`), and insets the page and
webview rectangles.

## Flags and `browsecraft://`

`flags.rs` holds user settings (`tabs-position`, `theme`, `chrome`) in a
`RwLock`, persisted as `key=value` lines in the per-user configuration directory.
An exclusive temporary file is synced, then atomically replaces `flags.txt`;
memory changes only after a successful save. `browsecraft://flags` is an
engine-internal page: `loader.rs` passes its query to `flags::apply_form`.
Each form owns a 128-bit OS-random one-time token. Up to 128 forms may remain
outstanding; issuing a new token preserves other tabs, evicting only the oldest
entry when full. Submitting consumes only that form's token, and replayed or
evicted forms show an expiration message. `browsecraft://` URLs are routed to the engine by
`request_in_webview`. The theme toggle (toolbar sun/moon) flips `theme` between
Light and Dark; `Auto` follows the OS.

## Chrome: engine or egui

The toolbar and tab sidebar are drawn either by the engine into a softbuffer
frame or by egui on wgpu (`egui_chrome.rs`). In `resumed()`, unless
`chrome = engine`, `EguiChrome::new` tries to create a wgpu adapter, device
and surface; `egui_chrome::resolve` then picks egui when that succeeded.
`EguiChrome` owns the window surface (`egui-wgpu` painter) and draws the
toolbar/sidebar as egui widgets, with the engine page raster uploaded as a
texture in the central panel. If GPU setup fails, or `chrome = engine`,
`browser.rs` creates the softbuffer surface (`Gfx::surface` is `Some`) and
draws its own chrome. The choice is made once at startup; the WebView2 pane is
placed from the egui central panel rect on the GPU path. See `docs/egui.md`.

## Ad blocking

`adblock.rs` is a port of uBlock Origin's static network filtering and `##`
element hiding, using EasyList/uBO syntax. `adblock::instance()` builds one
shared `AdBlocker` from a built-in starter list plus every `filters/*.txt`
file in the working directory and next to the executable. It feeds three
sinks:

1. **Rust-side network checks** (`matches_network`, through `blocked_page` and
   `blocked_subresource`): main-frame navigations and pop-ups are checked
   before they load (a blocked page becomes `about:blocked:<url>`), and engine
   pages skip blocked stylesheets and images.
2. **WebView2 pages:** `page_script()` is installed as the web view's
   initialization script. It cancels matching fetch/XHR/beacon/img/script/
   iframe loads and injects a `<style>` for the `##` selectors.
3. **Engine pages:** `engine_cosmetic_css(host)` returns a stylesheet that
   `loader.rs` appends to fetched pages, hiding matches with `display: none`.

Regex filters, `$redirect`/`$csp`/`$replace`, scriptlets and procedural
filters are dropped; `$domain=` is only honoured on the Rust side.

## History

`history.rs` keeps up to 500 visits (URL and title) in a global
`Mutex<Vec<_>>`, shared by all tabs, in memory only: it is not saved to disk.
Engine pages are recorded in `handle_load` and WebView2 pages in
`web_loaded`/`web_titled`; consecutive visits to the same URL update the title
instead of adding a row, and `browsecraft://`, `about:blank` and `view-source:` URLs
are skipped. `browsecraft://history` (Ctrl+H) renders the list, newest first. This
is separate from each tab's back/forward list (`HistoryEntry` in
`browser.rs`).

## Screenshots

`browsecraft --screenshot <out.png> <url> [width]` uses `headless::screenshot`: a
blocking engine load (`loader::load_blocking`), layout at the given width
(default 1200) and a PNG of up to 6000 px height. No WebView2 is involved.

In the window, Ctrl+Shift+S saves the visible page area to the Pictures folder
(or the working directory) as `browsecraft-YYYYMMDD-HHMMSS.png` (`save_screenshot`). Engine pages are cropped
from the last rendered frame. A WebView2 page is a separate native window, so
`capture::screen_region` grabs that rectangle from the screen with GDI
`BitBlt`; on other platforms capturing a web page fails with a notice.

## Known limitations

No JavaScript on engine pages, CSS Grid (laid out as blocks), transforms, SVG,
web fonts, background images, `::before`/`::after` content or text shaping
(ligatures and right-to-left scripts). Fixed elements are positioned once, not
kept in place while scrolling.
