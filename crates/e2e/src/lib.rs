//! Harness for end-to-end tests: a real server and runner in one process,
//! driven through a real MCP client.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ClientConfig, ContentBlock},
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::Value;
use telehand_proto::ProjectInfo;
use telehand_runner::{RunExit, RunnerConfig};
use telehand_server::{RunningServer, ServeOptions};
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use serde_json::json;

pub struct Env {
    pub tmp: TempDir,
    pub data_dir: PathBuf,
    pub key: String,
    pub server_url: String,
    server: Option<RunningServer>,
}

impl Env {
    /// Start a server with one key.
    pub async fn start() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let key = telehand_server::keys::create_in_file(&data_dir, None).unwrap();
        let server = telehand_server::start(ServeOptions {
            listen: "127.0.0.1:0".parse().unwrap(),
            data_dir: data_dir.clone(),
        })
        .await
        .unwrap();
        let server_url = format!("http://{}", server.addr);
        Env {
            tmp,
            data_dir,
            key,
            server_url,
            server: Some(server),
        }
    }

    pub fn mcp_url(&self) -> String {
        telehand_proto::mcp_url(&self.server_url, &self.key)
    }

    /// Create (and return the canonical path of) a directory inside the test's temp dir.
    pub fn dir(&self, rel: &str) -> PathBuf {
        let path = self.tmp.path().join(rel);
        std::fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    /// A registered runner config with the given projects (name, main folder).
    pub fn runner_config(&self, projects: &[(&str, &Path)]) -> RunnerConfig {
        RunnerConfig {
            server_url: self.server_url.clone(),
            key: self.key.clone(),
            projects: projects
                .iter()
                .map(|(name, dir)| ProjectInfo {
                    name: name.to_string(),
                    main_folder: dir.to_string_lossy().into_owned(),
                    extra_folders: Vec::new(),
                })
                .collect(),
        }
    }

    pub fn start_runner(&self, config: RunnerConfig) -> Runner {
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(telehand_runner::run(config, shutdown.clone(), |_| {}));
        Runner { shutdown, task }
    }

    pub async fn client(&self) -> Client {
        Client::connect(&self.mcp_url()).await
    }

    pub async fn stop_server(&mut self) {
        if let Some(server) = self.server.take() {
            server.stop().await.unwrap();
        }
    }
}

pub struct Runner {
    shutdown: CancellationToken,
    task: JoinHandle<anyhow::Result<RunExit>>,
}

impl Runner {
    pub async fn stop(self) {
        self.shutdown.cancel();
        let _ = self.task.await;
    }

    /// Wait for the runner to exit on its own.
    pub async fn exit(self) -> RunExit {
        tokio::time::timeout(Duration::from_secs(10), self.task)
            .await
            .expect("runner did not exit")
            .unwrap()
            .unwrap()
    }
}

pub struct Client {
    service: RunningService<RoleClient, ClientConfig>,
}

impl Client {
    pub async fn connect(url: &str) -> Client {
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url.to_string()),
        );
        let service = ClientConfig::default().serve(transport).await.unwrap();
        Client { service }
    }

    pub async fn tool_names(&self) -> Vec<String> {
        self.service
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    pub async fn call(&self, tool: &str, args: Value) -> CallToolResult {
        let args = match args {
            Value::Object(map) => map,
            _ => panic!("tool arguments must be an object"),
        };
        self.service
            .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args))
            .await
            .unwrap()
    }

    /// Call a tool that must succeed; returns its text.
    pub async fn ok(&self, tool: &str, args: Value) -> String {
        let result = self.call(tool, args).await;
        let text = text(&result);
        assert_ne!(result.is_error, Some(true), "{tool} failed: {text}");
        text
    }

    /// Call a tool that must fail; returns its error text.
    pub async fn err(&self, tool: &str, args: Value) -> String {
        let result = self.call(tool, args).await;
        let text = text(&result);
        assert_eq!(result.is_error, Some(true), "{tool} should fail: {text}");
        text
    }

    /// Wait until the runner is connected.
    pub async fn wait_online(&self) {
        for _ in 0..100 {
            if self.call("list_project", json!({})).await.is_error != Some(true) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("runner did not come online");
    }

    /// Wait until the runner is disconnected.
    pub async fn wait_offline(&self) {
        for _ in 0..100 {
            if self.call("list_project", json!({})).await.is_error == Some(true) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("runner did not go offline");
    }
}

/// All text blocks of a tool result, joined by newlines.
pub fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
