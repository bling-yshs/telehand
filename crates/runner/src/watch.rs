//! Reloading the config file while the runner runs, so project changes take
//! effect without a restart.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use notify_debouncer_mini::{
    DebounceEventResult, Debouncer, new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
};
use tokio::sync::watch;

use crate::RunnerConfig;

/// Saving the config emits several events (writing a temporary file, renaming
/// it over the config); they are handled as one.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Keeps the config file watched; dropping it stops watching.
pub struct ConfigWatcher {
    _debouncer: Debouncer<RecommendedWatcher>,
}

/// Watch the config file at `path`, which was loaded as `config`. Each time its
/// projects change, the receiver sees the new config. A file that fails to load
/// keeps the previous config. The server URL and key are not reloaded: they
/// take a restart.
pub fn watch_config(
    path: &Path,
    config: RunnerConfig,
) -> anyhow::Result<(watch::Receiver<RunnerConfig>, ConfigWatcher)> {
    let (tx, rx) = watch::channel(config);
    // The config is replaced by renaming a temporary file over it, which a
    // watch on the file itself may not survive: watch its folder instead.
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let file_name = path
        .file_name()
        .context("the config path has no file name")?
        .to_owned();
    let path = path.to_path_buf();
    let mut debouncer = new_debouncer(DEBOUNCE, move |result: DebounceEventResult| {
        let Ok(events) = result else { return };
        if events
            .iter()
            .any(|e| e.path.file_name() == Some(file_name.as_os_str()))
        {
            reload(&path, &tx);
        }
    })?;
    debouncer
        .watcher()
        .watch(&dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", dir.display()))?;
    Ok((
        rx,
        ConfigWatcher {
            _debouncer: debouncer,
        },
    ))
}

fn reload(path: &Path, tx: &watch::Sender<RunnerConfig>) {
    // Deleted (or not yet renamed into place): keep the previous config rather
    // than loading it as empty.
    if !path.is_file() {
        return;
    }
    let new = match RunnerConfig::load(path) {
        Ok(config) => config,
        Err(e) => {
            tracing::warn!(
                "cannot reload the config: {e:#}; keeping the previous projects until it is fixed"
            );
            return;
        }
    };
    tx.send_if_modified(|config| {
        if new.server_url != config.server_url || new.key != config.key {
            tracing::warn!(
                "the server URL or key in the config changed; restart the runner for that to take effect"
            );
        }
        if new.projects == config.projects {
            return false;
        }
        config.projects = new.projects;
        let names: Vec<&str> = config.projects.iter().map(|p| p.name.as_str()).collect();
        if names.is_empty() {
            tracing::info!("projects reloaded: none");
        } else {
            tracing::info!("projects reloaded: {}", names.join(", "));
        }
        true
    });
}
