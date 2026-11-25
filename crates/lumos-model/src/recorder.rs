use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::sources::CaptureSource;

/// Recorder states. Finalize is a job, not a state: see docs/02-data-model.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RecorderState {
    Idle,
    Preparing,
    Countdown,
    Recording,
    Paused,
    Stopping,
    Discarding,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ClientKind {
    Ui,
    Cli,
    Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub kind: ClientKind,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSource {
    pub source: CaptureSource,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStats {
    pub bytes_written: u64,
    pub dropped_frames: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStatus {
    pub state: RecorderState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_id: Option<String>,
    /// Recorded time so far, paused time excluded.
    pub elapsed_us: i64,
    pub segment_index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ActiveSource>,
    pub camera_enabled: bool,
    pub mic_enabled: bool,
    pub system_audio: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_by: Option<ClientInfo>,
    pub stats: RecorderStats,
    /// Finalize jobs of earlier recordings that are still being saved.
    pub pending_finalize: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<crate::ToolError>,
}

impl RecorderStatus {
    pub fn idle() -> Self {
        Self {
            state: RecorderState::Idle,
            recording_id: None,
            elapsed_us: 0,
            segment_index: 0,
            source: None,
            camera_enabled: false,
            mic_enabled: false,
            system_audio: false,
            started_by: None,
            stats: RecorderStats::default(),
            pending_finalize: Vec::new(),
            last_error: None,
        }
    }
}
