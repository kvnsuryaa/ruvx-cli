//! Disk-backed UTF-8 documents and sparse, incremental visual-line indexing.
use anyhow::{Context, Result, ensure};
use std::{
    collections::VecDeque,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    ops::Range,
    sync::{Arc, Mutex},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub const PAGE_BYTES: usize = 32 * 1024;
const STRIDE: usize = 128;
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub bytes: usize,
    pub lines: usize,
    pub chars: usize,
}
#[derive(Default)]
struct Sanitizer {
    state: u8,
}
impl Sanitizer {
    fn feed(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for ch in text.chars() {
            match self.state {
                0 if ch == '\x1b' => self.state = 1,
                0 if ch == '\n' || ch == '\t' || (!ch.is_control() && ch != '\x7f') => out.push(ch),
                1 => {
                    self.state = match ch {
                        '[' => 2,
                        ']' => 3,
                        _ => 0,
                    }
                }
                2 => {
                    if ('@'..='~').contains(&ch) {
                        self.state = 0;
                    }
                }
                3 => {
                    if ch == '\x07' {
                        self.state = 0;
                    } else if ch == '\x1b' {
                        self.state = 4;
                    }
                }
                4 => self.state = if ch == '\\' { 0 } else { 3 },
                _ => {}
            }
        }
        out
    }
}
struct Storage {
    stats: Stats,
    sanitizer: Sanitizer,
}
struct Backing {
    path: tempfile::TempPath,
    inner: Mutex<Storage>,
}
#[derive(Clone)]
pub struct Text {
    backing: Arc<Backing>,
}
impl Text {
    pub fn empty() -> Result<Self> {
        let temp = tempfile::NamedTempFile::new()?;
        let (file, path) = temp.into_parts();
        drop(file);
        Ok(Self {
            backing: Arc::new(Backing {
                path,
                inner: Mutex::new(Storage {
                    stats: Stats {
                        lines: 1,
                        ..Stats::default()
                    },
                    sanitizer: Sanitizer::default(),
                }),
            }),
        })
    }
    pub fn new(text: &str) -> Result<Self> {
        let result = Self::empty()?;
        result.append(text)?;
        Ok(result)
    }
    pub fn from_reader(reader: impl Read, stop: Option<&str>) -> Result<Self> {
        let result = Self::empty()?;
        let mut tail = String::new();
        let mut stopped = false;
        utf8_chunks(reader, |chunk| {
            if stopped {
                return Ok(false);
            }
            if let Some(marker) = stop {
                tail.push_str(chunk);
                if let Some(end) = tail.find(marker) {
                    result.append(&tail[..end])?;
                    stopped = true;
                    return Ok(false);
                }
                let mut cut = tail.len().saturating_sub(marker.len());
                while !tail.is_char_boundary(cut) {
                    cut -= 1;
                }
                result.append(&tail[..cut])?;
                tail = tail[cut..].to_owned();
            } else {
                result.append(chunk)?;
            }
            Ok(true)
        })?;
        if !stopped {
            result.append(&tail)?;
        }
        Ok(result)
    }
    pub fn append(&self, text: &str) -> Result<()> {
        let mut storage = self
            .backing
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("Text storage lock failed"))?;
        let clean = storage.sanitizer.feed(text);
        OpenOptions::new()
            .append(true)
            .open(&self.backing.path)?
            .write_all(clean.as_bytes())?;
        storage.stats.bytes += clean.len();
        storage.stats.lines += clean.bytes().filter(|b| *b == b'\n').count();
        storage.stats.chars += clean.chars().count();
        Ok(())
    }
    pub fn stats(&self) -> Stats {
        self.backing
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stats
    }
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.backing, &other.backing)
    }
    pub fn contents(&self) -> Result<String> {
        self.string(0..self.len())
    }
}

pub trait ReadText: Send + Sync {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn version(&self) -> u64;
    fn dirty_since(&self, version: u64) -> usize;
    fn bytes(&self, range: Range<usize>) -> Result<Vec<u8>>;
    fn string(&self, range: Range<usize>) -> Result<String> {
        String::from_utf8(self.bytes(range)?).context("Invalid UTF-8 text range")
    }
}
pub struct InlineText(pub String);
impl ReadText for InlineText {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn version(&self) -> u64 {
        0
    }
    fn dirty_since(&self, _version: u64) -> usize {
        0
    }
    fn bytes(&self, range: Range<usize>) -> Result<Vec<u8>> {
        Ok(self
            .0
            .as_bytes()
            .get(range)
            .context("Invalid inline text range")?
            .to_vec())
    }
}

impl ReadText for Text {
    fn len(&self) -> usize {
        self.stats().bytes
    }
    fn version(&self) -> u64 {
        self.len() as u64
    }
    fn dirty_since(&self, version: u64) -> usize {
        version as usize
    }
    fn bytes(&self, range: Range<usize>) -> Result<Vec<u8>> {
        ensure!(
            range.start <= range.end && range.end <= self.len(),
            "Invalid text range"
        );
        let mut file = File::open(&self.backing.path)?;
        file.seek(SeekFrom::Start(range.start as u64))?;
        let mut bytes = vec![0; range.len()];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }
}
#[derive(Clone)]
struct Piece {
    text: Text,
    range: Range<usize>,
}
#[derive(Clone)]
pub struct Document {
    pieces: Vec<Piece>,
    add: Text,
    len: usize,
    revision: u64,
    changes: VecDeque<(u64, usize)>,
}
impl Document {
    pub fn new() -> Result<Self> {
        Ok(Self {
            pieces: vec![],
            add: Text::empty()?,
            len: 0,
            revision: 0,
            changes: VecDeque::new(),
        })
    }
    pub fn from_text(text: Text) -> Result<Self> {
        let mut doc = Self::new()?;
        doc.push(text);
        Ok(doc)
    }
    pub fn push(&mut self, text: Text) {
        let end = text.len();
        let at = self.len;
        self.pieces.push(Piece {
            text,
            range: 0..end,
        });
        self.len += end;
        self.changed(at);
    }
    pub fn stats(&self) -> Result<Stats> {
        let mut stats = Stats {
            lines: 1,
            ..Stats::default()
        };
        for piece in &self.pieces {
            if piece.range.start == 0 && piece.range.end == piece.text.len() {
                let part = piece.text.stats();
                stats.bytes += part.bytes;
                stats.lines += part.lines - 1;
                stats.chars += part.chars;
            } else {
                let mut reader = File::open(&piece.text.backing.path)?;
                reader.seek(SeekFrom::Start(piece.range.start as u64))?;
                utf8_chunks(reader.take(piece.range.len() as u64), |part| {
                    stats.bytes += part.len();
                    stats.lines += part.bytes().filter(|b| *b == b'\n').count();
                    stats.chars += part.chars().count();
                    Ok(true)
                })?;
            }
        }
        Ok(stats)
    }
    fn changed(&mut self, at: usize) {
        self.revision += 1;
        self.changes.push_back((self.revision, at));
        if self.changes.len() > 128 {
            self.changes.pop_front();
        }
    }
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> Result<()> {
        ensure!(
            range.start <= range.end && range.end <= self.len,
            "Invalid edit range"
        );
        for at in [range.start, range.end] {
            if at < self.len {
                ensure!(
                    (self.bytes(at..at + 1)?[0] & 0xc0) != 0x80,
                    "Edit splits a UTF-8 character"
                );
            }
        }
        let start = self.add.len();
        self.add.append(text)?;
        let inserted = self.add.len() - start;
        let mut result = vec![];
        let mut offset = 0;
        let mut added = false;
        for piece in &self.pieces {
            let end = offset + piece.range.len();
            if offset < range.start && end > offset {
                let keep = (range.start - offset).min(piece.range.len());
                if keep > 0 {
                    result.push(Piece {
                        text: piece.text.clone(),
                        range: piece.range.start..piece.range.start + keep,
                    });
                }
            }
            if !added && end >= range.start {
                if inserted > 0 {
                    result.push(Piece {
                        text: self.add.clone(),
                        range: start..start + inserted,
                    });
                }
                added = true;
            }
            if end > range.end {
                let from = range.end.saturating_sub(offset).min(piece.range.len());
                result.push(Piece {
                    text: piece.text.clone(),
                    range: piece.range.start + from..piece.range.end,
                });
            }
            offset = end;
        }
        if !added && inserted > 0 {
            result.push(Piece {
                text: self.add.clone(),
                range: start..start + inserted,
            });
        }
        let mut merged: Vec<Piece> = vec![];
        for piece in result {
            if let Some(last) = merged.last_mut()
                && last.text.same(&piece.text)
                && last.range.end == piece.range.start
            {
                last.range.end = piece.range.end;
            } else if !piece.range.is_empty() {
                merged.push(piece);
            }
        }
        self.pieces = merged;
        self.len = self.len - range.len() + inserted;
        self.changed(range.start);
        Ok(())
    }
    pub fn save(&self) -> Result<Text> {
        let out = Text::empty()?;
        for piece in &self.pieces {
            let mut reader = File::open(&piece.text.backing.path)?;
            reader.seek(SeekFrom::Start(piece.range.start as u64))?;
            utf8_chunks(reader.take(piece.range.len() as u64), |chunk| {
                out.append(chunk)?;
                Ok(true)
            })?;
        }
        Ok(out)
    }
    pub fn materialize(&self, cancel: &tokio_util::sync::CancellationToken) -> Result<String> {
        let mut result = Vec::new();
        let mut buffer = [0u8; PAGE_BYTES];
        for piece in &self.pieces {
            ensure!(!cancel.is_cancelled(), "Preparation canceled");
            let mut file = File::open(&piece.text.backing.path)?;
            file.seek(SeekFrom::Start(piece.range.start as u64))?;
            let mut reader = file.take(piece.range.len() as u64);
            loop {
                ensure!(!cancel.is_cancelled(), "Preparation canceled");
                let n = reader.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                result.extend_from_slice(&buffer[..n]);
            }
        }
        ensure!(!cancel.is_cancelled(), "Preparation canceled");
        String::from_utf8(result).context("Invalid draft UTF-8")
    }
}
impl ReadText for Document {
    fn len(&self) -> usize {
        self.len
    }
    fn version(&self) -> u64 {
        self.revision
    }
    fn dirty_since(&self, version: u64) -> usize {
        if self.changes.front().is_some_and(|(v, _)| version + 1 < *v) {
            return 0;
        }
        self.changes
            .iter()
            .filter(|(v, _)| *v > version)
            .map(|(_, at)| *at)
            .min()
            .unwrap_or(self.len)
    }
    fn bytes(&self, range: Range<usize>) -> Result<Vec<u8>> {
        ensure!(
            range.start <= range.end && range.end <= self.len,
            "Invalid document range"
        );
        let mut result = Vec::with_capacity(range.len());
        let mut offset = 0;
        for piece in &self.pieces {
            let end = offset + piece.range.len();
            if range.start < end && range.end > offset {
                let start = range.start.saturating_sub(offset);
                let stop = (range.end - offset).min(piece.range.len());
                result.extend(
                    piece
                        .text
                        .bytes(piece.range.start + start..piece.range.start + stop)?,
                );
            }
            offset = end;
            if offset >= range.end {
                break;
            }
        }
        Ok(result)
    }
}

fn utf8_chunks(mut reader: impl Read, mut f: impl FnMut(&str) -> Result<bool>) -> Result<()> {
    let mut pending = Vec::new();
    let mut buffer = [0u8; PAGE_BYTES];
    loop {
        let n = reader.read(&mut buffer)?;
        pending.extend_from_slice(&buffer[..n]);
        let valid = match std::str::from_utf8(&pending) {
            Ok(_) => pending.len(),
            Err(e) => {
                ensure!(e.error_len().is_none(), "Invalid UTF-8 text");
                e.valid_up_to()
            }
        };
        if valid > 0 && !f(std::str::from_utf8(&pending[..valid])?)? {
            return Ok(());
        }
        pending.drain(..valid);
        if n == 0 {
            ensure!(pending.is_empty(), "Incomplete UTF-8 text");
            return Ok(());
        }
    }
}
struct Batch {
    rows: Vec<Range<usize>>,
    next: usize,
    eof: bool,
}
fn scan(source: &dyn ReadText, from: usize, width: usize, limit: usize) -> Result<Batch> {
    let mut rows = vec![];
    let mut row = from;
    let mut column = 0;
    let mut offset = from;
    let mut read = from;
    let mut pending = Vec::new();
    let len = source.len();
    loop {
        if read < len {
            let end = (read + PAGE_BYTES).min(len);
            pending.extend(source.bytes(read..end)?);
            read = end;
        }
        let valid = match std::str::from_utf8(&pending) {
            Ok(_) => pending.len(),
            Err(e) => {
                ensure!(e.error_len().is_none(), "Invalid UTF-8 text");
                e.valid_up_to()
            }
        };
        let text = std::str::from_utf8(&pending[..valid])?;
        let cut = if read == len {
            valid
        } else {
            text.grapheme_indices(true)
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0)
        };
        for (index, g) in text[..cut].grapheme_indices(true) {
            let at = offset + index;
            if g == "\n" {
                rows.push(row..at);
                row = at + 1;
                column = 0;
                if rows.len() == limit {
                    return Ok(Batch {
                        rows,
                        next: row,
                        eof: false,
                    });
                }
                continue;
            }
            let cells = if g == "\t" { 4 } else { g.width() };
            if column > 0 && column + cells > width.max(1) {
                rows.push(row..at);
                row = at;
                column = 0;
                if rows.len() == limit {
                    return Ok(Batch {
                        rows,
                        next: row,
                        eof: false,
                    });
                }
            }
            column += cells;
        }
        pending.drain(..cut);
        offset += cut;
        if read == len {
            ensure!(pending.is_empty(), "Incomplete UTF-8 text");
            rows.push(row..len);
            return Ok(Batch {
                rows,
                next: len,
                eof: true,
            });
        }
    }
}

#[derive(Default)]
pub struct Layout {
    width: usize,
    version: Option<u64>,
    checkpoints: Vec<(usize, usize)>,
    scanned: usize,
    next: usize,
    total: Option<usize>,
    pub scanned_bytes: usize,
}
impl Layout {
    fn sync(&mut self, source: &dyn ReadText, width: usize) {
        if self.width != width.max(1) || self.version.is_none() {
            *self = Self {
                width: width.max(1),
                version: Some(source.version()),
                checkpoints: vec![(0, 0)],
                ..Self::default()
            };
        } else if self.version != Some(source.version()) {
            let dirty = source.dirty_since(self.version.unwrap());
            let index = self
                .checkpoints
                .iter()
                .rposition(|(_, at)| *at < dirty)
                .unwrap_or(0);
            self.checkpoints.truncate(index + 1);
            (self.scanned, self.next) = self.checkpoints[index];
            self.total = None;
            self.version = Some(source.version());
        }
    }
    fn more(&mut self, source: &dyn ReadText) -> Result<()> {
        let batch = scan(source, self.next, self.width, STRIDE)?;
        self.scanned_bytes += batch.next - self.next;
        self.scanned += batch.rows.len();
        self.next = batch.next;
        if batch.eof {
            self.total = Some(self.scanned);
        } else {
            self.checkpoints.push((self.scanned, self.next));
        }
        Ok(())
    }
    pub fn count(&mut self, source: &dyn ReadText, width: usize) -> Result<usize> {
        self.sync(source, width);
        while self.total.is_none() {
            self.more(source)?;
        }
        Ok(self.total.unwrap())
    }
    pub fn rows(
        &mut self,
        source: &dyn ReadText,
        width: usize,
        start: usize,
        count: usize,
    ) -> Result<Vec<Range<usize>>> {
        self.sync(source, width);
        while self.total.is_none() && self.scanned < start.saturating_add(count) {
            self.more(source)?;
        }
        if self.total.is_some_and(|n| start >= n) {
            return Ok(vec![]);
        }
        let (row, offset) = self
            .checkpoints
            .iter()
            .rev()
            .find(|(r, _)| *r <= start)
            .copied()
            .unwrap_or((0, 0));
        let batch = scan(source, offset, self.width, start - row + count.max(1))?;
        Ok(batch
            .rows
            .into_iter()
            .skip(start - row)
            .take(count)
            .collect())
    }
    pub fn row_for_byte(
        &mut self,
        source: &dyn ReadText,
        width: usize,
        byte: usize,
    ) -> Result<usize> {
        self.sync(source, width);
        while self.total.is_none() && self.next <= byte {
            self.more(source)?;
        }
        let (row, offset) = self
            .checkpoints
            .iter()
            .rev()
            .find(|(_, at)| *at <= byte)
            .copied()
            .unwrap_or((0, 0));
        let batch = scan(source, offset, self.width, STRIDE + 1)?;
        Ok(row
            + batch
                .rows
                .iter()
                .rposition(|r| r.start <= byte)
                .unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_are_persistent_and_utf8_safe() {
        let mut doc = Document::from_text(Text::new("ab中🙂cd").unwrap()).unwrap();
        let old = doc.clone();
        assert!(doc.replace(3..4, "x").is_err());
        doc.replace(2..5, "e\u{301}").unwrap();
        assert_eq!(doc.string(0..doc.len()).unwrap(), "abe\u{301}🙂cd");
        doc.replace(0..2, "").unwrap();
        doc.replace(doc.len()..doc.len(), "!").unwrap();
        assert_eq!(old.string(0..old.len()).unwrap(), "ab中🙂cd");
        assert_eq!(doc.save().unwrap().contents().unwrap(), "e\u{301}🙂cd!");
    }
    #[test]
    fn sparse_layout_wraps_pages_and_updates_only_changed_tail() {
        let text = Text::new(&"中🙂e\u{301} ab\n".repeat(5000)).unwrap();
        let mut layout = Layout::default();
        assert_eq!(layout.count(&text, 80).unwrap(), 5001);
        assert_eq!(layout.rows(&text, 80, 3000, 1).unwrap().len(), 1);
        let before = layout.scanned_bytes;
        text.append("tail").unwrap();
        layout.count(&text, 80).unwrap();
        assert!(layout.scanned_bytes - before < 5000);
        let rows = layout.rows(&text, 4, 0, 3).unwrap();
        assert_eq!(text.string(rows[0].clone()).unwrap(), "中🙂");
        assert_eq!(text.string(rows[1].clone()).unwrap(), "e\u{301} ab");
    }
    #[test]
    fn streaming_escape_and_utf8_reader_are_safe() {
        let text = Text::empty().unwrap();
        text.append("abc\x1b[").unwrap();
        text.append("2Jd\x1b]52;c;secret").unwrap();
        text.append("\x07e").unwrap();
        assert_eq!(text.contents().unwrap(), "abcde");
        let input = format!("{}中🙂STOPignored", "a".repeat(PAGE_BYTES - 2));
        let text = Text::from_reader(input.as_bytes(), Some("STOP")).unwrap();
        assert!(text.contents().unwrap().ends_with("中🙂"));
        assert!(!text.contents().unwrap().contains("ignored"));
    }
}
