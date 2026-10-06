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

    /// Add a project whose main folder is `dir`, named `name` or else after the folder.
    pub fn add_project(&mut self, dir: &Path, name: Option<&str>) -> anyhow::Result<&ProjectInfo> {
        let main_folder = canonical_dir(dir)?;
        let name = match name {
            Some(name) => name.to_string(),
            None => match Path::new(&main_folder).file_name() {
                Some(folder) => folder.to_string_lossy().into_owned(),
                None => bail!("cannot name a project after {main_folder}; pass --name"),
            },
        };
        if let Err(e) = self.check_new_name(&name) {
            bail!("{e}; pass --name to choose another name");
        }
        self.projects.push(ProjectInfo {
            name,
            main_folder,
            extra_folders: Vec::new(),
        });
        Ok(self.projects.last().expect("just pushed"))
    }

    pub fn rename_project(&mut self, name: &str, new_name: &str) -> anyhow::Result<&ProjectInfo> {
        let Some(index) = self.projects.iter().position(|p| p.name == name) else {
            bail!("project '{name}' does not exist");
        };
        self.check_new_name(new_name)?;
        let project = &mut self.projects[index];
        project.name = new_name.to_string();
        Ok(project)
    }

    /// A name a new or renamed project may take: non-empty and unused.
    fn check_new_name(&self, name: &str) -> anyhow::Result<()> {
        if name.trim().is_empty() {
            bail!("project name must not be empty");
        }
        if let Some(existing) = self.project(name) {
            bail!(
                "project '{name}' already exists ({})",
                existing.main_folder
            );
        }
        Ok(())
    }

    /// Add an extra folder to an existing project.
    pub fn add_folder(&mut self, name: &str, dir: &Path) -> anyhow::Result<&ProjectInfo> {
        let folder = canonical_dir(dir)?;
        let Some(project) = self.projects.iter_mut().find(|p| p.name == name) else {
            bail!("project '{name}' does not exist");
        };
        if project.main_folder == folder || project.extra_folders.contains(&folder) {
            bail!("{folder} is already a folder of project '{name}'");
        }
        project.extra_folders.push(folder);
        Ok(project)
    }

    pub fn remove_project(&mut self, name: &str) -> anyhow::Result<ProjectInfo> {
        let Some(index) = self.projects.iter().position(|p| p.name == name) else {
            bail!("project '{name}' does not exist");
        };
        Ok(self.projects.remove(index))
    }
}

/// The canonical absolute path of an existing directory.
pub fn canonical_dir(dir: &Path) -> anyhow::Result<String> {
    let path = dir
        .canonicalize()
        .with_context(|| format!("folder {} does not exist", dir.display()))?;
    let path = strip_verbatim(&path.to_string_lossy());
    if !Path::new(&path).is_dir() {
        bail!("{path} is not a folder");
    }
    Ok(path)
}

/// Windows' `canonicalize` returns verbatim paths (`\\?\C:\x`, `\\?\UNC\host\share`);
/// show them the usual way (`C:\x`, `\\host\share`).
fn strip_verbatim(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    match path.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_string(),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_windows_paths_are_shown_plainly() {
        assert_eq!(strip_verbatim(r"\\?\C:\Users\me"), r"C:\Users\me");
        assert_eq!(strip_verbatim(r"\\?\UNC\host\share\x"), r"\\host\share\x");
        assert_eq!(strip_verbatim(r"\\?\Volume{abc}\x"), r"\\?\Volume{abc}\x");
        assert_eq!(strip_verbatim("/home/me"), "/home/me");
    }

    #[test]
    fn projects_round_trip_through_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg").join("runner.json");
        let mut config = RunnerConfig::load(&path).unwrap();
        assert!(!config.is_registered());
        config.server_url = "http://127.0.0.1:8080".into();
        config.key = "k".into();
        config.add_project(tmp.path(), Some("demo")).unwrap();
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
    fn extra_folders_and_removal() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let extra = tmp.path().join("extra");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        let mut config = RunnerConfig::default();
        config.add_project(&main, Some("demo")).unwrap();

        let project = config.add_folder("demo", &extra).unwrap();
        assert_eq!(
            project.extra_folders,
            vec![extra.canonicalize().unwrap().to_string_lossy().into_owned()]
        );
        assert!(config.add_folder("demo", &extra).is_err());
        assert!(config.add_folder("demo", &main).is_err());
        assert!(config.add_folder("nope", &extra).is_err());

        assert_eq!(config.remove_project("demo").unwrap().name, "demo");
        assert!(config.projects.is_empty());
        assert!(config.remove_project("demo").is_err());
    }

    #[test]
    fn project_names_are_unique_and_folders_must_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let mut config = RunnerConfig::default();
        config.add_project(tmp.path(), Some("demo")).unwrap();
        assert!(config.add_project(tmp.path(), Some("demo")).is_err());
        assert!(
            config
                .add_project(&tmp.path().join("missing"), Some("other"))
                .is_err()
        );
    }

    #[test]
    fn projects_are_named_after_their_folder_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("a").join("app");
        let second = tmp.path().join("b").join("app");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let mut config = RunnerConfig::default();

        assert_eq!(config.add_project(&first, None).unwrap().name, "app");
        let error = config.add_project(&second, None).unwrap_err().to_string();
        assert!(
            error.contains("project 'app' already exists") && error.contains("--name"),
            "{error}"
        );
        assert_eq!(
            config.add_project(&second, Some("app2")).unwrap().name,
            "app2"
        );
    }

    #[test]
    fn projects_can_be_renamed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut config = RunnerConfig::default();
        config.add_project(tmp.path(), Some("old")).unwrap();
        config.add_project(tmp.path(), Some("taken")).unwrap();

        assert_eq!(config.rename_project("old", "new").unwrap().name, "new");
        assert!(config.project("old").is_none());
        assert!(config.rename_project("new", "taken").is_err());
        assert!(config.rename_project("new", " ").is_err());
        assert!(config.rename_project("missing", "x").is_err());
    }
}
