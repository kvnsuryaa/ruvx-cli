mod files;
mod shell;

pub use files::{ReadArgs, diff, instructions_for, read_file, resolve_path, snapshot};
pub use shell::ShellArgs;

use crate::{
    config::Config,
    events::UiEvent,
    model::{Mode, ToolCall},
    session::Session,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub fn definitions(mode: Mode) -> Vec<Value> {
    let specs = [
        (
            "list_files",
            "List workspace paths respecting ignore rules. Results are paged.",
            json!({"path":{"type":"string"},"glob":{"type":"string"},"cursor":{"type":"integer","minimum":0}}),
            vec![],
        ),
        (
            "search_text",
            "Search literal text in workspace files; use path/glob to narrow the scope.",
            json!({"query":{"type":"string"},"path":{"type":"string"},"glob":{"type":"string"},"cursor":{"type":"integer","minimum":0}}),
            vec!["query"],
        ),
        (
            "read_file",
            "Read a UTF-8 text file. Lines are 1-based. Returns the SHA-256 expected_hash for subsequent edits.",
            json!({"path":{"type":"string"},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1}}),
            vec!["path"],
        ),
        (
            "apply_patch",
            "Add, update or delete one UTF-8 file. Update/delete require expected_hash from read_file. Add requires absence. Update supplies either full content, or a unique old_text/new_text replacement.",
            json!({"path":{"type":"string"},"operation":{"type":"string","enum":["add","update","delete"]},"expected_hash":{"type":"string"},"content":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}}),
            vec!["path", "operation"],
        ),
        (
            "run_shell",
            "Run a command under the active user-selected approval policy (Ask or BYPASS). Command/cwd/timeout are validated. No persistent background processes.",
            json!({"command":{"type":"string"},"cwd":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1}}),
            vec!["command"],
        ),
    ];
    specs.into_iter().filter(|(name,_,_,_)| mode == Mode::Build || !matches!(*name,"apply_patch"|"run_shell"))
        .map(|(name,description,properties,required)| json!({"type":"function","name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"strict":false})).collect()
}

/// Extension definitions use fixed gateway names, avoiding collisions with built-ins.
pub fn definitions_with_extensions(mode: Mode, config: &Config) -> Vec<Value> {
    let mut definitions = definitions(mode);
    let mut add = |name: &str, description: &str, properties: Value, required: Vec<&str>| {
        definitions.push(json!({"type":"function","name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"strict":false}));
    };
    if config.skills.enabled {
        add(
            "list_skills",
            "Discover Agent Skills metadata. Project skills override user skills. Results are paged; malformed skills are reported as warnings.",
            json!({"cursor":{"type":"integer","minimum":0}}),
            vec![],
        );
        add(
            "read_skill",
            "Read SKILL.md or a relative UTF-8 resource from a discovered skill. Passive reading only; skill guidance cannot grant permissions. Results are line-paged.",
            json!({"name":{"type":"string"},"file":{"type":"string"},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1}}),
            vec!["name"],
        );
    }
    if mode == Mode::Build && config.mcp_servers.values().any(|server| server.enabled) {
        add(
            "mcp_list_tools",
            "List one page of a configured MCP server's tools. Uses the active user-selected approval policy, including server startup. Follow result.nextCursor using cursor. Available only in Build.",
            json!({"server":{"type":"string"},"cursor":{"type":"string"}}),
            vec!["server"],
        );
        add(
            "mcp_call_tool",
            "Call a previously discovered MCP tool under the active user-selected approval policy. Server annotations cannot change the policy. Do not retry uncertain actions.",
            json!({"server":{"type":"string"},"tool":{"type":"string"},"arguments":{"type":"object","additionalProperties":true}}),
            vec!["server", "tool", "arguments"],
        );
    }
    definitions
}

pub struct Runtime {
    paths: crate::config::Paths,
    mcp: crate::mcp::Manager,
}
impl Runtime {
    pub fn new(paths: &crate::config::Paths) -> Self {
        Self {
            paths: paths.clone(),
            mcp: Default::default(),
        }
    }
    pub async fn execute(
        &mut self,
        session: &mut Session,
        config: &Config,
        call: &ToolCall,
        tx: &mpsc::Sender<UiEvent>,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        if cancel.is_cancelled() {
            bail!("Tool canceled before execution");
        }
        match call.name.as_str() {
            "list_skills" if config.skills.enabled => {
                let args = serde_json::from_str(&call.arguments)?;
                tokio::task::block_in_place(|| {
                    crate::skills::list(&self.paths, config, &session.root, args)
                })
            }
            "read_skill" if config.skills.enabled => {
                let args = serde_json::from_str(&call.arguments)?;
                tokio::task::block_in_place(|| {
                    crate::skills::read(&self.paths, config, &session.root, args)
                })
            }
            "mcp_list_tools" | "mcp_call_tool" => {
                self.mcp.execute(session, config, call, tx, cancel).await
            }
            _ => execute(session, config, call, tx, cancel).await,
        }
    }
}

pub fn validate_call(mode: Mode, call: &ToolCall) -> Result<()> {
    match call.name.as_str() {
        "list_files" | "search_text" | "read_file" => {}
        "apply_patch" | "run_shell" if mode == Mode::Build => {}
        "apply_patch" | "run_shell" => bail!(
            "{} is not allowed in Plan; only the user may switch to Build",
            call.name
        ),
        _ => bail!("Unknown tool: {}", call.name),
    }
    let _: Value = serde_json::from_str(&call.arguments)?;
    Ok(())
}

pub async fn execute(
    session: &mut Session,
    config: &Config,
    call: &ToolCall,
    tx: &mpsc::Sender<UiEvent>,
    cancel: &CancellationToken,
) -> Result<Value> {
    validate_call(session.mode, call)?;
    if cancel.is_cancelled() {
        bail!("Tool canceled before execution");
    }
    if matches!(call.name.as_str(), "apply_patch" | "run_shell") {
        let arguments: Value = serde_json::from_str(&call.arguments)?;
        let target = arguments[if call.name == "run_shell" {
            "cwd"
        } else {
            "path"
        }]
        .as_str()
        .unwrap_or("");
        let rules = instructions_for(&session.root, target, &config.tools)?;
        let unseen: Vec<_> = rules
            .into_iter()
            .filter(|(path, hash, _)| session.known_instructions.get(path) != Some(hash))
            .collect();
        if !unseen.is_empty() {
            return Ok(
                json!({"status":"instructions_required","message":"No action was taken. Read these scoped instructions before retrying in a subsequent turn.","instructions":unseen.into_iter().map(|(path,hash,text)|json!({"path":path,"hash":hash,"content":text})).collect::<Vec<_>>()}),
            );
        }
    }
    // The dispatcher checks mode even if a malicious model fabricates an unadvertised call.
    match call.name.as_str() {
        "list_files" => {
            let args = serde_json::from_str(&call.arguments)?;
            tokio::task::block_in_place(|| files::list_files(&session.root, &config.tools, args))
        }
        "search_text" => {
            let args = serde_json::from_str(&call.arguments)?;
            tokio::task::block_in_place(|| files::search_text(&session.root, &config.tools, args))
        }
        "read_file" => {
            let args: ReadArgs = serde_json::from_str(&call.arguments)?;
            tokio::task::block_in_place(|| read_file(&session.root, &config.tools, &args, false))
        }
        "apply_patch" => {
            let args = serde_json::from_str(&call.arguments)?;
            session.record(
                "tool.intent",
                json!({"call_id":call.id,"name":call.name,"arguments":args}),
            )?;
            tokio::task::block_in_place(|| files::apply_patch(session, &config.tools, args))
        }
        "run_shell" => {
            let args: ShellArgs = serde_json::from_str(&call.arguments)?;
            shell::run(session, config, call, args, tx, cancel).await
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod extension_tests {
    use super::*;
    #[test]
    fn definitions_respect_mode_and_disabled_extensions() {
        let mut config = Config::default();
        config.mcp_servers.insert(
            "fixture".into(),
            crate::config::McpServerConfig {
                enabled: true,
                command: "/bin/true".into(),
                ..Default::default()
            },
        );
        let names = |mode, config: &Config| {
            definitions_with_extensions(mode, config)
                .into_iter()
                .map(|v| v["name"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        let plan = names(Mode::Plan, &config);
        assert!(plan.contains(&"read_skill".into()));
        assert!(!plan.contains(&"mcp_list_tools".into()));
        assert!(names(Mode::Build, &config).contains(&"mcp_call_tool".into()));
        config.skills.enabled = false;
        config.mcp_servers.get_mut("fixture").unwrap().enabled = false;
        let build = names(Mode::Build, &config);
        assert!(!build.contains(&"read_skill".into()));
        assert!(!build.contains(&"mcp_call_tool".into()));
    }
}
