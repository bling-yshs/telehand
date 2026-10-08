//! The `grep` tool, following pi's `grep.ts`: searches file contents with
//! ripgrep (`rg`, expected on PATH).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

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

/// Matching lines longer than this many characters are cut.
const MAX_LINE_LENGTH: usize = 500;
const DEFAULT_LIMIT: f64 = 100.0;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    ignore_case: Option<bool>,
    #[serde(default)]
    literal: Option<bool>,
    #[serde(default)]
    context: Option<f64>,
    #[serde(default)]
    limit: Option<f64>,
}

struct Match {
    file: PathBuf,
    line_number: u64,
    line_text: Option<String>,
}

pub async fn run(project: &Project, args: Value, cancel: &CancellationToken) -> ToolOutput {
    let args: Args = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(e) => return ToolOutput::error(format!("Invalid arguments for grep: {e}")),
    };
    if cancel.is_cancelled() {
        return ToolOutput::error("Operation aborted");
    }
    let search_path = path::resolve(
        &project.main_folder,
        args.path
            .as_deref()
            .filter(|p| !p.is_empty())
            .unwrap_or("."),
    );
    let is_directory = match tokio::fs::metadata(&search_path).await {
        Ok(meta) => meta.is_dir(),
        Err(_) => {
            return ToolOutput::error(format!("Path not found: {}", search_path.display()));
        }
    };
    let context = args.context.filter(|c| *c > 0.0).map_or(0, |c| c as u64);
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).max(1.0);

    let mut cmd = Command::new("rg");
    cmd.args(["--json", "--line-number", "--color=never", "--hidden"]);
    if args.ignore_case == Some(true) {
        cmd.arg("--ignore-case");
    }
    if args.literal == Some(true) {
        cmd.arg("--fixed-strings");
    }
    if let Some(glob) = &args.glob {
        cmd.arg("--glob").arg(glob);
    }
    cmd.arg("--").arg(&args.pattern).arg(&search_path);

    let mut matches = Vec::new();
    let mut match_count = 0.0;
    let mut limit_reached = false;
    let finished = search::stream_lines(cmd, cancel, |line| {
        if line.trim().is_empty() || match_count >= limit {
            return true;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return true;
        };
        if event["type"] == "match" {
            match_count += 1.0;
            let data = &event["data"];
            if let (Some(file), Some(line_number)) =
                (data["path"]["text"].as_str(), data["line_number"].as_u64())
            {
                matches.push(Match {
                    file: PathBuf::from(file),
                    line_number,
                    line_text: data["lines"]["text"].as_str().map(str::to_string),
                });
            }
            if match_count >= limit {
                limit_reached = true;
                return false;
            }
        }
        true
    })
    .await;
    match finished {
        Err(e) => return ToolOutput::error(format!("Failed to run ripgrep: {e}")),
        Ok(Finished::Aborted) => return ToolOutput::error("Operation aborted"),
        Ok(Finished::Exited { code, stderr }) if code != Some(0) && code != Some(1) => {
            let message = stderr.trim();
            return ToolOutput::error(if message.is_empty() {
                format!("ripgrep exited with code {}", search::code_text(code))
            } else {
                message.to_string()
            });
        }
        Ok(_) => {}
    }
    if match_count == 0.0 {
        return ToolOutput::text("No matches found");
    }

    let display_path = |file: &Path| -> String {
        if is_directory && let Ok(relative) = file.strip_prefix(&search_path) {
            let relative = relative.to_string_lossy().replace('\\', "/");
            if !relative.is_empty() {
                return relative;
            }
        }
        file.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let mut files: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut lines_truncated = false;
    let mut output_lines = Vec::new();
    for m in &matches {
        let shown = display_path(&m.file);
        if let (0, Some(text)) = (context, &m.line_text) {
            let text = text.replace("\r\n", "\n").replace('\r', "");
            let text = text.strip_suffix('\n').unwrap_or(&text);
            let (text, cut) = truncate_line(text);
            lines_truncated |= cut;
            output_lines.push(format!("{shown}:{}: {text}", m.line_number));
            continue;
        }
        let lines = files
            .entry(m.file.clone())
            .or_insert_with(|| file_lines(&m.file));
        if lines.is_empty() {
            output_lines.push(format!("{shown}:{}: (unable to read file)", m.line_number));
            continue;
        }
        let total = lines.len() as u64;
        let (start, end) = if context > 0 {
            (
                m.line_number.saturating_sub(context).max(1),
                (m.line_number + context).min(total),
            )
        } else {
            (m.line_number, m.line_number)
        };
        for current in start..=end {
            let line = lines
                .get(current as usize - 1)
                .map(String::as_str)
                .unwrap_or("");
            let (text, cut) = truncate_line(&line.replace('\r', ""));
            lines_truncated |= cut;
            if current == m.line_number {
                output_lines.push(format!("{shown}:{current}: {text}"));
            } else {
                output_lines.push(format!("{shown}-{current}- {text}"));
            }
        }
    }

    // The match limit already caps the rows; only bytes are limited here.
    let truncation = truncate_head(&output_lines.join("\n"), usize::MAX, DEFAULT_MAX_BYTES);
    let mut output = truncation.content;
    let mut notices = Vec::new();
    if limit_reached {
        notices.push(format!(
            "{limit} matches limit reached. Use limit={} for more, or refine pattern",
            limit * 2.0
        ));
    }
    if truncation.truncated_by.is_some() {
        notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
    }
    if lines_truncated {
        notices.push(format!(
            "Some lines truncated to {MAX_LINE_LENGTH} chars. Use read tool to see full lines"
        ));
    }
    if !notices.is_empty() {
        output.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }
    ToolOutput::text(output)
}

/// The lines of a file, for context lines; empty if it cannot be read.
fn file_lines(file: &Path) -> Vec<String> {
    match std::fs::read(file) {
        Ok(bytes) => String::from_utf8_lossy(&bytes)
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .split('\n')
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Cut a line to [`MAX_LINE_LENGTH`] characters (pi's `truncateLine`).
fn truncate_line(line: &str) -> (String, bool) {
    match line.char_indices().nth(MAX_LINE_LENGTH) {
        None => (line.to_string(), false),
        Some((end, _)) => (format!("{}... [truncated]", &line[..end]), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lines_are_cut() {
        assert_eq!(truncate_line("short"), ("short".to_string(), false));
        let long = "é".repeat(MAX_LINE_LENGTH + 1);
        let (text, cut) = truncate_line(&long);
        assert!(cut);
        assert_eq!(
            text,
            format!("{}... [truncated]", "é".repeat(MAX_LINE_LENGTH))
        );
    }
}
