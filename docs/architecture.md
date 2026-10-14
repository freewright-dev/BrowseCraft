# Architecture

Prowl renders pages with its own engine. Each stage is a module in `src/`:

```
loader ──► net ──► html ──► dom
              │
              └──► css ──► style ──► layout ──► paint ──► softbuffer window
```

| Module         | Purpose                                                                 |
| -------------- | ----------------------------------------------------------------------- |
| `main.rs`      | Entry point; window mode or `--screenshot`                              |
| `browser.rs`   | Window, toolbar, tab strip, history, input, forms, render loop          |
| `flags.rs`     | User-settable flags persisted to `flags.txt`; the `prowl://flags` page  |
| `egui_chrome.rs` | GPU chrome: egui toolbar/sidebar on wgpu, page raster as a texture    |
| `icons.rs`     | Chrome icon codepoints (Phosphor) and the embedded icon font            |
| `loader.rs`    | Background page loads: document, stylesheets, then images               |
| `net.rs`       | `http(s)` (ureq + rustls), `file:`, `data:`, charset decoding           |
| `html.rs`      | Tokenizer and tree builder                                              |
| `dom.rs`       | Arena-based document tree                                               |
| `css.rs`       | Stylesheet, selector and value parsing; selector matching; media queries |
| `style.rs`     | Cascade and computed styles                                             |
| `ua.css`       | The default (user-agent) stylesheet                                     |
| `layout.rs`    | Box layout; produces the display list                                   |
| `paint.rs`     | Software rasterizer                                                     |
| `font.rs`      | System fonts through fontdue, glyph fallback, caches; Phosphor chrome icons |
| `textfield.rs` | Text editing for the address bar and form fields                        |
| `headless.rs`  | Renders a page to PNG without a window                                  |
| `address.rs`   | Turns address-bar input into a URL                                      |
| `home.html`    | The `about:home` start page                                             |

## Loading

`loader::spawn` runs on a background thread. It fetches the document, parses
it, and fetches every `<link rel=stylesheet>` in parallel, along with nested
`@import`s. It then sends a `LoadEvent::Page` to the UI thread through winit's
`EventLoopProxy`. Images follow one by one from a small worker pool
(`LoadEvent::Image`), and each one triggers a relayout. Every load has a
generation number, so events from an outdated load are dropped.

## Style

`style::compute_styles` gives every node an `Rc<Style>`. Rules are bucketed by
the subject's id, class or tag, so each element only tests likely selectors.
Declarations are sorted by importance, origin (UA, HTML hints, author, inline),
specificity and order. Custom properties resolve first so `var()` can be
substituted, then shorthands expand into longhands. Lengths come out in
**device pixels** (CSS px × scale factor × zoom); percentages stay for layout
to resolve.

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
glyph bitmaps. The browser draws its toolbar with the same `Canvas`, then
copies the frame to the window with softbuffer.

## Browser chrome and tabs

The toolbar, address bar and tab sidebar are drawn by the engine itself (the
same `Canvas` as page content), not the system. Icons are Phosphor (embedded
in the binary by the `egui-phosphor` crate, codepoints in `src/icons.rs`),
loaded as `Family::Icon` in `font.rs`.

Tabs are real: `browser.rs` keeps a `TabState` pantry per open tab (history,
history index, engine page, loading flag, webview URL/title). Exactly one tab
is active; its fields are the live `Browser` fields. `park_active_tab` /
`activate_pantry` swap between the two, so switching tabs never reloads engine
pages. WebView2 tabs re-issue `load_url` on activation. Ctrl+T/W/Tab and the
sidebar open, close and cycle tabs. The sidebar sits on the left or right
(`tabs-position`) and insets the viewport and webview rectangles.

## Flags and `prowl://`

`flags.rs` holds user settings (`tabs-position`, `theme`, `chrome`) in a
`RwLock`, persisted as `key=value` lines in `flags.txt` (cwd or exe dir).
`prowl://flags` is an engine-internal page: `loader.rs` parses its query string
with `flags::apply_query`, and the radio buttons submit
`prowl://flags?key=value`. `prowl://` URLs are routed to the engine by
`request_in_webview`. The theme toggle (toolbar sun/moon) flips `theme` between
Light and Dark; `Auto` follows the OS.

## Chrome: engine or egui

The toolbar and tab sidebar are drawn by the engine (the same `Canvas` as page
content) by default. GPU chrome lives in `egui_chrome.rs`: when
`chrome = auto | egui` and a wgpu adapter/device can be created in
`resumed()`, `EguiChrome` owns the window surface (`egui-wgpu` painter) and
draws the toolbar/sidebar as egui widgets, with the engine page raster uploaded
as a texture in the central panel. If GPU setup fails, or `chrome = engine`,
`browser.rs` keeps the softbuffer chrome (`Gfx::surface` is `Some`). The choice
is made once at startup; the WebView2 pane is placed from the egui central
panel rect on the GPU path. See `docs/egui.md`.

## Known limitations

No JavaScript, CSS Grid (laid out as blocks), transforms, SVG, web fonts,
background images, `::before`/`::after` content or text shaping (ligatures and
right-to-left scripts). Fixed elements are positioned once, not kept in place
while scrolling.
