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
| `browser.rs`   | Window, toolbar, history, input, forms, render loop                     |
| `loader.rs`    | Background page loads: document, stylesheets, then images               |
| `net.rs`       | `http(s)` (ureq + rustls), `file:`, `data:`, charset decoding           |
| `html.rs`      | Tokenizer and tree builder                                              |
| `dom.rs`       | Arena-based document tree                                               |
| `css.rs`       | Stylesheet, selector and value parsing; selector matching; media queries |
| `style.rs`     | Cascade and computed styles                                             |
| `ua.css`       | The default (user-agent) stylesheet                                     |
| `layout.rs`    | Box layout; produces the display list                                   |
| `paint.rs`     | Software rasterizer                                                     |
| `font.rs`      | System fonts through fontdue, glyph fallback, caches                    |
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

## Known limitations

No JavaScript, CSS Grid (laid out as blocks), transforms, SVG, web fonts,
background images, `::before`/`::after` content or text shaping (ligatures and
right-to-left scripts). Fixed elements are positioned once, not kept in place
while scrolling.
