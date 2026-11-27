//! The MCP server exposed to agents at `/mcp/{key}`.
//!
//! Each MCP session gets its own [`McpHandler`], which holds that session's
//! current project: agents sharing one MCP URL select projects independently.

use std::{
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

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
pub const SELECT_PROJECT: &str = "select_project";
pub const CURRENT_PROJECT: &str = "current_project";

pub const RUNNER_OFFLINE: &str = "Runner offline: no runner is connected for this key. Start `telehand-runner run` on the runner machine.";

#[derive(Clone)]
pub struct McpHandler {
    key: String,
    state: Weak<AppState>,
    /// The project selected in this MCP session.
    current: Arc<Mutex<Option<String>>>,
}

impl McpHandler {
    /// A handler for a new MCP session, with no project selected.
    pub fn new(key: String, state: Weak<AppState>) -> Self {
        Self {
            key,
            state,
            current: Arc::default(),
        }
    }

    fn current(&self) -> Option<String> {
        self.current.lock().unwrap().clone()
    }
}

fn schema(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => JsonObject::new(),
    }
}

fn tools() -> Vec<Tool> {
    let mut tools = vec![
        Tool::new(
            LIST_PROJECT,
            "List the projects registered on the runner, with their main folder and extra folders, and mark the current project. The read, write, edit, bash, grep and find tools operate on the current project: relative paths resolve against its main folder, write/edit may only modify files inside its folders, and bash runs commands in its main folder.",
            schema(json!({"type": "object", "properties": {}})),
        ),
        Tool::new(
            SELECT_PROJECT,
            "Select the current project by name. read, write, edit, bash, grep and find operate on the current project until another one is selected. The selection belongs to this MCP session: other agents using the same MCP URL keep their own.",
            schema(json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Name of the project to select"}
                },
                "required": ["name"]
            })),
        ),
        Tool::new(
            CURRENT_PROJECT,
            "Show the current project: its name, main folder (base for relative paths and working directory of bash) and extra folders.",
            schema(json!({"type": "object", "properties": {}})),
        ),
    ];
    tools.extend(
        tool_defs::runner_tools()
            .into_iter()
            .map(|def| Tool::new(def.name, def.description, schema(def.input_schema))),
    );
    tools
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

/// Tools about commands that already run, which need no selected project.
fn is_task_tool(tool: &str) -> bool {
    tool == tool_defs::BASH_RESULT || tool == tool_defs::BASH_KILL
}

const NO_PROJECT_SELECTED: &str = "No project selected in this MCP session (it may be a new session, or the server restarted or the session expired). Use list_project to see the available projects, then select_project to choose one.";

fn no_project_selected() -> ToolOutput {
    ToolOutput::error(NO_PROJECT_SELECTED)
}

fn project_missing(name: &str) -> ToolOutput {
    ToolOutput::error(format!(
        "The selected project '{name}' no longer exists on the runner. Use list_project to see the available projects, then select_project to choose one."
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
        let current = self.current();
        let lines: Vec<String> = projects
            .iter()
            .map(|p| {
                let marker = if current.as_deref() == Some(p.name.as_str()) {
                    " (current)"
                } else {
                    ""
                };
                format!("- {}{marker}", describe(p))
            })
            .collect();
        ToolOutput::text(lines.join("\n"))
    }

    fn select_project(&self, state: &AppState, args: &Value) -> ToolOutput {
        let Some(projects) = state.runner_projects(&self.key) else {
            return ToolOutput::error(RUNNER_OFFLINE);
        };
        let Some(name) = args.get("name").and_then(Value::as_str) else {
            return ToolOutput::error("Missing required argument: name");
        };
        let Some(project) = projects.iter().find(|p| p.name == name) else {
            let available: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
            return ToolOutput::error(format!(
                "Project '{name}' not found. Available projects: {}",
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            ));
        };
        *self.current.lock().unwrap() = Some(name.to_string());
        ToolOutput::text(format!("Selected project {}", describe(project)))
    }

    fn current_project(&self, state: &AppState) -> ToolOutput {
        let Some(projects) = state.runner_projects(&self.key) else {
            return ToolOutput::error(RUNNER_OFFLINE);
        };
        let Some(current) = self.current() else {
            return ToolOutput::text(NO_PROJECT_SELECTED);
        };
        match projects.iter().find(|p| p.name == current) {
            Some(project) => ToolOutput::text(format!("Current project: {}", describe(project))),
            None => project_missing(&current),
        }
    }

    async fn runner_tool(
        &self,
        state: &AppState,
        tool: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> ToolOutput {
        let Some(projects) = state.runner_projects(&self.key) else {
            return ToolOutput::error(RUNNER_OFFLINE);
        };
        let current = self.current();
        if !is_task_tool(tool) {
            let Some(current) = &current else {
                return no_project_selected();
            };
            if !projects.iter().any(|p| &p.name == current) {
                return project_missing(current);
            }
        }
        let timeout = response_timeout(tool, &args);
        let project = current.unwrap_or_default();
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
            SELECT_PROJECT => self.select_project(&state, &args),
            CURRENT_PROJECT => self.current_project(&state),
            name if tool_defs::runner_tools().iter().any(|def| def.name == name) => {
                self.runner_tool(&state, name, args, &context.ct).await
            }
            name => ToolOutput::error(format!("Unknown tool: {name}")),
        };
        Ok(to_result(output).into())
    }
}
