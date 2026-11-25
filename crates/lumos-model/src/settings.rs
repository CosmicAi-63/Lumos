//! `settings.json`. Every section has defaults so a partial file is valid.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::sources::CaptureSource;

pub const SETTINGS_SCHEMA: &str = "lumos.settings/1";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Settings {
    pub schema: String,
    pub storage: Storage,
    pub capture: Capture,
    pub camera: Camera,
    pub audio: Audio,
    pub recording: Recording,
    pub output: Output,
    pub shortcuts: Shortcuts,
    pub automation: Automation,
    pub ui: Ui,
    pub jobs: Jobs,
    pub advanced: Advanced,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: SETTINGS_SCHEMA.into(),
            storage: Storage::default(),
            capture: Capture::default(),
            camera: Camera::default(),
            audio: Audio::default(),
            recording: Recording::default(),
            output: Output::default(),
            shortcuts: Shortcuts::default(),
            automation: Automation::default(),
            ui: Ui::default(),
            jobs: Jobs::default(),
            advanced: Advanced::default(),
        }
    }
}

/// Where recordings are saved and how they are named.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Storage {
    /// Folder new recordings are saved to. `~` expands to the home folder.
    pub save_folder: String,
    /// Placeholders: `{title}`, `{date:<strftime>}`.
    pub file_name_template: String,
    pub default_title: String,
    /// `always`, `never`, or `days:N`.
    pub keep_source_tracks: String,
    pub subfolders: Subfolders,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            save_folder: "~/Movies/Lumos".into(),
            file_name_template: "{title} — {date:%Y-%m-%d %H.%M}".into(),
            default_title: "Recording".into(),
            keep_source_tracks: "always".into(),
            subfolders: Subfolders::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Subfolders {
    None,
    YearMonth,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Capture {
    pub default_source: CaptureSource,
    /// 24, 30 or 60.
    pub fps: u32,
    /// `native`, `1080p`, `1440p` or `4k`.
    pub max_resolution: String,
    pub show_cursor: bool,
    pub highlight_clicks: bool,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            default_source: CaptureSource::default(),
            fps: 30,
            max_resolution: "native".into(),
            show_cursor: true,
            highlight_clicks: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CameraShape {
    Circle,
    RoundedRect,
    Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CameraSize {
    S,
    M,
    L,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Anchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    BottomCenter,
    Free,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Placement {
    pub anchor: Anchor,
    /// 0.0–1.0 of the frame.
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Camera {
    pub enabled: bool,
    /// `null` = system default camera.
    pub device_id: Option<String>,
    pub shape: CameraShape,
    pub size: CameraSize,
    pub position: Placement,
    pub mirror: bool,
    /// `720p` or `1080p`.
    pub resolution: String,
    /// Bubble is captured with the screen. Forced off for window sources.
    pub burn_in: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            enabled: true,
            device_id: None,
            shape: CameraShape::Circle,
            size: CameraSize::M,
            position: Placement { anchor: Anchor::BottomLeft, x: 0.04, y: 0.78 },
            mirror: true,
            resolution: "720p".into(),
            burn_in: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Audio {
    pub mic_enabled: bool,
    pub mic_device_id: Option<String>,
    pub system_audio: bool,
    pub mic_gain_db: f64,
    pub system_gain_db: f64,
}

impl Default for Audio {
    fn default() -> Self {
        Self { mic_enabled: true, mic_device_id: None, system_audio: false, mic_gain_db: 0.0, system_gain_db: -6.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Recording {
    pub countdown_seconds: u32,
    pub max_duration_minutes: Option<u32>,
    /// Refuse to start (and auto-stop) below this much free space.
    #[serde(rename = "minFreeDiskMB")]
    pub min_free_disk_mb: u64,
}

impl Default for Recording {
    fn default() -> Self {
        Self { countdown_seconds: 3, max_duration_minutes: None, min_free_disk_mb: 2048 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AfterStop {
    Notify,
    Reveal,
    OpenLibrary,
    Nothing,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Output {
    pub auto_export: bool,
    pub preset: String,
    pub after_stop: AfterStop,
}

impl Default for Output {
    fn default() -> Self {
        Self { auto_export: true, preset: "default".into(), after_stop: AfterStop::Notify }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Shortcuts {
    pub start_stop: String,
    pub pause_resume: String,
    pub cancel: String,
    pub toggle_camera: String,
    pub add_marker: String,
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            start_stop: "CmdOrCtrl+Shift+L".into(),
            pause_resume: "CmdOrCtrl+Shift+P".into(),
            cancel: "CmdOrCtrl+Shift+Backspace".into(),
            toggle_camera: "CmdOrCtrl+Shift+C".into(),
            add_marker: "CmdOrCtrl+Shift+M".into(),
        }
    }
}

/// What agents may do. Only the desktop UI can change these.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Automation {
    pub cli_is_trusted: bool,
    pub agent_may_start_recording: bool,
    pub agent_may_delete: bool,
    pub agent_may_snapshot_screen: bool,
}

impl Default for Automation {
    fn default() -> Self {
        Self { cli_is_trusted: true, agent_may_start_recording: false, agent_may_delete: false, agent_may_snapshot_screen: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolbarPosition {
    pub anchor: Anchor,
    pub dx: f64,
    pub dy: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Ui {
    /// `system`, `light` or `dark`.
    pub theme: String,
    pub toolbar_position: ToolbarPosition,
    pub toolbar_collapsed: bool,
    pub launch_at_login: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            toolbar_position: ToolbarPosition { anchor: Anchor::BottomCenter, dx: 0.0, dy: -24.0 },
            toolbar_collapsed: false,
            launch_at_login: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Jobs {
    /// Heavy jobs (export, import, transcribe) at once. Paused while recording.
    pub max_concurrent: u32,
}

impl Default for Jobs {
    fn default() -> Self {
        Self { max_concurrent: 1 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Advanced {
    /// `hardware` or `software`.
    pub encoder: String,
    pub screen_bitrate_kbps: Option<u32>,
    pub camera_bitrate_kbps: Option<u32>,
    pub log_level: String,
    /// Path to a whisper.cpp `ggml-*.bin` model, used by transcripts when none is passed.
    pub whisper_model_path: Option<String>,
}

impl Default for Advanced {
    fn default() -> Self {
        Self { encoder: "hardware".into(), screen_bitrate_kbps: None, camera_bitrate_kbps: None, log_level: "info".into(), whisper_model_path: None }
    }
}
