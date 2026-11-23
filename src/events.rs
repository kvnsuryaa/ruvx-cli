use crate::{
    config::{ModelConfig, Profile},
    session::Session,
};
use std::collections::BTreeMap;
use tokio::sync::oneshot;

pub enum UiEvent {
    FileSuggestions {
        request: u64,
        result: Result<Vec<String>, String>,
    },
    Text(String),
    TurnEnded,
    Line(String),
    Usage(serde_json::Value),
    PreflightFinished {
        session: Box<Session>,
        revision: u64,
        result: Result<String, String>,
    },
    Tool {
        id: String,
        name: String,
        status: String,
        detail: String,
    },
    ClipboardRequest(String),
    ClipboardFinished(Result<Option<String>, String>),
    Status(String),
    /// Counters for the active run. None means complete token usage is unavailable.
    RunProgress {
        turns: u32,
        total_tokens: Option<u64>,
    },
    Approval {
        review: String,
        respond: oneshot::Sender<bool>,
    },
    RunFinished {
        session: Box<Session>,
        error: Option<String>,
    },
    ConnectionFinished {
        id: String,
        profile: Profile,
        error: Option<String>,
    },
    ModelsFinished {
        id: String,
        result: Result<BTreeMap<String, ModelConfig>, String>,
    },
}
