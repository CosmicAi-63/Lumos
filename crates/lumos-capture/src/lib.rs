//! Capture backends.
//!
//! The boundary is a *session* with segment-level calls, not individual frames:
//! a native backend (the macOS Swift shim) runs its capture streams and its
//! hardware writer together, and Rust only drives the lifecycle. See
//! docs/03-recording-pipeline.md.

pub mod fake;
#[cfg(target_os = "macos")]
pub mod macos;

use std::path::PathBuf;
use std::sync::Arc;

use lumos_model::ToolResult;
use lumos_model::manifest::TrackKind;
use lumos_model::sources::{CaptureSource, Device, Display, PermissionStatus};

/// What one recording captures.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub source: CaptureSource,
    pub fps: u32,
    /// `Some(device)` = record the camera; inner `None` = system default.
    pub camera: Option<Option<String>>,
    pub mic: Option<Option<String>>,
    pub system_audio: bool,
    /// The in-progress bundle; segment files go under `tracks/<kind>/`.
    pub bundle_dir: PathBuf,
}

/// One track a session writes.
#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub kind: TrackKind,
    pub codec: String,
    pub device_name: Option<String>,
}

/// Files written for one segment.
#[derive(Debug, Clone)]
pub struct SegmentOutput {
    /// Exact recorded length of the segment.
    pub duration_us: i64,
    /// Bundle-relative file per track.
    pub files: Vec<(TrackKind, String)>,
    pub bytes: u64,
}

#[async_trait::async_trait]
pub trait CaptureBackend: Send + Sync {
    /// `fake`, `macos`, ...
    fn name(&self) -> &'static str;
    /// False for backends that can't record yet (the macOS stub).
    fn can_record(&self) -> bool;
    async fn permissions(&self) -> PermissionStatus;
    /// Ask the OS for Screen Recording. On macOS this shows the system dialog and registers the app
    /// in the Screen Recording list. Does nothing on backends that need no permission.
    async fn request_screen_access(&self) {}
    async fn displays(&self) -> ToolResult<Vec<Display>>;
    /// Windows that can be recorded (visible, normal windows of other apps).
    async fn windows(&self) -> ToolResult<Vec<lumos_model::sources::WindowInfo>> {
        Ok(Vec::new())
    }
    async fn cameras(&self) -> ToolResult<Vec<Device>>;
    async fn microphones(&self) -> ToolResult<Vec<Device>>;
    /// Open all streams. They warm up while the countdown runs; nothing is written yet.
    async fn open(&self, config: SessionConfig) -> ToolResult<Box<dyn CaptureSession>>;
}

#[async_trait::async_trait]
pub trait CaptureSession: Send {
    /// Human name of the resolved source.
    fn source_name(&self) -> String;
    fn tracks(&self) -> Vec<TrackInfo>;
    /// Start writing segment `index` (`000`, `001`, … per track).
    async fn begin_segment(&mut self, index: u32) -> ToolResult<()>;
    /// Finish the current segment; every file is complete and playable afterwards.
    async fn end_segment(&mut self) -> ToolResult<SegmentOutput>;
    /// Close streams. Called after the last `end_segment`.
    async fn close(self: Box<Self>) -> ToolResult<()>;
}

/// Choose a backend: `LUMOS_BACKEND=fake|macos`, default per OS.
pub fn select_backend() -> Arc<dyn CaptureBackend> {
    let wanted = std::env::var("LUMOS_BACKEND").unwrap_or_default();
    match wanted.as_str() {
        "fake" => Arc::new(fake::FakeBackend),
        #[cfg(target_os = "macos")]
        "" | "macos" => Arc::new(macos::MacBackend),
        _ => Arc::new(fake::FakeBackend),
    }
}

pub(crate) fn segment_file(kind: TrackKind, index: u32) -> String {
    format!("tracks/{}/{index:03}.{}", kind.dir(), kind.ext())
}
