//! Names, descriptions and input schemas of the tools executed on the
//! runner. Descriptions follow the pi coding agent.

use serde_json::{Value, json};

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

pub const READ: &str = "read";
pub const WRITE: &str = "write";
pub const EDIT: &str = "edit";
pub const BASH: &str = "bash";
pub const BASH_RESULT: &str = "bash_result";
pub const BASH_KILL: &str = "bash_kill";
pub const GREP: &str = "grep";
pub const FIND: &str = "find";

/// How long `bash` and `bash_result` wait for a command by default before
/// returning while it keeps running.
pub const BASH_DEFAULT_WAIT_SECONDS: f64 = 50.0;

const READ_DESCRIPTION: &str = "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). Images are sent as attachments. For text files, output is truncated to 2000 lines or 50KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.";

const WRITE_DESCRIPTION: &str = "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.";

const EDIT_DESCRIPTION: &str = "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.";

const BASH_DESCRIPTION: &str = "Execute a bash command in the current working directory.";

const BASH_RESULT_DESCRIPTION: &str = "Wait for a bash command that was still running when bash returned. Waits up to `wait` seconds (default 50): returns the command's result if it finished, otherwise its latest output; call again to keep waiting.";

const GREP_DESCRIPTION: &str = "Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to 100 matches or 50KB (whichever is hit first). Long lines are truncated to 500 chars.";

const FIND_DESCRIPTION: &str = "Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to 1000 results or 50KB (whichever is hit first).";

const BASH_KILL_DESCRIPTION: &str = "Stop a running bash command by its task ID, killing it and every process it started, and return its output.";

/// The tools that run on the runner, in the context of the current project.
pub fn runner_tools() -> Vec<ToolDef> {
    vec![
        read_def(),
        ToolDef {
            name: WRITE,
            description: WRITE_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to write (relative or absolute)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDef {
            name: EDIT,
            description: EDIT_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to edit (relative or absolute)"
                    },
                    "edits": {
                        "type": "array",
                        "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "oldText": {
                                    "type": "string",
                                    "description": "Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."
                                },
                                "newText": {
                                    "type": "string",
                                    "description": "Replacement text for this targeted edit."
                                }
                            },
                            "required": ["oldText", "newText"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
        },
        ToolDef {
            name: BASH,
            description: BASH_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    },
                    "timeout": {
                        "type": "number",
                        "description": "Timeout in seconds (optional, no default timeout)"
                    },
                    "wait": {
                        "type": "number",
                        "description": "Seconds to wait for the command to finish before returning a task ID (optional, default 50). The command keeps running either way."
                    }
                },
                "required": ["command"]
            }),
        },
        ToolDef {
            name: BASH_RESULT,
            description: BASH_RESULT_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Task ID returned by bash"
                    },
                    "wait": {
                        "type": "number",
                        "description": "Seconds to wait for the command to finish (optional, default 50)"
                    }
                },
                "required": ["task_id"]
            }),
        },
        ToolDef {
            name: BASH_KILL,
            description: BASH_KILL_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Task ID returned by bash"
                    }
                },
                "required": ["task_id"]
            }),
        },
        ToolDef {
            name: GREP,
            description: GREP_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Search pattern (regex or literal string)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory or file to search (default: current directory)"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'"
                    },
                    "ignoreCase": {
                        "type": "boolean",
                        "description": "Case-insensitive search (default: false)"
                    },
                    "literal": {
                        "type": "boolean",
                        "description": "Treat pattern as literal string instead of regex (default: false)"
                    },
                    "context": {
                        "type": "number",
                        "description": "Number of lines to show before and after each match (default: 0)"
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of matches to return (default: 100)"
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDef {
            name: FIND,
            description: FIND_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (default: current directory)"
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of results (default: 1000)"
                    }
                },
                "required": ["pattern"]
            }),
        },
    ]
}

fn read_def() -> ToolDef {
    ToolDef {
        name: READ,
        description: READ_DESCRIPTION,
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read (relative or absolute)"
                },
                "offset": {
                    "type": "number",
                    "description": "Line number to start reading from (1-indexed)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of lines to read"
                }
            },
            "required": ["path"]
        }),
    }
}
