use super::*;
use crate::effort::{self, ReasoningEffort};
use serde_json::json;

impl App {
    pub(super) fn effort_label(&self) -> String {
        self.selection
            .as_ref()
            .and_then(|s| self.config.providers.get(&s.profile)?.models.get(&s.model))
            .and_then(|m| m.reasoning_effort)
            .map(|e| e.to_string())
            .unwrap_or_else(|| "default".into())
    }
    fn check_effort_target(&self, selection: &Selection) -> Result<()> {
        ensure!(
            self.active.is_none(),
            "Wait for active work or cancel before changing effort"
        );
        ensure!(
            self.selection.as_ref() == Some(selection),
            "The selected model changed; reopen /effort"
        );
        Ok(())
    }
    pub(super) fn effort_command(&mut self, args: &[&str], height: u16) -> Result<()> {
        let selection = self
            .selection
            .clone()
            .context("No model selected; use /connect and /model first")?;
        match args {
            [] => self.open_effort(selection, height),
            ["levels"] => {
                self.edit_effort_levels(selection);
                Ok(())
            }
            ["levels", levels @ ..] => {
                self.configure_efforts(&selection, &levels.join(" "))?;
                self.open_effort(selection, height)
            }
            [value] => self.set_effort(
                &selection,
                if *value == "default" {
                    None
                } else {
                    Some(value.parse()?)
                },
            ),
            _ => bail!(
                "Usage: /effort [default|level] or /effort levels [comma/space-separated levels|unsupported]"
            ),
        }
    }
    pub(super) fn open_effort(&mut self, selection: Selection, height: u16) -> Result<()> {
        self.check_effort_target(&selection)?;
        let profile = self
            .config
            .providers
            .get(&selection.profile)
            .context("Provider missing")?;
        let model = profile.model(&selection.model)?;
        let support = model.supported_efforts(profile.kind, &selection.model);
        let note = match support {
            None => {
                "Effort support is unknown. Configure supported levels from your provider documentation."
            }
            Some([]) => {
                "This model is configured without an effort control. Use the provider default."
            }
            Some(_) => "Choose a supported reasoning effort for this model.",
        };
        let text = format!(
            "{}/{}\nCurrent effort: {}\n\n{note}\n\nHigher effort usually takes more time and tokens.\nProvider default leaves the choice to the provider.\nChanging effort does not start a task.",
            selection.profile,
            selection.model,
            self.effort_label()
        );
        let levels = support.unwrap_or_default().to_vec();
        if self.saved_draft.is_none() && !self.editor.text.is_empty() {
            self.saved_draft = Some(self.editor.take_snapshot());
        }
        self.page(text, Dialog::EffortPick { selection, levels }, height);
        if let Some(overlay) = &mut self.overlay {
            overlay.title = "Reasoning effort".into();
        }
        Ok(())
    }
    pub(super) fn edit_effort_levels(&mut self, selection: Selection) {
        if self.saved_draft.is_none() && !self.editor.text.is_empty() {
            self.saved_draft = Some(self.editor.take_snapshot());
        }
        self.show_overlay("Supported efforts", "Enter the effort levels accepted by this provider/model, separated by commas or spaces.\n\nExamples: low, medium, high\nOr: none, low, medium, high, xhigh\n\nUse unsupported for a model without an effort control.\nThe level none is different: it explicitly requests no reasoning.\nNo request is sent while editing these settings.".into());
        self.dialog = Dialog::EffortLevels { selection };
    }
    pub(super) fn set_effort(
        &mut self,
        selection: &Selection,
        effort: Option<ReasoningEffort>,
    ) -> Result<()> {
        self.check_effort_target(selection)?;
        let mut config = self.config.clone();
        let profile = config
            .providers
            .get_mut(&selection.profile)
            .context("Provider missing")?;
        profile
            .models
            .get_mut(&selection.model)
            .context("Model missing")?
            .reasoning_effort = effort;
        self.paths.save(&config)?;
        self.config = config;
        self.session
            .as_mut()
            .context("Session unavailable")?
            .record(
                "model.effort_changed",
                json!({"selection":selection,"reasoning_effort":effort}),
            )?;
        self.line(format!(
            "Effort for {}/{}: {}. Applies to future requests; no task started.",
            selection.profile,
            selection.model,
            effort
                .map(|e| e.to_string())
                .unwrap_or_else(|| "provider default".into())
        ));
        if let Some(draft) = self.saved_draft.take() {
            self.editor.restore(draft);
        }
        Ok(())
    }
    pub(super) fn configure_efforts(&mut self, selection: &Selection, text: &str) -> Result<()> {
        self.check_effort_target(selection)?;
        let levels = effort::parse_levels(text)?;
        let mut config = self.config.clone();
        config
            .providers
            .get_mut(&selection.profile)
            .context("Provider missing")?
            .models
            .get_mut(&selection.model)
            .context("Model missing")?
            .reasoning_efforts = Some(levels.clone());
        self.paths.save(&config)?;
        self.config = config;
        self.session
            .as_mut()
            .context("Session unavailable")?
            .record(
                "model.efforts_configured",
                json!({"selection":selection,"reasoning_efforts":levels}),
            )?;
        Ok(())
    }
}
