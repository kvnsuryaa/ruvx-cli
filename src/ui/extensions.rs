//! User-directed extension configuration. These flows never start an agent or server.
use super::*;
use crate::config::McpServerConfig;
use std::path::PathBuf;

#[derive(Clone)]
pub(super) enum Setup {
    SkillAdd,
    SkillRemove(Vec<PathBuf>),
    Mcp(McpDraft),
}

#[derive(Clone, Copy)]
enum Step {
    Id,
    Command,
    Arguments,
    Environment,
    Timeout,
    Limit,
    Enabled,
    Save,
}

#[derive(Clone)]
pub(super) struct McpDraft {
    id: String,
    server: McpServerConfig,
    step: Step,
    editing: bool,
}

impl Setup {
    pub(super) fn label(&self) -> &str {
        match self {
            Self::SkillAdd => {
                "Local skill folder or one-level collection; relative to workspace; Escape cancels"
            }
            Self::SkillRemove(_) => {
                "Location number to unregister; source files are preserved; Escape cancels"
            }
            Self::Mcp(draft) => match draft.step {
                Step::Id => "New server ID (letters, digits, _ and -); Escape cancels",
                Step::Command => {
                    "Absolute executable path; blank keeps current value; Escape cancels"
                }
                Step::Arguments => {
                    "Arguments as JSON array, e.g. [\"--stdio\"]; blank keeps current value"
                }
                Step::Environment => {
                    "Inherited environment NAMES as JSON array; blank keeps current value"
                }
                Step::Timeout => "Timeout in seconds, 1–600; blank keeps current value",
                Step::Limit => "Message limit in bytes, 256–16777216; blank keeps current value",
                Step::Enabled => "Enabled: yes or no; blank keeps current value",
                Step::Save => "Review configuration; type save to persist or cancel to discard",
            },
        }
    }

    pub(super) fn choices(&self) -> Vec<(String, String)> {
        match self {
            Self::SkillRemove(paths) => paths
                .iter()
                .enumerate()
                .map(|(i, path)| (format!("{} {}", i + 1, path.display()), (i + 1).to_string()))
                .collect(),
            Self::Mcp(McpDraft {
                step: Step::Save, ..
            }) => vec![
                ("Save configuration".into(), "save".into()),
                ("Cancel".into(), "cancel".into()),
            ],
            _ => vec![],
        }
    }
}

impl App {
    fn save_extensions(&mut self, config: Config) -> Result<()> {
        ensure!(
            self.active.is_none(),
            "Wait for active work or cancel before changing extensions"
        );
        self.paths.save(&config)?;
        self.config = config;
        self.line("Extension configuration saved. Applies to the next agent run; no server or script was started.");
        Ok(())
    }

    pub(super) fn skills_command(&mut self, args: &[&str], height: u16) -> Result<()> {
        match args {
            [] => {
                let root = &self.session.as_ref().context("Session unavailable")?.root;
                let (skills, warnings) = crate::skills::discover(&self.paths, &self.config, root)?;
                let mut output = format!(
                    "Agent Skills · enabled: {}\nUser: {}\nProject: {}\n",
                    self.config.skills.enabled,
                    self.paths.config.join("skills").display(),
                    root.join(".agents/skills").display()
                );
                for path in &self.config.skills.paths {
                    output.push_str(&format!("Additional: {}\n", path.display()));
                }
                output.push_str("\n/skills add · /skills remove · /skills enable · /skills disable\nNo scripts are executed.\n\n");
                if skills.is_empty() {
                    output.push_str("No skills discovered.\n");
                }
                for skill in skills {
                    output.push_str(&format!(
                        "{} [{}]\n{}\n{}\n\n",
                        skill.name,
                        skill.source,
                        skill.description,
                        skill.directory.display()
                    ));
                }
                for warning in warnings {
                    output.push_str(&format!("Warning: {warning}\n"));
                }
                self.page(output, Dialog::None, height);
            }
            ["add"] => self.open_extension(Setup::SkillAdd, height),
            ["remove"] => {
                ensure!(
                    !self.config.skills.paths.is_empty(),
                    "No additional skill locations to remove"
                );
                self.open_extension(Setup::SkillRemove(self.config.skills.paths.clone()), height);
            }
            [value @ ("enable" | "disable")] => {
                let mut config = self.config.clone();
                config.skills.enabled = *value == "enable";
                self.save_extensions(config)?;
            }
            _ => bail!("Usage: /skills [add|remove|enable|disable]"),
        }
        Ok(())
    }

    pub(super) fn mcp_command(&mut self, args: &[&str], height: u16) -> Result<()> {
        match args {
            [] => {
                let mut output = format!(
                    "MCP servers · stdio only\nConfig: {}\n/mcp add · /mcp edit <id> · /mcp remove <id>\n/mcp enable <id> · /mcp disable <id>\nEach operation requires approval in Build. Servers stop when the agent run ends.\nNo servers are started by configuration commands.\n\n",
                    self.paths.config_file().display()
                );
                for (id, server) in &self.config.mcp_servers {
                    output.push_str(&format!(
                        "{id} · {} · {} · timeout {}s\n",
                        if server.enabled {
                            "enabled"
                        } else {
                            "disabled"
                        },
                        server.command.display(),
                        server.timeout_seconds
                    ));
                }
                if self.config.mcp_servers.is_empty() {
                    output.push_str("No MCP servers configured.\n");
                }
                self.page(output, Dialog::None, height);
            }
            ["add"] => self.open_extension(
                Setup::Mcp(McpDraft {
                    id: String::new(),
                    server: McpServerConfig {
                        enabled: true,
                        ..Default::default()
                    },
                    step: Step::Id,
                    editing: false,
                }),
                height,
            ),
            ["edit", id] => {
                let server = self
                    .config
                    .mcp_servers
                    .get(*id)
                    .context("Unknown MCP server ID")?
                    .clone();
                self.open_extension(
                    Setup::Mcp(McpDraft {
                        id: (*id).into(),
                        server,
                        step: Step::Command,
                        editing: true,
                    }),
                    height,
                );
            }
            [action @ ("enable" | "disable" | "remove"), id] => {
                let mut config = self.config.clone();
                let server = config
                    .mcp_servers
                    .get_mut(*id)
                    .context("Unknown MCP server ID")?;
                if *action == "remove" {
                    config.mcp_servers.remove(*id);
                } else {
                    server.enabled = *action == "enable";
                }
                self.save_extensions(config)?;
            }
            _ => bail!("Usage: /mcp [add|edit <id>|remove <id>|enable <id>|disable <id>]"),
        }
        Ok(())
    }

    fn open_extension(&mut self, setup: Setup, height: u16) {
        let prompt = match &setup {
            Setup::SkillAdd => "Register a local skill folder containing SKILL.md, or a folder containing skill folders.\nPaths may contain spaces; do not add shell quotes. Relative paths use the workspace. Files are never copied or deleted.".into(),
            Setup::SkillRemove(paths) => paths.iter().enumerate().map(|(i, path)| format!("{} {}", i + 1, path.display())).collect::<Vec<_>>().join("\n"),
            Setup::Mcp(draft) => {
                let current = match draft.step {
                    Step::Id => "Choose a unique MCP server ID.".into(),
                    Step::Command => format!("Current executable: {}", draft.server.command.display()),
                    Step::Arguments => format!("Current arguments: {}", serde_json::to_string(&draft.server.args).unwrap()),
                    Step::Environment => format!("Current inherited environment names: {}\nEnter names only, never token values.", serde_json::to_string(&draft.server.inherit_env).unwrap()),
                    Step::Timeout => format!("Current timeout: {} seconds", draft.server.timeout_seconds),
                    Step::Limit => format!("Current message limit: {} bytes", draft.server.max_message_bytes),
                    Step::Enabled => format!("Currently enabled: {}", draft.server.enabled),
                    Step::Save => format!("Server: {}\n{}\nNo server will be started. Changes apply to future runs.", draft.id, toml::to_string_pretty(&draft.server).unwrap()),
                };
                format!("MCP stdio configuration\n{current}\n\n{}", setup.label())
            }
        };
        self.page(prompt, Dialog::Extension(setup), height);
    }

    pub(super) fn submit_extension(
        &mut self,
        setup: Setup,
        value: &str,
        height: u16,
    ) -> Result<()> {
        ensure!(
            self.active.is_none(),
            "Wait for active work or cancel before changing extensions"
        );
        match setup {
            Setup::SkillAdd => {
                ensure!(!value.is_empty(), "Skill location cannot be empty");
                let mut config = self.config.clone();
                let root = &self.session.as_ref().context("Session unavailable")?.root;
                crate::skills::register(
                    &self.paths,
                    &mut config,
                    root,
                    std::path::Path::new(value),
                )?;
                self.save_extensions(config)?;
            }
            Setup::SkillRemove(paths) => {
                let path = value
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|n| paths.get(n))
                    .context("Choose a displayed location number")?;
                let mut config = self.config.clone();
                config.skills.paths.retain(|p| p != path);
                self.save_extensions(config)?;
            }
            Setup::Mcp(mut draft) => {
                draft.step = match draft.step {
                    Step::Id => {
                        ensure!(
                            valid_id(value) && !self.config.mcp_servers.contains_key(value),
                            "Use a new MCP server ID with letters, digits, _ or -"
                        );
                        draft.id = value.into();
                        Step::Command
                    }
                    Step::Command => {
                        if !value.is_empty() {
                            draft.server.command = value.into();
                        }
                        ensure!(
                            draft.server.command.is_absolute()
                                && !draft
                                    .server
                                    .command
                                    .as_os_str()
                                    .as_encoded_bytes()
                                    .contains(&0),
                            "Executable must be an absolute path without NUL"
                        );
                        Step::Arguments
                    }
                    Step::Arguments => {
                        if !value.is_empty() {
                            draft.server.args = serde_json::from_str(value).map_err(|_| {
                                anyhow::anyhow!("Arguments must be a JSON array of strings")
                            })?;
                        }
                        ensure!(
                            draft.server.args.iter().all(|arg| !arg.contains('\0')),
                            "Arguments must not contain NUL"
                        );
                        Step::Environment
                    }
                    Step::Environment => {
                        if !value.is_empty() {
                            draft.server.inherit_env =
                                serde_json::from_str(value).map_err(|_| {
                                    anyhow::anyhow!(
                                        "Environment names must be a JSON array of strings"
                                    )
                                })?;
                        }
                        Self::validate_mcp_draft(&draft)?;
                        Step::Timeout
                    }
                    Step::Timeout => {
                        if !value.is_empty() {
                            draft.server.timeout_seconds =
                                value.parse().context("Timeout must be an integer")?;
                        }
                        ensure!(
                            (1..=600).contains(&draft.server.timeout_seconds),
                            "Timeout must be 1–600 seconds"
                        );
                        Step::Limit
                    }
                    Step::Limit => {
                        if !value.is_empty() {
                            draft.server.max_message_bytes =
                                value.parse().context("Message limit must be an integer")?;
                        }
                        ensure!(
                            (256..=16 * 1024 * 1024).contains(&draft.server.max_message_bytes),
                            "Message limit must be 256–16777216 bytes"
                        );
                        Step::Enabled
                    }
                    Step::Enabled => {
                        if !value.is_empty() {
                            draft.server.enabled = match value {
                                "yes" => true,
                                "no" => false,
                                _ => bail!("Type yes or no, or leave blank"),
                            };
                        }
                        Self::validate_mcp_draft(&draft)?;
                        Step::Save
                    }
                    Step::Save => {
                        if value == "cancel" {
                            return Ok(());
                        }
                        ensure!(value == "save", "Type save or cancel");
                        let mut config = self.config.clone();
                        ensure!(
                            draft.editing == config.mcp_servers.contains_key(&draft.id),
                            "MCP server configuration changed; reopen setup"
                        );
                        config.mcp_servers.insert(draft.id, draft.server);
                        return self.save_extensions(config);
                    }
                };
                self.open_extension(Setup::Mcp(draft), height);
            }
        }
        Ok(())
    }

    fn validate_mcp_draft(draft: &McpDraft) -> Result<()> {
        let mut config = Config::default();
        config
            .mcp_servers
            .insert(draft.id.clone(), draft.server.clone());
        config.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::{fixture, render};

    fn enter(app: &mut App, text: &str) -> Result<()> {
        app.editor.set(text.into());
        app.submit(40)
    }

    fn add_server(app: &mut App) {
        app.command("/mcp add", 40).unwrap();
        for value in [
            "demo",
            "/bin/sh",
            "[\"-c\",\"touch must-not-exist\"]",
            "[\"PATH\"]",
            "",
            "",
            "",
        ] {
            enter(app, value).unwrap();
        }
        assert!(app.config.mcp_servers.is_empty());
        assert!(render(app, 100, 40).contains("touch must-not-exist"));
        enter(app, "save").unwrap();
    }

    #[test]
    fn skill_registration_removal_and_toggle_preserve_source_and_apply_immediately() {
        let (_temp, mut app, mut rx) = fixture();
        let root = app.session.as_ref().unwrap().root.clone();
        let source = root.join("my collection/demo");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: demo\ndescription: Example\n---\nBody\n",
        )
        .unwrap();
        app.command("/skills add", 40).unwrap();
        enter(&mut app, "my collection").unwrap();
        let paths = app.config.skills.paths.clone();
        assert_eq!(app.paths.load().unwrap().skills.paths, paths);
        assert_eq!(
            crate::skills::discover(&app.paths, &app.config, &root)
                .unwrap()
                .0
                .len(),
            1
        );
        app.command("/skills add", 40).unwrap();
        assert!(enter(&mut app, "my collection/.").is_err());
        assert_eq!(app.config.skills.paths, paths);
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 40)
            .unwrap();
        app.command("/skills disable", 40).unwrap();
        assert!(
            crate::skills::catalog(&app.paths, &app.config, &root)
                .unwrap()
                .is_empty()
        );
        app.command("/skills enable", 40).unwrap();
        app.command("/skills remove", 40).unwrap();
        enter(&mut app, "1").unwrap();
        assert!(app.config.skills.paths.is_empty());
        assert!(app.paths.load().unwrap().skills.paths.is_empty());
        assert!(source.join("SKILL.md").is_file());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn mcp_setup_edit_toggle_remove_never_start_a_server() {
        let (_temp, mut app, mut rx) = fixture();
        add_server(&mut app);
        let saved = app.paths.load().unwrap();
        assert_eq!(saved.mcp_servers["demo"].timeout_seconds, 60);
        assert_eq!(saved.mcp_servers["demo"].max_message_bytes, 1024 * 1024);
        assert!(saved.mcp_servers["demo"].enabled);
        app.command("/mcp add", 40).unwrap();
        assert!(enter(&mut app, "demo").is_err());
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 40)
            .unwrap();
        app.command("/mcp edit demo", 40).unwrap();
        for value in ["", "[]", "[]", "25", "4096", "no", "save"] {
            enter(&mut app, value).unwrap();
        }
        assert_eq!(app.config.mcp_servers["demo"].timeout_seconds, 25);
        assert!(app.config.mcp_servers["demo"].args.is_empty());
        assert!(app.config.mcp_servers["demo"].inherit_env.is_empty());
        assert!(!app.config.mcp_servers["demo"].enabled);
        app.command("/mcp enable demo", 40).unwrap();
        assert!(app.config.mcp_servers["demo"].enabled);
        app.command("/mcp disable demo", 40).unwrap();
        assert!(!app.paths.load().unwrap().mcp_servers["demo"].enabled);
        app.command("/mcp remove demo", 40).unwrap();
        assert!(app.paths.load().unwrap().mcp_servers.is_empty());
        assert!(
            !app.session
                .as_ref()
                .unwrap()
                .root
                .join("must-not-exist")
                .exists()
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn invalid_inputs_and_cancel_leave_config_untouched_and_retry_preserves_draft() {
        let (_temp, mut app, mut rx) = fixture();
        app.command("/mcp add", 40).unwrap();
        enter(&mut app, "demo").unwrap();
        assert!(enter(&mut app, "relative-command").is_err());
        enter(&mut app, "/bin/sh").unwrap();
        assert!(enter(&mut app, "not json").is_err());
        enter(&mut app, "[]").unwrap();
        assert!(enter(&mut app, "[\"TOKEN=value\"]").is_err());
        enter(&mut app, "[]").unwrap();
        assert!(enter(&mut app, "0").is_err());
        enter(&mut app, "10").unwrap();
        assert!(enter(&mut app, "255").is_err());
        enter(&mut app, "256").unwrap();
        assert!(enter(&mut app, "maybe").is_err());
        enter(&mut app, "yes").unwrap();
        enter(&mut app, "cancel").unwrap();
        assert!(app.config.mcp_servers.is_empty());
        assert!(!app.paths.config_file().exists());
        app.command("/mcp add", 40).unwrap();
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 40)
            .unwrap();
        assert!(matches!(app.dialog, Dialog::None));
        assert!(!app.paths.config_file().exists());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn failed_save_and_active_work_preserve_configuration() {
        let (_temp, mut app, mut rx) = fixture();
        let blocker = app.paths.config.clone();
        std::fs::create_dir_all(blocker.parent().unwrap()).unwrap();
        std::fs::write(&blocker, "cannot create config here").unwrap();
        assert!(app.command("/skills disable", 40).is_err());
        assert!(app.config.skills.enabled);
        std::fs::remove_file(blocker).unwrap();
        app.command("/mcp add", 40).unwrap();
        for value in ["demo", "/bin/sh", "[]", "[]", "", "", ""] {
            enter(&mut app, value).unwrap();
        }
        std::fs::create_dir_all(app.paths.config_file()).unwrap();
        assert!(enter(&mut app, "save").is_err());
        assert!(app.config.mcp_servers.is_empty());
        assert!(matches!(app.dialog, Dialog::Extension(_)));
        app.active = Some(CancellationToken::new());
        assert!(app.command("/skills disable", 40).is_err());
        assert!(app.command("/mcp add", 40).is_err());
        assert!(app.config.skills.enabled);
        assert!(rx.try_recv().is_err());
    }
}
