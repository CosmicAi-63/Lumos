use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lumos_capture::CaptureBackend;
use lumos_model::api::*;
use lumos_model::events::{self, Event};
use lumos_model::jobs::JobKind;
use lumos_model::recorder::{ClientInfo, RecorderState};
use lumos_model::settings::Settings;
use lumos_model::sources::PermissionState;
use lumos_model::{ErrorCode, PROTOCOL_VERSION, ToolError, ToolResult};
use lumos_rpc::paths::Paths;
use lumos_rpc::{ConnCtx, Dispatch};
use lumos_tools::ToolDef;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{Notify, broadcast};

use crate::fsutil::{expand_tilde, free_mb, is_writable};
use crate::jobs::Jobs;
use crate::policy;
use crate::recorder::Recorder;
use crate::settings::SettingsStore;

/// Longest a single `jobs.wait` / `recording.stop --wait` may block.
const MAX_WAIT_MS: u64 = 10 * 60 * 1000;
pub(crate) const DEFAULT_WAIT_MS: u64 = 55_000;

pub struct EngineOptions {
    pub paths: Paths,
    /// `app` or `lumosd`; reported by `system.info`.
    pub host: String,
    /// Defaults to `lumos_capture::select_backend()`.
    pub backend: Option<Arc<dyn CaptureBackend>>,
}

/// The engine. Cheap to clone via `Arc`.
pub struct Engine {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub paths: Paths,
    pub host: String,
    pub backend: Arc<dyn CaptureBackend>,
    pub settings: std::sync::Mutex<SettingsStore>,
    pub jobs: Jobs,
    pub events: broadcast::Sender<Event>,
    pub recorder: tokio::sync::Mutex<Recorder>,
    shutdown: Notify,
    shutting_down: AtomicBool,
}

impl Inner {
    pub fn emit(&self, kind: &str, data: impl Serialize) {
        let _ = self.events.send(Event::new(kind, data));
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().get().clone()
    }
}

impl Engine {
    pub fn new(opts: EngineOptions) -> ToolResult<Arc<Self>> {
        let settings = SettingsStore::load(opts.paths.settings_file.clone())?;
        let (events, _) = broadcast::channel(256);
        let inner = Arc::new(Inner {
            backend: opts.backend.unwrap_or_else(lumos_capture::select_backend),
            settings: std::sync::Mutex::new(settings),
            jobs: Jobs::new(events.clone()),
            events,
            recorder: tokio::sync::Mutex::new(Recorder::default()),
            shutdown: Notify::new(),
            shutting_down: AtomicBool::new(false),
            paths: opts.paths,
            host: opts.host,
        });
        Ok(Arc::new(Self { inner }))
    }

    pub fn backend_name(&self) -> &'static str {
        self.inner.backend.name()
    }

    /// Resolves after `system.shutdown` or [`Engine::request_shutdown`].
    pub async fn wait_shutdown(&self) {
        if !self.inner.shutting_down.load(Ordering::SeqCst) {
            self.inner.shutdown.notified().await;
        }
    }

    /// Ask the engine to shut down (signal handlers use this).
    pub fn request_shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::SeqCst);
        self.inner.shutdown.notify_one();
    }

    /// Wait for recordings still being saved, up to `timeout`.
    pub async fn drain(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        for id in self.inner.jobs.active(JobKind::Finalize) {
            let left = deadline.saturating_duration_since(Instant::now());
            let _ = self.inner.jobs.wait(&id, left).await;
        }
    }

    /// Call a tool in process, as `client`. Same checks as over the socket.
    pub async fn call_as(&self, client: &ClientInfo, method: &str, params: Value) -> ToolResult<Value> {
        let tool = find_tool(method)?;
        self.inner.call(client, tool, params).await
    }
}

fn find_tool(method: &str) -> ToolResult<&'static ToolDef> {
    lumos_tools::find(method).ok_or_else(|| ToolError::new(ErrorCode::MethodNotFound, format!("no tool '{method}'")))
}

fn parse<T: DeserializeOwned>(v: Value) -> ToolResult<T> {
    let v = if v.is_null() { Value::Object(Default::default()) } else { v };
    serde_json::from_value(v).map_err(|e| ToolError::invalid_params(e.to_string()))
}

fn json(v: impl Serialize) -> ToolResult<Value> {
    serde_json::to_value(v).map_err(|e| ToolError::backend(format!("serialize result: {e}")))
}

fn state_name(s: RecorderState) -> String {
    serde_json::to_value(s).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

#[async_trait::async_trait]
impl Dispatch for Engine {
    async fn call(&self, ctx: Arc<ConnCtx>, method: &str, params: Value) -> ToolResult<Value> {
        let tool = find_tool(method)?;
        if tool.name == "session.hello" {
            let p: HelloParams = parse(params)?;
            if p.protocol != PROTOCOL_VERSION {
                return Err(ToolError::invalid_params(format!(
                    "protocol {} is not supported; this engine speaks {PROTOCOL_VERSION}",
                    p.protocol
                )));
            }
            *ctx.client.lock().unwrap() = Some(p.client);
            return json(HelloResult {
                engine_version: env!("CARGO_PKG_VERSION").into(),
                protocol: PROTOCOL_VERSION,
                capabilities: vec!["recording".into(), "settings".into(), "jobs".into(), "events".into()],
            });
        }
        let client = ctx.client().ok_or_else(|| ToolError::invalid_state("call session.hello first"))?;
        if tool.name == "events.subscribe" {
            policy::check(tool, &client, &self.inner.settings().automation, &params)?;
            let p: SubscribeParams = parse(params)?;
            *ctx.subscriptions.lock().unwrap() = p.topics.clone();
            return json(SubscribeResult { subscribed: p.topics });
        }
        self.inner.call(&client, tool, params).await
    }

    fn events(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }
}

impl Inner {
    async fn call(self: &Arc<Self>, client: &ClientInfo, tool: &'static ToolDef, params: Value) -> ToolResult<Value> {
        let settings = self.settings();
        policy::check(tool, client, &settings.automation, &params)?;
        if let Some(states) = tool.states {
            let state = self.recorder.lock().await.state;
            if !states.contains(&state) {
                return Err(ToolError::invalid_state(format!("{} is not available while {}", tool.name, state_name(state))));
            }
        }
        match tool.name {
            "system.info" => json(self.info()),
            "system.health" => json(self.health().await),
            "system.shutdown" => {
                self.shutting_down.store(true, Ordering::SeqCst);
                self.shutdown.notify_one();
                json(Empty {})
            }
            "permissions.status" => json(self.backend.permissions().await),
            "sources.displays" => json(self.backend.displays().await?),
            "sources.cameras" => json(self.backend.cameras().await?),
            "sources.microphones" => json(self.backend.microphones().await?),

            "recording.start" => {
                let p: StartParams = parse(params)?;
                let agent = policy::is_agent(client, &settings.automation);
                json(self.start(client.clone(), agent, p).await?)
            }
            "recording.pause" => json(self.pause().await?),
            "recording.resume" => json(self.resume().await?),
            "recording.stop" => {
                let p: StopParams = parse(params)?;
                let timeout = Duration::from_millis(p.timeout_ms.unwrap_or(DEFAULT_WAIT_MS).min(MAX_WAIT_MS));
                json(self.stop(p.wait, timeout).await?)
            }
            "recording.cancel" => {
                let p: ConfirmParams = parse(params)?;
                if !p.confirm {
                    return Err(ToolError::invalid_params("pass confirm: true to discard the recording"));
                }
                json(self.cancel().await?)
            }
            "recording.status" => {
                let rec = self.recorder.lock().await;
                json(self.status(&rec))
            }
            "recording.mark" => {
                let p: MarkParams = parse(params)?;
                json(self.mark(p.label).await?)
            }

            "settings.get" => {
                let p: SettingsGetParams = parse(params)?;
                self.settings.lock().unwrap().get_path(p.path.as_deref())
            }
            "settings.set" => {
                let p: SettingsSetParams = parse(params)?;
                let (changed, all) = {
                    let mut s = self.settings.lock().unwrap();
                    (s.set(&p.patch)?, s.as_value())
                };
                self.settings_changed(changed, all)
            }
            "settings.reset" => {
                let p: SettingsResetParams = parse(params)?;
                let (changed, all) = {
                    let mut s = self.settings.lock().unwrap();
                    (s.reset(p.path.as_deref())?, s.as_value())
                };
                self.settings_changed(changed, all)
            }
            "settings.schema" => json(lumos_tools::settings_schema()),

            "jobs.list" => {
                let p: JobListParams = parse(params)?;
                json(self.jobs.list(p.limit.unwrap_or(50)))
            }
            "jobs.get" => {
                let p: JobIdParams = parse(params)?;
                json(self.jobs.get(&p.job_id)?)
            }
            "jobs.wait" => {
                let p: JobWaitParams = parse(params)?;
                let timeout = Duration::from_millis(p.timeout_ms.unwrap_or(DEFAULT_WAIT_MS).min(MAX_WAIT_MS));
                json(self.jobs.wait(&p.job_id, timeout).await?)
            }
            other => Err(ToolError::new(ErrorCode::MethodNotFound, format!("{other} is in the catalog but not implemented yet"))),
        }
    }

    fn settings_changed(&self, changed: Vec<String>, settings: Value) -> ToolResult<Value> {
        let payload = SettingsChanged { changed, settings };
        if !payload.changed.is_empty() {
            self.emit(events::SETTINGS_CHANGED, &payload);
        }
        json(payload)
    }

    fn info(&self) -> SystemInfo {
        let p = &self.paths;
        SystemInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            engine_host: self.host.clone(),
            backend: self.backend.name().into(),
            home: p.home.display().to_string(),
            settings_path: p.settings_file.display().to_string(),
            socket_path: p.socket.display().to_string(),
            logs_path: p.logs_dir.display().to_string(),
        }
    }

    async fn health(&self) -> Health {
        let settings = self.settings();
        let permissions = self.backend.permissions().await;
        let folder = expand_tilde(&settings.storage.save_folder);
        let _ = std::fs::create_dir_all(&folder);
        let writable = is_writable(&folder);
        let free = free_mb(&folder);
        let ffmpeg = lumos_media::ffmpeg_version().await;
        let active_id = self.recorder.lock().await.active.as_ref().map(|a| a.id.clone());
        let in_progress = leftover_bundles(&folder, active_id.as_deref());

        let mut problems = Vec::new();
        let mut problem = |code: &str, message: String, fix: &str| problems.push(Problem { code: code.into(), message, fix: fix.into() });
        if !self.backend.can_record() {
            problem(
                "BACKEND_NOT_READY",
                format!("the {} capture backend can't record yet (M0 spike)", self.backend.name()),
                "Run the engine with LUMOS_BACKEND=fake to try the pipeline",
            );
        }
        if permissions.screen == PermissionState::Denied {
            problem("PERMISSION_DENIED", "Screen Recording permission is not granted".into(), "System Settings → Privacy & Security → Screen Recording");
        }
        if ffmpeg.is_none() {
            problem("FFMPEG_MISSING", "ffmpeg was not found".into(), "brew install ffmpeg, or set LUMOS_FFMPEG");
        }
        if !writable {
            problem("SAVE_FOLDER", format!("can't write to {}", folder.display()), "lumos settings set storage.saveFolder <folder>");
        }
        if free.is_some_and(|f| f < settings.recording.min_free_disk_mb) {
            problem("DISK_LOW", format!("{} MB free in the save folder", free.unwrap_or(0)), "free up space or change storage.saveFolder");
        }
        if !in_progress.is_empty() {
            problem(
                "RECOVERY_PENDING",
                format!("{} unfinished recording(s) in .lumos-inprogress", in_progress.len()),
                "automatic recovery arrives with library.recover (M2)",
            );
        }
        Health {
            backend: self.backend.name().into(),
            permissions,
            save_folder: SaveFolderHealth { path: folder.display().to_string(), writable, free_mb: free },
            ffmpeg,
            in_progress,
            problems,
        }
    }
}

/// `<save>/.lumos-inprogress/*.lumos`, except the recording running now.
fn leftover_bundles(save_folder: &Path, active: Option<&str>) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(save_folder.join(".lumos-inprogress")) else { return vec![] };
    let mut out: Vec<String> = dir
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "lumos"))
        .filter(|p| active.is_none_or(|id| p.file_stem().is_none_or(|s| s != id)))
        .map(|p| p.display().to_string())
        .collect();
    out.sort();
    out
}
