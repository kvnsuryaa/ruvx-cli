use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use ruvx::{
    agent::{self, Action},
    auth,
    config::{AuthConfig, Config, ModelConfig, Paths, Profile, ProviderKind},
    events::UiEvent,
    model::{Message, Mode, Selection, Turn},
    session::{Session, hash},
    tools,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    sync::{Arc, Mutex},
};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct MockState {
    replies: Mutex<VecDeque<String>>,
    http_failures: Mutex<VecDeque<(StatusCode, Option<&'static str>)>>,
    header_delays: Mutex<VecDeque<std::time::Duration>>,
    requests: Mutex<Vec<Value>>,
    progress: Mutex<Vec<(u32, Option<u64>)>>,
    notices: Mutex<Vec<String>>,
}
async fn inference(
    State(state): State<Arc<MockState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    assert_eq!(headers.get("authorization").unwrap(), "Bearer fixture-key");
    state.requests.lock().unwrap().push(body);
    if let Some((status, retry_after)) = state.http_failures.lock().unwrap().pop_front() {
        let mut headers = HeaderMap::new();
        if let Some(value) = retry_after {
            headers.insert("retry-after", value.parse().unwrap());
        }
        return (status, headers, "fixture HTTP error").into_response();
    }
    let reply = match state.replies.lock().unwrap().pop_front() {
        Some(reply) => (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            reply,
        ),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("content-type", "text/plain")],
            "Unexpected request".into(),
        ),
    }
    .into_response();
    let delay = state.header_delays.lock().unwrap().pop_front();
    if let Some(delay) = delay {
        tokio::time::sleep(delay).await;
    }
    reply
}
struct Fixture {
    _temp: tempfile::TempDir,
    paths: Paths,
    root: std::path::PathBuf,
    config: Config,
    state: Arc<MockState>,
    server: JoinHandle<()>,
    kind: ProviderKind,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new(kind: ProviderKind) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(&temp.path().join("state"));
        let root = temp.path().join("work");
        fs::create_dir(&root).unwrap();
        auth::save_key(&paths, "fixture", "fixture-key").unwrap();
        let state = Arc::new(MockState::default());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = Router::new()
            .route("/v1/chat/completions", post(inference))
            .route("/v1/responses", post(inference))
            .with_state(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut config = Config::default();
        config.providers.insert(
            "mock".into(),
            Profile {
                kind,
                label: "mock".into(),
                base_url: Some(format!("http://127.0.0.1:{port}/v1")),
                auth: AuthConfig::Saved {
                    credential: "fixture".into(),
                },
                models: BTreeMap::from([(
                    "model".into(),
                    ModelConfig {
                        supports_tools: Some(true),
                        ..Default::default()
                    },
                )]),
            },
        );
        config.validate().unwrap();
        Self {
            _temp: temp,
            paths,
            root,
            config,
            state,
            server,
            kind,
        }
    }
    fn session(&self) -> Session {
        let mut session = Session::create(&self.paths, &self.root).unwrap();
        session
            .select(Selection {
                profile: "mock".into(),
                model: "model".into(),
            })
            .unwrap();
        session
    }
    fn reply(&self, text: &str, calls: Vec<(&str, &str, Value)>) {
        let usage = (self.kind != ProviderKind::Compatible)
            .then(|| json!({"input_tokens":10,"output_tokens":5}));
        self.reply_usage(text, calls, usage);
    }
    fn reply_usage(&self, text: &str, calls: Vec<(&str, &str, Value)>, usage: Option<Value>) {
        let mut reply = String::new();
        if self.kind == ProviderKind::Compatible {
            let tool_calls:Vec<_>=calls.iter().enumerate().map(|(i,(id,name,args))|json!({"index":i,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})).collect();
            let mut response = json!({"choices":[{"index":0,"delta":{"content":text,"tool_calls":tool_calls},"finish_reason":if calls.is_empty(){"stop"}else{"tool_calls"}}]});
            if let Some(usage) = usage {
                response["usage"] = usage;
            }
            reply.push_str(&format!("data: {response}\n\n"));
            reply.push_str("data: [DONE]\n\n");
        } else {
            let mut output = vec![];
            if !text.is_empty() {
                reply.push_str(&format!(
                    "data: {}\n\n",
                    json!({"type":"response.output_text.delta","delta":text})
                ));
                output.push(json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}));
            }
            for (id, name, args) in calls {
                output.push(json!({"type":"function_call","call_id":id,"name":name,"arguments":args.to_string()}));
            }
            let mut response = json!({"output":output});
            if let Some(usage) = usage {
                response["usage"] = usage;
            }
            reply.push_str(&format!(
                "data: {}\n\n",
                json!({"type":"response.completed","response":response})
            ));
        }
        self.state.replies.lock().unwrap().push_back(reply);
    }
    async fn run(
        &self,
        session: Session,
        action: Action,
        approvals: Vec<bool>,
    ) -> (Session, Option<String>) {
        let (tx, mut rx) = mpsc::channel(64);
        let config = self.config.clone();
        let paths = self.paths.clone();
        let task = tokio::spawn(agent::start(
            session,
            paths,
            config,
            action,
            tx,
            CancellationToken::new(),
        ));
        let mut approvals: VecDeque<_> = approvals.into();
        let mut finished = None;
        while let Some(event) = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
        {
            match event {
                UiEvent::RunProgress {
                    turns,
                    total_tokens,
                } => {
                    self.state
                        .progress
                        .lock()
                        .unwrap()
                        .push((turns, total_tokens));
                }
                UiEvent::Approval { respond, .. } => {
                    let _ = respond.send(approvals.pop_front().expect("Unexpected approval"));
                }
                UiEvent::RunFinished { session, error } => {
                    finished = Some((*session, error));
                    break;
                }
                UiEvent::Line(line) => {
                    self.state.notices.lock().unwrap().push(line.clone());
                    if let Some((name, _)) = line.trim_start().split_once(" · ") {
                        assert!(
                            !matches!(
                                name,
                                "read_file"
                                    | "list_files"
                                    | "search_text"
                                    | "apply_patch"
                                    | "run_shell"
                            ),
                            "Tool start duplicated the result in the transcript: {line}"
                        );
                    }
                }
                _ => {}
            }
        }
        task.await.unwrap();
        assert!(approvals.is_empty());
        finished.expect("Agent did not return its session")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn run_counters_accumulate_reported_usage_reset_and_preserve_unknown_totals() {
    for kind in [ProviderKind::Openai, ProviderKind::Compatible] {
        for missing in [None, Some(0), Some(1)] {
            let f = Fixture::new(kind).await;
            let first = (missing != Some(0))
                .then(|| json!({"total_tokens":100,"input_tokens":10,"output_tokens":5}));
            let second =
                (missing != Some(1)).then(|| json!({"prompt_tokens":12,"completion_tokens":3}));
            f.reply_usage("", vec![("files", "list_files", json!({}))], first);
            f.reply_usage("Done", vec![], second);
            let (session, error) = f
                .run(f.session(), Action::Message("Inspect".into()), vec![])
                .await;
            assert!(error.is_none(), "{error:?}");
            let first_total = (missing != Some(0)).then_some(100);
            let final_total = missing.is_none().then_some(115);
            assert_eq!(
                *f.state.progress.lock().unwrap(),
                vec![
                    (1, None),
                    (1, first_total),
                    (2, first_total),
                    (2, final_total)
                ]
            );
            f.state.progress.lock().unwrap().clear();
            f.reply_usage("Done again", vec![], Some(json!({"total_tokens":0})));
            let (_, error) = f
                .run(session, Action::Message("Another run".into()), vec![])
                .await;
            assert!(error.is_none(), "{error:?}");
            assert_eq!(
                *f.state.progress.lock().unwrap(),
                vec![(1, None), (1, Some(0))]
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn existing_repo_plan_build_approval_review_and_resume() {
    for kind in [ProviderKind::Compatible, ProviderKind::Openai] {
        let f = Fixture::new(kind).await;
        let original = "user's existing dirty line\nold\n";
        fs::write(f.root.join("app.txt"), original).unwrap();
        fs::write(
            f.root.join("AGENTS.md"),
            "Keep the user's dirty line intact.",
        )
        .unwrap();
        f.reply(
            "Inspect first",
            vec![
                ("read", "read_file", json!({"path":"app.txt"})),
                (
                    "forbidden",
                    "apply_patch",
                    json!({"path":"forbidden","operation":"add","content":"bad"}),
                ),
            ],
        );
        f.reply(
            "Plan: replace old while preserving the existing user line.",
            vec![],
        );
        let (mut session, error) = f
            .run(
                f.session(),
                Action::Message("Understand and plan the change".into()),
                vec![],
            )
            .await;
        assert!(error.is_none(), "{error:?}");
        assert_eq!(
            fs::read_to_string(f.root.join("app.txt")).unwrap(),
            original
        );
        assert!(!f.root.join("forbidden").exists());
        session.set_mode(Mode::Build).unwrap();
        f.reply("Apply the change",vec![("update","apply_patch",json!({"path":"app.txt","operation":"update","expected_hash":hash(original.as_bytes()),"old_text":"old","new_text":"new"})),("test-file","apply_patch",json!({"path":"test.sh","operation":"add","content":"grep -q new app.txt\n"}))]);
        f.reply(
            "Validate",
            vec![
                (
                    "deny",
                    "run_shell",
                    json!({"command":"touch should-not-exist"}),
                ),
                (
                    "approve",
                    "run_shell",
                    json!({"command":"sh test.sh","timeout_seconds":5}),
                ),
            ],
        );
        f.reply("Changed app.txt and added a validation script. The approved validation exited 0; the other command was denied.",vec![]);
        let (session, error) = f
            .run(
                session,
                Action::Message("Implement the plan".into()),
                vec![false, true],
            )
            .await;
        assert!(error.is_none(), "{error:?}");
        assert!(!f.root.join("should-not-exist").exists());
        assert_eq!(
            fs::read_to_string(f.root.join("app.txt")).unwrap(),
            "user's existing dirty line\nnew\n"
        );
        let diff = tools::diff(&session, None).unwrap();
        assert!(diff.contains("-old"));
        assert!(diff.contains("+new"));
        assert!(!diff.contains("+user's existing dirty line"));
        let history = session.history().unwrap();
        assert!(history.iter().any(
            |(_, m)| matches!(m,Message::Tool{content,..}if content.contains("not allowed in Plan"))
        ));
        assert!(history.iter().any(|(_,m)|matches!(m,Message::Tool{content,..}if content.contains("\"status\":\"denied\""))));
        let requests = f.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 5);
        assert!(
            requests[0]
                .to_string()
                .contains("Keep the user's dirty line")
        );
        drop(requests);
        let id = session.id.clone();
        let count = history.len();
        drop(session);
        let resumed = Session::resume(&f.paths, &id).unwrap();
        assert_eq!(resumed.mode, Mode::Build);
        assert_eq!(resumed.history().unwrap().len(), count);
        assert_eq!(f.state.requests.lock().unwrap().len(), 5);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn new_project_runs_without_git() {
    let f = Fixture::new(ProviderKind::Compatible).await;
    let mut session = f.session();
    f.reply("Plan a tiny runnable shell app and a check.", vec![]);
    let (s, error) = f
        .run(
            session,
            Action::Message("Plan a new small application".into()),
            vec![],
        )
        .await;
    assert!(error.is_none());
    session = s;
    assert_eq!(fs::read_dir(&f.root).unwrap().count(), 0);
    session.set_mode(Mode::Build).unwrap();
    f.reply(
        "Create application",
        vec![(
            "new-app",
            "apply_patch",
            json!({"path":"app.sh","operation":"add","content":"printf 'hello ruvx\\n'\n"}),
        )],
    );
    f.reply(
        "Run validation",
        vec![(
            "validate",
            "run_shell",
            json!({"command":"sh app.sh | grep -q 'hello ruvx'","timeout_seconds":5}),
        )],
    );
    f.reply(
        "app.sh is runnable with sh; the validation passed with exit 0.",
        vec![],
    );
    let (session, error) = f
        .run(session, Action::Message("Build it".into()), vec![true])
        .await;
    assert!(error.is_none(), "{error:?}");
    assert!(tools::diff(&session, None).unwrap().contains("+printf"));
    assert!(!f.root.join(".git").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn turn_limit_requires_explicit_continue() {
    let mut f = Fixture::new(ProviderKind::Compatible).await;
    f.config.agent.max_turns = 1;
    f.reply("", vec![("list", "list_files", json!({}))]);
    f.reply("Done", vec![]);
    let (session, error) = f
        .run(f.session(), Action::Message("Inspect".into()), vec![])
        .await;
    assert!(error.is_none());
    assert_eq!(session.stop_reason.as_deref(), Some("turn_limit"));
    assert_eq!(f.state.requests.lock().unwrap().len(), 1);
    let (session, error) = f.run(session, Action::Continue, vec![]).await;
    assert!(error.is_none());
    assert_eq!(session.stop_reason.as_deref(), Some("completed"));
    assert_eq!(f.state.requests.lock().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn full_context_does_not_trim_or_make_a_request() {
    let mut f = Fixture::new(ProviderKind::Compatible).await;
    let model = f
        .config
        .providers
        .get_mut("mock")
        .unwrap()
        .models
        .get_mut("model")
        .unwrap();
    model.context_limit = Some(100);
    model.output_reserve_tokens = 10;
    let (session, error) = f
        .run(
            f.session(),
            Action::Message("preserve this original message".repeat(20)),
            vec![],
        )
        .await;
    assert!(error.unwrap().contains("Nothing was trimmed"));
    assert_eq!(f.state.requests.lock().unwrap().len(), 0);
    assert_eq!(session.history().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_compaction_preserves_journal_and_recent_conversations() {
    let f = Fixture::new(ProviderKind::Compatible).await;
    let mut session = f.session();
    for n in 0..4 {
        session.user(&format!("user {n}")).unwrap();
        session
            .response(&Turn {
                text: format!("answer {n}"),
                ..Default::default()
            })
            .unwrap();
    }
    let original = fs::read(session.dir.join("events.jsonl")).unwrap();
    f.reply("Earlier goals and settled decisions.", vec![]);
    let (session, error) = f.run(session, Action::Compact, vec![]).await;
    assert!(error.is_none(), "{error:?}");
    assert!(
        fs::read(session.dir.join("events.jsonl"))
            .unwrap()
            .starts_with(&original)
    );
    let history = session.history().unwrap();
    assert_eq!(history.len(), 5);
    assert!(matches!(&history[0].1,Message::User{content}if content.contains("Earlier goals")));
    assert!(matches!(&history[1].1,Message::User{content}if content=="user 2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_terminal_event_never_executes_partial_tools() {
    let f = Fixture::new(ProviderKind::Openai).await;
    let mut session = f.session();
    session.set_mode(Mode::Build).unwrap();
    let event = json!({"type":"response.output_text.delta","delta":"partial answer"});
    let call = json!({"type":"response.output_item.added","item":{"type":"function_call","call_id":"bad","name":"apply_patch","arguments":"{\"path\":\"forbidden\",\"operation\":\"add\",\"content\":\"bad\"}"}});
    let fail = json!({"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}});
    f.state
        .replies
        .lock()
        .unwrap()
        .push_back(format!("data: {event}\n\ndata: {call}\n\ndata: {fail}\n\n"));
    let (session, error) = f
        .run(session, Action::Message("Build".into()), vec![])
        .await;
    assert!(error.unwrap().contains("usage_limit"));
    assert!(!f.root.join("forbidden").exists());
    assert!(session.pending_tools.is_empty());
    assert!(session.history().unwrap().iter().any(|(_,m)|matches!(m,Message::Assistant{turn}if turn.text.contains("partial answer")&&turn.calls.is_empty())));
}

#[tokio::test(flavor = "multi_thread")]
async fn overloaded_request_retries_same_turn_then_completes_without_replaying_tools() {
    for kind in [ProviderKind::Openai, ProviderKind::Compatible] {
        let mut f = Fixture::new(kind).await;
        f.config.network.retry_initial_delay_seconds = 1;
        let mut session = f.session();
        session.set_mode(Mode::Build).unwrap();
        // The initial request executes a tool once. Only the next inference is retried.
        f.reply(
            "",
            vec![(
                "write-once",
                "run_shell",
                json!({"command":"printf x >> marker"}),
            )],
        );
        let failure =
            json!({"error":{"code":"server_is_overloaded","type":"service_unavailable_error"}});
        f.state
            .replies
            .lock()
            .unwrap()
            .push_back(format!("data: {failure}\n\n"));
        f.reply("Recovered", vec![]);
        let (session, error) = f
            .run(session, Action::Message("Do the work".into()), vec![true])
            .await;
        assert!(error.is_none(), "{error:?}");
        assert_eq!(fs::read_to_string(f.root.join("marker")).unwrap(), "x");
        let requests = f.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1], requests[2]);
        let events = fs::read_to_string(session.dir.join("events.jsonl")).unwrap();
        assert_eq!(events.matches("\"kind\":\"turn.started\"").count(), 2);
        assert!(
            session
                .history()
                .unwrap()
                .iter()
                .any(|(_, m)| matches!(m, Message::Assistant { turn } if turn.text == "Recovered"))
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_retries_are_bounded_and_can_be_disabled() {
    for max_retries in [0, 2] {
        let mut f = Fixture::new(ProviderKind::Compatible).await;
        f.config.network.max_retries = max_retries;
        f.config.network.retry_initial_delay_seconds = 1;
        let failure = json!({"error":{"code":"server_is_overloaded"}});
        for _ in 0..=max_retries {
            f.state
                .replies
                .lock()
                .unwrap()
                .push_back(format!("data: {failure}\n\n"));
        }
        let (_, error) = f
            .run(f.session(), Action::Message("Inspect".into()), vec![])
            .await;
        let error = error.unwrap();
        assert!(error.contains("server_is_overloaded"));
        assert_eq!(
            f.state.requests.lock().unwrap().len(),
            max_retries as usize + 1
        );
        assert_eq!(f.state.notices.lock().unwrap().len(), max_retries as usize);
        if max_retries > 0 {
            assert!(error.contains("retries exhausted"), "{error}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn overload_after_partial_output_or_tools_is_never_retried() {
    for kind in [ProviderKind::Openai, ProviderKind::Compatible] {
        let events = if kind == ProviderKind::Openai {
            vec![
                json!({"type":"response.output_text.delta","delta":"Partial"}),
                json!({"type":"response.output_item.added","item":{"type":"function_call","name":"run_shell"}}),
                json!({"type":"response.reasoning_text.delta","delta":"hidden"}),
            ]
        } else {
            vec![
                json!({"choices":[{"delta":{"content":"Partial"}}]}),
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"run_shell","arguments":"{"}}]}}]}),
                json!({"choices":[{"delta":{"reasoning_content":"hidden"}}]}),
            ]
        };
        for event in events {
            let f = Fixture::new(kind).await;
            let failure = json!({"error":{"code":"server_is_overloaded"}});
            f.state
                .replies
                .lock()
                .unwrap()
                .push_back(format!("data: {event}\n\ndata: {failure}\n\n"));
            let (session, error) = f
                .run(f.session(), Action::Message("Inspect".into()), vec![])
                .await;
            assert!(error.unwrap().contains("server_is_overloaded"));
            assert_eq!(f.state.requests.lock().unwrap().len(), 1);
            assert!(f.state.notices.lock().unwrap().is_empty());
            assert!(session.pending_tools.is_empty());
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_http_retry_respects_limits_and_permanent_errors() {
    for status in [
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::BAD_GATEWAY,
        StatusCode::GATEWAY_TIMEOUT,
    ] {
        let mut f = Fixture::new(ProviderKind::Openai).await;
        f.config.network.retry_initial_delay_seconds = 1;
        f.state
            .http_failures
            .lock()
            .unwrap()
            .push_back((status, Some("0")));
        f.reply("Recovered", vec![]);
        let (_, error) = f
            .run(f.session(), Action::Message("Inspect".into()), vec![])
            .await;
        assert!(error.is_none(), "{error:?}");
        assert_eq!(f.state.requests.lock().unwrap().len(), 2);
    }
    for (status, retry_after) in [
        (StatusCode::UNAUTHORIZED, None),
        (StatusCode::TOO_MANY_REQUESTS, None),
        (StatusCode::SERVICE_UNAVAILABLE, Some("61")),
        (StatusCode::SERVICE_UNAVAILABLE, Some("invalid")),
    ] {
        let f = Fixture::new(ProviderKind::Compatible).await;
        f.state
            .http_failures
            .lock()
            .unwrap()
            .push_back((status, retry_after));
        let (_, error) = f
            .run(f.session(), Action::Message("Inspect".into()), vec![])
            .await;
        assert!(error.is_some());
        assert_eq!(f.state.requests.lock().unwrap().len(), 1);
        assert!(f.state.notices.lock().unwrap().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn request_and_header_timeouts_retry_identical_inference_before_output() {
    for kind in [ProviderKind::Openai, ProviderKind::Compatible] {
        for request_timeout in [false, true] {
            let mut f = Fixture::new(kind).await;
            f.config.network.retry_initial_delay_seconds = 1;
            f.config.network.header_timeout_seconds = if request_timeout { 5 } else { 1 };
            f.config.network.request_timeout_seconds = if request_timeout { 1 } else { 5 };
            f.state
                .header_delays
                .lock()
                .unwrap()
                .push_back(std::time::Duration::from_secs(2));
            f.reply("Stale reply", vec![]);
            f.reply("Recovered from timeout", vec![]);
            let (session, error) = f
                .run(f.session(), Action::Message("Inspect".into()), vec![])
                .await;
            assert!(
                error.is_none(),
                "{kind:?}, request timeout={request_timeout}: {error:?}"
            );
            let requests = f.state.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0], requests[1]);
            let notices = f.state.notices.lock().unwrap();
            assert_eq!(notices.len(), 1);
            assert!(notices[0].contains("timed out"), "{notices:?}");
            assert!(notices[0].contains("automatic retry 1/3 in 1s"));
            let history = session.history().unwrap();
            assert!(history.iter().any(|(_, m)| matches!(m, Message::Assistant { turn } if turn.text == "Recovered from timeout")));
            assert!(!history.iter().any(|(_, m)| matches!(m, Message::Assistant { turn } if turn.text.contains("Stale reply"))));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_retry_is_bounded_and_has_network_specific_guidance() {
    let mut f = Fixture::new(ProviderKind::Compatible).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    f.config.providers.get_mut("mock").unwrap().base_url = Some(format!("http://{address}/v1"));
    f.config.network.max_retries = 1;
    f.config.network.retry_initial_delay_seconds = 1;
    let (_, error) = f
        .run(f.session(), Action::Message("Inspect".into()), vec![])
        .await;
    let error = error.unwrap();
    assert!(error.contains("retries exhausted (1 retries)"), "{error}");
    assert!(error.contains("connection failed"), "{error}");
    assert!(error.contains("network connectivity"), "{error}");
    assert_eq!(f.state.notices.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_retry_wait_can_be_canceled_before_sending_another_request() {
    let mut f = Fixture::new(ProviderKind::Compatible).await;
    f.config.network.retry_initial_delay_seconds = 30;
    let failure = json!({"error":{"type":"service_unavailable_error"}});
    f.state
        .replies
        .lock()
        .unwrap()
        .push_back(format!("data: {failure}\n\n"));
    let (tx, mut rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let selection = Selection {
        profile: "mock".into(),
        model: "model".into(),
    };
    let history = vec![Message::user("Inspect")];
    let request = ruvx::provider::generate(
        &f.paths,
        &f.config,
        &selection,
        ruvx::provider::GenerateInput {
            history: &history,
            instructions: "test",
            tools: &[],
        },
        &tx,
        &cancel,
    );
    tokio::pin!(request);
    tokio::select! {
        result = &mut request => panic!("Request stopped before its retry wait: {result:?}"),
        notice = rx.recv() => assert!(matches!(notice, Some(UiEvent::Line(line)) if line.contains("retry 1/3 in 30s"))),
    }
    cancel.cancel();
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), request)
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("canceled during provider retry wait")
    );
    assert_eq!(f.state.requests.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn structured_stream_error_keeps_partial_text_and_reports_safe_diagnostic() {
    for kind in [ProviderKind::Openai, ProviderKind::Compatible] {
        let f = Fixture::new(kind).await;
        let mut session = f.session();
        session.set_mode(Mode::Build).unwrap();
        let partial = if kind == ProviderKind::Compatible {
            json!({"choices":[{"index":0,"delta":{"content":"Partial answer","tool_calls":[{"index":0,"id":"bad","function":{"name":"run_shell","arguments":"{\"command\":\"touch forbidden\"}"}}]}}]})
        } else {
            json!({"type":"response.output_text.delta","delta":"Partial answer"})
        };
        let failure = json!({"type":"error","error":{"type":"usage_limit_reached","message":"SECRET provider body"}});
        f.state
            .replies
            .lock()
            .unwrap()
            .push_back(format!("data: {partial}\n\ndata: {failure}\n\n"));
        let (session, error) = f
            .run(session, Action::Message("Inspect skills".into()), vec![])
            .await;
        let error = error.unwrap();
        assert!(error.contains("usage_limit_reached"), "{error}");
        assert!(error.contains("check account usage"), "{error}");
        assert!(!error.contains("SECRET"));
        assert!(!f.root.join("forbidden").exists());
        assert!(session.pending_tools.is_empty());
        assert!(session.history().unwrap().iter().any(|(_, m)| matches!(m, Message::Assistant { turn } if turn.text.contains("Partial answer") && turn.calls.is_empty())));
        assert_eq!(f.state.requests.lock().unwrap().len(), 1);
        let events = fs::read_to_string(session.dir.join("events.jsonl")).unwrap();
        assert!(events.contains("usage_limit_reached"));
        assert!(!events.contains("SECRET"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn new_nested_instructions_are_seen_before_mutation() {
    let f = Fixture::new(ProviderKind::Compatible).await;
    fs::create_dir(f.root.join("src")).unwrap();
    fs::write(
        f.root.join("src/AGENTS.md"),
        "Use the scoped convention before creating files here.",
    )
    .unwrap();
    let mut session = f.session();
    session.set_mode(Mode::Build).unwrap();
    f.reply(
        "",
        vec![(
            "first",
            "apply_patch",
            json!({"path":"src/new.txt","operation":"add","content":"scoped\n"}),
        )],
    );
    f.reply(
        "Now follow the scoped convention",
        vec![(
            "retry",
            "apply_patch",
            json!({"path":"src/new.txt","operation":"add","content":"scoped\n"}),
        )],
    );
    f.reply("Done", vec![]);
    let (session, error) = f
        .run(session, Action::Message("Create the file".into()), vec![])
        .await;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        fs::read_to_string(f.root.join("src/new.txt")).unwrap(),
        "scoped\n"
    );
    assert!(session.history().unwrap().iter().any(
        |(_, m)| matches!(m,Message::Tool{content,..}if content.contains("instructions_required"))
    ));
    let requests = f.state.requests.lock().unwrap();
    assert!(
        requests[1]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Use the scoped convention")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unlimited_setting_exceeds_default_turn_budget() {
    let mut f = Fixture::new(ProviderKind::Compatible).await;
    f.config.agent.max_turns = 0;
    for n in 0..31 {
        let id = format!("list-{n}");
        f.reply("", vec![(&id, "list_files", json!({}))]);
    }
    f.reply("Finished beyond the default budget", vec![]);
    let (session, error) = f
        .run(
            f.session(),
            Action::Message("Continue until done".into()),
            vec![],
        )
        .await;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(session.stop_reason.as_deref(), Some("completed"));
    assert_eq!(f.state.requests.lock().unwrap().len(), 32);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_compaction_keeps_active_context() {
    let f = Fixture::new(ProviderKind::Compatible).await;
    let mut session = f.session();
    for n in 0..4 {
        session.user(&format!("user {n}")).unwrap();
        session
            .response(&Turn {
                text: format!("answer {n}"),
                ..Default::default()
            })
            .unwrap();
    }
    f.reply("", vec![]);
    let (session, error) = f.run(session, Action::Compact, vec![]).await;
    assert!(error.is_some());
    assert!(session.summary.is_none());
    assert_eq!(session.context_after, 0);
    assert_eq!(session.history().unwrap().len(), 8);
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_compaction_normalizes_retained_opaque_context() {
    let f = Fixture::new(ProviderKind::Openai).await;
    let mut session = f.session();
    for n in 0..4 {
        session.user(&format!("user {n}")).unwrap();
        session
            .response(&Turn {
                text: format!("answer {n}"),
                opaque: vec![json!({"type":"reasoning","encrypted_content":"fixture-opaque"})],
                route: Some(f.config.providers["mock"].route()),
                ..Default::default()
            })
            .unwrap();
    }
    f.reply("Earlier decisions and progress", vec![]);
    let (session, error) = f.run(session, Action::Compact, vec![]).await;
    assert!(error.is_none(), "{error:?}");
    let history = session.history().unwrap();
    assert_eq!(history.len(), 5);
    assert!(
        history
            .iter()
            .all(|(_, m)| !matches!(m,Message::Assistant{turn}if ruvx::provider::has_opaque(turn)))
    );
    let id = session.id.clone();
    drop(session);
    let resumed = Session::resume(&f.paths, &id).unwrap();
    assert!(
        resumed
            .history()
            .unwrap()
            .iter()
            .all(|(_, m)| !matches!(m,Message::Assistant{turn}if ruvx::provider::has_opaque(turn)))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn effort_reaches_every_tool_turn_compaction_and_can_return_to_default() {
    use ruvx::effort::ReasoningEffort as E;
    for kind in [ProviderKind::Openai, ProviderKind::Compatible] {
        let mut f = Fixture::new(kind).await;
        let model = f
            .config
            .providers
            .get_mut("mock")
            .unwrap()
            .models
            .get_mut("model")
            .unwrap();
        model.reasoning_efforts = Some(vec![E::Low, E::High]);
        model.reasoning_effort = Some(E::High);
        f.reply("Inspect", vec![("list", "list_files", json!({}))]);
        f.reply("Done", vec![]);
        let (mut session, error) = f
            .run(
                f.session(),
                Action::Message("Inspect this workspace".into()),
                vec![],
            )
            .await;
        assert!(error.is_none(), "{error:?}");
        for n in 0..3 {
            session.user(&format!("Follow-up {n}")).unwrap();
            session
                .response(&Turn {
                    text: "answer".into(),
                    ..Default::default()
                })
                .unwrap();
        }
        f.config
            .providers
            .get_mut("mock")
            .unwrap()
            .models
            .get_mut("model")
            .unwrap()
            .reasoning_effort = Some(E::Low);
        f.reply("Summary", vec![]);
        let (session, error) = f.run(session, Action::Compact, vec![]).await;
        assert!(error.is_none(), "{error:?}");
        f.config
            .providers
            .get_mut("mock")
            .unwrap()
            .models
            .get_mut("model")
            .unwrap()
            .reasoning_effort = None;
        f.reply("Default response", vec![]);
        let (_session, error) = f
            .run(
                session,
                Action::Message("Continue with provider default".into()),
                vec![],
            )
            .await;
        assert!(error.is_none(), "{error:?}");
        let requests = f.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        for (request, expected) in
            requests
                .iter()
                .zip([Some("high"), Some("high"), Some("low"), None])
        {
            let value = if kind == ProviderKind::Compatible {
                request.get("reasoning_effort")
            } else {
                request.get("reasoning").and_then(|r| r.get("effort"))
            };
            assert_eq!(value.and_then(Value::as_str), expected);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn home_skills_are_passive_and_mcp_is_rejected_in_plan_for_both_providers() {
    for kind in [ProviderKind::Openai, ProviderKind::Compatible] {
        let mut f = Fixture::new(kind).await;
        f.config.bypass = true;
        f.config.mcp_servers.insert(
            "test".into(),
            ruvx::config::McpServerConfig {
                enabled: true,
                command: "/bin/sh".into(),
                args: vec!["-c".into(), "touch forbidden".into()],
                ..Default::default()
            },
        );
        let dir = f.paths.home.join(".agents/skills/demo");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "---\nname: demo\ndescription: A test skill\n---\nUse only the existing permission controls.\n").unwrap();
        f.reply("", vec![("s1", "list_skills", json!({}))]);
        f.reply("", vec![("s2", "read_skill", json!({"name":"demo"}))]);
        f.reply("", vec![("m1", "mcp_list_tools", json!({"server":"test"}))]);
        f.reply(
            "",
            vec![(
                "shell-plan",
                "run_shell",
                json!({"command":"touch forbidden-shell"}),
            )],
        );
        f.reply("Done", vec![]);
        let (session, error) = f
            .run(
                f.session(),
                Action::Message("Inspect skills".into()),
                vec![],
            )
            .await;
        assert!(error.is_none(), "{error:?}");
        assert!(!f.root.join("forbidden").exists());
        assert!(!f.root.join("forbidden-shell").exists());
        let history = session.history().unwrap();
        assert!(history.iter().any(|(_, m)| matches!(m, Message::Tool { name, content, .. } if name == "read_skill" && content.contains("existing permission controls"))));
        assert!(history.iter().any(|(_, m)| matches!(m, Message::Tool { name, content, .. } if name == "mcp_list_tools" && content.contains("not allowed in Plan"))));
        let requests = f.state.requests.lock().unwrap();
        assert!(requests[0].to_string().contains("A test skill"));
        let definitions = requests[0]["tools"].to_string();
        assert!(definitions.contains("list_skills"));
        assert!(!definitions.contains("mcp_list_tools"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn bypass_executes_shell_and_mcp_without_approval_then_ask_policy_returns() {
    for kind in [ProviderKind::Openai, ProviderKind::Compatible] {
        let mut f = Fixture::new(kind).await;
        f.config.bypass = true;
        f.config.mcp_servers.insert(
            "fixture".into(),
            ruvx::config::McpServerConfig {
                enabled: true,
                command: "/bin/sh".into(),
                args: vec!["-c".into(), include_str!("fixtures/mcp-stdio.sh").into()],
                ..Default::default()
            },
        );
        f.reply(
            "",
            vec![("list", "mcp_list_tools", json!({"server":"fixture"}))],
        );
        f.reply(
            "",
            vec![(
                "next",
                "mcp_list_tools",
                json!({"server":"fixture","cursor":"page2"}),
            )],
        );
        f.reply(
            "",
            vec![(
                "echo",
                "mcp_call_tool",
                json!({"server":"fixture","tool":"echo","arguments":{"text":"hello"}}),
            )],
        );
        f.reply(
            "",
            vec![(
                "shell",
                "run_shell",
                json!({"command":"printf executed > bypass.txt"}),
            )],
        );
        f.reply("Done", vec![]);
        let mut session = f.session();
        session.set_mode(Mode::Build).unwrap();
        let (session, error) = f
            .run(
                session,
                Action::Message("Execute with BYPASS".into()),
                vec![],
            )
            .await;
        assert!(error.is_none(), "{error:?}");
        assert_eq!(
            fs::read_to_string(f.root.join("bypass.txt")).unwrap(),
            "executed"
        );
        assert!(session.pending_tools.is_empty());
        let journal: Vec<Value> = fs::read_to_string(session.dir.join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let approvals: Vec<_> = journal
            .iter()
            .filter(|e| e["kind"] == "approval.resolved")
            .collect();
        assert_eq!(approvals.len(), 4);
        assert!(
            approvals
                .iter()
                .all(|e| e["payload"]["approved"] == true && e["payload"]["policy"] == "bypass")
        );
        let first_request = f.state.requests.lock().unwrap()[0].to_string();
        assert!(first_request.contains("Approval policy: BYPASS"));
        f.config.bypass = false;
        f.reply(
            "",
            vec![("ask", "run_shell", json!({"command":"touch must-not-run"}))],
        );
        f.reply("Denied", vec![]);
        let (_, error) = f
            .run(session, Action::Message("Ask again".into()), vec![false])
            .await;
        assert!(error.is_none(), "{error:?}");
        assert!(!f.root.join("must-not-run").exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn uncertain_mcp_stops_agent_and_blocks_replay() {
    for bypass in [false, true] {
        let mut f = Fixture::new(ProviderKind::Compatible).await;
        f.config.mcp_servers.insert(
            "test".into(),
            ruvx::config::McpServerConfig {
                enabled: true,
                command: "/bin/sh".into(),
                args: vec!["-c".into(), "exit 0".into()],
                ..Default::default()
            },
        );
        f.config.bypass = bypass;
        let mut session = f.session();
        session.set_mode(Mode::Build).unwrap();
        f.reply("", vec![("m1", "mcp_list_tools", json!({"server":"test"}))]);
        let (session, error) = f
            .run(
                session,
                Action::Message("List tools".into()),
                if bypass { vec![] } else { vec![true] },
            )
            .await;
        assert!(error.unwrap().contains("uncertain"));
        assert!(session.pending_tools.contains("m1"));
        let (_session, error) = f
            .run(session, Action::Message("Try again".into()), vec![])
            .await;
        assert!(error.unwrap().contains("uncertain outcomes"));
        assert_eq!(f.state.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_skill_metadata_and_paged_mcp_work_in_build_for_both_providers() {
    for kind in [ProviderKind::Openai, ProviderKind::Compatible] {
        let mut f = Fixture::new(kind).await;
        let skills = f.root.join("local skills/demo");
        fs::create_dir_all(skills.join("references")).unwrap();
        fs::write(skills.join("SKILL.md"), "---\nname: demo\ndescription: Use the fixture MCP echo tool\n---\nBODY_ONLY_AFTER_READ\n").unwrap();
        fs::write(
            skills.join("references/guide.md"),
            "RESOURCE_ONLY_AFTER_READ\n",
        )
        .unwrap();
        f.config.skills.paths.push(skills);
        f.config.mcp_servers.insert(
            "fixture".into(),
            ruvx::config::McpServerConfig {
                enabled: true,
                command: "/bin/sh".into(),
                args: vec!["-c".into(), include_str!("fixtures/mcp-stdio.sh").into()],
                ..Default::default()
            },
        );
        f.reply("", vec![("skill", "read_skill", json!({"name":"demo"}))]);
        f.reply(
            "",
            vec![(
                "resource",
                "read_skill",
                json!({"name":"demo","file":"references/guide.md"}),
            )],
        );
        f.reply(
            "",
            vec![("list", "mcp_list_tools", json!({"server":"fixture"}))],
        );
        f.reply(
            "",
            vec![(
                "next",
                "mcp_list_tools",
                json!({"server":"fixture","cursor":"page2"}),
            )],
        );
        f.reply(
            "",
            vec![(
                "echo",
                "mcp_call_tool",
                json!({"server":"fixture","tool":"echo","arguments":{"text":"hello"}}),
            )],
        );
        f.reply("Done", vec![]);
        let mut session = f.session();
        session.set_mode(Mode::Build).unwrap();
        let (session, error) = f
            .run(
                session,
                Action::Message("Use the available skill and MCP".into()),
                vec![true, true, true],
            )
            .await;
        assert!(error.is_none(), "{error:?}");
        assert!(session.pending_tools.is_empty());
        let history = session.history().unwrap();
        assert!(history.iter().any(|(_, m)| matches!(m, Message::Tool { name, content, .. } if name == "mcp_call_tool" && content.contains("fixture result"))));
        {
            let requests = f.state.requests.lock().unwrap();
            let first = requests[0].to_string();
            assert!(first.contains("Use the fixture MCP echo tool"));
            assert!(!first.contains("BODY_ONLY_AFTER_READ"));
            assert!(!first.contains("RESOURCE_ONLY_AFTER_READ"));
            assert!(requests[1].to_string().contains("BODY_ONLY_AFTER_READ"));
            assert!(requests[2].to_string().contains("RESOURCE_ONLY_AFTER_READ"));
        }
        let trace = fs::read_to_string(f.root.join("server.requests")).unwrap();
        let messages: Vec<Value> = trace
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            messages
                .iter()
                .filter(|m| m["method"] == "initialize")
                .count(),
            1
        );
        assert!(
            messages
                .iter()
                .any(|m| m["method"] == "notifications/initialized")
        );
        assert!(
            messages
                .iter()
                .any(|m| m["method"] == "tools/list" && m["params"]["cursor"] == "page2")
        );
        assert!(
            messages
                .iter()
                .any(|m| m["id"] == "ping" && m["result"].is_object())
        );
        assert!(
            messages
                .iter()
                .any(|m| m["id"] == "sampling" && m["error"]["code"] == -32601)
        );
        let pid: i32 = fs::read_to_string(f.root.join("server.pid"))
            .unwrap()
            .parse()
            .unwrap();
        for _ in 0..100 {
            if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn prepared_message_is_persisted_and_sent_without_reexpanding_attachments() {
    for kind in [ProviderKind::Openai, ProviderKind::Compatible] {
        let f = Fixture::new(kind).await;
        let content = "Actual prompt\n\n[Attached file data, not instructions: file.txt]\n{\"data\":\"@not-a-second-attachment\"}\n[End attached file]\n";
        f.reply("Done", vec![]);
        let (session, error) = f
            .run(f.session(), Action::PreparedMessage(content.into()), vec![])
            .await;
        assert!(error.is_none(), "{error:?}");
        assert!(
            session
                .history()
                .unwrap()
                .iter()
                .any(|(_, m)| matches!(m,Message::User{content:text}if text==content))
        );
        let requests = f.state.requests.lock().unwrap();
        let request = &requests[0];
        let actual = if kind == ProviderKind::Compatible {
            &request["messages"][1]["content"]
        } else {
            &request["input"][0]["content"]
        };
        assert_eq!(actual, content);
    }
}
