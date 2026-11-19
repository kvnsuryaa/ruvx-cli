//! The last explicitly selected model, shared by this user's workspaces.
use crate::{
    config::{Config, Paths},
    model::Selection,
    session::{Session, atomic_write, lock_file},
};
use anyhow::{Context, Result, ensure};
use std::fs::{self, File};

pub(crate) fn load(paths: &Paths) -> Result<Option<Selection>> {
    match fs::read(paths.data.join("last-model.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .context("Read last-model.json: invalid model preference")
            .map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("Read last-model.json"),
    }
}

pub(crate) fn remember(paths: &Paths, selection: &Selection) -> Result<()> {
    let _lock = lock_file(&paths.data.join("last-model.lock"), true)?;
    atomic_write(
        &paths.data.join("last-model.json"),
        &serde_json::to_vec(selection)?,
        None,
    )
}

pub(crate) fn clear_profile(paths: &Paths, profile: &str) -> Result<()> {
    let _lock = lock_file(&paths.data.join("last-model.lock"), true)?;
    if load(paths)?.is_some_and(|selection| selection.profile == profile) {
        fs::remove_file(paths.data.join("last-model.json"))?;
        File::open(&paths.data)?.sync_all()?;
    }
    Ok(())
}

fn validate(config: &Config, selection: &Selection) -> Result<()> {
    let profile = config
        .providers
        .get(&selection.profile)
        .context("Provider profile is no longer configured")?;
    let model = profile.model(&selection.model)?;
    ensure!(
        model.supports_tools != Some(false) && model.supports_streaming,
        "Model does not support agent tools/streaming"
    );
    Ok(())
}

/// Restore without updating the user's default or starting work.
pub(crate) fn restore(
    paths: &Paths,
    config: &Config,
    session: &mut Session,
) -> Result<Option<Selection>> {
    if let Some(selection) = &session.selection {
        validate(config, selection).context("Saved session model unavailable")?;
        return Ok(None);
    }
    let Some(selection) = load(paths)? else {
        return Ok(None);
    };
    validate(config, &selection).context("Last model unavailable")?;
    session.select(selection.clone())?;
    Ok(Some(selection))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, sync::Arc, thread};

    fn selection(model: &str) -> Selection {
        Selection {
            profile: "local".into(),
            model: model.into(),
        }
    }

    #[test]
    fn preference_is_private_atomic_and_separate_from_config() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path());
        paths.save(&Config::default()).unwrap();
        let original = fs::read(paths.config_file()).unwrap();
        assert_eq!(load(&paths).unwrap(), None);
        let paths = Arc::new(paths);
        let mut writers = Vec::new();
        for id in 0..8 {
            let paths = paths.clone();
            writers.push(thread::spawn(move || {
                for _ in 0..4 {
                    remember(&paths, &selection(&format!("model-{id}"))).unwrap();
                    // Readers can only observe complete JSON snapshots.
                    assert!(load(&paths).unwrap().is_some());
                }
            }));
        }
        for writer in writers {
            writer.join().unwrap();
        }
        remember(&paths, &selection("last")).unwrap();
        assert_eq!(load(&paths).unwrap(), Some(selection("last")));
        assert_eq!(fs::read(paths.config_file()).unwrap(), original);
        assert_eq!(
            fs::metadata(paths.data.join("last-model.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&paths.data).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn disconnect_only_clears_the_current_matching_preference() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path());
        remember(&paths, &selection("first")).unwrap();
        let newer = Selection {
            profile: "other-window".into(),
            model: "newer".into(),
        };
        remember(&paths, &newer).unwrap();
        clear_profile(&paths, "local").unwrap();
        assert_eq!(load(&paths).unwrap(), Some(newer));
        clear_profile(&paths, "other-window").unwrap();
        assert_eq!(load(&paths).unwrap(), None);
        clear_profile(&paths, "other-window").unwrap();
    }
}
