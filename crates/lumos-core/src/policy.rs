//! Who may call what. See docs/05-cli-and-mcp.md (agent safety).

use lumos_model::recorder::{ClientInfo, ClientKind};
use lumos_model::settings::Automation;
use lumos_model::{ToolError, ToolResult};
use lumos_tools::ToolDef;
use serde_json::Value;

/// Agents get at least this countdown so a person can see a recording begin.
pub const AGENT_MIN_COUNTDOWN: u32 = 3;

/// True if the caller is treated as an agent: MCP, or the CLI when not trusted.
pub fn is_agent(client: &ClientInfo, automation: &Automation) -> bool {
    match client.kind {
        ClientKind::Mcp => true,
        ClientKind::Cli => !automation.cli_is_trusted,
        ClientKind::Ui => false,
    }
}

pub fn check(tool: &ToolDef, client: &ClientInfo, automation: &Automation, params: &Value) -> ToolResult<()> {
    let agent = is_agent(client, automation);
    let allowed = match client.kind {
        ClientKind::Ui => tool.surfaces.ui,
        ClientKind::Cli => tool.surfaces.cli && (!agent || tool.surfaces.agent),
        ClientKind::Mcp => tool.surfaces.agent,
    };
    if !allowed {
        return Err(ToolError::policy(format!("{} is not available to {:?} clients", tool.name, client.kind)));
    }
    if agent && tool.surfaces.agent_opt_in {
        let ok = match tool.name {
            "recording.start" | "recording.resume" => automation.agent_may_start_recording,
            "library.delete" => automation.agent_may_delete,
            "system.snapshot" => automation.agent_may_snapshot_screen,
            "share.upload" => automation.agent_may_share,
            _ => false,
        };
        if !ok {
            return Err(ToolError::policy(format!("{} needs permission in Lumos → Settings → Automation", tool.name)));
        }
    }
    // Only the desktop UI may change what agents are allowed to do.
    let touches_automation = match tool.name {
        "settings.set" => params
            .get("patch")
            .and_then(Value::as_object)
            .is_some_and(|p| p.keys().any(|k| k == "automation" || k.starts_with("automation."))),
        "settings.reset" => params.get("path").and_then(Value::as_str).is_none_or(|p| p == "automation" || p.starts_with("automation.")),
        _ => false,
    };
    if touches_automation && client.kind != ClientKind::Ui {
        return Err(ToolError::policy("automation settings can only be changed in the Lumos app"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client(kind: ClientKind) -> ClientInfo {
        ClientInfo { kind, name: "t".into(), version: "0".into() }
    }

    #[test]
    fn agents_need_opt_in_to_start() {
        let start = lumos_tools::find("recording.start").unwrap();
        let mut a = Automation::default();
        assert!(check(start, &client(ClientKind::Mcp), &a, &json!({})).is_err());
        assert!(check(start, &client(ClientKind::Cli), &a, &json!({})).is_ok());
        a.cli_is_trusted = false;
        assert!(check(start, &client(ClientKind::Cli), &a, &json!({})).is_err());
        a.agent_may_start_recording = true;
        assert!(check(start, &client(ClientKind::Mcp), &a, &json!({})).is_ok());
    }

    #[test]
    fn agents_need_opt_in_to_share_and_never_connect() {
        let upload = lumos_tools::find("share.upload").unwrap();
        let mut a = Automation::default();
        assert!(check(upload, &client(ClientKind::Mcp), &a, &json!({})).is_err());
        assert!(check(upload, &client(ClientKind::Ui), &a, &json!({})).is_ok());
        a.agent_may_share = true;
        assert!(check(upload, &client(ClientKind::Mcp), &a, &json!({})).is_ok());
        let connect = lumos_tools::find("share.connect").unwrap();
        assert!(check(connect, &client(ClientKind::Mcp), &a, &json!({})).is_err());
    }

    #[test]
    fn human_only_tools_and_automation_settings() {
        let a = Automation::default();
        let shutdown = lumos_tools::find("system.shutdown").unwrap();
        assert!(check(shutdown, &client(ClientKind::Mcp), &a, &json!({})).is_err());
        let set = lumos_tools::find("settings.set").unwrap();
        let p = json!({ "patch": { "automation.agentMayDelete": true } });
        assert!(check(set, &client(ClientKind::Cli), &a, &p).is_err());
        assert!(check(set, &client(ClientKind::Ui), &a, &p).is_ok());
        let reset = lumos_tools::find("settings.reset").unwrap();
        assert!(check(reset, &client(ClientKind::Cli), &a, &json!({})).is_err());
        assert!(check(reset, &client(ClientKind::Cli), &a, &json!({ "path": "capture" })).is_ok());
    }
}
