//! Single-line text editing with a caret and a selection, shared by the
//! address bar and form fields.

use std::ops::Range;

#[derive(Default)]
pub struct TextField {
    pub text: String,
    /// Caret position as a byte index.
    pub cursor: usize,
    /// The other end of the selection; equal to `cursor` when nothing is
    /// selected.
    pub anchor: usize,
}

impl TextField {
    pub fn new(text: &str) -> Self {
        let mut field = TextField::default();
        field.set(text);
        field
    }

    /// Replaces the text and puts the caret at the end.
    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
        self.anchor = self.cursor;
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
    }

    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    fn delete_selection(&mut self) -> bool {
        let sel = self.selection();
        if sel.is_empty() {
            return false;
        }
        self.text.replace_range(sel.clone(), "");
        self.cursor = sel.start;
        self.anchor = sel.start;
        true
    }

    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
        self.anchor = self.cursor;
    }

    pub fn backspace(&mut self, word: bool) {
        if self.delete_selection() || self.cursor == 0 {
            return;
        }
        let start = if word { self.word_start(self.cursor) } else { self.prev(self.cursor) };
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.anchor = start;
    }

    pub fn delete(&mut self, word: bool) {
        if self.delete_selection() || self.cursor == self.text.len() {
            return;
        }
        let end = if word { self.word_end(self.cursor) } else { self.next(self.cursor) };
        self.text.replace_range(self.cursor..end, "");
    }

    pub fn left(&mut self, select: bool, word: bool) {
        let sel = self.selection();
        let target = if !select && !sel.is_empty() {
            sel.start
        } else if word {
            self.word_start(self.cursor)
        } else {
            self.prev(self.cursor)
        };
        self.move_to(target, select);
    }

    pub fn right(&mut self, select: bool, word: bool) {
        let sel = self.selection();
        let target = if !select && !sel.is_empty() {
            sel.end
        } else if word {
            self.word_end(self.cursor)
        } else {
            self.next(self.cursor)
        };
        self.move_to(target, select);
    }

    pub fn move_to(&mut self, pos: usize, select: bool) {
        self.cursor = pos.min(self.text.len());
        if !select {
            self.anchor = self.cursor;
        }
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map_or(0, |(p, _)| p)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..].chars().next().map_or(i, |c| i + c.len_utf8())
    }

    fn word_start(&self, i: usize) -> usize {
        let before = &self.text[..i];
        let trimmed = before.trim_end_matches(|c: char| !c.is_alphanumeric());
        trimmed.rfind(|c: char| !c.is_alphanumeric()).map_or(0, |p| p + trimmed[p..].chars().next().map_or(1, char::len_utf8))
    }

    fn word_end(&self, i: usize) -> usize {
        let after = &self.text[i..];
        let skip = after.len() - after.trim_start_matches(|c: char| !c.is_alphanumeric()).len();
        let rest = &after[skip..];
        i + skip + rest.find(|c: char| !c.is_alphanumeric()).unwrap_or(rest.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits() {
        let mut f = TextField::new("hello world");
        f.backspace(true);
        assert_eq!(f.text, "hello ");
        f.insert("rust");
        assert_eq!(f.text, "hello rust");
        f.left(false, true);
        assert_eq!(f.cursor, 6);
        f.right(true, false);
        assert_eq!(f.selected_text(), "r");
        f.insert("R");
        assert_eq!(f.text, "hello Rust");
        f.select_all();
        f.insert("ä");
        assert_eq!(f.text, "ä");
        f.backspace(false);
        assert_eq!(f.text, "");
    }
}
