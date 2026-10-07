//! The `find` tool, following pi's `find.ts`: finds files by glob pattern
//! with fd (expected on PATH).

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use telehand_proto::ToolOutput;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{
    Project, path,
    search::{self, Finished},
    truncate::{DEFAULT_MAX_BYTES, format_size, truncate_head},
};

const DEFAULT_LIMIT: f64 = 1000.0;

#[derive(Deserialize)]
struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    limit: Option<f64>,
}

pub async fn run(project: &Project, args: Value, cancel: &CancellationToken) -> ToolOutput {
    let args: Args = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(e) => return ToolOutput::error(format!("Invalid arguments for find: {e}")),
    };
    if cancel.is_cancelled() {
        return ToolOutput::error("Operation aborted");
    }
    let search_path = path::resolve(
        &project.main_folder,
        args.path.as_deref().filter(|p| !p.is_empty()).unwrap_or("."),
    );
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT);

    let mut cmd = Command::new("fd");
    cmd.args(["--glob", "--color=never", "--hidden"]);
    // fd ignores .gitignore outside git repositories unless told otherwise;
    // inside one, its default stops parent rules at nested repositories.
    if !search_path.ancestors().any(|dir| dir.join(".git").exists()) {
        cmd.arg("--no-require-git");
    }
    cmd.arg("--max-results").arg(limit.to_string());
    let (full_path, pattern) = fd_pattern(&args.pattern, cfg!(windows));
    if full_path {
        cmd.arg("--full-path");
    }
    cmd.arg("--").arg(pattern).arg(&search_path);

    let mut lines = Vec::new();
    let finished = search::stream_lines(cmd, cancel, |line| {
        lines.push(line.to_string());
        true
    })
    .await;
    let output = lines.join("\n");
    match finished {
        Err(e) => return ToolOutput::error(format!("Failed to run fd: {e}")),
        Ok(Finished::Aborted) => return ToolOutput::error("Operation aborted"),
        Ok(Finished::Exited { code, stderr }) if code != Some(0) && output.is_empty() => {
            let message = stderr.trim();
            return ToolOutput::error(if message.is_empty() {
                format!("fd exited with code {}", search::code_text(code))
            } else {
                message.to_string()
            });
        }
        Ok(_) => {}
    }
    if output.is_empty() {
        return ToolOutput::text("No files found matching pattern");
    }

    let results: Vec<String> = lines
        .iter()
        .map(|line| line.trim_end_matches('\r').trim())
        .filter(|line| !line.is_empty())
        .map(|line| relativize(line, &search_path))
        .collect();
    let limit_reached = results.len() as f64 >= limit;
    let truncation = truncate_head(&results.join("\n"), usize::MAX, DEFAULT_MAX_BYTES);
    let mut output = truncation.content;
    let mut notices = Vec::new();
    if limit_reached {
        notices.push(format!(
            "{limit} results limit reached. Use limit={} for more, or refine pattern",
            limit * 2.0
        ));
    }
    if truncation.truncated_by.is_some() {
        notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
    }
    if !notices.is_empty() {
        output.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }
    ToolOutput::text(output)
}

/// Whether fd must match full paths, and the pattern to give it. fd's glob
/// matches the file name unless `--full-path` is set; then it matches the
/// absolute path, so a pattern with a `/` needs a leading `**/` (and, on
/// Windows, either separator).
fn fd_pattern(pattern: &str, windows: bool) -> (bool, String) {
    if !pattern.contains('/') {
        return (false, pattern.to_string());
    }
    let mut effective = pattern.to_string();
    if !pattern.starts_with('/') && !pattern.starts_with("**/") && pattern != "**" {
        effective = format!("**/{pattern}");
    }
    if windows {
        effective = effective.replace('/', r"[/\\]");
    }
    (true, effective)
}

/// A result relative to the search directory, with `/` separators (keeping
/// the trailing separator fd puts after directories).
fn relativize(result: &str, search_path: &Path) -> String {
    let trailing_separator = result.ends_with(std::path::MAIN_SEPARATOR)
        || (cfg!(windows) && result.ends_with('/'));
    let path = Path::new(result);
    let relative = match path.strip_prefix(search_path) {
        Ok(relative) if path.is_absolute() => relative.to_string_lossy().into_owned(),
        _ => result.to_string(),
    };
    let relative = if cfg!(windows) {
        relative.replace('\\', "/")
    } else {
        relative
    };
    if trailing_separator && !relative.ends_with('/') {
        format!("{relative}/")
    } else {
        relative
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_patterns_match_full_paths() {
        assert_eq!(fd_pattern("*.rs", false), (false, "*.rs".to_string()));
        assert_eq!(
            fd_pattern("src/**/*.rs", false),
            (true, "**/src/**/*.rs".to_string())
        );
        assert_eq!(fd_pattern("**/a/b", false), (true, "**/a/b".to_string()));
        assert_eq!(
            fd_pattern("src/*.rs", true),
            (true, r"**[/\\]src[/\\]*.rs".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn results_are_relative_to_the_search_path() {
        let root = Path::new("/work/project");
        assert_eq!(relativize("/work/project/src/a.rs", root), "src/a.rs");
        assert_eq!(relativize("/work/project/src/", root), "src/");
        assert_eq!(relativize("src/a.rs", root), "src/a.rs");
    }
}
