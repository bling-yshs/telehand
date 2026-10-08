//! The connection to the server.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use telehand_proto::{
    CLOSE_INVALID_KEY, CLOSE_REPLACED, IDLE_TIMEOUT, RunnerMessage, ServerMessage, ToolOutput,
    tool_defs,
};
use telehand_tools::{
    Project,
    summary::{self, Outcome},
};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::{Message, protocol::CloseFrame};
use tokio_util::sync::CancellationToken;

use crate::config::RunnerConfig;

/// Why [`run`] returned.
#[derive(Debug, PartialEq, Eq)]
pub enum RunExit {
    /// Another runner connected with the same key.
    Replaced,
    /// The server does not know the key (or it was removed).
    KeyRejected,
    /// The shutdown token was cancelled.
    Shutdown,
}

/// How a single connection ended.
enum SessionEnd {
    Exit(RunExit),
    /// The connection dropped; reconnect.
    Disconnected,
}

type InFlight = Arc<Mutex<HashMap<u64, CancellationToken>>>;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Connect to the server and serve tool calls until told to stop, reconnecting
/// with exponential backoff when the connection drops.
/// `config` may change while running (see [`crate::watch_config`]); its new
/// projects are sent to the server.
/// `on_connected` is called with the MCP URL each time a connection is established.
pub async fn run(
    config: watch::Receiver<RunnerConfig>,
    shutdown: CancellationToken,
    on_connected: impl Fn(&str),
) -> anyhow::Result<RunExit> {
    let server_url = config.borrow().server_url.clone();
    let ws_url = telehand_proto::ws_url(&server_url).map_err(anyhow::Error::msg)?;
    let mut backoff = INITIAL_BACKOFF;
    loop {
        let mut connected = false;
        let result = tokio::select! {
            result = session(&config, &ws_url, &shutdown, &on_connected, &mut connected) => result,
            _ = shutdown.cancelled() => return Ok(RunExit::Shutdown),
        };
        match result {
            Ok(SessionEnd::Exit(exit)) => return Ok(exit),
            Ok(SessionEnd::Disconnected) => tracing::warn!("disconnected from server"),
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "connection failed"),
        }
        if connected {
            backoff = INITIAL_BACKOFF;
        }
        tracing::info!("reconnecting in {}s", backoff.as_secs());
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(RunExit::Shutdown),
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Check that the server accepts `key`, without taking over from a running runner.
pub async fn check_key(server_url: &str, key: &str) -> anyhow::Result<()> {
    let ws_url = telehand_proto::ws_url(server_url).map_err(anyhow::Error::msg)?;
    let (socket, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .with_context(|| format!("connecting to {ws_url}"))?;
    let (mut sink, mut stream) = socket.split();
    let probe = RunnerMessage::Probe {
        key: key.to_string(),
    };
    sink.send(Message::text(probe.to_json())).await?;
    while let Some(msg) = stream.next().await {
        match msg? {
            Message::Text(text) => {
                if serde_json::from_str::<ServerMessage>(&text)? == ServerMessage::Welcome {
                    return Ok(());
                }
            }
            Message::Close(frame) => {
                if close_exit(frame.as_ref()) == Some(RunExit::KeyRejected) {
                    bail!("the server rejected the key (unknown or removed)");
                }
                break;
            }
            _ => {}
        }
    }
    bail!("the server closed the connection without accepting the key")
}

fn close_exit(frame: Option<&CloseFrame>) -> Option<RunExit> {
    match frame.map(|f| u16::from(f.code)) {
        Some(CLOSE_REPLACED) => Some(RunExit::Replaced),
        Some(CLOSE_INVALID_KEY) => Some(RunExit::KeyRejected),
        _ => None,
    }
}

/// The longest tool name.
const TOOL_WIDTH: usize = tool_defs::BASH_RESULT.len();
/// The longest status.
const STATUS_WIDTH: usize = "cancelled".len();

/// A request in the console log, so the runner's owner sees what agents do.
/// Lines have aligned columns: tool, project, status, elapsed time, then the
/// key argument and any detail:
/// `bash         telehand  ok          39.2s  cargo build -q`.
struct LogLine {
    tool: String,
    project: String,
    /// The width of the project column: the longest project name.
    project_width: usize,
    summary: String,
}

impl LogLine {
    fn new(config: &RunnerConfig, project: &str, tool: &str, args: &Value) -> Self {
        let project_width = config
            .projects
            .iter()
            .map(|p| p.name.chars().count())
            .max()
            .unwrap_or(0);
        Self {
            tool: tool.to_string(),
            project: if project.is_empty() { "-" } else { project }.to_string(),
            project_width,
            summary: summary::request(tool, args),
        }
    }

    fn started(&self) {
        tracing::info!("{}", self.format("started", None, None));
    }

    fn finished(&self, output: &ToolOutput, cancel: &CancellationToken, elapsed: Duration) {
        let elapsed = Some(elapsed);
        if cancel.is_cancelled() {
            tracing::warn!("{}", self.format("cancelled", elapsed, None));
            return;
        }
        match summary::outcome(output) {
            Outcome::Ok => tracing::info!("{}", self.format("ok", elapsed, None)),
            Outcome::Running(task) => {
                tracing::info!(
                    "{}",
                    self.format("running", elapsed, Some(&format!("task {task}")))
                )
            }
            Outcome::Error(error) => {
                tracing::warn!("{}", self.format("error", elapsed, Some(&error)))
            }
        }
    }

    fn format(&self, status: &str, elapsed: Option<Duration>, detail: Option<&str>) -> String {
        let elapsed = match elapsed {
            None => String::new(),
            Some(d) if d < Duration::from_secs(1) => format!("{}ms", d.as_millis()),
            Some(d) => format!("{:.1}s", d.as_secs_f64()),
        };
        let mut line = format!(
            "{:<TOOL_WIDTH$}  {:<project_width$}  {status:<STATUS_WIDTH$}  {elapsed:>7}  {}",
            self.tool,
            self.project,
            self.summary,
            project_width = self.project_width,
        );
        if let Some(detail) = detail {
            line.push_str(": ");
            line.push_str(detail);
        }
        line.trim_end().to_string()
    }
}

async fn session(
    config: &watch::Receiver<RunnerConfig>,
    ws_url: &str,
    shutdown: &CancellationToken,
    on_connected: &impl Fn(&str),
    connected: &mut bool,
) -> anyhow::Result<SessionEnd> {
    let (socket, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .with_context(|| format!("connecting to {ws_url}"))?;
    let (mut sink, mut stream) = socket.split();

    // The hello reports the projects as they are now; later changes are sent
    // as they happen.
    let mut config = config.clone();
    let (hello, mcp_url) = {
        let config = config.borrow_and_update();
        let hello = RunnerMessage::Hello {
            key: config.key.clone(),
            projects: config.projects.clone(),
        };
        (
            hello,
            telehand_proto::mcp_url(&config.server_url, &config.key),
        )
    };
    sink.send(Message::text(hello.to_json())).await?;

    // Wait for the server to accept the key.
    loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => {
                if serde_json::from_str::<ServerMessage>(&text)? == ServerMessage::Welcome {
                    break;
                }
            }
            Some(Ok(Message::Close(frame))) => {
                return Ok(match close_exit(frame.as_ref()) {
                    Some(exit) => SessionEnd::Exit(exit),
                    None => SessionEnd::Disconnected,
                });
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(e.into()),
            None => return Ok(SessionEnd::Disconnected),
        }
    }
    *connected = true;
    on_connected(&mcp_url);

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<RunnerMessage>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(Message::text(msg.to_json())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Requests still running, so the server can cancel them. A dropped
    // connection cancels nothing: commands keep running, and the agent can
    // collect their results with bash_result after the runner reconnects.
    let in_flight: InFlight = Arc::new(Mutex::new(HashMap::new()));

    let mut idle_check = tokio::time::interval(Duration::from_secs(5));
    let mut last_seen = Instant::now();
    // False once nothing can change the config any more.
    let mut watching = true;
    let end = loop {
        tokio::select! {
            _ = shutdown.cancelled() => break SessionEnd::Exit(RunExit::Shutdown),
            changed = config.changed(), if watching => match changed {
                Ok(()) => {
                    let projects = config.borrow_and_update().projects.clone();
                    let _ = out_tx.send(RunnerMessage::Projects { projects });
                }
                Err(_) => watching = false,
            },
            _ = idle_check.tick() => {
                if last_seen.elapsed() > IDLE_TIMEOUT {
                    tracing::warn!("server stopped responding");
                    break SessionEnd::Disconnected;
                }
            }
            msg = stream.next() => {
                // Any frame from the server (including pings) shows it is alive.
                last_seen = Instant::now();
                match msg {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str::<ServerMessage>(&text) {
                        Ok(ServerMessage::Request { id, project: project_name, tool, args }) => {
                            let out_tx = out_tx.clone();
                            let (project, log) = {
                                let config = config.borrow();
                                let project = config.project(&project_name).map(|p| Project {
                                    main_folder: PathBuf::from(&p.main_folder),
                                    extra_folders: p.extra_folders.iter().map(PathBuf::from).collect(),
                                });
                                (project, LogLine::new(&config, &project_name, &tool, &args))
                            };
                            let cancel = CancellationToken::new();
                            in_flight.lock().unwrap().insert(id, cancel.clone());
                            let in_flight = in_flight.clone();
                            tokio::spawn(async move {
                                // Bash may run for long; show it right away.
                                if tool == tool_defs::BASH {
                                    log.started();
                                }
                                let started = Instant::now();
                                let output =
                                    telehand_tools::execute(project.as_ref(), &tool, args, &cancel).await;
                                in_flight.lock().unwrap().remove(&id);
                                log.finished(&output, &cancel, started.elapsed());
                                let _ = out_tx.send(RunnerMessage::Response { id, output });
                            });
                        }
                        Ok(ServerMessage::Cancel { id }) => {
                            if let Some(cancel) = in_flight.lock().unwrap().remove(&id) {
                                cancel.cancel();
                            }
                        }
                        Ok(ServerMessage::Welcome) => {}
                        Err(e) => tracing::warn!(error = %e, "invalid server message"),
                    },
                    Some(Ok(Message::Close(frame))) => {
                        break match close_exit(frame.as_ref()) {
                            Some(exit) => SessionEnd::Exit(exit),
                            None => SessionEnd::Disconnected,
                        };
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break SessionEnd::Disconnected,
                }
            }
        }
    };
    drop(out_tx);
    writer.abort();
    Ok(end)
}
