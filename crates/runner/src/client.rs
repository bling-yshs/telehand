//! The connection to the server.

use std::path::PathBuf;

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use telehand_proto::{CLOSE_INVALID_KEY, CLOSE_REPLACED, RunnerMessage, ServerMessage};
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

/// Connect to the server and serve tool calls until told to stop.
/// `on_connected` is called with the MCP URL each time a connection is established.
pub async fn run(
    config: RunnerConfig,
    shutdown: CancellationToken,
    on_connected: impl Fn(&str),
) -> anyhow::Result<RunExit> {
    let ws_url = telehand_proto::ws_url(&config.server_url).map_err(anyhow::Error::msg)?;
    loop {
        match session(&config, &ws_url, &shutdown, &on_connected).await {
            Ok(SessionEnd::Exit(exit)) => return Ok(exit),
            Ok(SessionEnd::Disconnected) => tracing::warn!("disconnected from server"),
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "connection failed"),
        }
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(RunExit::Shutdown),
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
        }
    }
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

    let end = loop {
        tokio::select! {
            _ = shutdown.cancelled() => break SessionEnd::Exit(RunExit::Shutdown),
            msg = stream.next() => match msg {
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
    };
    drop(out_tx);
    writer.abort();
    Ok(end)
}
