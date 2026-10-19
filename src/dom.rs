//! The document tree. Nodes live in one arena and refer to each other by index.

use std::collections::HashMap;
use std::sync::OnceLock;

pub type NodeId = usize;

#[derive(Clone, Debug)]
pub enum NodeKind {
    Document,
    Element(Element),
    Text(String),
}

#[derive(Clone, Debug)]
pub struct Element {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn id(&self) -> Option<&str> {
        self.attr("id").filter(|id| !id.is_empty())
    }

    pub fn classes(&self) -> impl Iterator<Item = &str> {
        self.attr("class").unwrap_or("").split_ascii_whitespace()
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

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

#[derive(Clone, Debug)]
pub struct Document {
    /// Structural additions go through append; tag changes through
    /// element_mut. Both invalidate the cached sibling metadata, so the list
    /// is private: read it through `nodes()`.
    nodes: Vec<Node>,
    /// No `<!doctype html>`: legacy rendering rules apply.
    pub quirks: bool,
    sibling_positions: OnceLock<Vec<SiblingPosition>>,
}

#[derive(Clone, Debug, Default)]
struct SiblingPosition {
    previous: Option<NodeId>,
    next: Option<NodeId>,
    /// One-based indices among element siblings and same-tag siblings.
    index: usize,
    type_index: usize,
    last_of_type: bool,
}

#[cfg(test)]
std::thread_local! {
    static SIBLING_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Document {
    pub const ROOT: NodeId = 0;

    pub fn new() -> Self {
        Document {
            nodes: vec![Node {
                kind: NodeKind::Document,
                parent: None,
                children: Vec::new(),
            }],
            quirks: true,
            sibling_positions: OnceLock::new(),
        }
    }

    pub fn append(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
        self.sibling_positions.take();
        let id = self.nodes.len();
        self.nodes.push(Node {
            kind,
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent].children.push(id);
        id
    }

    /// Every node, indexed by `NodeId`; parents come before their children.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// The text of a text node, to extend it (no structural change).
    pub fn text_mut(&mut self, id: NodeId) -> Option<&mut String> {
        match &mut self.nodes.get_mut(id)?.kind {
            NodeKind::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn element(&self, id: NodeId) -> Option<&Element> {
        match &self.nodes[id].kind {
            NodeKind::Element(e) => Some(e),
            _ => None,
        }
    }

    pub fn element_mut(&mut self, id: NodeId) -> Option<&mut Element> {
        self.sibling_positions.take();
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

    /// Build once per document state, in two linear passes over each parent's
    /// children. Text nodes have links too, but never consume an element index.
    fn sibling_positions(&self) -> &[SiblingPosition] {
        self.sibling_positions.get_or_init(|| {
            let mut positions = vec![SiblingPosition::default(); self.nodes.len()];
            for parent in &self.nodes {
                let mut last_by_tag: HashMap<&str, NodeId> = HashMap::new();
                let mut previous = None;
                let mut index = 0;
                for &child in &parent.children {
                    #[cfg(test)]
                    SIBLING_VISITS.with(|visits| visits.set(visits.get() + 1));
                    positions[child].previous = previous;
                    if let Some(el) = self.element(child) {
                        index += 1;
                        positions[child].index = index;
                        positions[child].type_index = match last_by_tag.insert(&el.tag, child) {
                            Some(last) => {
                                positions[last].last_of_type = false;
                                positions[last].type_index + 1
                            }
                            None => 1,
                        };
                        positions[child].last_of_type = true;
                        previous = Some(child);
                    }
                }
                let mut next = None;
                for &child in parent.children.iter().rev() {
                    #[cfg(test)]
                    SIBLING_VISITS.with(|visits| visits.set(visits.get() + 1));
                    positions[child].next = next;
                    if self.element(child).is_some() {
                        next = Some(child);
                    }
                }
            }
            positions
        })
    }

    pub fn element_index(&self, id: NodeId) -> usize {
        self.sibling_positions()[id].index
    }

    pub fn element_type_index(&self, id: NodeId) -> usize {
        self.sibling_positions()[id].type_index
    }

    pub fn is_last_of_type(&self, id: NodeId) -> bool {
        self.sibling_positions()[id].last_of_type
    }

    /// Element siblings before `id`, nearest first. Starting the iterator and
    /// visiting each returned element are O(1) after the shared index is built.
    pub fn previous_elements(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let positions = self.sibling_positions();
        std::iter::successors(positions[id].previous, |&n| positions[n].previous)
    }

    pub fn next_elements(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let positions = self.sibling_positions();
        std::iter::successors(positions[id].next, |&n| positions[n].next)
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
        self.descendants(Self::ROOT)
            .into_iter()
            .find(|&n| self.tag(n) == tag)
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
        let title = self
            .find_tag("title")
            .map(|t| self.text_content(t))
            .unwrap_or_default();
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
                    self.element(n)
                        .is_some_and(|e| e.tag == "a" && e.attr("name") == Some(name))
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{css, html};

    #[test]
    fn sibling_positions_ignore_text_and_stay_within_the_parent() {
        let doc = html::parse(
            "<div>lead<i id=a></i>middle<b id=b></b><i id=c></i>trail</div>\
             <p><i id=d></i></p>",
        );
        let [a, b, c, d] = ["a", "b", "c", "d"].map(|id| doc.find_anchor(id).unwrap());
        assert_eq!(doc.previous_elements(c).collect::<Vec<_>>(), [b, a]);
        assert_eq!(doc.next_elements(a).collect::<Vec<_>>(), [b, c]);
        assert_eq!(doc.previous_elements(b).collect::<Vec<_>>(), [a]);
        assert_eq!(doc.next_elements(b).collect::<Vec<_>>(), [c]);
        for (node, index, type_index, last) in [
            (a, 1, 1, false),
            (b, 2, 1, true),
            (c, 3, 2, true),
            (d, 1, 1, true),
        ] {
            assert_eq!(doc.element_index(node), index);
            assert_eq!(doc.element_type_index(node), type_index);
            assert_eq!(doc.is_last_of_type(node), last);
        }
        assert_eq!(doc.previous_elements(d).next(), None);
        assert_eq!(doc.next_elements(d).next(), None);
        let div = doc.find_tag("div").unwrap();
        let children = &doc.nodes[div].children;
        assert_eq!(doc.next_elements(children[0]).next(), Some(a));
        assert_eq!(doc.previous_elements(children[2]).next(), Some(a));
        assert_eq!(doc.next_elements(children[2]).next(), Some(b));
        assert_eq!(
            doc.previous_elements(*children.last().unwrap()).next(),
            Some(c)
        );
        for (source, expected) in [
            ("i:first-child", vec![a, d]),
            ("i:last-child", vec![c, d]),
            ("i:only-child", vec![d]),
            ("i:first-of-type", vec![a, d]),
            ("i:last-of-type", vec![c, d]),
            ("i:nth-child(3)", vec![c]),
            ("i + b", vec![b]),
            ("b + i", vec![c]),
        ] {
            let selector = css::parse_selector(source).unwrap();
            let actual: Vec<_> = [a, b, c, d]
                .into_iter()
                .filter(|&id| selector.matches(&doc, id))
                .collect();
            assert_eq!(actual, expected, "{source}");
        }
    }

    #[test]
    fn mutations_invalidate_sibling_positions_and_clones_are_independent() {
        let mut doc = html::parse("<div><i></i>text</div>");
        let parent = doc.find_tag("div").unwrap();
        let first = doc.find_tag("i").unwrap();
        assert!(doc.is_last_of_type(first));
        let second = doc.append(
            parent,
            NodeKind::Element(Element {
                tag: "i".into(),
                attrs: vec![],
            }),
        );
        assert_eq!(doc.next_elements(first).next(), Some(second));
        assert_eq!(doc.element_index(second), 2);
        assert_eq!(doc.element_type_index(second), 2);
        assert!(!doc.is_last_of_type(first));
        doc.element_mut(second).unwrap().tag = "b".into();
        assert!(doc.is_last_of_type(first));
        assert_eq!(doc.element_type_index(second), 1);
        let mut clone = doc.clone();
        clone.append(
            parent,
            NodeKind::Element(Element {
                tag: "i".into(),
                attrs: vec![],
            }),
        );
        assert!(!clone.is_last_of_type(first));
        assert!(doc.is_last_of_type(first));
        assert_eq!(doc.next_elements(second).next(), None);
    }

    #[test]
    fn flat_sibling_matching_builds_metadata_once() {
        let count = 40_000;
        let doc = html::parse(&"<i></i>text".repeat(count));
        let nodes: Vec<_> = (0..doc.nodes.len())
            .filter(|&n| doc.tag(n) == "i")
            .collect();
        SIBLING_VISITS.with(|visits| visits.set(0));
        let start = std::time::Instant::now();
        for (source, expected) in [
            ("i:first-child", 1),
            ("i:last-child", 1),
            ("i:only-child", 0),
            ("i:first-of-type", 1),
            ("i:last-of-type", 1),
            ("i:nth-child(odd)", count / 2),
            ("i:nth-child(even)", count / 2),
            ("i:nth-child(-n+3)", 3),
            ("i + i", count - 1),
        ] {
            let selector = css::parse_selector(source).unwrap();
            assert_eq!(
                nodes.iter().filter(|&&n| selector.matches(&doc, n)).count(),
                expected,
                "{source}"
            );
        }
        // All nine complete matching passes share one two-pass metadata build.
        assert_eq!(
            SIBLING_VISITS.with(|visits| visits.get()),
            2 * (doc.nodes.len() - 1)
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn distinct_tag_positions_are_indexed_per_parent() {
        let count = 10_000;
        let markup: String = (0..count).map(|i| format!("<t{i}></t{i}>text")).collect();
        let doc = html::parse(&markup);
        let first = css::parse_selector(":first-of-type").unwrap();
        let last = css::parse_selector(":last-of-type").unwrap();
        SIBLING_VISITS.with(|visits| visits.set(0));
        let start = std::time::Instant::now();
        let body = doc.find_tag("body").unwrap();
        let mut matched = 0;
        for &node in &doc.nodes[body].children {
            if doc.element(node).is_some() {
                assert!(first.matches(&doc, node));
                assert!(last.matches(&doc, node));
                matched += 1;
            }
        }
        assert_eq!(matched, count);
        assert_eq!(
            SIBLING_VISITS.with(|visits| visits.get()),
            2 * (doc.nodes.len() - 1)
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }
}
