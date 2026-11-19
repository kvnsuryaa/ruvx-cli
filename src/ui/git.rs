//! Small, read-only Git metadata polling; no Git process or repository walk.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle};

fn read_metadata(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(4097)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= 4096).then(|| String::from_utf8_lossy(&bytes).trim().to_owned())
}

fn branch(root: &Path) -> Option<String> {
    for dir in root.ancestors() {
        let marker = dir.join(".git");
        match fs::symlink_metadata(&marker) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        }
        let git_dir = if marker.is_dir() {
            marker
        } else {
            let file = read_metadata(&marker)?;
            let path = file.strip_prefix("gitdir:")?.trim();
            if path.is_empty() {
                return None;
            }
            dir.join(path)
        };
        let head = read_metadata(&git_dir.join("HEAD"))?;
        if let Some(name) = head.strip_prefix("ref: refs/heads/") {
            return (!name.is_empty() && !name.chars().any(char::is_control))
                .then(|| name.to_owned());
        }
        if matches!(head.len(), 40 | 64) && head.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(format!("detached {}", &head[..7]));
        }
        return None;
    }
    None
}

pub(super) struct Monitor {
    pub receiver: watch::Receiver<Option<String>>,
    task: JoinHandle<()>,
}

impl Monitor {
    pub fn new(root: PathBuf) -> Self {
        Self::with_interval(root, Duration::from_secs(2))
    }

    fn with_interval(root: PathBuf, interval: Duration) -> Self {
        let (tx, receiver) = watch::channel(None);
        let task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let root = root.clone();
                let next = tokio::task::spawn_blocking(move || branch(&root))
                    .await
                    .unwrap_or(None);
                tx.send_if_modified(|previous| {
                    if *previous == next {
                        return false;
                    }
                    *previous = next;
                    true
                });
            }
        });
        Self { receiver, task }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_unborn_nested_and_detached_heads_are_read_without_git() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        let head = root.join(".git/HEAD");
        fs::write(&head, "ref: refs/heads/feature/日本語\n").unwrap();
        assert_eq!(branch(&root).as_deref(), Some("feature/日本語"));
        assert_eq!(
            branch(&root.join("src/nested")).as_deref(),
            Some("feature/日本語")
        );
        fs::write(&head, "0123456789abcdef0123456789abcdef01234567\n").unwrap();
        assert_eq!(branch(&root).as_deref(), Some("detached 0123456"));
        fs::write(&head, "invalid HEAD").unwrap();
        assert!(branch(&root).is_none());
        fs::write(&head, "x".repeat(10000)).unwrap();
        assert!(branch(&root).is_none());
    }

    #[test]
    fn gitfiles_resolve_relative_and_absolute_worktree_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("worktree");
        let metadata = temp.path().join("metadata/worktrees/one");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&metadata).unwrap();
        fs::write(metadata.join("HEAD"), "ref: refs/heads/worktree-branch\n").unwrap();
        for path in [
            "../metadata/worktrees/one".to_owned(),
            metadata.display().to_string(),
        ] {
            fs::write(root.join(".git"), format!("gitdir: {path}\n")).unwrap();
            assert_eq!(branch(&root).as_deref(), Some("worktree-branch"));
        }
        fs::write(root.join(".git"), "invalid").unwrap();
        assert!(branch(&root).is_none());
        fs::remove_file(root.join(".git")).unwrap();
        assert!(branch(&root).is_none());
    }

    #[tokio::test]
    async fn monitor_updates_only_on_change_and_clears_removed_repositories() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join(".git")).unwrap();
        let head = temp.path().join(".git/HEAD");
        fs::write(&head, "ref: refs/heads/main\n").unwrap();
        let mut monitor = Monitor::with_interval(temp.path().to_owned(), Duration::from_millis(20));
        tokio::time::timeout(Duration::from_secs(5), monitor.receiver.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            monitor.receiver.borrow_and_update().as_deref(),
            Some("main")
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(70), monitor.receiver.changed())
                .await
                .is_err()
        );
        crate::session::atomic_write(&head, b"ref: refs/heads/next\n", None).unwrap();
        tokio::time::timeout(Duration::from_secs(5), monitor.receiver.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            monitor.receiver.borrow_and_update().as_deref(),
            Some("next")
        );
        fs::remove_file(&head).unwrap();
        tokio::time::timeout(Duration::from_secs(5), monitor.receiver.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(monitor.receiver.borrow_and_update().is_none());
    }
}
