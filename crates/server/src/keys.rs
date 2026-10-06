//! The key store: `keys.json` in the data directory.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEntry {
    #[serde(default)]
    pub name: Option<String>,
    /// Unix timestamp in seconds.
    pub created_at: u64,
    #[serde(default)]
    pub current_project: Option<String>,
}

pub type Keys = BTreeMap<String, KeyEntry>;

pub fn keys_path(data_dir: &Path) -> PathBuf {
    data_dir.join("keys.json")
}

/// Load the key store; a missing file is an empty store.
pub fn load(data_dir: &Path) -> anyhow::Result<Keys> {
    let path = keys_path(data_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Keys::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Atomically replace the key store with `keys`.
pub fn save(data_dir: &Path, keys: &Keys) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let path = keys_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(keys)?;
    std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

pub fn new_entry(name: Option<String>) -> (String, KeyEntry) {
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let entry = KeyEntry {
        name,
        created_at,
        current_project: None,
    };
    (uuid::Uuid::new_v4().to_string(), entry)
}

/// Create a key directly in the store file.
pub fn create_in_file(data_dir: &Path, name: Option<String>) -> anyhow::Result<String> {
    let mut keys = load(data_dir)?;
    let (key, entry) = new_entry(name);
    keys.insert(key.clone(), entry);
    save(data_dir, &keys)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn created_keys_are_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let a = create_in_file(dir.path(), Some("laptop".into())).unwrap();
        let b = create_in_file(dir.path(), None).unwrap();
        let keys = load(dir.path()).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[&a].name.as_deref(), Some("laptop"));
        assert!(keys.contains_key(&b));
        assert!(uuid::Uuid::parse_str(&a).is_ok());
    }

    #[test]
    fn missing_store_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).unwrap().is_empty());
    }
}
