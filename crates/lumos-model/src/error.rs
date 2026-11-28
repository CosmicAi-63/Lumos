use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable error codes. The string form is the contract; see docs/01-architecture.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidParams,
    InvalidState,
    PermissionDenied,
    NotFound,
    PolicyDenied,
    EngineUnavailable,
    DeviceBusy,
    DiskLow,
    BackendError,
    Cancelled,
    MethodNotFound,
    Timeout,
    /// A sharing account is not connected, or its sign-in expired. Reconnect it.
    AuthRequired,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidParams => "INVALID_PARAMS",
            Self::InvalidState => "INVALID_STATE",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::NotFound => "NOT_FOUND",
            Self::PolicyDenied => "POLICY_DENIED",
            Self::EngineUnavailable => "ENGINE_UNAVAILABLE",
            Self::DeviceBusy => "DEVICE_BUSY",
            Self::DiskLow => "DISK_LOW",
            Self::BackendError => "BACKEND_ERROR",
            Self::Cancelled => "CANCELLED",
            Self::MethodNotFound => "METHOD_NOT_FOUND",
            Self::Timeout => "TIMEOUT",
            Self::AuthRequired => "AUTH_REQUIRED",
        }
    }

    /// CLI process exit code.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::InvalidParams | Self::MethodNotFound => 2,
            Self::InvalidState => 3,
            Self::PermissionDenied => 4,
            Self::NotFound => 5,
            Self::PolicyDenied => 6,
            Self::EngineUnavailable => 7,
            Self::DeviceBusy => 8,
            Self::DiskLow => 9,
            Self::Cancelled => 130,
            Self::AuthRequired => 10,
            Self::BackendError | Self::Timeout => 1,
        }
    }

    /// JSON-RPC integer code. Standard codes where they exist, else the -32000 range.
    pub fn rpc_code(self) -> i64 {
        match self {
            Self::InvalidParams => -32602,
            Self::MethodNotFound => -32601,
            Self::InvalidState => -32001,
            Self::PermissionDenied => -32002,
            Self::NotFound => -32003,
            Self::PolicyDenied => -32004,
            Self::EngineUnavailable => -32005,
            Self::DeviceBusy => -32006,
            Self::DiskLow => -32007,
            Self::BackendError => -32008,
            Self::Cancelled => -32009,
            Self::Timeout => -32010,
            Self::AuthRequired => -32011,
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned by every tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ToolError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ToolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
    pub fn invalid_params(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidParams, m)
    }
    pub fn invalid_state(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidState, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, m)
    }
    pub fn backend(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::BackendError, m)
    }
    pub fn policy(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::PolicyDenied, m)
    }
    pub fn auth_required(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::AuthRequired, m)
    }
}

pub type ToolResult<T> = Result<T, ToolError>;
