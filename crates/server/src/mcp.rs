//! The MCP server exposed to agents at `/mcp/{key}`.

use std::sync::Weak;

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

use crate::state::{AppState, CallError};

pub const LIST_PROJECT: &str = "list_project";
pub const SELECT_PROJECT: &str = "select_project";
pub const CURRENT_PROJECT: &str = "current_project";

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
    let mut tools = vec![
        Tool::new(
            LIST_PROJECT,
            "List the projects registered on the runner, with their main folder and extra folders, and mark the current project. File tools (read, write, edit) operate on the current project: relative paths resolve against its main folder, and write/edit may only modify files inside its folders.",
            schema(json!({"type": "object", "properties": {}})),
        ),
        Tool::new(
            SELECT_PROJECT,
            "Select the current project by name. All file tools operate on the current project until another one is selected. The selection is shared by every agent using this MCP URL.",
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
            "Show the current project: its name, main folder (base for relative paths) and extra folders.",
            schema(json!({"type": "object", "properties": {}})),
        ),
    ];
    tools.extend(
        tool_defs::file_tools()
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

fn no_project_selected() -> ToolOutput {
    ToolOutput::error(
        "No project selected. Use list_project to see the available projects, then select_project to choose one.",
    )
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
                "No projects are registered on the runner. Add one on the runner machine with `telehand-runner project add <dir>` and restart the runner.",
            );
        }
        let current = state.current_project(&self.key);
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
        state.set_current_project(&self.key, name);
        ToolOutput::text(format!("Selected project {}", describe(project)))
    }

    fn current_project(&self, state: &AppState) -> ToolOutput {
        let Some(projects) = state.runner_projects(&self.key) else {
            return ToolOutput::error(RUNNER_OFFLINE);
        };
        let Some(current) = state.current_project(&self.key) else {
            return ToolOutput::text(
                "No project selected. Use list_project to see the available projects, then select_project to choose one.",
            );
        };
        match projects.iter().find(|p| p.name == current) {
            Some(project) => ToolOutput::text(format!("Current project: {}", describe(project))),
            None => project_missing(&current),
        }
    }

    async fn file_tool(&self, state: &AppState, tool: &str, args: Value) -> ToolOutput {
        let Some(projects) = state.runner_projects(&self.key) else {
            return ToolOutput::error(RUNNER_OFFLINE);
        };
        let Some(current) = state.current_project(&self.key) else {
            return no_project_selected();
        };
        if !projects.iter().any(|p| p.name == current) {
            return project_missing(&current);
        }
        match state.call_runner(&self.key, &current, tool, args).await {
            Ok(output) => output,
            Err(CallError::Offline) | Err(CallError::Disconnected) => {
                ToolOutput::error(RUNNER_OFFLINE)
            }
            Err(CallError::Timeout) => ToolOutput::error(format!(
                "Timed out after {} seconds waiting for the runner to respond.",
                crate::state::TOOL_TIMEOUT.as_secs()
            )),
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
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(state) = self.state.upgrade() else {
            return Err(ErrorData::internal_error("server is shutting down", None));
        };
        let args = Value::Object(request.arguments.unwrap_or_default());
        let output = match request.name.as_ref() {
            LIST_PROJECT => self.list_project(&state),
            SELECT_PROJECT => self.select_project(&state, &args),
            CURRENT_PROJECT => self.current_project(&state),
            name if tool_defs::file_tools().iter().any(|def| def.name == name) => {
                self.file_tool(&state, name, args).await
            }
            name => ToolOutput::error(format!("Unknown tool: {name}")),
        };
        Ok(to_result(output).into())
    }
}
