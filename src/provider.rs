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

#[derive(Debug)]
struct TransientFailure {
    message: String,
    notice: &'static str,
    retry_after: Option<Duration>,
}
impl std::fmt::Display for TransientFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for TransientFailure {}

fn transient_identifier(identifier: &str) -> bool {
    matches!(
        identifier,
        "server_is_overloaded"
            | "overloaded_error"
            | "server_error"
            | "internal_server_error"
            | "service_unavailable"
            | "service_unavailable_error"
    )
}

fn retry_delay(config: &Config, retry: u32, retry_after: Option<Duration>) -> Duration {
    let backoff = Duration::from_secs(
        config
            .network
            .retry_initial_delay_seconds
            .saturating_mul(1u64 << retry.min(10))
            .min(60),
    );
    backoff.max(retry_after.unwrap_or_default())
}

fn retryable_connection_cause(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut source = Some(error);
    while let Some(error) = source {
        if error.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::NotConnected
            )
        }) {
            return true;
        }
        source = error.source();
    }
    false
}

fn send_failure(error: reqwest::Error) -> anyhow::Error {
    let timed_out = error.is_timeout();
    if timed_out
        || ((error.is_connect() || error.is_request()) && retryable_connection_cause(&error))
    {
        let notice = if timed_out {
            "Provider connection timed out"
        } else {
            "Provider connection failed"
        };
        return TransientFailure {
            message: format!("{notice} before a response was received; check network connectivity and provider endpoint availability"),
            notice,
            retry_after: None,
        }.into();
    }
    // Builder, TLS certificate, and other unrecognized failures need inspection.
    error.into()
}

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
    for retry in 0..=config.network.max_retries {
        // The serialized body and credentials are identical on every attempt.
        let attempt = request
            .try_clone()
            .context("Provider request cannot be retried")?;
        let error = match generate_once(attempt, profile, config, tx, cancel).await {
            Ok(turn) => return Ok(turn),
            Err(error) => error,
        };
        let Some(failure) = error.downcast_ref::<TransientFailure>() else {
            return Err(error);
        };
        if retry == config.network.max_retries {
            return Err(error.context(format!(
                "Automatic provider retries exhausted ({retry} retries)"
            )));
        }
        let delay = retry_delay(config, retry, failure.retry_after);
        // A longer Retry-After requires an explicit retry, rather than an early
        // request or an unexpectedly long automatic wait.
        if delay > Duration::from_secs(60) {
            return Err(error
                .context("Provider requested a wait longer than the automatic retry limit (60s)"));
        }
        tx.send(UiEvent::Line(format!(
            "{}; automatic retry {}/{} in {}s. Ctrl+C cancels.",
            failure.notice,
            retry + 1,
            config.network.max_retries,
            delay.as_secs()
        )))
        .await
        .context("Terminal closed")?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("Request canceled during provider retry wait"),
            _ = tokio::time::sleep(delay) => {}
        }
    }
    unreachable!("Every provider attempt either succeeds or returns its final error")
}

async fn generate_once(
    request: reqwest::RequestBuilder,
    profile: &Profile,
    config: &Config,
    tx: &mpsc::Sender<UiEvent>,
    cancel: &CancellationToken,
) -> Result<Turn> {
    let response = tokio::select! {
        biased;
        _=cancel.cancelled()=>bail!("Request canceled"),
        response=tokio::time::timeout(Duration::from_secs(config.network.header_timeout_seconds),request.send())=>match response {
            Ok(response) => response.map_err(send_failure)?,
            Err(_) => return Err(TransientFailure {
                message: "Provider timed out waiting for response headers; check network connectivity and provider endpoint availability".into(),
                notice: "Provider timed out waiting for response headers",
                retry_after: None,
            }.into()),
        },
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
        if !turn.output_started && error.is::<TransientFailure>() {
            return Err(error);
        }
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
    let message = format!("Provider HTTP {status}: {stage}{retry}");
    if matches!(status.as_u16(), 500 | 502 | 503 | 504) {
        let retry_after = match headers.get("retry-after") {
            Some(header) => match header.to_str().ok().and_then(|s| s.parse::<u64>().ok()) {
                Some(seconds) => Some(Duration::from_secs(seconds)),
                // Do not retry early when the provider uses an unsupported hint.
                None => bail!(
                    "{message}; automatic retry skipped because Retry-After is not a number of seconds"
                ),
            },
            None => None,
        };
        return Err(TransientFailure {
            message,
            notice: "Provider temporarily unavailable",
            retry_after,
        }
        .into());
    }
    bail!("{message}")
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

fn error_identifier(value: &Value) -> Option<&str> {
    // Error messages can echo credentials or input. Only display short protocol
    // identifiers verbatim; never interpolate the provider's message/body.
    value.as_str().filter(|code| {
        !code.is_empty()
            && code.len() <= 100
            && code
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
    })
}

fn stream_error(value: &Value) -> anyhow::Error {
    let error = value
        .get("error")
        .filter(|e| !e.is_null())
        .or_else(|| value.pointer("/response/error").filter(|e| !e.is_null()))
        .unwrap_or(value);
    let code = error_identifier(&error["code"]);
    let kind = error_identifier(&error["type"])
        .filter(|kind| !matches!(*kind, "error" | "response.failed" | "response.incomplete"));
    let reason = value
        .pointer("/response/incomplete_details/reason")
        .and_then(error_identifier);
    let mut identifiers = vec![];
    for (name, identifier) in [("code", code), ("type", kind), ("reason", reason)] {
        if let Some(identifier) = identifier {
            identifiers.push(format!("{name}={identifier}"));
        }
    }
    let action = [code, kind, reason].into_iter().flatten().find_map(|identifier| {
        match identifier {
            "context_length_exceeded" | "context_window_exceeded" | "max_context_length_exceeded" =>
                Some("Context capacity exceeded; use /compact or /new, or select a model with a larger context using /model"),
            "usage_limit_reached" | "rate_limit_exceeded" | "insufficient_quota" | "quota_exceeded" | "subscription_sharing_usage_limit_exceeded" =>
                Some("Provider usage limit or quota reached; check account usage and retry explicitly after the limit resets or quota is restored"),
            "invalid_api_key" | "authentication_error" | "token_expired" | "invalid_token" | "unauthorized" | "authentication_required" =>
                Some("Provider authentication failed; use /connect to reconnect the selected profile"),
            "permission_denied" | "access_denied" | "forbidden" | "model_access_denied" =>
                Some("Provider access denied; check account permissions with /connect and model access with /model"),
            "model_not_found" | "unsupported_model" | "invalid_model" | "unsupported_value" | "unsupported_parameter" | "invalid_request_error" =>
                Some("Provider rejected the model or request settings; check /model and model capability configuration"),
            "server_is_overloaded" | "overloaded_error" =>
                Some("Provider temporarily unavailable due to server overload; retry explicitly later"),
            "server_error" | "internal_server_error" | "service_unavailable" | "service_unavailable_error" =>
                Some("Provider temporarily unavailable; retry explicitly later"),
            "max_output_tokens" => Some("Response reached the output token limit; request a shorter response or check the model output limit"),
            "content_filter" => Some("Response stopped by the provider content filter; review the request before retrying"),
            _ => None,
        }
    }).unwrap_or("Check provider account, /model, and context settings; retry explicitly after resolving the error");
    let detail = if identifiers.is_empty() {
        "unspecified".into()
    } else {
        identifiers.join(", ")
    };
    let message =
        format!("Provider error ({detail}): {action}; no incomplete tool calls were executed");
    if code.or(kind).is_some_and(transient_identifier) {
        TransientFailure {
            message,
            notice: "Provider temporarily unavailable",
            retry_after: None,
        }
        .into()
    } else {
        anyhow::anyhow!(message)
    }
}

pub struct StreamTurn {
    profile: Profile,
    text: String,
    calls: BTreeMap<u64, PartialCall>,
    completed: bool,
    finish_reason: bool,
    final_turn: Option<Turn>,
    usage: Option<Value>,
    output_started: bool,
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
            output_started: false,
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
        if !value["error"].is_null()
            || matches!(
                value["type"].as_str(),
                Some("response.failed" | "response.incomplete" | "error")
            )
        {
            return Err(stream_error(&value));
        }
        // Lifecycle events are safe to retry. Once any output, tool arguments,
        // reasoning or usage arrives, preserve the failure for explicit recovery.
        if self.profile.responses() {
            self.output_started |= !matches!(
                value["type"].as_str(),
                Some("response.created" | "response.in_progress")
            );
        } else {
            self.output_started |= value.get("usage").is_some_and(|v| !v.is_null())
                || value["choices"].as_array().is_some_and(|choices| {
                    choices.iter().any(|choice| {
                        !choice["finish_reason"].is_null()
                            || choice["delta"].as_object().is_some_and(|delta| {
                                delta.iter().any(|(name, v)| {
                                    name != "role"
                                        && !v.is_null()
                                        && v != ""
                                        && !v.as_array().is_some_and(Vec::is_empty)
                                        && !v.as_object().is_some_and(serde_json::Map::is_empty)
                                })
                            })
                    })
                });
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
    fn transient_io_classification_includes_os_timeouts_but_not_permanent_failures() {
        // os error 60 is ETIMEDOUT on macOS, the failure reported by the user.
        #[cfg(target_os = "macos")]
        assert!(retryable_connection_cause(
            &std::io::Error::from_raw_os_error(60)
        ));
        for kind in [
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::ConnectionRefused,
            std::io::ErrorKind::ConnectionReset,
        ] {
            assert!(retryable_connection_cause(&std::io::Error::from(kind)));
        }
        for kind in [
            std::io::ErrorKind::InvalidInput,
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::PermissionDenied,
        ] {
            assert!(!retryable_connection_cause(&std::io::Error::from(kind)));
        }
    }

    #[tokio::test]
    async fn connection_refused_before_response_is_a_retryable_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let config = Config::default();
        let request = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("http://{address}/responses"))
            .json(&json!({}));
        let (tx, _rx) = mpsc::channel(8);
        let error = generate_once(
            request,
            &compatible(),
            &config,
            &tx,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(error.is::<TransientFailure>(), "{error:#}");
        assert!(error.to_string().contains("connection failed"));
    }

    #[tokio::test]
    async fn connection_lost_after_partial_text_is_not_retryable() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(socket);
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            let mut socket = reader.into_inner();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 99999\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"Partial reply\"}}]}\n\n").await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let request = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}/responses"));
        let (tx, _rx) = mpsc::channel(8);
        let error = generate_once(
            request,
            &compatible(),
            &Config::default(),
            &tx,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(!error.is::<TransientFailure>());
        assert_eq!(
            error.downcast_ref::<PartialFailure>().unwrap().text,
            "Partial reply"
        );
        server.await.unwrap();
    }

    #[test]
    fn retry_backoff_is_bounded_and_respects_retry_after() {
        let config = Config::default();
        let delays: Vec<_> = (0..7)
            .map(|retry| retry_delay(&config, retry, None).as_secs())
            .collect();
        assert_eq!(delays, [2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(
            retry_delay(&config, 0, Some(Duration::from_secs(9))).as_secs(),
            9
        );
        assert_eq!(
            retry_delay(&config, 0, Some(Duration::from_secs(61))).as_secs(),
            61
        );
        for status in [500, 502, 503, 504] {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("retry-after", "7".parse().unwrap());
            let error = ensure_success(reqwest::StatusCode::from_u16(status).unwrap(), &headers)
                .unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<TransientFailure>()
                    .unwrap()
                    .retry_after,
                Some(Duration::from_secs(7))
            );
            headers.insert("retry-after", "unsupported".parse().unwrap());
            assert!(
                !ensure_success(reqwest::StatusCode::from_u16(status).unwrap(), &headers)
                    .unwrap_err()
                    .is::<TransientFailure>()
            );
        }
        for status in [400, 401, 403, 404, 422, 429] {
            assert!(
                !ensure_success(
                    reqwest::StatusCode::from_u16(status).unwrap(),
                    &Default::default()
                )
                .unwrap_err()
                .is::<TransientFailure>()
            );
        }
        for code in [
            "invalid_api_key",
            "usage_limit_reached",
            "context_length_exceeded",
            "unknown",
        ] {
            let error =
                stream_error(&json!({"error":{"code":code,"type":"service_unavailable_error"}}));
            assert!(!error.is::<TransientFailure>(), "{code}");
        }
    }

    #[test]
    fn streaming_output_disables_automatic_retry_including_hidden_reasoning_and_tools() {
        for kind in [ProviderKind::Openai, ProviderKind::Compatible] {
            let mut profile = compatible();
            profile.kind = kind;
            let lifecycle = if profile.responses() {
                json!({"type":"response.created"})
            } else {
                json!({"choices":[{"delta":{"role":"assistant","content":""}}]})
            };
            let mut turn = StreamTurn::new(profile.clone());
            turn.event(&lifecycle.to_string()).unwrap();
            assert!(!turn.output_started);
            let events = if profile.responses() {
                vec![
                    json!({"type":"response.reasoning_text.delta","delta":"hidden"}),
                    json!({"type":"response.output_item.added","item":{"type":"function_call"}}),
                ]
            } else {
                vec![
                    json!({"choices":[{"delta":{"reasoning_content":"hidden"}}]}),
                    json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"read_file","arguments":"{"}}]}}]}),
                ]
            };
            for event in events {
                let mut turn = StreamTurn::new(profile.clone());
                turn.event(&event.to_string()).unwrap();
                assert!(turn.output_started);
            }
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
    fn structured_errors_preserve_identifiers_and_action_without_message_contents() {
        let cases = [
            (
                json!({"error":{"type":"usage_limit_reached","message":"SECRET provider body"}}),
                "usage_limit_reached",
                "usage",
            ),
            (
                json!({"error":{"code":"context_length_exceeded","type":"invalid_request_error","message":"SECRET provider body"}}),
                "context_length_exceeded",
                "/compact",
            ),
            (
                json!({"type":"error","code":"invalid_api_key","message":"SECRET provider body"}),
                "invalid_api_key",
                "/connect",
            ),
            (
                json!({"type":"response.failed","response":{"error":{"code":"model_not_found","message":"SECRET provider body"}}}),
                "model_not_found",
                "/model",
            ),
            (
                json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
                "max_output_tokens",
                "output",
            ),
        ];
        for kind in [
            ProviderKind::Openai,
            ProviderKind::Chatgpt,
            ProviderKind::Compatible,
        ] {
            for (event, code, action) in &cases {
                let mut profile = compatible();
                profile.kind = kind;
                let mut turn = StreamTurn::new(profile);
                let error = turn.event(&event.to_string()).unwrap_err().to_string();
                assert!(error.contains(code), "{kind:?}: {error}");
                assert!(error.contains(action), "{kind:?}: {error}");
                assert!(!error.contains("SECRET"), "{error}");
                assert!(turn.finish().is_err());
            }
        }
    }
    #[test]
    fn overloaded_provider_reports_temporary_unavailability() {
        for kind in [
            ProviderKind::Openai,
            ProviderKind::Chatgpt,
            ProviderKind::Compatible,
        ] {
            for error in [
                json!({"code":"server_is_overloaded","type":"service_unavailable_error"}),
                json!({"code":"server_is_overloaded"}),
                json!({"type":"service_unavailable_error"}),
            ] {
                let mut profile = compatible();
                profile.kind = kind;
                let mut turn = StreamTurn::new(profile);
                let message = turn
                    .event(&json!({"error":error}).to_string())
                    .unwrap_err()
                    .to_string();
                assert!(message.contains("temporarily unavailable"), "{message}");
                assert!(message.contains("retry explicitly later"), "{message}");
                assert!(!message.contains("Check provider account"), "{message}");
                assert!(turn.finish().is_err());
            }
        }
    }
    #[test]
    fn structured_error_diagnostics_reject_unsafe_identifiers_and_raw_messages() {
        for identifier in [
            "sk-secret-token",
            "bad\n\u{1b}[2J",
            "SECRET",
            &"x".repeat(101),
        ] {
            let mut turn = StreamTurn::new(compatible());
            let event = json!({"error":{"code":identifier,"type":"rate_limit_exceeded","message":"Bearer SECRET provider body"}});
            let error = turn.event(&event.to_string()).unwrap_err().to_string();
            assert!(error.contains("type=rate_limit_exceeded"));
            assert!(!error.contains("code="), "{error}");
            assert!(!error.contains("SECRET"));
            assert!(!error.contains('\u{1b}'));
        }
        let mut turn = StreamTurn::new(compatible());
        let error = turn
            .event(&json!({"error":{"message":"SECRET"}}).to_string())
            .unwrap_err()
            .to_string();
        assert!(error.contains("unspecified"));
        assert!(!error.contains("SECRET"));
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
