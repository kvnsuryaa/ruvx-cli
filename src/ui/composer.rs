use super::*;
use crate::{
    model::Message,
    text::{Document, Text},
};
use editor::Snapshot;
use paste_editor::PasteEditor;

pub(super) struct Preflight {
    pub revision: u64,
    pub snapshot: Snapshot,
    pub document: Document,
}
impl App {
    pub(super) fn paste(&mut self, text: &str) -> Result<()> {
        let text = sanitize(&text.replace("\r\n", "\n").replace('\r', "\n"));
        if let Some(editor) = &mut self.paste_editor {
            editor.insert(&text)?;
        } else if matches!(self.dialog, Dialog::None) {
            self.editor.insert_paste(&text)?;
        } else {
            self.editor.insert(&text);
        }
        self.invalidate_preflight();
        Ok(())
    }
    pub(super) fn invalidate_preflight(&mut self) {
        if self
            .preflight
            .as_ref()
            .is_some_and(|p| p.revision != self.editor.revision)
        {
            if let Some(cancel) = &self.active {
                cancel.cancel();
            }
            self.status =
                "Draft changed; preparation canceled. Press Enter again when ready.".into();
        }
    }
    pub(super) fn prepare_submission(&mut self, mut snapshot: Snapshot) -> Result<()> {
        ensure!(
            self.active.is_none(),
            "Work is active. Your draft is preserved; Ctrl+C cancels."
        );
        let session = self.session.as_ref().context("Session unavailable")?;
        let selection = session
            .selection
            .as_ref()
            .context("No model selected. Use /connect and /model")?;
        let profile = self
            .config
            .providers
            .get(&selection.profile)
            .context("Selected profile missing; use /connect")?;
        let model = profile.model(&selection.model)?;
        ensure!(
            model.supports_tools != Some(false) && model.supports_streaming,
            "Selected model does not support agent tools/streaming"
        );
        model.validate_effort(profile.kind, &selection.model)?;
        let original = snapshot.clone();
        if snapshot.pastes.is_empty() && snapshot.text.starts_with("//") {
            snapshot.text.remove(0);
            snapshot.cursor = snapshot.cursor.saturating_sub(1);
        }
        let document = snapshot.document()?;
        let work = document.clone();
        let revision = self.editor.revision;
        let session = self.session.take().unwrap();
        let config = self.config.clone();
        let paths = self.paths.clone();
        let tx = self.tx.clone();
        let cancel = self.begin_work("Preparing", "Checking message and model context");
        self.preflight = Some(Preflight {
            revision,
            snapshot: original,
            document,
        });
        self.context_blocked = false;
        tokio::task::spawn_blocking(move || {
            let result = (|| -> Result<String> {
                ensure!(
                    session.pending_tools.is_empty(),
                    "Inspect uncertain actions, then /acknowledge or /new before continuing"
                );
                let raw = work.materialize(&cancel)?;
                ensure!(!raw.trim().is_empty(), "Message is empty; draft preserved");
                let content = context::prepare_message(&session, &config, &raw)?;
                drop(raw);
                ensure!(!cancel.is_cancelled(), "Preparation canceled");
                let selection = session.selection.as_ref().context("Model missing")?;
                let profile = config
                    .providers
                    .get(&selection.profile)
                    .context("Provider missing")?;
                if profile.model(&selection.model)?.context_limit.is_some() {
                    let mut history: Vec<_> =
                        session.history()?.into_iter().map(|(_, m)| m).collect();
                    history.push(Message::user(content));
                    let (instructions, _) =
                        context::instructions(&session, &paths, &config, &history)?;
                    context::check_capacity(
                        profile,
                        &selection.model,
                        &history,
                        &instructions,
                        &tools::definitions_with_extensions(session.mode, &config),
                    )?;
                    ensure!(!cancel.is_cancelled(), "Preparation canceled");
                    let Some(Message::User { content }) = history.pop() else {
                        unreachable!()
                    };
                    Ok(content)
                } else {
                    Ok(content)
                }
            })()
            .map_err(|e| format!("{e:#}"));
            let _ = tx.blocking_send(UiEvent::PreflightFinished {
                session: Box::new(session),
                revision,
                result,
            });
        });
        Ok(())
    }
    pub(super) fn preflight_finished(
        &mut self,
        session: Session,
        revision: u64,
        result: std::result::Result<String, String>,
    ) -> Result<()> {
        let canceled = self
            .active
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled);
        self.finish_work();
        self.session = Some(session);
        let Some(pending) = self.preflight.take() else {
            return Ok(());
        };
        if pending.revision != revision || revision != self.editor.revision || canceled {
            self.status = "Preparation canceled; draft preserved".into();
            return Ok(());
        }
        match result {
            Ok(content) => {
                self.start(Action::PreparedMessage(content))?;
                self.transcript
                    .push_document(transcript::Role::User, pending.document);
                self.transcript.bottom();
                self.history.push(pending.snapshot);
                if self.history.len() > 50 {
                    self.history.remove(0);
                }
                self.history_index = None;
                self.editor.take();
            }
            Err(error) => {
                self.context_blocked = error.contains("Context estimate exceeds model capacity");
                self.status = "Message not sent; draft preserved".into();
                self.error(error);
            }
        }
        Ok(())
    }
    pub(super) fn open_paste(&mut self, id: usize) -> Result<()> {
        if self.preflight.is_some()
            && let Some(cancel) = &self.active
        {
            cancel.cancel();
        }
        let source = self
            .editor
            .pastes
            .iter()
            .find(|p| p.id == id)
            .context("Paste no longer exists")?
            .source
            .clone();
        self.paste_editor = Some(PasteEditor::new(id, source)?);
        self.focused_button = None;
        self.gesture = None;
        Ok(())
    }
    pub(super) fn save_paste(&mut self) -> Result<()> {
        let editing = self.paste_editor.as_ref().context("No paste editor")?;
        let id = editing.id;
        let text = editing.document.save()?;
        ensure!(
            self.editor.replace_paste(id, text),
            "Draft changed; the original paste is no longer present"
        );
        self.paste_editor = None;
        self.invalidate_preflight();
        Ok(())
    }
    pub(super) fn show_text_overlay(&mut self, title: &str, text: Text) -> Result<()> {
        let mut view = Transcript::new()?;
        view.push_text(text);
        view.start();
        self.overlay = Some(Overlay {
            title: title.into(),
            view,
        });
        self.gesture = None;
        Ok(())
    }
    pub(super) fn resume_paste_editor(&mut self) {
        if self.paste_editor.is_none() {
            self.paste_editor = self.suspended_paste_editor.take();
        }
    }
}
