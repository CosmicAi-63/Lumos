use lumos_model::{ErrorCode, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Any line on the wire. Requests have `method` + `id`, notifications `method`
/// without `id`, responses `id` + `result`/`error`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    /// `{ "code": "<STABLE_STRING>", ... }`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Message {
    pub fn request(id: u64, method: &str, params: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id: Some(id.into()), method: Some(method.into()), params: Some(params), result: None, error: None }
    }
    pub fn notification(method: &str, params: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id: None, method: Some(method.into()), params: Some(params), result: None, error: None }
    }
    pub fn ok(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id: Some(id), method: None, params: None, result: Some(result), error: None }
    }
    pub fn err(id: Value, e: &ToolError) -> Self {
        Self { jsonrpc: "2.0".into(), id: Some(id), method: None, params: None, result: None, error: Some(RpcError::from(e)) }
    }
}

impl From<&ToolError> for RpcError {
    fn from(e: &ToolError) -> Self {
        let mut data = serde_json::json!({ "code": e.code.as_str() });
        if let Some(extra) = &e.data {
            data["detail"] = extra.clone();
        }
        Self { code: e.code.rpc_code(), message: e.message.clone(), data: Some(data) }
    }
}

impl From<RpcError> for ToolError {
    fn from(e: RpcError) -> Self {
        let code = e
            .data
            .as_ref()
            .and_then(|d| d.get("code"))
            .and_then(|c| serde_json::from_value::<ErrorCode>(c.clone()).ok())
            .unwrap_or(ErrorCode::BackendError);
        let detail = e.data.as_ref().and_then(|d| d.get("detail")).cloned();
        ToolError { code, message: e.message, data: detail }
    }
}

pub fn encode(m: &Message) -> String {
    let mut s = serde_json::to_string(m).expect("message serializes");
    s.push('\n');
    s
}
