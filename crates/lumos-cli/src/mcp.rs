//! `lumos mcp`: a Model Context Protocol server over stdio.
//!
//! Every tool in the `lumos-tools` registry whose surfaces include agents is
//! listed, with its JSON Schemas. Calls are forwarded to the engine as an `mcp`
//! client, so the engine's agent policy (opt-in for recording, no automation
//! settings, minimum countdown) applies unchanged. See docs/05-cli-and-mcp.md.

use lumos_model::{ErrorCode, ToolResult};
use lumos_tools::{Effect, ToolDef};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Newest MCP revision we implement; older ones are echoed back when requested.
const LATEST: &str = "2025-06-18";
const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Lumos is a screen recorder. Call lumos_system_health first: it reports permissions and \
problems with fixes. Starting a recording needs the user to allow agents under Lumos → Settings → Automation; \
a visible indicator is always shown while recording and a countdown of at least 3 seconds is enforced. \
lumos_recording_stop with wait: true returns the saved file path. Long jobs (export, import, recover) return a jobId; \
pass wait: true or poll lumos_jobs_get.";

/// How the server reaches the engine. Tests plug in a fake.
pub trait EngineCall {
    async fn call(&mut self, tool: &str, params: Value) -> ToolResult<Value>;
}

/// Tools an agent may be offered.
fn listed() -> impl Iterator<Item = &'static ToolDef> {
    lumos_tools::registry().iter().filter(|t| t.surfaces.agent && t.name != "session.hello")
}

fn object_schema(schema: Value) -> Value {
    if schema.get("type").and_then(Value::as_str) == Some("object") { schema } else { json!({ "type": "object" }) }
}

fn tool_json(t: &ToolDef) -> Value {
    let d = t.describe();
    let mut description = t.summary.to_string();
    if t.surfaces.agent_opt_in {
        description.push_str(" Needs the user's permission under Lumos → Settings → Automation.");
    }
    if t.long_running {
        description.push_str(" Long-running: returns a jobId unless wait is true; poll lumos_jobs_get.");
    }
    let mut tool = json!({
        "name": t.mcp_name(),
        "title": t.name,
        "description": description,
        "inputSchema": object_schema(d["params"].clone()),
        "annotations": {
            "title": t.name,
            "readOnlyHint": t.effect == Effect::ReadOnly,
            "destructiveHint": t.effect == Effect::Destructive,
            "idempotentHint": t.idempotent,
            "openWorldHint": false,
        },
    });
    // structuredContent must be an object, so only object-shaped results declare a schema.
    if d["result"].get("type").and_then(Value::as_str) == Some("object") {
        tool["outputSchema"] = d["result"].clone();
    }
    tool
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Handle one JSON-RPC message. `None` for notifications and responses.
pub async fn handle<E: EngineCall>(engine: &mut E, msg: Value) -> Option<Value> {
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        // A response to something we never asked, or garbage with an id.
        return msg.get("id").filter(|_| msg.get("result").is_none() && msg.get("error").is_none()).map(|id| rpc_error(id.clone(), -32600, "invalid request"));
    };
    let id = msg.get("id").cloned()?; // notifications carry no id and get no reply
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(LATEST);
            let version = if SUPPORTED.contains(&asked) { asked } else { LATEST };
            rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "lumos", "title": "Lumos screen recorder", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": listed().map(tool_json).collect::<Vec<_>>() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(id, -32602, "tools/call needs a tool name"));
            };
            let Some(tool) = listed().find(|t| t.mcp_name() == name) else {
                return Some(rpc_error(id, -32602, format!("unknown tool '{name}'")));
            };
            let args = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(a @ Value::Object(_)) => a.clone(),
                Some(_) => return Some(rpc_error(id, -32602, "arguments must be an object")),
            };
            rpc_result(id, tool_result(engine.call(tool.name, args).await))
        }
        other => rpc_error(id, -32601, format!("method '{other}' is not supported")),
    })
}

/// Tool failures are results with `isError`, so the model can read and react to them.
fn tool_result(r: ToolResult<Value>) -> Value {
    match r {
        Ok(v) => {
            let text = serde_json::to_string(&v).unwrap_or_default();
            let mut out = json!({ "content": [{ "type": "text", "text": text }], "isError": false });
            if v.is_object() {
                out["structuredContent"] = v;
            }
            out
        }
        Err(e) => json!({
            "content": [{ "type": "text", "text": format!("{}: {}", e.code.as_str(), e.message) }],
            "structuredContent": { "error": e },
            "isError": true,
        }),
    }
}

/// Newline-delimited JSON-RPC over `input`/`output` until EOF.
pub async fn serve<E: EngineCall>(
    engine: &mut E,
    input: impl tokio::io::AsyncRead + Unpin,
    mut output: impl tokio::io::AsyncWrite + Unpin,
) -> std::io::Result<()> {
    let mut lines = BufReader::new(input).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Err(e) => Some(rpc_error(Value::Null, -32700, format!("parse error: {e}"))),
            Ok(Value::Array(batch)) => {
                let mut replies = Vec::new();
                for m in batch {
                    replies.extend(handle(engine, m).await);
                }
                (!replies.is_empty()).then(|| Value::Array(replies))
            }
            Ok(m) => handle(engine, m).await,
        };
        if let Some(r) = reply {
            let mut bytes = serde_json::to_vec(&r).unwrap_or_default();
            bytes.push(b'\n');
            output.write_all(&bytes).await?;
            output.flush().await?;
        }
    }
    Ok(())
}

/// Connection error codes that mean "try a fresh connection".
pub fn is_connection_lost(code: ErrorCode) -> bool {
    code == ErrorCode::EngineUnavailable
}

#[cfg(test)]
mod tests {
    use lumos_model::ToolError;

    use super::*;

    #[derive(Default)]
    struct Fake {
        calls: Vec<(String, Value)>,
        fail: Option<ToolError>,
    }

    impl EngineCall for Fake {
        async fn call(&mut self, tool: &str, params: Value) -> ToolResult<Value> {
            self.calls.push((tool.to_string(), params));
            match self.fail.take() {
                Some(e) => Err(e),
                None => Ok(json!({ "state": "idle" })),
            }
        }
    }

    async fn ask(e: &mut Fake, msg: Value) -> Value {
        handle(e, msg).await.expect("a reply")
    }

    #[tokio::test]
    async fn initialize_negotiates_version_and_advertises_tools() {
        let mut e = Fake::default();
        let r = ask(&mut e, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2024-11-05" } })).await;
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        let r = ask(&mut e, json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } })).await;
        assert_eq!(r["result"]["protocolVersion"], LATEST);
        assert!(handle(&mut e, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await.is_none());
        assert_eq!(ask(&mut e, json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" })).await["result"], json!({}));
    }

    #[tokio::test]
    async fn lists_exactly_the_agent_tools_with_schemas_and_annotations() {
        let mut e = Fake::default();
        let r = ask(&mut e, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await;
        let tools = r["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for t in lumos_tools::registry() {
            let expected = t.surfaces.agent && t.name != "session.hello";
            assert_eq!(names.contains(&t.mcp_name().as_str()), expected, "{}", t.name);
        }
        assert!(!names.contains(&"lumos_system_shutdown"), "human-only tools stay hidden");
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(t["description"].as_str().unwrap().len() > 10);
        }
        let by = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap();
        assert_eq!(by("lumos_recording_status")["annotations"]["readOnlyHint"], true);
        assert_eq!(by("lumos_library_delete")["annotations"]["destructiveHint"], true);
        assert!(by("lumos_recording_start")["description"].as_str().unwrap().contains("Automation"));
        assert!(by("lumos_recording_status")["outputSchema"].is_object());
        // Array-shaped results can't be structuredContent, so they declare no outputSchema.
        assert!(by("lumos_sources_displays").get("outputSchema").is_none());
    }

    #[tokio::test]
    async fn calls_forward_to_the_engine_and_return_structured_content() {
        let mut e = Fake::default();
        let r = ask(&mut e, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "lumos_recording_status" } })).await;
        assert_eq!(e.calls, vec![("recording.status".to_string(), json!({}))]);
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(r["result"]["structuredContent"]["state"], "idle");
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("idle"));

        let args = json!({ "countdownSeconds": 5 });
        ask(&mut e, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "lumos_recording_start", "arguments": args } })).await;
        assert_eq!(e.calls[1], ("recording.start".to_string(), args));
    }

    #[tokio::test]
    async fn engine_errors_become_readable_tool_errors() {
        let mut e = Fake { fail: Some(ToolError::policy("recording.start needs permission in Lumos → Settings → Automation")), ..Fake::default() };
        let r = ask(&mut e, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "lumos_recording_start" } })).await;
        assert!(r.get("error").is_none(), "tool failures are results, not protocol errors");
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().starts_with("POLICY_DENIED: "));
        assert_eq!(r["result"]["structuredContent"]["error"]["code"], "POLICY_DENIED");
    }

    #[tokio::test]
    async fn protocol_errors_use_json_rpc_codes() {
        let mut e = Fake::default();
        let code = |r: Value| r["error"]["code"].as_i64().unwrap();
        assert_eq!(code(ask(&mut e, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "lumos_nope" } })).await), -32602);
        // Hidden tools are not callable either.
        assert_eq!(code(ask(&mut e, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "lumos_system_shutdown" } })).await), -32602);
        assert_eq!(code(ask(&mut e, json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "lumos_recording_status", "arguments": [1] } })).await), -32602);
        assert_eq!(code(ask(&mut e, json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/list" })).await), -32601);
        assert!(e.calls.is_empty(), "nothing reached the engine");
    }

    #[tokio::test]
    async fn serve_speaks_newline_delimited_json() {
        let mut e = Fake::default();
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n",
            "\n",
            "not json\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "[{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}]\n",
        );
        let mut out = Vec::new();
        serve(&mut e, input.as_bytes(), &mut out).await.unwrap();
        let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[1]["error"]["code"], -32700);
        assert_eq!(lines[2][0]["id"], 7);
    }
}
