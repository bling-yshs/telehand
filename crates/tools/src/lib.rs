//! File tools executed on the runner. Behavior follows the pi coding agent.

use std::path::PathBuf;

use serde_json::Value;
use telehand_proto::{ToolOutput, tool_defs};

mod edit;
mod edit_diff;
mod image;
mod path;
mod queue;
mod read;
mod scope;
mod truncate;
mod write;

/// The folders of the project a tool call operates on.
#[derive(Debug, Clone)]
pub struct Project {
    /// Relative paths resolve against this folder.
    pub main_folder: PathBuf,
    /// Additional folders that `write` and `edit` may modify.
    pub extra_folders: Vec<PathBuf>,
}

/// Run `tool` with `args` in the context of `project`.
pub async fn execute(project: &Project, tool: &str, args: Value) -> ToolOutput {
    match tool {
        tool_defs::READ => read::run(project, args).await,
        tool_defs::WRITE => write::run(project, args).await,
        tool_defs::EDIT => edit::run(project, args).await,
        _ => ToolOutput::error(format!("Unknown tool: {tool}")),
    }
}
