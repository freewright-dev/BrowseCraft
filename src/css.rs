//! CSS parsing: stylesheets, selectors, declarations and the value types the
//! style system understands.

use crate::dom::{Document, NodeId};

#[derive(Default)]
#[derive(Clone, Debug)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    /// URLs from `@import`, in order.
    pub imports: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
    /// The `@media` query this rule sits under, if any.
    pub media: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Declaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// A complex selector stored right to left: `parts[0]` is the subject, and
/// each later part is reached from the previous one through its combinator.
#[derive(Clone, Debug)]
pub struct Selector {
    pub parts: Vec<(Compound, Combinator)>,
    pub specificity: (u16, u16, u16),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Combinator {
    /// Only on the subject.
    None,
    Descendant,
    Child,
    NextSibling,
    LaterSibling,
}

#[derive(Clone, Debug, Default)]
pub struct Compound {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrSelector>,
    pub pseudos: Vec<Pseudo>,
}

#[derive(Clone, Debug)]
pub struct AttrSelector {
    pub name: String,
    pub op: Option<(char, String)>,
}

#[derive(Clone, Debug)]
pub enum Pseudo {
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    NthChild(i32, i32),
    Root,
    Link,
    Empty,
    Not(Vec<Selector>),
    Is(Vec<Selector>),
    /// Hover, focus, visited, unknown pseudo-classes and pseudo-elements.
    Never,
}

pub fn parse_stylesheet(source: &str) -> Stylesheet {
    let source = strip_comments(source);
    let mut sheet = Stylesheet::default();
    parse_rules(&source, None, &mut sheet);
    sheet
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        rest = rest[i + 2..]
            .find("*/")
            .map_or("", |e| &rest[i + 2 + e + 2..]);
    }
    out.push_str(rest);
    out
}

/// Finds the `}` that closes a block whose `{` is just before `s`.
fn block_end(s: &str) -> usize {
    let mut depth = 1;
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '{') => depth += 1,
            (None, '}') => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    s.len()
}

fn parse_rules(mut s: &str, media: Option<&str>, sheet: &mut Stylesheet) {
    loop {
        s = s.trim_start();
        if s.is_empty() {
            return;
        }
        if s.starts_with('}') || s.starts_with(';') {
            s = &s[1..];
            continue;
        }
        if s.starts_with('@') {
            let brace = s.find('{');
            let semi = s.find(';');
            if semi.is_some_and(|semi| brace.is_none_or(|b| semi < b)) {
                let statement = &s[..semi.unwrap()];
                if let Some(url) = statement.strip_prefix("@import")
                    && let Some(url) = import_url(url)
                {
                    sheet.imports.push(url);
                }
                s = &s[semi.unwrap() + 1..];
                continue;
            }
            let Some(brace) = brace else { return };
            let prelude = s[1..brace].trim();
            let body_len = block_end(&s[brace + 1..]);
            let body = &s[brace + 1..brace + 1 + body_len];
            let (keyword, condition) = prelude
                .split_once(|c: char| c.is_whitespace() || c == '(')
                .map_or((prelude, ""), |(k, _)| (k, prelude[k.len()..].trim()));
            match keyword.to_ascii_lowercase().as_str() {
                "media" => {
                    let combined = match media {
                        Some(outer) => format!("{outer}&&{condition}"),
                        None => condition.to_string(),
                    };
                    parse_rules(body, Some(&combined), sheet);
                }
                "supports" if !condition.trim_start().starts_with("not") => {
                    parse_rules(body, media, sheet)
                }
                "layer" | "document" | "-moz-document" | "container" | "scope" => {
                    parse_rules(body, media, sheet)
                }
                _ => {}
            }
            s = &s[(brace + 1 + body_len + 1).min(s.len())..];
            continue;
        }
        let Some(brace) = s.find('{') else { return };
        let prelude = &s[..brace];
        let body_len = block_end(&s[brace + 1..]);
        let body = &s[brace + 1..brace + 1 + body_len];
        s = &s[(brace + 1 + body_len + 1).min(s.len())..];

        let selectors: Vec<Selector> = split_top_level(prelude, ',')
            .into_iter()
            .filter_map(|sel| parse_selector(sel.trim()))
            .collect();
        if selectors.is_empty() {
            continue;
        }
        let declarations = parse_declarations(body);
        if !declarations.is_empty() {
            sheet.rules.push(Rule {
                selectors,
                declarations,
                media: media.map(str::to_string),
            });
        }
    }
}

fn import_url(s: &str) -> Option<String> {
    let s = s.trim();
    let s = s
        .strip_prefix("url(")
        .map_or(s, |r| r.split(')').next().unwrap_or(""));
    let url = s
        .trim()
        .split(|c: char| c.is_whitespace())
        .next()?
        .trim_matches(['"', '\'', ')']);
    (!url.is_empty()).then(|| url.to_string())
}

/// Splits on `sep` outside parentheses, brackets and quotes.
pub fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quote = None;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '[') => depth += 1,
            (None, ')' | ']') => depth -= 1,
            (None, c) if c == sep && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

pub fn parse_declarations(s: &str) -> Vec<Declaration> {
    split_top_level(s, ';')
        .into_iter()
        .filter_map(|decl| {
            let (name, value) = decl.split_once(':')?;
            let name = name.trim();
            let name = if name.starts_with("--") {
                name.to_string()
            } else {
                name.to_ascii_lowercase()
            };
            let mut value = value.trim();
            let mut important = false;
            if let Some(i) = value.rfind('!')
                && value[i + 1..].trim().eq_ignore_ascii_case("important")
            {
                important = true;
                value = value[..i].trim_end();
            }
            (!name.is_empty() && !value.is_empty() && !value.contains('{')).then(|| Declaration {
                name,
                value: value.to_string(),
                important,
            })
        })
        .collect()
}

pub fn parse_selector(s: &str) -> Option<Selector> {
    let mut parts: Vec<(Compound, Combinator)> = Vec::new();
    let mut pending = Combinator::None;
    let mut rest = s.trim();
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let (compound, after) = parse_compound(rest)?;
        parts.push((compound, pending));
        let trimmed = after.trim_start();
        let had_space = trimmed.len() != after.len();
        rest = trimmed;
        pending = match rest.chars().next() {
            Some('>') => Combinator::Child,
            Some('+') => Combinator::NextSibling,
            Some('~') => Combinator::LaterSibling,
            Some(_) if had_space => Combinator::Descendant,
            Some(_) => return None,
            None => break,
        };
        if pending != Combinator::Descendant {
            rest = rest[1..].trim_start();
        }
    }
    // Store right to left, with each part carrying the combinator that
    // links it to the part before it in this order.
    let mut out = Vec::with_capacity(parts.len());
    let mut combinator = Combinator::None;
    for (compound, comb) in parts.into_iter().rev() {
        out.push((compound, combinator));
        combinator = comb;
    }
    let specificity = out.iter().fold((0, 0, 0), |acc, (c, _)| {
        add_spec(acc, compound_specificity(c))
    });
    Some(Selector {
        parts: out,
        specificity,
    })
}

fn add_spec(a: (u16, u16, u16), b: (u16, u16, u16)) -> (u16, u16, u16) {
    (a.0 + b.0, a.1 + b.1, a.2 + b.2)
}

fn compound_specificity(c: &Compound) -> (u16, u16, u16) {
    let mut spec = (
        c.id.is_some() as u16,
        (c.classes.len() + c.attrs.len()) as u16,
        c.tag.as_ref().is_some_and(|t| t != "*") as u16,
    );
    for p in &c.pseudos {
        spec = match p {
            Pseudo::Not(list) | Pseudo::Is(list) => add_spec(
                spec,
                list.iter().map(|s| s.specificity).max().unwrap_or_default(),
            ),
            _ => add_spec(spec, (0, 1, 0)),
        };
    }
    spec
}

fn ident_end(s: &str) -> usize {
    s.find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '\\' || !c.is_ascii()))
        .unwrap_or(s.len())
}

fn parse_compound(s: &str) -> Option<(Compound, &str)> {
    let mut c = Compound::default();
    let mut rest = s;
    let start_len = s.len();
    if rest.starts_with('*') {
        rest = &rest[1..];
    } else {
        let end = ident_end(rest);
        if end > 0 {
            c.tag = Some(rest[..end].to_ascii_lowercase());
            rest = &rest[end..];
        }
    }
    loop {
        let Some(first) = rest.chars().next() else {
            break;
        };
        match first {
            '#' => {
                let end = ident_end(&rest[1..]) + 1;
                c.id = Some(unescape(&rest[1..end]));
                rest = &rest[end..];
            }
            '.' => {
                let end = ident_end(&rest[1..]) + 1;
                if end == 1 {
                    return None;
                }
                c.classes.push(unescape(&rest[1..end]));
                rest = &rest[end..];
            }
            '[' => {
                let end = rest.find(']')?;
                c.attrs.push(parse_attr_selector(&rest[1..end])?);
                rest = &rest[end + 1..];
            }
            ':' => {
                let element = rest.starts_with("::");
                let name_start = if element { 2 } else { 1 };
                let name_end = name_start + ident_end(&rest[name_start..]);
                let name = rest[name_start..name_end].to_ascii_lowercase();
                rest = &rest[name_end..];
                let mut arg = None;
                if rest.starts_with('(') {
                    let close = matching_paren(rest)?;
                    arg = Some(&rest[1..close]);
                    rest = &rest[close + 1..];
                }
                c.pseudos.push(if element {
                    Pseudo::Never
                } else {
                    parse_pseudo(&name, arg)
                });
            }
            _ => break,
        }
    }
    (rest.len() != start_len).then_some((c, rest))
}

fn unescape(s: &str) -> String {
    s.replace('\\', "")
}

fn matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_attr_selector(s: &str) -> Option<AttrSelector> {
    let s = s.trim();
    let Some(eq) = s.find('=') else {
        return Some(AttrSelector {
            name: s.to_ascii_lowercase(),
            op: None,
        });
    };
    let (name, op) = match s[..eq].chars().last() {
        Some(c @ ('~' | '|' | '^' | '$' | '*')) => (&s[..eq - 1], c),
        _ => (&s[..eq], '='),
    };
    let mut value = s[eq + 1..].trim();
    // Drop a trailing case-sensitivity flag.
    if let Some(stripped) = value
        .strip_suffix(" i")
        .or_else(|| value.strip_suffix(" s"))
    {
        value = stripped.trim();
    }
    let value = value.trim_matches(['"', '\'']).to_string();
    Some(AttrSelector {
        name: name.trim().to_ascii_lowercase(),
        op: Some((op, value)),
    })
}

fn parse_pseudo(name: &str, arg: Option<&str>) -> Pseudo {
    let list = |arg: Option<&str>| -> Vec<Selector> {
        arg.map(|a| {
            split_top_level(a, ',')
                .into_iter()
                .filter_map(|s| parse_selector(s.trim()))
                .collect()
        })
        .unwrap_or_default()
    };
    match name {
        "first-child" => Pseudo::FirstChild,
        "last-child" => Pseudo::LastChild,
        "only-child" => Pseudo::OnlyChild,
        "first-of-type" => Pseudo::FirstOfType,
        "last-of-type" => Pseudo::LastOfType,
        "root" => Pseudo::Root,
        "link" | "any-link" => Pseudo::Link,
        "empty" => Pseudo::Empty,
        "not" => Pseudo::Not(list(arg)),
        "is" | "where" | "matches" | "-webkit-any" => Pseudo::Is(list(arg)),
        "nth-child" => match arg.and_then(parse_nth) {
            Some((a, b)) => Pseudo::NthChild(a, b),
            None => Pseudo::Never,
        },
        _ => Pseudo::Never,
    }
}

/// Parses `an+b`, `odd`, `even` or a number.
fn parse_nth(s: &str) -> Option<(i32, i32)> {
    let s: String = s
        .split(" of ")
        .next()?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    match s.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    match s.split_once('n') {
        Some((a, b)) => {
            let a = match a {
                "" | "+" => 1,
                "-" => -1,
                a => a.parse().ok()?,
            };
            let b = if b.is_empty() { 0 } else { b.parse().ok()? };
            Some((a, b))
        }
        None => Some((0, s.parse().ok()?)),
    }
}

impl Selector {
    pub fn matches(&self, doc: &Document, node: NodeId) -> bool {
        matches_from(&self.parts, doc, node)
    }

    /// The most selective key of the subject, for rule bucketing.
    pub fn key(&self) -> SelectorKey {
        let (subject, _) = &self.parts[0];
        if let Some(id) = &subject.id {
            SelectorKey::Id(id.clone())
        } else if let Some(class) = subject.classes.first() {
            SelectorKey::Class(class.clone())
        } else if let Some(tag) = &subject.tag {
            SelectorKey::Tag(tag.clone())
        } else {
            SelectorKey::Universal
        }
    }
}

#[derive(Hash, PartialEq, Eq)]
pub enum SelectorKey {
    Id(String),
    Class(String),
    Tag(String),
    Universal,
}

fn matches_from(parts: &[(Compound, Combinator)], doc: &Document, node: NodeId) -> bool {
    let Some(((compound, _), rest)) = parts.split_first() else {
        return true;
    };
    if !compound_matches(compound, doc, node) {
        return false;
    }
    let Some((_, combinator)) = rest.first() else {
        return true;
    };
    match combinator {
        Combinator::None => true,
        Combinator::Child => doc
            .parent_element(node)
            .is_some_and(|p| matches_from(rest, doc, p)),
        Combinator::Descendant => doc
            .ancestors(node)
            .filter(|&a| doc.element(a).is_some())
            .any(|a| matches_from(rest, doc, a)),
        Combinator::NextSibling => doc
            .previous_elements(node)
            .next()
            .is_some_and(|s| matches_from(rest, doc, s)),
        Combinator::LaterSibling => doc
            .previous_elements(node)
            .any(|s| matches_from(rest, doc, s)),
    }
}

fn compound_matches(c: &Compound, doc: &Document, node: NodeId) -> bool {
    let Some(el) = doc.element(node) else {
        return false;
    };
    if c.tag.as_ref().is_some_and(|t| *t != el.tag) {
        return false;
    }
    if c.id.as_ref().is_some_and(|id| el.id() != Some(id)) {
        return false;
    }
    if !c.classes.iter().all(|class| el.has_class(class)) {
        return false;
    }
    for a in &c.attrs {
        let Some(value) = el.attr(&a.name) else {
            return false;
        };
        let ok = match &a.op {
            None => true,
            Some(('=', v)) => value == v,
            Some(('~', v)) => value.split_ascii_whitespace().any(|w| w == v),
            Some(('|', v)) => value == v || value.starts_with(&format!("{v}-")),
            Some(('^', v)) => !v.is_empty() && value.starts_with(v.as_str()),
            Some(('$', v)) => !v.is_empty() && value.ends_with(v.as_str()),
            Some(('*', v)) => !v.is_empty() && value.contains(v.as_str()),
            Some(_) => false,
        };
        if !ok {
            return false;
        }
    }
    c.pseudos.iter().all(|p| pseudo_matches(p, doc, node))
}

fn pseudo_matches(p: &Pseudo, doc: &Document, node: NodeId) -> bool {
    let tag = doc.tag(node);
    match p {
        Pseudo::FirstChild => doc.previous_elements(node).next().is_none(),
        Pseudo::LastChild => doc.next_elements(node).next().is_none(),
        Pseudo::OnlyChild => {
            doc.previous_elements(node).next().is_none() && doc.next_elements(node).next().is_none()
        }
        Pseudo::FirstOfType => !doc.previous_elements(node).any(|s| doc.tag(s) == tag),
        Pseudo::LastOfType => !doc.next_elements(node).any(|s| doc.tag(s) == tag),
        Pseudo::NthChild(a, b) => {
            let index = doc.previous_elements(node).count() as i32 + 1;
            match a {
                0 => index == *b,
                a => (index - b) % a == 0 && (index - b) / a >= 0,
            }
        }
        Pseudo::Root => tag == "html",
        Pseudo::Link => tag == "a" && doc.element(node).is_some_and(|e| e.attr("href").is_some()),
        Pseudo::Empty => doc.nodes[node].children.is_empty(),
        Pseudo::Not(list) => !list.iter().any(|s| s.matches(doc, node)),
        Pseudo::Is(list) => list.iter().any(|s| s.matches(doc, node)),
        Pseudo::Never => false,
    }
}

/// Evaluates a media query list against the viewport width in CSS pixels.
/// Nested `@media` blocks are joined with `&&`.
pub fn media_matches(query: &str, viewport_width: f32) -> bool {
    query.split("&&").all(|q| {
        split_top_level(q, ',')
            .iter()
            .any(|q| single_media_matches(q, viewport_width))
    })
}

fn single_media_matches(query: &str, width: f32) -> bool {
    let q = query.trim().to_ascii_lowercase();
    let (negate, q) = match q.strip_prefix("not ") {
        Some(rest) => (true, rest.trim().to_string()),
        None => (
            false,
            q.strip_prefix("only ").unwrap_or(&q).trim().to_string(),
        ),
    };
    let mut result = true;
    for part in q.split(" and ") {
        let part = part.trim();
        let ok = if let Some(feature) = part.strip_prefix('(').and_then(|p| p.strip_suffix(')')) {
            media_feature(feature, width)
        } else {
            matches!(part, "" | "all" | "screen")
        };
        result &= ok;
    }
    result != negate
}

fn media_feature(feature: &str, width: f32) -> bool {
    let px = |v: &str| -> Option<f32> {
        let v = v.trim();
        if let Some(n) = v.strip_suffix("px") {
            n.trim().parse().ok()
        } else if let Some(n) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
            n.trim().parse::<f32>().ok().map(|n| n * 16.0)
        } else {
            v.parse().ok()
        }
    };
    // Range syntax: (width >= 600px), (400px <= width <= 800px).
    if feature.contains(['<', '>']) && !feature.contains(':') {
        return range_feature(feature, width, &px);
    }
    let (name, value) = feature
        .split_once(':')
        .map_or((feature.trim(), ""), |(n, v)| (n.trim(), v.trim()));
    match name {
        "min-width" => px(value).is_some_and(|v| width >= v),
        "max-width" => px(value).is_some_and(|v| width <= v),
        "prefers-color-scheme" => value == "light",
        "prefers-reduced-motion" => value == "no-preference",
        "hover" | "any-hover" => value == "hover",
        "pointer" | "any-pointer" => value == "fine",
        "orientation" => value == "landscape",
        "color" | "min-resolution" | "-webkit-min-device-pixel-ratio" => true,
        _ => false,
    }
}

fn range_feature(s: &str, width: f32, px: &dyn Fn(&str) -> Option<f32>) -> bool {
    // Normalise to a list of (value, op) comparisons against `width`.
    let s = s.replace("width", " W ");
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '<' || c == '>' {
            if !cur.trim().is_empty() {
                tokens.push(cur.trim().to_string());
            }
            cur.clear();
            let mut op = c.to_string();
            if chars.get(i + 1) == Some(&'=') {
                op.push('=');
                i += 1;
            }
            tokens.push(op);
        } else {
            cur.push(c);
        }
        i += 1;
    }
    if !cur.trim().is_empty() {
        tokens.push(cur.trim().to_string());
    }
    let value = |t: &str| if t == "W" { Some(width) } else { px(t) };
    tokens
        .windows(3)
        .step_by(2)
        .all(|w| match (value(&w[0]), value(&w[2])) {
            (Some(a), Some(b)) => match w[1].as_str() {
                "<" => a < b,
                "<=" => a <= b,
                ">" => a > b,
                ">=" => a >= b,
                _ => false,
            },
            _ => false,
        })
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const TRANSPARENT: Color = Color {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Px(f32),
    Em(f32),
    Rem(f32),
    Percent(f32),
    Vw(f32),
    Vh(f32),
    Auto,
}

pub fn parse_length(s: &str) -> Option<Length> {
    let s = s.trim().to_ascii_lowercase();
    if s == "auto" {
        return Some(Length::Auto);
    }
    if let Some(inner) = s.strip_prefix("calc(").and_then(|r| r.strip_suffix(')')) {
        return parse_calc(inner);
    }
    let (num, unit) = split_number(&s);
    let n: f32 = num.parse().ok()?;
    Some(match unit {
        "px" => Length::Px(n),
        "" if n == 0.0 => Length::Px(0.0),
        "em" => Length::Em(n),
        "rem" => Length::Rem(n),
        "ex" | "ch" => Length::Em(n * 0.5),
        "%" => Length::Percent(n),
        "pt" => Length::Px(n * 4.0 / 3.0),
        "pc" => Length::Px(n * 16.0),
        "in" => Length::Px(n * 96.0),
        "cm" => Length::Px(n * 96.0 / 2.54),
        "mm" => Length::Px(n * 96.0 / 25.4),
        "vw" | "svw" | "lvw" | "dvw" => Length::Vw(n),
        "vh" | "svh" | "lvh" | "dvh" => Length::Vh(n),
        "vmin" => Length::Vw(n),
        "vmax" => Length::Vw(n),
        _ => return None,
    })
}

/// Splits `12.5px` into `("12.5", "px")`, including exponents like `1e3`.
pub fn split_number(s: &str) -> (&str, &str) {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && matches!(b[i], b'+' | b'-') {
        i += 1;
    }
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
        i += 1;
    }
    if i < b.len() && matches!(b[i], b'e' | b'E') {
        let mut j = i + 1;
        if j < b.len() && matches!(b[j], b'+' | b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    s.split_at(i)
}

/// Handles the common one-term and same-unit cases of `calc()`. Anything
/// mixing units falls back to its first term.
fn parse_calc(s: &str) -> Option<Length> {
    let s = s.trim();
    let s = s
        .strip_prefix("calc(")
        .and_then(|r| r.strip_suffix(')'))
        .unwrap_or(s);
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let mut acc = parse_length(tokens.first()?)?;
    let mut i = 1;
    while i + 1 < tokens.len() {
        let op = tokens[i];
        let rhs = tokens[i + 1];
        acc = match (op, acc, parse_length(rhs), rhs.parse::<f32>().ok()) {
            ("*", l, _, Some(k)) => scale_length(l, k),
            ("/", l, _, Some(k)) if k != 0.0 => scale_length(l, 1.0 / k),
            ("+", Length::Px(a), Some(Length::Px(b)), _) => Length::Px(a + b),
            ("-", Length::Px(a), Some(Length::Px(b)), _) => Length::Px(a - b),
            ("+", Length::Percent(a), Some(Length::Percent(b)), _) => Length::Percent(a + b),
            ("-", Length::Percent(a), Some(Length::Percent(b)), _) => Length::Percent(a - b),
            ("+", Length::Em(a), Some(Length::Em(b)), _) => Length::Em(a + b),
            ("+" | "-", l, _, _) => l,
            _ => return None,
        };
        i += 2;
    }
    Some(acc)
}

fn scale_length(l: Length, k: f32) -> Length {
    match l {
        Length::Px(v) => Length::Px(v * k),
        Length::Em(v) => Length::Em(v * k),
        Length::Rem(v) => Length::Rem(v * k),
        Length::Percent(v) => Length::Percent(v * k),
        Length::Vw(v) => Length::Vw(v * k),
        Length::Vh(v) => Length::Vh(v * k),
        Length::Auto => Length::Auto,
    }
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
        let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        return match hex.len() {
            3 | 4 => Some(Color {
                r: digit(0)? * 17,
                g: digit(1)? * 17,
                b: digit(2)? * 17,
                a: if hex.len() == 4 { digit(3)? * 17 } else { 255 },
            }),
            6 | 8 => Some(Color {
                r: pair(0)?,
                g: pair(2)?,
                b: pair(4)?,
                a: if hex.len() == 8 { pair(6)? } else { 255 },
            }),
            _ => None,
        };
    }
    if let Some(args) = s
        .strip_prefix("light-dark(")
        .and_then(|r| r.strip_suffix(')'))
    {
        return parse_color(split_top_level(args, ',').first()?);
    }
    if let Some(open) = s.find('(') {
        let func = &s[..open];
        let args = s[open + 1..].trim_end_matches(')');
        let args: Vec<&str> = args
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|a| !a.is_empty())
            .collect();
        let channel = |v: &str| -> Option<f32> {
            match v.strip_suffix('%') {
                Some(p) => p.parse::<f32>().ok().map(|p| p * 2.55),
                None => v.parse().ok(),
            }
        };
        let alpha = |v: Option<&&str>| -> Option<u8> {
            let Some(v) = v else { return Some(255) };
            let a = match v.strip_suffix('%') {
                Some(p) => p.parse::<f32>().ok()? / 100.0,
                None => v.parse::<f32>().ok()?,
            };
            Some((a.clamp(0.0, 1.0) * 255.0).round() as u8)
        };
        return match func {
            "rgb" | "rgba" if args.len() >= 3 => Some(Color {
                r: channel(args[0])?.clamp(0.0, 255.0) as u8,
                g: channel(args[1])?.clamp(0.0, 255.0) as u8,
                b: channel(args[2])?.clamp(0.0, 255.0) as u8,
                a: alpha(args.get(3))?,
            }),
            "hsl" | "hsla" if args.len() >= 3 => {
                let h = args[0].trim_end_matches("deg").parse::<f32>().ok()?;
                let sat = args[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
                let l = args[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
                let (r, g, b) = hsl_to_rgb(h, sat, l);
                Some(Color {
                    r,
                    g,
                    b,
                    a: alpha(args.get(3))?,
                })
            }
            _ => None,
        };
    }
    named_color(&s)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0) / 360.0;
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

fn named_color(name: &str) -> Option<Color> {
    let hex: u32 = match name {
        "transparent" => return Some(Color::TRANSPARENT),
        "black" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "green" => 0x008000,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "orange" => 0xffa500,
        "purple" => 0x800080,
        "gray" | "grey" => 0x808080,
        "silver" => 0xc0c0c0,
        "maroon" => 0x800000,
        "olive" => 0x808000,
        "lime" => 0x00ff00,
        "aqua" | "cyan" => 0x00ffff,
        "teal" => 0x008080,
        "navy" => 0x000080,
        "fuchsia" | "magenta" => 0xff00ff,
        "pink" => 0xffc0cb,
        "brown" => 0xa52a2a,
        "gold" => 0xffd700,
        "indigo" => 0x4b0082,
        "violet" => 0xee82ee,
        "darkgray" | "darkgrey" => 0xa9a9a9,
        "lightgray" | "lightgrey" => 0xd3d3d3,
        "dimgray" | "dimgrey" => 0x696969,
        "gainsboro" => 0xdcdcdc,
        "whitesmoke" => 0xf5f5f5,
        "darkred" => 0x8b0000,
        "darkgreen" => 0x006400,
        "darkblue" => 0x00008b,
        "darkorange" => 0xff8c00,
        "lightblue" => 0xadd8e6,
        "lightgreen" => 0x90ee90,
        "lightyellow" => 0xffffe0,
        "skyblue" => 0x87ceeb,
        "steelblue" => 0x4682b4,
        "royalblue" => 0x4169e1,
        "dodgerblue" => 0x1e90ff,
        "slategray" | "slategrey" => 0x708090,
        "crimson" => 0xdc143c,
        "tomato" => 0xff6347,
        "coral" => 0xff7f50,
        "salmon" => 0xfa8072,
        "khaki" => 0xf0e68c,
        "beige" => 0xf5f5dc,
        "ivory" => 0xfffff0,
        "linen" => 0xfaf0e6,
        "lavender" => 0xe6e6fa,
        "tan" => 0xd2b48c,
        "chocolate" => 0xd2691e,
        "firebrick" => 0xb22222,
        "forestgreen" => 0x228b22,
        "seagreen" => 0x2e8b57,
        "turquoise" => 0x40e0d0,
        "orchid" => 0xda70d6,
        "plum" => 0xdda0dd,
        "aliceblue" => 0xf0f8ff,
        "ghostwhite" => 0xf8f8ff,
        "honeydew" => 0xf0fff0,
        "mintcream" => 0xf5fffa,
        "azure" => 0xf0ffff,
        "snow" => 0xfffafa,
        "seashell" => 0xfff5ee,
        "oldlace" => 0xfdf5e6,
        "cornsilk" => 0xfff8dc,
        "lemonchiffon" => 0xfffacd,
        "wheat" => 0xf5deb3,
        "darkslategray" | "darkslategrey" => 0x2f4f4f,
        "midnightblue" => 0x191970,
        "cornflowerblue" => 0x6495ed,
        "cadetblue" => 0x5f9ea0,
        "darkcyan" => 0x008b8b,
        "goldenrod" => 0xdaa520,
        "darkgoldenrod" => 0xb8860b,
        "sienna" => 0xa0522d,
        "peru" => 0xcd853f,
        "rebeccapurple" => 0x663399,
        "hotpink" => 0xff69b4,
        "deeppink" => 0xff1493,
        "limegreen" => 0x32cd32,
        "yellowgreen" => 0x9acd32,
        "olivedrab" => 0x6b8e23,
        "darkolivegreen" => 0x556b2f,
        "mediumblue" => 0x0000cd,
        "lightsteelblue" => 0xb0c4de,
        "powderblue" => 0xb0e0e6,
        "lightcyan" => 0xe0ffff,
        "lightpink" => 0xffb6c1,
        "lightcoral" => 0xf08080,
        "lightsalmon" => 0xffa07a,
        "lightslategray" | "lightslategrey" => 0x778899,
        "mistyrose" => 0xffe4e1,
        "papayawhip" => 0xffefd5,
        "peachpuff" => 0xffdab9,
        "moccasin" => 0xffe4b5,
        "navajowhite" => 0xffdead,
        "antiquewhite" => 0xfaebd7,
        "bisque" => 0xffe4c4,
        "blanchedalmond" => 0xffebcd,
        "floralwhite" => 0xfffaf0,
        "lavenderblush" => 0xfff0f5,
        _ => return None,
    };
    Some(Color::rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8))
}

/// Pulls the URL out of `url(...)`.
pub fn parse_url(s: &str) -> Option<String> {
    let start = s.find("url(")? + 4;
    let end = start + s[start..].find(')')?;
    let url = s[start..end].trim().trim_matches(['"', '\'']);
    (!url.is_empty()).then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html;

    #[test]
    fn parses_rules_and_media() {
        let sheet = parse_stylesheet(
            "/* c */ a, .b { color: red !important; margin: 0 } @media (max-width: 600px) { p { x: y } }",
        );
        assert_eq!(sheet.rules.len(), 2);
        assert_eq!(sheet.rules[0].selectors.len(), 2);
        assert!(sheet.rules[0].declarations[0].important);
        assert_eq!(sheet.rules[1].media.as_deref(), Some("(max-width: 600px)"));
        assert!(media_matches("(max-width: 600px)", 500.0));
        assert!(!media_matches("(max-width: 600px)", 700.0));
        assert!(!media_matches("print", 700.0));
        assert!(media_matches("screen and (min-width: 40em)", 700.0));
        assert!(media_matches("(width >= 600px)", 700.0));
    }

    #[test]
    fn matches_selectors() {
        let doc = html::parse(r#"<div id="m" class="a b"><p>1</p><p class="x">2</p></div>"#);
        let p2 = doc
            .descendants(Document::ROOT)
            .into_iter()
            .find(|&n| doc.element(n).is_some_and(|e| e.has_class("x")))
            .unwrap();
        for (sel, expected) in [
            ("p", true),
            ("div p", true),
            ("#m > .x", true),
            ("div.a.b p.x", true),
            ("p + p", true),
            ("p:first-child", false),
            ("p:last-child", true),
            ("p:nth-child(2)", true),
            ("p:not(.x)", false),
            ("span p", false),
            ("p:hover", false),
            ("[class^=x]", true),
        ] {
            assert_eq!(
                parse_selector(sel).unwrap().matches(&doc, p2),
                expected,
                "{sel}"
            );
        }
    }

    #[test]
    fn specificity() {
        assert_eq!(parse_selector("#a .b c").unwrap().specificity, (1, 1, 1));
        assert_eq!(
            parse_selector("ul li:first-child").unwrap().specificity,
            (0, 1, 2)
        );
    }

    #[test]
    fn parses_values() {
        assert_eq!(parse_color("#fff"), Some(Color::WHITE));
        assert_eq!(
            parse_color("rgb(255 0 0 / 50%)"),
            Some(Color {
                r: 255,
                g: 0,
                b: 0,
                a: 128
            })
        );
        assert_eq!(
            parse_color("hsl(120, 100%, 25%)"),
            Some(Color::rgb(0, 128, 0))
        );
        assert_eq!(parse_length("1.5em"), Some(Length::Em(1.5)));
        assert_eq!(parse_length("0"), Some(Length::Px(0.0)));
        assert_eq!(
            parse_length("calc(100% - 20%)"),
            Some(Length::Percent(80.0))
        );
        assert_eq!(parse_length("1e3px"), Some(Length::Px(1000.0)));
    }
}
