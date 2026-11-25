//! Params and result types for tools. Doc comments become CLI help and MCP descriptions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::recorder::ClientInfo;
use crate::sources::{CaptureSource, PermissionStatus};

/// No parameters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

// ---- session / system --------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HelloParams {
    pub client: ClientInfo,
    pub protocol: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HelloResult {
    pub engine_version: String,
    pub protocol: u32,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeParams {
    /// Topic prefixes, e.g. `recorder.` or `job.`. `*` subscribes to everything.
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeResult {
    pub subscribed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub version: String,
    pub platform: String,
    pub arch: String,
    /// `app` or `lumosd`.
    pub engine_host: String,
    /// Capture backend in use: `macos`, `fake`, ...
    pub backend: String,
    pub home: String,
    pub settings_path: String,
    pub socket_path: String,
    pub logs_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub code: String,
    pub message: String,
    /// Human hint or the tool to run.
    pub fix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveFolderHealth {
    pub path: String,
    pub writable: bool,
    #[serde(rename = "freeMB")]
    pub free_mb: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub backend: String,
    pub permissions: PermissionStatus,
    pub save_folder: SaveFolderHealth,
    /// `ffmpeg -version` first line, if found.
    pub ffmpeg: Option<String>,
    /// Recordings left in `.lumos-inprogress` (crashed, or the one recording now).
    pub in_progress: Vec<String>,
    pub problems: Vec<Problem>,
}

// ---- recording -------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Toggle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

/// Start a recording. Every field falls back to Settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CaptureSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<Toggle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic: Option<Toggle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_audio: Option<bool>,
    /// 0 = no countdown. Agents always get at least 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub countdown_seconds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Stop automatically after this many seconds of recorded time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_seconds: Option<u64>,
    /// One-off override of `storage.saveFolder`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_folder: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StopParams {
    /// Block until the file is saved.
    #[serde(default)]
    pub wait: bool,
    /// Default 55000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StopResult {
    pub recording_id: String,
    pub finalize_job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmParams {
    /// Must be `true`; destructive.
    pub confirm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CancelResult {
    pub recording_id: String,
    pub discarded: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarkParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarkResult {
    pub marker_id: String,
    pub at_us: i64,
}

/// Result of a finalize job, also the payload of `recording.ready`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecordingReady {
    pub recording_id: String,
    pub output_path: String,
    pub bundle_path: String,
    pub duration_us: i64,
}

// ---- settings --------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsGetParams {
    /// Dotted path, e.g. `storage.saveFolder`. Omit for everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsSetParams {
    /// Dotted paths to new values: `{ "storage.saveFolder": "~/Videos/Lumos" }`.
    pub patch: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsResetParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChanged {
    pub changed: Vec<String>,
    pub settings: Value,
}

// ---- jobs ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobIdParams {
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobWaitParams {
    pub job_id: String,
    /// Default 55000. On timeout the job keeps running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}
