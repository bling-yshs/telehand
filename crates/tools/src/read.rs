//! The `read` tool, following pi's `read.ts`.

use std::{io::ErrorKind, path::Path};

use serde::Deserialize;
use serde_json::Value;
use telehand_proto::ToolOutput;

use crate::{
    Project, path,
    truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncatedBy, format_size, truncate_head},
};

#[derive(Deserialize)]
struct Args {
    path: String,
    offset: Option<f64>,
    limit: Option<f64>,
}

/// Format a number the way JavaScript prints it in template strings.
fn js_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The error message Node.js gives for a failed `fs.access`, which pi surfaces as-is.
fn access_error(error: &std::io::Error, path: &Path) -> String {
    let path = path.display();
    match error.kind() {
        ErrorKind::NotFound => format!("ENOENT: no such file or directory, access '{path}'"),
        ErrorKind::PermissionDenied => format!("EACCES: permission denied, access '{path}'"),
        _ => format!("{error}, access '{path}'"),
    }
}

pub async fn run(project: &Project, args: Value) -> ToolOutput {
    let args: Args = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(e) => return ToolOutput::error(format!("Invalid arguments for read: {e}")),
    };
    let path = path::resolve_read(&project.main_folder, &args.path);
    let bytes = match tokio::fs::metadata(&path).await {
        Err(e) => return ToolOutput::error(access_error(&e, &path)),
        Ok(meta) if meta.is_dir() => {
            return ToolOutput::error("EISDIR: illegal operation on a directory, read");
        }
        Ok(_) => match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(e) => return ToolOutput::error(access_error(&e, &path)),
        },
    };
    match read_text(&bytes, &args) {
        Ok(text) => ToolOutput::text(text),
        Err(message) => ToolOutput::error(message),
    }
}

fn read_text(bytes: &[u8], args: &Args) -> Result<String, String> {
    let text = String::from_utf8_lossy(bytes);
    let all_lines: Vec<&str> = text.split('\n').collect();
    let total_file_lines = all_lines.len();

    // Offset is 1-indexed.
    let start_line = match args.offset {
        Some(offset) if offset != 0.0 => (offset - 1.0).max(0.0) as usize,
        _ => 0,
    };
    let start_line_display = start_line + 1;
    if start_line >= all_lines.len() {
        return Err(format!(
            "Offset {} is beyond end of file ({} lines total)",
            js_number(args.offset.unwrap_or(0.0)),
            all_lines.len()
        ));
    }

    let mut user_limited_lines = None;
    let selected = match args.limit {
        Some(limit) => {
            let end_line = (start_line + limit.max(0.0) as usize).min(all_lines.len());
            user_limited_lines = Some(end_line - start_line);
            all_lines[start_line..end_line].join("\n")
        }
        None => all_lines[start_line..].join("\n"),
    };

    let truncation = truncate_head(&selected, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
    let output = if truncation.first_line_exceeds_limit {
        let first_line_size = format_size(all_lines[start_line].len());
        format!(
            "[Line {start_line_display} is {first_line_size}, exceeds {} limit. Use bash: sed -n '{start_line_display}p' {} | head -c {DEFAULT_MAX_BYTES}]",
            format_size(DEFAULT_MAX_BYTES),
            args.path
        )
    } else if let Some(truncated_by) = truncation.truncated_by {
        let end_line_display = start_line_display + truncation.output_lines - 1;
        let next_offset = end_line_display + 1;
        match truncated_by {
            TruncatedBy::Lines => format!(
                "{}\n\n[Showing lines {start_line_display}-{end_line_display} of {total_file_lines}. Use offset={next_offset} to continue.]",
                truncation.content
            ),
            TruncatedBy::Bytes => format!(
                "{}\n\n[Showing lines {start_line_display}-{end_line_display} of {total_file_lines} ({} limit). Use offset={next_offset} to continue.]",
                truncation.content,
                format_size(DEFAULT_MAX_BYTES)
            ),
        }
    } else if let Some(limited) = user_limited_lines
        && start_line + limited < all_lines.len()
    {
        let remaining = all_lines.len() - (start_line + limited);
        let next_offset = start_line + limited + 1;
        format!(
            "{}\n\n[{remaining} more lines in file. Use offset={next_offset} to continue.]",
            truncation.content
        )
    } else {
        truncation.content
    };
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;
    use telehand_proto::Content;

    use super::*;

    fn project(dir: &Path) -> Project {
        Project {
            main_folder: dir.to_path_buf(),
            extra_folders: Vec::new(),
        }
    }

    fn text(output: &ToolOutput) -> &str {
        match &output.content[0] {
            Content::Text { text } => text,
            other => panic!("expected text, got {other:?}"),
        }
    }

    async fn read(dir: &Path, args: Value) -> ToolOutput {
        run(&project(dir), args).await
    }

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    #[tokio::test]
    async fn reads_whole_small_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        let out = read(dir.path(), json!({"path": "a.txt"})).await;
        assert!(!out.is_error);
        assert_eq!(text(&out), "one\ntwo\n");
    }

    #[tokio::test]
    async fn offset_and_limit_select_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), numbered(10)).unwrap();
        let out = read(
            dir.path(),
            json!({"path": "a.txt", "offset": 3, "limit": 2}),
        )
        .await;
        assert_eq!(
            text(&out),
            "line 3\nline 4\n\n[7 more lines in file. Use offset=5 to continue.]"
        );
        let out = read(dir.path(), json!({"path": "a.txt", "offset": 9})).await;
        assert_eq!(text(&out), "line 9\nline 10\n");
    }

    #[tokio::test]
    async fn offset_beyond_end_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a\nb").unwrap();
        let out = read(dir.path(), json!({"path": "a.txt", "offset": 5})).await;
        assert!(out.is_error);
        assert_eq!(text(&out), "Offset 5 is beyond end of file (2 lines total)");
    }

    #[tokio::test]
    async fn long_files_are_truncated_by_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), numbered(2500)).unwrap();
        let out = read(dir.path(), json!({"path": "a.txt"})).await;
        let text = text(&out);
        assert!(text.starts_with("line 1\n"));
        assert!(text.ends_with(
            "line 2000\n\n[Showing lines 1-2000 of 2501. Use offset=2001 to continue.]"
        ));
    }

    #[tokio::test]
    async fn large_files_are_truncated_by_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let line = "x".repeat(99);
        let content: String = (0..1000).map(|_| format!("{line}\n")).collect();
        std::fs::write(dir.path().join("a.txt"), content).unwrap();
        let out = read(dir.path(), json!({"path": "a.txt", "offset": 1})).await;
        // 512 lines of 100 bytes (99 chars + newline) fit in 50KB; the 513th does not.
        assert!(text(&out).ends_with(
            "\n\n[Showing lines 1-512 of 1001 (50.0KB limit). Use offset=513 to continue.]"
        ));
    }

    #[tokio::test]
    async fn oversized_first_line_points_to_bash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "y".repeat(60 * 1024)).unwrap();
        let out = read(dir.path(), json!({"path": "a.txt"})).await;
        assert_eq!(
            text(&out),
            "[Line 1 is 60.0KB, exceeds 50.0KB limit. Use bash: sed -n '1p' a.txt | head -c 51200]"
        );
    }

    #[tokio::test]
    async fn missing_files_and_directories_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let out = read(dir.path(), json!({"path": "nope.txt"})).await;
        assert!(out.is_error);
        let expected: PathBuf = dir.path().join("nope.txt");
        assert_eq!(
            text(&out),
            format!(
                "ENOENT: no such file or directory, access '{}'",
                expected.display()
            )
        );
        let out = read(dir.path(), json!({"path": "."})).await;
        assert_eq!(text(&out), "EISDIR: illegal operation on a directory, read");
    }
}
