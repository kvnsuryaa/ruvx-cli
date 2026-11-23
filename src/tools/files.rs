use crate::{
    config::ToolConfig,
    session::{Blob, Session, atomic_write, hash},
};
use anyhow::{Context, Result, bail, ensure};
use globset::Glob;
use ignore::{WalkBuilder, gitignore::GitignoreBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

pub fn resolve_path(root: &Path, input: &str, allow_missing: bool) -> Result<PathBuf> {
    let relative = Path::new(input);
    ensure!(
        !relative.is_absolute()
            && relative
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "Path must be relative and cannot traverse outside the workspace"
    );
    ensure!(
        !relative.components().any(|c| c.as_os_str() == ".git"),
        ".git internals are not available to file tools"
    );
    let joined = root.join(relative);
    let mut existing = joined.as_path();
    while !existing.exists() {
        ensure!(allow_missing, "Path does not exist: {input}");
        // A broken symlink is not an absent path that may be overwritten.
        ensure!(
            fs::symlink_metadata(existing).is_err(),
            "Broken symlink is not a writable target"
        );
        existing = existing.parent().context("Invalid path")?;
    }
    ensure!(
        existing.canonicalize()?.starts_with(root.canonicalize()?),
        "Symlink points outside the workspace"
    );
    Ok(joined)
}

fn ignored(root: &Path, path: &Path) -> Result<bool> {
    let parent = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(root)
    };
    let mut bases = vec![root.to_path_buf()];
    let mut cursor = root.to_path_buf();
    for part in parent.strip_prefix(root)?.components() {
        cursor.push(part);
        bases.push(cursor.clone());
    }
    let mut ignored = false;
    for base in bases {
        let mut builder = GitignoreBuilder::new(&base);
        let file = base.join(".gitignore");
        if file.exists() {
            if let Some(error) = builder.add(file) {
                return Err(error.into());
            }
            match builder
                .build()?
                .matched_path_or_any_parents(path, path.is_dir())
            {
                ignore::Match::Ignore(_) => ignored = true,
                ignore::Match::Whitelist(_) => ignored = false,
                _ => {}
            }
        }
    }
    Ok(ignored)
}

fn protected(path: &Path, config: &ToolConfig) -> bool {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    config.protected_paths.iter().any(|p| {
        path.starts_with(p) || canonical.starts_with(p.canonicalize().unwrap_or_else(|_| p.clone()))
    })
}

fn walk(root: &Path, start: &Path, config: &ToolConfig) -> Result<Vec<PathBuf>> {
    walk_cancellable(root, start, config, None)
}

fn walk_cancellable(
    root: &Path,
    start: &Path,
    config: &ToolConfig,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<Vec<PathBuf>> {
    let canonical_root = root.canonicalize()?;
    ensure!(
        !protected(start, config),
        "ruvx private application data is not available to workspace tools"
    );
    let protected_config = config.clone();
    let mut paths = Vec::new();
    for entry in WalkBuilder::new(start)
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(move |e| {
            !matches!(
                e.file_name().to_str(),
                Some(".git" | "node_modules" | "target" | ".venv")
            ) && !protected(e.path(), &protected_config)
        })
        .build()
    {
        ensure!(
            cancel.is_none_or(|c| !c.is_cancelled()),
            "File search canceled"
        );
        let entry = entry?;
        if entry.path() == start && start == root {
            continue;
        }
        let path = entry.path().to_path_buf();
        if path
            .canonicalize()
            .is_ok_and(|p| p.starts_with(&canonical_root))
            && !ignored(root, &path)?
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Paths only: autocomplete never reads file contents. The caller runs this off the UI thread.
/// Return at most 100 suggestions, preferring path and basename prefixes over substrings.
pub fn file_suggestions(
    root: &Path,
    config: &ToolConfig,
    query: &str,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Vec<String>> {
    let query = query.to_lowercase();
    let mut matches = Vec::new();
    for path in walk_cancellable(root, root, config, Some(cancel))? {
        ensure!(!cancel.is_cancelled(), "File search canceled");
        if !path.is_file() {
            continue;
        }
        let Some(relative) = path.strip_prefix(root)?.to_str() else {
            continue;
        };
        // The reference syntax cannot represent quotes or multiline/control-character paths.
        if relative.chars().any(|c| c == '"' || c.is_control()) {
            continue;
        }
        let lower = relative.to_lowercase();
        if !lower.contains(&query) {
            continue;
        }
        let basename = lower.rsplit('/').next().unwrap_or(&lower);
        let rank = if lower.starts_with(&query) {
            0
        } else if basename.starts_with(&query) {
            1
        } else {
            2
        };
        matches.push((rank, relative.to_owned()));
    }
    matches.sort();
    Ok(matches
        .into_iter()
        .take(100)
        .map(|(_, path)| path)
        .collect())
}

fn read_text(path: &Path, limit: usize) -> Result<String> {
    ensure!(path.is_file(), "Path is not a file");
    ensure!(
        fs::metadata(path)?.len() <= limit as u64,
        "File exceeds text size limit"
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "File grew beyond text size limit");
    ensure!(
        !bytes.contains(&0),
        "Binary files are not supported by text tools"
    );
    String::from_utf8(bytes).context("File is not UTF-8 text")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadArgs {
    pub path: String,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

pub fn read_file(
    root: &Path,
    config: &ToolConfig,
    args: &ReadArgs,
    explicit: bool,
) -> Result<Value> {
    let path = resolve_path(root, &args.path, false)?;
    ensure!(
        !protected(&path, config),
        "ruvx private application data is not available to file tools"
    );
    ensure!(
        explicit || !ignored(root, &path)?,
        "File is ignored; an explicit user file reference is required"
    );
    let text = read_text(&path, config.max_file_bytes)?;
    let start = args.start_line.unwrap_or(1);
    ensure!(start >= 1, "start_line must be at least 1");
    let requested_end = args
        .end_line
        .unwrap_or(start.saturating_add(config.read_max_lines - 1));
    ensure!(
        requested_end >= start,
        "end_line must not precede start_line"
    );
    let end = requested_end.min(start.saturating_add(config.read_max_lines - 1));
    let mut content = String::new();
    let mut actual_end = start.saturating_sub(1);
    let mut total = 0;
    for (i, line) in text.lines().enumerate() {
        let number = i + 1;
        total = number;
        if number >= start && number <= end {
            let next = format!("{number}: {line}\n");
            ensure!(
                next.len() <= config.result_max_bytes,
                "Line {number} exceeds result_max_bytes; increase the configured limit before reading this line"
            );
            if content.len() + next.len() > config.result_max_bytes {
                continue;
            }
            // Once a line does not fit, keep the returned range contiguous.
            if actual_end + 1 != number {
                continue;
            }
            content.push_str(&next);
            actual_end = number;
        }
    }
    let next_line = if actual_end < total {
        Some(actual_end + 1)
    } else {
        None
    };
    Ok(
        json!({"status":"ok","path":args.path,"content":content,"start_line":start,"end_line":actual_end,"total_lines":total,"next_line":next_line,"truncated":next_line.is_some(),"expected_hash":hash(text.as_bytes())}),
    )
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ListArgs {
    pub path: String,
    pub glob: Option<String>,
    pub cursor: usize,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    pub query: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    pub cursor: Option<usize>,
}

fn glob_match(glob: &Option<String>) -> Result<Option<globset::GlobMatcher>> {
    glob.as_ref()
        .map(|g| Ok(Glob::new(g)?.compile_matcher()))
        .transpose()
}

pub fn list_files(root: &Path, config: &ToolConfig, args: ListArgs) -> Result<Value> {
    let start = resolve_path(root, &args.path, false)?;
    let glob = glob_match(&args.glob)?;
    let mut entries = Vec::new();
    let mut matched = 0;
    let mut bytes = 0;
    let mut next = None;
    for path in walk(root, &start, config)? {
        let relative = path.strip_prefix(root)?.to_string_lossy().into_owned();
        if glob.as_ref().is_some_and(|g| !g.is_match(&relative)) {
            continue;
        }
        matched += 1;
        if matched <= args.cursor {
            continue;
        }
        if entries.len() >= config.list_page_size
            || bytes + relative.len() > config.result_max_bytes
        {
            next = Some(matched - 1);
            break;
        }
        bytes += relative.len();
        entries.push(json!({"path":relative,"kind":if path.is_dir(){"directory"}else{"file"}}));
    }
    Ok(json!({"status":"ok","entries":entries,"next_cursor":next,"truncated":next.is_some()}))
}

pub fn search_text(root: &Path, config: &ToolConfig, args: SearchArgs) -> Result<Value> {
    ensure!(!args.query.is_empty(), "Search query must not be empty");
    let start = resolve_path(root, args.path.as_deref().unwrap_or(""), false)?;
    let glob = glob_match(&args.glob)?;
    let cursor = args.cursor.unwrap_or(0);
    let mut results = vec![];
    let mut matched = 0;
    let mut bytes = 0;
    let mut next = None;
    'outer: for path in walk(root, &start, config)? {
        if !path.is_file() {
            continue;
        }
        let relative = path.strip_prefix(root)?.to_string_lossy().into_owned();
        if glob.as_ref().is_some_and(|g| !g.is_match(&relative)) {
            continue;
        }
        let Ok(text) = read_text(&path, config.max_file_bytes) else {
            continue;
        };
        for (line, content) in text.lines().enumerate() {
            if !content.contains(&args.query) {
                continue;
            }
            matched += 1;
            if matched <= cursor {
                continue;
            }
            let excerpt = clip(content, config.result_max_bytes.min(2048));
            let item = json!({"path":relative,"line":line+1,"text":excerpt});
            let len = serde_json::to_vec(&item)?.len();
            if results.len() >= config.list_page_size || bytes + len > config.result_max_bytes {
                next = Some(matched - 1);
                break 'outer;
            }
            bytes += len;
            results.push(item);
        }
    }
    Ok(json!({"status":"ok","matches":results,"next_cursor":next,"truncated":next.is_some()}))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PatchArgs {
    pub path: String,
    pub operation: String,
    pub expected_hash: Option<String>,
    pub content: Option<String>,
    pub old_text: Option<String>,
    pub new_text: Option<String>,
}
pub fn apply_patch(session: &mut Session, config: &ToolConfig, args: PatchArgs) -> Result<Value> {
    let path = resolve_path(&session.root, &args.path, true)?;
    ensure!(
        !protected(&path, config),
        "ruvx private application data cannot be patched"
    );
    ensure!(path != session.root, "Cannot patch workspace root");
    ensure!(
        !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()),
        "Patch does not replace symlinks; use the resolved workspace path"
    );
    ensure!(
        !ignored(&session.root, &path)?,
        "Patching ignored files is not supported by built-in patch"
    );
    let before = if path.exists() {
        Some(read_text(&path, config.max_file_bytes)?)
    } else {
        None
    };
    let after = match args.operation.as_str() {
        "add" => {
            ensure!(
                before.is_none() && args.expected_hash.is_none(),
                "Add requires an absent file and no expected_hash"
            );
            ensure!(
                args.old_text.is_none() && args.new_text.is_none(),
                "Add uses content"
            );
            Some(args.content.context("Add requires content")?)
        }
        "update" | "delete" => {
            let original = before.as_ref().context("File does not exist")?;
            ensure!(
                args.expected_hash.as_deref() == Some(hash(original.as_bytes()).as_str()),
                "File version conflict; read the file again"
            );
            if args.operation == "delete" {
                ensure!(
                    args.content.is_none() && args.old_text.is_none() && args.new_text.is_none(),
                    "Delete does not accept replacement text"
                );
                None
            } else if let Some(content) = args.content {
                ensure!(
                    args.old_text.is_none() && args.new_text.is_none(),
                    "Choose content or old_text/new_text, not both"
                );
                Some(content)
            } else {
                let old = args
                    .old_text
                    .context("Update requires content or old_text/new_text")?;
                let new = args.new_text.context("Missing new_text")?;
                ensure!(
                    !old.is_empty() && original.matches(&old).count() == 1,
                    "old_text must match exactly once"
                );
                Some(original.replacen(&old, &new, 1))
            }
        }
        _ => bail!("operation must be add, update, or delete"),
    };
    if let Some(text) = &after {
        ensure!(
            text.len() <= config.max_file_bytes && !text.contains('\0'),
            "Replacement exceeds text limits or contains binary data"
        );
    }
    let before_blob = before
        .as_ref()
        .map(|t| session.blob(t.as_bytes()))
        .transpose()?;
    let after_blob = after
        .as_ref()
        .map(|t| session.blob(t.as_bytes()))
        .transpose()?;
    // Persist the baseline before touching the project.
    session.record_change(
        &args.path,
        before_blob.clone(),
        before_blob.clone(),
        "patch",
    )?;
    let current = if path.exists() {
        Some(read_text(&path, config.max_file_bytes)?)
    } else {
        None
    };
    ensure!(
        current == before,
        "File changed while preparing patch; read again"
    );
    if let Some(text) = &after {
        fs::create_dir_all(path.parent().context("Invalid parent")?)?;
        resolve_path(&session.root, &args.path, true)?;
        let permissions = fs::metadata(&path).ok().map(|m| m.permissions());
        if args.operation == "add" {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        } else {
            atomic_write(&path, text.as_bytes(), permissions)?;
        }
    } else {
        fs::remove_file(&path)?;
    }
    session.record_change(&args.path, before_blob, after_blob.clone(), "patch")?;
    Ok(
        json!({"status":"ok","path":args.path,"operation":args.operation,"expected_hash":after_blob.map(|b|b.id)}),
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub files: BTreeMap<String, Blob>,
    pub skipped: usize,
}
pub fn snapshot(session: &Session, config: &ToolConfig) -> Result<Snapshot> {
    let mut files = BTreeMap::new();
    let mut skipped = 0;
    for path in walk(&session.root, &session.root, config)? {
        if !path.is_file() || fs::symlink_metadata(&path)?.file_type().is_symlink() {
            continue;
        }
        if fs::metadata(&path)?.len() > config.max_file_bytes as u64 {
            skipped += 1;
            continue;
        }
        let mut bytes = Vec::new();
        File::open(&path)?
            .take(config.max_file_bytes as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > config.max_file_bytes
            || bytes.contains(&0)
            || std::str::from_utf8(&bytes).is_err()
        {
            skipped += 1;
            continue;
        }
        files.insert(
            path.strip_prefix(&session.root)?
                .to_string_lossy()
                .into_owned(),
            session.blob(&bytes)?,
        );
    }
    Ok(Snapshot { files, skipped })
}
pub fn record_snapshot_changes(
    session: &mut Session,
    before: &Snapshot,
    after: &Snapshot,
) -> Result<()> {
    let keys: BTreeSet<_> = before.files.keys().chain(after.files.keys()).collect();
    for path in keys {
        let a = before.files.get(path);
        let b = after.files.get(path);
        if a == b {
            continue;
        }
        // Newly ignored/oversized/binary files are outside coverage, not deleted files.
        let disk = session.root.join(path);
        if b.is_none() && disk.exists() {
            continue;
        }
        session.record_change(path, a.cloned(), b.cloned(), "observed shell interval")?;
    }
    Ok(())
}

pub fn diff(session: &Session, filter: Option<&str>) -> Result<String> {
    if let Some(path) = filter {
        resolve_path(&session.root, path, true)?;
    }
    let mut output = String::new();
    for (path, entry) in session.review()? {
        if filter.is_some_and(|p| p != path) || entry.before == entry.after {
            continue;
        }
        let before = entry
            .before
            .as_ref()
            .map(|b| session.read_blob(b))
            .transpose()?
            .unwrap_or_default();
        let after = entry
            .after
            .as_ref()
            .map(|b| session.read_blob(b))
            .transpose()?
            .unwrap_or_default();
        let actual = fs::read(session.root.join(&path)).ok().map(|b| hash(&b));
        if actual.as_deref() != entry.after.as_ref().map(|b| b.id.as_str()) {
            output.push_str(&format!(
                "[Changed outside session since recorded result: {path}]\n"
            ));
        }
        output.push_str(&format!("[{path} · {}]\n", entry.source));
        output.push_str(
            &similar::TextDiff::from_lines(
                std::str::from_utf8(&before)?,
                std::str::from_utf8(&after)?,
            )
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{path}"), &format!("b/{path}"))
            .to_string(),
        );
        output.push('\n');
    }
    if output.is_empty() {
        output.push_str("No recorded text changes in this session.\n");
    }
    output.push_str("Review excludes ignored, dependency/build, binary, oversized, and outside-workspace files. Shell intervals do not prove authorship.\n");
    Ok(output)
}

pub fn instructions_for(
    root: &Path,
    target: &str,
    config: &ToolConfig,
) -> Result<Vec<(String, String, String)>> {
    let target = resolve_path(root, target, true)?;
    let end = if target.is_dir() {
        target
    } else {
        target.parent().unwrap_or(root).to_path_buf()
    };
    let mut dirs = vec![root.to_path_buf()];
    let mut dir = root.to_path_buf();
    for part in end.strip_prefix(root)?.components() {
        dir.push(part);
        dirs.push(dir.clone());
    }
    let mut result = vec![];
    for dir in dirs {
        let file = dir.join("AGENTS.md");
        if file.exists() {
            ensure!(
                !protected(&file, config),
                "Instructions cannot be loaded from ruvx private data"
            );
            let relative = file.strip_prefix(root)?.to_string_lossy().into_owned();
            resolve_path(root, &relative, false)?;
            let text = read_text(&file, config.max_file_bytes)?;
            result.push((relative, hash(text.as_bytes()), text));
        }
    }
    Ok(result)
}
pub fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.into();
    }
    let mut boundary = max;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text[..boundary].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Paths, model::Mode};
    #[test]
    fn suggestions_are_ranked_bounded_and_respect_workspace_filters() {
        use tokio_util::sync::CancellationToken;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        for directory in ["src", "nested", "private", ".git", "target", "node_modules"] {
            fs::create_dir(root.path().join(directory)).unwrap();
        }
        for name in ["src/main.txt", "nested/src.txt", "nested/has-src.txt", "ignored.txt", "private/secret.txt", ".git/config.txt", "target/build.txt", "node_modules/pkg.txt", "bad\"name.txt", "bad\nname.txt"] {
            fs::write(root.path().join(name), "data").unwrap();
        }
        fs::write(root.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(outside.path().join("outside.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(outside.path().join("outside.txt"), root.path().join("outside.txt")).unwrap();
        std::os::unix::fs::symlink(root.path().join("private/secret.txt"), root.path().join("alias.txt")).unwrap();
        let config = ToolConfig {
            protected_paths: vec![root.path().join("private")],
            ..Default::default()
        };
        let cancel = CancellationToken::new();
        assert_eq!(file_suggestions(root.path(), &config, "SRC", &cancel).unwrap(),
            vec!["src/main.txt", "nested/src.txt", "nested/has-src.txt"]);
        let suggestions = file_suggestions(root.path(), &config, ".txt", &cancel).unwrap();
        assert_eq!(suggestions.len(), 3, "{suggestions:?}");
        assert!(file_suggestions(root.path(), &config, "not-found", &cancel).unwrap().is_empty());
        for index in 0..110 {
            fs::write(root.path().join(format!("file-{index:03}.txt")), "").unwrap();
        }
        let suggestions = file_suggestions(root.path(), &config, "file-", &cancel).unwrap();
        assert_eq!(suggestions.len(), 100);
        assert_eq!(suggestions[0], "file-000.txt");
        cancel.cancel();
        assert!(file_suggestions(root.path(), &config, "", &cancel).is_err());
    }

    #[test]
    fn paths_and_ignore_rules() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(root.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(root.path().join("ignored.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
        assert!(resolve_path(root.path(), "../outside", true).is_err());
        assert!(resolve_path(root.path(), "link/file", true).is_err());
        let args = ReadArgs {
            path: "ignored.txt".into(),
            start_line: None,
            end_line: None,
        };
        assert!(read_file(root.path(), &ToolConfig::default(), &args, false).is_err());
        assert!(read_file(root.path(), &ToolConfig::default(), &args, true).is_ok());
    }
    #[test]
    fn patch_preserves_dirty_baseline_and_detects_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(&temp.path().join("state"));
        let root = temp.path().join("work");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file"), "already dirty\n").unwrap();
        let mut session = Session::create(&paths, &root).unwrap();
        session.set_mode(Mode::Build).unwrap();
        let make = |expected: String, content: &str| PatchArgs {
            path: "file".into(),
            operation: "update".into(),
            expected_hash: Some(expected),
            content: Some(content.into()),
            old_text: None,
            new_text: None,
        };
        apply_patch(
            &mut session,
            &ToolConfig::default(),
            make(hash(b"already dirty\n"), "agent\n"),
        )
        .unwrap();
        assert!(
            apply_patch(
                &mut session,
                &ToolConfig::default(),
                make(hash(b"already dirty\n"), "wrong\n")
            )
            .is_err()
        );
        let output = diff(&session, None).unwrap();
        assert!(output.contains("-already dirty"));
        assert!(output.contains("+agent"));
        fs::write(root.join("file"), "external\n").unwrap();
        assert!(
            diff(&session, None)
                .unwrap()
                .contains("Changed outside session")
        );
    }
    #[test]
    fn nested_instructions_and_literal_search() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("AGENTS.md"), "root").unwrap();
        fs::write(root.path().join("src/AGENTS.md"), "nested").unwrap();
        fs::write(root.path().join("src/a"), "a.*b\nab\n").unwrap();
        assert_eq!(
            instructions_for(root.path(), "src/a", &ToolConfig::default())
                .unwrap()
                .len(),
            2
        );
        let result = search_text(
            root.path(),
            &ToolConfig::default(),
            SearchArgs {
                query: "a.*b".into(),
                path: None,
                glob: None,
                cursor: None,
            },
        )
        .unwrap();
        assert_eq!(result["matches"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn private_application_data_cannot_be_read_or_indexed() {
        let root = tempfile::tempdir().unwrap();
        let protected_dir = root.path().join("private");
        fs::create_dir(&protected_dir).unwrap();
        fs::write(protected_dir.join("credentials.json"), "never-expose-key").unwrap();
        let config = ToolConfig {
            protected_paths: vec![protected_dir],
            ..Default::default()
        };
        let args = ReadArgs {
            path: "private/credentials.json".into(),
            start_line: None,
            end_line: None,
        };
        assert!(read_file(root.path(), &config, &args, true).is_err());
        let result = list_files(root.path(), &config, ListArgs::default()).unwrap();
        assert!(result["entries"].as_array().unwrap().is_empty());
        let result = search_text(
            root.path(),
            &config,
            SearchArgs {
                query: "never-expose".into(),
                path: None,
                glob: None,
                cursor: None,
            },
        )
        .unwrap();
        assert!(result["matches"].as_array().unwrap().is_empty());
    }
}
