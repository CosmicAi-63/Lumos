use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lumos_model::events::Event;
use lumos_model::{ErrorCode, ToolError, ToolResult};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, oneshot};

use crate::wire::{Message, encode};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<ToolResult<Value>>>>>;

/// A connection to the engine.
pub struct Client {
    write: tokio::sync::Mutex<OwnedWriteHalf>,
    next_id: AtomicU64,
    pending: Pending,
    events: Mutex<Option<mpsc::UnboundedReceiver<Event>>>,
}

impl Client {
    pub async fn connect(path: &Path) -> ToolResult<Self> {
        crate::check_socket_path(path)?;
        let stream = UnixStream::connect(path).await.map_err(|e| {
            ToolError::new(ErrorCode::EngineUnavailable, format!("cannot connect to {}: {e}", path.display()))
        })?;
        let (read, write) = stream.into_split();
        let pending: Pending = Arc::default();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();

        let p = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(msg) = serde_json::from_str::<Message>(&line) else { continue };
                if msg.method.as_deref() == Some("event") {
                    if let Some(ev) = msg.params.and_then(|v| serde_json::from_value::<Event>(v).ok()) {
                        let _ = ev_tx.send(ev);
                    }
                    continue;
                }
                let Some(id) = msg.id.as_ref().and_then(Value::as_u64) else { continue };
                let res = match (msg.result, msg.error) {
                    (_, Some(e)) => Err(ToolError::from(e)),
                    (Some(v), None) => Ok(v),
                    (None, None) => Ok(Value::Null),
                };
                if let Some(tx) = p.lock().unwrap().remove(&id) {
                    let _ = tx.send(res);
                }
            }
            // Connection closed: fail everything still waiting.
            for (_, tx) in p.lock().unwrap().drain() {
                let _ = tx.send(Err(ToolError::new(ErrorCode::EngineUnavailable, "engine closed the connection")));
            }
        });

        Ok(Self { write: tokio::sync::Mutex::new(write), next_id: AtomicU64::new(1), pending, events: Mutex::new(Some(ev_rx)) })
    }

    pub async fn call(&self, method: &str, params: Value) -> ToolResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let line = encode(&Message::request(id, method, params));
        self.write
            .lock()
            .await
            .write_all(line.as_bytes())
            .await
            .map_err(|e| ToolError::new(ErrorCode::EngineUnavailable, format!("write failed: {e}")))?;
        rx.await.unwrap_or_else(|_| Err(ToolError::new(ErrorCode::EngineUnavailable, "engine went away")))
    }

    /// Typed convenience wrapper around [`Client::call`].
    pub async fn call_as<P: serde::Serialize, R: serde::de::DeserializeOwned>(&self, method: &str, params: &P) -> ToolResult<R> {
        let v = self.call(method, serde_json::to_value(params).map_err(|e| ToolError::invalid_params(e.to_string()))?).await?;
        serde_json::from_value(v).map_err(|e| ToolError::backend(format!("unexpected {method} result: {e}")))
    }

    /// Take the event stream (once). Subscribe with `events.subscribe` first.
    pub fn take_events(&self) -> Option<mpsc::UnboundedReceiver<Event>> {
        self.events.lock().unwrap().take()
    }
}
