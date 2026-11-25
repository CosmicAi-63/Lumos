//! The tool registry: every engine operation, declared once.
//!
//! The engine dispatches by these names, the CLI builds its `call`/`tools`
//! commands from them, `xtask schemas` exports their JSON Schemas, and the MCP
//! server (M3) will list them. See docs/04-tools.md.

use std::sync::OnceLock;

use lumos_model::api::*;
use lumos_model::jobs::Job;
use lumos_model::recorder::{RecorderState, RecorderStatus};
use lumos_model::settings::Settings;
use lumos_model::sources::{Device, Display, PermissionStatus};
use schemars::{JsonSchema, Schema};
use serde::Serialize;
use serde_json::Value;

/// Side effect class. Drives MCP annotations and agent policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Effect {
    ReadOnly,
    Mutating,
    Destructive,
    /// Starts or extends screen/camera/mic capture.
    Capture,
}

/// Which clients may call a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Surfaces {
    pub ui: bool,
    pub cli: bool,
    /// Listed for MCP agents.
    pub agent: bool,
    /// Agents need a Settings → Automation opt-in (`A*` in the docs).
    pub agent_opt_in: bool,
}

impl Surfaces {
    /// UI + CLI + agents.
    pub const ALL: Self = Self { ui: true, cli: true, agent: true, agent_opt_in: false };
    /// UI + CLI + agents after opt-in.
    pub const ALL_OPT_IN: Self = Self { ui: true, cli: true, agent: true, agent_opt_in: true };
    /// UI + CLI, never agents.
    pub const HUMAN: Self = Self { ui: true, cli: true, agent: false, agent_opt_in: false };
    /// Protocol plumbing every client uses.
    pub const INTERNAL: Self = Self { ui: true, cli: true, agent: true, agent_opt_in: false };
}

pub struct ToolDef {
    /// `namespace.verb`, also the JSON-RPC method.
    pub name: &'static str,
    pub summary: &'static str,
    pub params: fn() -> Schema,
    pub result: fn() -> Schema,
    pub effect: Effect,
    /// Returns `{ jobId }`-style work; callers may pass `wait`.
    pub long_running: bool,
    pub idempotent: bool,
    pub surfaces: Surfaces,
    /// Recorder states the tool is valid in. `None` = any.
    pub states: Option<&'static [RecorderState]>,
    /// Ergonomic CLI path, e.g. `["record", "start"]`. Every tool is also reachable via `lumos call`.
    pub cli: &'static [&'static str],
}

impl ToolDef {
    /// MCP tool name: `recording.start` → `lumos_recording_start`.
    pub fn mcp_name(&self) -> String {
        format!("lumos_{}", self.name.replace('.', "_"))
    }

    /// Machine-readable description, used by `lumos tools --json` and `xtask schemas`.
    pub fn describe(&self) -> Value {
        serde_json::json!({
            "name": self.name,
            "summary": self.summary,
            "effect": self.effect,
            "longRunning": self.long_running,
            "idempotent": self.idempotent,
            "surfaces": self.surfaces,
            "states": self.states,
            "cli": self.cli,
            "mcpName": self.mcp_name(),
            "params": (self.params)(),
            "result": (self.result)(),
        })
    }
}

fn s<T: JsonSchema>() -> Schema {
    schemars::schema_for!(T)
}

use RecorderState as S;

const ACTIVE: &[RecorderState] = &[S::Recording, S::Paused];
const CANCELLABLE: &[RecorderState] = &[S::Preparing, S::Countdown, S::Recording, S::Paused];

macro_rules! tool {
    ($name:literal, $summary:literal, $p:ty => $r:ty, $effect:ident, $surf:ident, $states:expr, [$($cli:literal),*] $(, $flag:ident)*) => {{
        #[allow(unused_mut)]
        let mut t = ToolDef {
            name: $name,
            summary: $summary,
            params: s::<$p>,
            result: s::<$r>,
            effect: Effect::$effect,
            long_running: false,
            idempotent: matches!(Effect::$effect, Effect::ReadOnly),
            surfaces: Surfaces::$surf,
            states: $states,
            cli: &[$($cli),*],
        };
        $( tool!(@flag t, $flag); )*
        t
    }};
    (@flag $t:ident, long) => { $t.long_running = true; };
    (@flag $t:ident, idem) => { $t.idempotent = true; };
}

fn build() -> Vec<ToolDef> {
    vec![
        // session / events
        tool!("session.hello", "Identify the client; first call on every connection.", HelloParams => HelloResult, ReadOnly, INTERNAL, None, []),
        tool!("events.subscribe", "Receive events whose type starts with any of the given prefixes.", SubscribeParams => SubscribeResult, ReadOnly, HUMAN, None, ["events", "subscribe"]),
        // system
        tool!("system.info", "Engine version, platform, backend and file locations.", Empty => SystemInfo, ReadOnly, ALL, None, ["engine", "info"]),
        tool!("system.health", "Permissions, save folder, ffmpeg and leftover recordings, with fixes.", Empty => Health, ReadOnly, ALL, None, ["doctor"]),
        tool!("system.shutdown", "Stop the engine. Refused while recording.", Empty => Empty, Mutating, HUMAN, Some(&[S::Idle, S::Failed]), ["engine", "stop"]),
        // permissions / sources
        tool!("permissions.status", "Screen, camera, microphone and accessibility permission states.", Empty => PermissionStatus, ReadOnly, ALL, None, ["permissions", "status"]),
        tool!("sources.displays", "Displays that can be recorded.", Empty => Vec<Display>, ReadOnly, ALL, None, ["sources", "displays"]),
        tool!("sources.cameras", "Cameras that can be used for the face bubble.", Empty => Vec<Device>, ReadOnly, ALL, None, ["sources", "cameras"]),
        tool!("sources.microphones", "Microphones that can be recorded.", Empty => Vec<Device>, ReadOnly, ALL, None, ["sources", "mics"]),
        // recording
        tool!("recording.start", "Start recording. Returns once the countdown begins.", StartParams => RecorderStatus, Capture, ALL_OPT_IN, Some(&[S::Idle]), ["record", "start"]),
        tool!("recording.pause", "Pause; closes the current segment.", Empty => RecorderStatus, Capture, ALL, Some(&[S::Recording]), ["record", "pause"]),
        tool!("recording.resume", "Resume into a new segment.", Empty => RecorderStatus, Capture, ALL_OPT_IN, Some(&[S::Paused]), ["record", "resume"]),
        tool!("recording.stop", "Stop and save to the save folder. `wait` blocks until the file exists.", StopParams => StopResult, Capture, ALL, Some(ACTIVE), ["record", "stop"]),
        tool!("recording.cancel", "Stop and discard the recording (moved to the Trash).", ConfirmParams => CancelResult, Destructive, ALL, Some(CANCELLABLE), ["record", "cancel"]),
        tool!("recording.status", "Current recorder state, elapsed time and any jobs still saving.", Empty => RecorderStatus, ReadOnly, ALL, None, ["record", "status"]),
        tool!("recording.mark", "Add a marker at the current time.", MarkParams => MarkResult, Mutating, ALL, Some(ACTIVE), ["record", "mark"]),
        // settings
        tool!("settings.get", "Read all settings or one dotted path.", SettingsGetParams => Value, ReadOnly, ALL, None, ["settings", "get"]),
        tool!("settings.set", "Change settings by dotted path. Validated before saving.", SettingsSetParams => SettingsChanged, Mutating, ALL, None, ["settings", "set"], idem),
        tool!("settings.reset", "Reset all settings, or one dotted path, to defaults.", SettingsResetParams => SettingsChanged, Mutating, HUMAN, None, ["settings", "reset"], idem),
        tool!("settings.schema", "JSON Schema of settings.json.", Empty => Value, ReadOnly, ALL, None, ["settings", "schema"]),
        // library
        tool!("library.list", "List saved recordings (filter, sort, page).", LibraryListParams => LibraryListResult, ReadOnly, ALL, None, ["library", "ls"]),
        tool!("library.get", "One recording's details and manifest. Id, id prefix or `last`.", RecordingIdParams => RecordingDetails, ReadOnly, ALL, None, ["library", "show"]),
        tool!("library.update", "Rename (moves the files) or re-tag a recording.", LibraryUpdateParams => RecordingSummary, Mutating, ALL, None, ["library", "update"]),
        tool!("library.delete", "Move a recording to the Trash. Needs confirm: true.", LibraryDeleteParams => LibraryDeleteResult, Destructive, ALL_OPT_IN, None, ["library", "rm"]),
        tool!("library.reveal", "Show a recording's file in Finder.", LibraryRevealParams => PathResult, ReadOnly, HUMAN, None, ["library", "reveal"]),
        tool!("library.recover", "Save recordings interrupted by a crash from .lumos-inprogress.", RecoverParams => RecoverResult, Mutating, ALL, None, ["recover"], long),
        // export / import
        tool!("export.presets", "Export presets: MP4 sizes, GIF, audio only.", Empty => Vec<ExportPreset>, ReadOnly, ALL, None, ["export", "presets"]),
        tool!("export.create", "Export a recording with a preset (job). `wait` blocks until done.", ExportParams => ExportResult, Mutating, ALL, None, ["export", "create"], long),
        tool!("import.formats", "File types the importers accept.", Empty => Vec<ImportFormat>, ReadOnly, ALL, None, ["import", "formats"]),
        tool!("import.create", "Import video files into the library (job). `wait` blocks until done.", ImportParams => ImportResult, Mutating, ALL, None, ["import", "create"], long),
        // transcripts
        tool!("transcript.create", "Transcribe a recording locally with whisper.cpp (job). `wait` blocks until done.", TranscriptCreateParams => TranscriptCreateResult, Mutating, ALL, None, ["transcript", "create"], long),
        tool!("transcript.get", "Read a recording's transcript as txt, srt or vtt. How an agent reads what was said.", TranscriptGetParams => TranscriptGetResult, ReadOnly, ALL, None, ["transcript", "get"]),
        // jobs
        tool!("jobs.list", "Recent jobs, newest first.", JobListParams => Vec<Job>, ReadOnly, ALL, None, ["jobs", "ls"]),
        tool!("jobs.get", "Status, progress and result of one background job (save, export, import).", JobIdParams => Job, ReadOnly, ALL, None, ["jobs", "show"]),
        tool!("jobs.wait", "Wait for a job to finish (default 55 s). On timeout the job keeps running.", JobWaitParams => Job, ReadOnly, ALL, None, ["jobs", "wait"]),
    ]
}

/// All tools, in catalog order.
pub fn registry() -> &'static [ToolDef] {
    static REG: OnceLock<Vec<ToolDef>> = OnceLock::new();
    REG.get_or_init(build)
}

pub fn find(name: &str) -> Option<&'static ToolDef> {
    registry().iter().find(|t| t.name == name)
}

/// Settings schema, exported for `settings.schema` and `xtask schemas`.
pub fn settings_schema() -> Schema {
    s::<Settings>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names_and_cli_paths_are_unique() {
        let mut names = HashSet::new();
        let mut paths = HashSet::new();
        for t in registry() {
            assert!(names.insert(t.name), "duplicate tool {}", t.name);
            assert!(t.name.split_once('.').is_some(), "{} is not namespace.verb", t.name);
            if !t.cli.is_empty() {
                assert!(paths.insert(t.cli.join(" ")), "duplicate CLI path for {}", t.name);
            }
        }
    }

    #[test]
    fn every_schema_generates() {
        for t in registry() {
            let d = t.describe();
            assert!(d["params"].is_object() || d["params"].is_boolean(), "{}", t.name);
        }
    }

    #[test]
    fn capture_tools_that_start_capture_need_opt_in() {
        for name in ["recording.start", "recording.resume"] {
            assert!(find(name).unwrap().surfaces.agent_opt_in, "{name}");
        }
    }
}
