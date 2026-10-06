//! Write scope: `write` and `edit` may only modify files inside the project's folders.

use std::path::{Path, PathBuf};

use crate::Project;

/// The real path of `target` (an absolute, lexically clean path): symlinks are
/// resolved; for a path that does not exist yet, its nearest existing ancestor
/// is resolved and the missing components are appended.
fn real_path(target: &Path) -> Result<PathBuf, String> {
    if target.symlink_metadata().is_ok() {
        return target
            .canonicalize()
            .map_err(|e| format!("Could not resolve {}: {e}", target.display()));
    }
    let mut missing = Vec::new();
    let mut ancestor = target;
    loop {
        let Some(parent) = ancestor.parent() else {
            return Err(format!("Could not resolve {}", target.display()));
        };
        missing.push(ancestor.file_name().unwrap_or_default().to_owned());
        ancestor = parent;
        if ancestor.symlink_metadata().is_ok() {
            break;
        }
    }
    let mut real = ancestor
        .canonicalize()
        .map_err(|e| format!("Could not resolve {}: {e}", ancestor.display()))?;
    for component in missing.iter().rev() {
        real.push(component);
    }
    Ok(real)
}

/// Check that `target` may be modified, returning its real path.
pub fn check_writable(project: &Project, target: &Path) -> Result<PathBuf, String> {
    let real = real_path(target)?;
    let folders: Vec<&PathBuf> = std::iter::once(&project.main_folder)
        .chain(project.extra_folders.iter())
        .collect();
    let allowed = folders
        .iter()
        .filter_map(|folder| folder.canonicalize().ok())
        .any(|root| real.starts_with(root));
    if allowed {
        Ok(real)
    } else {
        let folders: Vec<String> = folders.iter().map(|f| f.display().to_string()).collect();
        Err(format!(
            "Permission denied: {} is outside the current project's folders ({}). write and edit may only modify files inside these folders.",
            target.display(),
            folders.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(main: &Path, extra: &[&Path]) -> Project {
        Project {
            main_folder: main.to_path_buf(),
            extra_folders: extra.iter().map(|p| p.to_path_buf()).collect(),
        }
    }

    #[test]
    fn files_inside_project_folders_are_writable() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let extra = tmp.path().join("extra");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(main.join("a.txt"), "").unwrap();
        let p = project(&main, &[&extra]);

        assert!(check_writable(&p, &main.join("a.txt")).is_ok());
        assert!(check_writable(&p, &main.join("new/deep/b.txt")).is_ok());
        assert!(check_writable(&p, &extra.join("c.txt")).is_ok());
        assert!(check_writable(&p, &main).is_ok());
    }

    #[test]
    fn files_outside_project_folders_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let outside = tmp.path().join("main-sibling");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let p = project(&main, &[]);

        let err = check_writable(&p, &outside.join("x.txt")).unwrap_err();
        assert!(err.starts_with("Permission denied"), "{err}");
        assert!(check_writable(&p, &tmp.path().join("new/x.txt")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "").unwrap();
        std::os::unix::fs::symlink(&outside, main.join("link-dir")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), main.join("link-file")).unwrap();
        let p = project(&main, &[]);

        assert!(check_writable(&p, &main.join("link-file")).is_err());
        assert!(check_writable(&p, &main.join("link-dir/secret.txt")).is_err());
        assert!(check_writable(&p, &main.join("link-dir/new/x.txt")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_project_folder_is_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let p = project(&link, &[]);

        assert!(check_writable(&p, &link.join("a.txt")).is_ok());
        assert!(check_writable(&p, &real.join("a.txt")).is_ok());
    }
}
