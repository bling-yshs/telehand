//! Telehand server: forwards MCP tool calls from agents to runners.

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Router,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{any, get},
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub mod keys;
mod mcp;
mod state;
mod ws;

use state::AppState;

pub struct ServeOptions {
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
}

/// A server started with [`start`].
pub struct RunningServer {
    pub addr: SocketAddr,
    shutdown: CancellationToken,
    task: JoinHandle<std::io::Result<()>>,
}

impl RunningServer {
    /// Wait until the server stops.
    pub async fn wait(self) -> anyhow::Result<()> {
        self.task.await??;
        Ok(())
    }

    /// Stop the server and wait for it to finish.
    pub async fn stop(self) -> anyhow::Result<()> {
        self.shutdown.cancel();
        self.wait().await
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }
}

/// Bind and start serving in the background.
pub async fn start(options: ServeOptions) -> anyhow::Result<RunningServer> {
    let keys = keys::load(&options.data_dir)?;
    let shutdown = CancellationToken::new();
    let state = Arc::new(AppState::new(
        options.data_dir.clone(),
        keys,
        shutdown.clone(),
    ));

    let app = Router::new()
        .route(telehand_proto::WS_PATH, get(ws::handler))
        .route("/mcp/{key}", any(mcp_route))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(options.listen).await?;
    let addr = listener.local_addr()?;
    let token = shutdown.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(token.cancelled_owned())
            .await
    });
    Ok(RunningServer {
        addr,
        shutdown,
        task,
    })
}

async fn mcp_route(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    request: Request,
) -> Response {
    let Some(service) = state.mcp_service(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    service.handle(request).await.map(axum::body::Body::new)
}
