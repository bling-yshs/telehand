//! The connection to the server.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use telehand_proto::{
    CLOSE_INVALID_KEY, CLOSE_REPLACED, IDLE_TIMEOUT, RunnerMessage, ServerMessage,
};
use telehand_tools::Project;
use tokio::sync::mpsc;
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

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Connect to the server and serve tool calls until told to stop, reconnecting
/// with exponential backoff when the connection drops.
/// `on_connected` is called with the MCP URL each time a connection is established.
pub async fn run(
    config: RunnerConfig,
    shutdown: CancellationToken,
    on_connected: impl Fn(&str),
) -> anyhow::Result<RunExit> {
    let ws_url = telehand_proto::ws_url(&config.server_url).map_err(anyhow::Error::msg)?;
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

async fn session(
    config: &RunnerConfig,
    ws_url: &str,
    shutdown: &CancellationToken,
    on_connected: &impl Fn(&str),
    connected: &mut bool,
) -> anyhow::Result<SessionEnd> {
    let (socket, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .with_context(|| format!("connecting to {ws_url}"))?;
    let (mut sink, mut stream) = socket.split();

    let hello = RunnerMessage::Hello {
        key: config.key.clone(),
        projects: config.projects.clone(),
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
    on_connected(&telehand_proto::mcp_url(&config.server_url, &config.key));

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<RunnerMessage>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(Message::text(msg.to_json())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let mut idle_check = tokio::time::interval(Duration::from_secs(5));
    let mut last_seen = Instant::now();
    let end = loop {
        tokio::select! {
            _ = shutdown.cancelled() => break SessionEnd::Exit(RunExit::Shutdown),
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
                        Ok(ServerMessage::Request { id, project, tool, args }) => {
                            let out_tx = out_tx.clone();
                            let project = config.project(&project).map(|p| Project {
                                main_folder: PathBuf::from(&p.main_folder),
                                extra_folders: p.extra_folders.iter().map(PathBuf::from).collect(),
                            });
                            tokio::spawn(async move {
                                let output = match project {
                                    Some(project) => telehand_tools::execute(&project, &tool, args).await,
                                    None => telehand_proto::ToolOutput::error(
                                        "The selected project does not exist on the runner.",
                                    ),
                                };
                                let _ = out_tx.send(RunnerMessage::Response { id, output });
                            });
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
