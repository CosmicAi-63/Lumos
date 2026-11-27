//! Lumos desktop app: hosts the engine in-process (so the camera, microphone and
//! screen permissions belong to this one app), serves the same socket the `lumos`
//! CLI and MCP server use, and shows the toolbar, camera bubble and library.
//!
//! The web UI never touches devices or files itself: every action is one engine
//! tool, reached through the `call` command. See docs/06-ui-ux.md.

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lumos_core::{Engine, EngineOptions};
use lumos_model::ToolError;
use lumos_model::recorder::{ClientInfo, ClientKind};
use lumos_rpc::Dispatch;
use lumos_rpc::paths::Paths;
use serde_json::Value;
use tauri::image::Image;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{ActivationPolicy, AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, RunEvent, State};
use tokio::sync::broadcast::error::RecvError;

/// When the menu-bar popover last closed on its own (focus lost), in ms since the epoch.
static MENU_HIDDEN_AT: AtomicU64 = AtomicU64::new(0);
/// When the popover was last shown. A menu-bar app is not active yet at that moment, so the window
/// reports "focus lost" right away; blur events in this grace period are ignored.
static MENU_SHOWN_AT: AtomicU64 = AtomicU64::new(0);
/// The library / settings window is in front. The idle toolbar steps aside while it is.
static MAIN_FOCUSED: AtomicBool = AtomicBool::new(false);

/// Tell the toolbar which Lumos windows are up, so it can decide whether to show itself while idle.
fn windows_changed(app: &AppHandle) {
    let visible = |label: &str| app.get_webview_window(label).and_then(|w| w.is_visible().ok()).unwrap_or(false);
    let payload = serde_json::json!({
        "menu": visible("menu"),
        "notes": visible("notes"),
        "mainFocused": MAIN_FOCUSED.load(Ordering::SeqCst) && visible("main"),
    });
    let _ = app.emit_to("toolbar", "windows-changed", payload);
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Open the popover under the menu-bar icon, or close it if it is open.
fn toggle_menu(app: &AppHandle, icon: tauri::Rect) {
    let Some(w) = app.get_webview_window("menu") else { return };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        windows_changed(app);
        return;
    }
    // Clicking the icon while the popover is open first takes its focus, which already closed it.
    if now_ms().saturating_sub(MENU_HIDDEN_AT.load(Ordering::SeqCst)) < 300 {
        return;
    }
    let scale = w.scale_factor().unwrap_or(2.0);
    let pos = icon.position.to_physical::<f64>(scale);
    let size = icon.size.to_physical::<f64>(scale);
    let width = w.outer_size().map(|s| s.width as f64).unwrap_or(380.0 * scale);
    let x = (pos.x + size.width / 2.0 - width / 2.0).max(4.0 * scale);
    let y = pos.y + size.height;
    let _ = w.set_position(PhysicalPosition::new(x, y));
    MENU_SHOWN_AT.store(now_ms(), Ordering::SeqCst);
    let _ = w.show();
    let _ = w.set_focus();
    windows_changed(app);
}

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
    if label == "main" || label == "menu" || label == "notes" {
        w.set_focus().map_err(|e| e.to_string())?;
        if label == "notes" {
            let _ = app.emit_to("notes", "window-shown", ());
        }
    } else if let Some(menu) = app.get_webview_window("menu") {
        // Showing the bubble or toolbar must not pull focus away from an open popover.
        if menu.is_visible().unwrap_or(false) {
            let _ = menu.set_focus();
        }
    }
    windows_changed(&app);
    Ok(())
}

#[tauri::command]
fn hide_window(app: AppHandle, label: String) -> Result<(), String> {
    app.get_webview_window(&label).ok_or_else(|| format!("no window '{label}'"))?.hide().map_err(|e| e.to_string())?;
    windows_changed(&app);
    Ok(())
}

#[tauri::command]
fn set_window_size(app: AppHandle, label: String, width: f64, height: f64) -> Result<(), String> {
    let w = app.get_webview_window(&label).ok_or_else(|| format!("no window '{label}'"))?;
    // macOS keeps the bottom edge when a window resizes; keep the top edge instead, so the toolbar
    // grows and shrinks downward without moving.
    let top_left = w.outer_position().ok();
    w.set_size(LogicalSize::new(width.clamp(40.0, 4000.0), height.clamp(40.0, 4000.0))).map_err(|e| e.to_string())?;
    if let Some(p) = top_left {
        let _ = w.set_position(p);
    }
    // Once, center the toolbar on the left edge of the main display, sized for its recording layout
    // (about 340 points tall). After that it stays wherever the user drags it.
    if label == "toolbar" && !TOOLBAR_PLACED.swap(true, Ordering::SeqCst) {
        if let Ok(Some(m)) = w.primary_monitor() {
            let f = m.scale_factor();
            let h = m.size().height as f64 / f;
            let _ = w.set_position(LogicalPosition::new(16.0, ((h - 340.0) / 2.0).max(40.0)));
        }
    }
    Ok(())
}

static TOOLBAR_PLACED: AtomicBool = AtomicBool::new(false);

/// Full-screen overlay where the user drags the area to record. Returns `{x,y,w,h}` in points from
/// the top-left of the main display, or `None` if they press Esc.
#[tauri::command]
async fn pick_area(app: AppHandle) -> Result<Option<Value>, String> {
    use tauri::{Listener, WebviewUrl, WebviewWindowBuilder};
    if let Some(old) = app.get_webview_window("area") {
        let _ = old.close();
    }
    let monitor = app.primary_monitor().map_err(|e| e.to_string())?.ok_or("no display")?;
    let f = monitor.scale_factor();
    let (w, h) = (monitor.size().width as f64 / f, monitor.size().height as f64 / f);

    let (tx, rx) = tokio::sync::oneshot::channel::<Option<Value>>();
    let tx = std::sync::Mutex::new(Some(tx));
    app.once("area-result", move |ev| {
        let rect = serde_json::from_str::<Value>(ev.payload()).ok().filter(|v| v.is_object());
        if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
            let _ = tx.send(rect);
        }
    });
    let win = WebviewWindowBuilder::new(&app, "area", WebviewUrl::App("area.html".into()))
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .accept_first_mouse(true)
        .position(0.0, 0.0)
        .inner_size(w, h)
        .focused(true)
        .build()
        .map_err(|e| e.to_string())?;
    let _ = win.set_focus();
    let rect = rx.await.ok().flatten();
    let _ = win.close();
    Ok(rect)
}

/// Turn the drawing overlay on or off. It is a transparent full-screen window over the main display
/// that is recorded with the screen. Turning it off closes the window, which throws away the strokes.
#[tauri::command]
async fn set_draw_mode(app: AppHandle, on: bool) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    if !on {
        if let Some(w) = app.get_webview_window("draw") {
            w.close().map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    if app.get_webview_window("draw").is_some() {
        return Ok(());
    }
    let monitor = app.primary_monitor().map_err(|e| e.to_string())?.ok_or("no display")?;
    let f = monitor.scale_factor();
    let (w, h) = (monitor.size().width as f64 / f, monitor.size().height as f64 / f);
    let win = WebviewWindowBuilder::new(&app, "draw", WebviewUrl::App("draw.html".into()))
        // The capture helper records Lumos windows only when they carry this title.
        .title("Lumos Draw")
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .accept_first_mouse(true)
        .position(0.0, 0.0)
        .inner_size(w, h)
        .focused(true)
        .build()
        .map_err(|e| e.to_string())?;
    let _ = win.set_ignore_cursor_events(false);
    let _ = win.set_focus();
    Ok(())
}

/// Put the toolbar and face bubble one level above ordinary always-on-top windows, so the drawing
/// overlay (and clicks on it) never cover them. Must run on the main thread.
#[cfg(target_os = "macos")]
fn raise_above_overlays(w: &tauri::WebviewWindow) {
    use std::ffi::{c_char, c_void};
    #[link(name = "objc")]
    unsafe extern "C" {
        fn sel_registerName(name: *const c_char) -> *const c_void;
        fn objc_msgSend();
    }
    // NSFloatingWindowLevel is 3; always-on-top windows use it.
    const ABOVE_FLOATING: isize = 4;
    let Ok(ns) = w.ns_window() else { return };
    // SAFETY: `ns` is this window's live NSWindow, and `setLevel:` takes one NSInteger and returns nothing.
    unsafe {
        let send: unsafe extern "C" fn(*mut c_void, *const c_void, isize) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        send(ns, sel_registerName(c"setLevel:".as_ptr()), ABOVE_FLOATING);
    }
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
        .invoke_handler(tauri::generate_handler![call, show_window, hide_window, set_window_size, pick_area, set_draw_mode, pick_folder, pick_files, open_path, open_privacy_pane, quit])
        .setup(move |app| {
            // Lumos lives in the menu bar: no Dock icon, no Dock bounce.
            app.set_activation_policy(ActivationPolicy::Accessory);
            let icon = Image::from_bytes(include_bytes!("../icons/tray.png"))?;
            TrayIconBuilder::with_id("lumos")
                .icon(icon)
                .icon_as_template(true)
                .tooltip("Lumos")
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = event {
                        toggle_menu(tray.app_handle(), rect);
                    }
                })
                .build(app)?;
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
            #[cfg(target_os = "macos")]
            for label in ["toolbar", "bubble", "notes"] {
                if let Some(w) = app.get_webview_window(label) {
                    raise_above_overlays(&w);
                }
            }
            // The vertical toolbar starts on the left edge of the main display, vertically centered.
            if let Some(tb) = app.get_webview_window("toolbar") {
                if let Ok(Some(m)) = tb.primary_monitor() {
                    let f = m.scale_factor();
                    let h = m.size().height as f64 / f;
                    let _ = tb.set_position(LogicalPosition::new(16.0, (h - 360.0) / 2.0));
                    // The face bubble starts bottom-left, to the right of the toolbar.
                    if let Some(b) = app.get_webview_window("bubble") {
                        let _ = b.set_position(LogicalPosition::new(120.0, h - 290.0));
                    }
                    // Speaker notes start top-center, near the camera, so reading them looks like eye contact.
                    if let Some(n) = app.get_webview_window("notes") {
                        let w = m.size().width as f64 / f;
                        let _ = n.set_position(LogicalPosition::new(((w - 380.0) / 2.0).max(0.0), 40.0));
                    }
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // The popover closes when you click anywhere else.
            if window.label() == "menu" {
                if let tauri::WindowEvent::Focused(false) = event {
                    if now_ms().saturating_sub(MENU_SHOWN_AT.load(Ordering::SeqCst)) < 600 {
                        // Not active yet: take focus again instead of closing.
                        let _ = window.set_focus();
                    } else {
                        MENU_HIDDEN_AT.store(now_ms(), Ordering::SeqCst);
                        let _ = window.hide();
                        windows_changed(window.app_handle());
                    }
                }
            }
            if window.label() == "main" {
                if let tauri::WindowEvent::Focused(f) = event {
                    MAIN_FOCUSED.store(*f, Ordering::SeqCst);
                    windows_changed(window.app_handle());
                }
            }
            // Closing the library window hides it; the toolbar keeps the app alive.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                    MAIN_FOCUSED.store(false, Ordering::SeqCst);
                    windows_changed(window.app_handle());
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
