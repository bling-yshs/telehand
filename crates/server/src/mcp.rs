//! The MCP server exposed to agents at `/mcp/{key}`.
//!
//! The server keeps no current project: every tool that works in a project
//! names it in its `project` argument, so agents sharing one MCP URL (and
//! clients of the stateless MCP protocol, which have no session) never affect
//! each other.

use std::{sync::Weak, time::Duration};

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
        Tool,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use telehand_proto::{Content, ProjectInfo, ToolOutput, tool_defs};
use tokio_util::sync::CancellationToken;

use crate::state::{AppState, CallError, TOOL_TIMEOUT};

/// Time for the runner to answer after `wait` has passed.
const BASH_REPORT_GRACE: Duration = Duration::from_secs(30);

pub const LIST_PROJECT: &str = "list_project";

/// The argument naming the project a tool works in.
const PROJECT_ARG: &str = "project";

pub const RUNNER_OFFLINE: &str = "Runner offline: no runner is connected for this key. Start `telehand-runner run` on the runner machine.";

#[derive(Clone)]
pub struct McpHandler {
    key: String,
    state: Weak<AppState>,
}

impl McpHandler {
    pub fn new(key: String, state: Weak<AppState>) -> Self {
        Self { key, state }
    }
}

fn schema(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => JsonObject::new(),
    }
}

fn tools() -> Vec<Tool> {
    let mut tools = vec![Tool::new(
        LIST_PROJECT,
        "List the projects registered on the runner, with their main folder and extra folders. read, write, edit, bash, grep and find take the name of the project to work in as their `project` argument: relative paths resolve against its main folder, write/edit may only modify files inside its folders, and bash runs commands in its main folder.",
        schema(json!({"type": "object", "properties": {}})),
    )];
    tools.extend(tool_defs::runner_tools().into_iter().map(|def| {
        let mut input_schema = def.input_schema;
        if !is_task_tool(def.name) {
            add_project_arg(&mut input_schema);
        }
        Tool::new(def.name, def.description, schema(input_schema))
    }));
    tools
}

/// Add the required `project` argument to a runner tool's input schema.
fn add_project_arg(input_schema: &mut Value) {
    if let Some(properties) = input_schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        properties.insert(
            PROJECT_ARG.to_string(),
            json!({
                "type": "string",
                "description": "Name of the project to work in (see list_project)"
            }),
        );
    }
    if let Some(required) = input_schema
        .get_mut("required")
        .and_then(Value::as_array_mut)
    {
        required.insert(0, json!(PROJECT_ARG));
    }
}

fn to_result(output: ToolOutput) -> CallToolResult {
    let blocks = output
        .content
        .into_iter()
        .map(|content| match content {
            Content::Text { text } => ContentBlock::text(text),
            Content::Image { data, mime_type } => ContentBlock::image(data, mime_type),
        })
        .collect();
    if output.is_error {
        CallToolResult::error(blocks)
    } else {
        CallToolResult::success(blocks)
    }
}

fn describe(project: &ProjectInfo) -> String {
    let mut text = format!("{}\n  main folder: {}", project.name, project.main_folder);
    if !project.extra_folders.is_empty() {
        text.push_str(&format!(
            "\n  extra folders: {}",
            project.extra_folders.join(", ")
        ));
    }
    text
}

/// How long to wait for the runner. `bash` and `bash_result` answer after at
/// most `wait` seconds (a command still running by then continues as a task).
fn response_timeout(tool: &str, args: &Value) -> Duration {
    if tool != tool_defs::BASH && tool != tool_defs::BASH_RESULT {
        return TOOL_TIMEOUT;
    }
    let wait = args
        .get("wait")
        .and_then(Value::as_f64)
        .unwrap_or(tool_defs::BASH_DEFAULT_WAIT_SECONDS)
        .min(tool_defs::BASH_MAX_WAIT_SECONDS);
    Duration::try_from_secs_f64(wait)
        .ok()
        .and_then(|wait| wait.checked_add(BASH_REPORT_GRACE))
        .unwrap_or(TOOL_TIMEOUT)
}

/// Tools about commands that already run, which take no project.
fn is_task_tool(tool: &str) -> bool {
    tool == tool_defs::BASH_RESULT || tool == tool_defs::BASH_KILL
}

fn available(projects: &[ProjectInfo]) -> String {
    if projects.is_empty() {
        return "(none)".to_string();
    }
    let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
    names.join(", ")
}

fn project_arg_missing(projects: &[ProjectInfo]) -> ToolOutput {
    ToolOutput::error(format!(
        "Missing required argument: project. Pass the name of the project to work in. Available projects: {}",
        available(projects)
    ))
}

fn project_not_found(name: &str, projects: &[ProjectInfo]) -> ToolOutput {
    ToolOutput::error(format!(
        "Project '{name}' not found on the runner. Available projects: {}",
        available(projects)
    ))
}

impl McpHandler {
    fn list_project(&self, state: &AppState) -> ToolOutput {
        let Some(projects) = state.runner_projects(&self.key) else {
            return ToolOutput::error(RUNNER_OFFLINE);
        };
        if projects.is_empty() {
            return ToolOutput::text(
                "No projects are registered on the runner. Add one on the runner machine with `telehand-runner project add <dir>`.",
            );
        }
        let lines: Vec<String> = projects
            .iter()
            .map(|p| format!("- {}", describe(p)))
            .collect();
        ToolOutput::text(lines.join("\n"))
    }

    async fn runner_tool(
        &self,
        state: &AppState,
        tool: &str,
        mut args: Value,
        cancel: &CancellationToken,
    ) -> ToolOutput {
        let Some(projects) = state.runner_projects(&self.key) else {
            return ToolOutput::error(RUNNER_OFFLINE);
        };
        // The project travels in the request, not in the tool's arguments.
        let project = args
            .as_object_mut()
            .and_then(|args| args.remove(PROJECT_ARG));
        let project = if is_task_tool(tool) {
            String::new()
        } else {
            let Some(name) = project.as_ref().and_then(Value::as_str) else {
                return project_arg_missing(&projects);
            };
            if !projects.iter().any(|p| p.name == name) {
                return project_not_found(name, &projects);
            }
            name.to_string()
        };
        let timeout = response_timeout(tool, &args);
        match state
            .call_runner(&self.key, &project, tool, args, Some(timeout), cancel)
            .await
        {
            Ok(output) => output,
            Err(CallError::Offline) | Err(CallError::Disconnected) => {
                ToolOutput::error(RUNNER_OFFLINE)
            }
            Err(CallError::Timeout(limit)) => ToolOutput::error(format!(
                "Timed out after {} seconds waiting for the runner to respond.",
                limit.as_secs()
            )),
            Err(CallError::Cancelled) => ToolOutput::error("Cancelled"),
        }
    }
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("telehand", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(state) = self.state.upgrade() else {
            return Err(ErrorData::internal_error("server is shutting down", None));
        };
        let args = Value::Object(request.arguments.unwrap_or_default());
        let output = match request.name.as_ref() {
            LIST_PROJECT => self.list_project(&state),
            name if tool_defs::runner_tools().iter().any(|def| def.name == name) => {
                self.runner_tool(&state, name, args, &context.ct).await
            }
            name => ToolOutput::error(format!("Unknown tool: {name}")),
        };
        Ok(to_result(output).into())
    }
}
