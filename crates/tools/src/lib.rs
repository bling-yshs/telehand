//! Tools executed on the runner. Behavior follows the pi coding agent.

use std::path::PathBuf;

use serde_json::Value;
use telehand_proto::{ToolOutput, tool_defs};
use tokio_util::sync::CancellationToken;

mod bash;
mod edit;
mod edit_diff;
mod find;
mod grep;
mod image;
mod path;
mod queue;
mod read;
mod scope;
mod search;
mod shell;
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

/// Run `tool` with `args` in the context of `project` (`None`: the selected
/// project is not on this runner). Cancelling `cancel` means the caller gave
/// up: a command still waited on is killed.
pub async fn execute(
    project: Option<&Project>,
    tool: &str,
    args: Value,
    cancel: &CancellationToken,
) -> ToolOutput {
    // Commands that already run do not need the project.
    match tool {
        tool_defs::BASH_RESULT => return bash::result(args, cancel).await,
        tool_defs::BASH_KILL => return bash::kill(args).await,
        _ => {}
    }
    let Some(project) = project else {
        return ToolOutput::error("The selected project does not exist on the runner.");
    };
    match tool {
        tool_defs::READ => read::run(project, args).await,
        tool_defs::WRITE => write::run(project, args).await,
        tool_defs::EDIT => edit::run(project, args).await,
        tool_defs::BASH => bash::run(project, args, cancel).await,
        tool_defs::GREP => grep::run(project, args, cancel).await,
        tool_defs::FIND => find::run(project, args, cancel).await,
        _ => ToolOutput::error(format!("Unknown tool: {tool}")),
    }
}
