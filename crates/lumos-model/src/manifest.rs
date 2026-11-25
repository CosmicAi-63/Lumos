//! `manifest.json` inside each `.lumos` bundle. Source of truth for one recording.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::sources::CaptureSource;

pub const MANIFEST_SCHEMA: &str = "lumos.recording/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RecordingStatus {
    Recording,
    Paused,
    Finalizing,
    Ready,
    Failed,
    Recovered,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Origin {
    Recorded,
    #[serde(rename_all = "camelCase")]
    Imported { from: String, importer: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptureInfo {
    pub source: CaptureSource,
    /// Human name of the resolved source, e.g. "Built-in Retina Display".
    pub source_name: String,
    pub fps: u32,
    pub show_cursor: bool,
    pub camera_burned_in: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub id: String,
    pub start_us: i64,
    pub end_us: i64,
    pub wall_start: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    Screen,
    Camera,
    Mic,
    SystemAudio,
}

impl TrackKind {
    /// Folder name under `tracks/`.
    pub fn dir(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Camera => "camera",
            Self::Mic => "mic",
            Self::SystemAudio => "system",
        }
    }
    pub fn is_video(self) -> bool {
        matches!(self, Self::Screen | Self::Camera)
    }
    /// File extension for segment files.
    pub fn ext(self) -> &'static str {
        if self.is_video() { "mp4" } else { "m4a" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub kind: TrackKind,
    pub codec: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    #[serde(default)]
    pub offset_us: i64,
    /// Segment files, relative to the bundle.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    pub id: String,
    pub at_us: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Range {
    pub start_us: i64,
    pub end_us: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Edits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<Range>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cuts: Vec<Range>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutputFile {
    pub kind: String,
    pub preset: String,
    /// Relative to the bundle, e.g. `../Demo.mp4`.
    pub path: String,
    pub created_at: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    pub at_us: i64,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema: String,
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub status: RecordingStatus,
    pub duration_us: i64,
    pub origin: Origin,
    pub app: AppInfo,
    pub capture: CaptureInfo,
    pub segments: Vec<Segment>,
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default)]
    pub edits: Edits,
    #[serde(default)]
    pub outputs: Vec<OutputFile>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}
