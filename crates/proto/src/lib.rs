//! Messages exchanged between runner and server over the WebSocket, plus the
//! definitions of the file tools the runner executes.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod tool_defs;

/// Path of the runner WebSocket endpoint on the server.
pub const WS_PATH: &str = "/ws";

/// Close code sent to a runner that was replaced by a newer runner using the same key.
pub const CLOSE_REPLACED: u16 = 4000;
/// Close code sent to a runner whose key is unknown or has been removed.
pub const CLOSE_INVALID_KEY: u16 = 4001;

/// How often the server pings a runner.
pub const PING_INTERVAL: Duration = Duration::from_secs(15);
/// A connection that received nothing for this long is considered dead.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub main_folder: String,
    #[serde(default)]
    pub extra_folders: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerMessage {
    Hello {
        key: String,
        projects: Vec<ProjectInfo>,
    },
    /// Check that a key is valid without registering as the key's runner.
    Probe {
        key: String,
    },
    Response {
        id: u64,
        output: ToolOutput,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome,
    Request {
        id: u64,
        project: String,
        tool: String,
        args: Value,
    },
    /// The server no longer waits for request `id`; stop it (e.g. kill its command).
    Cancel {
        id: u64,
    },
}

/// Tool result content, serialized in the same shape as MCP content blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: Vec<Content>,
    #[serde(default)]
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text { text: text.into() }],
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text { text: text.into() }],
            is_error: true,
        }
    }
}

impl RunnerMessage {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("runner message serializes")
    }
}

impl ServerMessage {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("server message serializes")
    }
}

/// The MCP URL agents connect to for `key`.
pub fn mcp_url(server_url: &str, key: &str) -> String {
    format!("{}/mcp/{key}", server_url.trim_end_matches('/'))
}

/// The WebSocket URL a runner connects to, derived from the server's HTTP(S) URL.
pub fn ws_url(server_url: &str) -> Result<String, String> {
    let base = server_url.trim_end_matches('/');
    let rest = if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else {
        return Err(format!(
            "server URL must start with http:// or https://: {server_url}"
        ));
    };
    Ok(format!("{rest}{WS_PATH}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_derived_from_server_url() {
        assert_eq!(
            mcp_url("https://h.example/", "k"),
            "https://h.example/mcp/k"
        );
        assert_eq!(
            ws_url("http://127.0.0.1:8080").unwrap(),
            "ws://127.0.0.1:8080/ws"
        );
        assert_eq!(ws_url("https://h.example/").unwrap(), "wss://h.example/ws");
        assert!(ws_url("h.example").is_err());
    }

    #[test]
    fn content_uses_mcp_shape() {
        let image = Content::Image {
            data: "AA==".into(),
            mime_type: "image/png".into(),
        };
        assert_eq!(
            serde_json::to_value(&image).unwrap(),
            serde_json::json!({"type": "image", "data": "AA==", "mimeType": "image/png"})
        );
    }

    #[test]
    fn welcome_round_trips() {
        let json = ServerMessage::Welcome.to_json();
        assert_eq!(json, r#"{"type":"welcome"}"#);
        let back: ServerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ServerMessage::Welcome);
    }
}
