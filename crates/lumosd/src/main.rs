//! `lumosd`: hosts the Lumos engine on a Unix socket.
//!
//! On macOS the engine will normally live inside Lumos.app (one binary holds the
//! capture permissions); `lumosd` is the host for M0, CI and other platforms.
//! The `lumos` CLI starts it on demand.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use lumos_core::{Engine, EngineOptions};
use lumos_model::recorder::{ClientInfo, ClientKind};
use lumos_rpc::paths::Paths;

/// How long shutdown waits for recordings that are still being saved.
const DRAIN: Duration = Duration::from_secs(120);

#[tokio::main]
async fn main() -> ExitCode {
    let level = std::env::var("LUMOS_LOG").ok().and_then(|l| l.parse().ok()).unwrap_or(tracing::Level::INFO);
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_target(false).with_max_level(level).init();

    let paths = Paths::from_env();
    let listener = match lumos_rpc::bind(&paths.socket) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(socket = %paths.socket.display(), "cannot listen: {e}");
            return ExitCode::from(7);
        }
    };
    let engine = match Engine::new(EngineOptions { paths: paths.clone(), host: "lumosd".into(), backend: None }) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("cannot start the engine: {e}");
            let _ = std::fs::remove_file(&paths.socket);
            return ExitCode::from(1);
        }
    };
    tracing::info!(socket = %paths.socket.display(), backend = engine.backend_name(), pid = std::process::id(), "engine ready");

    let server = tokio::spawn(lumos_rpc::serve(listener, engine.clone() as Arc<dyn lumos_rpc::Dispatch>));

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
    let by_signal = tokio::select! {
        _ = engine.wait_shutdown() => false,
        _ = tokio::signal::ctrl_c() => true,
        _ = term.recv() => true,
    };
    if by_signal {
        // Save whatever is being recorded rather than lose it.
        let me = ClientInfo { kind: ClientKind::Ui, name: "lumosd".into(), version: env!("CARGO_PKG_VERSION").into() };
        if engine.call_as(&me, "recording.stop", serde_json::json!({})).await.is_ok() {
            tracing::info!("stopped the active recording before exit");
        }
        engine.request_shutdown();
    }
    tracing::info!("shutting down");
    engine.drain(DRAIN).await;
    // Let the reply to `system.shutdown` reach its client.
    tokio::time::sleep(Duration::from_millis(100)).await;
    server.abort();
    let _ = std::fs::remove_file(&paths.socket);
    ExitCode::SUCCESS
}
