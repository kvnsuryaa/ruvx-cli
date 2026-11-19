//! Backward editing shared by the composer and the paged paste editor.
use crate::text::ReadText;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy)]
pub(super) enum Backward {
    Word,
    Line,
}

pub(super) fn key(key: KeyEvent) -> Option<Backward> {
    if key.kind == KeyEventKind::Release
        || key
            .modifiers
            .intersects(KeyModifiers::SUPER | KeyModifiers::META)
    {
        return None;
    }
    match key.code {
        KeyCode::Backspace
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
        {
            Some(Backward::Word)
        }
        KeyCode::Char('w') if key.modifiers == KeyModifiers::CONTROL => Some(Backward::Word),
        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => Some(Backward::Line),
        _ => None,
    }
}

pub(super) fn start(prefix: &str, kind: Backward) -> usize {
    match kind {
        Backward::Line => prefix.rfind('\n').map_or(0, |at| at + 1),
        Backward::Word => {
            let text = prefix.trim_end_matches(char::is_whitespace);
            let boundary = text
                .split_word_bound_indices()
                .next_back()
                .map_or(0, |(at, _)| at);
            // Never split an extended grapheme, including emoji sequences.
            prefix
                .grapheme_indices(true)
                .take_while(|(at, _)| *at <= boundary)
                .last()
                .map_or(0, |(at, _)| at)
        }
    }
}

/// Only expand the backward window when the boundary lies outside it.
pub(super) fn paged_start(source: &dyn ReadText, cursor: usize, kind: Backward) -> Result<usize> {
    let mut window = 4096usize;
    loop {
        let from = cursor.saturating_sub(window);
        let bytes = source.bytes(from..cursor)?;
        let trim = bytes.iter().take_while(|b| (**b & 0xc0) == 0x80).count();
        let from = from + trim;
        let text = std::str::from_utf8(&bytes[trim..])?;
        let offset = start(text, kind);
        if offset > 0 || from == 0 {
            return Ok(from + offset);
        }
        window = window.saturating_mul(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Text;
    #[test]
    fn word_boundaries_handle_whitespace_punctuation_unicode_and_emoji() {
        for (text, left) in [
            ("hello world  ", "hello "),
            ("hello\nworld", "hello\n"),
            ("foo/bar.rs", "foo/"),
            ("don't", ""),
            ("a e\u{301}", "a "),
            ("hello 👨‍👩‍👧‍👦", "hello "),
            ("hi 中", "hi "),
            ("  \n", ""),
            ("", ""),
        ] {
            assert_eq!(&text[..start(text, Backward::Word)], left, "{text:?}");
        }
    }
    #[test]
    fn paged_boundaries_cross_windows_without_splitting_long_graphemes() {
        for tail in ["word".repeat(5000), format!("e{}", "\u{301}".repeat(6000))] {
            let text = Text::new(&format!("before\n{tail}")).unwrap();
            for kind in [Backward::Word, Backward::Line] {
                assert_eq!(
                    paged_start(&text, text.len(), kind).unwrap(),
                    "before\n".len()
                );
            }
        }
        let text = Text::new("first\nsecond").unwrap();
        assert_eq!(paged_start(&text, 6, Backward::Line).unwrap(), 6);
        assert_eq!(paged_start(&text, 0, Backward::Word).unwrap(), 0);
    }
}
