//! The document tree. Nodes live in one arena and refer to each other by index.

pub type NodeId = usize;

pub enum NodeKind {
    Document,
    Element(Element),
    Text(String),
}

pub struct Element {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub fn id(&self) -> Option<&str> {
        self.attr("id").filter(|id| !id.is_empty())
    }

    pub fn classes(&self) -> impl Iterator<Item = &str> {
        self.attr("class").unwrap_or("").split_ascii_whitespace()
    }

    pub fn has_class(&self, class: &str) -> bool {
        self.classes().any(|c| c == class)
    }

    /// The image URL to load for `<img>` and `<input type=image>`. Lazy
    /// loading scripts often keep the real URL in `data-src` and a tiny
    /// placeholder in `src`; without scripts we use `data-src` directly.
    pub fn image_source(&self) -> Option<&str> {
        let src = self.attr("src").map(str::trim).filter(|s| !s.is_empty());
        let lazy = ["data-src", "data-lazy-src", "data-original"]
            .iter()
            .find_map(|a| self.attr(a))
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let srcset = || {
            self.attr("srcset")
                .or_else(|| self.attr("data-srcset"))
                .and_then(|s| s.split(',').next())
                .and_then(|c| c.split_whitespace().next())
        };
        match (src, lazy) {
            (Some(s), Some(l)) if s.starts_with("data:") => Some(l),
            (Some(s), _) => Some(s),
            (None, Some(l)) => Some(l),
            (None, None) => srcset(),
        }
    }
}

pub struct Node {
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

pub struct Document {
    pub nodes: Vec<Node>,
    /// No `<!doctype html>`: legacy rendering rules apply.
    pub quirks: bool,
}

impl Document {
    pub const ROOT: NodeId = 0;

    pub fn new() -> Self {
        Document {
            nodes: vec![Node { kind: NodeKind::Document, parent: None, children: Vec::new() }],
            quirks: true,
        }
    }

    pub fn append(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node { kind, parent: Some(parent), children: Vec::new() });
        self.nodes[parent].children.push(id);
        id
    }

    pub fn element(&self, id: NodeId) -> Option<&Element> {
        match &self.nodes[id].kind {
            NodeKind::Element(e) => Some(e),
            _ => None,
        }
    }

    pub fn element_mut(&mut self, id: NodeId) -> Option<&mut Element> {
        match &mut self.nodes[id].kind {
            NodeKind::Element(e) => Some(e),
            _ => None,
        }
    }

    pub fn tag(&self, id: NodeId) -> &str {
        self.element(id).map_or("", |e| e.tag.as_str())
    }

    pub fn parent_element(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].parent.filter(|&p| self.element(p).is_some())
    }

    /// Element siblings before `id`, nearest first.
    pub fn previous_elements(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let siblings = self.nodes[id]
            .parent
            .map_or(&[][..], |p| self.nodes[p].children.as_slice());
        let pos = siblings.iter().position(|&s| s == id).unwrap_or(0);
        siblings[..pos].iter().rev().copied().filter(|&s| self.element(s).is_some())
    }

    pub fn next_elements(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let siblings = self.nodes[id]
            .parent
            .map_or(&[][..], |p| self.nodes[p].children.as_slice());
        let pos = siblings.iter().position(|&s| s == id).map_or(siblings.len(), |p| p + 1);
        siblings[pos..].iter().copied().filter(|&s| self.element(s).is_some())
    }

    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.nodes[id].parent, |&p| self.nodes[p].parent)
    }

    /// All nodes below `id` in document order, not including `id`.
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.nodes[id].children.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            out.push(n);
            stack.extend(self.nodes[n].children.iter().rev());
        }
        out
    }

    pub fn find_tag(&self, tag: &str) -> Option<NodeId> {
        self.descendants(Self::ROOT).into_iter().find(|&n| self.tag(n) == tag)
    }

    /// Concatenated text of every text node below `id`.
    pub fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        for n in self.descendants(id) {
            if let NodeKind::Text(t) = &self.nodes[n].kind {
                out.push_str(t);
            }
        }
        out
    }

    pub fn title(&self) -> String {
        let title = self.find_tag("title").map(|t| self.text_content(t)).unwrap_or_default();
        title.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The element with this `id`, or failing that an `<a name>` anchor.
    pub fn find_anchor(&self, name: &str) -> Option<NodeId> {
        let all = self.descendants(Self::ROOT);
        all.iter()
            .copied()
            .find(|&n| self.element(n).and_then(Element::id) == Some(name))
            .or_else(|| {
                all.iter().copied().find(|&n| {
                    self.element(n).is_some_and(|e| e.tag == "a" && e.attr("name") == Some(name))
                })
            })
    }
}
