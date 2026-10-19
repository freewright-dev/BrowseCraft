//! Layout: turns the styled document into a display list of positioned
//! rectangles, text runs and images, in device pixels.
//!
//! Boxes are laid out straight from the DOM. Each box appends its paint items
//! to one list, and parents that only learn a child's final position later
//! (centering, flex, tables, inline blocks) move the child's items by
//! translating the range it produced.

use std::collections::HashMap;
use std::rc::Rc;

use crate::css::{Color, Length};
use crate::dom::{Document, NodeId, NodeKind};
use crate::font::{FontKey, Fonts};
use crate::paint::Image;
use crate::style::{
    Align, Display, Float, Justify, ListStyle, Position, Style, TextAlign, TextTransform, VAlign,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

pub enum Item {
    Fill {
        rect: Rect,
        color: Color,
        radius: f32,
    },
    Border {
        rect: Rect,
        widths: [f32; 4],
        colors: [Color; 4],
        radius: f32,
    },
    Text {
        x: f32,
        y: f32,
        text: String,
        font: FontKey,
        size: f32,
        color: Color,
    },
    Image {
        rect: Rect,
        src: Rc<str>,
    },
    PushClip(Rect),
    PopClip,
}

impl Item {
    fn translate(&mut self, dx: f32, dy: f32) {
        match self {
            Item::Fill { rect, .. }
            | Item::Border { rect, .. }
            | Item::Image { rect, .. }
            | Item::PushClip(rect) => {
                rect.x += dx;
                rect.y += dy;
            }
            Item::Text { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            Item::PopClip => {}
        }
    }

    fn rect_mut(&mut self) -> Option<&mut Rect> {
        match self {
            Item::Fill { rect, .. } | Item::Border { rect, .. } | Item::PushClip(rect) => {
                Some(rect)
            }
            _ => None,
        }
    }
}

/// A form control's position, for hit testing and drawing the caret.
pub struct Control {
    pub rect: Rect,
    pub node: NodeId,
    pub text_x: f32,
    pub baseline: f32,
    pub font: FontKey,
    pub size: f32,
}

#[derive(Default)]
pub struct Layout {
    pub items: Vec<Item>,
    pub links: Vec<(Rect, Rc<str>)>,
    pub controls: Vec<Control>,
    /// Top edge of every element with an `id` or `<a name>`.
    pub anchors: Vec<(NodeId, f32)>,
    pub width: f32,
    pub height: f32,
    pub background: Color,
}

/// Values the user has typed into form controls.
#[derive(Default)]
pub struct FormState {
    pub values: HashMap<NodeId, String>,
    pub checked: HashMap<NodeId, bool>,
}

impl FormState {
    pub fn value<'a>(&'a self, doc: &'a Document, node: NodeId) -> &'a str {
        if let Some(v) = self.values.get(&node) {
            return v;
        }
        let Some(el) = doc.element(node) else {
            return "";
        };
        match el.tag.as_str() {
            "textarea" => doc.nodes[node]
                .children
                .first()
                .and_then(|&c| match &doc.nodes[c].kind {
                    NodeKind::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .unwrap_or(""),
            _ => el.attr("value").unwrap_or(""),
        }
    }

    pub fn is_checked(&self, doc: &Document, node: NodeId) -> bool {
        self.checked.get(&node).copied().unwrap_or_else(|| {
            doc.element(node)
                .is_some_and(|e| e.attr("checked").is_some())
        })
    }
}

pub struct LayoutInput<'a> {
    pub doc: &'a Document,
    pub styles: &'a [Rc<Style>],
    pub fonts: &'a Fonts,
    pub images: &'a HashMap<String, Rc<Image>>,
    pub form: &'a FormState,
    pub zoom: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
}

pub fn layout(input: &LayoutInput) -> Layout {
    let doc = input.doc;
    let html = doc.nodes[Document::ROOT]
        .children
        .iter()
        .copied()
        .find(|&n| doc.element(n).is_some());
    let mut engine = Engine {
        input,
        out: Layout::default(),
        abs_stack: vec![Vec::new()],
        hidden: 0,
        link: None,
        inline_bgs: Vec::new(),
        intrinsic_cache: HashMap::new(),
        first_baseline: None,
        ordinals: HashMap::new(),
    };
    let Some(html) = html else { return engine.out };

    // The root's background, or else the body's, paints the whole canvas.
    let body = doc.find_tag("body");
    let html_bg = input.styles[html].background;
    engine.out.background = if html_bg.a > 0 {
        html_bg
    } else {
        body.map(|b| input.styles[b].background)
            .filter(|c| c.a > 0)
            .unwrap_or(Color::WHITE)
    };

    let size = engine.block(html, 0.0, 0.0, input.viewport_width, Sizing::Fill, None);
    let pending = engine.abs_stack.pop().unwrap_or_default();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        w: input.viewport_width,
        h: input.viewport_height,
    };
    engine.place_absolutes(pending, viewport);
    engine.out.height = engine.out.height.max(size.margin_h());
    engine.out.width = engine.out.width.max(size.w);
    engine.out
}

/// Columns per table. Each one costs width bookkeeping for every row, and
/// `colspan` alone lets a few KB of HTML ask for millions.
const MAX_TABLE_COLUMNS: usize = 1000;

#[derive(Clone, Copy)]
enum Sizing {
    /// Auto width fills the containing block.
    Fill,
    /// Auto width shrinks to fit the content.
    Shrink,
    /// The border box is exactly this wide.
    Exact(f32),
}

#[derive(Clone, Copy)]
struct Mark {
    items: usize,
    links: usize,
    controls: usize,
    anchors: usize,
}

struct BoxSize {
    /// Margin box width.
    w: f32,
    /// Border box height.
    h: f32,
    mt: f32,
    mb: f32,
    /// Background and border items, which a table row or flex line may
    /// stretch to its full height.
    decorations: Vec<usize>,
}

impl BoxSize {
    fn margin_h(&self) -> f32 {
        self.mt + self.h + self.mb
    }
}

struct Pending {
    node: NodeId,
    x: f32,
    y: f32,
}

#[derive(Clone)]
struct InlineBg {
    color: Color,
    pad_top: f32,
    pad_bottom: f32,
    radius: f32,
}

enum InlineItem {
    Text {
        node: NodeId,
        text: String,
        link: Option<Rc<str>>,
        bg: Option<InlineBg>,
    },
    Atomic {
        node: NodeId,
        link: Option<Rc<str>>,
    },
    Break,
    Spacer(f32),
}

enum PieceKind {
    Word(String),
    Space,
    Atomic,
    Break,
    Spacer,
}

struct Piece {
    kind: PieceKind,
    width: f32,
    item: usize,
}

struct AtomicBox {
    size: BoxSize,
    start: Mark,
    end: Mark,
    float: Float,
}

struct Engine<'a> {
    input: &'a LayoutInput<'a>,
    out: Layout,
    /// Absolutely positioned boxes waiting for their containing block.
    abs_stack: Vec<Vec<Pending>>,
    /// Above zero inside `visibility: hidden` or `opacity: 0`.
    hidden: u32,
    link: Option<Rc<str>>,
    inline_bgs: Vec<InlineBg>,
    intrinsic_cache: HashMap<NodeId, (f32, f32)>,
    first_baseline: Option<f32>,
    /// List item numbers, filled in for a whole list at its first marker.
    ordinals: HashMap<NodeId, i32>,
}

fn resolve(l: Length, base: f32) -> Option<f32> {
    match l {
        Length::Px(v) => Some(v),
        Length::Percent(p) => Some(base * p / 100.0),
        _ => None,
    }
}

fn resolve0(l: Length, base: f32) -> f32 {
    resolve(l, base).unwrap_or(0.0)
}

fn font_key(style: &Style) -> FontKey {
    FontKey {
        family: style.family,
        bold: style.bold,
        italic: style.italic,
    }
}

fn is_replaced(tag: &str) -> bool {
    matches!(tag, "img" | "input" | "select" | "textarea")
}

impl<'a> Engine<'a> {
    fn doc(&self) -> &'a Document {
        self.input.doc
    }

    fn st(&self, node: NodeId) -> &'a Style {
        &self.input.styles[node]
    }

    fn fonts(&self) -> &'a Fonts {
        self.input.fonts
    }

    fn mark(&self) -> Mark {
        Mark {
            items: self.out.items.len(),
            links: self.out.links.len(),
            controls: self.out.controls.len(),
            anchors: self.out.anchors.len(),
        }
    }

    fn translate(&mut self, from: Mark, to: Mark, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        for item in &mut self.out.items[from.items..to.items] {
            item.translate(dx, dy);
        }
        for (r, _) in &mut self.out.links[from.links..to.links] {
            r.x += dx;
            r.y += dy;
        }
        for c in &mut self.out.controls[from.controls..to.controls] {
            c.rect.x += dx;
            c.rect.y += dy;
            c.text_x += dx;
            c.baseline += dy;
        }
        for (_, y) in &mut self.out.anchors[from.anchors..to.anchors] {
            *y += dy;
        }
    }

    fn translate_since(&mut self, from: Mark, dx: f32, dy: f32) {
        let to = self.mark();
        self.translate(from, to, dx, dy);
    }

    /// Appends a paint item and returns its index. Inside hidden content
    /// the item becomes an invisible placeholder, so indices stay valid.
    fn emit(&mut self, item: Item) -> usize {
        let item = if self.hidden > 0 {
            Item::Fill {
                rect: Rect::default(),
                color: Color::TRANSPARENT,
                radius: 0.0,
            }
        } else {
            item
        };
        self.out.items.push(item);
        self.out.items.len() - 1
    }

    fn stretch(&mut self, size: &BoxSize, new_h: f32) {
        let delta = new_h - size.h;
        if delta <= 0.0 {
            return;
        }
        for &i in &size.decorations {
            if let Some(r) = self.out.items[i].rect_mut() {
                r.h += delta;
            }
        }
    }

    fn margins(&self, style: &Style, cb_w: f32) -> [f32; 4] {
        std::array::from_fn(|i| resolve0(style.margin[i], cb_w))
    }

    fn paddings(&self, style: &Style, cb_w: f32) -> [f32; 4] {
        std::array::from_fn(|i| resolve0(style.padding[i], cb_w).max(0.0))
    }

    /// Lays out a block-level box with its margin box's top-left corner at
    /// (x, y) inside a containing block `cb_w` wide.
    fn block(
        &mut self,
        node: NodeId,
        x: f32,
        y: f32,
        cb_w: f32,
        sizing: Sizing,
        forced_h: Option<f32>,
    ) -> BoxSize {
        let st = self.st(node);
        let doc = self.doc();
        let tag = doc.tag(node);
        let mut m = self.margins(st, cb_w);
        let p = self.paddings(st, cb_w);
        let b = st.border_width;
        let extra_w = p[1] + p[3] + b[1] + b[3];
        let extra_h = p[0] + p[2] + b[0] + b[2];
        let natural = if is_replaced(tag) {
            self.natural_size(node)
        } else {
            None
        };

        let to_content = |w: f32| if st.border_box { w - extra_w } else { w };
        let mut content_w = match sizing {
            Sizing::Exact(w) => w - extra_w,
            _ => match resolve(st.width, cb_w) {
                Some(w) => to_content(w),
                None => {
                    let avail = cb_w - m[1] - m[3] - extra_w;
                    let shrink = matches!(sizing, Sizing::Shrink)
                        || st.display == Display::Table
                        || matches!(st.display, Display::InlineBlock | Display::InlineFlex);
                    if let Some((nw, nh)) = natural {
                        match resolve(st.height, 0.0)
                            .filter(|_| !matches!(st.height, Length::Percent(_)))
                        {
                            Some(h) if nh > 0.0 => to_content_h(st, h, extra_h) * nw / nh,
                            _ => nw,
                        }
                    } else if shrink {
                        let (min, max) = self.intrinsic_content(node);
                        max.min(avail.max(min))
                    } else {
                        avail
                    }
                }
            },
        };
        if !matches!(sizing, Sizing::Exact(_)) {
            if let Some(max) = resolve(st.max_width, cb_w) {
                content_w = content_w.min(to_content(max));
            }
            if let Some(min) = resolve(st.min_width, cb_w) {
                content_w = content_w.max(to_content(min));
            }
        }
        content_w = content_w.max(0.0);

        // Auto margins center a block with a definite width.
        if matches!(sizing, Sizing::Fill) && st.float == Float::None {
            let free = cb_w - content_w - extra_w - m[1] - m[3];
            let (auto_l, auto_r) = (st.margin[3] == Length::Auto, st.margin[1] == Length::Auto);
            let legacy_center = doc
                .parent_element(node)
                .is_some_and(|p| self.st(p).center_blocks);
            if free > 0.0 {
                if (auto_l && auto_r) || (legacy_center && !auto_l && !auto_r) {
                    m[3] += free / 2.0;
                    m[1] += free / 2.0;
                } else if auto_l {
                    m[3] += free;
                }
            }
        }

        let bx = x + m[3];
        let by = y + m[0];
        let cx = bx + b[3] + p[3];
        let cy = by + b[0] + p[0];
        let start = self.mark();

        let invisible = !st.visible || st.opacity < 0.01;
        if invisible {
            self.hidden += 1;
        }
        let mut decorations = Vec::new();
        if st.background.a > 0 {
            decorations.push(self.emit(Item::Fill {
                rect: Rect::default(),
                color: st.background,
                radius: st.border_radius,
            }));
        }
        if b.iter().any(|&w| w > 0.0) {
            let colors = std::array::from_fn(|i| st.border_color(i));
            decorations.push(self.emit(Item::Border {
                rect: Rect::default(),
                widths: b,
                colors,
                radius: st.border_radius,
            }));
        }
        let clips = st.overflow_hidden && !matches!(tag, "html" | "body");
        let clip_index = clips.then(|| self.emit(Item::PushClip(Rect::default())));

        let positioned = st.position != Position::Static;
        if positioned {
            self.abs_stack.push(Vec::new());
        }
        let saved_link = self.link.clone();
        if tag == "a"
            && let Some(href) = doc.element(node).and_then(|e| e.attr("href"))
        {
            self.link = Some(href.into());
        }
        let saved_baseline = self.first_baseline.take();
        let saved_bgs = std::mem::take(&mut self.inline_bgs);

        let content_h = if let Some((nw, nh)) = natural {
            // Images keep their aspect ratio; form controls keep their height.
            let is_image = tag == "img"
                || doc
                    .element(node)
                    .and_then(|e| e.attr("type"))
                    .is_some_and(|t| t.eq_ignore_ascii_case("image"));
            let h = if is_image && nw > 0.0 {
                content_w * nh / nw
            } else {
                nh
            };
            self.replaced_content(
                node,
                cx,
                cy,
                content_w,
                h,
                resolve(st.height, 0.0)
                    .map(|h| to_content_h(st, h, extra_h))
                    .unwrap_or(h),
            )
        } else {
            match st.display {
                Display::Flex | Display::InlineFlex => {
                    let definite_h = resolve(st.height, 0.0)
                        .filter(|_| !matches!(st.height, Length::Percent(_)))
                        .map(|h| to_content_h(st, h, extra_h));
                    self.flex(
                        node,
                        cx,
                        cy,
                        content_w,
                        definite_h.or(forced_h.map(|h| h - extra_h)),
                    )
                }
                Display::Table => self.table(node, cx, cy, content_w),
                _ => self.flow(node, cx, cy, content_w),
            }
        };

        let mut h = match (forced_h, st.height) {
            (Some(fh), _) => fh - extra_h,
            (None, Length::Px(px)) => to_content_h(st, px, extra_h),
            (None, Length::Percent(pc)) if matches!(tag, "html" | "body") => {
                to_content_h(st, self.input.viewport_height * pc / 100.0, extra_h)
            }
            _ => content_h,
        };
        if let Some(max) =
            resolve(st.max_height, 0.0).filter(|_| !matches!(st.max_height, Length::Percent(_)))
        {
            h = h.min(to_content_h(st, max, extra_h));
        }
        if let Some(min) =
            resolve(st.min_height, 0.0).filter(|_| !matches!(st.min_height, Length::Percent(_)))
        {
            h = h.max(to_content_h(st, min, extra_h));
        }
        h = h.max(0.0);
        let border_box = Rect {
            x: bx,
            y: by,
            w: content_w + extra_w,
            h: h + extra_h,
        };

        for &i in &decorations {
            if let Some(r) = self.out.items[i].rect_mut() {
                *r = border_box;
            }
        }
        if let Some(i) = clip_index {
            if let Some(r) = self.out.items[i].rect_mut() {
                *r = border_box;
            }
            self.emit(Item::PopClip);
        }

        if st.display == Display::ListItem && st.list_style != ListStyle::None {
            self.list_marker(node, cx, cy);
        }
        if positioned {
            let pending = self.abs_stack.pop().unwrap_or_default();
            let padding_box = Rect {
                x: bx + b[3],
                y: by + b[0],
                w: content_w + p[1] + p[3],
                h: h + p[0] + p[2],
            };
            self.place_absolutes(pending, padding_box);
        }
        if let (Some(link), Some(_)) = (&self.link, doc.element(node))
            && (tag == "a" || saved_link.is_some())
        {
            self.out.links.push((border_box, link.clone()));
        }
        if matches!(tag, "button" | "summary") {
            self.out.controls.push(Control {
                rect: border_box,
                node,
                text_x: cx,
                baseline: cy,
                font: font_key(st),
                size: st.font_size,
            });
        }
        if let Some(el) = doc.element(node)
            && (el.id().is_some() || (tag == "a" && el.attr("name").is_some()))
        {
            self.out.anchors.push((node, by));
        }

        self.link = saved_link;
        let baseline = self.first_baseline;
        self.first_baseline = saved_baseline.or(baseline);
        self.inline_bgs = saved_bgs;
        if invisible {
            self.hidden -= 1;
        }

        if st.position == Position::Relative || st.position == Position::Sticky {
            let dx = resolve(st.inset[3], cb_w)
                .or(resolve(st.inset[1], cb_w).map(|r| -r))
                .unwrap_or(0.0);
            let dy = resolve(st.inset[0], 0.0)
                .or(resolve(st.inset[2], 0.0).map(|b| -b))
                .unwrap_or(0.0);
            if st.position == Position::Relative {
                self.translate_since(start, dx, dy);
            }
        }

        self.out.width = self.out.width.max(border_box.x + border_box.w);
        self.out.height = self.out.height.max(border_box.y + border_box.h);
        BoxSize {
            w: content_w + extra_w + m[1] + m[3],
            h: h + extra_h,
            mt: m[0],
            mb: m[2],
            decorations,
        }
    }

    fn place_absolutes(&mut self, pending: Vec<Pending>, cb: Rect) {
        for Pending { node, x, y } in pending {
            let st = self.st(node);
            let m = self.margins(st, cb.w);
            let left = resolve(st.inset[3], cb.w);
            let right = resolve(st.inset[1], cb.w);
            let top = resolve(st.inset[0], cb.h);
            let bottom = resolve(st.inset[2], cb.h);
            let sizing = match (left, right, resolve(st.width, cb.w)) {
                (Some(l), Some(r), None) => Sizing::Exact((cb.w - l - r - m[1] - m[3]).max(0.0)),
                _ => Sizing::Shrink,
            };
            let start = self.mark();
            let size = self.block(node, 0.0, 0.0, cb.w, sizing, None);
            let bx = match (left, right) {
                (Some(l), _) => cb.x + l,
                (None, Some(r)) => cb.x + cb.w - r - size.w,
                (None, None) => x,
            };
            let by = match (top, bottom) {
                (Some(t), _) => cb.y + t,
                (None, Some(b)) => cb.y + cb.h - b - size.margin_h(),
                (None, None) => y,
            };
            self.translate_since(start, bx, by);
            self.out.width = self.out.width.max(bx + size.w);
            self.out.height = self.out.height.max(by + size.margin_h());
        }
    }

    /// The number of list item `node`: its `value`, or one more than the
    /// previous `<li>`, starting from the list's `start`. Numbers the whole
    /// list in one pass, since counting the previous siblings for every item
    /// is quadratic.
    fn ordinal(&mut self, node: NodeId) -> i32 {
        if let Some(&n) = self.ordinals.get(&node) {
            return n;
        }
        let doc = self.doc();
        let number = |n: NodeId, attr: &str| -> Option<i32> {
            doc.element(n)?.attr(attr)?.trim().parse().ok()
        };
        let Some(list) = doc.parent_element(node) else {
            return number(node, "value").unwrap_or(1);
        };
        let mut next = number(list, "start").unwrap_or(1);
        for &child in &doc.nodes[list].children {
            if doc.element(child).is_none() {
                continue;
            }
            let n = number(child, "value").unwrap_or(next);
            self.ordinals.insert(child, n);
            if doc.tag(child) == "li" {
                next = n.saturating_add(1);
            }
        }
        self.ordinals.get(&node).copied().unwrap_or(next)
    }

    fn list_marker(&mut self, node: NodeId, cx: f32, cy: f32) {
        let st = self.st(node);
        let mut ordinal = || self.ordinal(node);
        let text = match st.list_style {
            ListStyle::None => return,
            ListStyle::Disc => "•".to_string(),
            ListStyle::Circle => "◦".to_string(),
            ListStyle::Square => "▪".to_string(),
            ListStyle::Decimal => format!("{}.", ordinal()),
            ListStyle::LowerAlpha => format!("{}.", alpha(ordinal(), false)),
            ListStyle::UpperAlpha => format!("{}.", alpha(ordinal(), true)),
            ListStyle::LowerRoman => format!("{}.", roman(ordinal()).to_lowercase()),
            ListStyle::UpperRoman => format!("{}.", roman(ordinal())),
        };
        let key = font_key(st);
        let w = self.fonts().measure(key, &text, st.font_size);
        let baseline = self.first_baseline.unwrap_or_else(|| {
            let (asc, desc) = self.fonts().vertical_metrics(key, st.font_size);
            cy + (st.line_height_px() - asc - desc) / 2.0 + asc
        });
        self.emit(Item::Text {
            x: cx - w - st.font_size * 0.5,
            y: baseline,
            text,
            font: key,
            size: st.font_size,
            color: st.color,
        });
    }

    /// Natural content size of a replaced element, before CSS sizing.
    fn natural_size(&self, node: NodeId) -> Option<(f32, f32)> {
        let doc = self.doc();
        let el = doc.element(node)?;
        let st = self.st(node);
        let zoom = self.input.zoom;
        let fonts = self.fonts();
        let lh = st.line_height_px();
        let key = font_key(st);
        let input_type = el.attr("type").unwrap_or("text").to_ascii_lowercase();
        match el.tag.as_str() {
            "img" => Some(self.image_size(node)),
            "input" if input_type == "image" => Some(self.image_size(node)),
            "input" => Some(match input_type.as_str() {
                "checkbox" | "radio" => (13.0 * zoom, 13.0 * zoom),
                "submit" | "button" | "reset" | "file" => {
                    let label = button_label(el.attr("value"), &input_type);
                    (fonts.measure(key, label, st.font_size), lh)
                }
                "range" => (129.0 * zoom, 16.0 * zoom),
                "color" => (50.0 * zoom, 27.0 * zoom),
                _ => (st.font_size * 11.0, lh),
            }),
            "select" => {
                let widest = doc
                    .descendants(node)
                    .into_iter()
                    .filter(|&n| doc.tag(n) == "option")
                    .map(|n| fonts.measure(key, doc.text_content(n).trim(), st.font_size))
                    .fold(0.0, f32::max);
                Some((widest + 20.0 * zoom, lh))
            }
            "textarea" => {
                // `inf` and `1e30` parse as f32 too.
                let count = |name: &str, default: f32| -> f32 {
                    el.attr(name)
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .filter(|v| v.is_finite() && *v >= 1.0)
                        .map_or(default, |v| v.min(1000.0))
                };
                let (cols, rows) = (count("cols", 20.0), count("rows", 2.0));
                Some((cols * st.font_size * 0.55, rows * lh))
            }
            _ => None,
        }
    }

    fn image_size(&self, node: NodeId) -> (f32, f32) {
        let zoom = self.input.zoom;
        let el = self.doc().element(node);
        let image = el
            .and_then(|e| e.image_source())
            .and_then(|src| self.input.images.get(src));
        let st = self.st(node);
        match image {
            Some(img) => {
                let (w, h) = (img.width as f32 * zoom, img.height as f32 * zoom);
                // Honour one specified dimension and keep the aspect ratio.
                match (
                    resolve(st.width, 0.0).filter(|_| !matches!(st.width, Length::Percent(_))),
                    st.height,
                ) {
                    (Some(sw), Length::Auto) if w > 0.0 => (sw, sw * h / w),
                    _ => (w, h),
                }
            }
            None => (0.0, 0.0),
        }
    }

    /// Draws a replaced element into its content box and returns its height.
    fn replaced_content(
        &mut self,
        node: NodeId,
        cx: f32,
        cy: f32,
        w: f32,
        natural_h: f32,
        h: f32,
    ) -> f32 {
        let doc = self.doc();
        let Some(el) = doc.element(node) else {
            return natural_h;
        };
        let st = self.st(node);
        let key = font_key(st);
        let input_type = el.attr("type").unwrap_or("text").to_ascii_lowercase();
        let rect = Rect { x: cx, y: cy, w, h };
        let is_image = el.tag == "img" || (el.tag == "input" && input_type == "image");
        if is_image {
            if let Some(src) = el.image_source()
                && self.input.images.contains_key(src)
            {
                self.emit(Item::Image {
                    rect,
                    src: src.into(),
                });
            }
            if el.tag == "input" {
                self.push_control(node, rect, cx, cy, key, st.font_size);
            }
            return h;
        }

        let (asc, desc) = self.fonts().vertical_metrics(key, st.font_size);
        let baseline = cy + (h.min(st.line_height_px()) - asc - desc) / 2.0 + asc;
        let form = self.input.form;
        let gray = Color::rgb(0x75, 0x75, 0x75);
        let (text, color) = match (el.tag.as_str(), input_type.as_str()) {
            ("input", "checkbox") => {
                let checked = form.is_checked(doc, node);
                (
                    if checked {
                        "✓".to_string()
                    } else {
                        String::new()
                    },
                    st.color,
                )
            }
            ("input", "radio") => {
                let checked = form.is_checked(doc, node);
                (
                    if checked {
                        "●".to_string()
                    } else {
                        String::new()
                    },
                    st.color,
                )
            }
            ("input", "submit" | "button" | "reset") => (
                button_label(el.attr("value"), &input_type).to_string(),
                st.color,
            ),
            ("input", "file") => ("Choose file".to_string(), st.color),
            ("select", _) => {
                let options: Vec<NodeId> = doc
                    .descendants(node)
                    .into_iter()
                    .filter(|&n| doc.tag(n) == "option")
                    .collect();
                let selected = form
                    .values
                    .get(&node)
                    .cloned()
                    .or_else(|| {
                        options
                            .iter()
                            .find(|&&o| {
                                doc.element(o).is_some_and(|e| e.attr("selected").is_some())
                            })
                            .or(options.first())
                            .map(|&o| doc.text_content(o).trim().to_string())
                    })
                    .unwrap_or_default();
                (selected, st.color)
            }
            _ => {
                let value = form.value(doc, node);
                if value.is_empty() {
                    (el.attr("placeholder").unwrap_or("").to_string(), gray)
                } else if input_type == "password" {
                    ("•".repeat(value.chars().count()), st.color)
                } else {
                    (value.replace('\n', " "), st.color)
                }
            }
        };
        let centered = matches!(
            input_type.as_str(),
            "submit" | "button" | "reset" | "checkbox" | "radio"
        ) && el.tag == "input";
        let text_w = self.fonts().measure(key, &text, st.font_size);
        let text_x = if centered {
            cx + (w - text_w) / 2.0
        } else {
            cx
        };
        self.emit(Item::PushClip(rect));
        if !text.is_empty() {
            self.emit(Item::Text {
                x: text_x,
                y: baseline,
                text,
                font: key,
                size: st.font_size,
                color,
            });
        }
        if el.tag == "select" {
            let arrow_size = st.font_size * 0.8;
            let aw = self.fonts().measure(key, "▾", arrow_size);
            self.emit(Item::Text {
                x: cx + w - aw,
                y: baseline,
                text: "▾".into(),
                font: key,
                size: arrow_size,
                color: st.color,
            });
        }
        self.emit(Item::PopClip);
        self.push_control(node, rect, text_x, baseline, key, st.font_size);
        h
    }

    fn push_control(
        &mut self,
        node: NodeId,
        rect: Rect,
        text_x: f32,
        baseline: f32,
        font: FontKey,
        size: f32,
    ) {
        let st = self.st(node);
        let p = self.paddings(st, rect.w);
        let b = st.border_width;
        let outer = Rect {
            x: rect.x - p[3] - b[3],
            y: rect.y - p[0] - b[0],
            w: rect.w + p[1] + p[3] + b[1] + b[3],
            h: rect.h + p[0] + p[2] + b[0] + b[2],
        };
        self.out.controls.push(Control {
            rect: outer,
            node,
            text_x,
            baseline,
            font,
            size,
        });
    }

    /// Normal flow: block children stacked vertically, runs of inline
    /// content broken into lines. Returns the content height.
    fn flow(&mut self, node: NodeId, cx: f32, cy: f32, w: f32) -> f32 {
        let mut state = FlowState {
            cursor: cy,
            prev_mb: 0.0,
            inline: Vec::new(),
        };
        for &child in &self.doc().nodes[node].children {
            self.flow_child(node, child, &mut state, cx, w);
        }
        self.flush_inline(node, &mut state, cx, w);
        state.cursor + state.prev_mb - cy
    }

    fn flow_child(
        &mut self,
        container: NodeId,
        child: NodeId,
        state: &mut FlowState,
        cx: f32,
        w: f32,
    ) {
        let doc = self.doc();
        let el = match &doc.nodes[child].kind {
            NodeKind::Text(text) => {
                state.inline.push(InlineItem::Text {
                    node: child,
                    text: text.clone(),
                    link: self.link.clone(),
                    bg: self.inline_bgs.last().cloned(),
                });
                return;
            }
            NodeKind::Element(el) => el,
            NodeKind::Document => return,
        };
        let st = self.st(child);
        let tag = doc.tag(child);
        match st.display {
            Display::None => return,
            Display::Contents => {
                for &c in &doc.nodes[child].children {
                    self.flow_child(container, c, state, cx, w);
                }
                return;
            }
            _ => {}
        }
        if st.is_out_of_flow() {
            let y = state.cursor + state.prev_mb;
            let frame = if st.position == Position::Fixed {
                0
            } else {
                self.abs_stack.len() - 1
            };
            self.abs_stack[frame].push(Pending {
                node: child,
                x: cx,
                y,
            });
            return;
        }
        if tag == "br" {
            state.inline.push(InlineItem::Break);
            return;
        }
        if st.float != Float::None
            || matches!(st.display, Display::InlineBlock | Display::InlineFlex)
            || is_replaced(tag)
            || tag == "button"
        {
            state.inline.push(InlineItem::Atomic {
                node: child,
                link: self.link.clone(),
            });
            return;
        }
        if st.display.is_block_level() {
            self.flush_inline(container, state, cx, w);
            let m = self.margins(st, w);
            let collapsed = collapse(state.prev_mb, m[0]);
            let top = state.cursor + collapsed - m[0];
            let size = self.block(child, cx, top, w, Sizing::Fill, None);
            state.cursor = top + size.mt + size.h;
            state.prev_mb = size.mb;
            return;
        }

        // An inline element: walk its children in the same inline run.
        if el.id().is_some() || (tag == "a" && el.attr("name").is_some()) {
            self.out.anchors.push((child, state.cursor + state.prev_mb));
        }
        let p = self.paddings(st, w);
        let m = self.margins(st, w);
        let b = st.border_width;
        let lead = p[3] + m[3] + b[3];
        let trail = p[1] + m[1] + b[1];
        if lead > 0.0 {
            state.inline.push(InlineItem::Spacer(lead));
        }
        let saved_link = self.link.clone();
        if tag == "a"
            && let Some(href) = el.attr("href")
        {
            self.link = Some(href.into());
        }
        let pushed_bg = st.background.a > 0;
        if pushed_bg {
            self.inline_bgs.push(InlineBg {
                color: st.background,
                pad_top: p[0] + b[0],
                pad_bottom: p[2] + b[2],
                radius: st.border_radius,
            });
        }
        let invisible = !st.visible || st.opacity < 0.01;
        if invisible {
            // Hidden inline content still takes space; drop it from paint by
            // wrapping it in an atomic-free run is complex, so skip it.
            self.link = saved_link;
            if pushed_bg {
                self.inline_bgs.pop();
            }
            return;
        }
        for &c in &doc.nodes[child].children {
            self.flow_child(container, c, state, cx, w);
        }
        if pushed_bg {
            self.inline_bgs.pop();
        }
        self.link = saved_link;
        if trail > 0.0 {
            state.inline.push(InlineItem::Spacer(trail));
        }
    }

    fn flush_inline(&mut self, container: NodeId, state: &mut FlowState, cx: f32, w: f32) {
        if state.inline.is_empty() {
            return;
        }
        let items = std::mem::take(&mut state.inline);
        let has_content = items.iter().any(|i| match i {
            InlineItem::Text { text, node, .. } => {
                !self.st(*node).white_space.collapses_spaces()
                    || text.chars().any(|c| !c.is_ascii_whitespace())
            }
            InlineItem::Atomic { .. } | InlineItem::Break => true,
            InlineItem::Spacer(_) => false,
        });
        if !has_content {
            return;
        }
        state.cursor += state.prev_mb;
        state.prev_mb = 0.0;
        let h = self.lines(container, items, cx, state.cursor, w);
        state.cursor += h;
    }

    /// Breaks inline content into line boxes. Returns their total height.
    fn lines(
        &mut self,
        container: NodeId,
        items: Vec<InlineItem>,
        cx: f32,
        cy: f32,
        w: f32,
    ) -> f32 {
        let fonts = self.fonts();
        let cst = self.st(container);

        // Lay out atomic boxes first so their sizes are known.
        let mut atomics: HashMap<usize, AtomicBox> = HashMap::new();
        for (i, item) in items.iter().enumerate() {
            if let InlineItem::Atomic { node, link } = item {
                let saved = std::mem::replace(&mut self.link, link.clone());
                let start = self.mark();
                let size = self.block(*node, 0.0, 0.0, w, Sizing::Shrink, None);
                let end = self.mark();
                self.link = saved;
                atomics.insert(
                    i,
                    AtomicBox {
                        size,
                        start,
                        end,
                        float: self.st(*node).float,
                    },
                );
            }
        }

        // Split into words and spaces.
        let mut pieces: Vec<Piece> = Vec::new();
        let mut after_space = true;
        for (i, item) in items.iter().enumerate() {
            match item {
                InlineItem::Text { node, text, .. } => {
                    let st = self.st(*node);
                    let key = font_key(st);
                    let ws = st.white_space;
                    let space_w = fonts.advance(key, ' ', st.font_size);
                    let mut word = String::new();
                    let flush_word = |word: &mut String, pieces: &mut Vec<Piece>| {
                        if !word.is_empty() {
                            let text = transform(std::mem::take(word), st.text_transform);
                            let width = fonts.measure(key, &text, st.font_size);
                            pieces.push(Piece {
                                kind: PieceKind::Word(text),
                                width,
                                item: i,
                            });
                        }
                    };
                    for c in text.chars() {
                        let is_space = matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c');
                        if !is_space {
                            word.push(c);
                            after_space = false;
                            continue;
                        }
                        flush_word(&mut word, &mut pieces);
                        if c == '\n' && ws.keeps_newlines() {
                            pieces.push(Piece {
                                kind: PieceKind::Break,
                                width: 0.0,
                                item: i,
                            });
                            after_space = true;
                        } else if ws.collapses_spaces() {
                            if !after_space {
                                pieces.push(Piece {
                                    kind: PieceKind::Space,
                                    width: space_w,
                                    item: i,
                                });
                                after_space = true;
                            }
                        } else if c != '\r' {
                            let width = if c == '\t' { space_w * 8.0 } else { space_w };
                            pieces.push(Piece {
                                kind: PieceKind::Space,
                                width,
                                item: i,
                            });
                        }
                    }
                    flush_word(&mut word, &mut pieces);
                }
                InlineItem::Atomic { .. } => {
                    let width = atomics.get(&i).map_or(0.0, |a| a.size.w);
                    pieces.push(Piece {
                        kind: PieceKind::Atomic,
                        width,
                        item: i,
                    });
                    after_space = false;
                }
                InlineItem::Break => {
                    pieces.push(Piece {
                        kind: PieceKind::Break,
                        width: 0.0,
                        item: i,
                    });
                    after_space = true;
                }
                InlineItem::Spacer(width) => {
                    pieces.push(Piece {
                        kind: PieceKind::Spacer,
                        width: *width,
                        item: i,
                    });
                }
            }
        }

        // Break into lines, placing each as soon as it is complete so the
        // available width can account for floats placed so far.
        let indent = resolve0(cst.text_indent, w);
        let styles = self.input.styles;
        let item_wraps = |item: usize| match &items[item] {
            InlineItem::Text { node, .. } => styles[*node].white_space.wraps(),
            InlineItem::Atomic { node, .. } => {
                styles[*node].white_space.wraps() || cst.white_space.wraps()
            }
            _ => true,
        };
        let mut ls = LineState {
            y: cy,
            floats: Vec::new(),
            line: Vec::new(),
            line_w: 0.0,
            avail: 0.0,
            first: true,
        };
        ls.reset(w, indent);
        let mut pieces = pieces.into_iter();
        let mut pending: Option<Piece> = None;
        while let Some(piece) = pending.take().or_else(|| pieces.next()) {
            if let (PieceKind::Atomic, Some(a)) = (&piece.kind, atomics.get(&piece.item))
                && a.float != Float::None
            {
                let (l, r) = ls.bounds();
                let right = a.float == Float::Right;
                let fx = if right { cx + w - r - a.size.w } else { cx + l };
                let (start, end, fw, fh) = (a.start, a.end, a.size.w, a.size.margin_h());
                self.translate(start, end, fx, ls.y);
                if let InlineItem::Atomic {
                    link: Some(link), ..
                } = &items[piece.item]
                {
                    self.out.links.push((
                        Rect {
                            x: fx,
                            y: ls.y,
                            w: fw,
                            h: fh,
                        },
                        link.clone(),
                    ));
                }
                ls.floats.push((right, fw, ls.y + fh));
                ls.avail -= fw;
                continue;
            }
            match piece.kind {
                PieceKind::Break => {
                    self.finish_line(&mut ls, &items, &atomics, cst, cx, w, indent);
                    continue;
                }
                PieceKind::Space => {
                    let collapses = match &items[piece.item] {
                        InlineItem::Text { node, .. } => {
                            self.st(*node).white_space.collapses_spaces()
                        }
                        _ => true,
                    };
                    if ls.line.is_empty() && collapses {
                        continue;
                    }
                    ls.line_w += piece.width;
                    ls.line.push(piece);
                    continue;
                }
                _ => {}
            }
            let has_content = ls
                .line
                .iter()
                .any(|p| !matches!(p.kind, PieceKind::Space | PieceKind::Spacer));
            let wraps = item_wraps(piece.item);
            if ls.line_w + piece.width > ls.avail + 0.01 && wraps {
                if has_content {
                    self.finish_line(&mut ls, &items, &atomics, cst, cx, w, indent);
                    pending = Some(piece);
                    continue;
                }
                // Nothing fits beside the floats: move below the nearest one.
                if let Some(bottom) = ls.next_float_bottom() {
                    ls.line.clear();
                    ls.y = bottom;
                    ls.reset(w, indent);
                    pending = Some(piece);
                    continue;
                }
            }
            // Break words wider than a whole line, a line's worth at a time.
            // Cutting by offset copies each character once; handing the
            // rest back as a new piece would copy it again for every line.
            if let (PieceKind::Word(text), true) = (
                &piece.kind,
                piece.width > ls.avail - ls.line_w + 0.01 && wraps,
            ) {
                let InlineItem::Text { node, .. } = &items[piece.item] else {
                    unreachable!()
                };
                let st = self.st(*node);
                let key = font_key(st);
                let mut start = 0;
                let mut rest_w = piece.width;
                loop {
                    let room = ls.avail - ls.line_w;
                    let mut split = start;
                    let mut used = 0.0;
                    for (i, c) in text[start..].char_indices() {
                        let cw = fonts.advance(key, c, st.font_size);
                        if used + cw > room && i > 0 {
                            break;
                        }
                        used += cw;
                        split = start + i + c.len_utf8();
                    }
                    ls.line.push(Piece {
                        kind: PieceKind::Word(text[start..split].to_string()),
                        width: used,
                        item: piece.item,
                    });
                    ls.line_w += used;
                    rest_w -= used;
                    start = split;
                    if start == text.len() {
                        break;
                    }
                    self.finish_line(&mut ls, &items, &atomics, cst, cx, w, indent);
                    // The rest fits, or floats are in the way: the main loop
                    // places it (and moves below the floats if need be).
                    if rest_w <= ls.avail - ls.line_w + 0.01 || ls.next_float_bottom().is_some() {
                        pending = Some(Piece {
                            kind: PieceKind::Word(text[start..].to_string()),
                            width: rest_w,
                            item: piece.item,
                        });
                        break;
                    }
                }
                continue;
            }
            ls.line_w += piece.width;
            ls.line.push(piece);
        }
        if !ls.line.is_empty() {
            self.finish_line(&mut ls, &items, &atomics, cst, cx, w, indent);
        }
        let floats_bottom = ls.floats.iter().map(|f| f.2).fold(ls.y, f32::max);
        floats_bottom - cy
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_line(
        &mut self,
        ls: &mut LineState,
        items: &[InlineItem],
        atomics: &HashMap<usize, AtomicBox>,
        cst: &Style,
        cx: f32,
        w: f32,
        indent: f32,
    ) {
        trim_trailing_spaces(&mut ls.line);
        let line = std::mem::take(&mut ls.line);
        let (l, r) = ls.bounds();
        let ind = if ls.first { indent } else { 0.0 };
        let h = self.place_line(
            items,
            atomics,
            &line,
            cst,
            cx + l + ind,
            w - l - r - ind,
            ls.y,
        );
        ls.y += h;
        ls.first = false;
        ls.reset(w, indent);
    }

    /// Positions one line's pieces and returns the line height.
    #[allow(clippy::too_many_arguments)]
    fn place_line(
        &mut self,
        items: &[InlineItem],
        atomics: &HashMap<usize, AtomicBox>,
        line: &[Piece],
        cst: &Style,
        x0: f32,
        width: f32,
        y: f32,
    ) -> f32 {
        let fonts = self.fonts();
        let (casc, cdesc) = fonts.vertical_metrics(font_key(cst), cst.font_size);
        let clh = cst.line_height_px();
        let strut_above = (clh - casc - cdesc) / 2.0 + casc;
        let mut above = strut_above;
        let mut below = clh - strut_above;
        for piece in line {
            match &items[piece.item] {
                InlineItem::Text { node, .. } => {
                    let st = self.st(*node);
                    let (asc, desc) = fonts.vertical_metrics(font_key(st), st.font_size);
                    let half = (st.line_height_px() - asc - desc) / 2.0;
                    above = above.max(asc + half);
                    below = below.max(desc + half);
                }
                InlineItem::Atomic { node, .. } => {
                    if let Some(a) = atomics.get(&piece.item) {
                        let h = a.size.margin_h();
                        if self.st(*node).vertical_align == VAlign::Middle {
                            let half_x = cst.font_size * 0.25;
                            above = above.max(h / 2.0 + half_x);
                            below = below.max(h / 2.0 - half_x);
                        } else {
                            above = above.max(h);
                        }
                    }
                }
                _ => {}
            }
        }
        let baseline = y + above;
        let line_h = above + below;
        if self.first_baseline.is_none() && !line.is_empty() {
            self.first_baseline = Some(baseline);
        }

        let content_w: f32 = line.iter().map(|p| p.width).sum();
        let free = (width - content_w).max(0.0);
        let mut x = x0
            + match cst.text_align {
                TextAlign::Left => 0.0,
                TextAlign::Center => free / 2.0,
                TextAlign::Right => free,
            };
        let mut run: Option<(usize, f32, String, f32)> = None;
        for piece in line {
            let continues = run.as_ref().is_some_and(|(item, ..)| *item == piece.item);
            if !continues && let Some(r) = run.take() {
                self.emit_run(items, r, baseline, y, line_h);
            }
            match &piece.kind {
                PieceKind::Word(text) => {
                    let r = run.get_or_insert_with(|| (piece.item, x, String::new(), 0.0));
                    r.2.push_str(text);
                    r.3 += piece.width;
                    x += piece.width;
                }
                PieceKind::Space => {
                    let r = run.get_or_insert_with(|| (piece.item, x, String::new(), 0.0));
                    r.2.push(' ');
                    r.3 += piece.width;
                    x += piece.width;
                }
                PieceKind::Spacer => x += piece.width,
                PieceKind::Atomic => {
                    let Some(a) = atomics.get(&piece.item) else {
                        continue;
                    };
                    let InlineItem::Atomic { node, link } = &items[piece.item] else {
                        continue;
                    };
                    let h = a.size.margin_h();
                    let top = if self.st(*node).vertical_align == VAlign::Middle {
                        baseline - cst.font_size * 0.25 - h / 2.0
                    } else {
                        baseline - h
                    };
                    self.translate(a.start, a.end, x, top);
                    if let Some(link) = link {
                        self.out.links.push((
                            Rect {
                                x,
                                y: top,
                                w: a.size.w,
                                h,
                            },
                            link.clone(),
                        ));
                    }
                    x += piece.width;
                }
                PieceKind::Break => {}
            }
        }
        if let Some(r) = run.take() {
            self.emit_run(items, r, baseline, y, line_h);
        }
        line_h
    }

    fn emit_run(
        &mut self,
        items: &[InlineItem],
        run: (usize, f32, String, f32),
        baseline: f32,
        line_top: f32,
        line_h: f32,
    ) {
        let (item, x, text, width) = run;
        let InlineItem::Text { node, link, bg, .. } = &items[item] else {
            return;
        };
        let st = self.st(*node);
        let key = font_key(st);
        let (asc, desc) = self.fonts().vertical_metrics(key, st.font_size);
        if let Some(bg) = bg {
            let rect = Rect {
                x,
                y: baseline - asc - bg.pad_top,
                w: width,
                h: asc + desc + bg.pad_top + bg.pad_bottom,
            };
            self.emit(Item::Fill {
                rect,
                color: bg.color,
                radius: bg.radius,
            });
        }
        let thickness = (st.font_size / 14.0).max(1.0).round();
        if st.underline {
            let rect = Rect {
                x,
                y: (baseline + st.font_size * 0.1).round(),
                w: width,
                h: thickness,
            };
            self.emit(Item::Fill {
                rect,
                color: st.color,
                radius: 0.0,
            });
        }
        if st.line_through {
            let rect = Rect {
                x,
                y: (baseline - asc * 0.3).round(),
                w: width,
                h: thickness,
            };
            self.emit(Item::Fill {
                rect,
                color: st.color,
                radius: 0.0,
            });
        }
        if let Some(link) = link {
            self.out.links.push((
                Rect {
                    x,
                    y: line_top,
                    w: width,
                    h: line_h,
                },
                link.clone(),
            ));
        }
        if !text.trim().is_empty() {
            self.emit(Item::Text {
                x,
                y: baseline,
                text,
                font: key,
                size: st.font_size,
                color: st.color,
            });
        }
    }

    /// Flexbox, single-pass: no baseline alignment and no multi-line
    /// `align-content`.
    fn flex(&mut self, node: NodeId, cx: f32, cy: f32, w: f32, definite_h: Option<f32>) -> f32 {
        let doc = self.doc();
        let st = self.st(node);
        let mut children: Vec<NodeId> = Vec::new();
        let mut contents = doc.nodes[node].children.clone();
        contents.reverse();
        while let Some(c) = contents.pop() {
            match &doc.nodes[c].kind {
                NodeKind::Text(t) if !t.trim().is_empty() => children.push(c),
                NodeKind::Element(_) => {
                    let cs = self.st(c);
                    if cs.display == Display::Contents {
                        contents.extend(doc.nodes[c].children.iter().rev());
                    } else if cs.display == Display::None {
                    } else if cs.is_out_of_flow() {
                        let frame = if cs.position == Position::Fixed {
                            0
                        } else {
                            self.abs_stack.len() - 1
                        };
                        self.abs_stack[frame].push(Pending {
                            node: c,
                            x: cx,
                            y: cy,
                        });
                    } else {
                        children.push(c);
                    }
                }
                _ => {}
            }
        }
        children.sort_by_key(|&c| {
            if doc.element(c).is_some() {
                self.st(c).order
            } else {
                0
            }
        });
        if st.flex_reverse {
            children.reverse();
        }
        let col_gap = resolve0(st.column_gap, w);
        let row_gap = resolve0(st.row_gap, w);

        if st.flex_column {
            return self.flex_column(node, &children, cx, cy, w, definite_h, row_gap);
        }

        // Hypothetical main sizes (margin boxes).
        struct FlexItem {
            node: NodeId,
            base: f32,
            min: f32,
            grow: f32,
            shrink: f32,
            margins: [f32; 4],
            auto_left: bool,
            auto_right: bool,
            size: f32,
        }
        let mut flex_items: Vec<FlexItem> = children
            .iter()
            .map(|&c| {
                let (min, max) = self.intrinsic(c);
                if doc.element(c).is_none() {
                    return FlexItem {
                        node: c,
                        base: max,
                        min,
                        grow: 0.0,
                        shrink: 1.0,
                        margins: [0.0; 4],
                        auto_left: false,
                        auto_right: false,
                        size: max,
                    };
                }
                let cs = self.st(c);
                let m = self.margins(cs, w);
                let p = self.paddings(cs, w);
                let extra = p[1] + p[3] + cs.border_width[1] + cs.border_width[3] + m[1] + m[3];
                let to_outer = |v: f32| {
                    if cs.border_box {
                        v + m[1] + m[3]
                    } else {
                        v + extra
                    }
                };
                let base = resolve(cs.flex_basis, w)
                    .map(to_outer)
                    .or_else(|| resolve(cs.width, w).map(to_outer))
                    .unwrap_or(max);
                let mut base = base;
                if let Some(maxw) = resolve(cs.max_width, w) {
                    base = base.min(to_outer(maxw));
                }
                let min_size = match resolve(cs.min_width, w) {
                    Some(mw) => to_outer(mw),
                    None if cs.overflow_hidden => extra,
                    None => min.min(base),
                };
                FlexItem {
                    node: c,
                    base: base.max(min_size),
                    min: min_size,
                    grow: cs.flex_grow,
                    shrink: cs.flex_shrink,
                    margins: m,
                    auto_left: cs.margin[3] == Length::Auto,
                    auto_right: cs.margin[1] == Length::Auto,
                    size: 0.0,
                }
            })
            .collect();

        // Break into lines.
        let mut lines: Vec<std::ops::Range<usize>> = Vec::new();
        let mut start = 0;
        let mut used = 0.0;
        for (i, item) in flex_items.iter().enumerate() {
            let gap = if i > start { col_gap } else { 0.0 };
            if st.flex_wrap && i > start && used + gap + item.base > w + 0.01 {
                lines.push(start..i);
                start = i;
                used = item.base;
            } else {
                used += gap + item.base;
            }
        }
        lines.push(start..flex_items.len());

        let mut y = cy;
        for (li, range) in lines.iter().enumerate() {
            if range.is_empty() {
                continue;
            }
            let count = range.len();
            let gaps = col_gap * (count.saturating_sub(1)) as f32;
            let total: f32 = flex_items[range.clone()].iter().map(|i| i.base).sum();
            let mut free = w - total - gaps;
            for item in &mut flex_items[range.clone()] {
                item.size = item.base;
            }
            if free > 0.0 {
                let grow: f32 = flex_items[range.clone()].iter().map(|i| i.grow).sum();
                if grow > 0.0 {
                    for item in &mut flex_items[range.clone()] {
                        item.size += free * item.grow / grow;
                    }
                    free = 0.0;
                }
            } else if free < 0.0 {
                // Shrink, clamping at each item's minimum, a few rounds.
                for _ in 0..3 {
                    let weight: f32 = flex_items[range.clone()]
                        .iter()
                        .filter(|i| i.size > i.min)
                        .map(|i| i.shrink * i.base)
                        .sum();
                    if weight <= 0.0 || free >= -0.01 {
                        break;
                    }
                    let mut taken = 0.0;
                    for item in &mut flex_items[range.clone()] {
                        if item.size > item.min {
                            let cut = -free * item.shrink * item.base / weight;
                            let new = (item.size - cut).max(item.min);
                            taken += item.size - new;
                            item.size = new;
                        }
                    }
                    free += taken;
                }
                free = free.max(0.0);
            }

            // Auto margins take free space before justify-content does.
            let autos: usize = flex_items[range.clone()]
                .iter()
                .map(|i| i.auto_left as usize + i.auto_right as usize)
                .sum();
            let (mut x, spacing, auto_share) = if autos > 0 && free > 0.0 {
                (cx, 0.0, free / autos as f32)
            } else {
                let (offset, spacing) = match st.justify {
                    Justify::Start => (0.0, 0.0),
                    Justify::Center => (free / 2.0, 0.0),
                    Justify::End => (free, 0.0),
                    Justify::SpaceBetween if count > 1 => (0.0, free / (count - 1) as f32),
                    Justify::SpaceBetween => (0.0, 0.0),
                    Justify::SpaceAround => (free / count as f32 / 2.0, free / count as f32),
                    Justify::SpaceEvenly => (free / (count + 1) as f32, free / (count + 1) as f32),
                };
                (cx + offset, spacing, 0.0)
            };

            // Lay out the items, then align them in the line.
            let mut placed: Vec<(Mark, Mark, BoxSize, NodeId)> = Vec::new();
            let mut line_h: f32 = 0.0;
            for item in &flex_items[range.clone()] {
                if item.auto_left {
                    x += auto_share;
                }
                let start = self.mark();
                let size = if doc.element(item.node).is_some() {
                    let border_w = item.size - item.margins[1] - item.margins[3];
                    let size = self.block(
                        item.node,
                        x,
                        y,
                        item.size,
                        Sizing::Exact(border_w.max(0.0)),
                        None,
                    );
                    BoxSize {
                        w: item.size,
                        ..size
                    }
                } else {
                    let h = self.lines(node, vec![self.text_item(item.node)], x, y, item.size);
                    BoxSize {
                        w: item.size,
                        h,
                        mt: 0.0,
                        mb: 0.0,
                        decorations: Vec::new(),
                    }
                };
                let end = self.mark();
                line_h = line_h.max(size.margin_h());
                x += item.size + col_gap + spacing;
                if item.auto_right {
                    x += auto_share;
                }
                placed.push((start, end, size, item.node));
            }
            if lines.len() == 1
                && let Some(h) = definite_h
            {
                line_h = line_h.max(h);
            }
            for (start, end, size, child) in placed {
                let align = if doc.element(child).is_some() {
                    self.st(child).align_self.unwrap_or(st.align_items)
                } else {
                    st.align_items
                };
                let room = line_h - size.margin_h();
                match align {
                    Align::Center => self.translate(start, end, 0.0, room / 2.0),
                    Align::End => self.translate(start, end, 0.0, room),
                    Align::Stretch
                        if doc
                            .element(child)
                            .is_some_and(|_| self.st(child).height == Length::Auto) =>
                    {
                        self.stretch(&size, line_h - size.mt - size.mb);
                    }
                    _ => {}
                }
            }
            y += line_h;
            if li + 1 < lines.len() {
                y += row_gap;
            }
        }
        y - cy
    }

    fn flex_column(
        &mut self,
        node: NodeId,
        children: &[NodeId],
        cx: f32,
        cy: f32,
        w: f32,
        definite_h: Option<f32>,
        gap: f32,
    ) -> f32 {
        let doc = self.doc();
        let st = self.st(node);
        let mut y = cy;
        let mut placed: Vec<(Mark, Mark, f32, f32)> = Vec::new();
        for (i, &child) in children.iter().enumerate() {
            if i > 0 {
                y += gap;
            }
            let start = self.mark();
            let (w_used, h) = if doc.element(child).is_some() {
                let cs = self.st(child);
                let align = cs.align_self.unwrap_or(st.align_items);
                let sizing = if align == Align::Stretch && cs.width == Length::Auto {
                    Sizing::Fill
                } else {
                    Sizing::Shrink
                };
                let size = self.block(child, cx, y, w, sizing, None);
                (size.w, size.margin_h())
            } else {
                let h = self.lines(node, vec![self.text_item(child)], cx, y, w);
                (self.intrinsic(child).1.min(w), h)
            };
            let end = self.mark();
            placed.push((start, end, w_used, h));
            y += h;
        }
        let content_h = y - cy;
        let free = definite_h.map_or(0.0, |h| (h - content_h).max(0.0));
        let offset = match st.justify {
            Justify::Center => free / 2.0,
            Justify::End => free,
            _ => 0.0,
        };
        for (i, (start, end, item_w, _)) in placed.into_iter().enumerate() {
            let align = children
                .get(i)
                .filter(|&&c| doc.element(c).is_some())
                .map_or(st.align_items, |&c| {
                    self.st(c).align_self.unwrap_or(st.align_items)
                });
            let dx = match align {
                Align::Center => (w - item_w) / 2.0,
                Align::End => w - item_w,
                _ => 0.0,
            };
            self.translate(start, end, dx, offset);
        }
        content_h
    }

    fn text_item(&self, node: NodeId) -> InlineItem {
        let text = match &self.doc().nodes[node].kind {
            NodeKind::Text(t) => t.clone(),
            _ => String::new(),
        };
        InlineItem::Text {
            node,
            text,
            link: self.link.clone(),
            bg: None,
        }
    }

    /// Rows of a table, each a list of cells, looking through row groups and
    /// stray wrappers such as a `<form>` between `<table>` and `<tr>`.
    fn table_rows(&self, table: NodeId) -> (Vec<(Option<NodeId>, Vec<NodeId>)>, Vec<NodeId>) {
        let doc = self.doc();
        let mut rows = Vec::new();
        let mut captions = Vec::new();
        let mut loose_cells = Vec::new();
        let mut stack: Vec<NodeId> = doc.nodes[table].children.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            if doc.element(n).is_none() {
                continue;
            }
            let st = self.st(n);
            match (doc.tag(n), st.display) {
                (_, Display::None) => {}
                ("caption", _) => captions.push(n),
                (_, Display::TableRow) => {
                    if !loose_cells.is_empty() {
                        rows.push((None, std::mem::take(&mut loose_cells)));
                    }
                    let cells = doc.nodes[n]
                        .children
                        .iter()
                        .copied()
                        .filter(|&c| {
                            doc.element(c).is_some() && self.st(c).display != Display::None
                        })
                        .collect();
                    rows.push((Some(n), cells));
                }
                (_, Display::TableCell) => loose_cells.push(n),
                _ => stack.extend(doc.nodes[n].children.iter().rev()),
            }
        }
        if !loose_cells.is_empty() {
            rows.push((None, loose_cells));
        }
        (rows, captions)
    }

    fn colspan(&self, cell: NodeId) -> usize {
        self.doc()
            .element(cell)
            .and_then(|e| e.attr("colspan"))
            .and_then(|c| c.trim().parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 1000)
    }

    /// Minimum and maximum width of each column.
    fn table_columns(&mut self, rows: &[(Option<NodeId>, Vec<NodeId>)]) -> (Vec<f32>, Vec<f32>) {
        let n = rows
            .iter()
            .map(|(_, cells)| cells.iter().map(|&c| self.colspan(c)).sum::<usize>())
            .max()
            .unwrap_or(0)
            .min(MAX_TABLE_COLUMNS);
        let mut mins = vec![0.0f32; n];
        let mut maxs = vec![0.0f32; n];
        let mut spans = Vec::new();
        for (_, cells) in rows {
            let mut col = 0;
            for &cell in cells {
                if col >= n {
                    break; // past MAX_TABLE_COLUMNS
                }
                let span = self.colspan(cell);
                let (mut min, mut max) = self.intrinsic(cell);
                if let Length::Px(w) = self.st(cell).width {
                    let cs = self.st(cell);
                    let extra = if cs.border_box {
                        0.0
                    } else {
                        let p = self.paddings(cs, 0.0);
                        p[1] + p[3] + cs.border_width[1] + cs.border_width[3]
                    };
                    max = (w + extra).max(min);
                    min = min.max(w + extra);
                }
                if span == 1 {
                    mins[col] = mins[col].max(min);
                    maxs[col] = maxs[col].max(max);
                } else {
                    spans.push((col, span.min(n - col), min, max));
                }
                col += span;
            }
        }
        for (col, span, min, max) in spans {
            let range = col..col + span;
            let cur_min: f32 = mins[range.clone()].iter().sum();
            if min > cur_min {
                let add = (min - cur_min) / span as f32;
                mins[range.clone()].iter_mut().for_each(|m| *m += add);
            }
            let cur_max: f32 = maxs[range.clone()].iter().sum();
            if max > cur_max {
                let add = (max - cur_max) / span as f32;
                maxs[range].iter_mut().for_each(|m| *m += add);
            }
        }
        for i in 0..n {
            maxs[i] = maxs[i].max(mins[i]);
        }
        (mins, maxs)
    }

    fn table(&mut self, node: NodeId, cx: f32, cy: f32, w: f32) -> f32 {
        let st = self.st(node);
        let (rows, captions) = self.table_rows(node);
        let mut y = cy;
        for caption in captions {
            let size = self.block(caption, cx, y, w, Sizing::Fill, None);
            y += size.margin_h();
        }
        let (mins, maxs) = self.table_columns(&rows);
        let n = mins.len();
        if n == 0 {
            return y - cy;
        }
        let s = st.border_spacing;
        let avail = (w - s * (n + 1) as f32).max(0.0);
        let sum_min: f32 = mins.iter().sum();
        let sum_max: f32 = maxs.iter().sum();
        let widths: Vec<f32> = if sum_max <= avail {
            let extra = avail - sum_max;
            (0..n)
                .map(|i| {
                    maxs[i]
                        + if sum_max > 0.0 {
                            extra * maxs[i] / sum_max
                        } else {
                            extra / n as f32
                        }
                })
                .collect()
        } else if sum_min < avail {
            let room = avail - sum_min;
            let flex = sum_max - sum_min;
            (0..n)
                .map(|i| mins[i] + room * (maxs[i] - mins[i]) / flex)
                .collect()
        } else {
            mins.clone()
        };

        y += s;
        for (row, cells) in rows {
            let row_bg = row.and_then(|r| {
                let rs = self.st(r);
                (rs.background.a > 0).then(|| {
                    self.emit(Item::Fill {
                        rect: Rect::default(),
                        color: rs.background,
                        radius: 0.0,
                    })
                })
            });
            let mut x = cx + s;
            let mut col = 0;
            let mut placed = Vec::new();
            let mut row_h = row
                .and_then(|r| resolve(self.st(r).height, 0.0))
                .unwrap_or(0.0);
            for cell in cells {
                let span = self.colspan(cell).min(n - col.min(n));
                if col >= n {
                    break;
                }
                let cell_w: f32 =
                    widths[col..col + span].iter().sum::<f32>() + s * (span - 1) as f32;
                let start = self.mark();
                let size = self.block(cell, x, y, cell_w, Sizing::Exact(cell_w), None);
                let end = self.mark();
                row_h = row_h.max(size.margin_h());
                placed.push((start, end, size, cell));
                x += cell_w + s;
                col += span;
            }
            for (start, end, size, cell) in placed {
                let room = row_h - size.margin_h();
                if room > 0.0 {
                    let valign = self.st(cell).vertical_align;
                    let content_start = Mark {
                        items: start.items + size.decorations.len(),
                        ..start
                    };
                    let dy = match valign {
                        VAlign::Middle => room / 2.0,
                        VAlign::Bottom => room,
                        _ => 0.0,
                    };
                    self.translate(content_start, end, 0.0, dy);
                    self.stretch(&size, row_h);
                }
            }
            if let Some(i) = row_bg
                && let Some(r) = self.out.items[i].rect_mut()
            {
                *r = Rect {
                    x: cx + s,
                    y,
                    w: x - cx - 2.0 * s,
                    h: row_h,
                };
            }
            y += row_h + s;
        }
        y - cy
    }

    /// Content-box min-content and max-content widths of an element.
    fn intrinsic_content(&mut self, node: NodeId) -> (f32, f32) {
        let doc = self.doc();
        if doc.element(node).is_none() {
            return self.intrinsic(node);
        }
        let st = self.st(node);
        if let Some((w, _)) = self.natural_size(node) {
            return (w, w);
        }
        match st.display {
            Display::Table => {
                let (rows, _) = self.table_rows(node);
                let (mins, maxs) = self.table_columns(&rows);
                let spacing = st.border_spacing * (mins.len() + 1) as f32;
                (
                    mins.iter().sum::<f32>() + spacing,
                    maxs.iter().sum::<f32>() + spacing,
                )
            }
            Display::Flex | Display::InlineFlex if !st.flex_column => {
                let gap = resolve0(st.column_gap, 0.0);
                let mut min: f32 = 0.0;
                let mut max = 0.0;
                let mut count: usize = 0;
                for &c in &doc.nodes[node].children {
                    if self.skip_for_intrinsic(c) {
                        continue;
                    }
                    let (cmin, cmax) = self.intrinsic(c);
                    min = if st.flex_wrap {
                        min.max(cmin)
                    } else {
                        min + cmin
                    };
                    max += cmax;
                    count += 1;
                }
                let gaps = gap * count.saturating_sub(1) as f32;
                (min + if st.flex_wrap { 0.0 } else { gaps }, max + gaps)
            }
            Display::TableRow => {
                let mut min = 0.0;
                let mut max = 0.0;
                for &c in &doc.nodes[node].children {
                    if !self.skip_for_intrinsic(c) {
                        let (a, b) = self.intrinsic(c);
                        min += a;
                        max += b;
                    }
                }
                (min, max)
            }
            _ => self.children_intrinsic(node),
        }
    }

    fn skip_for_intrinsic(&self, node: NodeId) -> bool {
        match &self.doc().nodes[node].kind {
            NodeKind::Text(t) => t.trim().is_empty(),
            NodeKind::Element(_) => {
                let st = self.st(node);
                st.display == Display::None || st.is_out_of_flow()
            }
            NodeKind::Document => true,
        }
    }

    fn children_intrinsic(&mut self, node: NodeId) -> (f32, f32) {
        let doc = self.doc();
        let mut min: f32 = 0.0;
        let mut max: f32 = 0.0;
        let mut line = 0.0;
        for &c in &doc.nodes[node].children {
            let block_level = match &doc.nodes[c].kind {
                NodeKind::Element(_) => {
                    let st = self.st(c);
                    if st.display == Display::None || st.is_out_of_flow() {
                        continue;
                    }
                    if doc.tag(c) == "br" {
                        max = max.max(line);
                        line = 0.0;
                        continue;
                    }
                    st.display.is_block_level() && st.float == Float::None
                }
                NodeKind::Text(_) => false,
                NodeKind::Document => continue,
            };
            let (cmin, cmax) = self.intrinsic(c);
            min = min.max(cmin);
            if block_level {
                max = max.max(line).max(cmax);
                line = 0.0;
            } else {
                line += cmax;
            }
        }
        (min, max.max(line))
    }

    /// Margin-box min-content and max-content widths of any node.
    fn intrinsic(&mut self, node: NodeId) -> (f32, f32) {
        if let Some(&v) = self.intrinsic_cache.get(&node) {
            return v;
        }
        let doc = self.doc();
        let result = match &doc.nodes[node].kind {
            NodeKind::Text(text) => self.text_intrinsic(node, text),
            NodeKind::Document => (0.0, 0.0),
            NodeKind::Element(_) => {
                let st = self.st(node);
                if st.display == Display::None {
                    (0.0, 0.0)
                } else {
                    let p = self.paddings(st, 0.0);
                    let m: [f32; 4] = std::array::from_fn(|i| match st.margin[i] {
                        Length::Px(v) => v,
                        _ => 0.0,
                    });
                    let pb = p[1] + p[3] + st.border_width[1] + st.border_width[3];
                    let (mut min, mut max) = match st.width {
                        Length::Px(w) => {
                            let w = if st.border_box { w - pb } else { w };
                            (w, w)
                        }
                        _ => self.intrinsic_content(node),
                    };
                    if let Length::Px(mw) = st.max_width {
                        let mw = if st.border_box { mw - pb } else { mw };
                        max = max.min(mw);
                        min = min.min(mw);
                    }
                    if let Length::Px(mw) = st.min_width {
                        let mw = if st.border_box { mw - pb } else { mw };
                        max = max.max(mw);
                        min = min.max(mw);
                    }
                    let extra = pb + m[1] + m[3];
                    (min.max(0.0) + extra, max.max(0.0) + extra)
                }
            }
        };
        self.intrinsic_cache.insert(node, result);
        result
    }

    fn text_intrinsic(&self, node: NodeId, text: &str) -> (f32, f32) {
        let st = self.st(node);
        let key = font_key(st);
        let fonts = self.fonts();
        let measure = |s: &str| {
            fonts.measure(
                key,
                &transform(s.to_string(), st.text_transform),
                st.font_size,
            )
        };
        if st.white_space.collapses_spaces() {
            let words: Vec<&str> = text.split_ascii_whitespace().collect();
            let space = fonts.advance(key, ' ', st.font_size);
            let widths: Vec<f32> = words.iter().map(|w| measure(w)).collect();
            // Collapsible space at either end still renders as one space
            // between neighbouring inline boxes.
            let edges = text.starts_with(|c: char| c.is_ascii_whitespace()) as usize
                + text.ends_with(|c: char| c.is_ascii_whitespace()) as usize;
            let gaps = (words.len().saturating_sub(1) + edges)
                .max((!text.is_empty()) as usize * edges.min(1));
            let total = widths.iter().sum::<f32>() + space * gaps as f32;
            if !st.white_space.wraps() {
                return (total, total);
            }
            let longest = widths.iter().copied().fold(0.0, f32::max);
            (longest, total)
        } else {
            let widest = text
                .lines()
                .map(|l| measure(&l.replace('\t', "        ")))
                .fold(0.0, f32::max);
            if st.white_space.wraps() {
                let longest = text.split_whitespace().map(measure).fold(0.0, f32::max);
                (longest, widest)
            } else {
                (widest, widest)
            }
        }
    }
}

struct LineState {
    y: f32,
    /// Floats placed in this run: (is right, width, bottom edge).
    floats: Vec<(bool, f32, f32)>,
    line: Vec<Piece>,
    line_w: f32,
    avail: f32,
    first: bool,
}

impl LineState {
    /// Widths taken by floats on the left and right at the current line.
    fn bounds(&self) -> (f32, f32) {
        let mut left = 0.0;
        let mut right = 0.0;
        for &(is_right, w, bottom) in &self.floats {
            if bottom > self.y + 0.01 {
                if is_right { right += w } else { left += w }
            }
        }
        (left, right)
    }

    fn next_float_bottom(&self) -> Option<f32> {
        self.floats
            .iter()
            .map(|f| f.2)
            .filter(|&b| b > self.y + 0.01)
            .reduce(f32::min)
    }

    fn reset(&mut self, w: f32, indent: f32) {
        let (l, r) = self.bounds();
        let ind = if self.first { indent } else { 0.0 };
        self.avail = w - l - r - ind;
        self.line_w = 0.0;
    }
}

struct FlowState {
    cursor: f32,
    /// Bottom margin of the previous block, not yet added to `cursor`, so the
    /// next sibling's top margin can collapse with it.
    prev_mb: f32,
    inline: Vec<InlineItem>,
}

fn to_content_h(st: &Style, h: f32, extra_h: f32) -> f32 {
    if st.border_box {
        (h - extra_h).max(0.0)
    } else {
        h
    }
}

fn collapse(a: f32, b: f32) -> f32 {
    if a >= 0.0 && b >= 0.0 {
        a.max(b)
    } else if a < 0.0 && b < 0.0 {
        a.min(b)
    } else {
        a + b
    }
}

fn trim_trailing_spaces(line: &mut Vec<Piece>) {
    while line
        .last()
        .is_some_and(|p| matches!(p.kind, PieceKind::Space))
    {
        line.pop();
    }
}

fn transform(text: String, t: TextTransform) -> String {
    match t {
        TextTransform::None => text,
        TextTransform::Uppercase => text.to_uppercase(),
        TextTransform::Lowercase => text.to_lowercase(),
        TextTransform::Capitalize => {
            let mut chars = text.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => text,
            }
        }
    }
}

fn button_label<'a>(value: Option<&'a str>, input_type: &str) -> &'a str {
    value.filter(|v| !v.is_empty()).unwrap_or(match input_type {
        "reset" => "Reset",
        "file" => "Choose file",
        "button" => "",
        _ => "Submit",
    })
}

fn alpha(n: i32, upper: bool) -> String {
    let mut n = n.max(1) as u32;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        let base = if upper { b'A' } else { b'a' };
        out.push((base + (n % 26) as u8) as char);
        n /= 26;
    }
    out.iter().rev().collect()
}

fn roman(n: i32) -> String {
    let mut n = n.clamp(1, 3999);
    let table = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (v, s) in table {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{css, html, paint};

    /// Styles, lays out and paints `markup` on a thread with the main thread's
    /// stack size (set in `build.rs`).
    fn render_on_main_stack(markup: String) {
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn(move || {
                let Ok(fonts) = crate::font::Fonts::new() else {
                    return; // no system fonts on this machine
                };
                let doc = html::parse(&markup);
                let sheets = vec![css::parse_stylesheet("")];
                let ctx = crate::style::StyleContext {
                    zoom: 1.0,
                    viewport_width: 800.0,
                    viewport_height: 600.0,
                };
                let styles = crate::style::compute_styles(&doc, &sheets, &ctx);
                let images = HashMap::new();
                let form = FormState::default();
                let input = LayoutInput {
                    doc: &doc,
                    styles: &styles,
                    fonts: &fonts,
                    images: &images,
                    form: &form,
                    zoom: 1.0,
                    viewport_width: 800.0,
                    viewport_height: 600.0,
                };
                let layout = layout(&input);
                let mut pixels = vec![0u32; 64 * 64];
                let mut canvas = paint::Canvas::new(&mut pixels, 64, 64);
                paint::paint_items(&mut canvas, &layout.items, &fonts, &images, 0.0, 0.0);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn textarea_sizes_must_be_finite() {
        render_on_main_stack(r#"<textarea cols="inf" rows="1e30">x</textarea>"#.to_string());
        render_on_main_stack(r#"<textarea cols="-5" rows="NaN">x</textarea>"#.to_string());
    }

    #[test]
    fn deep_nesting_fits_the_main_thread_stack() {
        let n = html::MAX_DEPTH * 2;
        for open in [
            "<div>",
            "<span>",
            "<ul><li>",
            "<table><tr><td>",
            "<div style=\"display:flex\">",
            "<div style=\"display:inline-block\">",
            "<div style=\"float:left\">",
        ] {
            render_on_main_stack(open.repeat(n) + "x");
        }
    }
}

#[cfg(test)]
mod scaling_tests {
    use super::*;
    use crate::{css, html};
    use std::time::{Duration, Instant};

    /// The text items `markup` lays out to, or `None` without system fonts.
    fn texts(markup: &str, css: &str) -> Option<Vec<String>> {
        let fonts = crate::font::Fonts::new().ok()?;
        let doc = html::parse(markup);
        let sheets = vec![css::parse_stylesheet(css)];
        let ctx = crate::style::StyleContext {
            zoom: 1.0,
            viewport_width: 800.0,
            viewport_height: 600.0,
        };
        let styles = crate::style::compute_styles(&doc, &sheets, &ctx);
        let (images, form) = (HashMap::new(), FormState::default());
        let input = LayoutInput {
            doc: &doc,
            styles: &styles,
            fonts: &fonts,
            images: &images,
            form: &form,
            zoom: 1.0,
            viewport_width: 800.0,
            viewport_height: 600.0,
        };
        let items = layout(&input).items;
        Some(
            items
                .into_iter()
                .filter_map(|i| match i {
                    Item::Text { text, .. } => Some(text),
                    _ => None,
                })
                .collect(),
        )
    }

    #[test]
    fn list_numbers_follow_start_and_value() {
        let markup = r#"<ol start="5"><li>a<li>b<li value="10">c<p>x</p><li>d</ol>"#;
        let Some(t) = texts(markup, "") else { return };
        let markers: Vec<_> = t.iter().filter(|s| s.ends_with('.')).collect();
        assert_eq!(markers, ["5.", "6.", "10.", "11."]);
        let Some(t) = texts(r#"<ol><li value="2147483647">a<li>b</ol>"#, "") else {
            return;
        };
        assert!(t.contains(&"2147483647.".to_string()));
    }

    #[test]
    fn long_lists_tables_and_words_lay_out_quickly() {
        let start = Instant::now();
        let list = "<ol>".to_string() + &"<li>x".repeat(40_000) + "</ol>";
        let Some(t) = texts(&list, "") else { return };
        assert!(t.contains(&"40000.".to_string()));

        let cells = "<td colspan=1000>x</td>".repeat(20_000);
        texts(
            &format!("<table><tr>{cells}</tr><tr><td>y</td></tr></table>"),
            "",
        );

        let word = "w".repeat(400_000);
        let t = texts(&format!("<div style=\"width:1px\">{word}</div>"), "").unwrap();
        let pieces: Vec<_> = t.iter().filter(|s| s.starts_with('w')).collect();
        assert_eq!(pieces.iter().map(|s| s.len()).sum::<usize>(), 400_000);
        assert!(pieces.len() > 1000);
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "{:?}",
            start.elapsed()
        );
    }
}
