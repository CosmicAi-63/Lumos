use std::path::Path;
use std::sync::{Arc, Mutex};

use lumos_model::events::Event;
use lumos_model::recorder::ClientInfo;
use lumos_model::{ToolError, ToolResult};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

use crate::wire::{Message, encode};

/// Per-connection state the engine reads and writes.
#[derive(Default)]
pub struct ConnCtx {
    pub client: Mutex<Option<ClientInfo>>,
    pub subscriptions: Mutex<Vec<String>>,
}

impl ConnCtx {
    pub fn client(&self) -> Option<ClientInfo> {
        self.client.lock().unwrap().clone()
    }
    fn wants(&self, event_type: &str) -> bool {
        self.subscriptions.lock().unwrap().iter().any(|p| p == "*" || event_type.starts_with(p.as_str()))
    }
}

/// Implemented by the engine.
#[async_trait::async_trait]
pub trait Dispatch: Send + Sync + 'static {
    async fn call(&self, ctx: Arc<ConnCtx>, method: &str, params: Value) -> ToolResult<Value>;
    fn events(&self) -> broadcast::Receiver<Event>;
}

/// Bind `path` (removing a stale socket) with owner-only permissions.
pub fn bind(path: &Path) -> std::io::Result<UnixListener> {
    crate::check_socket_path(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.message))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if path.exists() {
        // A live engine would accept; a stale file would not.
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "another engine is already running"));
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Accept connections until the task is dropped.
pub async fn serve(listener: UnixListener, dispatch: Arc<dyn Dispatch>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let d = dispatch.clone();
                tokio::spawn(async move { handle(stream, d).await });
            }
            Err(e) => {
                eprintln!("lumos-rpc: accept failed: {e}");
            }
        }
    }
}

async fn handle(stream: UnixStream, dispatch: Arc<dyn Dispatch>) {
    let (read, mut write) = stream.into_split();
    let ctx = Arc::new(ConnCtx::default());
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();

    // Writer: responses and events share one ordered stream.
    let writer = tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if write.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    // Event forwarder.
    let mut events = dispatch.events();
    let ev_ctx = ctx.clone();
    let ev_tx = out_tx.clone();
    let forwarder = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(ev) => {
                    if ev_ctx.wants(&ev.kind) {
                        let msg = Message::notification("event", serde_json::to_value(&ev).unwrap_or(Value::Null));
                        if ev_tx.send(encode(&msg)).is_err() {
                            break;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Message = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                let err = ToolError::invalid_params(format!("parse error: {e}"));
                let _ = out_tx.send(encode(&Message::err(Value::Null, &err)));
                continue;
            }
        };
        let (Some(method), id) = (msg.method, msg.id) else { continue };
        let params = msg.params.unwrap_or(Value::Object(Default::default()));
        let d = dispatch.clone();
        let c = ctx.clone();
        let tx = out_tx.clone();
        // Requests run concurrently so a long `jobs.wait` never blocks the connection.
        tokio::spawn(async move {
            let res = d.call(c, &method, params).await;
            if let Some(id) = id {
                let reply = match res {
                    Ok(v) => Message::ok(id, v),
                    Err(e) => Message::err(id, &e),
                };
                let _ = tx.send(encode(&reply));
            }
        });
    }

    forwarder.abort();
    drop(out_tx);
    let _ = writer.await;
}
