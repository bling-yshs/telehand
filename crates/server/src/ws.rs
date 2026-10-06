//! Runner connections.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt, stream::SplitStream};
use telehand_proto::{CLOSE_INVALID_KEY, CLOSE_REPLACED, RunnerMessage, ServerMessage};
use tokio::sync::mpsc;

use crate::state::{AppState, Outbound, Pending, RunnerHandle};

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(state, socket))
}

async fn next_text(stream: &mut SplitStream<WebSocket>) -> Option<String> {
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Text(text) => return Some(text.to_string()),
            Message::Close(_) => return None,
            _ => {}
        }
    }
    None
}

fn close(code: u16, reason: &str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

async fn handle_socket(state: Arc<AppState>, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();

    let hello = match tokio::time::timeout(HELLO_TIMEOUT, next_text(&mut stream)).await {
        Ok(Some(text)) => text,
        _ => return,
    };
    let (key, projects) = match serde_json::from_str::<RunnerMessage>(&hello) {
        Ok(RunnerMessage::Hello { key, projects }) => (key, projects),
        _ => {
            let _ = sink.send(close(1002, "expected hello")).await;
            return;
        }
    };
    if !state.key_exists(&key) {
        let _ = sink
            .send(close(CLOSE_INVALID_KEY, "key is invalid or has been removed"))
            .await;
        return;
    }

    let conn_id = state.next_id();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let previous = state.runners.lock().unwrap().insert(
        key.clone(),
        RunnerHandle {
            conn_id,
            tx: tx.clone(),
            projects,
            pending: pending.clone(),
        },
    );
    if let Some(previous) = previous {
        tracing::warn!(key = %key, "a new runner connected with this key; replacing the old one");
        let _ = previous.tx.send(Outbound::Close(
            CLOSE_REPLACED,
            "replaced by a newer runner using the same key".into(),
        ));
    }
    tracing::info!(key = %key, "runner connected");
    let _ = tx.send(Outbound::Message(ServerMessage::Welcome));
    drop(tx);

    let writer = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Outbound::Message(msg) => {
                    if sink.send(Message::Text(msg.to_json().into())).await.is_err() {
                        break;
                    }
                }
                Outbound::Close(code, reason) => {
                    let _ = sink.send(close(code, &reason)).await;
                    break;
                }
            }
        }
    });

    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<RunnerMessage>(&text) {
                        Ok(RunnerMessage::Response { id, output }) => {
                            if let Some(waiter) = pending.lock().unwrap().remove(&id) {
                                let _ = waiter.send(output);
                            }
                        }
                        Ok(other) => tracing::warn!(?other, "unexpected runner message"),
                        Err(e) => tracing::warn!(error = %e, "invalid runner message"),
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            }
        }
    }

    {
        let mut runners = state.runners.lock().unwrap();
        if runners.get(&key).is_some_and(|r| r.conn_id == conn_id) {
            runners.remove(&key);
        }
    }
    // Dropping the waiters makes in-flight calls fail instead of waiting for the timeout.
    pending.lock().unwrap().clear();
    writer.abort();
    tracing::info!(key = %key, "runner disconnected");
}
