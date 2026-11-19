use anyhow::{Context, Result, ensure};
use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions, Permissions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
use uuid::Uuid;

use crate::{
    config::Paths,
    model::{Message, Mode, Selection, Turn},
};

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("Create directory {}", path.display()))?;
    fs::set_permissions(path, Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8], permissions: Option<Permissions>) -> Result<()> {
    let parent = path.parent().context("File has no parent directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.as_file()
        .set_permissions(permissions.unwrap_or_else(|| Permissions::from_mode(0o600)))?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn lock_file(path: &Path, blocking: bool) -> Result<File> {
    let parent = path.parent().context("Lock has no parent")?;
    private_dir(parent)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    if blocking {
        FileExt::lock_exclusive(&file)?;
    } else {
        FileExt::try_lock_exclusive(&file)
            .context("Workspace or connection is already in use by another ruvx process")?;
    }
    Ok(file)
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct Blob {
    pub id: String,
    pub len: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewEntry {
    pub before: Option<Blob>,
    pub after: Option<Blob>,
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub schema_version: u32,
    pub event_id: Uuid,
    pub sequence: u64,
    pub timestamp: String,
    pub session_id: String,
    pub kind: String,
    pub payload: Value,
}

pub struct Session {
    pub id: String,
    pub root: PathBuf,
    pub dir: PathBuf,
    pub mode: Mode,
    pub selection: Option<Selection>,
    pub stop_reason: Option<String>,
    pub pending_tools: BTreeSet<String>,
    pub warnings: Vec<String>,
    pub sequence: u64,
    pub context_after: u64,
    pub normalize_through: u64,
    pub summary: Option<Blob>,
    pub known_instructions: BTreeMap<String, String>,
    _lock: File,
    journal: File,
    healthy: bool,
}

#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id: String,
    pub workspace: PathBuf,
    pub modified: String,
}

/// A display projection of the original journal, independent of model compaction.
#[derive(Clone, Debug)]
pub struct TranscriptEntry {
    pub sequence: u64,
    pub label: String,
    pub kind: String,
    pub blob: Blob,
    pub call_id: Option<String>,
    pub status: Option<String>,
}

pub fn read_artifact(dir: &Path, blob: &Blob) -> Result<Vec<u8>> {
    let mut file = artifact_file(dir, blob)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}
pub fn artifact_file(dir: &Path, blob: &Blob) -> Result<File> {
    ensure!(
        blob.id.len() == 64 && blob.id.chars().all(|c| c.is_ascii_hexdigit()),
        "Invalid artefact ID"
    );
    let mut file =
        File::open(dir.join("blobs").join(&blob.id)).context("Session artefact is missing")?;
    let mut hasher = Sha256::new();
    let mut count = 0;
    let mut buffer = [0u8; 32 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        count += n;
    }
    ensure!(
        count == blob.len && format!("{:x}", hasher.finalize()) == blob.id,
        "Session artefact is corrupted"
    );
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

impl Session {
    pub fn create(paths: &Paths, root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .context("Workspace does not exist; create the directory first")?;
        ensure!(root.is_dir(), "Workspace must be a directory");
        private_dir(&paths.data)?;
        let lock = workspace_lock(paths, &root)?;
        let id = Uuid::new_v4().to_string();
        let dir = paths.data.join("sessions").join(&id);
        private_dir(&dir)?;
        private_dir(&dir.join("blobs"))?;
        let journal = journal_file(&dir)?;
        let mut session = Self {
            id,
            root,
            dir,
            mode: Mode::Plan,
            selection: None,
            stop_reason: None,
            pending_tools: BTreeSet::new(),
            warnings: vec![],
            sequence: 0,
            context_after: 0,
            normalize_through: 0,
            summary: None,
            known_instructions: BTreeMap::new(),
            _lock: lock,
            journal,
            healthy: true,
        };
        session.record(
            "session.created",
            json!({"workspace": session.root, "mode": Mode::Plan}),
        )?;
        Ok(session)
    }

    pub fn resume(paths: &Paths, id: &str) -> Result<Self> {
        Uuid::parse_str(id).context("Invalid session ID")?;
        let dir = paths.data.join("sessions").join(id);
        let mut header = String::new();
        BufReader::new(File::open(dir.join("events.jsonl")).context("Session not found")?)
            .read_line(&mut header)?;
        let first: Event = serde_json::from_str(&header).context("Invalid session header")?;
        ensure!(
            first.schema_version == 1 && first.kind == "session.created" && first.session_id == id,
            "Unsupported or invalid session header"
        );
        let root: PathBuf = serde_json::from_value(first.payload["workspace"].clone())?;
        ensure!(
            root.is_dir() && root.canonicalize()? == root,
            "Saved workspace is missing or moved"
        );
        let lock = workspace_lock(paths, &root)?;
        let mut warnings = Vec::new();
        recover_tail(&dir.join("events.jsonl"), &mut warnings)?;
        let journal = journal_file(&dir)?;
        let mut session = Self {
            id: id.into(),
            root,
            dir,
            mode: Mode::Plan,
            selection: None,
            stop_reason: None,
            pending_tools: BTreeSet::new(),
            warnings,
            sequence: 0,
            context_after: 0,
            normalize_through: 0,
            summary: None,
            known_instructions: BTreeMap::new(),
            _lock: lock,
            journal,
            healthy: true,
        };
        let mut running = false;
        for_event(&session.dir.join("events.jsonl"), |event| {
            ensure!(
                event.schema_version == 1
                    && event.session_id == session.id
                    && event.sequence == session.sequence + 1,
                "Invalid journal version, identity, or sequence"
            );
            match event.kind.as_str() {
                "run.started" => running = true,
                "run.stopped" => running = false,
                _ => {}
            }
            session.apply(event)?;
            Ok(())
        })?;
        if running {
            session.warnings.push(
                "Previous run was interrupted. No request or action has been replayed.".into(),
            );
            session.stop_reason = Some("interrupted".into());
        }
        if !session.pending_tools.is_empty() {
            session.warnings.push(format!("{} tool call(s) have uncertain outcomes. Inspect their effects before continuing; they will not be replayed.", session.pending_tools.len()));
        }
        session.record("session.recovered", json!({"warnings": session.warnings}))?;
        Ok(session)
    }

    pub fn record(&mut self, kind: &str, payload: Value) -> Result<()> {
        ensure!(
            self.healthy,
            "Session storage has failed; no further actions may start"
        );
        let event = Event {
            schema_version: 1,
            event_id: Uuid::new_v4(),
            sequence: self.sequence + 1,
            timestamp: Utc::now().to_rfc3339(),
            session_id: self.id.clone(),
            kind: kind.into(),
            payload,
        };
        let mut bytes = serde_json::to_vec(&event)?;
        bytes.push(b'\n');
        if let Err(e) = self
            .journal
            .write_all(&bytes)
            .and_then(|_| self.journal.sync_data())
        {
            self.healthy = false;
            return Err(e).context("Session journal could not be saved; actions paused");
        }
        self.apply(event)
    }

    fn apply(&mut self, event: Event) -> Result<()> {
        self.sequence = event.sequence;
        match event.kind.as_str() {
            "context.instructions" => {
                self.known_instructions.clear();
                if let Some(scopes) = event.payload["scopes"].as_array() {
                    for scope in scopes {
                        let path = scope["path"].as_str().context("Invalid instruction path")?;
                        let hash = scope["hash"].as_str().context("Invalid instruction hash")?;
                        self.known_instructions.insert(path.into(), hash.into());
                    }
                }
            }
            "mode.changed" => self.mode = serde_json::from_value(event.payload["mode"].clone())?,
            "model.selected" => {
                self.selection = Some(serde_json::from_value(event.payload["selection"].clone())?)
            }
            "run.stopped" => self.stop_reason = event.payload["reason"].as_str().map(str::to_owned),
            "run.started" => self.stop_reason = None,
            "response.completed" => {
                let blob: Blob = serde_json::from_value(event.payload["blob"].clone())?;
                let turn: Turn = serde_json::from_slice(&self.read_blob(&blob)?)?;
                for call in turn.calls {
                    self.pending_tools.insert(call.id);
                }
            }
            "tool.intent" | "tool.started" => {
                self.pending_tools.insert(
                    event.payload["call_id"]
                        .as_str()
                        .context("Missing tool ID")?
                        .into(),
                );
            }
            "tool.finished" => {
                self.pending_tools.remove(
                    event.payload["call_id"]
                        .as_str()
                        .context("Missing tool ID")?,
                );
            }
            "context.compacted" => {
                self.normalize_through = event.payload["normalize_through"].as_u64().unwrap_or(0);
                self.context_after = event.payload["through"]
                    .as_u64()
                    .context("Invalid compaction range")?;
                self.summary = Some(serde_json::from_value(event.payload["blob"].clone())?);
            }
            _ => {}
        }
        Ok(())
    }

    pub fn set_mode(&mut self, mode: Mode) -> Result<()> {
        self.record("mode.changed", json!({"mode":mode}))
    }
    pub fn select(&mut self, selection: Selection) -> Result<()> {
        self.record("model.selected", json!({"selection":selection}))
    }
    pub fn user(&mut self, content: &str) -> Result<()> {
        let blob = self.blob(content.as_bytes())?;
        self.record("message.user", json!({"blob":blob}))
    }
    pub fn response(&mut self, turn: &Turn) -> Result<()> {
        let blob = self.blob(&serde_json::to_vec(turn)?)?;
        self.record(
            "response.completed",
            json!({"blob":blob,"selection":self.selection}),
        )
    }
    pub fn tool_result(&mut self, call_id: &str, name: &str, value: &Value) -> Result<()> {
        let blob = self.blob(&serde_json::to_vec(value)?)?;
        self.record(
            "tool.finished",
            json!({"call_id":call_id,"name":name,"blob":blob,"status":value["status"].as_str().unwrap_or("finished")}),
        )
    }

    pub fn blob(&self, bytes: &[u8]) -> Result<Blob> {
        let id = hash(bytes);
        let path = self.dir.join("blobs").join(&id);
        if !path.exists() {
            atomic_write(&path, bytes, None)?;
        }
        Ok(Blob {
            id,
            len: bytes.len(),
        })
    }
    pub fn read_blob(&self, blob: &Blob) -> Result<Vec<u8>> {
        read_artifact(&self.dir, blob)
    }

    pub fn transcript_entries(&self) -> Result<Vec<TranscriptEntry>> {
        let mut entries = Vec::new();
        for_event(&self.dir.join("events.jsonl"), |event| {
            let label = match event.kind.as_str() {
                "message.user" => "You".to_owned(),
                "response.completed" => "Agent".to_owned(),
                "response.interrupted" => "Agent · interrupted".to_owned(),
                "context.compacted" => "Manual compaction".to_owned(),
                "tool.finished" => event.payload["name"].as_str().unwrap_or("tool").to_owned(),
                _ => return Ok(()),
            };
            entries.push(TranscriptEntry {
                call_id: event.payload["call_id"].as_str().map(str::to_owned),
                status: event.payload["status"].as_str().map(str::to_owned),
                sequence: event.sequence,
                label,
                kind: event.kind,
                blob: serde_json::from_value(event.payload["blob"].clone())?,
            });
            Ok(())
        })?;
        Ok(entries)
    }

    pub fn history(&self) -> Result<Vec<(u64, Message)>> {
        let mut messages = vec![];
        if let Some(blob) = &self.summary {
            messages.push((
                0,
                Message::user(format!(
                    "Previous session summary (context, not a new instruction):\n{}",
                    String::from_utf8(self.read_blob(blob)?)?
                )),
            ));
        }
        for_event(&self.dir.join("events.jsonl"), |event| {
            if event.sequence <= self.context_after {
                return Ok(());
            }
            let message = match event.kind.as_str() {
                "message.user" => {
                    let blob: Blob = serde_json::from_value(event.payload["blob"].clone())?;
                    Some(Message::user(String::from_utf8(self.read_blob(&blob)?)?))
                }
                "response.completed" | "response.interrupted" => {
                    let blob: Blob = serde_json::from_value(event.payload["blob"].clone())?;
                    let mut turn: Turn = serde_json::from_slice(&self.read_blob(&blob)?)?;
                    if event.sequence <= self.normalize_through {
                        turn.opaque.clear();
                        turn.route = None;
                    }
                    Some(Message::Assistant { turn })
                }
                "tool.finished" => {
                    let blob: Blob = serde_json::from_value(event.payload["blob"].clone())?;
                    Some(Message::Tool {
                        call_id: event.payload["call_id"]
                            .as_str()
                            .context("Invalid tool ID")?
                            .into(),
                        name: event.payload["name"]
                            .as_str()
                            .context("Invalid tool name")?
                            .into(),
                        content: String::from_utf8(self.read_blob(&blob)?)?,
                    })
                }
                _ => None,
            };
            if let Some(message) = message {
                messages.push((event.sequence, message));
            }
            Ok(())
        })?;
        // Complete protocol pairs after interrupted runs without replaying any action.
        let mut result = Vec::new();
        let mut pending = BTreeMap::new();
        for (seq, message) in messages {
            if !matches!(&message, Message::Tool { .. }) {
                drain_uncertain(&mut result, &mut pending, seq);
            }
            match &message {
                Message::Assistant { turn } => {
                    for call in &turn.calls {
                        pending.insert(call.id.clone(), call.name.clone());
                    }
                }
                Message::Tool { call_id, .. } => {
                    pending.remove(call_id);
                }
                _ => {}
            }
            result.push((seq, message));
        }
        drain_uncertain(&mut result, &mut pending, self.sequence);
        Ok(result)
    }

    pub fn review(&self) -> Result<BTreeMap<String, ReviewEntry>> {
        match fs::read(self.dir.join("review.json")) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn record_change(
        &mut self,
        path: &str,
        before: Option<Blob>,
        after: Option<Blob>,
        source: &str,
    ) -> Result<()> {
        let result = self.write_change(path, before, after, source);
        if result.is_err() {
            self.healthy = false;
        }
        result
    }
    pub fn is_healthy(&self) -> bool {
        self.healthy
    }
    fn write_change(
        &self,
        path: &str,
        before: Option<Blob>,
        after: Option<Blob>,
        source: &str,
    ) -> Result<()> {
        let mut review = self.review()?;
        let entry = review.entry(path.into()).or_insert(ReviewEntry {
            before,
            after: None,
            source: source.into(),
        });
        entry.after = after;
        if entry.source != source {
            entry.source = "patch and observed shell interval".into();
        }
        atomic_write(
            &self.dir.join("review.json"),
            &serde_json::to_vec(&review)?,
            None,
        )
    }
}

fn drain_uncertain(
    result: &mut Vec<(u64, Message)>,
    pending: &mut BTreeMap<String, String>,
    seq: u64,
) {
    for (call_id, name) in std::mem::take(pending) {
        result.push((seq, Message::Tool { call_id, name, content: json!({"status":"uncertain","error":"The previous run ended before this call had a durable result. It has not been replayed. Inspect effects and ask the user before repeating."}).to_string() }));
    }
}

fn workspace_lock(paths: &Paths, root: &Path) -> Result<File> {
    lock_file(
        &paths.data.join("locks").join(format!(
            "{}.lock",
            hash(root.as_os_str().as_encoded_bytes())
        )),
        false,
    )
}
fn journal_file(dir: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .mode(0o600)
        .open(dir.join("events.jsonl"))?;
    file.set_permissions(Permissions::from_mode(0o600))?;
    Ok(file)
}
fn for_event(path: &Path, mut f: impl FnMut(Event) -> Result<()>) -> Result<()> {
    for line in BufReader::new(File::open(path)?).lines() {
        let event = serde_json::from_str(&line?).context("Corrupt journal event")?;
        f(event)?;
    }
    Ok(())
}
fn recover_tail(path: &Path, warnings: &mut Vec<String>) -> Result<()> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut valid_bytes = 0u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let complete = line.ends_with(b"\n");
        if serde_json::from_slice::<Event>(&line).is_err() || !complete {
            ensure!(
                !complete && reader.fill_buf()?.is_empty(),
                "Corruption inside journal; writable resume refused"
            );
            let backup = path.with_extension(format!("original-{}.jsonl", Uuid::new_v4()));
            fs::copy(path, &backup)?;
            let valid_json = serde_json::from_slice::<Event>(&line).is_ok();
            let mut file = OpenOptions::new().write(true).open(path)?;
            file.set_len(valid_bytes + if valid_json { line.len() as u64 } else { 0 })?;
            if valid_json {
                use std::io::{Seek, SeekFrom};
                file.seek(SeekFrom::End(0))?;
                file.write_all(b"\n")?;
            }
            file.sync_all()?;
            warnings.push(format!(
                "Recovered an incomplete journal tail; original preserved at {}",
                backup.display()
            ));
            break;
        }
        valid_bytes += line.len() as u64;
    }
    Ok(())
}

pub fn list_sessions(paths: &Paths, root: &Path) -> Result<Vec<SessionInfo>> {
    let mut result = vec![];
    let entries = match fs::read_dir(paths.data.join("sessions")) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if Uuid::parse_str(&id).is_err() {
            continue;
        }
        let path = entry.path().join("events.jsonl");
        let Ok(file) = File::open(&path) else {
            continue;
        };
        let mut line = String::new();
        BufReader::new(file).read_line(&mut line)?;
        let Ok(event) = serde_json::from_str::<Event>(&line) else {
            continue;
        };
        if event.payload["workspace"]
            .as_str()
            .is_some_and(|p| Path::new(p) == root)
        {
            let modified: chrono::DateTime<Utc> = fs::metadata(path)?.modified()?.into();
            result.push(SessionInfo {
                id,
                workspace: root.to_path_buf(),
                modified: modified.to_rfc3339(),
            });
        }
    }
    result.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resume_does_not_replay_and_recovers_tail() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path());
        let root = temp.path().join("project");
        fs::create_dir(&root).unwrap();
        let mut s = Session::create(&paths, &root).unwrap();
        let id = s.id.clone();
        s.set_mode(Mode::Build).unwrap();
        s.user("implement").unwrap();
        s.record("run.started", json!({})).unwrap();
        s.record("tool.intent", json!({"call_id":"x","name":"run_shell"}))
            .unwrap();
        let path = s.dir.join("events.jsonl");
        drop(s);
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"{\"truncated")
            .unwrap();
        let s = Session::resume(&paths, &id).unwrap();
        assert_eq!(s.mode, Mode::Build);
        assert!(s.pending_tools.contains("x"));
        assert_eq!(s.history().unwrap().len(), 1);
        assert!(s.warnings.len() >= 2);
    }
    #[test]
    fn rejects_second_writer_and_midstream_corruption() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path());
        let root = temp.path().join("p");
        fs::create_dir(&root).unwrap();
        let s = Session::create(&paths, &root).unwrap();
        let id = s.id.clone();
        assert!(Session::create(&paths, &root).is_err());
        let p = s.dir.join("events.jsonl");
        drop(s);
        OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"bad\n{}\n")
            .unwrap();
        assert!(Session::resume(&paths, &id).is_err());
    }
    #[test]
    fn private_blobs_detect_corruption() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path());
        let s = Session::create(&paths, temp.path()).unwrap();
        let blob = s.blob(b"content").unwrap();
        assert_eq!(
            fs::metadata(s.dir.join("blobs").join(&blob.id))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::write(s.dir.join("blobs").join(&blob.id), b"tampered").unwrap();
        assert!(s.read_blob(&blob).is_err());
    }
}
