use serde::Deserialize;
use serde_json::Value;
use telehand_proto::ToolOutput;

use crate::{Project, path};

#[derive(Deserialize)]
struct Args {
    path: String,
}

pub async fn run(project: &Project, args: Value) -> ToolOutput {
    let args: Args = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(e) => return ToolOutput::error(format!("Invalid arguments for read: {e}")),
    };
    let path = path::resolve(&project.main_folder, &args.path);
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => ToolOutput::text(text),
        Err(e) => ToolOutput::error(format!("Could not read file: {}: {e}", args.path)),
    }
}
