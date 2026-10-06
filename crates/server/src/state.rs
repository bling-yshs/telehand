use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::Value;
use telehand_proto::{CLOSE_INVALID_KEY, ProjectInfo, ServerMessage, ToolOutput};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{mcp::McpHandler, store::KeyStore};

pub const TOOL_TIMEOUT: Duration = Duration::from_secs(60);

pub type McpService = StreamableHttpService<McpHandler, LocalSessionManager>;

/// Something the connection task should send to its runner.
pub enum Outbound {
    Message(ServerMessage),
    Ping,
    Close(u16, String),
}

pub type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<ToolOutput>>>>;

pub struct RunnerHandle {
    pub conn_id: u64,
    pub tx: mpsc::UnboundedSender<Outbound>,
    pub projects: Vec<ProjectInfo>,
    pub pending: Pending,
}

#[derive(Debug)]
pub struct UnknownKey;

#[derive(Debug, PartialEq, Eq)]
pub enum CallError {
    Offline,
    Disconnected,
    Timeout,
}

pub struct AppState {
    pub keys: Arc<KeyStore>,
    pub runners: Mutex<HashMap<String, RunnerHandle>>,
    pub mcp_services: Mutex<HashMap<String, McpService>>,
    pub shutdown: CancellationToken,
    next_id: AtomicU64,
}

impl AppState {
    pub fn new(keys: Arc<KeyStore>, shutdown: CancellationToken) -> Self {
        Self {
            keys,
            runners: Mutex::new(HashMap::new()),
            mcp_services: Mutex::new(HashMap::new()),
            shutdown,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn key_exists(&self, key: &str) -> bool {
        self.keys.read(|keys| keys.contains_key(key))
    }

    pub fn current_project(&self, key: &str) -> Option<String> {
        self.keys.read(|keys| {
            keys.get(key)
                .and_then(|entry| entry.current_project.clone())
        })
    }

    pub fn set_current_project(&self, key: &str, project: &str) {
        self.keys.update(|keys| {
            if let Some(entry) = keys.get_mut(key) {
                entry.current_project = Some(project.to_string());
            }
        });
    }

    pub fn create_key(&self, name: Option<String>) -> String {
        let (key, entry) = crate::keys::new_entry(name);
        self.keys.update(|keys| keys.insert(key.clone(), entry));
        key
    }

    /// Remove `key`: its runner is disconnected and its MCP endpoint disappears.
    pub fn remove_key(&self, key: &str) -> bool {
        if !self.keys.update(|keys| keys.remove(key).is_some()) {
            return false;
        }
        self.mcp_services.lock().unwrap().remove(key);
        if let Some(runner) = self.runners.lock().unwrap().remove(key) {
            let _ = runner.tx.send(Outbound::Close(
                CLOSE_INVALID_KEY,
                "key has been removed".into(),
            ));
        }
        true
    }

    /// Register the runner connection for `key`, returning the one it replaces.
    /// `Err` if the key does not exist. The key is checked under the runners
    /// lock, so a concurrent `remove_key` either rejects this runner or
    /// disconnects it afterwards.
    pub fn attach_runner(
        &self,
        key: &str,
        handle: RunnerHandle,
    ) -> Result<Option<RunnerHandle>, UnknownKey> {
        let mut runners = self.runners.lock().unwrap();
        if !self.key_exists(key) {
            return Err(UnknownKey);
        }
        Ok(runners.insert(key.to_string(), handle))
    }

    /// The projects reported by the runner for `key`, or `None` if it is offline.
    pub fn runner_projects(&self, key: &str) -> Option<Vec<ProjectInfo>> {
        self.runners
            .lock()
            .unwrap()
            .get(key)
            .map(|runner| runner.projects.clone())
    }

    /// The MCP service for `key`, created on first use. `None` if the key is unknown.
    pub fn mcp_service(self: &Arc<Self>, key: &str) -> Option<McpService> {
        // Checked under the services lock, like `attach_runner`.
        let mut services = self.mcp_services.lock().unwrap();
        if !self.key_exists(key) {
            return None;
        }
        let service = services.entry(key.to_string()).or_insert_with(|| {
            let handler = McpHandler::new(key.to_string(), Arc::downgrade(self));
            StreamableHttpService::new(
                move || Ok(handler.clone()),
                Default::default(),
                StreamableHttpServerConfig::default()
                    .disable_allowed_hosts()
                    .with_cancellation_token(self.shutdown.child_token()),
            )
        });
        Some(service.clone())
    }

    /// Send a tool call to the runner for `key` and wait for its output.
    pub async fn call_runner(
        &self,
        key: &str,
        project: &str,
        tool: &str,
        args: Value,
    ) -> Result<ToolOutput, CallError> {
        let id = self.next_id();
        let (tx, rx) = oneshot::channel();
        let pending = {
            let runners = self.runners.lock().unwrap();
            let runner = runners.get(key).ok_or(CallError::Offline)?;
            runner.pending.lock().unwrap().insert(id, tx);
            let request = ServerMessage::Request {
                id,
                project: project.to_string(),
                tool: tool.to_string(),
                args,
            };
            if runner.tx.send(Outbound::Message(request)).is_err() {
                runner.pending.lock().unwrap().remove(&id);
                return Err(CallError::Offline);
            }
            runner.pending.clone()
        };
        match tokio::time::timeout(TOOL_TIMEOUT, rx).await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(_)) => Err(CallError::Disconnected),
            Err(_) => {
                pending.lock().unwrap().remove(&id);
                Err(CallError::Timeout)
            }
        }
    }
}
