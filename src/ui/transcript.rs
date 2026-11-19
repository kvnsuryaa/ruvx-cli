//! Source-coordinate transcript rendering: role, folding, and layout never alter message text.
use crate::{
    model::Turn,
    session::{self, Session, TranscriptEntry},
    text::{Document, InlineText, Layout, ReadText, Stats, Text},
    usage::Usage,
};
use anyhow::{Result, ensure};
use std::{ops::Range, path::PathBuf};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Role {
    User,
    Assistant,
    Tool,
    #[default]
    System,
}
impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "USER",
            Self::Assistant => "AI · ruvx",
            Self::Tool => "Tool",
            Self::System => "System",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub block: usize,
    pub byte: usize,
}
#[derive(Clone, Copy, Debug)]
pub struct Selection {
    pub anchor: Position,
    pub end: Position,
}
impl Selection {
    pub fn range(self) -> (Position, Position) {
        if self.anchor <= self.end {
            (self.anchor, self.end)
        } else {
            (self.end, self.anchor)
        }
    }
}
enum Source {
    Inline(InlineText),
    Text(Text),
    Document(Document),
    Journal {
        dir: PathBuf,
        entry: TranscriptEntry,
    },
}
impl Source {
    fn read(&self) -> &dyn ReadText {
        match self {
            Self::Text(text) => text,
            Self::Inline(text) => text,
            Self::Document(doc) => doc,
            Self::Journal { .. } => unreachable!("journal block is hydrated before rendering"),
        }
    }
    fn stats(&self) -> Result<Stats> {
        match self {
            Self::Text(text) => Ok(text.stats()),
            Self::Inline(text) => Ok(Stats {
                bytes: text.len(),
                lines: text.0.split('\n').count(),
                chars: text.0.chars().count(),
            }),
            Self::Document(doc) => doc.stats(),
            Self::Journal { .. } => unreachable!(),
        }
    }
}
struct Block {
    role: Role,
    label: String,
    source: Source,
    layout: Layout,
    detail: Option<Text>,
    collapsed: bool,
    folded: bool,
    foldable: bool,
    stats: Option<Stats>,
    usage: Option<Usage>,
    artifact_detail: Option<(PathBuf, session::Blob)>,
}
impl Block {
    fn heading_rows(&self) -> usize {
        if self.label.is_empty() {
            0
        } else if matches!(self.role, Role::User | Role::Assistant) {
            2
        } else {
            1
        }
    }
    fn new(role: Role, label: String, source: Source, collapsed: bool) -> Self {
        Self {
            role,
            label,
            source,
            layout: Layout::default(),
            detail: None,
            collapsed,
            folded: false,
            foldable: false,
            stats: None,
            usage: None,
            artifact_detail: None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Row {
    pub block: usize,
    pub range: Option<Range<usize>>,
    pub text: String,
    pub label: bool,
    pub detail: bool,
    pub fold: bool,
    pub role: Role,
    pub inset: usize,
}
impl Row {
    pub fn position(&self, column: usize, inclusive: bool) -> Option<Position> {
        let range = self.range.as_ref()?;
        let mut cells = 0;
        let column = column.saturating_sub(self.inset);
        for (offset, g) in self.text.grapheme_indices(true) {
            let width = if g == "\t" { 4 } else { g.width() };
            if cells + width > column {
                return Some(Position {
                    block: self.block,
                    byte: range.start + offset + if inclusive { g.len() } else { 0 },
                });
            }
            cells += width;
        }
        Some(Position {
            block: self.block,
            byte: range.end,
        })
    }
}
pub struct Transcript {
    blocks: Vec<Block>,
    top: (usize, usize),
    pub follow: bool,
    pub selection: Option<Selection>,
    pub bottom_seen: bool,
    pub new_blocks: usize,
    layout_width: usize,
}
impl Transcript {
    pub fn new() -> Result<Self> {
        Ok(Self {
            blocks: vec![],
            top: (0, 0),
            follow: true,
            selection: None,
            bottom_seen: false,
            new_blocks: 0,
            layout_width: 0,
        })
    }
    pub fn from_session(session: &Session) -> Result<Self> {
        let mut view = Self::new()?;
        for entry in session.transcript_entries()? {
            let role = match entry.kind.as_str() {
                "message.user" => Role::User,
                "response.completed" | "response.interrupted" => Role::Assistant,
                "tool.finished" => Role::Tool,
                _ => Role::System,
            };
            let label = if matches!(role, Role::User | Role::Assistant) {
                role.label().into()
            } else if role == Role::Tool {
                format!(
                    "{} → {}",
                    entry.label,
                    entry.status.as_deref().unwrap_or("finished")
                )
            } else {
                entry.label.clone()
            };
            view.blocks.push(Block::new(
                role,
                label,
                Source::Journal {
                    dir: session.dir.clone(),
                    entry,
                },
                role == Role::Tool,
            ));
        }
        Ok(view)
    }
    pub fn push(&mut self, label: impl Into<String>, text: &str, detail: Option<String>) -> usize {
        self.push_role(
            if detail.is_some() {
                Role::Tool
            } else {
                Role::System
            },
            label,
            text,
            detail,
        )
    }
    pub fn push_role(
        &mut self,
        role: Role,
        label: impl Into<String>,
        text: &str,
        detail: Option<String>,
    ) -> usize {
        let source = match Text::new(text) {
            Ok(source) => Source::Text(source),
            Err(error) => Source::Inline(InlineText(super::sanitize(&format!(
                "Unable to cache message: {error}"
            )))),
        };
        let mut block = Block::new(
            role,
            super::sanitize(&label.into()),
            source,
            detail.is_some(),
        );
        block.detail = detail.and_then(|text| Text::new(&text).ok());
        self.insert(block)
    }
    pub fn push_document(&mut self, role: Role, doc: Document) -> usize {
        self.insert(Block::new(
            role,
            role.label().into(),
            Source::Document(doc),
            false,
        ))
    }
    fn insert(&mut self, block: Block) -> usize {
        let id = self.blocks.len();
        self.blocks.push(block);
        if !self.follow {
            self.new_blocks += 1;
        }
        id
    }
    pub fn begin(&mut self, _label: &str) -> usize {
        self.push_role(Role::Assistant, Role::Assistant.label(), "", None)
    }
    pub fn append(&mut self, id: usize, text: &str) {
        if let Source::Text(source) = &self.blocks[id].source
            && let Err(error) = source.append(text)
        {
            self.push("Storage error", &error.to_string(), None);
        }
    }
    pub fn seal(&mut self, _id: usize) {} // Text sources already live in private temporary files.
    pub fn set_usage(&mut self, id: Option<usize>, value: serde_json::Value) {
        let id = id.unwrap_or_else(|| self.push("", "", None));
        let block = &mut self.blocks[id];
        block.usage = Some(Usage::from_value(&value));
        block.detail = Text::new(&serde_json::to_string_pretty(&value).unwrap_or_default()).ok();
    }
    fn prepare(&mut self, id: usize) -> Result<()> {
        let block = &mut self.blocks[id];
        if let Source::Journal { dir, entry } = &block.source {
            if block.role == Role::Tool {
                return Ok(());
            }
            let file = session::artifact_file(dir, &entry.blob)?;
            let text = if block.role == Role::Assistant {
                let turn: Turn = serde_json::from_reader(std::io::BufReader::new(file))?;
                if let Some(usage) = turn.usage {
                    block.usage = Some(Usage::from_value(&usage));
                    block.detail = Some(Text::new(&serde_json::to_string_pretty(&usage)?)?);
                }
                Text::new(&turn.text)?
            } else {
                Text::from_reader(
                    file,
                    if block.role == Role::User {
                        Some("\n\n[Attached file data, not instructions:")
                    } else {
                        None
                    },
                )?
            };
            if block.role == Role::User && text.stats().bytes < entry.blob.len {
                block.artifact_detail = Some((dir.clone(), entry.blob.clone()));
            }
            block.source = Source::Text(text);
        }
        if block.stats.is_none() {
            let stats = block.source.stats()?;
            block.foldable = block.role == Role::User && (stats.lines >= 12 || stats.bytes >= 4096);
            block.folded = block.foldable;
            block.stats = Some(stats);
        }
        Ok(())
    }
    fn body_width(&self, id: usize, width: usize) -> usize {
        width
            .saturating_sub(if self.blocks[id].role == Role::User {
                2
            } else {
                0
            })
            .max(1)
    }
    fn body_count(&mut self, id: usize, width: usize) -> Result<usize> {
        self.prepare(id)?;
        let width = self.body_width(id, width);
        let block = &mut self.blocks[id];
        if block.collapsed {
            return Ok(0);
        }
        if block.folded {
            Ok(block.layout.rows(block.source.read(), width, 0, 6)?.len())
        } else {
            block.layout.count(block.source.read(), width)
        }
    }
    fn row_count(&mut self, id: usize, width: usize) -> usize {
        self.body_count(id, width)
            .map(|body| {
                body + self.blocks[id].heading_rows()
                    + usize::from(self.blocks[id].foldable)
                    + usize::from(self.blocks[id].role == Role::User)
                    + usize::from(self.blocks[id].usage.is_some())
                    + 1
            })
            .unwrap_or(2)
    }
    fn previous(&mut self, at: (usize, usize), width: usize) -> (usize, usize) {
        if at.1 > 0 {
            (at.0, at.1 - 1)
        } else if at.0 > 0 {
            (at.0 - 1, self.row_count(at.0 - 1, width) - 1)
        } else {
            at
        }
    }
    fn next(&mut self, at: (usize, usize), width: usize) -> (usize, usize) {
        if at.1 + 1 < self.row_count(at.0, width) {
            (at.0, at.1 + 1)
        } else if at.0 + 1 < self.blocks.len() {
            (at.0 + 1, 0)
        } else {
            at
        }
    }
    pub fn scroll(&mut self, delta: isize, width: usize, height: usize) {
        if self.blocks.is_empty() {
            return;
        }
        if self.follow {
            self.visible(width, height);
        }
        self.follow = false;
        let last = self.blocks.len() - 1;
        let mut tail = (last, self.row_count(last, width) - 1);
        for _ in 1..height {
            tail = self.previous(tail, width);
        }
        for _ in 0..delta.unsigned_abs().min(10000) {
            self.top = if delta < 0 {
                self.previous(self.top, width)
            } else {
                self.next(self.top, width).min(tail)
            };
        }
    }
    pub fn bottom(&mut self) {
        self.follow = true;
        self.new_blocks = 0;
    }
    pub fn start(&mut self) {
        self.follow = false;
        self.top = (0, 0);
    }
    pub fn toggle(&mut self, id: usize) -> Result<()> {
        self.prepare(id)?;
        if self.blocks[id].foldable {
            self.blocks[id].folded = !self.blocks[id].folded;
            self.top = (id, 0);
            self.follow = false;
            self.selection = None;
        }
        Ok(())
    }
    fn row(&mut self, id: usize, index: usize, width: usize) -> Result<Row> {
        self.prepare(id)?;
        let body = self.body_count(id, width)?;
        let width = self.body_width(id, width);
        let block = &mut self.blocks[id];
        let heading = block.heading_rows();
        let mut row = Row {
            block: id,
            range: None,
            text: String::new(),
            label: false,
            detail: false,
            fold: false,
            role: block.role,
            inset: if block.role == Role::User { 1 } else { 0 },
        };
        if index == 0 && heading > 0 {
            row.text = block.label.clone();
            row.label = true;
            row.detail = block.collapsed || block.artifact_detail.is_some();
            if row.detail {
                row.text.push_str("  [Detail]");
            }
            return Ok(row);
        }
        if index < heading {
            return Ok(row);
        }
        let index = index.saturating_sub(heading);
        if index < body {
            if let Some(range) = block
                .layout
                .rows(block.source.read(), width, index, 1)?
                .pop()
            {
                row.text = block.source.read().string(range.clone())?;
                row.range = Some(range);
                row.inset = if block.role == Role::User { 1 } else { 0 };
            }
        } else if block.foldable && index == body {
            row.fold = true;
            let stats = block.stats.unwrap_or_default();
            row.text = if block.folded {
                format!(
                    "[Open message · {} lines · {} chars]",
                    stats.lines, stats.chars
                )
            } else {
                "[Close message]".into()
            };
        } else if block.role == Role::User && index == body + usize::from(block.foldable) {
            // Bottom padding belongs to the user surface; the next separator does not.
        } else if let Some(usage) = &block.usage
            && index == body + usize::from(block.foldable) + usize::from(block.role == Role::User)
        {
            row.text = format!("{}  [Detail]", usage.label());
            row.detail = true;
            row.role = Role::System;
        } else {
            row.role = Role::System;
            row.inset = 0;
        }
        Ok(row)
    }
    pub fn visible(&mut self, width: usize, height: usize) -> Vec<Row> {
        if self.blocks.is_empty() || height == 0 {
            return vec![];
        }
        if !self.follow && self.layout_width > 0 && self.layout_width != width {
            let id = self.top.0.min(self.blocks.len() - 1);
            let old = self
                .row(id, self.top.1, self.layout_width)
                .ok()
                .and_then(|r| r.range)
                .map(|r| r.start);
            if let Some(byte) = old {
                let newwidth = self.body_width(id, width);
                let block = &mut self.blocks[id];
                if let Ok(row) = block
                    .layout
                    .row_for_byte(block.source.read(), newwidth, byte)
                {
                    self.top.1 = row + block.heading_rows();
                }
            }
        }
        self.layout_width = width;
        if self.follow {
            let id = self.blocks.len() - 1;
            self.top = (id, self.row_count(id, width) - 1);
            for _ in 1..height {
                self.top = self.previous(self.top, width);
            }
        }
        self.top.0 = self.top.0.min(self.blocks.len() - 1);
        self.top.1 = self.top.1.min(self.row_count(self.top.0, width) - 1);
        let mut at = self.top;
        let mut rows = vec![];
        for _ in 0..height {
            rows.push(self.row(at.0, at.1, width).unwrap_or_else(|e| Row {
                block: at.0,
                range: None,
                text: format!("History error: {e}"),
                label: false,
                detail: false,
                fold: false,
                role: Role::System,
                inset: 0,
            }));
            let next = self.next(at, width);
            if next == at {
                self.bottom_seen = true;
                break;
            }
            at = next;
        }
        rows
    }
    pub fn selected(&mut self) -> Result<Option<String>> {
        let Some(selection) = self.selection else {
            return Ok(None);
        };
        let (start, end) = selection.range();
        if start == end {
            return Ok(None);
        }
        let mut output = String::new();
        for id in start.block..=end.block {
            self.prepare(id)?;
            let block = &self.blocks[id];
            if block.collapsed {
                continue;
            }
            let source = block.source.read();
            let from = if id == start.block {
                start.byte.min(source.len())
            } else {
                0
            };
            let mut to = if id == end.block {
                end.byte.min(source.len())
            } else {
                source.len()
            };
            if block.folded {
                let visible = self.visible_end(id)?;
                to = to.min(visible);
            }
            if from <= to {
                ensure!(
                    output.len() + to - from <= 16 * 1024 * 1024,
                    "Selection exceeds 16 MiB; select a smaller range"
                );
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(&self.blocks[id].source.read().string(from..to)?);
            }
        }
        Ok(Some(output))
    }
    fn visible_end(&mut self, id: usize) -> Result<usize> {
        let width = self.body_width(id, self.layout_width);
        let block = &mut self.blocks[id];
        Ok(block
            .layout
            .rows(block.source.read(), width, 0, 6)?
            .last()
            .map(|r| r.end)
            .unwrap_or(0))
    }
    pub fn detail_text(&mut self, id: usize) -> Result<Text> {
        if let Some((dir, blob)) = &self.blocks[id].artifact_detail {
            return Text::from_reader(session::artifact_file(dir, blob)?, None);
        }
        if let Some(detail) = &self.blocks[id].detail {
            return Ok(detail.clone());
        }
        let block = &self.blocks[id];
        if let Source::Journal { dir, entry } = &block.source {
            let value: serde_json::Value = serde_json::from_reader(std::io::BufReader::new(
                session::artifact_file(dir, &entry.blob)?,
            ))?;
            if block.role == Role::Tool {
                return Text::new(&format!(
                    "Tool: {}\nCall ID: {}\nStatus: {}\n\nResult:\n{}",
                    entry.label,
                    entry.call_id.as_deref().unwrap_or("unknown"),
                    entry
                        .status
                        .as_deref()
                        .or_else(|| value["status"].as_str())
                        .unwrap_or("finished"),
                    serde_json::to_string_pretty(&value)?,
                ));
            }
            return Text::new(&serde_json::to_string_pretty(&value)?);
        }
        match &block.source {
            Source::Text(text) => Ok(text.clone()),
            Source::Inline(text) => Text::new(&text.0),
            Source::Document(doc) => doc.save(),
            _ => unreachable!(),
        }
    }
    pub fn push_text(&mut self, text: Text) -> usize {
        self.insert(Block::new(
            Role::System,
            String::new(),
            Source::Text(text),
            false,
        ))
    }
    #[cfg(test)]
    fn detail(&mut self, id: usize) -> Result<String> {
        self.detail_text(id)?.contents()
    }
    #[cfg(test)]
    fn load(&mut self, id: usize) -> Result<String> {
        self.prepare(id)?;
        self.blocks[id]
            .source
            .read()
            .string(0..self.blocks[id].source.read().len())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn resumed_tools_hide_ids_and_read_full_details_lazily_including_old_journals() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::config::Paths::at(&temp.path().join("state"));
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        let mut session = Session::create(&paths, &root).unwrap();
        session
            .tool_result(
                "call_new",
                "read_file",
                &serde_json::json!({"status":"ok","content":"full result"}),
            )
            .unwrap();
        let blob = session
            .blob(br#"{"status":"error","error":"legacy failure"}"#)
            .unwrap();
        session
            .record(
                "tool.finished",
                serde_json::json!({"call_id":"call_old","name":"apply_patch","blob":blob}),
            )
            .unwrap();
        let mut view = Transcript::from_session(&session).unwrap();
        assert!(
            view.blocks.iter().all(
                |block| matches!(block.source, Source::Journal { .. }) && block.stats.is_none()
            )
        );
        let rows = view.visible(80, 30);
        let labels: Vec<_> = rows
            .iter()
            .filter(|row| row.label)
            .map(|row| row.text.as_str())
            .collect();
        assert_eq!(
            labels,
            [
                "read_file → ok  [Detail]",
                "apply_patch → finished  [Detail]"
            ]
        );
        assert!(!rows.iter().any(|row| row.text.contains("call_")));
        let detail = view.detail(0).unwrap();
        assert!(detail.contains("Call ID: call_new"));
        assert!(detail.contains("full result"));
        let detail = view.detail(1).unwrap();
        assert!(detail.contains("Call ID: call_old"));
        assert!(detail.contains("Status: error"));
        assert!(detail.contains("legacy failure"));
        assert!(
            view.blocks
                .iter()
                .all(|block| matches!(block.source, Source::Journal { .. }))
        );
    }
    #[test]
    fn role_heading_spacing_is_virtual_and_preserves_copy() {
        let mut view = Transcript::new().unwrap();
        for role in [Role::User, Role::Assistant] {
            let id = view.push_role(role, role.label(), "    中🙂\nnext", None);
            assert!(view.row(id, 0, 80).unwrap().label);
            let gap = view.row(id, 1, 80).unwrap();
            assert_eq!(gap.role, role);
            assert!(gap.text.is_empty());
            assert!(gap.range.is_none());
            let body = view.row(id, 2, 80).unwrap();
            assert_eq!(body.text, "    中🙂");
            assert_eq!(body.range.unwrap().start, 0);
            view.selection = Some(Selection {
                anchor: Position { block: id, byte: 0 },
                end: Position {
                    block: id,
                    byte: "    中🙂\nnext".len(),
                },
            });
            assert_eq!(view.selected().unwrap().unwrap(), "    中🙂\nnext");
        }
    }
    #[test]
    fn unicode_soft_wrap_copy_preserves_source() {
        let mut view = Transcript::new().unwrap();
        let id = view.push("Agent", "    中🙂e\u{301}\nnext", None);
        let rows = view.visible(5, 30);
        let text_rows: Vec<_> = rows.iter().filter(|r| r.range.is_some()).collect();
        assert!(text_rows.len() > 2);
        view.selection = Some(Selection {
            anchor: Position { block: id, byte: 0 },
            end: Position {
                block: id,
                byte: "    中🙂e\u{301}\nnext".len(),
            },
        });
        assert_eq!(view.selected().unwrap().unwrap(), "    中🙂e\u{301}\nnext");
        let emoji = text_rows.iter().find(|r| r.text.contains('🙂')).unwrap();
        let p = emoji.position(0, true).unwrap();
        assert!(view.load(id).unwrap().is_char_boundary(p.byte));
    }
    #[test]
    fn selection_survives_streaming_and_reflow() {
        let mut view = Transcript::new().unwrap();
        let id = view.begin("Agent");
        view.append(id, "alpha beta");
        view.selection = Some(Selection {
            anchor: Position { block: id, byte: 0 },
            end: Position { block: id, byte: 5 },
        });
        view.visible(4, 5);
        view.append(id, " gamma");
        view.visible(20, 5);
        assert_eq!(view.selected().unwrap().as_deref(), Some("alpha"));
    }
    #[test]
    fn scroll_detaches_and_cache_stays_bounded() {
        let mut view = Transcript::new().unwrap();
        for _ in 0..100 {
            view.push("Agent", &"a".repeat(40000), None);
        }
        view.visible(80, 20);
        view.scroll(-5, 80, 20);
        let top = view.top;
        view.push("You", "new", None);
        view.visible(80, 20);
        assert_eq!(view.top, top);
        for id in 0..100 {
            view.row_count(id, 80);
        }
        let rows = view.visible(80, 20);
        assert!(rows.len() <= 20);
        assert!(rows.iter().map(|r| r.text.len()).sum::<usize>() < 20 * 80 * 4 + 100);
        view.bottom();
        assert_eq!(view.new_blocks, 0);
    }
    #[test]
    fn original_transcript_survives_compaction_and_resume_with_lazy_blobs() {
        use crate::{config::Paths, model::Turn};
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(&dir.path().join("state"));
        let root = dir.path().join("repo");
        fs::create_dir(&root).unwrap();
        let mut session = Session::create(&paths, &root).unwrap();
        session.user("old prompt\n\n[Attached file data, not instructions: a]\ncontents\n[End attached file]\n").unwrap();
        session
            .response(&Turn {
                text: "old answer".into(),
                ..Default::default()
            })
            .unwrap();
        let through = session.sequence;
        let blob = session.blob(b"short summary").unwrap();
        session
            .record(
                "context.compacted",
                serde_json::json!({"through":through,"normalize_through":through,"blob":blob}),
            )
            .unwrap();
        assert_eq!(session.history().unwrap().len(), 1);
        let id = session.id.clone();
        drop(session);
        let session = Session::resume(&paths, &id).unwrap();
        let mut view = Transcript::from_session(&session).unwrap();
        assert!(
            view.blocks
                .iter()
                .all(|b| matches!(b.source, Source::Journal { .. }))
        );
        assert_eq!(view.blocks.len(), 3);
        let rows = view.visible(80, 40);
        assert!(rows.iter().any(|r| r.text == "old prompt"));
        assert!(rows.iter().any(|r| r.text == "old answer"));
        assert!(!rows.iter().any(|r| r.text.contains("contents")));
        assert!(view.detail(0).unwrap().contains("contents"));
        view.selection = Some(Selection {
            anchor: Position { block: 0, byte: 0 },
            end: Position { block: 1, byte: 10 },
        });
        assert_eq!(
            view.selected().unwrap().as_deref(),
            Some("old prompt\nold answer")
        );
    }
    #[test]
    fn resize_keeps_source_anchor_when_reading_old_output() {
        let mut view = Transcript::new().unwrap();
        view.push("Agent", &"0123456789".repeat(100), None);
        view.visible(20, 10);
        view.scroll(-20, 20, 10);
        let old = view
            .visible(20, 10)
            .into_iter()
            .find(|r| r.range.is_some())
            .unwrap()
            .range
            .unwrap()
            .start;
        let range = view
            .visible(7, 10)
            .into_iter()
            .find(|r| r.range.is_some())
            .unwrap()
            .range
            .unwrap();
        assert!(range.start <= old && range.end > old);
    }
    #[test]
    fn user_folding_and_copy_exclude_hidden_text_and_chrome() {
        let mut view = Transcript::new().unwrap();
        let text = (0..20)
            .map(|i| format!("    line {i}\n"))
            .collect::<String>();
        let id = view.push_role(Role::User, "USER", &text, None);
        let rows = view.visible(80, 40);
        assert!(rows.iter().any(|r| r.fold));
        assert!(rows.iter().filter(|r| r.range.is_some()).count() <= 6);
        assert!(
            rows.iter()
                .filter(|r| r.range.is_some())
                .all(|r| r.role == Role::User && r.inset == 1)
        );
        view.selection = Some(Selection {
            anchor: Position { block: id, byte: 0 },
            end: Position {
                block: id,
                byte: text.len(),
            },
        });
        let copied = view.selected().unwrap().unwrap();
        assert!(!copied.contains("line 19"));
        assert!(!copied.contains("Open message"));
        view.toggle(id).unwrap();
        view.selection = Some(Selection {
            anchor: Position { block: id, byte: 0 },
            end: Position {
                block: id,
                byte: text.len(),
            },
        });
        assert_eq!(view.selected().unwrap().unwrap(), text);
        let ai = view.push_role(Role::Assistant, "AI · ruvx", &text, None);
        view.visible(80, 40);
        assert!(!view.blocks[ai].folded);
    }
    #[test]
    fn ten_thousand_messages_hydrate_only_visible_sources_and_large_preview_is_paged() {
        let mut view = Transcript::new().unwrap();
        for _ in 0..10000 {
            view.push_role(Role::Assistant, "AI · ruvx", "reply", None);
        }
        assert_eq!(view.visible(80, 40).len(), 40);
        assert!(view.blocks.iter().filter(|b| b.stats.is_some()).count() < 50);
        let text = Text::new(&"x".repeat(10 * 1024 * 1024)).unwrap();
        let id = view.push_document(Role::User, Document::from_text(text).unwrap());
        view.visible(80, 40);
        assert!(view.blocks[id].folded);
        assert!(view.blocks[id].layout.scanned_bytes < 32768);
    }
}
