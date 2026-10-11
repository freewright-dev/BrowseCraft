//! The cascade: matches rules against elements and computes each element's
//! style. Lengths come out in device pixels (CSS pixels times zoom), except
//! percentages, which layout resolves.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use crate::css::{self, Color, Declaration, Length, Selector, SelectorKey, Stylesheet};
use crate::dom::{Document, NodeId, NodeKind};

const UA_CSS: &str = include_str!("ua.css");

/// Pages without a doctype don't inherit text and font styles into tables.
const QUIRKS_CSS: &str = "table { text-align: left; font-size: medium; font-weight: normal; font-style: normal; line-height: normal; white-space: normal }";

fn ua_sheet() -> &'static Stylesheet {
    static SHEET: OnceLock<Stylesheet> = OnceLock::new();
    SHEET.get_or_init(|| css::parse_stylesheet(UA_CSS))
}

fn quirks_sheet() -> &'static Stylesheet {
    static SHEET: OnceLock<Stylesheet> = OnceLock::new();
    SHEET.get_or_init(|| css::parse_stylesheet(QUIRKS_CSS))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Display {
    None,
    Contents,
    Inline,
    Block,
    InlineBlock,
    ListItem,
    Flex,
    InlineFlex,
    Table,
    TableRowGroup,
    TableRow,
    TableCell,
}

impl Display {
    pub fn is_block_level(self) -> bool {
        matches!(
            self,
            Display::Block
                | Display::ListItem
                | Display::Flex
                | Display::Table
                | Display::TableRowGroup
                | Display::TableRow
                | Display::TableCell
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Family {
    Sans,
    Serif,
    Mono,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WhiteSpace {
    Normal,
    NoWrap,
    Pre,
    PreWrap,
    PreLine,
}

impl WhiteSpace {
    pub fn collapses_spaces(self) -> bool {
        matches!(self, WhiteSpace::Normal | WhiteSpace::NoWrap | WhiteSpace::PreLine)
    }
    pub fn keeps_newlines(self) -> bool {
        matches!(self, WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::PreLine)
    }
    pub fn wraps(self) -> bool {
        !matches!(self, WhiteSpace::NoWrap | WhiteSpace::Pre)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ListStyle {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextTransform {
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    Normal,
    Factor(f32),
    Px(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Justify {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Stretch,
    Start,
    Center,
    End,
    Baseline,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VAlign {
    Baseline,
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Float {
    None,
    Left,
    Right,
}

#[derive(Clone, Debug)]
pub struct Style {
    pub display: Display,
    pub position: Position,
    pub float: Float,
    pub visible: bool,
    pub opacity: f32,
    pub overflow_hidden: bool,

    pub color: Color,
    pub background: Color,
    pub background_image: Option<String>,

    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub family: Family,
    pub line_height: LineHeight,
    pub text_align: TextAlign,
    /// `-webkit-center`: also centers child blocks (`<center>`, `align`).
    pub center_blocks: bool,
    pub underline: bool,
    pub line_through: bool,
    pub white_space: WhiteSpace,
    pub text_transform: TextTransform,
    pub list_style: ListStyle,
    pub vertical_align: VAlign,
    pub text_indent: Length,

    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    pub max_width: Length,
    pub min_height: Length,
    pub max_height: Length,
    pub border_box: bool,
    /// Top, right, bottom, left.
    pub margin: [Length; 4],
    pub padding: [Length; 4],
    pub border_width: [f32; 4],
    pub border_color: [Option<Color>; 4],
    /// Whether each side has a visible `border-style`.
    pub border_style: [bool; 4],
    pub border_radius: f32,
    pub inset: [Length; 4],

    pub flex_column: bool,
    pub flex_wrap: bool,
    pub flex_reverse: bool,
    pub justify: Justify,
    pub align_items: Align,
    pub align_self: Option<Align>,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Length,
    pub order: i32,
    pub row_gap: Length,
    pub column_gap: Length,
    pub border_spacing: f32,

    /// Custom properties (`--name`), inherited.
    pub vars: Rc<HashMap<String, String>>,
}

impl Style {
    pub fn initial(zoom: f32) -> Style {
        Style {
            display: Display::Inline,
            position: Position::Static,
            float: Float::None,
            visible: true,
            opacity: 1.0,
            overflow_hidden: false,
            color: Color::BLACK,
            background: Color::TRANSPARENT,
            background_image: None,
            font_size: 16.0 * zoom,
            bold: false,
            italic: false,
            family: Family::Sans,
            line_height: LineHeight::Normal,
            text_align: TextAlign::Left,
            center_blocks: false,
            underline: false,
            line_through: false,
            white_space: WhiteSpace::Normal,
            text_transform: TextTransform::None,
            list_style: ListStyle::Disc,
            vertical_align: VAlign::Baseline,
            text_indent: Length::Px(0.0),
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            max_width: Length::Auto,
            min_height: Length::Auto,
            max_height: Length::Auto,
            border_box: false,
            margin: [Length::Px(0.0); 4],
            padding: [Length::Px(0.0); 4],
            border_width: [0.0; 4],
            border_color: [None; 4],
            border_style: [false; 4],
            border_radius: 0.0,
            inset: [Length::Auto; 4],
            flex_column: false,
            flex_wrap: false,
            flex_reverse: false,
            justify: Justify::Start,
            align_items: Align::Stretch,
            align_self: None,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            order: 0,
            row_gap: Length::Px(0.0),
            column_gap: Length::Px(0.0),
            border_spacing: 0.0,
            vars: Rc::new(HashMap::new()),
        }
    }

    /// A child style: inherited properties copied, the rest reset.
    fn inherit(parent: &Style, zoom: f32) -> Style {
        Style {
            visible: parent.visible,
            color: parent.color,
            font_size: parent.font_size,
            bold: parent.bold,
            italic: parent.italic,
            family: parent.family,
            line_height: parent.line_height,
            text_align: parent.text_align,
            center_blocks: parent.center_blocks,
            underline: parent.underline,
            line_through: parent.line_through,
            white_space: parent.white_space,
            text_transform: parent.text_transform,
            list_style: parent.list_style,
            text_indent: parent.text_indent,
            border_spacing: parent.border_spacing,
            vars: parent.vars.clone(),
            ..Style::initial(zoom)
        }
    }

    pub fn line_height_px(&self) -> f32 {
        match self.line_height {
            LineHeight::Normal => self.font_size * 1.2,
            LineHeight::Factor(f) => self.font_size * f,
            LineHeight::Px(px) => px,
        }
    }

    pub fn border_color(&self, side: usize) -> Color {
        self.border_color[side].unwrap_or(self.color)
    }

    pub fn is_out_of_flow(&self) -> bool {
        matches!(self.position, Position::Absolute | Position::Fixed)
    }
}

struct Entry<'a> {
    selector: &'a Selector,
    declarations: &'a [Declaration],
    origin: u8,
    order: u32,
}

const ORIGIN_UA: u8 = 0;
const ORIGIN_HINT: u8 = 1;
const ORIGIN_AUTHOR: u8 = 2;
const ORIGIN_INLINE: u8 = 3;

/// Rules from every stylesheet that applies at the current viewport width,
/// bucketed by the most selective part of their subject.
struct RuleIndex<'a> {
    buckets: HashMap<SelectorKey, Vec<Entry<'a>>>,
}

impl<'a> RuleIndex<'a> {
    fn new(sheets: &'a [Stylesheet], viewport_css_width: f32, quirks: bool) -> Self {
        let mut buckets: HashMap<SelectorKey, Vec<Entry<'a>>> = HashMap::new();
        let mut order = 0;
        let ua = std::iter::once(ua_sheet()).chain(quirks.then(quirks_sheet));
        let all = ua.map(|s| (ORIGIN_UA, s)).chain(sheets.iter().map(|s| (ORIGIN_AUTHOR, s)));
        for (origin, sheet) in all {
            for rule in &sheet.rules {
                if rule.media.as_ref().is_some_and(|m| !css::media_matches(m, viewport_css_width)) {
                    continue;
                }
                for selector in &rule.selectors {
                    order += 1;
                    buckets.entry(selector.key()).or_default().push(Entry {
                        selector,
                        declarations: &rule.declarations,
                        origin,
                        order,
                    });
                }
            }
        }
        RuleIndex { buckets }
    }

    fn matching(&self, doc: &Document, node: NodeId) -> Vec<&Entry<'a>> {
        let Some(el) = doc.element(node) else { return Vec::new() };
        let mut keys = vec![SelectorKey::Universal, SelectorKey::Tag(el.tag.clone())];
        if let Some(id) = el.id() {
            keys.push(SelectorKey::Id(id.to_string()));
        }
        keys.extend(el.classes().map(|c| SelectorKey::Class(c.to_string())));
        keys.dedup();
        let mut out = Vec::new();
        for key in &keys {
            if let Some(entries) = self.buckets.get(key) {
                out.extend(entries.iter().filter(|e| e.selector.matches(doc, node)));
            }
        }
        out
    }
}

/// A fingerprint of which `@media` queries match, so callers can skip a
/// restyle when a resize does not cross a breakpoint.
pub fn media_signature(sheets: &[Stylesheet], viewport_css_width: f32) -> Vec<bool> {
    let mut seen: Vec<&str> = Vec::new();
    for rule in sheets.iter().flat_map(|s| &s.rules) {
        if let Some(m) = &rule.media
            && !seen.contains(&m.as_str()) {
                seen.push(m);
            }
    }
    seen.iter().map(|m| css::media_matches(m, viewport_css_width)).collect()
}

pub struct StyleContext {
    pub zoom: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
}

/// Computes a style for every node. Text nodes share their parent's style.
pub fn compute_styles(doc: &Document, sheets: &[Stylesheet], ctx: &StyleContext) -> Vec<Rc<Style>> {
    let index = RuleIndex::new(sheets, ctx.viewport_width / ctx.zoom, doc.quirks);
    let root_style = Rc::new(Style::initial(ctx.zoom));
    let mut styles: Vec<Rc<Style>> = vec![root_style.clone(); doc.nodes.len()];
    let mut root_font = 16.0 * ctx.zoom;

    // Nodes are created in document order, so parents come before children.
    for id in 1..doc.nodes.len() {
        let parent = doc.nodes[id].parent.map_or(root_style.clone(), |p| styles[p].clone());
        styles[id] = match &doc.nodes[id].kind {
            NodeKind::Element(_) => {
                let style = compute_element(doc, id, &parent, &index, ctx, root_font);
                if doc.tag(id) == "html" {
                    root_font = style.font_size;
                }
                Rc::new(style)
            }
            _ => parent,
        };
    }
    styles
}

fn compute_element(
    doc: &Document,
    node: NodeId,
    parent: &Style,
    index: &RuleIndex,
    ctx: &StyleContext,
    root_font: f32,
) -> Style {
    let el = doc.element(node).expect("element");
    let mut entries: Vec<(bool, u8, (u16, u16, u16), u32, &Declaration)> = Vec::new();
    for e in index.matching(doc, node) {
        for d in e.declarations {
            entries.push((d.important, e.origin, e.selector.specificity, e.order, d));
        }
    }
    let hints = presentational_hints(doc, node);
    for d in &hints {
        entries.push((false, ORIGIN_HINT, (0, 0, 0), 0, d));
    }
    let inline = el.attr("style").map(css::parse_declarations).unwrap_or_default();
    for d in &inline {
        entries.push((d.important, ORIGIN_INLINE, (0, 0, 0), u32::MAX, d));
    }
    entries.sort_by_key(|a| (a.0, a.1, a.2, a.3));

    // Custom properties first, so var() can see this element's own values.
    let mut vars: Option<HashMap<String, String>> = None;
    for (_, _, _, _, d) in &entries {
        if d.name.starts_with("--") {
            vars.get_or_insert_with(|| (*parent.vars).clone()).insert(d.name.clone(), d.value.clone());
        }
    }
    let vars = vars.map_or_else(|| parent.vars.clone(), Rc::new);

    let mut specified: HashMap<String, String> = HashMap::new();
    for (_, _, _, _, d) in &entries {
        if d.name.starts_with("--") {
            continue;
        }
        let Some(value) = substitute_vars(&d.value, &vars, 0) else { continue };
        for (name, value) in expand_shorthand(&d.name, &value) {
            specified.insert(name, value);
        }
    }

    let mut style = Style::inherit(parent, ctx.zoom);
    style.vars = vars;
    let resolver = Resolver { parent, ctx, root_font };

    // Font size and color first: other values depend on them.
    if let Some(v) = specified.get("font-size") {
        apply(&mut style, "font-size", v, &resolver);
    }
    if let Some(v) = specified.get("color") {
        apply(&mut style, "color", v, &resolver);
    }
    for (name, value) in &specified {
        if name != "font-size" && name != "color" {
            apply(&mut style, name, value, &resolver);
        }
    }

    // Blockify floated and absolutely positioned boxes; flex and table
    // children follow their own rules in layout.
    if style.float != Float::None || style.is_out_of_flow() {
        style.display = match style.display {
            Display::Inline | Display::InlineBlock => Display::Block,
            Display::InlineFlex => Display::Flex,
            d => d,
        };
    }
    for side in 0..4 {
        if !style.border_style[side] || style.border_color[side].is_some_and(|c| c.a == 0) {
            style.border_width[side] = 0.0;
        }
    }
    style
}

struct Resolver<'a> {
    parent: &'a Style,
    ctx: &'a StyleContext,
    root_font: f32,
}

impl Resolver<'_> {
    /// Resolves a length against `font_size` into device pixels or a
    /// percentage.
    fn length(&self, v: &str, font_size: f32) -> Option<Length> {
        let zoom = self.ctx.zoom;
        Some(match css::parse_length(v)? {
            Length::Px(px) => Length::Px(px * zoom),
            Length::Em(em) => Length::Px(em * font_size),
            Length::Rem(rem) => Length::Px(rem * self.root_font),
            Length::Vw(vw) => Length::Px(vw * self.ctx.viewport_width / 100.0),
            Length::Vh(vh) => Length::Px(vh * self.ctx.viewport_height / 100.0),
            l => l,
        })
    }

    fn px(&self, v: &str, font_size: f32) -> Option<f32> {
        match self.length(v, font_size)? {
            Length::Px(px) => Some(px),
            _ => None,
        }
    }
}

fn apply(style: &mut Style, name: &str, value: &str, r: &Resolver) {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    let fs = style.font_size;
    if matches!(lower.as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer") {
        let initial = Style::initial(r.ctx.zoom);
        let from = if lower == "inherit" { r.parent } else { &initial };
        copy_property(name, from, style);
        return;
    }
    let len = |v: &str| r.length(v, fs);
    match name {
        "display" => {
            style.display = match lower.as_str() {
                "none" => Display::None,
                "block" | "flow-root" | "grid" | "table-caption" | "flow" => Display::Block,
                "inline" => Display::Inline,
                "inline-block" | "inline-grid" | "inline-table" => Display::InlineBlock,
                "list-item" => Display::ListItem,
                "flex" | "-webkit-box" | "-webkit-flex" => Display::Flex,
                "inline-flex" | "-webkit-inline-flex" => Display::InlineFlex,
                "table" => Display::Table,
                "table-row-group" | "table-header-group" | "table-footer-group" => Display::TableRowGroup,
                "table-row" => Display::TableRow,
                "table-cell" => Display::TableCell,
                "contents" => Display::Contents,
                "table-column" | "table-column-group" => Display::None,
                _ => style.display,
            }
        }
        "position" => {
            style.position = match lower.as_str() {
                "relative" => Position::Relative,
                "absolute" => Position::Absolute,
                "fixed" => Position::Fixed,
                "sticky" | "-webkit-sticky" => Position::Sticky,
                _ => Position::Static,
            }
        }
        "float" => {
            style.float = match lower.as_str() {
                "left" | "inline-start" => Float::Left,
                "right" | "inline-end" => Float::Right,
                _ => Float::None,
            }
        }
        "visibility" => style.visible = lower == "visible",
        "opacity" => {
            if let Ok(o) = lower.trim_end_matches('%').parse::<f32>() {
                style.opacity = if lower.ends_with('%') { o / 100.0 } else { o };
            }
        }
        "overflow" | "overflow-x" | "overflow-y" => {
            if name == "overflow" || name == "overflow-y" {
                style.overflow_hidden = matches!(lower.as_str(), "hidden" | "clip" | "auto" | "scroll")
                    && !lower.contains("visible");
            }
        }
        "color" => {
            if let Some(c) = parse_color_value(&lower, r.parent.color) {
                style.color = c;
            }
        }
        "background-color" => {
            if let Some(c) = parse_color_value(&lower, style.color) {
                style.background = c;
            }
        }
        "background-image" => style.background_image = css::parse_url(v),
        "font-size" => {
            let parent = r.parent.font_size;
            let zoom = r.ctx.zoom;
            let size = match lower.as_str() {
                "xx-small" => Some(9.0 * zoom),
                "x-small" => Some(10.0 * zoom),
                "small" => Some(13.0 * zoom),
                "medium" => Some(16.0 * zoom),
                "large" => Some(18.0 * zoom),
                "x-large" => Some(24.0 * zoom),
                "xx-large" => Some(32.0 * zoom),
                "xxx-large" => Some(48.0 * zoom),
                "smaller" => Some(parent / 1.2),
                "larger" => Some(parent * 1.2),
                _ => match r.length(&lower, parent) {
                    Some(Length::Px(px)) => Some(px),
                    Some(Length::Percent(p)) => Some(parent * p / 100.0),
                    _ => None,
                },
            };
            if let Some(size) = size.filter(|s| *s >= 0.0) {
                style.font_size = size;
            }
        }
        "font-weight" => {
            style.bold = match lower.as_str() {
                "bold" | "bolder" => true,
                "normal" | "lighter" => false,
                n => n.parse::<u32>().map_or(style.bold, |w| w >= 600),
            }
        }
        "font-style" => style.italic = lower.starts_with("italic") || lower.starts_with("oblique"),
        "font-family" => style.family = parse_family(&lower).unwrap_or(style.family),
        "line-height" => {
            style.line_height = if lower == "normal" {
                LineHeight::Normal
            } else if let Ok(f) = lower.parse::<f32>() {
                LineHeight::Factor(f)
            } else {
                match len(&lower) {
                    Some(Length::Px(px)) => LineHeight::Px(px),
                    Some(Length::Percent(p)) => LineHeight::Px(fs * p / 100.0),
                    _ => style.line_height,
                }
            }
        }
        "text-align" => {
            style.center_blocks = matches!(lower.as_str(), "-webkit-center" | "-moz-center");
            style.text_align = match lower.as_str() {
                "center" | "-webkit-center" | "-moz-center" => TextAlign::Center,
                "right" | "end" | "-webkit-right" => TextAlign::Right,
                _ => TextAlign::Left,
            }
        }
        "text-decoration-line" => {
            style.underline = lower.contains("underline");
            style.line_through = lower.contains("line-through");
        }
        "white-space" | "white-space-collapse" => {
            style.white_space = match lower.as_str() {
                "nowrap" => WhiteSpace::NoWrap,
                "pre" => WhiteSpace::Pre,
                "pre-wrap" | "break-spaces" | "preserve" => WhiteSpace::PreWrap,
                "pre-line" | "preserve-breaks" => WhiteSpace::PreLine,
                _ => WhiteSpace::Normal,
            }
        }
        "text-wrap" | "text-wrap-mode" if lower == "nowrap" => style.white_space = WhiteSpace::NoWrap,
        "text-transform" => {
            style.text_transform = match lower.as_str() {
                "uppercase" => TextTransform::Uppercase,
                "lowercase" => TextTransform::Lowercase,
                "capitalize" => TextTransform::Capitalize,
                _ => TextTransform::None,
            }
        }
        "list-style-type" => {
            style.list_style = match lower.as_str() {
                "none" => ListStyle::None,
                "circle" => ListStyle::Circle,
                "square" => ListStyle::Square,
                "decimal" | "decimal-leading-zero" => ListStyle::Decimal,
                "lower-alpha" | "lower-latin" => ListStyle::LowerAlpha,
                "upper-alpha" | "upper-latin" => ListStyle::UpperAlpha,
                "lower-roman" => ListStyle::LowerRoman,
                "upper-roman" => ListStyle::UpperRoman,
                "disc" => ListStyle::Disc,
                _ => style.list_style,
            }
        }
        "vertical-align" => {
            style.vertical_align = match lower.as_str() {
                "top" | "text-top" => VAlign::Top,
                "middle" => VAlign::Middle,
                "bottom" | "text-bottom" => VAlign::Bottom,
                _ => VAlign::Baseline,
            }
        }
        "text-indent" => style.text_indent = len(&lower).unwrap_or(style.text_indent),
        "width" => style.width = size_value(&lower, len),
        "height" => style.height = size_value(&lower, len),
        "min-width" => style.min_width = size_value(&lower, len),
        "min-height" => style.min_height = size_value(&lower, len),
        "max-width" => style.max_width = size_value(&lower, len),
        "max-height" => style.max_height = size_value(&lower, len),
        "box-sizing" => style.border_box = lower == "border-box",
        "border-radius" => {
            let first = lower.split_whitespace().next().unwrap_or("");
            style.border_radius = match len(first) {
                Some(Length::Px(px)) => px,
                Some(Length::Percent(p)) if p >= 50.0 => f32::INFINITY,
                _ => 0.0,
            };
        }
        "flex-direction" => {
            style.flex_column = lower.starts_with("column");
            style.flex_reverse = lower.ends_with("reverse");
        }
        "flex-wrap" => style.flex_wrap = lower.starts_with("wrap"),
        "justify-content" => {
            style.justify = match lower.trim_start_matches("safe ").trim_start_matches("unsafe ") {
                "center" => Justify::Center,
                "flex-end" | "end" | "right" => Justify::End,
                "space-between" => Justify::SpaceBetween,
                "space-around" => Justify::SpaceAround,
                "space-evenly" => Justify::SpaceEvenly,
                _ => Justify::Start,
            }
        }
        "align-items" => style.align_items = parse_align(&lower).unwrap_or(style.align_items),
        "align-self" => style.align_self = parse_align(&lower),
        "flex-grow" => style.flex_grow = lower.parse().unwrap_or(style.flex_grow),
        "flex-shrink" => style.flex_shrink = lower.parse().unwrap_or(style.flex_shrink),
        "flex-basis" => style.flex_basis = size_value(&lower, len),
        "order" => style.order = lower.parse().unwrap_or(0),
        "row-gap" | "grid-row-gap" => style.row_gap = len(&lower).unwrap_or(style.row_gap),
        "column-gap" | "grid-column-gap" => style.column_gap = len(&lower).unwrap_or(style.column_gap),
        "border-spacing" => {
            if let Some(px) = r.px(lower.split_whitespace().next().unwrap_or(""), fs) {
                style.border_spacing = px;
            }
        }
        _ => {
            if let Some(side) = side_index(name, "margin-") {
                if let Some(l) = len(&lower) {
                    style.margin[side] = l;
                }
            } else if let Some(side) = side_index(name, "padding-") {
                if let Some(l) = len(&lower) {
                    style.padding[side] = l;
                }
            } else if let Some(side) = border_side(name, "-width") {
                let w = match lower.as_str() {
                    "thin" => Some(1.0 * r.ctx.zoom),
                    "medium" => Some(3.0 * r.ctx.zoom),
                    "thick" => Some(5.0 * r.ctx.zoom),
                    _ => r.px(&lower, fs),
                };
                if let Some(w) = w {
                    // Keep hairlines visible at any zoom.
                    style.border_width[side] = if w > 0.0 { w.max(1.0) } else { 0.0 };
                }
            } else if let Some(side) = border_side(name, "-style") {
                style.border_style[side] = !matches!(lower.as_str(), "none" | "hidden");
            } else if let Some(side) = border_side(name, "-color") {
                if lower != "currentcolor" {
                    if let Some(c) = parse_color_value(&lower, style.color) {
                        style.border_color[side] = Some(c);
                    }
                } else {
                    style.border_color[side] = None;
                }
            } else if let Some(side) = ["top", "right", "bottom", "left"].iter().position(|s| *s == name) {
                style.inset[side] = len(&lower).unwrap_or(Length::Auto);
            }
        }
    }
}

/// Copies one property for `inherit` / `initial`.
fn copy_property(name: &str, from: &Style, to: &mut Style) {
    match name {
        "color" => to.color = from.color,
        "background-color" => to.background = from.background,
        "font-size" => to.font_size = from.font_size,
        "font-weight" => to.bold = from.bold,
        "font-style" => to.italic = from.italic,
        "font-family" => to.family = from.family,
        "line-height" => to.line_height = from.line_height,
        "text-align" => {
            to.text_align = from.text_align;
            to.center_blocks = from.center_blocks;
        }
        "display" => to.display = from.display,
        "white-space" => to.white_space = from.white_space,
        "visibility" => to.visible = from.visible,
        "width" => to.width = from.width,
        "height" => to.height = from.height,
        "max-width" => to.max_width = from.max_width,
        "position" => to.position = from.position,
        "float" => to.float = from.float,
        "text-transform" => to.text_transform = from.text_transform,
        "list-style-type" => to.list_style = from.list_style,
        "text-decoration-line" => {
            to.underline = from.underline;
            to.line_through = from.line_through;
        }
        _ => {
            if let Some(side) = side_index(name, "margin-") {
                to.margin[side] = from.margin[side];
            } else if let Some(side) = side_index(name, "padding-") {
                to.padding[side] = from.padding[side];
            } else if let Some(side) = border_side(name, "-width") {
                to.border_width[side] = from.border_width[side];
            } else if let Some(side) = border_side(name, "-style") {
                to.border_style[side] = from.border_style[side];
            } else if let Some(side) = border_side(name, "-color") {
                to.border_color[side] = from.border_color[side];
            }
        }
    }
}

fn size_value(v: &str, len: impl Fn(&str) -> Option<Length>) -> Length {
    match v {
        "none" | "auto" | "fit-content" | "max-content" | "min-content" | "-webkit-fill-available"
        | "stretch" => Length::Auto,
        _ => len(v).unwrap_or(Length::Auto),
    }
}

fn parse_align(v: &str) -> Option<Align> {
    Some(match v.trim_start_matches("safe ").trim_start_matches("unsafe ") {
        "stretch" | "normal" => Align::Stretch,
        "flex-start" | "start" | "self-start" => Align::Start,
        "center" => Align::Center,
        "flex-end" | "end" | "self-end" => Align::End,
        "baseline" | "first baseline" | "last baseline" => Align::Baseline,
        _ => return None,
    })
}

fn side_index(name: &str, prefix: &str) -> Option<usize> {
    let side = name.strip_prefix(prefix)?;
    ["top", "right", "bottom", "left"].iter().position(|s| *s == side).or(match side {
        "block-start" => Some(0),
        "inline-end" => Some(1),
        "block-end" => Some(2),
        "inline-start" => Some(3),
        _ => None,
    })
}

fn border_side(name: &str, suffix: &str) -> Option<usize> {
    side_index(name.strip_suffix(suffix)?, "border-")
}

fn parse_color_value(v: &str, current: Color) -> Option<Color> {
    if v == "currentcolor" {
        return Some(current);
    }
    css::parse_color(v)
}

fn parse_family(v: &str) -> Option<Family> {
    for name in v.split(',') {
        let name = name.trim().trim_matches(['"', '\'']).trim();
        let mono = ["monospace", "courier", "consolas", "menlo", "monaco", "mono", "code", "fira code", "source code"];
        if mono.iter().any(|m| name.contains(m)) {
            return Some(Family::Mono);
        }
        if name == "serif"
            || ["times", "georgia", "garamond", "cambria", "palatino", "baskerville", "book antiqua", "charter"]
                .iter()
                .any(|s| name.contains(s))
        {
            return Some(Family::Serif);
        }
        if name == "sans-serif"
            || name == "system-ui"
            || name.starts_with("-apple-system")
            || name.starts_with("blinkmacsystemfont")
            || ["arial", "helvetica", "verdana", "tahoma", "segoe", "roboto", "inter", "open sans", "lato", "noto sans", "ubuntu"]
                .iter()
                .any(|s| name.contains(s))
        {
            return Some(Family::Sans);
        }
    }
    None
}

/// Replaces `var(--name, fallback)` references. Returns `None` when a
/// variable is missing and has no fallback, which makes the declaration
/// invalid.
fn substitute_vars(value: &str, vars: &HashMap<String, String>, depth: u32) -> Option<String> {
    if !value.contains("var(") {
        return Some(value.to_string());
    }
    if depth > 8 {
        return None;
    }
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("var(") {
        out.push_str(&rest[..start]);
        let inner_start = start + 4;
        let mut depth_paren = 1;
        let mut end = None;
        for (i, c) in rest[inner_start..].char_indices() {
            match c {
                '(' => depth_paren += 1,
                ')' => {
                    depth_paren -= 1;
                    if depth_paren == 0 {
                        end = Some(inner_start + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        let inner = &rest[inner_start..end];
        let (name, fallback) = match inner.split_once(',') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (inner.trim(), None),
        };
        let resolved = match vars.get(name) {
            Some(v) => substitute_vars(v, vars, depth + 1)?,
            None => substitute_vars(fallback?, vars, depth + 1)?,
        };
        out.push_str(&resolved);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Splits a value into space-separated tokens, keeping functions together.
fn tokens(v: &str) -> Vec<&str> {
    css::split_top_level(v, ' ').into_iter().map(str::trim).filter(|t| !t.is_empty()).collect()
}

fn four_sides(v: &str) -> Option<[String; 4]> {
    let t = tokens(v);
    let s = |i: usize| t[i].to_string();
    Some(match t.len() {
        1 => [s(0), s(0), s(0), s(0)],
        2 => [s(0), s(1), s(0), s(1)],
        3 => [s(0), s(1), s(2), s(1)],
        4 => [s(0), s(1), s(2), s(3)],
        _ => return None,
    })
}

const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];

fn expand_shorthand(name: &str, value: &str) -> Vec<(String, String)> {
    let one = |n: &str, v: &str| vec![(n.to_string(), v.to_string())];
    let sides = |prefix: &str, suffix: &str| -> Vec<(String, String)> {
        four_sides(value).map_or_else(Vec::new, |vals| {
            SIDES.iter().zip(vals).map(|(s, v)| (format!("{prefix}{s}{suffix}"), v)).collect()
        })
    };
    match name {
        "margin" | "padding" => sides(&format!("{name}-"), ""),
        "inset" => sides("", ""),
        "margin-inline" | "padding-inline" => {
            let base = name.trim_end_matches("-inline");
            let t = tokens(value);
            let (a, b) = (t.first().copied().unwrap_or("0"), t.get(1).or(t.first()).copied().unwrap_or("0"));
            vec![(format!("{base}-left"), a.into()), (format!("{base}-right"), b.into())]
        }
        "margin-block" | "padding-block" => {
            let base = name.trim_end_matches("-block");
            let t = tokens(value);
            let (a, b) = (t.first().copied().unwrap_or("0"), t.get(1).or(t.first()).copied().unwrap_or("0"));
            vec![(format!("{base}-top"), a.into()), (format!("{base}-bottom"), b.into())]
        }
        "border-width" => sides("border-", "-width"),
        "border-style" => sides("border-", "-style"),
        "border-color" => sides("border-", "-color"),
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left" | "border-inline"
        | "border-block" => {
            let targets: Vec<&str> = match name {
                "border" => SIDES.to_vec(),
                "border-inline" => vec!["left", "right"],
                "border-block" => vec!["top", "bottom"],
                n => vec![n.trim_start_matches("border-")],
            };
            let (mut width, mut style, mut color) = ("medium".to_string(), "none".to_string(), "currentcolor".to_string());
            for t in tokens(value) {
                let lower = t.to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "none" | "hidden" | "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset"
                ) {
                    style = lower;
                } else if matches!(lower.as_str(), "thin" | "medium" | "thick") || css::parse_length(&lower).is_some() {
                    width = lower;
                } else {
                    color = lower;
                }
            }
            let mut out = Vec::new();
            for side in targets {
                out.push((format!("border-{side}-width"), width.clone()));
                out.push((format!("border-{side}-color"), color.clone()));
                out.push((format!("border-{side}-style"), style.clone()));
            }
            out
        }
        "background" => {
            let mut color = "transparent".to_string();
            let mut image = "none".to_string();
            // Only the last layer carries a color.
            let layers = css::split_top_level(value, ',');
            for t in tokens(layers.last().copied().unwrap_or(value)) {
                if t.starts_with("url(") {
                    image = t.to_string();
                } else if css::parse_color(t).is_some() || t.eq_ignore_ascii_case("currentcolor") {
                    color = t.to_string();
                }
            }
            if let Some(url) = layers.iter().find_map(|l| tokens(l).into_iter().find(|t| t.starts_with("url("))) {
                image = url.to_string();
            }
            vec![("background-color".into(), color), ("background-image".into(), image)]
        }
        "font" => expand_font(value),
        "list-style" => tokens(value)
            .into_iter()
            .find(|t| !t.starts_with("url(") && !matches!(*t, "inside" | "outside"))
            .map_or_else(Vec::new, |t| one("list-style-type", t)),
        "text-decoration" => one("text-decoration-line", value),
        "flex" => {
            let t = tokens(value);
            let (grow, shrink, basis) = match t.as_slice() {
                ["none"] => ("0", "0", "auto"),
                ["auto"] => ("1", "1", "auto"),
                ["initial"] => ("0", "1", "auto"),
                [g] if g.parse::<f32>().is_ok() => (*g, "1", "0"),
                [b] => ("1", "1", *b),
                [g, s] if s.parse::<f32>().is_ok() => (*g, *s, "0"),
                [g, b] => (*g, "1", *b),
                [g, s, b, ..] => (*g, *s, *b),
                [] => return Vec::new(),
            };
            vec![
                ("flex-grow".into(), grow.into()),
                ("flex-shrink".into(), shrink.into()),
                ("flex-basis".into(), basis.into()),
            ]
        }
        "flex-flow" => tokens(value)
            .into_iter()
            .map(|t| {
                let prop = if t.contains("wrap") { "flex-wrap" } else { "flex-direction" };
                (prop.to_string(), t.to_string())
            })
            .collect(),
        "gap" | "grid-gap" => {
            let t = tokens(value);
            let row = t.first().copied().unwrap_or("0");
            let col = t.get(1).copied().unwrap_or(row);
            vec![("row-gap".into(), row.into()), ("column-gap".into(), col.into())]
        }
        "place-items" => one("align-items", tokens(value).first().copied().unwrap_or("")),
        "inline-size" => one("width", value),
        "block-size" => one("height", value),
        "max-inline-size" => one("max-width", value),
        "min-inline-size" => one("min-width", value),
        "inset-inline-start" => one("left", value),
        "inset-inline-end" => one("right", value),
        "inset-block-start" => one("top", value),
        "inset-block-end" => one("bottom", value),
        _ => one(name, value),
    }
}

fn expand_font(value: &str) -> Vec<(String, String)> {
    let lower = value.to_ascii_lowercase();
    let mut out = vec![
        ("font-style".to_string(), "normal".to_string()),
        ("font-weight".to_string(), "normal".to_string()),
        ("line-height".to_string(), "normal".to_string()),
    ];
    let t = tokens(&lower);
    let mut i = 0;
    while i < t.len() {
        let tok = t[i];
        match tok {
            "italic" | "oblique" => out[0].1 = tok.to_string(),
            "bold" | "bolder" | "lighter" => out[1].1 = tok.to_string(),
            "normal" | "small-caps" => {}
            _ if tok.parse::<u32>().is_ok() => out[1].1 = tok.to_string(),
            _ => break,
        }
        i += 1;
    }
    let Some(size_tok) = t.get(i) else { return Vec::new() };
    let (size, line_height) = size_tok.split_once('/').unwrap_or((size_tok, ""));
    out.push(("font-size".into(), size.to_string()));
    if !line_height.is_empty() {
        out[2].1 = line_height.to_string();
    }
    let rest = &t[i + 1..];
    let rest: Vec<&str> = if rest.first() == Some(&"/") {
        if let Some(lh) = rest.get(1) {
            out[2].1 = lh.to_string();
        }
        rest[2.min(rest.len())..].to_vec()
    } else {
        rest.to_vec()
    };
    if !rest.is_empty() {
        out.push(("font-family".into(), rest.join(" ")));
    }
    out
}

/// Turns legacy HTML attributes like `bgcolor` and `width` into declarations.
fn presentational_hints(doc: &Document, node: NodeId) -> Vec<Declaration> {
    let el = doc.element(node).expect("element");
    let mut out = Vec::new();
    let mut add = |name: &str, value: String| {
        out.push(Declaration { name: name.to_string(), value, important: false });
    };
    let dim = |v: &str| -> Option<String> {
        let v = v.trim();
        if let Some(p) = v.strip_suffix('%') {
            return p.trim().parse::<f32>().ok().map(|p| format!("{p}%"));
        }
        let n: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        n.parse::<f32>().ok().map(|n| format!("{n}px"))
    };
    let tag = el.tag.as_str();
    if let Some(c) = el.attr("bgcolor") {
        add("background-color", legacy_color(c));
    }
    if matches!(tag, "img" | "table" | "td" | "th" | "col" | "hr" | "canvas" | "video" | "iframe" | "embed" | "object" | "input" | "svg") {
        if let Some(w) = el.attr("width").and_then(dim) {
            add("width", w);
        }
        if let Some(h) = el.attr("height").and_then(dim)
            && (tag != "td" && tag != "th" || !h.ends_with('%')) {
                add("height", h);
            }
    }
    if matches!(tag, "td" | "th" | "tr")
        && let Some(h) = el.attr("height").and_then(dim) {
            add("height", h);
        }
    if let Some(align) = el.attr("align").map(str::to_ascii_lowercase) {
        match tag {
            "table" if align == "center" => {
                add("margin-left", "auto".into());
                add("margin-right", "auto".into());
            }
            "img" | "table" if align == "left" || align == "right" => add("float", align),
            "img" => {}
            _ if align == "center" => add("text-align", "-webkit-center".into()),
            _ => add("text-align", align),
        }
    }
    if let Some(v) = el.attr("valign") {
        add("vertical-align", v.to_ascii_lowercase());
    }
    if tag == "font" {
        if let Some(c) = el.attr("color") {
            add("color", legacy_color(c));
        }
        if let Some(face) = el.attr("face") {
            add("font-family", face.to_string());
        }
        if let Some(size) = el.attr("size") {
            let n: i32 = match size.trim() {
                s if s.starts_with('+') => 3 + s[1..].parse::<i32>().unwrap_or(0),
                s if s.starts_with('-') => 3 - s[1..].parse::<i32>().unwrap_or(0),
                s => s.parse().unwrap_or(3),
            };
            let size = ["x-small", "x-small", "small", "medium", "large", "x-large", "xx-large", "xxx-large"]
                [n.clamp(0, 7) as usize];
            add("font-size", size.into());
        }
    }
    if tag == "body"
        && let Some(c) = el.attr("text") {
            add("color", legacy_color(c));
        }
    if tag == "table" {
        if let Some(b) = el.attr("border") {
            let b = b.trim().parse::<f32>().unwrap_or(1.0);
            if b > 0.0 {
                add("border", format!("{b}px outset gray"));
            }
        }
        if let Some(s) = el.attr("cellspacing").and_then(dim) {
            add("border-spacing", s);
        }
    }
    if matches!(tag, "td" | "th") {
        if let Some(table) = doc.ancestors(node).find(|&a| doc.tag(a) == "table") {
            let table_el = doc.element(table).expect("element");
            if let Some(p) = table_el.attr("cellpadding").and_then(dim) {
                add("padding", p);
            }
            if table_el.attr("border").is_some_and(|b| b.trim() != "0") {
                add("border", "1px inset gray".into());
            }
        }
        if el.attr("nowrap").is_some() {
            add("white-space", "nowrap".into());
        }
    }
    if matches!(tag, "ul" | "ol")
        && let Some(t) = el.attr("type") {
            let v = match t {
                "1" => "decimal",
                "a" => "lower-alpha",
                "A" => "upper-alpha",
                "i" => "lower-roman",
                "I" => "upper-roman",
                other => other,
            };
            add("list-style-type", v.to_ascii_lowercase());
        }
    if tag == "input"
        && let Some(size) = el.attr("size").and_then(|s| s.parse::<f32>().ok())
            && el.attr("width").is_none() {
                add("width", format!("{:.1}em", size * 0.55 + 0.4));
            }
    out
}

/// Legacy color attributes allow bare hex digits without `#`.
fn legacy_color(v: &str) -> String {
    let v = v.trim();
    if v.len() == 6 && v.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("#{v}")
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html;

    fn style_of(markup: &str, css: &str, tag: &str) -> Style {
        let doc = html::parse(markup);
        let sheets = vec![css::parse_stylesheet(css)];
        let ctx = StyleContext { zoom: 1.0, viewport_width: 1000.0, viewport_height: 800.0 };
        let styles = compute_styles(&doc, &sheets, &ctx);
        let node = doc.find_tag(tag).unwrap();
        (*styles[node]).clone()
    }

    #[test]
    fn cascade_order() {
        let s = style_of(
            r#"<p id="x" class="c" style="margin-left: 3px">t</p>"#,
            "p { color: red } .c { color: blue } #x { color: green } p { margin-left: 9px !important }",
            "p",
        );
        assert_eq!(s.color, Color::rgb(0, 128, 0));
        assert_eq!(s.margin[3], Length::Px(9.0));
        assert_eq!(s.margin[0], Length::Px(16.0));
    }

    #[test]
    fn inherits_and_resolves_em() {
        let s = style_of("<div><span>t</span></div>", "div { font-size: 20px; color: #123456 } span { padding: 0.5em 1em }", "span");
        assert_eq!(s.font_size, 20.0);
        assert_eq!(s.color, Color::rgb(0x12, 0x34, 0x56));
        assert_eq!(s.padding, [Length::Px(10.0), Length::Px(20.0), Length::Px(10.0), Length::Px(20.0)]);
    }

    #[test]
    fn custom_properties() {
        let s = style_of(
            "<div><p>t</p></div>",
            ":root { --accent: #ff0000; --gap: 4px } p { color: var(--accent); margin-top: var(--missing, var(--gap)) }",
            "p",
        );
        assert_eq!(s.color, Color::rgb(255, 0, 0));
        assert_eq!(s.margin[0], Length::Px(4.0));
    }

    #[test]
    fn shorthands() {
        let s = style_of("<div>t</div>", "div { border: 2px solid red; background: #eee url(x.png); font: bold 12px/1.5 monospace }", "div");
        assert_eq!(s.border_width, [2.0; 4]);
        assert_eq!(s.border_color[0], Some(Color::rgb(255, 0, 0)));
        assert_eq!(s.background, Color::rgb(0xee, 0xee, 0xee));
        assert_eq!(s.background_image.as_deref(), Some("x.png"));
        assert!(s.bold);
        assert_eq!(s.font_size, 12.0);
        assert_eq!(s.line_height, LineHeight::Factor(1.5));
        assert_eq!(s.family, Family::Mono);
    }

    #[test]
    fn media_queries_apply() {
        let narrow = style_of("<p>t</p>", "@media (max-width: 600px) { p { color: red } }", "p");
        assert_eq!(narrow.color, Color::BLACK);
    }
}
