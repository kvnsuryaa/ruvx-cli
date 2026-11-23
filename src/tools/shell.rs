use super::files::{clip, record_snapshot_changes, resolve_path, snapshot};
use crate::{
    config::Config,
    events::UiEvent,
    model::ToolCall,
    session::{Session, private_dir},
};
use anyhow::{Context, Result, ensure};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::{Pid, setsid},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    os::unix::{fs::OpenOptionsExt, process::ExitStatusExt},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShellArgs {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_seconds: Option<u64>,
}

pub async fn run(
    session: &mut Session,
    config: &Config,
    call: &ToolCall,
    args: ShellArgs,
    tx: &mpsc::Sender<UiEvent>,
    cancel: &CancellationToken,
) -> Result<Value> {
    ensure!(
        !args.command.trim().is_empty() && !args.command.contains('\0'),
        "Invalid shell command"
    );
    let cwd = resolve_path(&session.root, args.cwd.as_deref().unwrap_or(""), false)?;
    ensure!(cwd.is_dir(), "Shell cwd must be a directory");
    let timeout = args.timeout_seconds.unwrap_or(config.shell.timeout_seconds);
    ensure!(
        timeout > 0 && timeout <= config.shell.max_timeout_seconds,
        "Shell timeout exceeds configured limits"
    );
    let _ = tx
        .send(UiEvent::Status(
            "Preparing workspace review snapshot…".into(),
        ))
        .await;
    let before = tokio::task::block_in_place(|| snapshot(session, &config.tools))?;
    ensure!(!cancel.is_cancelled(), "Shell canceled before approval");
    let review = format!(
        "Shell approval · run once or deny\nCommand:\n{}\nWorking directory: {}\nTimeout: {timeout}s\nReview: {} text files; {} binary/oversized files excluded. Ignore/dependency/build paths and outside-workspace effects are excluded.\nShell uses your OS permissions.\n",
        args.command,
        cwd.display(),
        before.files.len(),
        before.skipped
    );
    let approved = if config.bypass {
        !cancel.is_cancelled()
    } else {
        let (respond, answer) = oneshot::channel();
        tx.send(UiEvent::Approval { review, respond })
            .await
            .context("Terminal closed before approval")?;
        tokio::select! { _=cancel.cancelled()=>false, answer=answer=>answer.unwrap_or(false) }
    };
    session.record("approval.resolved",json!({"call_id":call.id,"approved":approved,"policy":if config.bypass {"bypass"} else {"ask"},"command":args.command,"cwd":cwd,"timeout_seconds":timeout}))?;
    if !approved {
        return Ok(
            json!({"status":if cancel.is_cancelled(){"canceled"}else{"denied"},"message":"Command was not executed"}),
        );
    }
    let current = tokio::task::block_in_place(|| snapshot(session, &config.tools))?;
    ensure!(
        current == before,
        "Workspace changed while awaiting approval; prepare a new call and approval"
    );
    ensure!(!cancel.is_cancelled(), "Shell canceled before spawn");
    let logs = session.dir.join("artifacts");
    private_dir(&logs)?;
    let prefix = Uuid::new_v4().to_string();
    let stdout_path = logs.join(format!("{prefix}-stdout.log"));
    let stderr_path = logs.join(format!("{prefix}-stderr.log"));
    let open_log = |path: &std::path::Path| {
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
    };
    let stdout_file = open_log(&stdout_path)?;
    let stderr_file = open_log(&stderr_path)?;
    session.record(
        "tool.intent",
        json!({"call_id":call.id,"name":call.name,"arguments":args}),
    )?;
    session.record("tool.started",json!({"call_id":call.id,"name":call.name,"command":args.command,"cwd":cwd,"timeout_seconds":timeout}))?;
    let started = Instant::now();
    let mut command = Command::new(&config.shell.executable);
    command
        .arg("-c")
        .arg(&args.command)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("GIT_TERMINAL_PROMPT", "0");
    // A separate process group still shares the parent's controlling terminal.
    // setsid also detaches /dev/tty and creates the group used by killpg below.
    // SAFETY: the post-fork hook only calls the async-signal-safe setsid syscall
    // and converts errno; it does not allocate, lock, or access shared state.
    unsafe {
        command.pre_exec(|| setsid().map(|_| ()).map_err(std::io::Error::from));
    }
    let mut child = command.spawn().context("Could not start shell")?;
    let pid = child.id().context("Shell process has no PID")?;
    let mut process_guard = ProcessGroupGuard { pid, armed: true };
    let out_task = capture_task(
        child.stdout.take().context("Missing stdout")?,
        stdout_file,
        config.tools.result_max_bytes / 2,
        tx.clone(),
        "stdout",
        cancel.clone(),
    );
    let err_task = capture_task(
        child.stderr.take().context("Missing stderr")?,
        stderr_file,
        config.tools.result_max_bytes / 2,
        tx.clone(),
        "stderr",
        cancel.clone(),
    );
    let mut reason = "exited";
    let status = tokio::select! {
        status=child.wait()=>status?,
        _=cancel.cancelled()=>{reason="canceled";terminate(&mut child,pid).await?},
        _=tokio::time::sleep(Duration::from_secs(timeout))=>{reason="timed_out";terminate(&mut child,pid).await?},
    };
    // Background processes are not supported: close any descendants still holding pipes.
    let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGTERM);
    tokio::time::sleep(Duration::from_millis(25)).await;
    let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    process_guard.armed = false;
    let out = tokio::time::timeout(Duration::from_secs(3), out_task)
        .await
        .context("stdout reader did not finish")???;
    let err = tokio::time::timeout(Duration::from_secs(3), err_task)
        .await
        .context("stderr reader did not finish")???;
    let after = tokio::task::block_in_place(|| snapshot(session, &config.tools))?;
    tokio::task::block_in_place(|| record_snapshot_changes(session, &before, &after))?;
    let stderr = err.text();
    let auth_required = !status.success() && stderr.contains("terminal prompts disabled");
    let mut result = json!({"status":if reason!="exited"{reason}else if status.success(){"ok"}else{"failed"},"exit_code":status.code(),"signal":status.signal(),"stdout":out.text(),"stderr":stderr,"truncated":out.truncated()||err.truncated(),"stdout_bytes":out.total,"stderr_bytes":err.total,"artifacts":[stdout_path,stderr_path],"duration_ms":started.elapsed().as_millis(),"review_note":"Changes observed during command interval; ignored/build/binary/oversized/outside-workspace files excluded."});
    if auth_required {
        result["auth_hint"] = json!(
            "Git authentication is required. Authenticate Git in a separate terminal, then retry explicitly. Existing credential helpers and SSH agents remain available."
        );
    }
    Ok(result)
}

async fn terminate(
    child: &mut tokio::process::Child,
    pid: u32,
) -> Result<std::process::ExitStatus> {
    let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGTERM);
    match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(status) => Ok(status?),
        Err(_) => {
            let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
            Ok(child.wait().await?)
        }
    }
}

struct ProcessGroupGuard {
    pid: u32,
    armed: bool,
}
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = killpg(Pid::from_raw(self.pid as i32), Signal::SIGKILL);
        }
    }
}

fn capture_task(
    pipe: impl AsyncRead + Unpin + Send + 'static,
    file: std::fs::File,
    limit: usize,
    tx: mpsc::Sender<UiEvent>,
    label: &'static str,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<Result<Excerpt>> {
    tokio::spawn(async move {
        let result = capture(pipe, file, limit, tx, label).await;
        if result.is_err() {
            cancel.cancel();
        }
        result
    })
}

#[derive(Default)]
struct Excerpt {
    head: Vec<u8>,
    tail: Vec<u8>,
    total: u64,
    limit: usize,
}
impl Excerpt {
    fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        let half = self.limit / 2;
        let take = (half - self.head.len()).min(bytes.len());
        self.head.extend_from_slice(&bytes[..take]);
        self.tail.extend_from_slice(&bytes[take..]);
        if self.tail.len() > half {
            self.tail.drain(..self.tail.len() - half);
        }
    }
    fn truncated(&self) -> bool {
        self.total > self.limit as u64
    }
    fn text(&self) -> String {
        let mut result = String::from_utf8_lossy(&self.head).into_owned();
        if self.truncated() {
            result.push_str("\n[output truncated; complete output in local artefact]\n");
        }
        result.push_str(&String::from_utf8_lossy(&self.tail));
        result
    }
}

async fn capture(
    mut pipe: impl AsyncRead + Unpin,
    file: std::fs::File,
    limit: usize,
    tx: mpsc::Sender<UiEvent>,
    label: &'static str,
) -> Result<Excerpt> {
    let mut file = tokio::fs::File::from_std(file);
    let mut excerpt = Excerpt {
        limit,
        ..Default::default()
    };
    let mut buffer = [0u8; 8192];
    loop {
        let n = pipe.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n]).await?;
        excerpt.push(&buffer[..n]);
        let text = String::from_utf8_lossy(&buffer[..n]);
        let tail = text.lines().last().unwrap_or("");
        // Output is live status, not repeatedly appended transcript content.
        let _ = tx
            .send(UiEvent::Status(format!("{label}: {}", clip(tail, 500))))
            .await;
    }
    file.sync_data().await?;
    Ok(excerpt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Paths, model::Mode};
    #[tokio::test(flavor = "multi_thread")]
    async fn bypass_bypasses_approval_but_preserves_timeouts() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(&temp.path().join("state"));
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        let mut session = Session::create(&paths, &root).unwrap();
        session.set_mode(Mode::Build).unwrap();
        let config = Config {
            bypass: true,
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::channel(32);
        let watcher = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                assert!(!matches!(event, UiEvent::Approval { .. }));
            }
        });
        let result = run(
            &mut session,
            &config,
            &ToolCall {
                id: "bypass-timeout".into(),
                name: "run_shell".into(),
                arguments: "{}".into(),
            },
            ShellArgs {
                command: "sleep 30 & wait".into(),
                cwd: None,
                timeout_seconds: Some(1),
            },
            &tx,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "timed_out");
        drop(tx);
        watcher.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn denial_never_spawns_and_approval_captures_diff() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(&temp.path().join("state"));
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        let mut s = Session::create(&paths, &root).unwrap();
        s.set_mode(Mode::Build).unwrap();
        let config = Config::default();
        let cancel = CancellationToken::new();
        let call = ToolCall {
            id: "shell1".into(),
            name: "run_shell".into(),
            arguments: "{}".into(),
        };
        let args = ShellArgs {
            command: "printf hello > file; cat file".into(),
            cwd: None,
            timeout_seconds: Some(5),
        };
        let (tx, mut rx) = mpsc::channel(32);
        let watcher = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let UiEvent::Approval { respond, .. } = event {
                    let _ = respond.send(false);
                }
            }
        });
        let result = run(&mut s, &config, &call, args.clone(), &tx, &cancel)
            .await
            .unwrap();
        assert_eq!(result["status"], "denied");
        assert!(!root.join("file").exists());
        drop(tx);
        watcher.await.unwrap();
        let (tx, mut rx) = mpsc::channel(32);
        let watcher = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let UiEvent::Approval { respond, .. } = event {
                    let _ = respond.send(true);
                }
            }
        });
        let result = run(&mut s, &config, &call, args, &tx, &cancel)
            .await
            .unwrap();
        assert_eq!(result["status"], "ok");
        assert!(super::super::diff(&s, None).unwrap().contains("+hello"));
        drop(tx);
        watcher.await.unwrap();
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn timeout_terminates_process_group() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(&temp.path().join("state"));
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        let mut s = Session::create(&paths, &root).unwrap();
        s.set_mode(Mode::Build).unwrap();
        let (tx, mut rx) = mpsc::channel(32);
        let watcher = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let UiEvent::Approval { respond, .. } = event {
                    let _ = respond.send(true);
                }
            }
        });
        let result = run(
            &mut s,
            &Config::default(),
            &ToolCall {
                id: "timeout".into(),
                name: "run_shell".into(),
                arguments: "{}".into(),
            },
            ShellArgs {
                command: "sleep 30 & wait".into(),
                cwd: None,
                timeout_seconds: Some(1),
            },
            &tx,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "timed_out");
        drop(tx);
        watcher.await.unwrap();
    }
}
