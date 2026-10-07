//! Finding the shell for the `bash` tool, following pi's `utils/shell.ts`.

use std::path::{Path, PathBuf};

/// Overrides the bash executable on every platform.
pub const BASH_PATH_ENV: &str = "TELEHAND_GIT_BASH_PATH";

const GIT_BASH: &str = r"C:\Program Files\Git\bin\bash.exe";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellConfig {
    pub shell: PathBuf,
    pub args: Vec<&'static str>,
    /// Pass the command on stdin instead of as an argument (WSL's legacy bash.exe).
    pub command_via_stdin: bool,
}

/// The shell to run commands with.
///
/// 1. `TELEHAND_GIT_BASH_PATH`, if set.
/// 2. Windows: `C:\Program Files\Git\bin\bash.exe`, then `bash.exe` on PATH.
/// 3. Elsewhere: `/bin/bash`, then `bash` on PATH, then `sh`.
pub fn shell_config() -> Result<ShellConfig, String> {
    let custom = std::env::var_os(BASH_PATH_ENV).filter(|value| !value.is_empty());
    resolve(custom.as_deref().map(Path::new))
}

fn resolve(custom: Option<&Path>) -> Result<ShellConfig, String> {
    if let Some(path) = custom {
        if path.exists() {
            return Ok(bash_config(path));
        }
        return Err(format!(
            "{BASH_PATH_ENV} points to a shell that was not found: {}",
            path.display()
        ));
    }

    if cfg!(windows) {
        if Path::new(GIT_BASH).exists() {
            return Ok(bash_config(Path::new(GIT_BASH)));
        }
        if let Some(path) = find_on_path("bash.exe") {
            return Ok(bash_config(&path));
        }
        return Err(format!(
            "No bash shell found. Options:\n  1. Install Git for Windows: https://git-scm.com/download/win\n  2. Add your bash to PATH (Cygwin, MSYS2, etc.)\n  3. Set {BASH_PATH_ENV} to the path of bash.exe\n\nSearched Git Bash in:\n  {GIT_BASH}"
        ));
    }

    if Path::new("/bin/bash").exists() {
        return Ok(bash_config(Path::new("/bin/bash")));
    }
    if let Some(path) = find_on_path("bash") {
        return Ok(bash_config(&path));
    }
    Ok(ShellConfig {
        shell: PathBuf::from("sh"),
        args: vec!["-c"],
        command_via_stdin: false,
    })
}

fn bash_config(shell: &Path) -> ShellConfig {
    if is_legacy_wsl_bash(shell) {
        ShellConfig {
            shell: shell.to_path_buf(),
            args: vec!["-s"],
            command_via_stdin: true,
        }
    } else {
        ShellConfig {
            shell: shell.to_path_buf(),
            args: vec!["-c"],
            command_via_stdin: false,
        }
    }
}

/// `C:\Windows\System32\bash.exe` (or `Sysnative`): WSL's launcher, which
/// mangles commands passed with `-c`.
fn is_legacy_wsl_bash(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let bytes = normalized.as_bytes();
    bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && &bytes[1..3] == b":\\"
        && matches!(
            &normalized[3..],
            "windows\\system32\\bash.exe" | "windows\\sysnative\\bash.exe"
        )
}

fn find_on_path(executable: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(executable))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_shell_must_exist() {
        let error = resolve(Some(Path::new("/no/such/bash"))).unwrap_err();
        assert!(error.contains(BASH_PATH_ENV), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_custom_shell_is_used() {
        let config = resolve(Some(Path::new("/bin/sh"))).unwrap();
        assert_eq!(config.shell, PathBuf::from("/bin/sh"));
        assert_eq!(config.args, vec!["-c"]);
    }

    #[cfg(unix)]
    #[test]
    fn bash_is_found_by_default() {
        let config = resolve(None).unwrap();
        assert!(
            config.shell.ends_with("bash") || config.shell == Path::new("sh"),
            "{config:?}"
        );
        assert!(!config.command_via_stdin);
    }

    #[test]
    fn legacy_wsl_bash_reads_the_command_from_stdin() {
        assert!(is_legacy_wsl_bash(Path::new(r"C:\Windows\System32\bash.exe")));
        assert!(is_legacy_wsl_bash(Path::new("c:/windows/sysnative/BASH.EXE")));
        assert!(!is_legacy_wsl_bash(Path::new(GIT_BASH)));
        assert!(bash_config(Path::new(r"C:\Windows\System32\bash.exe")).command_via_stdin);
    }
}
