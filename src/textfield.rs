//! Single-line text editing with a caret and a selection, shared by the
//! address bar and form fields.

use std::ops::Range;

#[derive(Default)]
pub struct TextField {
    /// Public because egui edits the address bar's string in place, which
    /// leaves `cursor` and `anchor` stale; every method clamps them first.
    pub text: String,
    /// Caret position as a byte index.
    cursor: usize,
    /// The other end of the selection; equal to `cursor` when nothing is
    /// selected.
    anchor: usize,
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

    /// The caret position as a byte index on a char boundary.
    pub fn cursor(&self) -> usize {
        self.text.floor_char_boundary(self.cursor)
    }

    /// Pulls `cursor` and `anchor` back inside `text` and onto char
    /// boundaries, in case `text` changed behind our back.
    fn clamp(&mut self) {
        self.cursor = self.cursor();
        self.anchor = self.text.floor_char_boundary(self.anchor);
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
    }

    pub fn selection(&self) -> Range<usize> {
        let (cursor, anchor) = (self.cursor(), self.text.floor_char_boundary(self.anchor));
        anchor.min(cursor)..anchor.max(cursor)
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    fn delete_selection(&mut self) -> bool {
        self.clamp();
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
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        self.insert_raw(&s);
    }

    /// Inserts a line break (for `<textarea>`), which `insert` filters out.
    pub fn newline(&mut self) {
        self.insert_raw("\n");
    }

    fn insert_raw(&mut self, s: &str) {
        self.delete_selection();
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.anchor = self.cursor;
    }

    pub fn backspace(&mut self, word: bool) {
        if self.delete_selection() || self.cursor == 0 {
            return;
        }
        let start = if word {
            self.word_start(self.cursor)
        } else {
            self.prev(self.cursor)
        };
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.anchor = start;
    }

    pub fn delete(&mut self, word: bool) {
        if self.delete_selection() || self.cursor == self.text.len() {
            return;
        }
        let end = if word {
            self.word_end(self.cursor)
        } else {
            self.next(self.cursor)
        };
        self.text.replace_range(self.cursor..end, "");
    }

    pub fn left(&mut self, select: bool, word: bool) {
        self.clamp();
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
        self.clamp();
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
        self.clamp();
        self.cursor = self.text.floor_char_boundary(pos);
        if !select {
            self.anchor = self.cursor;
        }
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i]
            .char_indices()
            .next_back()
            .map_or(0, |(p, _)| p)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..]
            .chars()
            .next()
            .map_or(i, |c| i + c.len_utf8())
    }

    fn word_start(&self, i: usize) -> usize {
        let before = &self.text[..i];
        let trimmed = before.trim_end_matches(|c: char| !c.is_alphanumeric());
        trimmed
            .rfind(|c: char| !c.is_alphanumeric())
            .map_or(0, |p| {
                p + trimmed[p..].chars().next().map_or(1, char::len_utf8)
            })
    }

    fn word_end(&self, i: usize) -> usize {
        let after = &self.text[i..];
        let skip = after.len()
            - after
                .trim_start_matches(|c: char| !c.is_alphanumeric())
                .len();
        let rest = &after[skip..];
        i + skip
            + rest
                .find(|c: char| !c.is_alphanumeric())
                .unwrap_or(rest.len())
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

    /// egui edits `text` directly without moving the caret (issue #14).
    #[test]
    fn survives_text_changed_behind_its_back() {
        let mut f = TextField::new("https://example.com/ä");
        f.select_all();
        f.text = "aä".to_string();
        assert_eq!(f.selected_text(), "aä");
        assert_eq!(f.cursor(), 3);
        f.text = "ää".to_string();
        f.move_to(1, false); // inside the first 'ä'
        assert_eq!(f.cursor(), 0);
        f.select_all();
        f.text = "a".to_string();
        f.backspace(false);
        assert_eq!(f.text, "");
        for edit in [
            |f: &mut TextField| f.insert("x"),
            |f: &mut TextField| f.backspace(true),
            |f: &mut TextField| f.delete(false),
            |f: &mut TextField| f.left(true, true),
            |f: &mut TextField| f.right(false, false),
            |f: &mut TextField| f.newline(),
        ] {
            let mut f = TextField::new("hello wörld");
            f.text = "hé".to_string();
            edit(&mut f);
            let _ = f.selected_text();
        }
    }
}
