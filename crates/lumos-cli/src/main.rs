//! `lumos`: command-line client for the Lumos engine.
//!
//! Every command is one engine tool from the `lumos-tools` registry; `lumos call`
//! reaches any tool by name. This crate never links the engine itself.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use lumos_model::api::{StartParams, Toggle};
use lumos_model::events::{RECORDER_STATE_CHANGED, RECORDER_TICK, RECORDING_FAILED, RECORDING_READY};
use lumos_model::recorder::{ClientInfo, ClientKind};
use lumos_model::sources::CaptureSource;
use lumos_model::{ErrorCode, PROTOCOL_VERSION, ToolError, ToolResult};
use lumos_rpc::Client;
use lumos_rpc::paths::Paths;
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "lumos", version, about = "Lumos screen recorder: command-line client for the Lumos engine")]
struct Cli {
    /// Print machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,
    /// Engine socket (default: $LUMOS_SOCKET, else <LUMOS_HOME>/engine.sock).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Don't start the engine if it isn't running.
    #[arg(long, global = true)]
    no_autostart: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start, pause, resume, stop and cancel recordings.
    #[command(subcommand)]
    Record(RecordCmd),
    /// Read and change settings.
    #[command(subcommand)]
    Settings(SettingsCmd),
    /// Displays, cameras and microphones.
    #[command(subcommand)]
    Sources(SourcesCmd),
    /// Screen, camera and microphone permissions.
    #[command(subcommand)]
    Permissions(PermissionsCmd),
    /// Check permissions, ffmpeg, the save folder and leftover recordings.
    Doctor,
    /// The engine process.
    #[command(subcommand)]
    Engine(EngineCmd),
    /// Background jobs (saving, exporting).
    #[command(subcommand)]
    Jobs(JobsCmd),
    /// Stream engine events as JSON lines.
    #[command(subcommand)]
    Events(EventsCmd),
    /// List every engine tool.
    Tools,
    /// Call any engine tool by name with JSON params.
    Call {
        /// Tool name, e.g. `recording.status`.
        tool: String,
        /// JSON object of params.
        #[arg(long, default_value = "{}")]
        params: String,
    },
}

#[derive(Subcommand)]
enum RecordCmd {
    /// Start recording.
    Start(StartArgs),
    /// Pause the recording.
    Pause,
    /// Resume a paused recording.
    Resume,
    /// Stop and save. Waits for the file unless --no-wait.
    Stop {
        /// Return as soon as saving starts.
        #[arg(long)]
        no_wait: bool,
        /// How long to wait for the file, in ms.
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
    /// Stop and discard the recording (moved to the Trash).
    Cancel {
        /// Don't ask for confirmation.
        #[arg(long, short)]
        yes: bool,
    },
    /// Show the recorder state.
    Status,
    /// Add a marker at the current time.
    Mark {
        /// Optional label.
        label: Option<String>,
    },
}

#[derive(Args)]
struct StartArgs {
    /// Title used in the file name.
    #[arg(long)]
    title: Option<String>,
    /// Display id from `lumos sources displays` (default: Settings).
    #[arg(long)]
    display: Option<String>,
    /// Countdown in seconds; 0 starts at once.
    #[arg(long)]
    countdown: Option<u32>,
    /// Record without the camera.
    #[arg(long, conflicts_with = "camera")]
    no_camera: bool,
    /// Camera device id.
    #[arg(long)]
    camera: Option<String>,
    /// Record without the microphone.
    #[arg(long, conflicts_with = "mic")]
    no_mic: bool,
    /// Microphone device id.
    #[arg(long)]
    mic: Option<String>,
    /// Also record system audio.
    #[arg(long)]
    system_audio: bool,
    /// Stop automatically after this many seconds.
    #[arg(long)]
    max_duration: Option<u64>,
    /// Save to this folder instead of the one in Settings.
    #[arg(long)]
    save_folder: Option<PathBuf>,
    /// Stay attached until the recording is saved; Ctrl-C stops and saves.
    #[arg(long)]
    wait: bool,
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// Print all settings, or one dotted path.
    Get { path: Option<String> },
    /// Set one dotted path. The value is parsed as JSON, else used as text.
    Set { path: String, value: String },
    /// Reset one dotted path or section to its default.
    Reset { path: Option<String> },
    /// Print the JSON Schema of settings.json.
    Schema,
}

#[derive(Subcommand)]
enum SourcesCmd {
    /// Displays that can be recorded.
    Displays,
    /// Cameras.
    Cameras,
    /// Microphones.
    Mics,
}

#[derive(Subcommand)]
enum PermissionsCmd {
    /// Permission states.
    Status,
}

#[derive(Subcommand)]
enum EngineCmd {
    /// Version, backend and file locations.
    Info,
    /// Stop the engine (refused while recording).
    Stop,
}

#[derive(Subcommand)]
enum JobsCmd {
    /// Recent jobs.
    Ls {
        #[arg(long)]
        limit: Option<usize>,
    },
    /// One job.
    Show { job_id: String },
    /// Wait for a job to finish.
    Wait {
        job_id: String,
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
}

#[derive(Subcommand)]
enum EventsCmd {
    /// Print events whose type starts with any prefix (default: all).
    Subscribe { topics: Vec<String> },
}

/// What a command does.
enum Plan {
    /// Call one tool.
    Tool(&'static str, Value),
    /// `recording.start`, then follow events until the file is saved.
    StartAndWait(Value),
    /// `recording.cancel` after confirmation.
    Cancel { yes: bool },
    /// `system.shutdown`, without starting an engine just to stop it.
    StopEngine,
    Events(Vec<String>),
    Tools,
    Call(String, Value),
}

fn plan(cmd: Cmd) -> ToolResult<Plan> {
    use Plan::Tool;
    let e = || json!({});
    Ok(match cmd {
        Cmd::Record(r) => match r {
            RecordCmd::Start(a) => {
                let wait = a.wait;
                let params = serde_json::to_value(start_params(a)).map_err(|e| ToolError::invalid_params(e.to_string()))?;
                if wait { Plan::StartAndWait(params) } else { Tool("recording.start", params) }
            }
            RecordCmd::Pause => Tool("recording.pause", e()),
            RecordCmd::Resume => Tool("recording.resume", e()),
            RecordCmd::Stop { no_wait, timeout_ms } => {
                let mut p = json!({ "wait": !no_wait });
                if let Some(t) = timeout_ms {
                    p["timeoutMs"] = t.into();
                }
                Tool("recording.stop", p)
            }
            RecordCmd::Cancel { yes } => Plan::Cancel { yes },
            RecordCmd::Status => Tool("recording.status", e()),
            RecordCmd::Mark { label } => Tool("recording.mark", json!({ "label": label })),
        },
        Cmd::Settings(s) => match s {
            SettingsCmd::Get { path } => Tool("settings.get", json!({ "path": path })),
            SettingsCmd::Set { path, value } => {
                let v = serde_json::from_str(&value).unwrap_or(Value::String(value));
                Tool("settings.set", json!({ "patch": { path: v } }))
            }
            SettingsCmd::Reset { path } => Tool("settings.reset", json!({ "path": path })),
            SettingsCmd::Schema => Tool("settings.schema", e()),
        },
        Cmd::Sources(s) => match s {
            SourcesCmd::Displays => Tool("sources.displays", e()),
            SourcesCmd::Cameras => Tool("sources.cameras", e()),
            SourcesCmd::Mics => Tool("sources.microphones", e()),
        },
        Cmd::Permissions(PermissionsCmd::Status) => Tool("permissions.status", e()),
        Cmd::Doctor => Tool("system.health", e()),
        Cmd::Engine(EngineCmd::Info) => Tool("system.info", e()),
        Cmd::Engine(EngineCmd::Stop) => Plan::StopEngine,
        Cmd::Jobs(j) => match j {
            JobsCmd::Ls { limit } => Tool("jobs.list", json!({ "limit": limit })),
            JobsCmd::Show { job_id } => Tool("jobs.get", json!({ "jobId": job_id })),
            JobsCmd::Wait { job_id, timeout_ms } => Tool("jobs.wait", json!({ "jobId": job_id, "timeoutMs": timeout_ms })),
        },
        Cmd::Events(EventsCmd::Subscribe { topics }) => Plan::Events(if topics.is_empty() { vec!["*".into()] } else { topics }),
        Cmd::Tools => Plan::Tools,
        Cmd::Call { tool, params } => {
            let p = serde_json::from_str(&params).map_err(|e| ToolError::invalid_params(format!("--params is not JSON: {e}")))?;
            Plan::Call(tool, p)
        }
    })
}

fn start_params(a: StartArgs) -> StartParams {
    let toggle = |off: bool, device: Option<String>| match (off, device) {
        (true, _) => Some(Toggle { enabled: Some(false), device_id: None }),
        (false, Some(d)) => Some(Toggle { enabled: Some(true), device_id: Some(d) }),
        (false, None) => None,
    };
    StartParams {
        source: a.display.map(|display_id| CaptureSource::Display { display_id }),
        camera: toggle(a.no_camera, a.camera),
        mic: toggle(a.no_mic, a.mic),
        system_audio: a.system_audio.then_some(true),
        countdown_seconds: a.countdown,
        title: a.title,
        max_duration_seconds: a.max_duration,
        save_folder: a.save_folder.map(|p| std::path::absolute(&p).unwrap_or(p).display().to_string()),
    }
}

/// Drop `null` params: the engine treats a missing field as "use the default".
fn strip_nulls(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(m.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k, strip_nulls(v))).collect()),
        other => other,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let json_out = cli.json;
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if json_out {
                let body = json!({ "error": { "code": e.code.as_str(), "message": e.message, "data": e.data } });
                eprintln!("{body}");
            } else {
                eprintln!("lumos: {}: {}", e.code, e.message);
            }
            ExitCode::from(e.code.exit_code() as u8)
        }
    }
}

async fn run(cli: Cli) -> ToolResult<()> {
    let mut paths = Paths::from_env();
    let custom_socket = cli.socket.is_some();
    if let Some(s) = cli.socket {
        paths.socket = s;
    }
    let autostart = !cli.no_autostart && !custom_socket;
    let out = Out { json: cli.json };

    match plan(cli.cmd)? {
        Plan::Tools => {
            print_tools(&out);
            Ok(())
        }
        Plan::StopEngine => match connect(&paths, false).await {
            Ok(c) => {
                let v = c.call("system.shutdown", json!({})).await?;
                out.print("system.shutdown", &v);
                Ok(())
            }
            Err(e) if e.code == ErrorCode::EngineUnavailable => {
                out.note("the engine is not running");
                Ok(())
            }
            Err(e) => Err(e),
        },
        Plan::Tool(name, params) => {
            let c = connect(&paths, autostart).await?;
            let v = c.call(name, strip_nulls(params)).await?;
            out.print(name, &v);
            Ok(())
        }
        Plan::Call(name, params) => {
            let c = connect(&paths, autostart).await?;
            let v = c.call(&name, params).await?;
            out.print(&name, &v);
            Ok(())
        }
        Plan::Cancel { yes } => {
            if !yes && !confirm("Discard this recording? It will be moved to the Trash. [y/N] ")? {
                return Err(ToolError::new(ErrorCode::Cancelled, "not discarded"));
            }
            let c = connect(&paths, autostart).await?;
            let v = c.call("recording.cancel", json!({ "confirm": true })).await?;
            out.print("recording.cancel", &v);
            Ok(())
        }
        Plan::Events(topics) => {
            let c = connect(&paths, autostart).await?;
            c.call("events.subscribe", json!({ "topics": topics })).await?;
            let mut rx = c.take_events().expect("fresh client");
            loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Some(ev) => println!("{}", serde_json::to_string(&ev).unwrap_or_default()),
                        None => return Err(ToolError::new(ErrorCode::EngineUnavailable, "engine closed the connection")),
                    },
                    _ = tokio::signal::ctrl_c() => return Ok(()),
                }
            }
        }
        Plan::StartAndWait(params) => start_and_wait(&paths, autostart, &out, strip_nulls(params)).await,
    }
}

async fn start_and_wait(paths: &Paths, autostart: bool, out: &Out, params: Value) -> ToolResult<()> {
    let c = connect(paths, autostart).await?;
    // Subscribe first so no event is missed.
    c.call("events.subscribe", json!({ "topics": ["recorder.", "recording."] })).await?;
    let mut rx = c.take_events().expect("fresh client");
    let status = c.call("recording.start", params).await?;
    let id = status["recordingId"].as_str().unwrap_or_default().to_string();
    out.note(&format!("{} {id}", describe_state(&status)));
    let mut stopping = false;
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let Some(ev) = ev else {
                    return Err(ToolError::new(ErrorCode::EngineUnavailable, "engine closed the connection"));
                };
                // State-change events carry the id as `recordingId` too.
                if ev.data["recordingId"].as_str() != Some(id.as_str()) {
                    continue;
                }
                match ev.kind.as_str() {
                    RECORDER_TICK => out.progress(&format!("Recording {}", clock(ev.data["elapsedUs"].as_i64().unwrap_or(0)))),
                    RECORDER_STATE_CHANGED if ev.data["state"] == "paused" => out.note("Paused"),
                    RECORDING_READY => {
                        out.print("recording.ready", &ev.data);
                        return Ok(());
                    }
                    RECORDING_FAILED => {
                        let err: ToolError = serde_json::from_value(ev.data["error"].clone())
                            .unwrap_or_else(|_| ToolError::backend("recording failed"));
                        return Err(err);
                    }
                    _ => {}
                }
            }
            _ = tokio::signal::ctrl_c(), if !stopping => {
                stopping = true;
                out.note("Stopping…");
                // Saving continues in the engine; keep waiting for `recording.ready`.
                if let Err(e) = c.call("recording.stop", json!({})).await {
                    if e.code != ErrorCode::InvalidState {
                        return Err(e);
                    }
                }
            }
        }
    }
}

fn confirm(prompt: &str) -> ToolResult<bool> {
    if !std::io::stdin().is_terminal() {
        return Err(ToolError::invalid_params("pass --yes to discard without a prompt"));
    }
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| ToolError::backend(e.to_string()))?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

// ---- engine connection -------------------------------------------------------

async fn connect(paths: &Paths, autostart: bool) -> ToolResult<Client> {
    let client = match Client::connect(&paths.socket).await {
        Ok(c) => c,
        Err(e) if e.code == ErrorCode::EngineUnavailable && autostart => {
            spawn_engine(paths)?;
            wait_for_engine(paths).await?
        }
        Err(e) => return Err(e),
    };
    let hello = json!({
        "client": ClientInfo { kind: ClientKind::Cli, name: "lumos".into(), version: env!("CARGO_PKG_VERSION").into() },
        "protocol": PROTOCOL_VERSION,
    });
    client.call("session.hello", hello).await?;
    Ok(client)
}

/// `$LUMOS_DAEMON`, else `lumosd` next to this binary.
fn daemon_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LUMOS_DAEMON") {
        return p.into();
    }
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("lumosd"))).unwrap_or_else(|| "lumosd".into())
}

/// Start `lumosd` detached: own process group, no inherited stdio, log to a file.
fn spawn_engine(paths: &Paths) -> ToolResult<()> {
    use std::os::unix::process::CommandExt;
    std::fs::create_dir_all(&paths.logs_dir).map_err(|e| ToolError::backend(format!("create {}: {e}", paths.logs_dir.display())))?;
    let log_path = paths.logs_dir.join("lumosd.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| ToolError::backend(format!("open {}: {e}", log_path.display())))?;
    let err = log.try_clone().map_err(|e| ToolError::backend(e.to_string()))?;
    let daemon = daemon_path();
    std::process::Command::new(&daemon)
        .env("LUMOS_SOCKET", &paths.socket)
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(err)
        .process_group(0)
        .spawn()
        .map_err(|e| ToolError::new(ErrorCode::EngineUnavailable, format!("cannot start {}: {e}", daemon.display())))?;
    Ok(())
}

async fn wait_for_engine(paths: &Paths) -> ToolResult<Client> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match Client::connect(&paths.socket).await {
            Ok(c) => return Ok(c),
            Err(e) if std::time::Instant::now() >= deadline => {
                return Err(ToolError::new(
                    ErrorCode::EngineUnavailable,
                    format!("{} (see {})", e.message, paths.logs_dir.join("lumosd.log").display()),
                ));
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

// ---- output ------------------------------------------------------------------

struct Out {
    json: bool,
}

impl Out {
    fn print(&self, tool: &str, v: &Value) {
        if self.json {
            println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
            return;
        }
        match tool {
            "recording.start" | "recording.pause" | "recording.resume" | "recording.status" => {
                let id = v["recordingId"].as_str().map(|s| format!(" {s}")).unwrap_or_default();
                println!("{}{id}", describe_state(v));
                for j in v["pendingFinalize"].as_array().into_iter().flatten() {
                    println!("  saving an earlier recording: {}", j.as_str().unwrap_or_default());
                }
                if let Some(err) = v["lastError"].as_object() {
                    println!("  last error: {}", err.get("message").and_then(Value::as_str).unwrap_or_default());
                }
            }
            "recording.stop" => match v["outputPath"].as_str() {
                Some(p) => println!("Saved {p}"),
                None => {
                    let job = v["finalizeJobId"].as_str().unwrap_or_default();
                    println!("Saving… (job {job}; run `lumos jobs wait {job}`)");
                }
            },
            "recording.ready" => println!("Saved {}", v["outputPath"].as_str().unwrap_or_default()),
            "recording.cancel" => println!("Discarded {}", v["recordingId"].as_str().unwrap_or_default()),
            "system.health" => {
                let problems = v["problems"].as_array().cloned().unwrap_or_default();
                println!("backend   {}", v["backend"].as_str().unwrap_or_default());
                println!("ffmpeg    {}", v["ffmpeg"].as_str().unwrap_or("not found"));
                println!("save to   {}", v["saveFolder"]["path"].as_str().unwrap_or_default());
                if problems.is_empty() {
                    println!("No problems found.");
                }
                for p in problems {
                    println!(
                        "✗ {}: {}\n    fix: {}",
                        p["code"].as_str().unwrap_or_default(),
                        p["message"].as_str().unwrap_or_default(),
                        p["fix"].as_str().unwrap_or_default()
                    );
                }
            }
            "settings.set" | "settings.reset" => {
                let changed: Vec<&str> = v["changed"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                if changed.is_empty() {
                    println!("Nothing changed.");
                }
                for c in changed {
                    let now = v["settings"].pointer(&format!("/{}", c.replace('.', "/"))).cloned().unwrap_or_default();
                    println!("{c} = {now}");
                }
            }
            "system.shutdown" => println!("Engine stopped."),
            _ => match v {
                Value::String(s) => println!("{s}"),
                other => println!("{}", serde_json::to_string_pretty(other).unwrap_or_default()),
            },
        }
    }

    fn note(&self, msg: &str) {
        if !self.json {
            eprintln!("{msg}");
        }
    }

    /// Single updating status line on a terminal.
    fn progress(&self, msg: &str) {
        if !self.json && std::io::stderr().is_terminal() {
            eprint!("\r{msg}   ");
            let _ = std::io::stderr().flush();
        }
    }
}

fn describe_state(v: &Value) -> String {
    let state = v["state"].as_str().unwrap_or("unknown");
    let mut s = format!("{}{}", state[..1].to_uppercase(), &state[1..]);
    if matches!(state, "recording" | "paused") {
        s.push_str(&format!(" {}", clock(v["elapsedUs"].as_i64().unwrap_or(0))));
    }
    s
}

fn clock(us: i64) -> String {
    let secs = us / 1_000_000;
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn print_tools(out: &Out) {
    let reg = lumos_tools::registry();
    if out.json {
        let all: Vec<Value> = reg.iter().map(|t| t.describe()).collect();
        println!("{}", serde_json::to_string_pretty(&all).unwrap_or_default());
        return;
    }
    for t in reg {
        let cli = if t.cli.is_empty() { "(lumos call)".to_string() } else { format!("lumos {}", t.cli.join(" ")) };
        println!("{:<22} {:<28} {}", t.name, cli, t.summary);
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    /// Required positional args of a leaf command, filled with placeholders.
    fn placeholders(cmd: &clap::Command) -> Vec<String> {
        cmd.get_positionals().filter(|a| a.is_required_set()).map(|_| "1".to_string()).collect()
    }

    #[test]
    fn every_tool_with_a_cli_path_runs_that_tool() {
        let root = Cli::command();
        for t in lumos_tools::registry().iter().filter(|t| !t.cli.is_empty()) {
            let mut cmd = &root;
            for part in t.cli {
                cmd = cmd.find_subcommand(part).unwrap_or_else(|| panic!("`lumos {}` is missing for {}", t.cli.join(" "), t.name));
            }
            let mut argv = vec!["lumos".to_string()];
            argv.extend(t.cli.iter().map(|s| s.to_string()));
            argv.extend(placeholders(cmd));
            if t.name == "recording.cancel" {
                argv.push("--yes".into());
            }
            let parsed = Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
            let name = match plan(parsed.cmd).unwrap() {
                Plan::Tool(n, _) => n,
                Plan::Cancel { .. } => "recording.cancel",
                Plan::StopEngine => "system.shutdown",
                Plan::Events(_) => "events.subscribe",
                Plan::StartAndWait(_) => "recording.start",
                Plan::Tools | Plan::Call(..) => panic!("{argv:?} is not a tool command"),
            };
            assert_eq!(name, t.name, "`{}` runs the wrong tool", argv.join(" "));
        }
    }

    #[test]
    fn every_leaf_command_is_a_registry_tool() {
        fn leaves(cmd: &clap::Command, path: Vec<String>, out: &mut Vec<Vec<String>>) {
            let subs: Vec<_> = cmd.get_subcommands().filter(|c| c.get_name() != "help").collect();
            if subs.is_empty() {
                out.push(path.clone());
            }
            for s in subs {
                let mut p = path.clone();
                p.push(s.get_name().to_string());
                leaves(s, p, out);
            }
        }
        let mut all = Vec::new();
        leaves(&Cli::command(), vec![], &mut all);
        for path in all {
            if path == ["tools"] || path == ["call"] {
                continue;
            }
            let found = lumos_tools::registry().iter().any(|t| t.cli.iter().copied().eq(path.iter().map(String::as_str)));
            assert!(found, "`lumos {}` has no tool in the registry", path.join(" "));
        }
    }

    #[test]
    fn start_flags_map_to_params() {
        let cli = Cli::try_parse_from(["lumos", "record", "start", "--no-camera", "--mic", "m1", "--countdown", "0", "--max-duration", "5"]).unwrap();
        let Plan::Tool(_, p) = plan(cli.cmd).unwrap() else { panic!() };
        let p = strip_nulls(p);
        assert_eq!(p["camera"], json!({ "enabled": false }));
        assert_eq!(p["mic"], json!({ "enabled": true, "deviceId": "m1" }));
        assert_eq!(p["countdownSeconds"], 0);
        assert_eq!(p["maxDurationSeconds"], 5);
    }
}
