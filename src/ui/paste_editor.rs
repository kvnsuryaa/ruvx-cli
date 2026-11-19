use crate::text::{Document, Layout, ReadText, Text};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub type Display = (Vec<String>, Option<(usize, usize)>);

pub struct PasteEditor {
    pub id: usize,
    pub document: Document,
    pub cursor: usize,
    pub top: usize,
    layout: Layout,
    width: usize,
    height: usize,
    pub rows: Vec<Range<usize>>,
    follow: bool,
}
impl PasteEditor {
    pub fn new(id: usize, text: Text) -> Result<Self> {
        Ok(Self {
            id,
            document: Document::from_text(text)?,
            cursor: 0,
            top: 0,
            layout: Layout::default(),
            width: 80,
            height: 20,
            rows: vec![],
            follow: true,
        })
    }
    pub fn display(&mut self, width: usize, height: usize) -> Result<Display> {
        self.width = width.max(1);
        self.height = height.max(1);
        let row = self
            .layout
            .row_for_byte(&self.document, self.width, self.cursor)?;
        if self.follow {
            if row < self.top {
                self.top = row;
            }
            if row >= self.top + self.height {
                self.top = row - self.height + 1;
            }
        }
        self.rows = self
            .layout
            .rows(&self.document, self.width, self.top, self.height)?;
        if self.rows.is_empty() && self.top > 0 {
            self.top = self
                .layout
                .count(&self.document, self.width)?
                .saturating_sub(self.height);
            self.rows = self
                .layout
                .rows(&self.document, self.width, self.top, self.height)?;
        }
        let cursor = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.start <= self.cursor && self.cursor <= r.end)
            .next_back()
            .map(|(i, r)| {
                (
                    self.document
                        .string(r.start..self.cursor)
                        .map(|s| {
                            s.graphemes(true)
                                .map(|g| if g == "\t" { 4 } else { g.width() })
                                .sum()
                        })
                        .unwrap_or(0),
                    i,
                )
            });
        let lines = self
            .rows
            .iter()
            .map(|r| {
                self.document
                    .string(r.clone())
                    .map(|s| s.replace('\t', "    "))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((lines, cursor))
    }
    fn before(&self) -> Result<usize> {
        if self.cursor == 0 {
            return Ok(0);
        }
        let mut window = 4096usize;
        loop {
            let mut start = self.cursor.saturating_sub(window);
            let bytes = self.document.bytes(start..self.cursor)?;
            let trim = bytes.iter().take_while(|b| (**b & 0xc0) == 0x80).count();
            start += trim;
            let text = std::str::from_utf8(&bytes[trim..])?;
            let offset = text
                .grapheme_indices(true)
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            if offset > 0 || start == 0 {
                return Ok(start + offset);
            }
            window = window.saturating_mul(2);
        }
    }
    fn after(&self) -> Result<usize> {
        if self.cursor == self.document.len() {
            return Ok(self.cursor);
        }
        let mut window = 4096usize;
        loop {
            let mut end = self.cursor.saturating_add(window).min(self.document.len());
            let mut bytes = self.document.bytes(self.cursor..end)?;
            while std::str::from_utf8(&bytes).is_err() {
                bytes.pop();
                end -= 1;
            }
            let size = std::str::from_utf8(&bytes)?
                .graphemes(true)
                .next()
                .map(str::len)
                .unwrap_or(0);
            if size < bytes.len() || end == self.document.len() {
                return Ok(self.cursor + size);
            }
            window = window.saturating_mul(2);
        }
    }
    pub fn insert(&mut self, text: &str) -> Result<()> {
        let before = self.document.len();
        self.document.replace(self.cursor..self.cursor, text)?;
        self.cursor += self.document.len() - before;
        self.follow = true;
        Ok(())
    }
    pub fn key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        if matches!(key.code, KeyCode::Backspace | KeyCode::Delete)
            && key
                .modifiers
                .intersects(KeyModifiers::SUPER | KeyModifiers::META)
        {
            return Ok(());
        }
        if let Some(kind) = super::deletion::key(key) {
            let start = super::deletion::paged_start(&self.document, self.cursor, kind)?;
            if start < self.cursor {
                self.document.replace(start..self.cursor, "")?;
                self.cursor = start;
            }
            self.follow = true;
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Home => self.cursor = 0,
                KeyCode::End => self.cursor = self.document.len(),
                _ => return Ok(()),
            }
            self.follow = true;
            return Ok(());
        }
        self.follow = true;
        match key.code {
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::META) =>
            {
                self.insert(&ch.to_string())?
            }
            KeyCode::Enter => self.insert("\n")?,
            KeyCode::Tab => self.insert("    ")?,
            KeyCode::Left => self.cursor = self.before()?,
            KeyCode::Right => self.cursor = self.after()?,
            KeyCode::Backspace => {
                let start = self.before()?;
                self.document.replace(start..self.cursor, "")?;
                self.cursor = start;
            }
            KeyCode::Delete => {
                let end = self.after()?;
                self.document.replace(self.cursor..end, "")?;
            }
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End => {
                let row = self
                    .layout
                    .row_for_byte(&self.document, self.width, self.cursor)?;
                let current = self
                    .layout
                    .rows(&self.document, self.width, row, 1)?
                    .pop()
                    .unwrap_or(0..0);
                let column = self
                    .document
                    .string(current.start..self.cursor)?
                    .graphemes(true)
                    .map(|g| if g == "\t" { 4 } else { g.width() })
                    .sum();
                let target = match key.code {
                    KeyCode::Up => row.saturating_sub(1),
                    KeyCode::Down => row + 1,
                    KeyCode::PageUp => row.saturating_sub(self.height),
                    KeyCode::PageDown => row + self.height,
                    _ => row,
                };
                if let Some(range) = self
                    .layout
                    .rows(&self.document, self.width, target, 1)?
                    .pop()
                {
                    let col = if key.code == KeyCode::Home {
                        0
                    } else if key.code == KeyCode::End {
                        usize::MAX
                    } else {
                        column
                    };
                    self.cursor = self.at_column(&range, col)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn at_column(&self, range: &Range<usize>, column: usize) -> Result<usize> {
        let text = self.document.string(range.clone())?;
        let mut cells = 0;
        for (offset, g) in text.grapheme_indices(true) {
            let size = if g == "\t" { 4 } else { g.width() };
            if cells + size > column {
                return Ok(range.start + offset);
            }
            cells += size;
        }
        Ok(range.end)
    }
    pub fn click(&mut self, column: usize, row: usize) -> Result<()> {
        if let Some(range) = self.rows.get(row) {
            self.cursor = self.at_column(range, column)?;
        }
        self.follow = true;
        Ok(())
    }
    pub fn scroll(&mut self, delta: isize) {
        self.top = self.top.saturating_add_signed(delta);
        self.follow = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn word_and_line_shortcuts_edit_large_documents_without_dropping_the_prefix() {
        let prefix = "unchanged\n".repeat(100000);
        let mut edit = PasteEditor::new(1, Text::new(&format!("{prefix}one 👨‍👩‍👧‍👦")).unwrap()).unwrap();
        edit.key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL))
            .unwrap();
        edit.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
            .unwrap();
        assert_eq!(edit.document.len(), prefix.len() + "one ".len());
        assert_eq!(
            edit.document
                .string(prefix.len()..edit.document.len())
                .unwrap(),
            "one "
        );
        edit.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::SUPER))
            .unwrap();
        assert_eq!(edit.document.len(), prefix.len() + "one ".len());
        edit.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(edit.document.len(), prefix.len());
        assert_eq!(edit.document.save().unwrap().contents().unwrap(), prefix);
        edit.insert("new words").unwrap();
        edit.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(
            edit.document
                .string(prefix.len()..edit.document.len())
                .unwrap(),
            "new "
        );
        edit.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(edit.document.len(), prefix.len());
    }
    #[test]
    fn editing_large_document_keeps_full_source_without_reflowing_tail() {
        let text = format!("start 中🙂\n{}end", "line\n".repeat(20000));
        let mut edit = PasteEditor::new(1, Text::new(&text).unwrap()).unwrap();
        edit.display(74, 20).unwrap();
        let before = edit.layout.scanned_bytes;
        edit.insert("prefix ").unwrap();
        edit.display(74, 20).unwrap();
        assert!(edit.layout.scanned_bytes - before < 32768);
        edit.key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL))
            .unwrap();
        edit.insert(" tail").unwrap();
        assert_eq!(
            edit.document.save().unwrap().contents().unwrap(),
            format!("prefix {text} tail")
        );
        edit.display(30, 10).unwrap();
        edit.scroll(1000000);
        assert!(!edit.display(30, 10).unwrap().0.is_empty());
    }
    #[test]
    fn deletion_respects_extended_graphemes_larger_than_one_page() {
        let grapheme = format!("e{}", "\u{301}".repeat(5000));
        let mut edit = PasteEditor::new(1, Text::new(&format!("{grapheme}中🙂")).unwrap()).unwrap();
        edit.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(edit.cursor, grapheme.len());
        edit.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(edit.document.save().unwrap().contents().unwrap(), "中🙂");
    }
}
