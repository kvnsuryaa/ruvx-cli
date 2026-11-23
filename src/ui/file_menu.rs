//! Transient @file completion. Discovery is debounced, cancellable, and off the UI thread.
use super::*;
use std::{ops::Range, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Reference {
    range: Range<usize>,
    query: String,
}

/// Match the same whitespace-delimited @ syntax as context::file_references, including fences.
/// Keep the entire token range so completing in its middle does not leave a stale suffix.
fn reference_at(text: &str, cursor: usize) -> Option<Reference> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let mut fenced = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && cursor >= offset && cursor <= offset + line.len() {
            let mut chars = line.char_indices().peekable();
            let mut previous = None;
            while let Some((index, ch)) = chars.next() {
                if ch == '@' && previous.is_none_or(char::is_whitespace) {
                    let start = index + 1;
                    let quoted = line[start..].starts_with('"');
                    let body = start + usize::from(quoted);
                    let closing = if quoted {
                        line[body..].find('"').map(|i| body + i)
                    } else {
                        None
                    };
                    let end = if quoted {
                        closing.map_or(line.trim_end_matches('\n').len(), |i| i + 1)
                    } else {
                        line[start..]
                            .find(char::is_whitespace)
                            .map_or(line.len(), |i| start + i)
                    };
                    let local_cursor = cursor - offset;
                    if local_cursor >= body
                        && local_cursor <= end
                        && closing.is_none_or(|i| local_cursor <= i)
                    {
                        return Some(Reference {
                            range: offset + index..offset + end,
                            query: line[body..local_cursor].to_owned(),
                        });
                    }
                    while chars.peek().is_some_and(|(i, _)| *i < end) {
                        chars.next();
                    }
                    previous = line[..end].chars().next_back();
                } else {
                    previous = Some(ch);
                }
            }
        }
        offset += line.len();
        if offset > cursor {
            break;
        }
    }
    None
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    revision: u64,
    cursor: usize,
    root: PathBuf,
    reference: Reference,
}

#[derive(Default)]
pub(super) struct Menu {
    key: Option<Key>,
    dismissed: Option<(u64, usize)>,
    request: u64,
    cancel: Option<CancellationToken>,
    pub selected: usize,
    pub items: Vec<String>,
    pub loading: bool,
    pub error: Option<String>,
}
impl Drop for Menu {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
    }
}

impl App {
    fn file_menu_key(&self) -> Option<Key> {
        if !matches!(self.dialog, Dialog::None)
            || self.paste_editor.is_some()
            || self.skill_browser.is_some()
            || self.mcp_browser.is_some()
            || self.active.is_some()
            || (self.editor.text.starts_with('/') && !self.editor.text.starts_with("//"))
            || self.file_menu.dismissed == Some((self.editor.revision, self.editor.cursor))
        {
            return None;
        }
        let reference = reference_at(&self.editor.text, self.editor.cursor)?;
        if self
            .editor
            .pastes
            .iter()
            .any(|p| p.start < reference.range.end && p.end > reference.range.start)
        {
            return None;
        }
        Some(Key {
            revision: self.editor.revision,
            cursor: self.editor.cursor,
            root: self.session.as_ref()?.root.clone(),
            reference,
        })
    }

    pub(super) fn file_matches(&mut self) -> Option<Vec<String>> {
        let key = self.file_menu_key();
        if self.file_menu.key != key {
            if let Some(cancel) = self.file_menu.cancel.take() {
                cancel.cancel();
            }
            self.file_menu.request += 1;
            self.file_menu.key = key.clone();
            self.file_menu.items.clear();
            self.file_menu.selected = 0;
            self.file_menu.error = None;
            self.file_menu.loading = key.is_some();
            if let Some(key) = key {
                let request = self.file_menu.request;
                let cancel = CancellationToken::new();
                self.file_menu.cancel = Some(cancel.clone());
                let config = self.config.tools.clone();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                    let worker_cancel = cancel.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        tools::file_suggestions(
                            &key.root,
                            &config,
                            &key.reference.query,
                            &worker_cancel,
                        )
                        .map_err(|e| format!("{e:#}"))
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                    if !cancel.is_cancelled() {
                        let _ = tx.send(UiEvent::FileSuggestions { request, result }).await;
                    }
                });
            }
        }
        self.file_menu.key.as_ref()?;
        Some(self.file_menu.items.clone())
    }

    pub(super) fn file_suggestions_finished(
        &mut self,
        request: u64,
        result: std::result::Result<Vec<String>, String>,
    ) {
        // A draft/cursor/dialog may have changed before its next render.
        self.file_matches();
        if request != self.file_menu.request || self.file_menu.key.is_none() {
            return;
        }
        self.file_menu.loading = false;
        match result {
            Ok(items) => self.file_menu.items = items,
            Err(error) => self.file_menu.error = Some(error),
        }
    }

    pub(super) fn move_file(&mut self, down: bool) {
        if let Some(items) = self.file_matches()
            && !items.is_empty()
        {
            let len = items.len();
            self.file_menu.selected =
                (self.file_menu.selected + if down { 1 } else { len - 1 }) % len;
        }
    }

    pub(super) fn choose_file(&mut self, path: &str) {
        if !self
            .file_matches()
            .is_some_and(|items| items.iter().any(|p| p == path))
        {
            return;
        }
        let range = self.file_menu.key.as_ref().unwrap().reference.range.clone();
        let reference = if path.chars().any(char::is_whitespace) {
            format!("@\"{path}\"")
        } else {
            format!("@{path}")
        };
        let mut snapshot = self.editor.snapshot();
        let suffix_has_space = snapshot.text[range.end..].starts_with(char::is_whitespace);
        let replacement = if suffix_has_space {
            reference
        } else {
            format!("{reference} ")
        };
        for paste in &mut snapshot.pastes {
            if paste.start >= range.end {
                paste.start = paste.start - range.len() + replacement.len();
                paste.end = paste.end - range.len() + replacement.len();
            }
        }
        snapshot.text.replace_range(range.clone(), &replacement);
        snapshot.cursor = range.start + replacement.len();
        if suffix_has_space {
            snapshot.cursor += snapshot.text[snapshot.cursor..]
                .chars()
                .next()
                .unwrap()
                .len_utf8();
        }
        self.editor.restore(snapshot);
        self.focused_button = None;
        self.gesture = None;
        self.view().selection = None;
        self.file_matches();
    }

    pub(super) fn file_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.is_empty() || self.focused_button.is_some() {
            return false;
        }
        let Some(items) = self.file_matches() else {
            return false;
        };
        match key.code {
            KeyCode::Up | KeyCode::Down => self.move_file(key.code == KeyCode::Down),
            KeyCode::Enter | KeyCode::Tab => {
                if let Some(path) = items.get(self.file_menu.selected) {
                    self.choose_file(path);
                }
                // Loading/no matches must not accidentally submit the prompt.
            }
            KeyCode::Esc => {
                self.file_menu.dismissed = Some((self.editor.revision, self.editor.cursor));
                self.focused_button = None;
                self.gesture = None;
                self.file_matches();
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::{fixture, render};

    fn key(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE), 40).unwrap();
    }
    async fn discover(app: &mut App, rx: &mut mpsc::Receiver<UiEvent>) {
        assert!(app.file_matches().is_some());
        while app.file_menu.loading {
            let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            app.event(event, 40).unwrap();
        }
        assert!(app.file_menu.error.is_none(), "{:?}", app.file_menu.error);
    }
    fn write(app: &App, path: &str) {
        let path = app.session.as_ref().unwrap().root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "file data\n").unwrap();
    }

    #[test]
    fn detects_references_at_cursor_not_emails_fences_or_completed_quotes() {
        for text in [
            "@",
            "explain @src/main.rs",
            "first @a then @src/main.rs",
            "日本語 @src/main.rs",
        ] {
            let reference = reference_at(text, text.len()).unwrap();
            assert_eq!(
                &text[reference.range],
                if text == "@" { "@" } else { "@src/main.rs" }
            );
        }
        for text in ["x@y.com", "hello@src", "```rust\n@src", "@\"a b\"", "@src "] {
            assert!(reference_at(text, text.len()).is_none(), "{text}");
        }
        let text = "```text\n@ignored\n```\nread @src";
        assert_eq!(reference_at(text, text.len()).unwrap().query, "src");
        let text = "read @\"docs/a b.md\" now";
        let cursor = text.find(" b.md").unwrap();
        let reference = reference_at(text, cursor).unwrap();
        assert_eq!(reference.query, "docs/a");
        assert_eq!(&text[reference.range], "@\"docs/a b.md\"");
        assert!(reference_at("é", 1).is_none());
        assert!(reference_at("@", 100).is_none());
    }

    #[tokio::test]
    async fn keyboard_selection_quotes_paths_and_never_submits() {
        let (_temp, mut app, mut rx) = fixture();
        write(&app, "docs/a b.md");
        write(&app, "docs/日本語.md");
        app.editor.set("explain @docs/".into());
        discover(&mut app, &mut rx).await;
        assert_eq!(app.file_menu.items.len(), 2);
        key(&mut app, KeyCode::Up);
        assert_eq!(app.file_menu.selected, 1);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.file_menu.selected, 0);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.editor.text, "explain @\"docs/a b.md\" ");
        assert!(app.file_matches().is_none());
        assert!(app.history.is_empty());
        assert!(app.active.is_none());
        assert!(rx.try_recv().is_err());
        app.editor.insert("and @日本");
        discover(&mut app, &mut rx).await;
        key(&mut app, KeyCode::Tab);
        assert_eq!(
            app.editor.text,
            "explain @\"docs/a b.md\" and @docs/日本語.md "
        );
        assert_eq!(
            context::file_references(&app.editor.text).unwrap(),
            vec!["docs/a b.md", "docs/日本語.md"]
        );
    }

    #[tokio::test]
    async fn loading_empty_and_escape_keep_draft_and_do_not_send() {
        let (_temp, mut app, mut rx) = fixture();
        app.editor.set("@missing".into());
        key(&mut app, KeyCode::Enter);
        assert!(app.file_menu.loading);
        assert_eq!(app.editor.text, "@missing");
        discover(&mut app, &mut rx).await;
        assert!(render(&mut app, 100, 40).contains("No matching files"));
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.editor.text, "@missing");
        key(&mut app, KeyCode::Esc);
        assert!(app.file_matches().is_none());
        assert_eq!(app.editor.text, "@missing");
        assert!(app.active.is_none());
        app.editor.insert("x");
        assert!(app.file_matches().is_some());
    }

    #[tokio::test]
    async fn stale_results_errors_and_clicks_cannot_replace_a_new_draft() {
        let (_temp, mut app, mut rx) = fixture();
        write(&app, "a.txt");
        app.editor.set("@a".into());
        discover(&mut app, &mut rx).await;
        let request = app.file_menu.request;
        app.editor.set("@b".into());
        app.file_suggestions_finished(request, Ok(vec!["a.txt".into()]));
        assert!(app.file_menu.items.is_empty());
        app.choose_file("a.txt");
        assert_eq!(app.editor.text, "@b");
        let request = app.file_menu.request;
        app.file_suggestions_finished(request, Err("test failure".into()));
        assert!(render(&mut app, 100, 40).contains("File search failed: test failure"));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.editor.text, "@b");
        app.editor.cursor = 0;
        app.file_suggestions_finished(request, Ok(vec!["a.txt".into()]));
        assert!(app.file_matches().is_none());
        assert!(app.file_menu.items.is_empty());
    }

    #[tokio::test]
    async fn completion_in_middle_preserves_suffix_and_long_paste_offsets() {
        let (_temp, mut app, mut rx) = fixture();
        write(&app, "src/main.rs");
        app.editor.set("read @src/old.rs next ".into());
        app.editor.insert_paste(&"long paste\n".repeat(1000)).unwrap();
        let source = app.editor.pastes[0].source.clone();
        app.editor.cursor = "read @src/".len();
        discover(&mut app, &mut rx).await;
        key(&mut app, KeyCode::Tab);
        assert!(app.editor.text.starts_with("read @src/main.rs next "));
        assert_eq!(app.editor.cursor, "read @src/main.rs ".len());
        let paste = &app.editor.pastes[0];
        assert_eq!(paste.start, "read @src/main.rs next ".len());
        assert!(paste.source.same(&source));
        let document = app.editor.snapshot().document().unwrap();
        assert!(
            document
                .materialize(&CancellationToken::new())
                .unwrap()
                .ends_with(&"long paste\n".repeat(1000))
        );
    }

    #[tokio::test]
    async fn menu_does_not_interfere_with_commands_dialogs_fences_or_paste_previews() {
        let (_temp, mut app, _rx) = fixture();
        app.editor.set("/diff @src".into());
        assert!(app.file_matches().is_none());
        app.editor.set("/".into());
        assert!(app.command_matches().is_some());
        assert!(app.file_matches().is_none());
        app.editor.set("```text\n@src".into());
        assert!(app.file_matches().is_none());
        app.editor.take();
        app.editor.insert_paste(&"@src\n".repeat(1000)).unwrap();
        assert!(app.file_matches().is_none());
        app.editor.insert(" @");
        assert!(app.file_matches().is_some());
        app.dialog = Dialog::Acknowledge;
        assert!(app.file_matches().is_none());
        app.dialog = Dialog::None;
        assert!(app.file_matches().is_some());
    }

    #[tokio::test]
    async fn mouse_keyboard_focus_and_small_layout_complete_without_sending() {
        let (_temp, mut app, mut rx) = fixture();
        write(&app, "a.txt");
        write(&app, "b.txt");
        app.editor.set("@".into());
        discover(&mut app, &mut rx).await;
        for (width, height) in [(100, 40), (48, 16), (24, 10), (8, 5)] {
            render(&mut app, width, height);
            assert!(app.frame.file_menu.bottom() <= app.frame.composer.y);
            assert!(app.frame.file_menu.height <= 7);
        }
        render(&mut app, 100, 40);
        let (rect, button) = app
            .frame
            .buttons
            .iter()
            .find(|(_, b)| matches!(b, screen::Button::FileChoice(p) if p == "b.txt"))
            .unwrap()
            .clone();
        let mouse = |kind| MouseEvent {
            kind,
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(mouse(MouseEventKind::ScrollDown), 40).unwrap();
        assert_eq!(app.file_menu.selected, 1);
        app.mouse(mouse(MouseEventKind::ScrollUp), 40).unwrap();
        assert_eq!(app.file_menu.selected, 0);
        app.mouse(mouse(MouseEventKind::Down(MouseButton::Left)), 40)
            .unwrap();
        app.mouse(mouse(MouseEventKind::Up(MouseButton::Left)), 40)
            .unwrap();
        assert_eq!(app.editor.text, "@b.txt ");
        app.activate(button, 40).unwrap(); // stale/repeated click
        assert_eq!(app.editor.text, "@b.txt ");
        assert!(app.active.is_none());
        assert!(app.history.is_empty());
        app.editor.set("@a".into());
        discover(&mut app, &mut rx).await;
        render(&mut app, 100, 40);
        let button = app
            .frame
            .buttons
            .iter()
            .find_map(|(_, b)| matches!(b, screen::Button::FileChoice(_)).then_some(b.clone()))
            .unwrap();
        app.focused_button = Some(button);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.editor.text, "@a.txt ");
    }
}
