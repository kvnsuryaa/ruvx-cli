mod activity;
mod clipboard;
mod commands;
mod composer;
mod deletion;
mod editor;
mod extensions;
mod git;
mod paste_editor;
mod preferences;
mod screen;
mod transcript;
use crate::{
    agent::{self, Action},
    auth,
    config::{AuthConfig, Config, ModelConfig, Paths, Profile, ProviderKind, valid_id},
    context,
    effort::ReasoningEffort,
    events::UiEvent,
    model::{Mode, Selection},
    model_preference, provider,
    session::{Session, list_sessions},
    tools,
};
use anyhow::{Context, Result, bail, ensure};
use crossterm::{
    cursor::Show,
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags, poll, read,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use editor::Editor;
use ratatui::{Terminal, backend::CrosstermBackend};
use screen::{FrameMap, Gesture};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use transcript::{Selection as TextSelection, Transcript};
use uuid::Uuid;

type Term = Terminal<CrosstermBackend<io::Stdout>>;
static KEYBOARD_ENHANCEMENTS_ACTIVE: AtomicBool = AtomicBool::new(false);
struct Screen {
    terminal: Term,
}
impl Screen {
    fn new() -> Result<Self> {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));
        enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen) {
            restore_terminal();
            return Err(error.into());
        }
        KEYBOARD_ENHANCEMENTS_ACTIVE.store(true, Ordering::Relaxed);
        if let Err(error) = execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
            EnableBracketedPaste,
            EnableMouseCapture
        ) {
            restore_terminal();
            return Err(error.into());
        }
        match Terminal::new(CrosstermBackend::new(io::stdout())) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                restore_terminal();
                Err(error.into())
            }
        }
    }
    fn draw(&mut self, app: &mut App) -> Result<()> {
        self.terminal.draw(|frame| screen::render(frame, app))?;
        Ok(())
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        restore_terminal();
    }
}
fn restore_terminal() {
    let _ = disable_raw_mode();
    if KEYBOARD_ENHANCEMENTS_ACTIVE.swap(false, Ordering::Relaxed) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    );
}

pub fn sanitize(text: &str) -> String {
    let mut output = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if c == '\n' || c == '\t' || (!c.is_control() && c != '\u{7f}') {
            output.push(c);
        }
    }
    output
}
#[derive(Clone)]
struct Draft {
    id: String,
    kind: ProviderKind,
    base_url: Option<String>,
}
enum Dialog {
    None,
    Extension(extensions::Setup),
    Provider,
    ProfileName {
        kind: ProviderKind,
    },
    Endpoint {
        draft: Draft,
    },
    AuthMethod {
        draft: Draft,
    },
    ApiKey {
        draft: Draft,
    },
    Environment {
        draft: Draft,
    },
    ModelPick {
        choices: Vec<Selection>,
    },
    EffortPick {
        selection: Selection,
        levels: Vec<ReasoningEffort>,
    },
    EffortLevels {
        selection: Selection,
    },
    ManualModel {
        profile: String,
    },
    ModelTools {
        profile: String,
        model: String,
    },
    ModelLimit {
        profile: String,
        model: String,
    },
    DisconnectPick {
        ids: Vec<String>,
    },
    DisconnectConfirm {
        id: String,
    },
    Acknowledge,
    Approval {
        respond: oneshot::Sender<bool>,
    },
}
impl Dialog {
    fn label(&self) -> &str {
        match self {
            Self::Extension(setup) => setup.label(),
            Self::None => "Enter sends · Alt+Enter newline · /help · Ctrl+C cancel",
            Self::Provider => {
                "Provider: 1 OpenAI API · 2 Continue with ChatGPT · 3 Custom compatible"
            }
            Self::ProfileName { .. } => "Profile name (letters, digits, _ and -); Escape cancels",
            Self::Endpoint { .. } => "API base URL including prefix, e.g. https://host/v1",
            Self::AuthMethod { draft } => {
                if draft.kind == ProviderKind::Compatible {
                    "Auth: 1 saved API key · 2 environment variable · 3 none (local only)"
                } else {
                    "Auth: 1 saved API key · 2 environment variable"
                }
            }
            Self::ApiKey { .. } => "API key (hidden; never added to transcript)",
            Self::Environment { .. } => {
                "Environment variable name, e.g. OPENAI_API_KEY (not its value)"
            }
            Self::ModelPick { .. } => "Model number, or Escape then /model <profile> <model-id>",
            Self::EffortPick { .. } => {
                "Choose an effort · arrows/Tab + Enter · or type a level · Escape cancels"
            }
            Self::EffortLevels { .. } => {
                "Supported levels, e.g. low,medium,high; or unsupported · Escape cancels"
            }
            Self::ManualModel { .. } => "Exact model ID provided by this endpoint",
            Self::ModelTools { .. } => {
                "Does this model support tool calling AND streaming? Type yes or no"
            }
            Self::ModelLimit { .. } => {
                "Context token limit, or blank if unknown (never silently trimmed)"
            }
            Self::DisconnectPick { .. } => "Profile number to disconnect",
            Self::DisconnectConfirm { .. } => {
                "Type disconnect to remove this profile's local credentials; Escape cancels"
            }
            Self::Acknowledge => {
                "Inspect effects first. Type acknowledge to record uncertain outcomes without replay"
            }
            Self::Approval { .. } => "Type run to approve once, or deny / Escape to reject",
        }
    }
    fn masked(&self) -> bool {
        matches!(self, Self::ApiKey { .. })
    }
}

struct Overlay {
    title: String,
    view: Transcript,
}

struct App {
    paths: Paths,
    config: Config,
    session: Option<Session>,
    mode: Mode,
    selection: Option<Selection>,
    editor: Editor,
    command_menu: commands::Menu,
    dialog: Dialog,
    status: String,
    active: Option<CancellationToken>,
    activity: activity::Activity,
    tx: mpsc::Sender<UiEvent>,
    lines: VecDeque<String>,
    transcript: Transcript,
    overlay: Option<Overlay>,
    streaming: Option<usize>,
    frame: FrameMap,
    gesture: Option<Gesture>,
    pointer: (u16, u16),
    last_drag_scroll: std::time::Instant,
    focus: usize,
    focused_button: Option<screen::Button>,
    workspace: String,
    git_branch: Option<String>,
    history: Vec<editor::Snapshot>,
    history_index: Option<usize>,
    history_draft: editor::Snapshot,
    saved_draft: Option<editor::Snapshot>,
    preflight: Option<composer::Preflight>,
    usage_target: Option<usize>,
    paste_editor: Option<paste_editor::PasteEditor>,
    suspended_paste_editor: Option<paste_editor::PasteEditor>,
    context_blocked: bool,
    quit: bool,
}
impl Drop for App {
    fn drop(&mut self) {
        if let Some(cancel) = &self.active {
            cancel.cancel();
        }
    }
}

impl App {
    fn new(paths: Paths, config: Config, mut session: Session, tx: mpsc::Sender<UiEvent>) -> Self {
        let resumed = session.sequence > 1;
        let restored = model_preference::restore(&paths, &config, &mut session);
        let mode = session.mode;
        let transcript = Transcript::from_session(&session).unwrap_or_else(|error| {
            let mut view = Transcript::new().expect("create private transcript cache");
            view.push("History error", &error.to_string(), None);
            view
        });
        let workspace = session
            .root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let selection = session.selection.clone();
        let mut lines = VecDeque::new();
        lines.push_back(format!(
            "ruvx {} · {} · session {}",
            env!("CARGO_PKG_VERSION"),
            session.root.display(),
            session.id
        ));
        lines.push_back(
            "Use /connect to add a provider, /model to choose a model, /help for commands.".into(),
        );
        for warning in &session.warnings {
            lines.push_back(warning.clone());
        }
        if resumed {
            lines.push_back("Session restored. Context is available to the agent; no requests or actions have been replayed.".into());
        }
        match restored {
            Ok(Some(model)) => lines.push_back(format!(
                "Using last model {}/{}",
                model.profile, model.model
            )),
            Ok(None) => {}
            Err(error) => lines.push_back(format!(
                "Model preference: {error:#}. Use /model or /connect."
            )),
        }
        Self {
            paths,
            config,
            session: Some(session),
            mode,
            selection,
            editor: Editor::default(),
            command_menu: commands::Menu::default(),
            dialog: Dialog::None,
            status: "ready".into(),
            active: None,
            activity: activity::Activity::default(),
            tx,
            lines,
            transcript,
            overlay: None,
            streaming: None,
            frame: FrameMap::default(),
            gesture: None,
            pointer: (0, 0),
            last_drag_scroll: std::time::Instant::now(),
            focus: 0,
            focused_button: None,
            workspace,
            git_branch: None,
            history: vec![],
            history_index: None,
            history_draft: editor::Snapshot::default(),
            saved_draft: None,
            preflight: None,
            usage_target: None,
            paste_editor: None,
            suspended_paste_editor: None,
            context_blocked: false,
            quit: false,
        }
    }
    fn line(&mut self, text: impl Into<String>) {
        self.transcript.push("", &text.into(), None);
    }
    fn error(&mut self, error: impl std::fmt::Display) {
        self.line(format!("Error: {error}"));
    }
    fn page(&mut self, text: String, next: Dialog, _height: u16) {
        self.show_overlay("Review", text);
        self.dialog = next;
        self.focus = 0;
    }
    fn show_overlay(&mut self, title: &str, text: String) {
        match Transcript::new() {
            Ok(mut view) => {
                view.push("", &text, None);
                view.start();
                self.overlay = Some(Overlay {
                    title: title.into(),
                    view,
                });
                self.gesture = None;
            }
            Err(error) => self.error(error),
        }
    }
    fn view(&mut self) -> &mut Transcript {
        if let Some(overlay) = &mut self.overlay {
            &mut overlay.view
        } else {
            &mut self.transcript
        }
    }
    fn flush_lines(&mut self) {
        for text in self.lines.drain(..) {
            self.transcript.push("", &text, None);
        }
    }
    fn copy_selection(&mut self, clear: bool) -> Result<bool> {
        let Some(text) = self.view().selected()? else {
            return Ok(false);
        };
        if text.is_empty() {
            return Ok(false);
        }
        self.tx
            .try_send(UiEvent::ClipboardRequest(text))
            .context("Clipboard queue is busy; try again")?;
        if clear {
            self.view().selection = None;
        }
        Ok(true)
    }
    fn cancel(&mut self) {
        self.overlay = None;
        self.gesture = None;
        if let Some(token) = &self.active {
            token.cancel();
            self.status = "canceling…".into();
            if !matches!(self.dialog, Dialog::None) {
                self.editor.take();
            }
            self.dialog = Dialog::None;
            if let Some(draft) = self.saved_draft.take() {
                self.editor.restore(draft);
            }
        } else if !matches!(self.dialog, Dialog::None) {
            self.dialog = Dialog::None;
            self.editor.take();
            if let Some(draft) = self.saved_draft.take() {
                self.editor.restore(draft);
            }
        } else if !self.editor.text.is_empty() {
            self.editor.take();
        } else {
            self.quit = true;
        }
    }
    fn start(&mut self, action: Action) -> Result<()> {
        ensure!(
            self.active.is_none(),
            "Wait for active work or cancel with Ctrl+C"
        );
        let s = self.session.as_ref().context("Session not available")?;
        let selection = s
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
        match &action {
            Action::Message(text) => {
                context::prepare_message(s, &self.config, text)?;
            }
            Action::PreparedMessage(_) => {}
            Action::Continue => ensure!(
                s.stop_reason.as_deref() == Some("turn_limit"),
                "/continue is available after reaching the turn limit"
            ),
            Action::Compact => {
                context::compaction(s)?;
            }
        }
        let session = self.session.take().unwrap();
        let cancel = self.begin_work("AI Working", "Preparing request");
        tokio::spawn(agent::start(
            session,
            self.paths.clone(),
            self.config.clone(),
            action,
            self.tx.clone(),
            cancel,
        ));
        Ok(())
    }
    fn choose_model(&mut self, selection: Selection) -> Result<()> {
        let session = self.session.as_ref().context("Session unavailable")?;
        let profile = self
            .config
            .providers
            .get(&selection.profile)
            .context("Unknown profile")?;
        let model = profile.model(&selection.model)?;
        ensure!(
            model.supports_tools != Some(false) && model.supports_streaming,
            "This model is not configured for tools/streaming"
        );
        if session.selection.as_ref() != Some(&selection) {
            for (_, message) in session.history()? {
                if let crate::model::Message::Assistant { turn } = message {
                    ensure!(
                        !provider::has_opaque(&turn),
                        "Current history contains provider-specific reasoning; use /compact or /new before changing provider"
                    );
                }
            }
        }
        self.session.as_mut().unwrap().select(selection.clone())?;
        self.selection = Some(selection.clone());
        self.line(format!(
            "Selected {}/{}",
            selection.profile, selection.model
        ));
        if let Err(error) = model_preference::remember(&self.paths, &selection) {
            self.line(format!("Model selected for this session, but the global default could not be saved: {error:#}"));
        }
        Ok(())
    }
    fn connection(&mut self, id: String, profile: Profile) -> Result<()> {
        let mut config = self.config.clone();
        config.providers.insert(id.clone(), profile.clone());
        self.paths.save(&config)?;
        self.config = config;
        self.line(format!("Profile {id} saved. Authentication/inference remains subject to the provider's response."));
        if profile.kind == ProviderKind::Compatible {
            self.dialog = Dialog::ManualModel { profile: id };
        } else {
            self.discover(id, profile);
        }
        Ok(())
    }
    fn discover(&mut self, id: String, profile: Profile) {
        let paths = self.paths.clone();
        let config = self.config.clone();
        let tx = self.tx.clone();
        let cancel = self.begin_work("Loading models", "Contacting provider");
        tokio::spawn(async move {
            let result = provider::discover(&paths, &config, &id, &profile, &cancel)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(UiEvent::ModelsFinished { id, result }).await;
        });
    }
    fn chatgpt(&mut self, id: String, profile: Profile) {
        let paths = self.paths.clone();
        let config = self.config.clone();
        let tx = self.tx.clone();
        let cancel = self.begin_work("Signing in", "Waiting for ChatGPT sign-in");
        tokio::spawn(async move {
            let AuthConfig::Chatgpt { credential } = &profile.auth else {
                return;
            };
            let error = auth::login(&paths, &config, credential, &tx, &cancel)
                .await
                .err()
                .map(|e| format!("{e:#}"));
            let _ = tx
                .send(UiEvent::ConnectionFinished { id, profile, error })
                .await;
        });
    }
    fn profile_from(draft: &Draft, auth: AuthConfig) -> Profile {
        Profile {
            kind: draft.kind,
            label: draft.id.clone(),
            base_url: draft.base_url.clone(),
            auth,
            models: BTreeMap::new(),
        }
    }
    fn switch_session(&mut self, id: Option<&str>) -> Result<()> {
        let current = self.session.as_ref().context("Session unavailable")?;
        let root = current.root.clone();
        let old_id = current.id.clone();
        // Refresh global settings before releasing the existing workspace writer.
        let config = self.paths.load()?;
        if let Some(id) = id {
            ensure!(
                list_sessions(&self.paths, &root)?
                    .iter()
                    .any(|s| s.id == id),
                "Session is not in this workspace"
            );
        }
        drop(self.session.take());
        let next = (|| -> Result<_> {
            let mut session = id.map_or_else(
                || Session::create(&self.paths, &root),
                |id| Session::resume(&self.paths, id),
            )?;
            let restored = model_preference::restore(&self.paths, &config, &mut session);
            let transcript = Transcript::from_session(&session)?;
            Ok((session, transcript, restored))
        })();
        match next {
            Ok((session, transcript, restored)) => {
                self.config = config;
                self.context_blocked = false;
                self.history.clear();
                self.history_index = None;
                self.transcript = transcript;
                self.overlay = None;
                self.streaming = None;
                self.lines.clear();
                self.mode = session.mode;
                self.selection = session.selection.clone();
                self.line(format!(
                    "Session {} · {} · {}",
                    session.id,
                    session.root.display(),
                    session.mode
                ));
                for warning in &session.warnings {
                    self.line(warning.clone());
                }
                match restored {
                    Ok(Some(model)) => self.line(format!(
                        "Using last model {}/{}",
                        model.profile, model.model
                    )),
                    Ok(None) => {}
                    Err(error) => self.line(format!(
                        "Model preference: {error:#}. Use /model or /connect."
                    )),
                }
                self.session = Some(session);
                Ok(())
            }
            Err(error) => {
                self.session = Session::resume(&self.paths, &old_id).ok();
                Err(error)
            }
        }
    }
    fn command(&mut self, text: &str, height: u16) -> Result<()> {
        ensure!(
            self.active.is_none(),
            "Work is active. Ctrl+C cancels before changing modes, models, or sessions."
        );
        let mut words = text.split_whitespace();
        let command = words.next().unwrap_or("");
        let args: Vec<_> = words.collect();
        match command {
            "/help" => self.page(commands::help(), Dialog::None, height),
            "/effort" => self.effort_command(&args, height)?,
            "/quit" => self.quit = true,
            "/connect" => {
                if let Some(id) = args.first() {
                    let profile = self
                        .config
                        .providers
                        .get(*id)
                        .cloned()
                        .context("Unknown profile")?;
                    ensure!(
                        profile.kind == ProviderKind::Chatgpt,
                        "Use a new profile name to replace API-key connections; existing credentials are preserved"
                    );
                    self.chatgpt((*id).into(), profile);
                } else {
                    self.dialog = Dialog::Provider;
                }
            }
            "/disconnect" => {
                let ids: Vec<_> = self.config.providers.keys().cloned().collect();
                ensure!(!ids.is_empty(), "No profiles to disconnect");
                let text = ids
                    .iter()
                    .enumerate()
                    .map(|(i, id)| format!("{} {id}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.page(text, Dialog::DisconnectPick { ids }, height);
            }
            "/model" => {
                if args.len() == 2 {
                    let id = args[0];
                    let model = args[1];
                    let profile = self
                        .config
                        .providers
                        .get(id)
                        .context("Unknown provider profile")?;
                    if profile.models.contains_key(model) {
                        self.choose_model(Selection {
                            profile: id.into(),
                            model: model.into(),
                        })?;
                    } else if profile.kind == ProviderKind::Compatible {
                        self.dialog = Dialog::ModelTools {
                            profile: id.into(),
                            model: model.into(),
                        };
                    } else {
                        let mut config = self.config.clone();
                        config.providers.get_mut(id).unwrap().models.insert(
                            model.into(),
                            ModelConfig {
                                supports_tools: Some(true),
                                ..Default::default()
                            },
                        );
                        self.paths.save(&config)?;
                        self.config = config;
                        self.choose_model(Selection {
                            profile: id.into(),
                            model: model.into(),
                        })?;
                    }
                } else {
                    ensure!(args.is_empty(), "Usage: /model [profile model-id]");
                    let mut choices = vec![];
                    let mut lines = vec![];
                    for (id, profile) in &self.config.providers {
                        for (model, details) in &profile.models {
                            if details.supports_tools == Some(false) || !details.supports_streaming
                            {
                                continue;
                            }
                            choices.push(Selection {
                                profile: id.clone(),
                                model: model.clone(),
                            });
                            lines.push(format!("{} {id}/{model} {}", choices.len(), details.name));
                            if choices.len() >= 100 {
                                break;
                            }
                        }
                        if choices.len() >= 100 {
                            break;
                        }
                    }
                    ensure!(
                        !choices.is_empty(),
                        "No selectable models. Use /connect or /model <profile> <model-id>"
                    );
                    lines.push("Showing at most 100 models. Select any exact ID using /model <profile> <model-id>.".into());
                    self.page(lines.join("\n"), Dialog::ModelPick { choices }, height);
                }
            }
            "/mode" => {
                ensure!(args.len() == 1, "Usage: /mode plan|build");
                let mode = match args[0] {
                    "plan" => Mode::Plan,
                    "build" => Mode::Build,
                    _ => bail!("Mode must be plan or build"),
                };
                self.session
                    .as_mut()
                    .context("Session unavailable")?
                    .set_mode(mode)?;
                self.mode = mode;
                self.line(format!("Mode: {mode}. No task was started."));
            }
            "/new" => self.switch_session(None)?,
            "/sessions" => {
                let root = &self.session.as_ref().context("Session unavailable")?.root;
                let sessions = list_sessions(&self.paths, root)?;
                self.page(
                    sessions
                        .iter()
                        .map(|s| format!("{}  {}", s.id, s.modified))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    Dialog::None,
                    height,
                );
            }
            "/resume" => {
                ensure!(args.len() == 1, "Usage: /resume <session-id>");
                self.switch_session(Some(args[0]))?;
            }
            "/skills" => self.skills_command(&args, height)?,
            "/mcp" => self.mcp_command(&args, height)?,
            "/diff" => {
                let path = text.strip_prefix("/diff").unwrap().trim();
                let output = tools::diff(
                    self.session.as_ref().context("Session unavailable")?,
                    if path.is_empty() { None } else { Some(path) },
                )?;
                self.page(output, Dialog::None, height);
            }
            "/continue" => self.start(Action::Continue)?,
            "/compact" => self.start(Action::Compact)?,
            "/acknowledge" => {
                ensure!(
                    !self
                        .session
                        .as_ref()
                        .context("Session unavailable")?
                        .pending_tools
                        .is_empty(),
                    "No uncertain actions to acknowledge"
                );
                self.dialog = Dialog::Acknowledge;
            }
            _ => bail!("Unknown command. Use /help; prefix // to send a literal slash."),
        }
        Ok(())
    }
    fn submit(&mut self, height: u16) -> Result<()> {
        if matches!(self.dialog, Dialog::Approval { .. })
            && self.editor.text.trim() == "run"
            && self.overlay.as_ref().is_some_and(|o| !o.view.bottom_seen)
        {
            self.status = "Read the entire operation review: scroll down before approving".into();
            self.activity.approval_hint =
                Some("Scroll down to read the entire review before approving");
            self.editor.take();
            return Ok(());
        }
        if matches!(self.dialog, Dialog::Approval { .. })
            && !matches!(self.editor.text.trim(), "run" | "deny")
        {
            self.editor.take();
            self.status = "Choose Run once or Deny, or type run / deny".into();
            self.activity.approval_hint = Some("Choose Run once or Deny");
            return Ok(());
        }
        self.overlay = None;
        self.gesture = None;
        self.focus = 0;
        let snapshot = self.editor.snapshot();
        let text = self.editor.take();
        let value = text.trim();
        let dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        match dialog {
            Dialog::None => {
                if value.is_empty() {
                    return Ok(());
                }
                if self.active.is_some() {
                    self.editor.restore(snapshot);
                    bail!("Work is active. Your draft is preserved; Ctrl+C cancels.");
                }
                if snapshot.pastes.is_empty() && value.starts_with('/') && !value.starts_with("//")
                {
                    self.line(format!("> {value}"));
                    self.command(value, height)?;
                } else {
                    self.editor.restore(snapshot.clone());
                    self.prepare_submission(snapshot)?;
                }
            }
            Dialog::Extension(setup) => {
                let retry = setup.clone();
                if let Err(error) = self.submit_extension(setup, value, height) {
                    self.dialog = Dialog::Extension(retry);
                    return Err(error);
                }
            }
            Dialog::Provider => {
                let kind = match value {
                    "1" => ProviderKind::Openai,
                    "2" => ProviderKind::Chatgpt,
                    "3" => ProviderKind::Compatible,
                    _ => {
                        self.dialog = Dialog::Provider;
                        bail!("Choose 1, 2, or 3");
                    }
                };
                self.dialog = Dialog::ProfileName { kind };
            }
            Dialog::ProfileName { kind } => {
                if !valid_id(value) || self.config.providers.contains_key(value) {
                    self.dialog = Dialog::ProfileName { kind };
                    bail!("Use a new profile name with letters, digits, _ or -");
                }
                let draft = Draft {
                    id: value.into(),
                    kind,
                    base_url: None,
                };
                if kind == ProviderKind::Compatible {
                    self.dialog = Dialog::Endpoint { draft };
                } else if kind == ProviderKind::Chatgpt {
                    let credential = Uuid::new_v4().to_string();
                    let profile = Self::profile_from(&draft, AuthConfig::Chatgpt { credential });
                    self.chatgpt(draft.id, profile);
                } else {
                    self.dialog = Dialog::AuthMethod { draft };
                }
            }
            Dialog::Endpoint { mut draft } => {
                if let Err(error) = crate::config::validate_url(value) {
                    self.dialog = Dialog::Endpoint { draft };
                    return Err(error);
                }
                draft.base_url = Some(value.trim_end_matches('/').into());
                self.dialog = Dialog::AuthMethod { draft };
            }
            Dialog::AuthMethod { draft } => match value {
                "1" => self.dialog = Dialog::ApiKey { draft },
                "2" => self.dialog = Dialog::Environment { draft },
                "3" => {
                    let profile = Self::profile_from(&draft, AuthConfig::None);
                    if let Err(e) = self.connection(draft.id.clone(), profile) {
                        self.dialog = Dialog::AuthMethod { draft };
                        return Err(e);
                    }
                }
                _ => {
                    self.dialog = Dialog::AuthMethod { draft };
                    bail!("Choose 1, 2, or 3");
                }
            },
            Dialog::ApiKey { draft } => {
                let credential = Uuid::new_v4().to_string();
                if let Err(error) = auth::save_key(&self.paths, &credential, value) {
                    self.dialog = Dialog::ApiKey { draft };
                    return Err(error);
                }
                let profile = Self::profile_from(&draft, AuthConfig::Saved { credential });
                self.connection(draft.id, profile)?;
            }
            Dialog::Environment { draft } => {
                let profile = Self::profile_from(
                    &draft,
                    AuthConfig::Environment {
                        variable: value.into(),
                    },
                );
                if let Err(error) = self.connection(draft.id.clone(), profile) {
                    self.dialog = Dialog::Environment { draft };
                    return Err(error);
                }
            }
            Dialog::ManualModel { profile } => {
                if value.is_empty() {
                    self.dialog = Dialog::ManualModel { profile };
                    bail!("Model ID cannot be empty");
                }
                self.dialog = Dialog::ModelTools {
                    profile,
                    model: value.into(),
                };
            }
            Dialog::ModelTools { profile, model } => match value {
                "yes" => self.dialog = Dialog::ModelLimit { profile, model },
                "no" => {
                    self.line("Agent requires tool calling and streaming; model was not selected.");
                }
                _ => {
                    self.dialog = Dialog::ModelTools { profile, model };
                    bail!("Type yes or no explicitly");
                }
            },
            Dialog::ModelLimit { profile, model } => {
                let limit = if value.is_empty() {
                    None
                } else {
                    match value.parse::<usize>() {
                        Ok(n) if n >= 128 => Some(n),
                        _ => {
                            self.dialog = Dialog::ModelLimit { profile, model };
                            bail!("Use a context limit of at least 128, or leave unknown");
                        }
                    }
                };
                let reserve = limit.map(|n| (n / 4).min(4096)).unwrap_or(4096);
                let details = ModelConfig {
                    supports_tools: Some(true),
                    context_limit: limit,
                    output_reserve_tokens: reserve,
                    ..Default::default()
                };
                let mut config = self.config.clone();
                config
                    .providers
                    .get_mut(&profile)
                    .context("Profile disappeared")?
                    .models
                    .insert(model.clone(), details);
                self.paths.save(&config)?;
                self.config = config;
                self.choose_model(Selection { profile, model })?;
            }
            Dialog::ModelPick { choices } => {
                let selection = value
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|n| choices.get(n))
                    .cloned();
                if let Some(selection) = selection {
                    self.choose_model(selection)?;
                } else {
                    self.dialog = Dialog::ModelPick { choices };
                    bail!("Choose a displayed model number");
                }
            }
            Dialog::EffortPick { selection, levels } => {
                let mut values = vec!["default".to_owned()];
                values.extend(levels.iter().map(ToString::to_string));
                values.push("configure".into());
                let choice = value
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|n| values.get(n))
                    .map(String::as_str)
                    .unwrap_or(value);
                if choice == "configure" {
                    self.edit_effort_levels(selection);
                } else {
                    let result = if choice == "default" {
                        self.set_effort(&selection, None)
                    } else {
                        choice
                            .parse()
                            .and_then(|level| self.set_effort(&selection, Some(level)))
                    };
                    if let Err(error) = result {
                        self.dialog = Dialog::EffortPick { selection, levels };
                        return Err(error);
                    }
                }
            }
            Dialog::EffortLevels { selection } => {
                if let Err(error) = self.configure_efforts(&selection, value) {
                    self.dialog = Dialog::EffortLevels { selection };
                    return Err(error);
                }
                self.open_effort(selection, height)?;
            }
            Dialog::DisconnectPick { ids } => {
                let id = value
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|n| ids.get(n))
                    .cloned();
                if let Some(id) = id {
                    self.line(format!("Disconnect {id}? Session history will remain."));
                    self.dialog = Dialog::DisconnectConfirm { id };
                } else {
                    self.dialog = Dialog::DisconnectPick { ids };
                    bail!("Choose a displayed profile number");
                }
            }
            Dialog::DisconnectConfirm { id } => {
                ensure!(value == "disconnect", "Disconnect canceled");
                let profile = self
                    .config
                    .providers
                    .get(&id)
                    .context("Profile missing")?
                    .clone();
                auth::disconnect(&self.paths, &profile)?;
                let mut config = self.config.clone();
                config.providers.remove(&id);
                self.paths.save(&config)?;
                self.config = config;
                if let Err(error) = model_preference::clear_profile(&self.paths, &id) {
                    self.line(format!(
                        "Could not clear the global model preference: {error:#}"
                    ));
                }
                self.line(format!(
                    "Disconnected {id}. To revoke ChatGPT access remotely, use ChatGPT Settings."
                ));
            }
            Dialog::Acknowledge => {
                ensure!(value == "acknowledge", "Acknowledgement canceled");
                agent::acknowledge_uncertain(self.session.as_mut().context("Session missing")?)?;
                self.line(
                    "Uncertain actions acknowledged without asserting success or replaying them.",
                );
            }
            Dialog::Approval { respond } => {
                self.resume_paste_editor();
                if value == "run" {
                    let _ = respond.send(true);
                } else if value == "deny" {
                    let _ = respond.send(false);
                } else {
                    self.dialog = Dialog::Approval { respond };
                    bail!("Type run to approve once or deny to reject");
                }
                if let Some(draft) = self.saved_draft.take() {
                    self.editor.restore(draft);
                }
            }
        }
        if matches!(self.dialog, Dialog::None)
            && self.active.is_none()
            && let Some(draft) = self.saved_draft.take()
        {
            self.editor.restore(draft);
        }
        Ok(())
    }
    fn key(&mut self, key: KeyEvent, height: u16) -> Result<()> {
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
        if self.paste_editor.is_some() {
            if key.code == KeyCode::Esc {
                self.paste_editor = None;
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                return self.save_paste();
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('d') {
                if let Some(editor) = self.paste_editor.take() {
                    self.editor.remove_paste(editor.id);
                }
                self.invalidate_preflight();
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                if self.active.is_some() {
                    self.cancel();
                } else {
                    self.paste_editor = None;
                }
                return Ok(());
            }
            self.paste_editor.as_mut().unwrap().key(key)?;
            return Ok(());
        }
        let plain_modifiers = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
        let switch_mode = key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers == KeyModifiers::SHIFT);
        if plain_modifiers && switch_mode && matches!(self.dialog, Dialog::None) {
            if key.kind == KeyEventKind::Repeat {
                return Ok(());
            }
            self.command(
                if self.mode == Mode::Plan {
                    "/mode build"
                } else {
                    "/mode plan"
                },
                height,
            )?;
            self.focused_button = None;
            return Ok(());
        }
        if key.code == KeyCode::F(2) && matches!(self.dialog, Dialog::None) {
            if let Some(paste) = self
                .editor
                .pastes
                .iter()
                .find(|p| p.start <= self.editor.cursor && self.editor.cursor <= p.end)
                .or_else(|| self.editor.pastes.last())
            {
                self.open_paste(paste.id)?;
            }
            return Ok(());
        }
        if self.command_key(key, height)? {
            return Ok(());
        }
        if self.focused_button.is_some()
            || (key.code == KeyCode::F(6) && plain_modifiers)
            || (key.code == KeyCode::BackTab && self.dialog.choices().is_empty())
        {
            let buttons: Vec<_> = self.frame.buttons.iter().map(|(_, b)| b.clone()).collect();
            if !buttons.is_empty() {
                let index = self
                    .focused_button
                    .as_ref()
                    .and_then(|b| buttons.iter().position(|v| v == b));
                match key.code {
                    KeyCode::F(6)
                    | KeyCode::BackTab
                    | KeyCode::Left
                    | KeyCode::Up
                    | KeyCode::Tab
                    | KeyCode::Right
                    | KeyCode::Down => {
                        let backward =
                            matches!(key.code, KeyCode::BackTab | KeyCode::Left | KeyCode::Up)
                                || (key.code == KeyCode::F(6)
                                    && key.modifiers.contains(KeyModifiers::SHIFT));
                        let next = if backward {
                            index
                                .map(|i| (i + buttons.len() - 1) % buttons.len())
                                .unwrap_or(buttons.len() - 1)
                        } else {
                            index.map(|i| (i + 1) % buttons.len()).unwrap_or(0)
                        };
                        self.focused_button = Some(buttons[next].clone());
                        return Ok(());
                    }
                    KeyCode::Enter => {
                        if let Some(button) = self.focused_button.take() {
                            self.activate(button, height)?;
                        }
                        return Ok(());
                    }
                    KeyCode::Esc => {
                        self.focused_button = None;
                        return Ok(());
                    }
                    _ => {
                        self.focused_button = None;
                    }
                }
            } else {
                self.focused_button = None;
            }
        }
        if let Some(kind) = deletion::key(key) {
            self.view().selection = None;
            self.editor.erase_backward(kind);
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    if !self.copy_selection(true)? {
                        self.cancel();
                    }
                }
                KeyCode::Char('d')
                    if self.active.is_none()
                        && self.editor.text.is_empty()
                        && matches!(self.dialog, Dialog::None) =>
                {
                    self.quit = true
                }
                _ => {}
            }
            return Ok(());
        }
        if key.code == KeyCode::PageUp || key.code == KeyCode::PageDown {
            let n = self.frame.transcript.height.saturating_sub(2).max(1) as isize;
            self.scroll_view(if key.code == KeyCode::PageUp { -n } else { n });
            return Ok(());
        }
        if key.code == KeyCode::End && key.modifiers.contains(KeyModifiers::ALT) {
            self.view().bottom();
            return Ok(());
        }
        if key.code == KeyCode::Esc && self.view().selection.take().is_some() {
            return Ok(());
        }
        if matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete | KeyCode::Enter
        ) {
            self.view().selection = None;
        }
        let choices = self.dialog.choices();
        if !choices.is_empty()
            && self.editor.text.is_empty()
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            match key.code {
                KeyCode::Tab | KeyCode::Down | KeyCode::Right => {
                    self.focus = (self.focus + 1) % choices.len();
                    return Ok(());
                }
                KeyCode::BackTab | KeyCode::Up | KeyCode::Left => {
                    self.focus = (self.focus + choices.len() - 1) % choices.len();
                    return Ok(());
                }
                KeyCode::Enter => {
                    self.editor
                        .set(choices[self.focus.min(choices.len() - 1)].1.clone());
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => self.editor.insert("\n"),
            KeyCode::Enter => self.submit(height)?,
            KeyCode::Esc => {
                self.overlay = None;
                self.gesture = None;
                self.resume_paste_editor();
                if !matches!(self.dialog, Dialog::None) {
                    self.dialog = Dialog::None;
                    self.editor.take();
                    if let Some(draft) = self.saved_draft.take() {
                        self.editor.restore(draft);
                    }
                }
            }
            KeyCode::Backspace => self.editor.backspace(),
            KeyCode::Delete => self.editor.delete(),
            KeyCode::Left => self.editor.left(),
            KeyCode::Right => self.editor.right(),
            KeyCode::Home => self.editor.home(),
            KeyCode::End => self.editor.end(),
            KeyCode::Up | KeyCode::Down => {
                if !self.editor.text.contains('\n')
                    && matches!(self.dialog, Dialog::None)
                    && !self.history.is_empty()
                {
                    let index = if key.code == KeyCode::Up {
                        match self.history_index {
                            Some(i) => i.saturating_sub(1),
                            None => {
                                self.history_draft = self.editor.snapshot();
                                self.history.len() - 1
                            }
                        }
                    } else {
                        self.history_index
                            .map(|i| i + 1)
                            .unwrap_or(self.history.len())
                    };
                    if index >= self.history.len() {
                        self.history_index = None;
                        self.editor.restore(self.history_draft.clone());
                    } else {
                        self.history_index = Some(index);
                        self.editor.restore(self.history[index].clone());
                    }
                } else {
                    self.editor.vertical(key.code == KeyCode::Down);
                }
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::META)
                    && !c.is_control() =>
            {
                self.editor.insert(&c.to_string())
            }
            KeyCode::Tab => self.editor.insert("    "),
            _ => {}
        }
        Ok(())
    }
    fn event(&mut self, event: UiEvent, height: u16) -> Result<()> {
        match event {
            UiEvent::PreflightFinished {
                session,
                revision,
                result,
            } => self.preflight_finished(*session, revision, result)?,
            UiEvent::Usage(value) => self.transcript.set_usage(self.usage_target, value),
            UiEvent::Text(text) => {
                let id = *self
                    .streaming
                    .get_or_insert_with(|| self.transcript.begin("Agent"));
                self.transcript.append(id, &text);
            }
            UiEvent::TurnEnded => {
                self.usage_target = self.streaming.take();
                if let Some(id) = self.usage_target {
                    self.transcript.seal(id);
                }
            }
            UiEvent::Tool {
                id,
                name,
                status,
                detail,
            } => {
                self.transcript.push(
                    format!("{name} → {status}"),
                    "",
                    Some(format!(
                        "Tool: {name}\nCall ID: {id}\nStatus: {status}\n\n{detail}"
                    )),
                );
            }
            UiEvent::ClipboardRequest(text) => {
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let result = clipboard::copy(text).await.map_err(|e| e.to_string());
                    let _ = tx.send(UiEvent::ClipboardFinished(result)).await;
                });
            }
            UiEvent::ClipboardFinished(result) => match result {
                Ok(None) => self.status = "Copied to clipboard".into(),
                Ok(Some(sequence)) => {
                    io::stdout().write_all(sequence.as_bytes())?;
                    io::stdout().flush()?;
                    self.status = "Copy sent to terminal (OSC52); clipboard access depends on terminal settings".into();
                }
                Err(error) => self.status = format!("Copy failed: {error}"),
            },
            UiEvent::Line(text) => self.line(text),
            UiEvent::Status(status) => {
                self.status = sanitize(&status);
            }
            UiEvent::RunProgress {
                turns,
                total_tokens,
            } => {
                self.activity.turns = turns;
                self.activity.total_tokens = total_tokens;
            }
            UiEvent::Approval { review, respond } => {
                self.suspended_paste_editor = self.paste_editor.take();
                self.saved_draft = Some(self.editor.take_snapshot());
                self.status = "waiting for approval".into();
                self.activity.approval_hint = None;
                let title = if review.starts_with("MCP approval") {
                    "MCP approval"
                } else {
                    "Shell approval"
                };
                self.page(review, Dialog::Approval { respond }, height);
                if let Some(overlay) = &mut self.overlay {
                    overlay.title = title.into();
                }
            }
            UiEvent::RunFinished { session, error } => {
                self.mode = session.mode;
                self.selection = session.selection.clone();
                self.session = Some(*session);
                self.finish_work();
                self.dialog = Dialog::None;
                self.resume_paste_editor();
                self.overlay = None;
                if let Some(draft) = self.saved_draft.take() {
                    self.editor.restore(draft);
                }
                if let Some(error) = error {
                    self.error(error);
                }
            }
            UiEvent::ConnectionFinished { id, profile, error } => {
                self.finish_work();
                if let Some(error) = error {
                    self.error(error);
                    if !self.config.providers.contains_key(&id) {
                        let mut config = self.config.clone();
                        config.providers.insert(id.clone(), profile);
                        self.paths.save(&config)?;
                        self.config = config;
                    }
                    self.line(format!("Registration is retained without activating inference. Use /connect {id} to retry this account."));
                } else {
                    self.line("ChatGPT identity and plan permission verified.");
                    self.connection(id, profile)?;
                }
            }
            UiEvent::ModelsFinished { id, result } => {
                self.finish_work();
                match result{Ok(models)=>{let count=models.len();let mut config=self.config.clone();if let Some(profile)=config.providers.get_mut(&id){profile.models.extend(models);}self.paths.save(&config)?;self.config=config;self.line(format!("Found {count} models for {id}. Use /model to choose; no inference request was sent."));},Err(error)=>self.error(format!("Model discovery: {error}. Configure exact model using /model {id} <model-id>."))}
            }
        }
        if matches!(self.dialog, Dialog::None)
            && self.active.is_none()
            && let Some(draft) = self.saved_draft.take()
        {
            self.editor.restore(draft);
        }
        Ok(())
    }
}

pub async fn run(paths: Paths, config: Config, session: Session) -> Result<()> {
    let mut git = git::Monitor::new(session.root.clone());
    let (tx, mut rx) = mpsc::channel(64);
    let mut app = App::new(paths, config, session, tx);
    let mut screen = Screen::new()?;
    let mut tick = tokio::time::interval(Duration::from_millis(
        1000 / u64::from(app.config.ui.max_fps),
    ));
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut dirty = true;
    let mut closing = false;
    app.flush_lines();
    screen.draw(&mut app)?;
    while !app.quit {
        let height = screen.terminal.size()?.height;
        tokio::select! {
            Ok(())=git.receiver.changed()=>{
                app.git_branch = git.receiver.borrow_and_update().clone();
                dirty=true;
            },
            event=rx.recv()=>if let Some(event)=event { if let Err(error)=app.event(event,height) {app.error(error);}dirty=true; },
            _=tick.tick()=>{
                // Keep one terminal reader and writer; networking and tools run on other tasks.
                for _ in 0..64 {
                    if !poll(Duration::ZERO)? { break; }
                    match read()? {
                        Event::Key(key)=>{if let Err(error)=app.key(key,height){app.error(error);}app.invalidate_preflight();dirty=true;},
                        Event::Paste(text)=>{app.view().selection=None;if let Err(error)=app.paste(&text){app.error(error);}dirty=true;},
                        Event::Mouse(event) if event.kind != MouseEventKind::Moved=>{if let Err(error)=app.mouse(event,height){app.status=format!("Mouse: {error}");}dirty=true;},
                        Event::Resize(_,_)=>{app.gesture=None;dirty=true;},
                        _=>{}
                    }
                }
                if app.drag_scroll(){dirty=true;}
                if app.activity_tick(std::time::Instant::now()) {dirty=true;}
                if dirty{app.flush_lines();screen.draw(&mut app)?;dirty=false;}
            },
            _=term.recv()=>{closing=true;app.cancel();},
            _=interrupt.recv()=>{app.cancel();dirty=true;},
        }
        if closing && app.active.is_none() {
            app.quit = true;
        }
    }
    if let Some(token) = &app.active {
        token.cancel();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn fixture() -> (tempfile::TempDir, App, mpsc::Receiver<UiEvent>) {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(&temp.path().join("state"));
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        let session = Session::create(&paths, &root).unwrap();
        let (tx, rx) = mpsc::channel(8);
        let mut app = App::new(paths, Config::default(), session, tx);
        app.flush_lines();
        (temp, app, rx)
    }
    pub(super) fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal =
            Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| screen::render(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }
    #[test]
    fn sanitizer_removes_terminal_commands() {
        assert_eq!(sanitize("a\x1b[2Jb\x1b]0;owned\x07c\n"), "abc\n");
    }
    #[test]
    fn git_branch_does_not_overlap_status_controls_or_change_the_draft() {
        let (_temp, mut app, mut rx) = effort_fixture();
        app.git_branch = Some("feature/日本語/very-long-branch-name".into());
        app.editor.set("keep this draft".into());
        for (width, height) in [(100, 40), (48, 16), (24, 10), (8, 5)] {
            let view = render(&mut app, width, height);
            if width >= 24 {
                assert!(view.contains("git:"));
            }
            let branch_width = (width - 2) / 3;
            for (rect, button) in &app.frame.buttons {
                if matches!(button, screen::Button::Command(command) if command == "/effort") {
                    assert!(rect.right() <= width - branch_width - 3);
                }
            }
        }
        app.git_branch = None;
        assert!(!render(&mut app, 100, 40).contains("git:"));
        assert_eq!(app.editor.text, "keep this draft");
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn word_and_line_shortcuts_work_in_composer_and_masked_dialogs_without_submitting() {
        let (_temp, mut app, mut rx) = fixture();
        for key in [
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
        ] {
            app.editor.set("hello e\u{301} tail".into());
            app.editor.cursor = "hello e\u{301}".len();
            app.key(key, 40).unwrap();
            assert_eq!(app.editor.text, "hello  tail");
        }
        app.editor.set("first\nsecond tail".into());
        app.editor.cursor = "first\nsecond".len();
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL), 40)
            .unwrap();
        assert_eq!(app.editor.text, "first\n tail");
        app.editor.set("keep this word".into());
        let original = app.editor.text.clone();
        let revision = app.editor.revision;
        for modifier in [KeyModifiers::SUPER, KeyModifiers::META] {
            app.key(KeyEvent::new(KeyCode::Backspace, modifier), 40)
                .unwrap();
            assert_eq!(app.editor.text, original);
            assert_eq!(app.editor.revision, revision);
        }
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER), 40)
            .unwrap();
        assert_eq!(app.editor.text, original);
        app.dialog = Dialog::ApiKey {
            draft: Draft {
                id: "key".into(),
                kind: ProviderKind::Openai,
                base_url: None,
            },
        };
        app.editor.set("secret words".into());
        app.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT), 40)
            .unwrap();
        assert_eq!(app.editor.text, "secret ");
        assert!(!render(&mut app, 100, 40).contains("secret"));
        assert!(app.active.is_none());
        assert!(rx.try_recv().is_err());
        assert_eq!(app.mode, Mode::Plan);
    }
    #[test]
    fn tool_calls_show_one_compact_result_and_keep_ids_in_detail() {
        let (_temp, mut app, _rx) = fixture();
        app.event(UiEvent::Status("Running read_file…".into()), 40)
            .unwrap();
        app.event(
            UiEvent::Tool {
                id: "call_first".into(),
                name: "read_file".into(),
                status: "ok".into(),
                detail: "Arguments:\n{\"path\":\"file.rs\"}\n\nResult:\ncontents".into(),
            },
            40,
        )
        .unwrap();
        app.event(
            UiEvent::Tool {
                id: "call_second".into(),
                name: "read_file".into(),
                status: "error".into(),
                detail: "Arguments:\n{\"path\":\"missing.rs\"}\n\nResult:\nfile not found".into(),
            },
            40,
        )
        .unwrap();
        let view = render(&mut app, 100, 40);
        assert_eq!(view.matches("read_file → ok").count(), 1);
        assert_eq!(view.matches("read_file → error").count(), 1);
        assert!(!view.contains("call_first"));
        assert!(!view.contains("call_second"));
        let (_, button) = app
            .frame
            .buttons
            .iter()
            .rev()
            .find(|(_, button)| matches!(button, screen::Button::Detail(_)))
            .unwrap()
            .clone();
        app.activate(button, 40).unwrap();
        let detail = render(&mut app, 100, 40);
        assert!(detail.contains("Call ID: call_second"));
        assert!(detail.contains("missing.rs"));
        assert!(detail.contains("file not found"));
    }
    #[test]
    fn shift_tab_toggles_mode_preserves_the_draft_and_persists_without_inference() {
        let (_temp, mut app, mut rx) = fixture();
        let draft = "unsent paste\n".repeat(20);
        app.paste(&draft).unwrap();
        let text = app.editor.text.clone();
        let revision = app.editor.revision;
        let cursor = app.editor.cursor;
        let sequence = app.session.as_ref().unwrap().sequence;
        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        app.key(key, 40).unwrap();
        assert_eq!(app.mode, Mode::Build);
        assert!(render(&mut app, 100, 40).contains("/connect · Build · F2/click edits paste"));
        assert_eq!(app.session.as_ref().unwrap().mode, Mode::Build);
        assert_eq!(app.session.as_ref().unwrap().sequence, sequence + 1);
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            app.key(KeyEvent { kind, ..key }, 40).unwrap();
            assert_eq!(app.mode, Mode::Build);
        }
        // Some terminals report Shift+Tab as Tab with the Shift modifier.
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT), 40)
            .unwrap();
        assert_eq!(app.mode, Mode::Plan);
        app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE), 40)
            .unwrap();
        assert_eq!(app.mode, Mode::Build);
        assert_eq!(app.editor.text, text);
        assert_eq!(app.editor.revision, revision);
        assert_eq!(app.editor.cursor, cursor);
        assert_eq!(app.editor.pastes[0].source.contents().unwrap(), draft);
        assert!(app.active.is_none());
        assert!(rx.try_recv().is_err());
        let paths = app.paths.clone();
        let id = app.session.as_ref().unwrap().id.clone();
        drop(app);
        assert_eq!(Session::resume(&paths, &id).unwrap().mode, Mode::Build);
    }
    #[test]
    fn shift_tab_respects_active_work_dialogs_and_paste_editor() {
        let (_temp, mut app, _rx) = fixture();
        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        let cancel = CancellationToken::new();
        app.active = Some(cancel.clone());
        app.editor.set("/m".into());
        let sequence = app.session.as_ref().unwrap().sequence;
        assert!(app.key(key, 40).is_err());
        assert_eq!(app.mode, Mode::Plan);
        assert_eq!(app.editor.text, "/m");
        assert_eq!(app.session.as_ref().unwrap().sequence, sequence);
        assert!(!cancel.is_cancelled());
        app.active = None;
        render(&mut app, 100, 40);
        assert_eq!(app.mode, Mode::Plan);
        app.dialog = Dialog::Provider;
        app.editor.take();
        app.key(key, 40).unwrap();
        assert_eq!(app.focus, app.dialog.choices().len() - 1);
        assert_eq!(app.mode, Mode::Plan);
        let (tx, mut rx) = oneshot::channel();
        app.dialog = Dialog::Approval { respond: tx };
        app.key(key, 40).unwrap();
        assert_eq!(app.mode, Mode::Plan);
        assert!(rx.try_recv().is_err());
        assert!(matches!(app.dialog, Dialog::Approval { .. }));
        app.dialog = Dialog::None;
        app.paste(&"paste line\n".repeat(20)).unwrap();
        app.open_paste(app.editor.pastes[0].id).unwrap();
        app.key(key, 40).unwrap();
        assert!(app.paste_editor.is_some());
        assert_eq!(app.mode, Mode::Plan);
    }
    fn menu_key(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE), 40)
            .unwrap();
    }
    #[test]
    fn slash_menu_filters_dismisses_and_reopens_only_after_editing() {
        let (_temp, mut app, _rx) = fixture();
        app.editor.set("/".into());
        let items = app.command_matches().unwrap();
        assert_eq!(items.len(), 16);
        assert_eq!(items[0].name, "/help");
        assert!(!items.iter().any(|c| c.name == "/usage"));
        let help = commands::help();
        assert!(items.iter().all(|c| help.contains(c.description)));
        app.editor.insert("m");
        assert_eq!(
            app.command_matches()
                .unwrap()
                .iter()
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            ["/model", "/mcp", "/mode"]
        );
        menu_key(&mut app, KeyCode::Esc);
        assert_eq!(app.editor.text, "/m");
        menu_key(&mut app, KeyCode::Left);
        menu_key(&mut app, KeyCode::Right);
        assert!(app.command_matches().is_none());
        app.editor.insert("o");
        assert_eq!(app.command_matches().unwrap().len(), 2);
        app.editor.set("/unknown".into());
        assert!(render(&mut app, 100, 40).contains("No matching commands"));
        menu_key(&mut app, KeyCode::Enter);
        menu_key(&mut app, KeyCode::Tab);
        assert_eq!(app.editor.text, "/unknown");
        assert!(app.overlay.is_none());
        for input in ["//", "/mode build", "/help\n", "ordinary text", ""] {
            app.editor.set(input.into());
            assert!(app.command_matches().is_none(), "{input:?}");
        }
    }
    #[test]
    fn slash_menu_navigation_completion_and_required_arguments_never_submit_a_prompt() {
        let (_temp, mut app, mut rx) = effort_fixture();
        app.editor.set("/".into());
        render(&mut app, 100, 40);
        menu_key(&mut app, KeyCode::F(6));
        assert!(app.focused_button.is_some());
        menu_key(&mut app, KeyCode::Tab);
        assert_eq!(app.editor.text, "/");
        menu_key(&mut app, KeyCode::Esc);
        assert!(app.focused_button.is_none());
        assert!(app.command_matches().is_some());
        menu_key(&mut app, KeyCode::Tab);
        assert_eq!(app.editor.text, "/help ");
        app.editor.set("/m".into());
        menu_key(&mut app, KeyCode::Up);
        assert_eq!(app.command_menu.selected, 2);
        menu_key(&mut app, KeyCode::Down);
        assert_eq!(app.command_menu.selected, 0);
        menu_key(&mut app, KeyCode::Tab);
        assert_eq!(app.editor.text, "/model ");
        assert!(app.command_matches().is_none());
        assert!(matches!(app.dialog, Dialog::None));
        app.editor.set("/mo".into());
        menu_key(&mut app, KeyCode::Down);
        menu_key(&mut app, KeyCode::Enter);
        assert_eq!(app.editor.text, "/mode ");
        assert_eq!(app.mode, Mode::Plan);
        app.editor.set("/r".into());
        menu_key(&mut app, KeyCode::Enter);
        assert_eq!(app.editor.text, "/resume ");
        assert!(app.active.is_none());
        assert!(rx.try_recv().is_err());
        app.editor.set("/model".into());
        menu_key(&mut app, KeyCode::Enter);
        assert!(matches!(app.dialog, Dialog::ModelPick { .. }));
        assert!(app.editor.text.is_empty());
        assert!(app.saved_draft.is_none());
        menu_key(&mut app, KeyCode::Esc);
        assert!(app.editor.text.is_empty());
    }
    #[test]
    fn slash_menu_mouse_and_small_layout_keep_controls_separate() {
        let (_temp, mut app, _rx) = fixture();
        app.editor.set("/".into());
        for (width, height) in [(100, 40), (48, 16), (24, 10), (8, 5)] {
            render(&mut app, width, height);
            let menu = app.frame.command_menu;
            assert!(menu.bottom() <= app.frame.composer.y);
            assert!(menu.height <= 7);
            assert!(
                app.frame
                    .buttons
                    .iter()
                    .filter(|(_, b)| matches!(b, screen::Button::CommandChoice(_)))
                    .count()
                    <= 6
            );
        }
        render(&mut app, 100, 40);
        let (rect, _) = app
            .frame
            .buttons
            .iter()
            .find(|(_, b)| matches!(b, screen::Button::CommandChoice(name) if name == "/model"))
            .unwrap()
            .clone();
        let event = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(event(MouseEventKind::ScrollDown, rect.x, rect.y), 40)
            .unwrap();
        assert_eq!(app.command_menu.selected, 1);
        app.mouse(event(MouseEventKind::ScrollUp, rect.x, rect.y), 40)
            .unwrap();
        assert_eq!(app.command_menu.selected, 0);
        app.mouse(event(MouseEventKind::ScrollDown, 0, 0), 40)
            .unwrap();
        assert_eq!(app.command_menu.selected, 0);
        // Moving even one cell during a click must not execute a command.
        app.mouse(
            event(MouseEventKind::Down(MouseButton::Left), rect.x, rect.y),
            40,
        )
        .unwrap();
        app.mouse(
            event(MouseEventKind::Drag(MouseButton::Left), rect.x + 1, rect.y),
            40,
        )
        .unwrap();
        app.mouse(
            event(MouseEventKind::Up(MouseButton::Left), rect.x + 1, rect.y),
            40,
        )
        .unwrap();
        assert_eq!(app.editor.text, "/");
        assert!(matches!(app.dialog, Dialog::None));
        render(&mut app, 100, 40);
        let (rect, _) = app
            .frame
            .buttons
            .iter()
            .find(|(_, b)| matches!(b, screen::Button::CommandChoice(name) if name == "/help"))
            .unwrap()
            .clone();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.mouse(event(kind, rect.x, rect.y), 40).unwrap();
        }
        assert!(app.overlay.is_some());
        assert!(app.editor.text.is_empty());
    }
    #[test]
    fn slash_menu_excludes_dialogs_paste_nodes_and_full_paste_editor() {
        let (_temp, mut app, _rx) = fixture();
        app.editor.set("/".into());
        app.dialog = Dialog::Provider;
        assert!(app.command_matches().is_none());
        app.dialog = Dialog::ApiKey {
            draft: Draft {
                id: "key".into(),
                kind: ProviderKind::Openai,
                base_url: None,
            },
        };
        assert!(app.command_matches().is_none());
        assert!(!render(&mut app, 100, 40).contains("Commands ·"));
        let (tx, _rx) = oneshot::channel();
        app.dialog = Dialog::Approval { respond: tx };
        assert!(app.command_matches().is_none());
        app.dialog = Dialog::None;
        app.editor.take();
        app.paste(&"/quit\n".repeat(9)).unwrap();
        assert!(!app.editor.pastes.is_empty());
        assert!(app.command_matches().is_none());
        app.open_paste(app.editor.pastes[0].id).unwrap();
        app.editor.set("/".into());
        assert!(app.command_matches().is_none());
        assert!(!render(&mut app, 100, 40).contains("Commands ·"));
    }
    #[test]
    fn slash_menu_does_not_queue_commands_during_active_work() {
        let (_temp, mut app, mut rx) = fixture();
        app.editor.set("/help".into());
        let token = CancellationToken::new();
        app.active = Some(token.clone());
        assert!(render(&mut app, 100, 40).contains("Commands unavailable"));
        assert!(
            !app.frame
                .buttons
                .iter()
                .any(|(_, b)| matches!(b, screen::Button::CommandChoice(_)))
        );
        menu_key(&mut app, KeyCode::Enter);
        app.choose_command("/help", false, 40).unwrap();
        assert_eq!(app.editor.text, "/help");
        assert!(!token.is_cancelled());
        assert!(app.overlay.is_none());
        app.active = None;
        render(&mut app, 100, 40);
        assert!(app.overlay.is_none());
        assert!(rx.try_recv().is_err());
        menu_key(&mut app, KeyCode::Enter);
        assert!(app.overlay.is_some());
        app.overlay = None;
        app.editor.set("/help".into());
        app.active = Some(token);
        menu_key(&mut app, KeyCode::Tab);
        assert_eq!(app.editor.text, "/help ");
        assert!(app.overlay.is_none());
    }
    #[test]
    fn incomplete_account_registration_can_be_retried() {
        let (_temp, mut app, _rx) = fixture();
        let profile = Profile {
            kind: ProviderKind::Chatgpt,
            label: "personal".into(),
            base_url: None,
            auth: AuthConfig::Chatgpt {
                credential: "retained-reference".into(),
            },
            models: BTreeMap::new(),
        };
        app.event(
            UiEvent::ConnectionFinished {
                id: "personal".into(),
                profile,
                error: Some("Code exchange failed".into()),
            },
            40,
        )
        .unwrap();
        assert!(app.paths.load().unwrap().providers.contains_key("personal"));
        assert!(app.selection.is_none());
    }
    #[test]
    fn fullscreen_render_and_small_terminal_preserve_draft() {
        let (_temp, mut app, _rx) = fixture();
        app.editor.set("draft 中🙂".into());
        let view = render(&mut app, 100, 40);
        assert!(view.contains("ruvx"));
        assert!(view.contains("draft 中"));
        assert!(view.contains("🙂"));
        render(&mut app, 8, 5);
        render(&mut app, 1, 1);
        render(&mut app, 74, 28);
        assert_eq!(app.editor.text, "draft 中🙂");
    }
    #[tokio::test]
    async fn shell_defaults_to_deny_and_drag_never_approves() {
        let (_temp, mut app, _rx) = fixture();
        let (tx, mut rx) = oneshot::channel();
        app.event(
            UiEvent::Approval {
                review: "Shell approval\nprintf safe\ncwd: /work\ntimeout: 30".into(),
                respond: tx,
            },
            40,
        )
        .unwrap();
        render(&mut app, 100, 40);
        let (rect, _) = app
            .frame
            .buttons
            .iter()
            .find(|(_, b)| matches!(b,screen::Button::Choice(v) if v=="run"))
            .unwrap()
            .clone();
        for (kind, column) in [
            (MouseEventKind::Down(MouseButton::Left), rect.x),
            (MouseEventKind::Drag(MouseButton::Left), rect.x + 2),
            (MouseEventKind::Up(MouseButton::Left), rect.x + 2),
        ] {
            app.mouse(
                MouseEvent {
                    kind,
                    column,
                    row: rect.y,
                    modifiers: KeyModifiers::NONE,
                },
                40,
            )
            .unwrap();
        }
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 40)
            .unwrap();
        assert!(!rx.await.unwrap());
    }
    #[test]
    fn long_shell_review_must_be_read_before_run() {
        let (_temp, mut app, _rx) = fixture();
        let (tx, mut rx) = oneshot::channel();
        app.event(
            UiEvent::Approval {
                review: (0..200).map(|i| format!("line {i}\n")).collect(),
                respond: tx,
            },
            40,
        )
        .unwrap();
        render(&mut app, 100, 40);
        app.editor.set("run".into());
        app.submit(40).unwrap();
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(app.overlay.is_some());
        app.view().bottom();
        render(&mut app, 100, 40);
        app.editor.set("run".into());
        app.submit(40).unwrap();
        assert!(rx.try_recv().unwrap());
    }
    #[test]
    fn menus_support_keyboard_and_click_and_secret_is_masked() {
        let (_temp, mut app, _rx) = fixture();
        app.command("/connect", 40).unwrap();
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), 40)
            .unwrap();
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), 40)
            .unwrap();
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 40)
            .unwrap();
        assert!(matches!(
            app.dialog,
            Dialog::ProfileName {
                kind: ProviderKind::Compatible
            }
        ));
        app.dialog = Dialog::Provider;
        render(&mut app, 100, 40);
        let (rect, _) = app
            .frame
            .buttons
            .iter()
            .find(|(_, b)| matches!(b,screen::Button::Choice(v) if v=="1"))
            .unwrap()
            .clone();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.mouse(
                MouseEvent {
                    kind,
                    column: rect.x,
                    row: rect.y,
                    modifiers: KeyModifiers::NONE,
                },
                40,
            )
            .unwrap();
        }
        assert!(matches!(
            app.dialog,
            Dialog::ProfileName {
                kind: ProviderKind::Openai
            }
        ));
        app.dialog = Dialog::ApiKey {
            draft: Draft {
                id: "x".into(),
                kind: ProviderKind::Openai,
                base_url: None,
            },
        };
        app.editor.set("secret-do-not-show".into());
        let output = render(&mut app, 100, 40);
        assert!(!output.contains("secret-do-not-show"));
        assert!(output.contains("*****"));
    }
    #[test]
    fn drag_copy_and_ctrl_c_prioritize_selection_before_cancellation() {
        let (_temp, mut app, mut rx) = fixture();
        let id = app.transcript.push("Agent", "hello 中🙂 world", None);
        render(&mut app, 100, 40);
        let row = app
            .frame
            .rows
            .iter()
            .position(|r| r.block == id && r.range.is_some())
            .unwrap() as u16
            + app.frame.transcript.y;
        let x = app.frame.transcript.x;
        for (kind, column) in [
            (MouseEventKind::Down(MouseButton::Left), x),
            (MouseEventKind::Drag(MouseButton::Left), x + 4),
            (MouseEventKind::Up(MouseButton::Left), x + 4),
        ] {
            app.mouse(
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                40,
            )
            .unwrap();
        }
        assert!(
            matches!(rx.try_recv().unwrap(),UiEvent::ClipboardRequest(text) if text == "hello")
        );
        let cancel = CancellationToken::new();
        app.active = Some(cancel.clone());
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), 40)
            .unwrap();
        assert!(!cancel.is_cancelled());
        assert!(app.view().selection.is_none());
        assert!(
            matches!(rx.try_recv().unwrap(),UiEvent::ClipboardRequest(text) if text == "hello")
        );
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), 40)
            .unwrap();
        assert!(cancel.is_cancelled());
    }
    #[test]
    fn keyboard_opens_tool_details_without_losing_draft() {
        let (_temp, mut app, _rx) = fixture();
        app.transcript
            .push("read_file · result", "", Some("    code\nnext".into()));
        app.editor.set("unsent draft".into());
        render(&mut app, 100, 40);
        app.key(KeyEvent::new(KeyCode::F(6), KeyModifiers::SHIFT), 40)
            .unwrap();
        assert!(matches!(
            app.focused_button,
            Some(screen::Button::Detail(_))
        ));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 40)
            .unwrap();
        assert!(app.overlay.is_some());
        assert_eq!(app.editor.text, "unsent draft");
        assert!(render(&mut app, 100, 40).contains("    code"));
    }
    #[test]
    fn edge_drag_selects_across_multiple_viewports() {
        let (_temp, mut app, mut rx) = fixture();
        app.show_overlay("Code", (0..100).map(|i| format!("line {i}\n")).collect());
        render(&mut app, 80, 15);
        let rect = app.frame.transcript;
        app.mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x,
                row: rect.y,
                modifiers: KeyModifiers::NONE,
            },
            15,
        )
        .unwrap();
        app.mouse(
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: rect.x + 6,
                row: rect.bottom() - 1,
                modifiers: KeyModifiers::NONE,
            },
            15,
        )
        .unwrap();
        for _ in 0..30 {
            app.last_drag_scroll = std::time::Instant::now() - Duration::from_millis(100);
            assert!(app.drag_scroll());
        }
        app.mouse(
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: rect.x + 6,
                row: rect.bottom() - 1,
                modifiers: KeyModifiers::NONE,
            },
            15,
        )
        .unwrap();
        let UiEvent::ClipboardRequest(text) = rx.try_recv().unwrap() else {
            panic!("Expected clipboard request")
        };
        assert!(text.starts_with("line 0\n"));
        assert!(text.contains("line 20\n"));
        assert!(!text.contains("Code"));
    }
    fn effort_fixture() -> (tempfile::TempDir, App, mpsc::Receiver<UiEvent>) {
        let (temp, mut app, rx) = fixture();
        app.config.providers.insert(
            "local".into(),
            Profile {
                kind: ProviderKind::Compatible,
                label: "Local".into(),
                base_url: Some("http://127.0.0.1:1/v1".into()),
                auth: AuthConfig::None,
                models: BTreeMap::from([
                    (
                        "a".into(),
                        ModelConfig {
                            supports_tools: Some(true),
                            reasoning_efforts: Some(vec![
                                ReasoningEffort::Low,
                                ReasoningEffort::High,
                            ]),
                            ..Default::default()
                        },
                    ),
                    (
                        "b".into(),
                        ModelConfig {
                            supports_tools: Some(true),
                            reasoning_efforts: Some(vec![
                                ReasoningEffort::None,
                                ReasoningEffort::Medium,
                            ]),
                            ..Default::default()
                        },
                    ),
                ]),
            },
        );
        app.paths.save(&app.config).unwrap();
        app.choose_model(Selection {
            profile: "local".into(),
            model: "a".into(),
        })
        .unwrap();
        (temp, app, rx)
    }
    #[test]
    fn effort_is_saved_per_model_and_restored_without_starting_work() {
        let (_temp, mut app, mut rx) = effort_fixture();
        app.command("/effort high", 40).unwrap();
        assert_eq!(app.effort_label(), "high");
        app.command("/model local b", 40).unwrap();
        assert_eq!(app.effort_label(), "default");
        app.command("/effort none", 40).unwrap();
        assert_eq!(app.effort_label(), "none");
        app.command("/model local a", 40).unwrap();
        assert_eq!(app.effort_label(), "high");
        let paths = app.paths.clone();
        let id = app.session.as_ref().unwrap().id.clone();
        drop(app);
        let session = Session::resume(&paths, &id).unwrap();
        let (tx, _) = mpsc::channel(8);
        let mut app = App::new(paths.clone(), paths.load().unwrap(), session, tx);
        assert_eq!(app.effort_label(), "high");
        assert!(app.active.is_none());
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
        assert_eq!(
            app.config.providers["local"].models["b"].reasoning_effort,
            Some(ReasoningEffort::None)
        );
        app.command("/effort default", 40).unwrap();
        assert_eq!(
            app.paths.load().unwrap().providers["local"].models["a"].reasoning_effort,
            None
        );
    }
    #[test]
    fn last_model_survives_restart_and_a_different_workspace_without_inference() {
        let (temp, mut app, _rx) = effort_fixture();
        app.command("/effort high", 40).unwrap();
        let paths = app.paths.clone();
        let root = app.session.as_ref().unwrap().root.clone();
        let expected = app.selection.clone();
        let preference = std::fs::read(paths.data.join("last-model.json")).unwrap();
        drop(app);
        for root in [root, temp.path().join("other-workspace")] {
            std::fs::create_dir_all(&root).unwrap();
            let session = Session::create(&paths, &root).unwrap();
            let id = session.id.clone();
            let (tx, mut rx) = mpsc::channel(8);
            let app = App::new(paths.clone(), paths.load().unwrap(), session, tx);
            assert_eq!(app.selection, expected);
            assert_eq!(app.session.as_ref().unwrap().selection, expected);
            assert_eq!(app.effort_label(), "high");
            assert_eq!(app.mode, Mode::Plan);
            assert!(app.active.is_none());
            assert!(rx.try_recv().is_err());
            assert!(
                app.lines
                    .iter()
                    .any(|line| line.contains("Using last model local/a"))
            );
            assert!(
                !app.lines
                    .iter()
                    .any(|line| line.contains("Session restored."))
            );
            drop(app);
            let session = Session::resume(&paths, &id).unwrap();
            assert_eq!(session.selection, expected);
        }
        assert_eq!(
            std::fs::read(paths.data.join("last-model.json")).unwrap(),
            preference
        );
    }
    #[test]
    fn resume_preserves_its_model_and_opaque_context_without_changing_the_global_default() {
        let (_temp, mut app, _rx) = effort_fixture();
        let paths = app.paths.clone();
        let expected = app.selection.clone();
        let session = app.session.as_mut().unwrap();
        session
            .response(&crate::model::Turn {
                text: "old answer".into(),
                opaque: vec![serde_json::json!({"type":"reasoning","id":"old-context"})],
                ..Default::default()
            })
            .unwrap();
        let id = session.id.clone();
        let global = Selection {
            profile: "local".into(),
            model: "b".into(),
        };
        model_preference::remember(&paths, &global).unwrap();
        drop(app);
        let (tx, mut rx) = mpsc::channel(8);
        let session = Session::resume(&paths, &id).unwrap();
        let sequence = session.sequence;
        let mut app = App::new(paths.clone(), paths.load().unwrap(), session, tx);
        assert_eq!(app.selection, expected);
        assert_eq!(app.session.as_ref().unwrap().sequence, sequence);
        assert_eq!(
            model_preference::load(&paths).unwrap(),
            Some(global.clone())
        );
        assert!(app.choose_model(global.clone()).is_err());
        assert_eq!(app.selection, expected);
        assert_eq!(model_preference::load(&paths).unwrap(), Some(global));
        assert!(rx.try_recv().is_err());
        // An unavailable saved model is reported, never silently replaced.
        drop(app);
        let mut config = paths.load().unwrap();
        config
            .providers
            .get_mut("local")
            .unwrap()
            .models
            .remove("a");
        let session = Session::resume(&paths, &id).unwrap();
        let (tx, _) = mpsc::channel(8);
        let app = App::new(paths, config, session, tx);
        assert_eq!(app.selection, expected);
        assert!(
            app.lines
                .iter()
                .any(|line| line.contains("Saved session model unavailable"))
        );
    }
    #[test]
    fn new_session_reads_latest_global_model_and_effort_and_preserves_the_draft() {
        let (_temp, mut app, mut rx) = effort_fixture();
        app.command("/effort high", 40).unwrap();
        app.command("/mode build", 40).unwrap();
        let old_id = app.session.as_ref().unwrap().id.clone();
        let draft = "unsent draft\n".repeat(20);
        app.paste(&draft).unwrap();
        // Simulate settings changed in a second window after this one started.
        let mut config = app.paths.load().unwrap();
        config
            .providers
            .get_mut("local")
            .unwrap()
            .models
            .get_mut("b")
            .unwrap()
            .reasoning_effort = Some(ReasoningEffort::Medium);
        app.paths.save(&config).unwrap();
        let global = Selection {
            profile: "local".into(),
            model: "b".into(),
        };
        model_preference::remember(&app.paths, &global).unwrap();
        assert_eq!(app.selection.as_ref().unwrap().model, "a");
        assert_eq!(app.effort_label(), "high");
        app.command("/new", 40).unwrap();
        assert_eq!(app.selection, Some(global.clone()));
        assert_eq!(app.effort_label(), "medium");
        assert_eq!(app.mode, Mode::Plan);
        assert_ne!(app.session.as_ref().unwrap().id, old_id);
        assert_eq!(app.editor.pastes[0].source.contents().unwrap(), draft);
        app.command(&format!("/resume {old_id}"), 40).unwrap();
        assert_eq!(app.selection.as_ref().unwrap().model, "a");
        assert_eq!(app.effort_label(), "high");
        assert_eq!(app.mode, Mode::Build);
        assert_eq!(model_preference::load(&app.paths).unwrap(), Some(global));
        assert!(app.active.is_none());
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn invalid_config_on_new_keeps_the_current_session_and_draft() {
        let (_temp, mut app, _rx) = effort_fixture();
        let old_id = app.session.as_ref().unwrap().id.clone();
        let selection = app.selection.clone();
        app.editor.set("keep this draft".into());
        std::fs::write(app.paths.config_file(), "invalid [ TOML").unwrap();
        assert!(app.command("/new", 40).is_err());
        assert_eq!(app.session.as_ref().unwrap().id, old_id);
        assert_eq!(app.selection, selection);
        assert_eq!(app.editor.text, "keep this draft");
        assert!(Session::create(&app.paths, &app.session.as_ref().unwrap().root).is_err());
    }
    #[test]
    fn invalid_global_preferences_leave_new_sessions_unselected_with_a_notice() {
        let (_temp, app, _rx) = effort_fixture();
        let paths = app.paths.clone();
        let root = app.session.as_ref().unwrap().root.clone();
        drop(app);
        for case in [
            "corrupt",
            "missing-profile",
            "missing-model",
            "no-tools",
            "no-stream",
        ] {
            let mut config = paths.load().unwrap();
            let mut selection = Selection {
                profile: "local".into(),
                model: "a".into(),
            };
            match case {
                "missing-profile" => selection.profile = "removed".into(),
                "missing-model" => selection.model = "removed".into(),
                "no-tools" => {
                    config
                        .providers
                        .get_mut("local")
                        .unwrap()
                        .models
                        .get_mut("a")
                        .unwrap()
                        .supports_tools = Some(false)
                }
                "no-stream" => {
                    config
                        .providers
                        .get_mut("local")
                        .unwrap()
                        .models
                        .get_mut("a")
                        .unwrap()
                        .supports_streaming = false
                }
                _ => {}
            }
            model_preference::remember(&paths, &selection).unwrap();
            if case == "corrupt" {
                std::fs::write(paths.data.join("last-model.json"), "{broken").unwrap();
            }
            let session = Session::create(&paths, &root).unwrap();
            let (tx, mut rx) = mpsc::channel(8);
            let app = App::new(paths.clone(), config, session, tx);
            assert!(app.selection.is_none(), "{case}");
            assert!(app.session.as_ref().unwrap().selection.is_none());
            assert!(
                app.lines
                    .iter()
                    .any(|line| line.contains("Model preference:")
                        && line.contains("/model or /connect")),
                "{case}"
            );
            assert!(app.active.is_none());
            assert!(rx.try_recv().is_err());
        }
    }
    #[test]
    fn failed_preference_save_keeps_a_successful_local_selection() {
        let (_temp, mut app, _rx) = effort_fixture();
        let file = app.paths.data.join("last-model.json");
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        app.command("/model local b", 40).unwrap();
        assert_eq!(app.selection.as_ref().unwrap().model, "b");
        assert_eq!(app.session.as_ref().unwrap().selection, app.selection);
        assert!(render(&mut app, 100, 40).contains("global default could not be saved"));
        let sequence = app.session.as_ref().unwrap().sequence;
        assert!(app.command("/model missing nope", 40).is_err());
        assert_eq!(app.session.as_ref().unwrap().sequence, sequence);
        assert_eq!(app.selection.as_ref().unwrap().model, "b");
    }
    #[test]
    fn disconnect_removes_the_matching_default_without_removing_session_history() {
        let (_temp, mut app, _rx) = effort_fixture();
        let old_id = app.session.as_ref().unwrap().id.clone();
        app.dialog = Dialog::DisconnectConfirm { id: "local".into() };
        app.editor.set("disconnect".into());
        app.submit(40).unwrap();
        assert!(model_preference::load(&app.paths).unwrap().is_none());
        assert!(!app.paths.load().unwrap().providers.contains_key("local"));
        app.command("/new", 40).unwrap();
        assert!(app.selection.is_none());
        assert!(
            list_sessions(&app.paths, &app.session.as_ref().unwrap().root)
                .unwrap()
                .iter()
                .any(|s| s.id == old_id)
        );
    }
    #[test]
    fn effort_picker_click_keyboard_and_cancel_preserve_draft() {
        let (_temp, mut app, _rx) = effort_fixture();
        app.editor.set("unsent instructions".into());
        app.command("/effort", 40).unwrap();
        let choices = app.dialog.choices();
        assert!(!choices.iter().any(|(_, v)| v == "xhigh"));
        render(&mut app, 100, 40);
        let (rect, _) = app
            .frame
            .buttons
            .iter()
            .find(|(_, b)| matches!(b,screen::Button::Choice(v) if v=="high"))
            .unwrap()
            .clone();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.mouse(
                MouseEvent {
                    kind,
                    column: rect.x,
                    row: rect.y,
                    modifiers: KeyModifiers::NONE,
                },
                40,
            )
            .unwrap();
        }
        assert_eq!(app.effort_label(), "high");
        assert_eq!(app.editor.text, "unsent instructions");
        assert!(render(&mut app, 100, 40).contains("[Effort: high]"));
        app.command("/effort", 40).unwrap();
        app.cancel();
        assert_eq!(app.editor.text, "unsent instructions");
        app.editor.take();
        app.command("/effort", 40).unwrap();
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), 40)
            .unwrap();
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 40)
            .unwrap();
        assert_eq!(app.effort_label(), "low");
    }
    #[test]
    fn unknown_efforts_require_configuration_and_active_runs_block_changes() {
        let (_temp, mut app, _rx) = effort_fixture();
        app.config
            .providers
            .get_mut("local")
            .unwrap()
            .models
            .get_mut("a")
            .unwrap()
            .reasoning_efforts = None;
        assert!(app.command("/effort high", 40).is_err());
        assert_eq!(app.effort_label(), "default");
        app.command("/effort", 40).unwrap();
        assert_eq!(app.dialog.choices().len(), 2);
        app.cancel();
        app.command("/effort levels low high", 40).unwrap();
        app.editor.set("high".into());
        app.submit(40).unwrap();
        assert_eq!(app.effort_label(), "high");
        assert!(app.command("/effort max", 40).is_err());
        assert_eq!(app.effort_label(), "high");
        let cancel = CancellationToken::new();
        app.active = Some(cancel.clone());
        assert!(app.command("/effort low", 40).is_err());
        assert_eq!(app.effort_label(), "high");
        assert!(!cancel.is_cancelled());
    }
    #[tokio::test]
    async fn context_failure_and_changed_revision_keep_the_draft_unsent() {
        let (_temp, mut app, mut rx) = effort_fixture();
        let model = app
            .config
            .providers
            .get_mut("local")
            .unwrap()
            .models
            .get_mut("a")
            .unwrap();
        model.context_limit = Some(1024);
        model.output_reserve_tokens = 128;
        let text = "large context ".repeat(15000);
        app.paste(&text).unwrap();
        let revision = app.editor.revision;
        app.submit(40).unwrap();
        assert!(app.active.is_some());
        let event = rx.recv().await.unwrap();
        assert!(matches!(
            &event,
            UiEvent::PreflightFinished { result: Err(_), .. }
        ));
        app.event(event, 40).unwrap();
        assert_eq!(app.editor.pastes.len(), 1);
        assert_eq!(
            app.editor
                .snapshot()
                .document()
                .unwrap()
                .materialize(&CancellationToken::new())
                .unwrap(),
            text
        );
        assert!(app.active.is_none());
        assert!(app.history.is_empty());
        assert!(app.editor.revision > revision);
        app.config
            .providers
            .get_mut("local")
            .unwrap()
            .models
            .get_mut("a")
            .unwrap()
            .context_limit = None;
        app.submit(40).unwrap();
        app.editor.insert(" changed");
        app.invalidate_preflight();
        app.event(rx.recv().await.unwrap(), 40).unwrap();
        assert!(app.active.is_none());
        assert!(app.editor.text.ends_with(" changed"));
        let journal =
            std::fs::read_to_string(app.session.as_ref().unwrap().dir.join("events.jsonl"))
                .unwrap();
        assert!(!journal.contains("message.user"));
    }
    #[test]
    fn paste_editor_save_cancel_delete_and_dialog_masking() {
        let (_temp, mut app, _rx) = fixture();
        let text = "line 中🙂\n".repeat(20);
        app.paste(&text).unwrap();
        let id = app.editor.pastes[0].id;
        assert!(render(&mut app, 100, 40).contains("[Paste 1"));
        app.open_paste(id).unwrap();
        app.paste_editor.as_mut().unwrap().insert("new ").unwrap();
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 40)
            .unwrap();
        assert_eq!(app.editor.pastes[0].source.contents().unwrap(), text);
        app.open_paste(id).unwrap();
        app.paste_editor.as_mut().unwrap().insert("new ").unwrap();
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL), 40)
            .unwrap();
        assert_eq!(
            app.editor.pastes[0].source.contents().unwrap(),
            format!("new {text}")
        );
        app.open_paste(app.editor.pastes[0].id).unwrap();
        app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL), 40)
            .unwrap();
        assert!(app.editor.pastes.is_empty());
        app.dialog = Dialog::ApiKey {
            draft: Draft {
                id: "key".into(),
                kind: ProviderKind::Openai,
                base_url: None,
            },
        };
        app.paste(&"secret-fixture".repeat(1000)).unwrap();
        assert!(app.editor.pastes.is_empty());
        assert!(!render(&mut app, 100, 40).contains("secret-fixture"));
    }
    #[test]
    fn usage_is_quiet_and_roles_remain_distinct_without_color() {
        let (_temp, mut app, _rx) = fixture();
        app.transcript
            .push_role(transcript::Role::User, "USER", "question", None);
        app.event(UiEvent::Text("answer".into()), 40).unwrap();
        app.event(UiEvent::TurnEnded, 40).unwrap();
        app.event(UiEvent::Usage(serde_json::json!({"input_tokens":413,"output_tokens":14,"total_tokens":427,"attribution":{"internal":"details"}})),40).unwrap();
        let screen = render(&mut app, 100, 40);
        assert!(screen.contains("USER"));
        assert!(!screen.contains("┌"));
        assert!(!screen.contains("└"));
        assert!(!screen.contains("│"));
        assert!(screen.contains("AI · ruvx"));
        assert!(screen.contains("Tokens · input 413"));
        assert!(!screen.contains("attribution"));
        let (_, button) = app
            .frame
            .buttons
            .iter()
            .find(|(_, b)| matches!(b, screen::Button::Detail(_)))
            .unwrap()
            .clone();
        app.activate(button, 40).unwrap();
        assert!(render(&mut app, 100, 40).contains("attribution"));
    }
    #[test]
    fn shell_review_suspends_and_restores_unsaved_paste_edits() {
        let (_temp, mut app, _rx) = fixture();
        app.paste(&"line\n".repeat(20)).unwrap();
        let id = app.editor.pastes[0].id;
        app.open_paste(id).unwrap();
        app.paste_editor
            .as_mut()
            .unwrap()
            .insert("draft edit ")
            .unwrap();
        let (tx, mut rx) = oneshot::channel();
        app.event(
            UiEvent::Approval {
                review: "Shell approval\nprintf safe".into(),
                respond: tx,
            },
            40,
        )
        .unwrap();
        assert!(app.paste_editor.is_none());
        assert!(render(&mut app, 100, 40).contains("Shell approval"));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 40)
            .unwrap();
        assert!(app.paste_editor.is_some());
        assert!(rx.try_recv().is_err());
        app.save_paste().unwrap();
        assert!(
            app.editor.pastes[0]
                .source
                .contents()
                .unwrap()
                .starts_with("draft edit ")
        );
    }
    #[test]
    fn context_recovery_and_model_picker_preserve_large_draft() {
        let (_temp, mut app, _rx) = effort_fixture();
        let text = "draft line\n".repeat(20);
        app.paste(&text).unwrap();
        app.context_blocked = true;
        let view = render(&mut app, 100, 40);
        assert!(view.contains("[Compact context]"));
        assert!(
            app.activate(screen::Button::Command("/compact".into()), 40)
                .is_err()
        );
        assert_eq!(app.editor.pastes[0].source.contents().unwrap(), text);
        app.activate(screen::Button::Command("/new".into()), 40)
            .unwrap();
        assert_eq!(app.selection.as_ref().unwrap().model, "a");
        let view = render(&mut app, 100, 40);
        assert!(view.contains("local/a"));
        app.activate(screen::Button::Command("/model".into()), 40)
            .unwrap();
        assert!(app.editor.pastes.is_empty());
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 40)
            .unwrap();
        assert!(app.selection.is_some());
        assert_eq!(app.editor.pastes[0].source.contents().unwrap(), text);
    }
}
