//! macOS backend. Displays and the Screen Recording check use CoreGraphics
//! directly; capture, cameras, microphones and the camera/mic permissions go
//! through `lumos-capture-helper` (ScreenCaptureKit, AVFoundation and
//! AVAssetWriter in Swift, see swift/helper.swift), driven with JSON lines.
//!
//! Helper lookup: `LUMOS_CAPTURE_HELPER`, then next to the running binary, then
//! the path baked in by build.rs (development).

use std::ffi::c_void;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use lumos_model::manifest::TrackKind;
use lumos_model::sources::{CaptureSource, Device, Display, PermissionState, PermissionStatus, Rect, WindowInfo};
use lumos_model::{ErrorCode, ToolError, ToolResult};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::{CaptureBackend, CaptureSession, SegmentOutput, SessionConfig, TrackInfo};

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGDisplayCopyDisplayMode(display: u32) -> *mut c_void;
    fn CGDisplayModeGetPixelWidth(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetRefreshRate(mode: *mut c_void) -> f64;
    fn CGDisplayModeRelease(mode: *mut c_void);
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Longest a single helper call may take (a segment's writers finishing).
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

pub struct MacBackend;

fn helper_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LUMOS_CAPTURE_HELPER") {
        return Some(p.into());
    }
    let beside = std::env::current_exe().ok()?.parent()?.join("lumos-capture-helper");
    if beside.is_file() {
        return Some(beside);
    }
    option_env!("LUMOS_HELPER_BUILD_PATH").map(PathBuf::from).filter(|p| p.is_file())
}

fn helper_missing() -> ToolError {
    ToolError::backend("lumos-capture-helper was not found or could not be built (needs the Swift toolchain); set LUMOS_CAPTURE_HELPER")
}

/// One helper process.
struct Helper {
    child: Child,
    stdin: ChildStdin,
    out: tokio::io::Lines<BufReader<ChildStdout>>,
}

impl Helper {
    async fn spawn() -> ToolResult<Self> {
        let path = helper_path().ok_or_else(helper_missing)?;
        let mut child = Command::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| ToolError::backend(format!("cannot start {}: {e}", path.display())))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let out = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
        Ok(Self { child, stdin, out })
    }

    async fn call(&mut self, req: Value) -> ToolResult<Value> {
        let mut line = req.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.map_err(|e| ToolError::backend(format!("capture helper is gone: {e}")))?;
        let reply = tokio::time::timeout(CALL_TIMEOUT, self.out.next_line())
            .await
            .map_err(|_| ToolError::new(ErrorCode::Timeout, "capture helper did not answer in time"))?
            .map_err(|e| ToolError::backend(format!("capture helper read failed: {e}")))?
            .ok_or_else(|| ToolError::backend("capture helper exited unexpectedly"))?;
        let v: Value = serde_json::from_str(&reply).map_err(|e| ToolError::backend(format!("bad helper reply: {e}: {reply}")))?;
        if v["ok"].as_bool() == Some(true) {
            return Ok(v);
        }
        let code = serde_json::from_value::<ErrorCode>(v["code"].clone()).unwrap_or(ErrorCode::BackendError);
        Err(ToolError::new(code, v["message"].as_str().unwrap_or("capture helper failed")))
    }

    /// Ask the helper to stop, then reap it.
    async fn quit(mut self) {
        let _ = self.call(json!({ "cmd": "quit" })).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
    }
}

/// Run one command on a short-lived helper.
async fn once(cmd: Value) -> ToolResult<Value> {
    let mut h = Helper::spawn().await?;
    let r = h.call(cmd).await;
    h.quit().await;
    r
}

fn perm(v: &Value) -> PermissionState {
    match v.as_str() {
        Some("granted") => PermissionState::Granted,
        Some("denied") => PermissionState::Denied,
        Some("restricted") => PermissionState::Restricted,
        Some("notRequested") => PermissionState::NotRequested,
        _ => PermissionState::Unknown,
    }
}

fn devices(v: &Value, key: &str) -> Vec<Device> {
    v[key]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| Device {
            id: d["id"].as_str().unwrap_or_default().into(),
            name: d["name"].as_str().unwrap_or_default().into(),
            default: d["default"].as_bool().unwrap_or(false),
        })
        .collect()
}

#[async_trait::async_trait]
impl CaptureBackend for MacBackend {
    fn name(&self) -> &'static str {
        "macos"
    }

    fn can_record(&self) -> bool {
        helper_path().is_some()
    }

    async fn permissions(&self) -> PermissionStatus {
        // SAFETY: plain C call with no arguments.
        let screen = if unsafe { CGPreflightScreenCaptureAccess() } { PermissionState::Granted } else { PermissionState::Denied };
        let (camera, microphone) = match once(json!({ "cmd": "permissions" })).await {
            Ok(v) => (perm(&v["camera"]), perm(&v["microphone"])),
            Err(_) => (PermissionState::Unknown, PermissionState::Unknown),
        };
        PermissionStatus { screen, camera, microphone, accessibility: PermissionState::Unknown }
    }

    async fn request_screen_access(&self) {
        // SAFETY: plain C call with no arguments. It returns at once; the dialog is shown by macOS.
        let _ = unsafe { CGRequestScreenCaptureAccess() };
    }

    async fn displays(&self) -> ToolResult<Vec<Display>> {
        let mut ids = [0u32; 16];
        let mut count = 0u32;
        // SAFETY: buffer of 16 ids, CoreGraphics writes at most `max` entries.
        let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
        if err != 0 {
            return Err(ToolError::backend(format!("CGGetActiveDisplayList failed ({err})")));
        }
        // SAFETY: no arguments.
        let main = unsafe { CGMainDisplayID() };
        let mut out = Vec::new();
        for (i, &id) in ids[..count as usize].iter().enumerate() {
            // SAFETY: `id` came from CGGetActiveDisplayList; the mode is released below.
            let (bounds, scale, hz) = unsafe {
                let b = CGDisplayBounds(id);
                let mode = CGDisplayCopyDisplayMode(id);
                let (px, hz) = if mode.is_null() {
                    (b.size.width as usize, 0.0)
                } else {
                    let r = (CGDisplayModeGetPixelWidth(mode), CGDisplayModeGetRefreshRate(mode));
                    CGDisplayModeRelease(mode);
                    r
                };
                (b, if b.size.width > 0.0 { px as f64 / b.size.width } else { 1.0 }, hz)
            };
            out.push(Display {
                id: id.to_string(),
                name: if id == main { "Main display".into() } else { format!("Display {}", i + 1) },
                main: id == main,
                bounds: Rect { x: bounds.origin.x, y: bounds.origin.y, w: bounds.size.width, h: bounds.size.height },
                scale,
                refresh_hz: hz,
            });
        }
        Ok(out)
    }

    async fn windows(&self) -> ToolResult<Vec<WindowInfo>> {
        let reply = once(json!({ "cmd": "windows" })).await?;
        Ok(reply["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| {
                Some(WindowInfo {
                    id: w["id"].as_u64()?,
                    app: w["app"].as_str().unwrap_or_default().into(),
                    title: w["title"].as_str().unwrap_or_default().into(),
                    bounds: Rect {
                        x: w["bounds"]["x"].as_f64().unwrap_or(0.0),
                        y: w["bounds"]["y"].as_f64().unwrap_or(0.0),
                        w: w["bounds"]["w"].as_f64().unwrap_or(0.0),
                        h: w["bounds"]["h"].as_f64().unwrap_or(0.0),
                    },
                })
            })
            .collect())
    }

    async fn cameras(&self) -> ToolResult<Vec<Device>> {
        Ok(devices(&once(json!({ "cmd": "devices" })).await?, "cameras"))
    }

    async fn microphones(&self) -> ToolResult<Vec<Device>> {
        Ok(devices(&once(json!({ "cmd": "devices" })).await?, "mics"))
    }

    async fn open(&self, config: SessionConfig) -> ToolResult<Box<dyn CaptureSession>> {
        let source = match &config.source {
            CaptureSource::Display { display_id } => json!({ "kind": "display", "displayId": display_id }),
            CaptureSource::Area { display_id, rect } => {
                json!({ "kind": "area", "displayId": display_id, "rect": { "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h } })
            }
            CaptureSource::Window { window_id, app, title_contains } => {
                json!({ "kind": "window", "windowId": window_id, "app": app, "titleContains": title_contains })
            }
        };
        let device = |d: &Option<Option<String>>| d.as_ref().map(|id| json!({ "deviceId": id }));
        let mut helper = Helper::spawn().await?;
        let reply = helper
            .call(json!({
                "cmd": "open",
                "dir": config.bundle_dir,
                "source": source,
                "fps": config.fps,
                "mic": device(&config.mic),
                "camera": device(&config.camera),
                "system": config.system_audio,
            }))
            .await;
        let reply = match reply {
            Ok(r) => r,
            Err(e) => {
                helper.quit().await;
                return Err(e);
            }
        };
        let tracks = reply["tracks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                Some(TrackInfo {
                    kind: serde_json::from_value::<TrackKind>(t["kind"].clone()).ok()?,
                    codec: t["codec"].as_str().unwrap_or_default().into(),
                    device_name: t["deviceName"].as_str().map(str::to_owned),
                })
            })
            .collect();
        Ok(Box::new(MacSession { helper, name: reply["sourceName"].as_str().unwrap_or("Screen").to_owned(), tracks }))
    }
}

struct MacSession {
    helper: Helper,
    name: String,
    tracks: Vec<TrackInfo>,
}

#[async_trait::async_trait]
impl CaptureSession for MacSession {
    fn source_name(&self) -> String {
        self.name.clone()
    }

    fn tracks(&self) -> Vec<TrackInfo> {
        self.tracks.clone()
    }

    async fn begin_segment(&mut self, index: u32) -> ToolResult<()> {
        self.helper.call(json!({ "cmd": "begin", "index": index })).await.map(|_| ())
    }

    async fn end_segment(&mut self) -> ToolResult<SegmentOutput> {
        let v = self.helper.call(json!({ "cmd": "end" })).await?;
        let files = v["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| Some((serde_json::from_value::<TrackKind>(f[0].clone()).ok()?, f[1].as_str()?.to_owned())))
            .collect();
        Ok(SegmentOutput {
            duration_us: v["durationUs"].as_i64().unwrap_or(0),
            files,
            bytes: v["bytes"].as_u64().unwrap_or(0),
        })
    }

    async fn close(self: Box<Self>) -> ToolResult<()> {
        self.helper.quit().await;
        Ok(())
    }
}
