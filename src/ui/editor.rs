use crate::text::{Document, Text};
use anyhow::Result;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone)]
pub struct Paste {
    pub id: usize,
    pub start: usize,
    pub end: usize,
    pub source: Text,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub text: String,
    pub cursor: usize,
    pub pastes: Vec<Paste>,
}
impl Snapshot {
    pub fn document(&self) -> Result<Document> {
        let mut doc = Document::new()?;
        let mut from = 0;
        for paste in &self.pastes {
            if paste.start > from {
                doc.push(Text::new(&self.text[from..paste.start])?);
            }
            doc.push(paste.source.clone());
            from = paste.end;
        }
        if from < self.text.len() {
            doc.push(Text::new(&self.text[from..])?);
        }
        Ok(doc)
    }
}
#[derive(Default)]
pub struct Editor {
    pub text: String,
    pub cursor: usize,
    pub pastes: Vec<Paste>,
    pub revision: u64,
    next_paste: usize,
}
impl Editor {
    pub fn set(&mut self, text: String) {
        self.revision += 1;
        self.pastes.clear();
        self.cursor = text.len();
        self.text = text;
    }
    pub fn take(&mut self) -> String {
        self.revision += 1;
        self.pastes.clear();
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }
    pub fn insert(&mut self, text: &str) {
        self.snap();
        self.revision += 1;
        for paste in &mut self.pastes {
            if paste.start >= self.cursor {
                paste.start += text.len();
                paste.end += text.len();
            }
        }
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        if !self
            .text
            .grapheme_indices(true)
            .any(|(i, _)| i == self.cursor)
            && self.cursor != self.text.len()
        {
            self.cursor = self
                .text
                .grapheme_indices(true)
                .map(|(i, _)| i)
                .find(|&i| i > self.cursor)
                .unwrap_or(self.text.len());
        }
    }
    pub fn left(&mut self) {
        if let Some(paste) = self.pastes.iter().find(|p| p.end == self.cursor) {
            self.cursor = paste.start;
            return;
        }

        self.cursor = self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }
    pub fn right(&mut self) {
        if let Some(paste) = self.pastes.iter().find(|p| p.start == self.cursor) {
            self.cursor = paste.end;
            return;
        }

        if let Some(g) = self.text[self.cursor..].graphemes(true).next() {
            self.cursor += g.len();
        }
    }
    fn erase(&mut self, start: usize, end: usize) {
        self.revision += 1;
        self.pastes.retain(|p| !(p.start >= start && p.end <= end));
        for paste in &mut self.pastes {
            if paste.start >= end {
                paste.start -= end - start;
                paste.end -= end - start;
            }
        }
        self.text.replace_range(start..end, "");
        self.cursor = start;
    }
    pub fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.erase(self.cursor, end);
    }
    pub fn erase_backward(&mut self, kind: super::deletion::Backward) {
        self.snap();
        let end = self.cursor;
        let mut start = super::deletion::start(&self.text[..end], kind);
        for paste in &self.pastes {
            if paste.start < end && paste.end > start {
                start = start.min(paste.start);
            }
        }
        if start < end {
            self.erase(start, end);
        }
    }
    pub fn delete(&mut self) {
        let start = self.cursor;
        self.right();
        self.erase(start, self.cursor);
    }
    fn snap(&mut self) {
        if let Some(paste) = self
            .pastes
            .iter()
            .find(|p| p.start < self.cursor && self.cursor < p.end)
        {
            self.cursor = paste.end;
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            pastes: self.pastes.clone(),
        }
    }
    pub fn take_snapshot(&mut self) -> Snapshot {
        let snapshot = self.snapshot();
        self.take();
        snapshot
    }
    pub fn restore(&mut self, snapshot: Snapshot) {
        self.revision += 1;
        self.text = snapshot.text;
        self.cursor = snapshot.cursor;
        self.pastes = snapshot.pastes;
    }
    pub fn insert_paste(&mut self, text: &str) -> Result<()> {
        if text.len() < 4096 && text.split('\n').count() < 8 {
            self.insert(text);
            return Ok(());
        }
        self.add_paste(Text::new(text)?);
        Ok(())
    }
    pub fn add_paste(&mut self, source: Text) {
        self.next_paste += 1;
        self.add_paste_with_id(source, self.next_paste);
    }
    fn add_paste_with_id(&mut self, source: Text, id: usize) {
        self.snap();
        let start = self.cursor;
        let marker = format!("[Paste {id} · {} lines]", source.stats().lines);
        self.insert(&marker);
        let end = self.cursor;
        self.pastes.push(Paste {
            id,
            start,
            end,
            source,
        });
        self.pastes.sort_by_key(|p| p.start);
    }
    pub fn replace_paste(&mut self, id: usize, source: Text) -> bool {
        let Some(paste) = self.pastes.iter().find(|p| p.id == id).cloned() else {
            return false;
        };
        self.erase(paste.start, paste.end);
        self.add_paste_with_id(source, id);
        true
    }
    pub fn remove_paste(&mut self, id: usize) {
        if let Some(paste) = self.pastes.iter().find(|p| p.id == id).cloned() {
            self.erase(paste.start, paste.end);
        }
    }
    pub fn home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
    }
    pub fn end(&mut self) {
        self.snap();
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.text.len());
    }
    pub fn vertical(&mut self, down: bool) {
        let start = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let column = self.text[start..self.cursor].width();
        let next = if down {
            self.text[self.cursor..]
                .find('\n')
                .map(|i| self.cursor + i + 1)
        } else {
            start
                .checked_sub(1)
                .map(|end| self.text[..end].rfind('\n').map(|i| i + 1).unwrap_or(0))
        };
        if let Some(next) = next {
            let line = self.text[next..].split('\n').next().unwrap_or("");
            let mut width = 0;
            self.cursor = next;
            for (gidx, g) in line.grapheme_indices(true) {
                if width + g.width() > column {
                    break;
                }
                width += g.width();
                self.cursor = next + gidx + g.len();
            }
        }
        self.snap();
    }
    pub fn set_cell(&mut self, width: usize, row: usize, column: usize, masked: bool) {
        let mut y = 0;
        let mut x = 0;
        let mut nearest = self.text.len();
        for (offset, g) in self.text.grapheme_indices(true) {
            let cells = if masked {
                1
            } else if g == "\t" {
                4
            } else {
                g.width()
            };
            if g != "\n" && x > 0 && x + cells > width.max(1) {
                y += 1;
                x = 0;
            }
            if y > row || (y == row && (x >= column || x + cells > column)) {
                nearest = offset;
                break;
            }
            if g == "\n" {
                if y == row {
                    nearest = offset;
                    break;
                }
                y += 1;
                x = 0;
            } else {
                x += cells;
            }
        }
        self.cursor = nearest;
        self.snap();
    }
    pub fn paste_cells(&self, width: usize) -> Vec<(usize, usize, usize, usize)> {
        let mut cells: Vec<(usize, usize, usize, usize)> = vec![];
        let mut row = 0;
        let mut col = 0;
        let mut paste_index = 0;
        for (offset, g) in self.text.grapheme_indices(true) {
            let size = if g == "\t" { 4 } else { g.width() };
            if g != "\n" && col > 0 && col + size > width.max(1) {
                row += 1;
                col = 0;
            }
            while paste_index < self.pastes.len() && self.pastes[paste_index].end <= offset {
                paste_index += 1;
            }
            if let Some(paste) = self.pastes.get(paste_index).filter(|p| p.start <= offset) {
                if let Some(last) = cells.last_mut()
                    && last.0 == paste.id
                    && last.1 == row
                {
                    last.3 += size;
                } else {
                    cells.push((paste.id, row, col, size));
                }
            }
            if g == "\n" {
                row += 1;
                col = 0;
            } else {
                col += size;
            }
        }
        cells
    }
    pub fn display(&self, width: usize, masked: bool) -> (Vec<String>, usize, usize) {
        let width = width.max(1);
        let mut lines = vec![String::new()];
        let mut col = 0;
        let mut cursor_row = 0;
        let mut cursor_col = 0;
        for (index, g) in self.text.grapheme_indices(true) {
            let value = if masked { "*" } else { g };
            let size = if value == "\t" { 4 } else { value.width() };
            if value != "\n" && col + size > width && col > 0 {
                lines.push(String::new());
                col = 0;
            }
            if index == self.cursor {
                cursor_row = lines.len() - 1;
                cursor_col = col;
            }
            if value == "\n" {
                lines.push(String::new());
                col = 0;
            } else {
                lines
                    .last_mut()
                    .unwrap()
                    .push_str(if value == "\t" { "    " } else { value });
                col += size;
            }
        }
        if self.cursor == self.text.len() {
            cursor_row = lines.len() - 1;
            cursor_col = col;
        }
        if cursor_col >= width {
            lines.push(String::new());
            cursor_row += 1;
            cursor_col = 0;
        }
        (lines, cursor_row, cursor_col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backward_deletion_keeps_paste_nodes_atomic_and_other_offsets_valid() {
        use super::super::deletion::Backward;
        let mut editor = Editor::default();
        editor.insert("prefix ");
        editor.insert_paste(&"first\n".repeat(10)).unwrap();
        editor.insert(" between ");
        editor.insert_paste(&"second\n".repeat(10)).unwrap();
        let second = editor.pastes[1].clone();
        editor.cursor = editor.pastes[0].end;
        editor.erase_backward(Backward::Word);
        assert_eq!(
            editor.text,
            format!(
                "prefix  between [Paste 2 · {} lines]",
                second.source.stats().lines
            )
        );
        assert_eq!(editor.pastes.len(), 1);
        assert_eq!(editor.pastes[0].start, "prefix  between ".len());
        assert_eq!(
            editor.pastes[0].source.contents().unwrap(),
            "second\n".repeat(10)
        );
        editor.cursor = editor.text.len();
        editor.erase_backward(Backward::Line);
        assert!(editor.text.is_empty());
        assert!(editor.pastes.is_empty());
        editor.set("first\nsecond tail".into());
        editor.cursor = "first\nsecond".len();
        editor.erase_backward(Backward::Line);
        assert_eq!(editor.text, "first\n tail");
        let revision = editor.revision;
        editor.erase_backward(Backward::Line);
        assert_eq!(editor.revision, revision);
    }
    #[test]
    fn grapheme_editing() {
        let mut e = Editor::default();
        e.insert("a👨‍👩‍👧‍👦e\u{301}中");
        e.backspace();
        assert!(e.text.ends_with("e\u{301}"));
        e.backspace();
        assert!(e.text.ends_with("👨‍👩‍👧‍👦"));
        e.left();
        e.delete();
        assert_eq!(e.text, "a");
    }
    #[test]
    fn mask_and_wrapping_use_cells() {
        let mut e = Editor::default();
        e.insert("中🙂a");
        let (lines, row, col) = e.display(4, false);
        assert_eq!(lines[0], "中🙂");
        assert_eq!((row, col), (1, 1));
        assert_eq!(e.display(40, true).0[0], "***");
    }
    #[test]
    fn mixed_paste_nodes_preserve_order_and_are_atomic() {
        let mut e = Editor::default();
        let first = "one\n".repeat(12);
        let second = "中🙂".repeat(1000);
        e.insert("Before: ");
        e.insert_paste(&first).unwrap();
        e.insert(" between ");
        e.insert_paste(&second).unwrap();
        e.insert(" after");
        let doc = e.snapshot().document().unwrap();
        assert_eq!(
            doc.materialize(&tokio_util::sync::CancellationToken::new())
                .unwrap(),
            format!("Before: {first} between {second} after")
        );
        assert!(e.text.len() < 150);
        let paste = e.pastes[0].clone();
        e.cursor = paste.end;
        e.left();
        assert_eq!(e.cursor, paste.start);
        e.delete();
        assert_eq!(e.pastes.len(), 1);
        assert!(
            e.snapshot()
                .document()
                .unwrap()
                .materialize(&tokio_util::sync::CancellationToken::new())
                .unwrap()
                .contains(&second)
        );
        e.set("[Paste 1 · 12 lines]".into());
        assert!(e.pastes.is_empty());
        assert_eq!(
            e.snapshot()
                .document()
                .unwrap()
                .materialize(&tokio_util::sync::CancellationToken::new())
                .unwrap(),
            "[Paste 1 · 12 lines]"
        );
    }
    #[test]
    fn one_and_ten_mib_pastes_have_small_preview_and_complete_payload() {
        for size in [1024 * 1024, 10 * 1024 * 1024] {
            let text = "x".repeat(size);
            let mut e = Editor::default();
            e.insert_paste(&text).unwrap();
            assert!(e.text.len() < 80);
            let snapshot = e.take_snapshot();
            e.restore(snapshot.clone());
            assert_eq!(
                snapshot
                    .document()
                    .unwrap()
                    .materialize(&tokio_util::sync::CancellationToken::new())
                    .unwrap(),
                text
            );
            assert!(e.display(74, false).0.len() < 3);
        }
    }
}
