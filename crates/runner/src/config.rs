//! The runner's local configuration file.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use telehand_proto::ProjectInfo;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunnerConfig {
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub projects: Vec<ProjectInfo>,
}

/// `~/.config/telehand/runner.json` (or the platform equivalent).
pub fn default_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("telehand")
        .join("runner.json")
}

impl RunnerConfig {
    /// Load the config; a missing file is an empty config.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn is_registered(&self) -> bool {
        !self.server_url.is_empty() && !self.key.is_empty()
    }

    pub fn project(&self, name: &str) -> Option<&ProjectInfo> {
        self.projects.iter().find(|p| p.name == name)
    }

    /// Add a project whose main folder is `dir`.
    pub fn add_project(&mut self, name: &str, dir: &Path) -> anyhow::Result<&ProjectInfo> {
        if name.trim().is_empty() {
            bail!("project name must not be empty");
        }
        if self.project(name).is_some() {
            bail!("project '{name}' already exists");
        }
        let main_folder = canonical_dir(dir)?;
        self.projects.push(ProjectInfo {
            name: name.to_string(),
            main_folder,
            extra_folders: Vec::new(),
        });
        Ok(self.projects.last().expect("just pushed"))
    }
}

/// The canonical absolute path of an existing directory.
pub fn canonical_dir(dir: &Path) -> anyhow::Result<String> {
    let path = dir
        .canonicalize()
        .with_context(|| format!("folder {} does not exist", dir.display()))?;
    if !path.is_dir() {
        bail!("{} is not a folder", path.display());
    }
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_round_trip_through_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg").join("runner.json");
        let mut config = RunnerConfig::load(&path).unwrap();
        assert!(!config.is_registered());
        config.server_url = "http://127.0.0.1:8080".into();
        config.key = "k".into();
        config.add_project("demo", tmp.path()).unwrap();
        config.save(&path).unwrap();

        let loaded = RunnerConfig::load(&path).unwrap();
        assert_eq!(loaded, config);
        assert!(loaded.is_registered());
        assert_eq!(
            loaded.project("demo").unwrap().main_folder,
            tmp.path().canonicalize().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn project_names_are_unique_and_folders_must_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let mut config = RunnerConfig::default();
        config.add_project("demo", tmp.path()).unwrap();
        assert!(config.add_project("demo", tmp.path()).is_err());
        assert!(
            config
                .add_project("other", &tmp.path().join("missing"))
                .is_err()
        );
    }
}
