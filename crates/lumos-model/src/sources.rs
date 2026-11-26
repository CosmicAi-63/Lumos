use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// What to record. `displayId: "main"` means the main display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CaptureSource {
    #[serde(rename_all = "camelCase")]
    Display { display_id: String },
    #[serde(rename_all = "camelCase")]
    Window {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window_id: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title_contains: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Area { display_id: String, rect: Rect },
}

impl Default for CaptureSource {
    fn default() -> Self {
        Self::Display { display_id: "main".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Display {
    pub id: String,
    pub name: String,
    pub main: bool,
    pub bounds: Rect,
    pub scale: f64,
    pub refresh_hz: f64,
}

/// A window that can be recorded. Lumos's own windows are never listed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    pub id: u64,
    pub app: String,
    pub title: String,
    pub bounds: Rect,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PermissionState {
    Granted,
    Denied,
    NotRequested,
    Restricted,
    /// The backend can't check this permission yet.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    Screen,
    Camera,
    Microphone,
    Accessibility,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PermissionStatus {
    pub screen: PermissionState,
    pub camera: PermissionState,
    pub microphone: PermissionState,
    pub accessibility: PermissionState,
}

impl PermissionStatus {
    pub fn all(state: PermissionState) -> Self {
        Self { screen: state, camera: state, microphone: state, accessibility: state }
    }
}
