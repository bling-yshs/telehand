//! Key management. Commands go to the running server through the admin
//! socket in its data directory, so the server's in-memory key store stays
//! the only writer of `keys.json`. When no server is running, commands edit
//! `keys.json` directly.
//!
//! The socket is a Unix domain socket; on other platforms there is none and
//! commands always edit `keys.json`.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::bail;
#[cfg(unix)]
use anyhow::Context;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    keys::{self, Keys},
    state::AppState,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request {
    Create { name: Option<String> },
    List,
    Rm { key: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
enum Response {
    Created { key: String },
    Keys { keys: Keys },
    Removed,
    Error { message: String },
}

pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("admin.sock")
}

/// Send `request` to the running server; `None` if no server is running.
#[cfg(unix)]
async fn send(data_dir: &Path, request: &Request) -> anyhow::Result<Option<Response>> {
    use std::io::ErrorKind;

    let path = socket_path(data_dir);
    let stream = match UnixStream::connect(&path).await {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            return Ok(None);
        }
        Err(e) => return Err(e).with_context(|| format!("connecting to {}", path.display())),
    };
    let (reader, mut writer) = stream.into_split();
    let mut line = serde_json::to_string(request)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    let mut response = String::new();
    BufReader::new(reader).read_line(&mut response).await?;
    Ok(Some(
        serde_json::from_str(&response).context("invalid admin response")?,
    ))
}

#[cfg(not(unix))]
async fn send(_data_dir: &Path, _request: &Request) -> anyhow::Result<Option<Response>> {
    Ok(None)
}

fn unexpected(response: Response) -> anyhow::Error {
    match response {
        Response::Error { message } => anyhow::anyhow!(message),
        other => anyhow::anyhow!("unexpected admin response: {other:?}"),
    }
}

pub async fn create_key(data_dir: &Path, name: Option<String>) -> anyhow::Result<String> {
    match send(data_dir, &Request::Create { name: name.clone() }).await? {
        Some(Response::Created { key }) => Ok(key),
        Some(other) => Err(unexpected(other)),
        None => keys::create_in_file(data_dir, name),
    }
}

pub async fn list_keys(data_dir: &Path) -> anyhow::Result<Keys> {
    match send(data_dir, &Request::List).await? {
        Some(Response::Keys { keys }) => Ok(keys),
        Some(other) => Err(unexpected(other)),
        None => keys::load(data_dir),
    }
}

pub async fn remove_key(data_dir: &Path, key: &str) -> anyhow::Result<()> {
    let request = Request::Rm {
        key: key.to_string(),
    };
    match send(data_dir, &request).await? {
        Some(Response::Removed) => Ok(()),
        Some(other) => Err(unexpected(other)),
        None => {
            let mut keys = keys::load(data_dir)?;
            if keys.remove(key).is_none() {
                bail!("no such key: {key}");
            }
            keys::save(data_dir, &keys)
        }
    }
}

/// Start answering admin requests until `shutdown`.
pub fn start(
    data_dir: &Path,
    state: Arc<AppState>,
    shutdown: CancellationToken,
) -> anyhow::Result<JoinHandle<()>> {
    #[cfg(unix)]
    {
        let listener = bind(data_dir)?;
        Ok(tokio::spawn(serve(
            listener,
            socket_path(data_dir),
            state,
            shutdown,
        )))
    }
    #[cfg(not(unix))]
    {
        let _ = (data_dir, state, shutdown);
        Ok(tokio::spawn(async {}))
    }
}

/// Bind the admin socket, replacing a stale socket file.
#[cfg(unix)]
fn bind(data_dir: &Path) -> anyhow::Result<UnixListener> {
    let path = socket_path(data_dir);
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            bail!(
                "another server is already running with data dir {}",
                data_dir.display()
            );
        }
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))
}

/// Answer admin requests until `shutdown`, then remove the socket file.
#[cfg(unix)]
async fn serve(
    listener: UnixListener,
    path: PathBuf,
    state: Arc<AppState>,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, &state).await {
                            tracing::warn!(error = %format!("{e:#}"), "admin request failed");
                        }
                    });
                }
                Err(e) => tracing::warn!(error = %e, "admin accept failed"),
            }
        }
    }
    let _ = std::fs::remove_file(path);
}

#[cfg(unix)]
async fn handle(stream: UnixStream, state: &AppState) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut line = String::new();
    BufReader::new(reader).read_line(&mut line).await?;
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(Request::Create { name }) => Response::Created {
            key: state.create_key(name),
        },
        Ok(Request::List) => Response::Keys {
            keys: state.keys.read(Clone::clone),
        },
        Ok(Request::Rm { key }) => {
            if state.remove_key(&key) {
                Response::Removed
            } else {
                Response::Error {
                    message: format!("no such key: {key}"),
                }
            }
        }
        Err(e) => Response::Error {
            message: format!("invalid request: {e}"),
        },
    };
    let mut out = serde_json::to_string(&response)?;
    out.push('\n');
    writer.write_all(out.as_bytes()).await?;
    Ok(())
}
