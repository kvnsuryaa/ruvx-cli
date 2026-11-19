use crate::{
    auth,
    config::{Config, ModelConfig, Paths, Profile, ProviderKind},
    events::UiEvent,
    model::{Message, Selection, ToolCall, Turn},
};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct PartialFailure {
    pub text: String,
    pub message: String,
}
impl std::fmt::Display for PartialFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for PartialFailure {}

pub fn client(config: &Config) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(config.network.header_timeout_seconds))
        .timeout(Duration::from_secs(config.network.request_timeout_seconds))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("ruvx/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

pub fn build_request(
    profile: &Profile,
    model: &str,
    history: &[Message],
    instructions: &str,
    tools: &[Value],
) -> Result<Value> {
    let model_config = profile.model(model)?;
    model_config.validate_effort(profile.kind, model)?;
    ensure!(
        model_config.supports_tools != Some(false) && model_config.supports_streaming,
        "Selected model must support tools and streaming"
    );
    let model_id = model_config.model_id.as_deref().unwrap_or(model);
    // Preserve the namespace of saved opaque calls without rewriting provider history.
    let namespace = if profile.kind == ProviderKind::Chatgpt
        && history.iter().any(|message| {
            matches!(message, Message::Assistant { turn } if turn.opaque.iter().any(|item| {
                item["type"] == "function_call" && (item["namespace"] == "kovr"
                    || item["name"].as_str().is_some_and(|name| name.starts_with("kovr.")))
            }))
        }) {
        "kovr"
    } else {
        "ruvx"
    };
    if profile.responses() {
        let mut input = vec![];
        for message in history {
            match message {
                Message::User { content } => input.push(json!({"role":"user","content":content})),
                Message::Assistant { turn } => {
                    if !turn.opaque.is_empty()
                        && turn.route.as_deref() == Some(profile.route().as_str())
                    {
                        input.extend(turn.opaque.clone());
                    } else {
                        ensure!(
                            !has_opaque(turn),
                            "Provider-specific reasoning cannot be translated; use /compact or /new before changing provider"
                        );
                        if !turn.text.is_empty() {
                            input.push(json!({"role":"assistant","content":turn.text}));
                        }
                        for call in &turn.calls {
                            let mut item = json!({"type":"function_call","call_id":call.id,"name":call.name,"arguments":call.arguments});
                            if profile.kind == ProviderKind::Chatgpt {
                                item["namespace"] = json!(namespace);
                            }
                            input.push(item);
                        }
                    }
                }
                Message::Tool {
                    call_id, content, ..
                } => input.push(
                    json!({"type":"function_call_output","call_id":call_id,"output":content}),
                ),
            }
        }
        let wire_tools = if profile.kind == ProviderKind::Chatgpt && !tools.is_empty() {
            vec![
                json!({"type":"namespace","name":namespace,"description":"Local workspace tools controlled by the user's Plan/Build mode.","tools":tools}),
            ]
        } else {
            tools.to_vec()
        };
        let mut request = json!({"model":model_id,"instructions":instructions,"input":input,"tools":wire_tools,"store":false,"stream":true,"include":["reasoning.encrypted_content"]});
        if let Some(effort) = model_config.reasoning_effort {
            request["reasoning"] = json!({"effort":effort});
        }
        Ok(request)
    } else {
        let mut messages = vec![json!({"role":"system","content":instructions})];
        for message in history {
            match message {
                Message::User { content } => {
                    messages.push(json!({"role":"user","content":content}))
                }
                Message::Assistant { turn } => {
                    ensure!(
                        !has_opaque(turn),
                        "Provider-specific reasoning cannot be translated; use /compact or /new"
                    );
                    let mut item = json!({"role":"assistant","content":if turn.text.is_empty(){Value::Null}else{json!(turn.text)}});
                    if !turn.calls.is_empty() {
                        item["tool_calls"]=json!(turn.calls.iter().map(|c|json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments}})).collect::<Vec<_>>());
                    }
                    messages.push(item);
                }
                Message::Tool {
                    call_id, content, ..
                } => messages.push(json!({"role":"tool","tool_call_id":call_id,"content":content})),
            }
        }
        let tools:Vec<_>=tools.iter().map(|t|json!({"type":"function","function":{"name":t["name"],"description":t["description"],"parameters":t["parameters"]}})).collect();
        let mut request = json!({"model":model_id,"messages":messages,"stream":true});
        if let Some(effort) = model_config.reasoning_effort {
            request["reasoning_effort"] = json!(effort);
        }
        if !tools.is_empty() {
            request["tools"] = json!(tools);
        }
        Ok(request)
    }
}

pub fn has_opaque(turn: &Turn) -> bool {
    turn.opaque
        .iter()
        .any(|v| !matches!(v["type"].as_str(), Some("message" | "function_call")))
}

pub struct GenerateInput<'a> {
    pub history: &'a [Message],
    pub instructions: &'a str,
    pub tools: &'a [Value],
}

pub async fn generate(
    paths: &Paths,
    config: &Config,
    selection: &Selection,
    input: GenerateInput<'_>,
    tx: &mpsc::Sender<UiEvent>,
    cancel: &CancellationToken,
) -> Result<Turn> {
    let profile = config
        .providers
        .get(&selection.profile)
        .context("Selected provider profile was removed; use /connect")?;
    let body = build_request(
        profile,
        &selection.model,
        input.history,
        input.instructions,
        input.tools,
    )?;
    let token = auth::resolve(paths, profile, config, cancel).await?;
    let client = client(config)?;
    let endpoint = format!(
        "{}/{}",
        profile.base_url().trim_end_matches('/'),
        if profile.responses() {
            "responses"
        } else {
            "chat/completions"
        }
    );
    let mut request = client.post(endpoint).json(&body);
    if let Some(token) = &token {
        request = request.bearer_auth(token);
    }
    let response = tokio::select! {
        _=cancel.cancelled()=>bail!("Request canceled"),
        response=tokio::time::timeout(Duration::from_secs(config.network.header_timeout_seconds),request.send())=>response.context("Timed out waiting for response headers")??,
    };
    ensure_success(response.status(), response.headers())?;
    let mut chunks = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut turn = StreamTurn::new(profile.clone());
    let streamed: Result<()> = async { loop{
        let next=tokio::select!{
            _=cancel.cancelled()=>bail!("Request canceled; incomplete tool calls were not executed"),
            chunk=tokio::time::timeout(Duration::from_secs(config.network.chunk_timeout_seconds),chunks.next())=>chunk.context("Provider stream stalled")?,
        };
        let Some(chunk)=next else{break;};
        let chunk=chunk.context("Provider stream disconnected")?;
        for data in decoder.push(&chunk)?{
            if let Some(text)=turn.event(&data)?{tx.send(UiEvent::Text(text)).await.context("Terminal closed")?;}
            if turn.completed { break; }
        }
        if turn.completed { break; }
    } Ok(()) }.await;
    if let Err(error) = streamed {
        return Err(PartialFailure {
            text: turn.text.clone(),
            message: format!("{error:#}"),
        }
        .into());
    }
    let partial = turn.text.clone();
    let result = turn.finish().map_err(|error| PartialFailure {
        text: partial,
        message: format!("{error:#}"),
    })?;
    tx.send(UiEvent::TurnEnded)
        .await
        .context("Terminal closed")?;
    Ok(result)
}

fn ensure_success(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    let retry = headers
        .get("retry-after")
        .and_then(|h| h.to_str().ok())
        .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
        .map(|s| format!("; retry after {s}s"))
        .unwrap_or_default();
    let stage = match status.as_u16() {
        401 | 403 => "authentication or account permission; use /connect",
        429 => "rate limit or quota; retry explicitly later",
        400 | 404 | 422 => {
            "model, context capacity, endpoint, or request capability; check /model and config"
        }
        _ => "provider availability; retry explicitly",
    };
    bail!("Provider HTTP {status}: {stage}{retry}")
}

pub async fn discover(
    paths: &Paths,
    config: &Config,
    id: &str,
    profile: &Profile,
    cancel: &CancellationToken,
) -> Result<BTreeMap<String, ModelConfig>> {
    let token = auth::resolve(paths, profile, config, cancel).await?;
    let mut request = client(config)?.get(format!(
        "{}/models",
        profile.base_url().trim_end_matches('/')
    ));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = tokio::select! {_=cancel.cancelled()=>bail!("Model discovery canceled"),response=request.send()=>response?};
    ensure_success(response.status(), response.headers())?;
    let body: Value = response.json().await.context("Invalid model inventory")?;
    models_from_inventory(profile, &body, id)
}

fn models_from_inventory(
    profile: &Profile,
    body: &Value,
    id: &str,
) -> Result<BTreeMap<String, ModelConfig>> {
    let mut models = BTreeMap::new();
    let items = if profile.kind == ProviderKind::Chatgpt {
        body["models"].as_array()
    } else {
        body["data"].as_array()
    }
    .context("Endpoint has no supported model inventory; configure models manually")?;
    for item in items {
        if profile.kind == ProviderKind::Chatgpt
            && item["visibility"].as_str().is_some_and(|s| s != "list")
        {
            continue;
        }
        let model_id = if profile.kind == ProviderKind::Chatgpt {
            item["slug"].as_str()
        } else {
            item["id"].as_str()
        };
        let Some(model_id) = model_id.filter(|s| !s.is_empty()) else {
            continue;
        };
        let mut model = profile.models.get(model_id).cloned().unwrap_or_default();
        if model.reasoning_efforts.is_none()
            && let Some(levels) = item["supported_reasoning_levels"].as_array()
        {
            let mut supported = vec![];
            for item in levels {
                if let Some(level) = item["effort"].as_str().and_then(|v| v.parse().ok())
                    && !supported.contains(&level)
                {
                    supported.push(level);
                }
            }
            model.reasoning_efforts = Some(supported);
        }
        model.name = item["display_name"].as_str().unwrap_or(model_id).into();
        if profile.kind != ProviderKind::Compatible {
            model.supports_tools = Some(true);
        }
        models.insert(model_id.into(), model);
    }
    ensure!(
        !models.is_empty(),
        "No visible models were returned for {id}"
    );
    Ok(models)
}

#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    data: Vec<String>,
    event_bytes: usize,
}
impl SseDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        self.buffer.extend_from_slice(bytes);
        let mut result = vec![];
        while let Some(end) = self.buffer.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line).context("SSE event is not valid UTF-8")?;
            if line.is_empty() {
                if !self.data.is_empty() {
                    result.push(self.data.join("\n"));
                    self.data.clear();
                    self.event_bytes = 0;
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                let data = data.strip_prefix(' ').unwrap_or(data);
                self.event_bytes += data.len();
                ensure!(
                    self.event_bytes <= 1024 * 1024,
                    "SSE event exceeds 1 MiB limit"
                );
                self.data.push(data.into());
            }
        }
        ensure!(
            self.buffer.len() <= 1024 * 1024,
            "SSE line exceeds 1 MiB limit"
        );
        Ok(result)
    }
    pub fn finished(&self) -> bool {
        self.buffer.is_empty() && self.data.is_empty()
    }
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}
pub struct StreamTurn {
    profile: Profile,
    text: String,
    calls: BTreeMap<u64, PartialCall>,
    completed: bool,
    finish_reason: bool,
    final_turn: Option<Turn>,
    usage: Option<Value>,
}
impl StreamTurn {
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            text: String::new(),
            calls: BTreeMap::new(),
            completed: false,
            finish_reason: false,
            final_turn: None,
            usage: None,
        }
    }
    pub fn event(&mut self, data: &str) -> Result<Option<String>> {
        if data == "[DONE]" {
            ensure!(
                !self.profile.responses(),
                "Responses requires response.completed"
            );
            self.completed = true;
            return Ok(None);
        }
        let value: Value = serde_json::from_str(data).context("Invalid provider SSE JSON")?;
        if !value["error"].is_null() {
            bail!(
                "Provider returned a structured error; inspect account, quota, model, and context settings"
            );
        }
        if self.profile.responses() {
            match value["type"].as_str() {
                Some("response.output_text.delta") => {
                    let delta = value["delta"].as_str().context("Missing text delta")?;
                    self.text.push_str(delta);
                    return Ok(Some(delta.into()));
                }
                Some("response.completed") => {
                    ensure!(!self.completed, "Duplicate response.completed");
                    let response = &value["response"];
                    let items = response["output"]
                        .as_array()
                        .context("Completed response has no output array")?;
                    let mut text = String::new();
                    let mut calls = vec![];
                    for item in items {
                        match item["type"].as_str() {
                            Some("message") => {
                                if let Some(content) = item["content"].as_array() {
                                    for part in content {
                                        if let Some(t) = part["text"].as_str() {
                                            text.push_str(t);
                                        }
                                    }
                                }
                            }
                            Some("function_call") => calls.push(ToolCall {
                                id: item["call_id"].as_str().context("Missing call_id")?.into(),
                                name: normalize_name(
                                    item["name"].as_str().context("Missing tool name")?,
                                ),
                                arguments: item["arguments"]
                                    .as_str()
                                    .context("Missing arguments")?
                                    .into(),
                            }),
                            _ => {}
                        }
                    }
                    let delta = if self.text.is_empty() && !text.is_empty() {
                        Some(text.clone())
                    } else {
                        None
                    };
                    if text.is_empty() {
                        text = self.text.clone();
                    }
                    self.final_turn = Some(Turn {
                        text,
                        calls,
                        opaque: items.clone(),
                        route: Some(self.profile.route()),
                        usage: response.get("usage").cloned(),
                    });
                    self.completed = true;
                    return Ok(delta);
                }
                Some("response.failed" | "response.incomplete" | "error") => {
                    let code = value
                        .pointer("/response/error/code")
                        .and_then(Value::as_str)
                        .unwrap_or("incomplete_or_failed");
                    let code: String = code
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .take(100)
                        .collect();
                    bail!("Response failed ({code}); no incomplete tool calls were executed");
                }
                _ => {}
            }
        } else {
            if let Some(usage) = value.get("usage").filter(|v| !v.is_null()) {
                self.usage = Some(usage.clone());
            }
            if let Some(choices) = value["choices"].as_array() {
                for choice in choices {
                    if choice["index"].as_u64().unwrap_or(0) != 0 {
                        continue;
                    }
                    let delta = &choice["delta"];
                    if let Some(calls) = delta["tool_calls"].as_array() {
                        for call in calls {
                            let index =
                                call["index"].as_u64().context("Missing tool call index")?;
                            let entry = self.calls.entry(index).or_default();
                            if let Some(id) = call["id"].as_str() {
                                if entry.id.is_empty() {
                                    entry.id = id.into();
                                } else {
                                    ensure!(entry.id == id, "Tool call changed ID during stream");
                                }
                            }
                            if let Some(name) = call["function"]["name"].as_str() {
                                entry.name.push_str(name);
                            }
                            if let Some(arguments) = call["function"]["arguments"].as_str() {
                                entry.arguments.push_str(arguments);
                                ensure!(
                                    entry.arguments.len() <= 1024 * 1024,
                                    "Tool arguments exceed limit"
                                );
                            }
                        }
                    }
                    if let Some(reason) = choice["finish_reason"].as_str() {
                        ensure!(
                            matches!(reason, "stop" | "tool_calls"),
                            "Provider response incomplete: {reason}"
                        );
                        self.finish_reason = true;
                    }
                    if let Some(text) = delta["content"].as_str() {
                        self.text.push_str(text);
                        return Ok(Some(text.into()));
                    }
                }
            }
        }
        Ok(None)
    }
    pub fn finish(self) -> Result<Turn> {
        ensure!(
            self.completed,
            "Stream ended without successful terminal completion; incomplete tool calls were not executed"
        );
        let result = if self.profile.responses() {
            self.final_turn.context("Missing final response")?
        } else {
            ensure!(
                self.finish_reason,
                "Chat completion ended without finish_reason"
            );
            Turn {
                text: self.text,
                calls: self
                    .calls
                    .into_values()
                    .map(|c| ToolCall {
                        id: c.id,
                        name: normalize_name(&c.name),
                        arguments: c.arguments,
                    })
                    .collect(),
                opaque: vec![],
                route: Some(self.profile.route()),
                usage: self.usage,
            }
        };
        let mut ids = BTreeSet::new();
        for call in &result.calls {
            ensure!(
                !call.id.is_empty() && !call.name.is_empty() && ids.insert(&call.id),
                "Invalid or duplicate tool call identity"
            );
            let args: Value = serde_json::from_str(&call.arguments)
                .context("Incomplete or invalid tool arguments")?;
            ensure!(args.is_object(), "Tool arguments must be an object");
        }
        Ok(result)
    }
}
fn normalize_name(name: &str) -> String {
    name.strip_prefix("ruvx.")
        .or_else(|| name.strip_prefix("kovr."))
        .unwrap_or(name)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::AuthConfig, model::Mode, tools};
    fn compatible() -> Profile {
        Profile {
            kind: ProviderKind::Compatible,
            label: String::new(),
            base_url: Some("http://127.0.0.1:1/v1".into()),
            auth: AuthConfig::None,
            models: BTreeMap::from([(
                "m".into(),
                ModelConfig {
                    supports_tools: Some(true),
                    ..Default::default()
                },
            )]),
        }
    }
    #[test]
    fn byte_split_utf8_sse() {
        let source = "data: {\"text\":\"你好🙂\"}\r\n\r\n";
        let mut decoder = SseDecoder::default();
        let mut events = vec![];
        for byte in source.as_bytes() {
            events.extend(decoder.push(&[*byte]).unwrap());
        }
        assert_eq!(
            serde_json::from_str::<Value>(&events[0]).unwrap()["text"],
            "你好🙂"
        );
        assert!(decoder.finished());
    }
    #[test]
    fn fragmented_tool_arguments_and_late_failure() {
        let mut turn = StreamTurn::new(compatible());
        turn.event(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}).to_string()).unwrap();
        turn.event(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"file\"}"}}]},"finish_reason":"tool_calls"}]}).to_string()).unwrap();
        turn.event("[DONE]").unwrap();
        assert_eq!(
            turn.finish().unwrap().calls[0].arguments,
            "{\"path\":\"file\"}"
        );
        let mut broken = StreamTurn::new(compatible());
        assert!(
            broken
                .event("{\"error\":{\"message\":\"late failure\"}}")
                .is_err()
        );
        assert!(broken.finish().is_err());
    }
    #[test]
    fn subscription_payload_and_completion_gate() {
        let mut profile = compatible();
        profile.kind = ProviderKind::Chatgpt;
        profile.base_url = None;
        profile.auth = AuthConfig::Chatgpt {
            credential: "x".into(),
        };
        let body = build_request(
            &profile,
            "m",
            &[Message::user("hi")],
            "instructions",
            &tools::definitions(Mode::Plan),
        )
        .unwrap();
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"][0]["type"], "namespace");
        assert_eq!(body["tools"][0]["name"], "ruvx");
        assert!(body.get("max_output_tokens").is_none());
        let mut turn = StreamTurn::new(profile);
        turn.event("{\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}")
            .unwrap();
        assert!(turn.finish().is_err());
    }
    #[test]
    fn renamed_tool_namespace_preserves_legacy_opaque_history() {
        assert_eq!(normalize_name("ruvx.read_file"), "read_file");
        assert_eq!(normalize_name("kovr.read_file"), "read_file");
        assert_eq!(normalize_name("read_file"), "read_file");
        let mut profile = compatible();
        profile.kind = ProviderKind::Chatgpt;
        profile.base_url = None;
        profile.auth = AuthConfig::Chatgpt {
            credential: "x".into(),
        };
        let opaque = json!({"type":"function_call","namespace":"kovr","name":"read_file","call_id":"legacy-call","arguments":"{}"});
        let history = vec![Message::Assistant {
            turn: Turn {
                opaque: vec![opaque.clone()],
                route: Some(profile.route()),
                ..Default::default()
            },
        }];
        let body = build_request(
            &profile,
            "m",
            &history,
            "instructions",
            &tools::definitions(Mode::Plan),
        )
        .unwrap();
        assert_eq!(body["tools"][0]["name"], "kovr");
        assert_eq!(body["input"][0], opaque);
    }
    #[test]
    fn effort_wire_fields_match_protocol_and_default_omits_parameters() {
        use crate::effort::ReasoningEffort as E;
        for kind in [
            ProviderKind::Openai,
            ProviderKind::Chatgpt,
            ProviderKind::Compatible,
        ] {
            let mut profile = compatible();
            profile.kind = kind;
            let body =
                build_request(&profile, "m", &[Message::user("hi")], "instructions", &[]).unwrap();
            assert!(body.get("reasoning").is_none());
            assert!(body.get("reasoning_effort").is_none());
            let model = profile.models.get_mut("m").unwrap();
            model.reasoning_efforts = Some(vec![E::None, E::Low, E::High]);
            model.reasoning_effort = Some(E::High);
            let body = build_request(&profile, "m", &[], "instructions", &[]).unwrap();
            if profile.responses() {
                assert_eq!(body["reasoning"]["effort"], "high");
                assert!(body.get("reasoning_effort").is_none());
            } else {
                assert_eq!(body["reasoning_effort"], "high");
                assert!(body.get("reasoning").is_none());
            }
            profile.models.get_mut("m").unwrap().reasoning_effort = Some(E::None);
            let body = build_request(&profile, "m", &[], "instructions", &[]).unwrap();
            assert_eq!(
                if profile.responses() {
                    &body["reasoning"]["effort"]
                } else {
                    &body["reasoning_effort"]
                },
                "none"
            );
            profile.models.get_mut("m").unwrap().reasoning_effort = Some(E::Max);
            assert!(build_request(&profile, "m", &[], "instructions", &[]).is_err());
        }
    }
    #[test]
    fn inventory_learns_supported_levels_without_changing_user_preference() {
        use crate::effort::ReasoningEffort as E;
        let mut profile = compatible();
        profile.kind = ProviderKind::Chatgpt;
        let body = json!({"models":[{"slug":"m","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"high"},{"effort":"high"},{"effort":"ultra"}],"default_reasoning_level":"high"}]});
        let models = models_from_inventory(&profile, &body, "test").unwrap();
        assert_eq!(
            models["m"].reasoning_efforts.as_deref(),
            Some(&[E::Low, E::High][..])
        );
        assert_eq!(models["m"].reasoning_effort, None);
        let model = profile.models.get_mut("m").unwrap();
        model.reasoning_efforts = Some(vec![E::Low, E::Medium, E::High]);
        model.reasoning_effort = Some(E::Medium);
        let models = models_from_inventory(&profile, &body, "test").unwrap();
        assert_eq!(models["m"].reasoning_effort, Some(E::Medium));
        assert!(
            models["m"]
                .reasoning_efforts
                .as_ref()
                .unwrap()
                .contains(&E::Medium)
        );
    }
}
