//! A forgiving HTML parser. It follows the broad strokes of the HTML spec
//! (implied `<html>`, `<head>` and `<body>`, void elements, auto-closed `<p>`,
//! `<li>`, table parts) without the full insertion-mode state machine.

use crate::dom::{Document, Element, NodeId, NodeKind};

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];
const RAW_TEXT: &[&str] = &[
    "script", "style", "textarea", "title", "xmp", "iframe", "noembed",
];
/// The most open elements whose children nest further. Deeper start tags are
/// attached beside the innermost element instead of inside it (as Blink does),
/// so styling, layout and painting never recurse deeper than this.
pub const MAX_DEPTH: usize = 512;
const HEAD_ONLY: &[&str] = &[
    "title", "meta", "link", "style", "script", "base", "noscript",
];
/// Start tags that close an open `<p>`.
const CLOSES_P: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "li",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "ul",
];
/// Elements that stop the search for an open `<p>` to close.
const P_SCOPE: &[&str] = &[
    "html", "body", "table", "td", "th", "caption", "button", "template", "object",
];

pub fn parse(source: &str) -> Document {
    let mut builder = Builder::new();
    let mut tok = Tokenizer {
        src: source,
        pos: 0,
    };
    while let Some(token) = tok.next(builder.raw_text_tag()) {
        builder.token(token);
    }
    let mut doc = builder.finish();
    let start = source.trim_start_matches('\u{feff}').trim_start();
    doc.quirks = !start
        .get(..14)
        .is_some_and(|s| s.eq_ignore_ascii_case("<!doctype html"));
    doc
}

enum Token {
    Start {
        tag: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    End(String),
    Text(String),
}

struct Tokenizer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn next(&mut self, raw_text: Option<&str>) -> Option<Token> {
        if self.pos >= self.src.len() {
            return None;
        }
        if let Some(tag) = raw_text {
            return Some(self.raw_text(tag));
        }
        // Skip comments, doctypes and processing instructions in a loop: by
        // recursion, a page of `<!---->` would overflow the stack.
        loop {
            let rest = self.rest();
            self.pos += if let Some(after) = rest.strip_prefix("<!--") {
                4 + after.find("-->").map_or(after.len(), |i| i + 3)
            } else if rest.starts_with("<!") || rest.starts_with("<?") {
                rest.find('>').map_or(rest.len(), |i| i + 1)
            } else {
                break;
            };
            if self.pos >= self.src.len() {
                return None;
            }
        }
        let rest = self.rest();
        if let Some(after) = rest.strip_prefix("</")
            && after.starts_with(|c: char| c.is_ascii_alphabetic())
        {
            let end = after.find('>').map_or(after.len(), |i| i + 1);
            let name = tag_name(&after[..end]);
            self.pos += 2 + end;
            return Some(Token::End(name));
        }
        if rest.starts_with('<') && rest[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            return Some(self.start_tag());
        }
        // Text up to the next thing that looks like markup.
        let bytes = rest.as_bytes();
        let mut end = 1;
        while end < bytes.len() {
            if bytes[end] == b'<' {
                let next = bytes.get(end + 1).copied().unwrap_or(0);
                if next.is_ascii_alphabetic() || matches!(next, b'/' | b'!' | b'?') {
                    break;
                }
            }
            end += 1;
        }
        self.pos += end;
        Some(Token::Text(decode_entities(&rest[..end])))
    }

    fn raw_text(&mut self, tag: &str) -> Token {
        let rest = self.rest();
        let lower = rest.to_ascii_lowercase();
        let close = format!("</{tag}");
        let end = lower.find(&close).unwrap_or(rest.len());
        let text = &rest[..end];
        self.pos += end;
        if end == 0 {
            // Consume the end tag so the builder leaves raw-text mode.
            let after = &rest[close.len()..];
            self.pos += close.len() + after.find('>').map_or(after.len(), |i| i + 1);
            return Token::End(tag.to_string());
        }
        let text = if matches!(tag, "textarea" | "title") {
            decode_entities(text)
        } else {
            text.to_string()
        };
        Token::Text(text)
    }

    fn start_tag(&mut self) -> Token {
        let bytes = self.src.as_bytes();
        let mut i = self.pos + 1;
        let name_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'>' | b'/')
        {
            i += 1;
        }
        let tag = self.src[name_start..i].to_ascii_lowercase();
        let mut attrs: Vec<(String, String)> = Vec::new();
        let mut self_closing = false;
        loop {
            while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
                self_closing = bytes[i] == b'/';
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }
            if bytes[i] == b'>' {
                i += 1;
                break;
            }
            self_closing = false;
            let start = i;
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && !matches!(bytes[i], b'>' | b'=' | b'/')
            {
                i += 1;
            }
            // Never stall on a lone '=' or similar.
            if i == start {
                i += 1;
                continue;
            }
            let name = self.src[start..i].to_ascii_lowercase();
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let mut value = String::new();
            if i < bytes.len() && bytes[i] == b'=' {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i < bytes.len() && matches!(bytes[i], b'"' | b'\'') {
                    let quote = bytes[i];
                    let vstart = i + 1;
                    i = vstart;
                    while i < bytes.len() && bytes[i] != quote {
                        i += 1;
                    }
                    value = decode_entities(&self.src[vstart..i]);
                    i = (i + 1).min(bytes.len());
                } else {
                    let vstart = i;
                    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                        i += 1;
                    }
                    value = decode_entities(&self.src[vstart..i]);
                }
            }
            if !attrs.iter().any(|(n, _)| *n == name) {
                attrs.push((name, value));
            }
        }
        self.pos = i;
        Token::Start {
            tag,
            attrs,
            self_closing,
        }
    }
}

fn tag_name(s: &str) -> String {
    s.split(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

struct Builder {
    doc: Document,
    html: NodeId,
    head: Option<NodeId>,
    body: Option<NodeId>,
    /// Open elements below `<html>`, innermost last.
    stack: Vec<NodeId>,
    /// Skip one newline right after `<pre>` or `<textarea>`.
    skip_newline: bool,
}

impl Builder {
    fn new() -> Self {
        let mut doc = Document::new();
        let html = doc.append(
            Document::ROOT,
            NodeKind::Element(Element {
                tag: "html".into(),
                attrs: Vec::new(),
            }),
        );
        Builder {
            doc,
            html,
            head: None,
            body: None,
            stack: Vec::new(),
            skip_newline: false,
        }
    }

    fn current(&self) -> NodeId {
        self.stack.last().copied().unwrap_or(self.html)
    }

    fn raw_text_tag(&self) -> Option<&str> {
        let tag = self.doc.tag(self.current());
        RAW_TEXT.contains(&tag).then_some(tag)
    }

    fn open_tags(&self) -> impl Iterator<Item = &str> {
        self.stack.iter().rev().map(|&n| self.doc.tag(n))
    }

    fn ensure_head(&mut self) -> NodeId {
        *self.head.get_or_insert_with(|| {
            self.doc.append(
                self.html,
                NodeKind::Element(Element {
                    tag: "head".into(),
                    attrs: Vec::new(),
                }),
            )
        })
    }

    fn ensure_body(&mut self) {
        if self.body.is_none() {
            self.ensure_head();
            self.stack.clear();
            let body = self.doc.append(
                self.html,
                NodeKind::Element(Element {
                    tag: "body".into(),
                    attrs: Vec::new(),
                }),
            );
            self.body = Some(body);
            self.stack.push(body);
        }
    }

    /// Pops open elements up to and including the innermost `tag`, unless one
    /// of `boundary` is reached first.
    fn close(&mut self, tags: &[&str], boundary: &[&str]) -> bool {
        let found = self
            .open_tags()
            .position(|t| tags.contains(&t) || boundary.contains(&t));
        match found {
            Some(depth) if tags.contains(&self.open_tags().nth(depth).unwrap_or("")) => {
                let keep = self.stack.len() - depth - 1;
                // Never pop <body> itself.
                if self.body.is_some_and(|b| self.stack[keep] == b) {
                    return false;
                }
                self.stack.truncate(keep);
                true
            }
            _ => false,
        }
    }

    fn token(&mut self, token: Token) {
        match token {
            Token::Text(text) => self.text(text),
            Token::Start {
                tag,
                attrs,
                self_closing,
            } => self.start(tag, attrs, self_closing),
            Token::End(tag) => self.end(&tag),
        }
    }

    fn text(&mut self, mut text: String) {
        if std::mem::take(&mut self.skip_newline)
            && let Some(rest) = text.strip_prefix('\n')
        {
            text = rest.to_string();
        }
        if text.is_empty() {
            return;
        }
        let in_head = self.head.is_some_and(|h| self.current() == h) || self.stack.is_empty();
        if self.body.is_none() && in_head {
            if text.trim().is_empty() {
                return;
            }
            self.ensure_body();
        }
        let parent = self.current();
        if let Some(&last) = self.doc.nodes[parent].children.last()
            && let NodeKind::Text(existing) = &mut self.doc.nodes[last].kind
        {
            existing.push_str(&text);
            return;
        }
        self.doc.append(parent, NodeKind::Text(text));
    }

    fn start(&mut self, tag: String, attrs: Vec<(String, String)>, self_closing: bool) {
        match tag.as_str() {
            "html" => {
                merge_attrs(self.doc.element_mut(self.html), attrs);
                return;
            }
            "head" => {
                if self.head.is_none() && self.body.is_none() {
                    let head = self.ensure_head();
                    merge_attrs(self.doc.element_mut(head), attrs);
                    self.stack = vec![head];
                }
                return;
            }
            "body" => {
                self.ensure_body();
                merge_attrs(self.body.and_then(|b| self.doc.element_mut(b)), attrs);
                return;
            }
            _ => {}
        }

        if self.body.is_none() && HEAD_ONLY.contains(&tag.as_str()) {
            let head = self.ensure_head();
            if self.stack.is_empty() {
                self.stack.push(head);
            }
        } else {
            self.ensure_body();
        }

        match tag.as_str() {
            t if CLOSES_P.contains(&t) => {
                self.close(&["p"], P_SCOPE);
            }
            _ => {}
        }
        match tag.as_str() {
            "li" => {
                self.close(&["li"], &["ul", "ol", "menu", "table"]);
            }
            "dt" | "dd" => {
                self.close(&["dt", "dd"], &["dl", "table"]);
            }
            "option" | "optgroup" => {
                self.close(&["option"], &["select", "datalist"]);
            }
            "tr" => {
                self.close(&["tr"], &["table", "tbody", "thead", "tfoot"]);
            }
            "td" | "th" => {
                self.close(&["td", "th"], &["tr", "table"]);
            }
            "thead" | "tbody" | "tfoot" => {
                self.close(&["thead", "tbody", "tfoot"], &["table"]);
            }
            "a" => {
                self.close(&["a"], &["table", "td", "th"]);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if matches!(
                    self.doc.tag(self.current()),
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                ) {
                    self.stack.pop();
                }
            }
            _ => {}
        }

        let mut parent = self.current();
        if self.stack.len() >= MAX_DEPTH {
            parent = self.doc.nodes[parent].parent.unwrap_or(parent);
        }
        self.skip_newline = matches!(tag.as_str(), "pre" | "textarea" | "listing");
        let is_void = VOID.contains(&tag.as_str());
        let node = self
            .doc
            .append(parent, NodeKind::Element(Element { tag, attrs }));
        let raw = RAW_TEXT.contains(&self.doc.tag(node));
        if !is_void && (!self_closing || raw) {
            self.stack.push(node);
        }
    }

    fn end(&mut self, tag: &str) {
        match tag {
            "html" | "body" | "br" => {}
            "head" => {
                if self.head.is_some_and(|h| self.stack.first() == Some(&h)) {
                    self.stack.clear();
                }
            }
            _ => {
                if self.raw_text_tag().is_some() {
                    // Leaving a raw-text element.
                    self.stack.pop();
                } else {
                    self.close(&[tag], &[]);
                }
            }
        }
    }

    fn finish(mut self) -> Document {
        self.ensure_head();
        self.ensure_body();
        self.doc
    }
}

fn merge_attrs(element: Option<&mut Element>, attrs: Vec<(String, String)>) {
    if let Some(e) = element {
        for (name, value) in attrs {
            if e.attr(&name).is_none() {
                e.attrs.push((name, value));
            }
        }
    }
}

/// Replaces character references such as `&amp;`, `&#169;` and `&#x2014;`.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..]
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '#')
            .map_or(rest.len(), |e| e + 1);
        let name = &rest[1..end];
        let decoded = if let Some(num) = name.strip_prefix('#') {
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => num.parse::<u32>().ok(),
            };
            code.map(|c| {
                char::from_u32(c)
                    .filter(|&c| c != '\0')
                    .unwrap_or('\u{FFFD}')
            })
        } else {
            named_entity(name)
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end..];
                if rest.starts_with(';') {
                    rest = &rest[1..];
                }
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" | "AMP" => '&',
        "lt" | "LT" => '<',
        "gt" | "GT" => '>',
        "quot" | "QUOT" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" | "COPY" => '©',
        "reg" | "REG" => '®',
        "trade" => '™',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "lsaquo" => '‹',
        "rsaquo" => '›',
        "lsquo" => '‘',
        "rsquo" => '’',
        "sbquo" => '‚',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "bull" => '•',
        "middot" => '·',
        "times" => '×',
        "divide" => '÷',
        "minus" => '−',
        "plusmn" => '±',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "deg" => '°',
        "para" => '¶',
        "sect" => '§',
        "dagger" => '†',
        "Dagger" => '‡',
        "prime" => '′',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "harr" => '↔',
        "rArr" => '⇒',
        "hearts" => '♥',
        "check" => '✓',
        "iexcl" => '¡',
        "iquest" => '¿',
        "shy" => '\u{ad}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "thinsp" => '\u{2009}',
        "zwj" => '\u{200d}',
        "zwnj" => '\u{200c}',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "sup2" => '²',
        "sup3" => '³',
        "micro" => 'µ',
        "auml" => 'ä',
        "ouml" => 'ö',
        "uuml" => 'ü',
        "Auml" => 'Ä',
        "Ouml" => 'Ö',
        "Uuml" => 'Ü',
        "szlig" => 'ß',
        "eacute" => 'é',
        "egrave" => 'è',
        "ecirc" => 'ê',
        "Eacute" => 'É',
        "aacute" => 'á',
        "agrave" => 'à',
        "acirc" => 'â',
        "iacute" => 'í',
        "oacute" => 'ó',
        "uacute" => 'ú',
        "ccedil" => 'ç',
        "ntilde" => 'ñ',
        "alpha" => 'α',
        "beta" => 'β',
        "pi" => 'π',
        "lambda" => 'λ',
        "infin" => '∞',
        "ne" => '≠',
        "le" => '≤',
        "ge" => '≥',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(doc: &Document, parent: NodeId) -> Vec<String> {
        doc.nodes[parent]
            .children
            .iter()
            .map(|&c| doc.tag(c).to_string())
            .collect()
    }

    fn depth(doc: &Document, mut node: NodeId) -> usize {
        let mut d = 0;
        while let Some(p) = doc.nodes[node].parent {
            node = p;
            d += 1;
        }
        d
    }

    #[test]
    fn many_comments_do_not_recurse() {
        let markup = "<!---->".repeat(200_000) + "<!doctype html><?x?><p>x";
        let doc = parse(&markup);
        assert_eq!(doc.text_content(doc.find_tag("p").unwrap()), "x");
    }

    #[test]
    fn caps_dom_depth() {
        let n = MAX_DEPTH * 4;
        let markup = "<div>".repeat(n) + "<p>x</p>" + &"</div>".repeat(n) + "<span>after</span>";
        let doc = parse(&markup);
        let deepest = (0..doc.nodes.len())
            .map(|id| depth(&doc, id))
            .max()
            .unwrap();
        assert!(deepest <= MAX_DEPTH + 3, "{deepest}");
        let p = doc.find_tag("p").unwrap();
        assert_eq!(doc.text_content(p), "x");
        // The end tags still close the right elements.
        let span = doc.find_tag("span").unwrap();
        assert_eq!(doc.tag(doc.nodes[span].parent.unwrap()), "body");
    }

    #[test]
    fn builds_implied_structure() {
        let doc = parse("<title>Hi</title><p>Hello");
        let html = doc.nodes[Document::ROOT].children[0];
        assert_eq!(tags(&doc, html), ["head", "body"]);
        assert_eq!(doc.title(), "Hi");
        let body = doc.find_tag("body").unwrap();
        assert_eq!(doc.text_content(body), "Hello");
    }

    #[test]
    fn auto_closes_paragraphs_and_list_items() {
        let doc = parse("<body><p>a<p>b<ul><li>x<li>y</ul>");
        let body = doc.find_tag("body").unwrap();
        assert_eq!(tags(&doc, body), ["p", "p", "ul"]);
        let ul = doc.find_tag("ul").unwrap();
        assert_eq!(tags(&doc, ul), ["li", "li"]);
    }

    #[test]
    fn keeps_script_text_raw() {
        let doc = parse("<script>if (a < b && c) {}</script><p>x</p>");
        let script = doc.find_tag("script").unwrap();
        assert_eq!(doc.text_content(script), "if (a < b && c) {}");
        assert!(doc.find_tag("p").is_some());
    }

    #[test]
    fn parses_attributes() {
        let doc = parse(r#"<a href="/x?a=1&amp;b=2" class=link data-x>t</a>"#);
        let a = doc.element(doc.find_tag("a").unwrap()).unwrap();
        assert_eq!(a.attr("href"), Some("/x?a=1&b=2"));
        assert_eq!(a.attr("class"), Some("link"));
        assert_eq!(a.attr("data-x"), Some(""));
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode_entities("a &lt; b &amp;&amp; &#169; &#x2014; &bogus;"),
            "a < b && © — &bogus;"
        );
    }

    #[test]
    fn table_cells_close_each_other() {
        let doc = parse("<table><tr><td>a<td>b<tr><td>c</table>");
        let table = doc.find_tag("table").unwrap();
        assert_eq!(tags(&doc, table), ["tr", "tr"]);
        let tr = doc.nodes[table].children[0];
        assert_eq!(tags(&doc, tr), ["td", "td"]);
    }
}
