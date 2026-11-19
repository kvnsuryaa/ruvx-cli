//! Local command metadata and the transient composer menu.
use super::*;

pub(super) struct Command {
    pub name: &'static str,
    pub args: &'static str,
    pub description: &'static str,
    pub required: bool,
}

pub(super) const COMMANDS: &[Command] = &[
    Command {
        name: "/help",
        args: "",
        description: "Show commands and shortcuts",
        required: false,
    },
    Command {
        name: "/connect",
        args: "[profile]",
        description: "Connect API key, ChatGPT account, or compatible endpoint",
        required: false,
    },
    Command {
        name: "/disconnect",
        args: "",
        description: "Select a profile to disconnect",
        required: false,
    },
    Command {
        name: "/model",
        args: "[profile model]",
        description: "Choose or add a model",
        required: false,
    },
    Command {
        name: "/effort",
        args: "[default|level]",
        description: "Choose reasoning effort for the selected model",
        required: false,
    },
    Command {
        name: "/mode",
        args: "plan|build",
        description: "Change mode while idle",
        required: true,
    },
    Command {
        name: "/new",
        args: "",
        description: "New Plan session in this workspace",
        required: false,
    },
    Command {
        name: "/sessions",
        args: "",
        description: "List saved sessions",
        required: false,
    },
    Command {
        name: "/resume",
        args: "<session-id>",
        description: "Resume without replaying actions",
        required: true,
    },
    Command {
        name: "/compact",
        args: "",
        description: "Manually summarize older context (uses model)",
        required: false,
    },
    Command {
        name: "/diff",
        args: "[path]",
        description: "Review changes recorded in this session",
        required: false,
    },
    Command {
        name: "/continue",
        args: "",
        description: "Continue after the turn limit",
        required: false,
    },
    Command {
        name: "/acknowledge",
        args: "",
        description: "Acknowledge inspecting uncertain interrupted actions",
        required: false,
    },
    Command {
        name: "/quit",
        args: "",
        description: "Save and exit",
        required: false,
    },
];

#[derive(Default)]
pub(super) struct Menu {
    revision: Option<u64>,
    dismissed: Option<u64>,
    pub selected: usize,
}

impl App {
    pub(super) fn command_matches(&mut self) -> Option<Vec<&'static Command>> {
        if self.command_menu.revision != Some(self.editor.revision) {
            self.command_menu.revision = Some(self.editor.revision);
            self.command_menu.selected = 0;
        }
        let text = &self.editor.text;
        if !matches!(self.dialog, Dialog::None)
            || self.paste_editor.is_some()
            || !self.editor.pastes.is_empty()
            || !text.starts_with('/')
            || text.starts_with("//")
            || text.chars().any(char::is_whitespace)
            || self.command_menu.dismissed == Some(self.editor.revision)
        {
            return None;
        }
        let matches: Vec<_> = COMMANDS
            .iter()
            .filter(|c| c.name.starts_with(text))
            .collect();
        self.command_menu.selected = self
            .command_menu
            .selected
            .min(matches.len().saturating_sub(1));
        Some(matches)
    }

    pub(super) fn move_command(&mut self, down: bool) {
        if let Some(matches) = self.command_matches()
            && !matches.is_empty()
        {
            let len = matches.len();
            self.command_menu.selected =
                (self.command_menu.selected + if down { 1 } else { len - 1 }) % len;
        }
    }

    pub(super) fn choose_command(&mut self, name: &str, complete: bool, height: u16) -> Result<()> {
        let Some(command) = self
            .command_matches()
            .and_then(|items| items.into_iter().find(|c| c.name == name))
        else {
            return Ok(());
        };
        if !complete && self.active.is_some() {
            self.status =
                "Work is active. Wait or Ctrl+C to cancel; select the command again when ready."
                    .into();
            return Ok(());
        }
        self.focused_button = None;
        self.gesture = None;
        self.view().selection = None;
        if complete || command.required {
            self.editor.set(format!("{} ", command.name));
        } else {
            self.editor.set(command.name.into());
            self.submit(height)?;
        }
        Ok(())
    }

    pub(super) fn command_key(&mut self, key: KeyEvent, height: u16) -> Result<bool> {
        if !key.modifiers.is_empty() || self.focused_button.is_some() {
            return Ok(false);
        }
        let Some(items) = self.command_matches() else {
            return Ok(false);
        };
        match key.code {
            KeyCode::Up | KeyCode::Down => self.move_command(key.code == KeyCode::Down),
            KeyCode::Enter | KeyCode::Tab => {
                if let Some(command) = items.get(self.command_menu.selected) {
                    self.choose_command(command.name, key.code == KeyCode::Tab, height)?;
                }
            }
            KeyCode::Esc => {
                self.command_menu.dismissed = Some(self.editor.revision);
                self.focused_button = None;
                self.gesture = None;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

pub(super) fn help() -> String {
    let mut text = String::from("ruvx commands\n");
    for command in COMMANDS {
        let syntax = format!("{} {}", command.name, command.args);
        text.push_str(&format!("{syntax:<24} {}\n", command.description));
        if command.name == "/effort" {
            text.push_str(
                "/effort levels [...]     Configure supported efforts for a custom/unknown model\n",
            );
        }
    }
    text.push_str(INPUT_HELP);
    text
}

const INPUT_HELP: &str = "\n/ opens commands · Up/Down selects · Enter opens · Tab completes · Escape closes\n\nEnter sends · Alt+Enter adds a line · bracketed paste accepts multiline\nOption+Delete / Ctrl+W deletes a word · Ctrl+U deletes to line start\nOption may need Meta/Esc mapping\nLong pastes become previews · click or F2 edits · Ctrl+S saves · Escape cancels\nDrag selects and copies on release; Ctrl+C copies/clears a selection first\nPgUp/PgDn or wheel scroll · Alt+End latest · Shift+Tab switches Plan/Build (idle) · F6/Shift+F6 focuses controls\nTab/arrows navigate controls · Enter opens · Escape returns to input or closes review\nCtrl+C without selection cancels active work; idle: clear draft, then exit\n@path or @\"path with spaces\" attaches file data; // sends a literal /\n\nPlan: read/search only. Build: direct edits; every shell command needs approval.\nShell uses OS permissions; cwd is not an OS sandbox.\n";
