//! One-line descriptions of tool calls and their results, for the runner's
//! console log.

use serde_json::Value;
use telehand_proto::{Content, ToolOutput, tool_defs};

const MAX_CHARS: usize = 100;

/// The key argument of a call to `tool`: its path, command, pattern or task ID.
/// File contents and edit texts are left out.
pub fn request(tool: &str, args: &Value) -> String {
    let arg = |name: &str| args.get(name).and_then(Value::as_str);
    let summary = match tool {
        tool_defs::READ | tool_defs::WRITE | tool_defs::EDIT => {
            arg("path").unwrap_or("").to_string()
        }
        tool_defs::BASH => {
            let command = arg("command").unwrap_or("");
            let mut lines = command.lines();
            let first = lines.next().unwrap_or("").to_string();
            if lines.next().is_some() {
                format!("{first} ...")
            } else {
                first
            }
        }
        tool_defs::BASH_RESULT | tool_defs::BASH_KILL => arg("task_id").unwrap_or("").to_string(),
        tool_defs::GREP | tool_defs::FIND => match (arg("pattern"), arg("path")) {
            (Some(pattern), Some(path)) => format!("{pattern} in {path}"),
            (pattern, _) => pattern.unwrap_or("").to_string(),
        },
        _ => String::new(),
    };
    truncate(&summary)
}

/// How a call ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// A bash command outlived its wait and keeps running as this task.
    Running(String),
    /// The error's last line, where tools put the reason
    /// (e.g. "Command exited with code 1").
    Error(String),
}

pub fn outcome(output: &ToolOutput) -> Outcome {
    let text = output
        .content
        .iter()
        .find_map(|c| match c {
            Content::Text { text } => Some(text.as_str()),
            Content::Image { .. } => None,
        })
        .unwrap_or("");
    if output.is_error {
        let last = text
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("error");
        return Outcome::Error(truncate(last.trim()));
    }
    // A bash command outliving its wait; see bash::run.
    if text.starts_with("Command is still running")
        && let Some((_, rest)) = text.split_once("Task ID: ")
    {
        let id = rest.lines().next().unwrap_or("").trim();
        return Outcome::Running(id.to_string());
    }
    Outcome::Ok
}

fn truncate(s: &str) -> String {
    match s.char_indices().nth(MAX_CHARS) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn request_shows_the_key_argument() {
        assert_eq!(
            request("read", &json!({"path": "src/main.rs"})),
            "src/main.rs"
        );
        assert_eq!(
            request("write", &json!({"path": "a.txt", "content": "secret"})),
            "a.txt"
        );
        assert_eq!(
            request("bash", &json!({"command": "cargo build"})),
            "cargo build"
        );
        assert_eq!(request("bash", &json!({"command": "cd x\nls"})), "cd x ...");
        assert_eq!(
            request("grep", &json!({"pattern": "foo", "path": "src"})),
            "foo in src"
        );
        assert_eq!(request("find", &json!({"pattern": "*.rs"})), "*.rs");
        assert_eq!(
            request("bash_kill", &json!({"task_id": "bash-1"})),
            "bash-1"
        );
        assert_eq!(request("read", &json!({})), "");
    }

    #[test]
    fn request_is_truncated() {
        let long = "x".repeat(150);
        assert_eq!(
            request("bash", &json!({ "command": long })),
            format!("{}...", "x".repeat(MAX_CHARS))
        );
    }

    #[test]
    fn outcome_of_errors_is_their_last_line() {
        let out = ToolOutput::error("out\n\n\nCommand exited with code 3");
        assert_eq!(
            outcome(&out),
            Outcome::Error("Command exited with code 3".to_string())
        );
    }

    #[test]
    fn outcome_of_a_command_still_running_names_its_task() {
        let out = ToolOutput::text(
            "Command is still running after 50 seconds. Task ID: bash-7\nCall bash_result ...",
        );
        assert_eq!(outcome(&out), Outcome::Running("bash-7".to_string()));
        assert_eq!(outcome(&ToolOutput::text("hello")), Outcome::Ok);
    }
}
