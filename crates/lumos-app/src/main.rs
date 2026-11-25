//! Lumos desktop app: hosts the engine in-process (so the camera, microphone and
//! screen permissions belong to this one app), serves the same socket the `lumos`
//! CLI and MCP server use, and shows the toolbar, camera bubble and library.
//!
//! The web UI never touches devices or files itself: every action is one engine
//! tool, reached through the `call` command. See docs/06-ui-ux.md.

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lumos_core::{Engine, EngineOptions};
use lumos_model::ToolError;
use lumos_model::recorder::{ClientInfo, ClientKind};
use lumos_rpc::Dispatch;
use lumos_rpc::paths::Paths;
use serde_json::Value;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, RunEvent, State};
use tokio::sync::broadcast::error::RecvError;

/// How long quitting waits for recordings that are still being saved.
const DRAIN: Duration = Duration::from_secs(120);

struct App {
    engine: Arc<Engine>,
    me: ClientInfo,
    paths: Paths,
    quitting: AtomicBool,
}

impl App {
    /// Save whatever is being recorded, wait for files to finish, remove the socket.
    async fn shutdown(&self) {
        if self.engine.call_as(&self.me, "recording.stop", Value::Null).await.is_ok() {
            tracing::info!("stopped the active recording before exit");
        }
        self.engine.request_shutdown();
        self.engine.drain(DRAIN).await;
        let _ = std::fs::remove_file(&self.paths.socket);
    }
}

/// Every UI action: one engine tool by name.
#[tauri::command]
async fn call(state: State<'_, Arc<App>>, method: String, params: Option<Value>) -> Result<Value, ToolError> {
    state.engine.call_as(&state.me, &method, params.unwrap_or(Value::Null)).await
}

#[tauri::command]
fn show_window(app: AppHandle, label: String) -> Result<(), String> {
    let w = app.get_webview_window(&label).ok_or_else(|| format!("no window '{label}'"))?;
    w.show().map_err(|e| e.to_string())?;
    if label == "main" {
        w.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn hide_window(app: AppHandle, label: String) -> Result<(), String> {
    app.get_webview_window(&label).ok_or_else(|| format!("no window '{label}'"))?.hide().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_window_size(app: AppHandle, label: String, width: f64, height: f64) -> Result<(), String> {
    let w = app.get_webview_window(&label).ok_or_else(|| format!("no window '{label}'"))?;
    w.set_size(LogicalSize::new(width.clamp(40.0, 4000.0), height.clamp(40.0, 4000.0))).map_err(|e| e.to_string())
}

/// Native folder chooser. `None` when the user cancels.
#[tauri::command]
async fn pick_folder() -> Result<Option<String>, String> {
    let out = osascript(&["POSIX path of (choose folder with prompt \"Choose where Lumos saves recordings\")"]).await?;
    Ok(out.map(|s| s.trim_end_matches('/').to_string()).filter(|s| !s.is_empty()))
}

/// Native file chooser for importing videos.
#[tauri::command]
async fn pick_files() -> Result<Vec<String>, String> {
    let script = [
        "set fs to choose file with prompt \"Choose videos to import\" with multiple selections allowed",
        "set out to \"\"",
        "repeat with f in fs",
        "set out to out & POSIX path of f & linefeed",
        "end repeat",
        "return out",
    ];
    Ok(osascript(&script).await?.map(|s| s.lines().map(str::to_owned).filter(|l| !l.is_empty()).collect()).unwrap_or_default())
}

/// Open a file with its default app (the video player).
#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    if !std::path::Path::new(&path).exists() {
        return Err(format!("{path} does not exist"));
    }
    Command::new("open").arg(&path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Open one pane of System Settings > Privacy & Security and show Lumos.app in Finder, so the
/// user can drag it into the list (dropping it there turns the permission on). Changes nothing itself.
#[tauri::command]
fn open_privacy_pane(kind: String) -> Result<(), String> {
    let pane = match kind.as_str() {
        "screen" => "Privacy_ScreenCapture",
        "camera" => "Privacy_Camera",
        "microphone" => "Privacy_Microphone",
        "accessibility" => "Privacy_Accessibility",
        other => return Err(format!("unknown permission '{other}'")),
    };
    Command::new("open")
        .arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}"))
        .spawn()
        .map_err(|e| e.to_string())?;
    // Reveal the enclosing .app (or the binary itself when running unbundled).
    if let Ok(exe) = std::env::current_exe() {
        let app = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(|p| p.to_path_buf()).unwrap_or(exe);
        Command::new("open").arg("-R").arg(app).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn quit(app: AppHandle, state: State<'_, Arc<App>>) -> Result<(), String> {
    state.quitting.store(true, Ordering::SeqCst);
    state.shutdown().await;
    app.exit(0);
    Ok(())
}

/// Run AppleScript lines; `Ok(None)` when the user cancels the dialog.
async fn osascript(lines: &[&str]) -> Result<Option<String>, String> {
    let mut cmd = tokio::process::Command::new("osascript");
    for l in lines {
        cmd.arg("-e").arg(l);
    }
    let out = cmd.output().await.map_err(|e| format!("osascript: {e}"))?;
    if out.status.success() {
        return Ok(Some(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string()));
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("-128") { Ok(None) } else { Err(err.trim().to_string()) }
}

fn main() {
    let level = std::env::var("LUMOS_LOG").ok().and_then(|l| l.parse().ok()).unwrap_or(tracing::Level::INFO);
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_target(false).with_max_level(level).init();

    let paths = Paths::from_env();
    let listener = match tauri::async_runtime::block_on(async { lumos_rpc::bind(&paths.socket) }) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Lumos can't start: {e}. If the engine is running on its own, stop it with `lumos engine stop`.");
            std::process::exit(7);
        }
    };
    let engine = match Engine::new(EngineOptions { paths: paths.clone(), host: "app".into(), backend: None }) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Lumos can't start the engine: {e}");
            let _ = std::fs::remove_file(&paths.socket);
            std::process::exit(1);
        }
    };
    tracing::info!(socket = %paths.socket.display(), backend = engine.backend_name(), "engine ready");
    tauri::async_runtime::spawn(lumos_rpc::serve(listener, engine.clone() as Arc<dyn Dispatch>));

    let me = ClientInfo { kind: ClientKind::Ui, name: "lumos-app".into(), version: env!("CARGO_PKG_VERSION").into() };
    let app_state = Arc::new(App { engine: engine.clone(), me, paths, quitting: AtomicBool::new(false) });

    let built = tauri::Builder::default()
        .manage(app_state.clone())
        .invoke_handler(tauri::generate_handler![call, show_window, hide_window, set_window_size, pick_folder, pick_files, open_path, open_privacy_pane, quit])
        .setup(move |app| {
            // Forward engine events to every window.
            let mut rx = engine.events();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            let _ = handle.emit("engine-event", &ev);
                        }
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
            });
            // Toolbar starts bottom-center on the main display.
            if let Some(tb) = app.get_webview_window("toolbar") {
                if let Ok(Some(m)) = tb.primary_monitor() {
                    let f = m.scale_factor();
                    let (w, h) = (m.size().width as f64 / f, m.size().height as f64 / f);
                    let _ = tb.set_position(LogicalPosition::new((w - 480.0) / 2.0, h - 150.0));
                    // The face bubble starts bottom-left, clear of the toolbar.
                    if let Some(b) = app.get_webview_window("bubble") {
                        let _ = b.set_position(LogicalPosition::new(48.0, h - 290.0));
                    }
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the library window hides it; the toolbar keeps the app alive.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Lumos app");

    built.run(move |app, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            if !app_state.quitting.swap(true, Ordering::SeqCst) {
                api.prevent_exit();
                let (handle, state) = (app.clone(), app_state.clone());
                tauri::async_runtime::spawn(async move {
                    state.shutdown().await;
                    handle.exit(0);
                });
            }
        }
    });
}
