//! Path handling, following pi's `resolveToCwd` / `resolveReadPath`.

use std::path::{Component, Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

const NARROW_NO_BREAK_SPACE: char = '\u{202F}';

fn is_unicode_space(c: char) -> bool {
    matches!(
        c,
        '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Normalize a path as typed by the agent: unicode spaces become regular
/// spaces, a leading `@` is stripped, `~` expands to the home directory and
/// `file://` URLs become paths.
pub fn normalize(input: &str) -> PathBuf {
    let mut normalized: String = input
        .chars()
        .map(|c| if is_unicode_space(c) { ' ' } else { c })
        .collect();
    if let Some(rest) = normalized.strip_prefix('@') {
        normalized = rest.to_string();
    }
    if let Some(home) = home_dir() {
        if normalized == "~" {
            return home;
        }
        if let Some(rest) = normalized.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    if let Some(path) = file_url_to_path(&normalized) {
        return path;
    }
    PathBuf::from(normalized)
}

fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(percent_decode(path)))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(byte) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Lexically resolve `.` and `..` components, like Node's `path.resolve`.
pub fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Resolve a path given by the agent: relative paths resolve against the
/// project's main folder, absolute paths are used as-is.
pub fn resolve(main_folder: &Path, raw: &str) -> PathBuf {
    let path = normalize(raw);
    if path.is_absolute() {
        clean(&path)
    } else {
        clean(&main_folder.join(path))
    }
}

/// Like [`resolve`], but if the file does not exist, try the variants macOS
/// uses in screenshot file names.
pub fn resolve_read(main_folder: &Path, raw: &str) -> PathBuf {
    let resolved = resolve(main_folder, raw);
    if resolved.exists() {
        return resolved;
    }
    let text = resolved.to_string_lossy().into_owned();
    let nfd: String = text.nfd().collect();
    let candidates = [
        am_pm_variant(&text),
        nfd.clone(),
        text.replace('\'', "\u{2019}"),
        nfd.replace('\'', "\u{2019}"),
    ];
    candidates
        .into_iter()
        .filter(|candidate| *candidate != text)
        .map(PathBuf::from)
        .find(|candidate| candidate.exists())
        .unwrap_or(resolved)
}

/// macOS screenshot names put a narrow no-break space before AM/PM.
fn am_pm_variant(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(pos) = rest.find(' ') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let is_am_pm = after.len() >= 3
            && after.is_char_boundary(2)
            && matches!(after[..2].to_ascii_uppercase().as_str(), "AM" | "PM")
            && after[2..].starts_with('.');
        out.push(if is_am_pm { NARROW_NO_BREAK_SPACE } else { ' ' });
        rest = after;
    }
    out.push_str(rest);
    out
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
        assert_eq!(
            resolve(Path::new("/p/q"), "../r/./s"),
            PathBuf::from("/p/r/s")
        );
    }

    #[test]
    fn agent_path_spellings_are_normalized() {
        assert_eq!(
            resolve(Path::new("/p"), "@src/a.rs"),
            PathBuf::from("/p/src/a.rs")
        );
        assert_eq!(
            resolve(Path::new("/p"), "a\u{00A0}b\u{3000}c"),
            PathBuf::from("/p/a b c")
        );
        assert_eq!(
            resolve(Path::new("/p"), "file:///tmp/a%20b.txt"),
            PathBuf::from("/tmp/a b.txt")
        );
        if let Some(home) = home_dir() {
            assert_eq!(resolve(Path::new("/p"), "~"), home);
            assert_eq!(resolve(Path::new("/p"), "~/x"), home.join("x"));
        }
    }

    #[test]
    fn macos_screenshot_variants_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let actual = dir.path().join("Shot 9.41.33\u{202F}AM.png");
        std::fs::write(&actual, b"x").unwrap();
        assert_eq!(resolve_read(dir.path(), "Shot 9.41.33 AM.png"), actual);

        let curly = dir.path().join("Capture d\u{2019}e\u{301}cran.png");
        std::fs::write(&curly, b"x").unwrap();
        assert_eq!(resolve_read(dir.path(), "Capture d'\u{e9}cran.png"), curly);

        let missing = resolve_read(dir.path(), "missing.txt");
        assert_eq!(missing, dir.path().join("missing.txt"));
    }
}
