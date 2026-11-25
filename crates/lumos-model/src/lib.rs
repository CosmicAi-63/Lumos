//! Pure data types shared by every Lumos crate. No I/O lives here.
//!
//! JSON is camelCase. Wall-clock times are RFC 3339 UTC strings; media times are
//! `i64` microseconds on the recording timeline (fields end in `Us`).

pub mod api;
pub mod error;
pub mod events;
pub mod jobs;
pub mod manifest;
pub mod recorder;
pub mod settings;
pub mod sources;

pub use error::{ErrorCode, ToolError, ToolResult};

/// New prefixed ULID, e.g. `rec_01JDX8Q3M7K2…`.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", ulid::Ulid::generate())
}

/// Current time as RFC 3339 UTC with millisecond precision.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Protocol version spoken over JSON-RPC.
pub const PROTOCOL_VERSION: u32 = 1;
