//! Minimal MCP stdio client (JSON-RPC, newline-delimited JSON).
//! Connections are approved, scoped to one agent run, and never retried.
use crate::{
    config::{Config, McpServerConfig},
    events::UiEvent,
    model::{Mode, ToolCall},
    session::Session,
};
use anyhow::{Context, Result, bail, ensure};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

const PROTOCOL: &str = "2024-11-05";

/// The server may have acted. Keep the journal pending and stop the run.
#[derive(Debug)]
pub struct UncertainAction(pub String);
impl fmt::Display for UncertainAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MCP outcome is uncertain: {}. Inspect effects, then /acknowledge or /new; do not replay automatically",
            self.0
        )
    }
}
impl std::error::Error for UncertainAction {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    server: String,
    cursor: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallArgs {
    server: String,
    tool: String,
    arguments: Value,
}

#[derive(Default)]
pub struct Manager {
    connections: BTreeMap<String, Connection>,
}

impl Manager {
    pub async fn execute(
        &mut self,
        session: &mut Session,
        config: &Config,
        call: &ToolCall,
        tx: &mpsc::Sender<UiEvent>,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        ensure!(
            session.mode == Mode::Build,
            "MCP is not allowed in Plan; only the user may switch to Build"
        );
        let (server_id, method, params) = match call.name.as_str() {
            "mcp_list_tools" => {
                let args: ListArgs = serde_json::from_str(&call.arguments)?;
                (
                    args.server,
                    "tools/list",
                    match args.cursor {
                        Some(cursor) => json!({"cursor":cursor}),
                        None => json!({}),
                    },
                )
            }
            "mcp_call_tool" => {
                let args: CallArgs = serde_json::from_str(&call.arguments)?;
                ensure!(
                    args.arguments.is_object(),
                    "MCP arguments must be an object"
                );
                ensure!(
                    self.connections
                        .get(&args.server)
                        .is_some_and(|c| c.tools.contains(&args.tool)),
                    "Discover this tool with mcp_list_tools first (including additional cursors if needed)"
                );
                (
                    args.server,
                    "tools/call",
                    json!({"name":args.tool,"arguments":args.arguments}),
                )
            }
            _ => bail!("Unknown MCP operation"),
        };
        let server = config
            .mcp_servers
            .get(&server_id)
            .filter(|s| s.enabled)
            .context("MCP server is missing or disabled")?;
        let request_id = self
            .connections
            .get(&server_id)
            .map_or(2, |connection| connection.next_id + 1);
        let request = json!({"jsonrpc":"2.0","id":request_id,"method":method,"params":params});
        ensure!(
            serde_json::to_vec(&request)?.len() < server.max_message_bytes,
            "MCP request exceeds max_message_bytes; no operation was sent"
        );
        ensure!(!cancel.is_cancelled(), "MCP canceled before approval");
        let starting = !self.connections.contains_key(&server_id);
        let review = format!(
            "MCP approval · run once or deny\nServer: {server_id}\nOperation: {method}\nParameters:\n{}\nStart server: {starting}\nExecutable: {}\nArguments: {}\nWorking directory: {}\nInherited environment names: {}\nTimeout: {}s\nServer runs with your OS permissions, not in a sandbox, and may access network/files outside this workspace. Startup and discovery can also have side effects. The server stays alive only until this agent run ends. MCP effects are not included in /diff. Approve only a trusted server.\n",
            serde_json::to_string_pretty(&params)?,
            server.command.display(),
            serde_json::to_string(&server.args)?,
            session.root.display(),
            server.inherit_env.join(", "),
            server.timeout_seconds
        );
        let (respond, answer) = oneshot::channel();
        tx.send(UiEvent::Approval { review, respond })
            .await
            .context("Terminal closed before MCP approval")?;
        let approved = tokio::select! { biased; _ = cancel.cancelled() => false, answer = answer => answer.unwrap_or(false) };
        session.record("approval.resolved", json!({"call_id":call.id,"approved":approved,"server":server_id,"method":method,"params":params,"starting":starting}))?;
        if !approved {
            return Ok(
                json!({"status":if cancel.is_cancelled(){"canceled"}else{"denied"},"message":"MCP operation was not executed"}),
            );
        }
        ensure!(!cancel.is_cancelled(), "MCP canceled before execution");
        // Persist the intent before spawning or writing any request.
        session.record("tool.intent", json!({"call_id":call.id,"name":call.name,"server":server_id,"method":method,"params":params}))?;
        let operation = async {
            if starting {
                let mut connection = Connection::spawn(server, &session.root)?;
                connection.initialize().await?;
                self.connections.insert(server_id.clone(), connection);
            }
            let connection = self
                .connections
                .get_mut(&server_id)
                .context("MCP connection missing")?;
            let result = connection.request(method, params).await?;
            if method == "tools/list" {
                validate_tools(&result)?;
            } else {
                ensure!(
                    result.is_object()
                        && result["content"].as_array().is_some_and(|items| items
                            .iter()
                            .all(|item| item.is_object() && item["type"].is_string())),
                    "Invalid MCP tools/call content"
                );
                ensure!(
                    result.get("isError").is_none_or(Value::is_boolean),
                    "Invalid MCP isError flag"
                );
            }
            Ok::<_, anyhow::Error>(result)
        };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(anyhow::anyhow!("operation canceled")),
            result = tokio::time::timeout(Duration::from_secs(server.timeout_seconds), operation) => match result {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("operation timed out")),
            }
        };
        match result {
            Ok(result) => {
                let status = if result["isError"].as_bool() == Some(true) {
                    "error"
                } else {
                    "ok"
                };
                let response = json!({"status":status,"result":result,"truncated":false});
                if serde_json::to_vec(&response)?.len() <= config.tools.result_max_bytes {
                    if method == "tools/list" {
                        // Only cache names after their complete schemas fit the visible result.
                        let connection = self
                            .connections
                            .get_mut(&server_id)
                            .context("MCP connection missing")?;
                        for tool in response["result"]["tools"].as_array().unwrap() {
                            connection
                                .tools
                                .insert(tool["name"].as_str().unwrap().into());
                        }
                    }
                    Ok(response)
                } else if method == "tools/list" {
                    Ok(
                        json!({"status":"error","message":"MCP discovery exceeds tools.result_max_bytes. No tools from this page were registered. Request a smaller server page or increase the limit."}),
                    )
                } else {
                    excerpt(status, &result, config.tools.result_max_bytes)
                }
            }
            Err(error) => {
                // Dropping a connection terminates the entire process group, including descendants.
                self.connections.remove(&server_id);
                Err(UncertainAction(format!("{server_id}: {error:#}")).into())
            }
        }
    }
}

fn validate_tools(result: &Value) -> Result<()> {
    let tools = result["tools"]
        .as_array()
        .context("Invalid MCP tools/list result")?;
    ensure!(
        result.get("nextCursor").is_none_or(Value::is_string),
        "Invalid MCP tools cursor"
    );
    let mut names = BTreeSet::new();
    for tool in tools {
        let name = tool["name"].as_str().context("MCP tool has no name")?;
        ensure!(
            !name.is_empty() && names.insert(name),
            "Empty or duplicate MCP tool name"
        );
        ensure!(
            tool.get("description").is_none_or(Value::is_string),
            "Invalid MCP tool description"
        );
        let schema = &tool["inputSchema"];
        ensure!(
            schema.is_object()
                && schema["type"] == "object"
                && schema.get("properties").is_none_or(Value::is_object)
                && schema.get("required").is_none_or(|required| required
                    .as_array()
                    .is_some_and(|items| items.iter().all(Value::is_string))),
            "Invalid MCP tool inputSchema"
        );
    }
    Ok(())
}

fn excerpt(status: &str, result: &Value, limit: usize) -> Result<Value> {
    let text = serde_json::to_string(result)?;
    let mut response = json!({"status":status,"result_excerpt":"","truncated":true});
    let (mut low, mut high) = (0, text.len());
    while low < high {
        let candidate = low + (high - low).div_ceil(2);
        let mut end = candidate;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        response["result_excerpt"] = json!(&text[..end]);
        if serde_json::to_vec(&response)?.len() <= limit {
            low = candidate;
        } else {
            high = candidate - 1;
        }
    }
    while !text.is_char_boundary(low) {
        low -= 1;
    }
    response["result_excerpt"] = json!(&text[..low]);
    ensure!(
        serde_json::to_vec(&response)?.len() <= limit,
        "MCP result envelope exceeds result_max_bytes"
    );
    Ok(response)
}

struct Connection {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    pid: u32,
    next_id: u64,
    max_message_bytes: usize,
    tools: BTreeSet<String>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = killpg(Pid::from_raw(self.pid as i32), Signal::SIGKILL);
        let _ = self.child.start_kill();
    }
}
impl Connection {
    fn spawn(config: &McpServerConfig, root: &Path) -> Result<Self> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .current_dir(root)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);
        for name in &config.inherit_env {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut child = command.spawn().context("Could not start MCP server")?;
        let pid = child.id().context("MCP child has no PID")?;
        let stdin = child.stdin.take().context("Missing MCP stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("Missing MCP stdout")?);
        Ok(Self {
            child,
            stdin,
            stdout,
            pid,
            next_id: 0,
            max_message_bytes: config.max_message_bytes,
            tools: BTreeSet::new(),
        })
    }
    async fn initialize(&mut self) -> Result<()> {
        let result = self.request("initialize", json!({"protocolVersion":PROTOCOL,"capabilities":{},"clientInfo":{"name":"ruvx","version":env!("CARGO_PKG_VERSION")}})).await?;
        ensure!(
            result["protocolVersion"] == PROTOCOL,
            "Unsupported negotiated MCP protocol version"
        );
        ensure!(
            result["capabilities"]["tools"].is_object(),
            "MCP server does not advertise tools"
        );
        ensure!(
            result["serverInfo"]["name"]
                .as_str()
                .is_some_and(|name| !name.is_empty())
                && result["serverInfo"]["version"].is_string(),
            "Invalid MCP serverInfo"
        );
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await
    }
    async fn send(&mut self, value: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() <= self.max_message_bytes,
            "MCP request exceeds max_message_bytes"
        );
        self.stdin.write_all(&bytes).await?;
        self.stdin.flush().await?;
        Ok(())
    }
    async fn receive(&mut self) -> Result<Value> {
        let mut line = Vec::new();
        loop {
            let available = self.stdout.fill_buf().await?;
            ensure!(!available.is_empty(), "MCP server closed stdout");
            let end = available.iter().position(|b| *b == b'\n').map(|n| n + 1);
            let count = end.unwrap_or(available.len());
            ensure!(
                line.len() + count <= self.max_message_bytes,
                "MCP response exceeds max_message_bytes"
            );
            line.extend_from_slice(&available[..count]);
            self.stdout.consume(count);
            if end.is_some() {
                break;
            }
        }
        serde_json::from_slice(&line).map_err(|_| anyhow::anyhow!("Invalid MCP JSON message"))
    }
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        loop {
            let message = self.receive().await?;
            ensure!(message["jsonrpc"] == "2.0", "Invalid MCP JSON-RPC version");
            ensure!(message.is_object(), "MCP message must be an object");
            if let Some(method) = message.get("method") {
                let method = method.as_str().context("Invalid MCP method")?;
                if let Some(request_id) = message.get("id") {
                    ensure!(
                        request_id.is_string() || request_id.is_number(),
                        "Invalid MCP request ID"
                    );
                    // Ping has no side effects; no other server-initiated capabilities are offered.
                    let response = if method == "ping" {
                        json!({"jsonrpc":"2.0","id":request_id,"result":{}})
                    } else {
                        json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"Client capability not supported"}})
                    };
                    self.send(response).await?;
                }
                continue;
            }
            ensure!(message["id"] == id, "Unexpected MCP response ID");
            ensure!(
                message.get("result").is_some() != message.get("error").is_some(),
                "MCP response must contain exactly one result or error"
            );
            if message.get("error").is_some() {
                bail!("MCP server returned a JSON-RPC error");
            }
            return message
                .get("result")
                .cloned()
                .context("MCP response has no result");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;

    // A portable local JSON-RPC fixture; no LLM, network, npm, or Python needed.
    const FIXTURE: &str = r#"
        printf '%s' "$$" > server.pid
        n=0
        while IFS= read -r line; do
            case "$line" in
                *'"method":"initialize"'*)
                    n=$((n+1))
                    printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}\n' "$n";;
                *'"method":"tools/list"'*)
                    n=$((n+1))
                    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}'
                    printf '%s\n' '{"jsonrpc":"2.0","id":"server-request","method":"sampling/createMessage","params":{}}'
                    printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo input","inputSchema":{"type":"object"}}],"nextCursor":"page2"}}\n' "$n";;
                *'"method":"tools/call"'*)
                    n=$((n+1))
                    printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"fixture result"}]}}\n' "$n";;
            esac
        done
    "#;
    fn setup(script: &str) -> (tempfile::TempDir, Config, Session) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        let paths = Paths::at(&temp.path().join("state"));
        let mut session = Session::create(&paths, &root).unwrap();
        session.set_mode(Mode::Build).unwrap();
        let mut config = Config::default();
        config.mcp_servers.insert(
            "fixture".into(),
            McpServerConfig {
                enabled: true,
                command: "/bin/sh".into(),
                args: vec!["-c".into(), script.into()],
                timeout_seconds: 1,
                ..Default::default()
            },
        );
        config.validate().unwrap();
        (temp, config, session)
    }
    fn list_call() -> ToolCall {
        ToolCall {
            id: "list".into(),
            name: "mcp_list_tools".into(),
            arguments: json!({"server":"fixture"}).to_string(),
        }
    }
    async fn execute(
        mut manager: Manager,
        mut session: Session,
        config: Config,
        call: ToolCall,
        approve: bool,
    ) -> (Manager, Session, Result<Value>) {
        let (tx, mut rx) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            let result = manager
                .execute(&mut session, &config, &call, &tx, &CancellationToken::new())
                .await;
            (manager, session, result)
        });
        while let Some(event) = rx.recv().await {
            if let UiEvent::Approval { review, respond } = event {
                assert!(review.contains("OS permissions"));
                respond.send(approve).unwrap();
            }
        }
        task.await.unwrap()
    }
    #[tokio::test]
    async fn discovery_call_and_drop_cleanup() {
        let (_temp, config, session) = setup(FIXTURE);
        let root = session.root.clone();
        let (manager, session, result) = execute(
            Manager::default(),
            session,
            config.clone(),
            list_call(),
            true,
        )
        .await;
        assert_eq!(result.unwrap()["result"]["nextCursor"], "page2");
        assert_eq!(manager.connections.len(), 1);
        let call = ToolCall {
            id: "echo".into(),
            name: "mcp_call_tool".into(),
            arguments: json!({"server":"fixture","tool":"echo","arguments":{"text":"test"}})
                .to_string(),
        };
        let (manager, _session, result) = execute(manager, session, config, call, true).await;
        assert_eq!(
            result.unwrap()["result"]["content"][0]["text"],
            "fixture result"
        );
        let pid: i32 = std::fs::read_to_string(root.join("server.pid"))
            .unwrap()
            .parse()
            .unwrap();
        drop(manager);
        for _ in 0..100 {
            if nix::sys::signal::kill(Pid::from_raw(pid), None).is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("MCP process survived connection drop");
    }
    #[tokio::test]
    async fn denial_and_plan_do_not_spawn() {
        let (_temp, config, session) = setup(FIXTURE);
        let root = session.root.clone();
        let (manager, mut session, result) = execute(
            Manager::default(),
            session,
            config.clone(),
            list_call(),
            false,
        )
        .await;
        assert_eq!(result.unwrap()["status"], "denied");
        assert!(!root.join("server.pid").exists());
        session.set_mode(Mode::Plan).unwrap();
        let (_, _, result) = execute(manager, session, config, list_call(), true).await;
        assert!(result.is_err());
        assert!(!root.join("server.pid").exists());
    }
    #[tokio::test]
    async fn timeout_and_oversized_messages_are_uncertain() {
        for script in [
            "sleep 10".to_owned(),
            "while IFS= read -r line; do printf '%0300d\\n' 0; done".to_owned(),
        ] {
            let (_temp, mut config, session) = setup(&script);
            config
                .mcp_servers
                .get_mut("fixture")
                .unwrap()
                .max_message_bytes = 256;
            let (manager, session, result) =
                execute(Manager::default(), session, config, list_call(), true).await;
            assert!(
                result
                    .unwrap_err()
                    .downcast_ref::<UncertainAction>()
                    .is_some()
            );
            assert!(manager.connections.is_empty());
            assert!(session.pending_tools.contains("list"));
        }
    }
    #[tokio::test]
    async fn cancellation_is_uncertain_and_does_not_replay() {
        let (_temp, config, mut session) = setup("sleep 10");
        let (tx, mut rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            let mut manager = Manager::default();
            let result = manager
                .execute(&mut session, &config, &list_call(), &tx, &token)
                .await;
            (session, result)
        });
        if let Some(UiEvent::Approval { respond, .. }) = rx.recv().await {
            respond.send(true).unwrap();
        } else {
            panic!("Missing approval");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
        let (session, result) = task.await.unwrap();
        assert!(
            result
                .unwrap_err()
                .downcast_ref::<UncertainAction>()
                .is_some()
        );
        assert!(session.pending_tools.contains("list"));
    }

    #[tokio::test]
    async fn oversized_discovery_does_not_cache_hidden_tools_and_can_be_retried_with_larger_budget()
    {
        let script = FIXTURE.replace("Echo input", &"large ".repeat(100));
        let (_temp, mut config, session) = setup(&script);
        config.tools.result_max_bytes = 256;
        let (manager, session, result) = execute(
            Manager::default(),
            session,
            config.clone(),
            list_call(),
            true,
        )
        .await;
        let result = result.unwrap();
        assert_eq!(result["status"], "error");
        assert!(serde_json::to_vec(&result).unwrap().len() <= 256);
        assert!(manager.connections["fixture"].tools.is_empty());
        let call = ToolCall {
            id: "hidden".into(),
            name: "mcp_call_tool".into(),
            arguments: json!({"server":"fixture","tool":"echo","arguments":{}}).to_string(),
        };
        let (manager, session, result) =
            execute(manager, session, config.clone(), call, true).await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Discover this tool")
        );
        config.tools.result_max_bytes = 4096;
        let (manager, session, result) =
            execute(manager, session, config.clone(), list_call(), true).await;
        assert_eq!(result.unwrap()["result"]["tools"][0]["name"], "echo");
        let call = ToolCall { id: "too-large".into(), name: "mcp_call_tool".into(), arguments: json!({"server":"fixture","tool":"echo","arguments":{"data":"x".repeat(1024 * 1024)}}).to_string() };
        let (_, session, result) = execute(manager, session, config, call, true).await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no operation was sent")
        );
        assert!(!session.pending_tools.contains("too-large"));
    }

    #[tokio::test]
    async fn protocol_failures_stop_the_connection_without_cache_or_retry() {
        for script in [
            FIXTURE.replace(PROTOCOL, "2099-01-01"),
            FIXTURE.replace("\"tools\":{}", "\"tools\":false"),
            FIXTURE.replace("\"serverInfo\"", "\"missingServerInfo\""),
            FIXTURE.replace(
                "\"inputSchema\":{\"type\":\"object\"}",
                "\"inputSchema\":{\"type\":\"string\"}",
            ),
            FIXTURE.replace("\"nextCursor\":\"page2\"", "\"nextCursor\":42"),
            FIXTURE.replace("\"jsonrpc\":\"2.0\"", "\"jsonrpc\":\"1.0\""),
            FIXTURE.replace("\"id\":%s", "\"id\":999"),
            "printf '%s\\n' 'not json'".into(),
            "exit 0".into(),
        ] {
            let (_temp, config, session) = setup(&script);
            let (manager, session, result) =
                execute(Manager::default(), session, config, list_call(), true).await;
            assert!(
                result
                    .unwrap_err()
                    .downcast_ref::<UncertainAction>()
                    .is_some()
            );
            assert!(manager.connections.is_empty());
            assert!(session.pending_tools.contains("list"));
        }
    }

    #[tokio::test]
    async fn server_tool_errors_are_known_results_and_output_is_bounded() {
        let script = FIXTURE
            .replace("fixture result", &"failed 日本語 ".repeat(100))
            .replace(
                "\"result\":{\"content\"",
                "\"result\":{\"isError\":true,\"content\"",
            );
        let (_temp, mut config, session) = setup(&script);
        let (manager, session, result) = execute(
            Manager::default(),
            session,
            config.clone(),
            list_call(),
            true,
        )
        .await;
        assert_eq!(result.unwrap()["status"], "ok");
        config.tools.result_max_bytes = 256;
        let call = ToolCall {
            id: "error".into(),
            name: "mcp_call_tool".into(),
            arguments: json!({"server":"fixture","tool":"echo","arguments":{}}).to_string(),
        };
        let (manager, _, result) = execute(manager, session, config, call, true).await;
        let result = result.unwrap();
        assert_eq!(result["status"], "error");
        assert_eq!(result["truncated"], true);
        assert!(serde_json::to_vec(&result).unwrap().len() <= 256);
        assert_eq!(manager.connections.len(), 1);
    }

    #[test]
    fn schemas_and_escaped_excerpts_are_validated_without_losing_boundaries() {
        for result in [
            json!({"tools":[{"name":"echo","inputSchema":{"type":"object"}},{"name":"echo","inputSchema":{"type":"object"}}]}),
            json!({"tools":[{"name":"echo","inputSchema":{"type":"object","required":[42]}}]}),
            json!({"tools":[{"name":"echo","inputSchema":{"type":"object","properties":[]}}]}),
        ] {
            assert!(validate_tools(&result).is_err());
        }
        let result = json!({"text":"\\\"日本語\n".repeat(100)});
        for limit in [256, 257, 512, 1024] {
            let response = excerpt("ok", &result, limit).unwrap();
            assert!(serde_json::to_vec(&response).unwrap().len() <= limit);
            assert_eq!(response["truncated"], true);
            assert!(!response["result_excerpt"].as_str().unwrap().is_empty());
        }
    }
}
