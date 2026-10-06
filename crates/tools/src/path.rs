use std::path::{Path, PathBuf};

/// Resolve a path given by the agent: relative paths resolve against the
/// project's main folder, absolute paths are used as-is.
pub fn resolve(main_folder: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        main_folder.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_resolve_against_main_folder() {
        assert_eq!(
            resolve(Path::new("/p"), "src/a.rs"),
            PathBuf::from("/p/src/a.rs")
        );
        assert_eq!(resolve(Path::new("/p"), "/etc/x"), PathBuf::from("/etc/x"));
    }
}
