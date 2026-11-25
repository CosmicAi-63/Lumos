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

// ---- library ---------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecordingSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub duration_us: i64,
    pub status: crate::manifest::RecordingStatus,
    /// `recorded` or `imported`.
    pub origin: String,
    pub tags: Vec<String>,
    pub bundle_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_path: Option<String>,
    pub bytes: u64,
    /// A transcript exists (`transcript.get` returns it).
    #[serde(default)]
    pub has_transcript: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryListParams {
    /// Case-insensitive match on the title or the transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Recordings that have all of these tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// `recorded` or `imported`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// `newest` (default), `oldest`, `longest` or `title`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// Default 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Skip this many results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryListResult {
    pub items: Vec<RecordingSummary>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordingIdParams {
    /// Recording id, an unambiguous id prefix, or `last`.
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecordingDetails {
    pub summary: RecordingSummary,
    pub manifest: crate::manifest::Manifest,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryUpdateParams {
    pub id: String,
    /// Renaming also renames the MP4 and the bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Replaces all tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryDeleteParams {
    pub id: String,
    /// Must be `true`.
    pub confirm: bool,
    /// Keep the MP4 and move only the source tracks to the Trash.
    #[serde(default)]
    pub keep_output: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDeleteResult {
    pub id: String,
    /// Where the items went (the Trash).
    pub trashed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryRevealParams {
    pub id: String,
    /// `output` (default) or `bundle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub which: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PathResult {
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoverParams {
    /// Recording id to recover; default: every unfinished recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default)]
    pub wait: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecoverResult {
    pub job_id: String,
    /// Filled when `wait` is set and the job finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered: Option<Vec<RecordingSummary>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<Vec<String>>,
}

// ---- export ----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreset {
    pub id: String,
    pub name: String,
    /// `mp4`, `gif` or `audio`.
    pub kind: String,
    /// Downscale to at most this height. `None` keeps the size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    /// Video bitrate in kbit/s (mp4 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_kbps: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportParams {
    pub id: String,
    /// Preset id from `export.presets`.
    pub preset: String,
    /// Default: next to the recording's MP4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    /// Export only this part of the recording, in seconds. The original is never changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<TimeRange>,
    #[serde(default)]
    pub wait: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// A span of the recording in seconds from its start. `endSeconds` defaults to the end.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeRange {
    #[serde(default)]
    pub start_seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_us: Option<i64>,
}

// ---- transcripts -----------------------------------------------------------

/// Transcribe a recording locally with whisper.cpp (job).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptCreateParams {
    pub id: String,
    /// Path to a whisper.cpp `ggml-*.bin` model. Default: `$LUMOS_WHISPER_MODEL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Language code such as `en`, or `auto` (default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub wait: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptCreateResult {
    pub job_id: String,
    /// The `.srt` file, once the job is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptGetParams {
    pub id: String,
    /// `txt` (default), `srt` or `vtt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptGetResult {
    pub format: String,
    pub text: String,
}

// ---- import ----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportFormat {
    pub importer: String,
    pub extensions: Vec<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportParams {
    /// Video files to bring into the library.
    pub paths: Vec<String>,
    /// Default: the file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub wait: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recordings: Option<Vec<RecordingSummary>>,
    /// `(path, reason)` pairs for files that could not be imported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<Vec<String>>,
}
