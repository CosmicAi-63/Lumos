use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Sent to subscribed clients as the JSON-RPC notification `event`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    #[serde(rename = "type")]
    pub kind: String,
    pub ts: String,
    pub data: Value,
}

impl Event {
    pub fn new(kind: &str, data: impl Serialize) -> Self {
        Self { kind: kind.into(), ts: crate::now_rfc3339(), data: serde_json::to_value(data).unwrap_or(Value::Null) }
    }
}

pub const RECORDER_STATE_CHANGED: &str = "recorder.stateChanged";
pub const RECORDER_TICK: &str = "recorder.tick";
pub const RECORDER_WARNING: &str = "recorder.warning";
pub const RECORDING_READY: &str = "recording.ready";
pub const RECORDING_FAILED: &str = "recording.failed";
pub const JOB_PROGRESS: &str = "job.progress";
pub const JOB_FINISHED: &str = "job.finished";
pub const SETTINGS_CHANGED: &str = "settings.changed";
pub const LIBRARY_CHANGED: &str = "library.changed";
