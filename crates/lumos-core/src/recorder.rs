//! The recorder state machine. See docs/02-data-model.md (recorder states) and
//! docs/03-recording-pipeline.md.
//!
//! One `tokio::Mutex<Recorder>` serialises every transition. Finalize runs as a
//! job after the recorder is already back to `idle`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lumos_capture::{CaptureSession, SessionConfig};
use lumos_model::api::{CancelResult, MarkResult, RecordingReady, StartParams, StopResult};
use lumos_model::events;
use lumos_model::jobs::{JobKind, JobStatus};
use lumos_model::manifest::*;
use lumos_model::recorder::{ActiveSource, ClientInfo, RecorderState, RecorderStats, RecorderStatus};
use lumos_model::sources::{CaptureSource, PermissionState};
use lumos_model::{ErrorCode, ToolError, ToolResult, new_id, now_rfc3339};
use serde_json::json;
use tokio::task::JoinHandle;

use crate::engine::Inner;
use crate::finalize::{self, FinalizeInput};
use crate::fsutil::{expand_tilde, free_mb, io_err, is_writable, write_json_atomic};
use crate::policy::AGENT_MIN_COUNTDOWN;

const TICK: Duration = Duration::from_millis(100);
/// Check free disk space every this many ticks.
const DISK_CHECK_EVERY: u64 = 20;

pub(crate) struct Recorder {
    pub state: RecorderState,
    pub active: Option<Active>,
    pub last_error: Option<ToolError>,
}

impl Default for Recorder {
    fn default() -> Self {
        Self { state: RecorderState::Idle, active: None, last_error: None }
    }
}

pub(crate) struct Active {
    pub id: String,
    pub title: String,
    pub created_local: chrono::DateTime<chrono::Local>,
    pub save_folder: PathBuf,
    pub bundle_dir: PathBuf,
    pub session: Option<Box<dyn CaptureSession>>,
    pub manifest: Manifest,
    /// Index of the open segment, or of the next one while paused.
    pub segment_index: u32,
    /// Recorded time in closed segments.
    pub closed_us: i64,
    /// Open segment: monotonic start and wall-clock start.
    pub open_segment: Option<(Instant, String)>,
    pub source: ActiveSource,
    pub camera_enabled: bool,
    pub mic_enabled: bool,
    pub system_audio: bool,
    pub started_by: ClientInfo,
    pub max_duration_us: Option<i64>,
    pub bytes: u64,
    pub countdown: Option<JoinHandle<()>>,
}

impl Active {
    pub fn elapsed_us(&self) -> i64 {
        self.closed_us + self.open_segment.as_ref().map(|(t, _)| t.elapsed().as_micros() as i64).unwrap_or(0)
    }

    fn write_manifest(&self) -> ToolResult<()> {
        write_json_atomic(&self.bundle_dir.join("manifest.json"), &self.manifest)
    }
}

impl Inner {
    pub(crate) fn status(&self, rec: &Recorder) -> RecorderStatus {
        let mut s = RecorderStatus::idle();
        s.state = rec.state;
        s.pending_finalize = self.jobs.active(JobKind::Finalize);
        s.last_error = rec.last_error.clone();
        if let Some(a) = &rec.active {
            s.recording_id = Some(a.id.clone());
            s.elapsed_us = a.elapsed_us();
            s.segment_index = a.segment_index;
            s.source = Some(a.source.clone());
            s.camera_enabled = a.camera_enabled;
            s.mic_enabled = a.mic_enabled;
            s.system_audio = a.system_audio;
            s.started_by = Some(a.started_by.clone());
            s.stats = RecorderStats { bytes_written: a.bytes, dropped_frames: 0 };
        }
        s
    }

    fn set_state(&self, rec: &mut Recorder, state: RecorderState) {
        rec.state = state;
        self.emit(events::RECORDER_STATE_CHANGED, self.status(rec));
    }

    async fn begin_segment(&self, a: &mut Active) -> ToolResult<()> {
        let session = a.session.as_mut().ok_or_else(|| ToolError::invalid_state("capture session is closed"))?;
        session.begin_segment(a.segment_index).await?;
        a.open_segment = Some((Instant::now(), now_rfc3339()));
        Ok(())
    }

    /// Close the open segment and record it in the manifest.
    async fn end_segment(&self, a: &mut Active) -> ToolResult<()> {
        let (_, wall_start) = a.open_segment.take().ok_or_else(|| ToolError::invalid_state("no open segment"))?;
        let session = a.session.as_mut().ok_or_else(|| ToolError::invalid_state("capture session is closed"))?;
        let out = session.end_segment().await?;
        a.manifest.segments.push(Segment {
            id: format!("seg_{:03}", a.segment_index),
            start_us: a.closed_us,
            end_us: a.closed_us + out.duration_us,
            wall_start,
        });
        for (kind, file) in out.files {
            if let Some(t) = a.manifest.tracks.iter_mut().find(|t| t.kind == kind) {
                t.files.push(file);
            }
        }
        a.closed_us += out.duration_us;
        a.bytes += out.bytes;
        a.manifest.duration_us = a.closed_us;
        a.segment_index += 1;
        a.write_manifest()
    }

    /// Abandon the active recording after an error. Its bundle stays in
    /// `.lumos-inprogress` for recovery.
    async fn fail(&self, rec: &mut Recorder, err: ToolError) -> ToolError {
        if let Some(mut a) = rec.active.take() {
            if let Some(h) = a.countdown.take() {
                h.abort();
            }
            if let Some(s) = a.session.take() {
                let _ = s.close().await;
            }
            a.manifest.status = RecordingStatus::Failed;
            a.manifest.warnings.push(Warning { at_us: a.closed_us, code: err.code.as_str().into(), message: err.message.clone() });
            let _ = a.write_manifest();
            self.emit(events::RECORDING_FAILED, json!({ "recordingId": a.id, "error": &err }));
        }
        rec.last_error = Some(err.clone());
        self.set_state(rec, RecorderState::Idle);
        err
    }

    pub(crate) async fn start(self: &Arc<Self>, client: ClientInfo, agent: bool, p: StartParams) -> ToolResult<RecorderStatus> {
        let s = self.settings();
        let mut rec = self.recorder.lock().await;
        if rec.state != RecorderState::Idle {
            return Err(ToolError::invalid_state("a recording is already in progress"));
        }

        let source = p.source.clone().unwrap_or_else(|| s.capture.default_source.clone());
        let camera = p.camera.clone().unwrap_or_default();
        let camera_enabled = camera.enabled.unwrap_or(s.camera.enabled);
        let mic = p.mic.clone().unwrap_or_default();
        let mic_enabled = mic.enabled.unwrap_or(s.audio.mic_enabled);
        let system_audio = p.system_audio.unwrap_or(s.audio.system_audio);
        let mut countdown = p.countdown_seconds.unwrap_or(s.recording.countdown_seconds).min(10);
        if agent {
            countdown = countdown.max(AGENT_MIN_COUNTDOWN);
        }
        let max_duration_us = p
            .max_duration_seconds
            .map(|x| x as i64 * 1_000_000)
            .or(s.recording.max_duration_minutes.map(|m| m as i64 * 60_000_000))
            .filter(|&us| us > 0);

        // Save folder: must exist, be writable and have room.
        let save_folder = expand_tilde(p.save_folder.as_deref().unwrap_or(&s.storage.save_folder));
        std::fs::create_dir_all(&save_folder).map_err(|e| io_err("create save folder", &save_folder, e))?;
        if !is_writable(&save_folder) {
            return Err(ToolError::new(ErrorCode::PermissionDenied, format!("can't write to {}", save_folder.display())));
        }
        if let Some(free) = free_mb(&save_folder) {
            if free < s.recording.min_free_disk_mb {
                return Err(ToolError::new(
                    ErrorCode::DiskLow,
                    format!("only {free} MB free in {} (minimum {} MB)", save_folder.display(), s.recording.min_free_disk_mb),
                ));
            }
        }
        if self.backend.permissions().await.screen == PermissionState::Denied {
            // Let macOS ask: it shows its own dialog and lists the app, which a hand-added entry may not match.
            self.backend.request_screen_access().await;
            if self.backend.permissions().await.screen == PermissionState::Denied {
                return Err(ToolError::new(
                    ErrorCode::PermissionDenied,
                    "Screen Recording permission is not granted. macOS should be asking now: choose Open System Settings, switch Lumos on, then record again. If Lumos is listed but still refused, select it, press the minus button, and record again so macOS asks.",
                ));
            }
        }

        let id = new_id("rec");
        let bundle_dir = save_folder.join(".lumos-inprogress").join(format!("{id}.lumos"));
        std::fs::create_dir_all(&bundle_dir).map_err(|e| io_err("create bundle", &bundle_dir, e))?;
        rec.last_error = None;
        self.set_state(&mut rec, RecorderState::Preparing);

        let config = SessionConfig {
            source: source.clone(),
            fps: s.capture.fps,
            max_short_side: s.capture.max_short_side(),
            screen_bitrate_kbps: s.advanced.screen_bitrate_kbps,
            camera_bitrate_kbps: s.advanced.camera_bitrate_kbps,
            camera: camera_enabled.then(|| camera.device_id.clone().or(s.camera.device_id.clone())),
            mic: mic_enabled.then(|| mic.device_id.clone().or(s.audio.mic_device_id.clone())),
            system_audio,
            bundle_dir: bundle_dir.clone(),
        };
        let session = match self.backend.open(config).await {
            Ok(session) => session,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&bundle_dir);
                rec.last_error = Some(e.clone());
                self.set_state(&mut rec, RecorderState::Idle);
                return Err(e);
            }
        };

        let title = p.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| s.storage.default_title.clone());
        let source_name = session.source_name();
        let tracks = session
            .tracks()
            .into_iter()
            .map(|t| Track {
                id: format!("trk_{}", t.kind.dir()),
                kind: t.kind,
                codec: t.codec,
                device_name: t.device_name,
                offset_us: 0,
                files: Vec::new(),
            })
            .collect();
        let manifest = Manifest {
            schema: MANIFEST_SCHEMA.into(),
            id: id.clone(),
            title: title.clone(),
            created_at: now_rfc3339(),
            status: RecordingStatus::Recording,
            duration_us: 0,
            origin: Origin::Recorded,
            app: AppInfo { version: env!("CARGO_PKG_VERSION").into(), platform: std::env::consts::OS.into() },
            capture: CaptureInfo {
                source: source.clone(),
                source_name: source_name.clone(),
                fps: s.capture.fps,
                show_cursor: s.capture.show_cursor,
                camera_burned_in: camera_enabled && s.camera.burn_in && !matches!(source, CaptureSource::Window { .. }),
            },
            segments: Vec::new(),
            tracks,
            markers: Vec::new(),
            edits: Edits::default(),
            outputs: Vec::new(),
            tags: Vec::new(),
            warnings: Vec::new(),
            shares: Vec::new(),
        };
        let mut active = Active {
            id: id.clone(),
            title,
            created_local: chrono::Local::now(),
            save_folder,
            bundle_dir,
            session: Some(session),
            manifest,
            segment_index: 0,
            closed_us: 0,
            open_segment: None,
            source: ActiveSource { source, name: source_name },
            camera_enabled,
            mic_enabled,
            system_audio,
            started_by: client,
            max_duration_us,
            bytes: 0,
            countdown: None,
        };
        if let Err(e) = active.write_manifest() {
            rec.active = Some(active);
            return Err(self.fail(&mut rec, e).await);
        }

        if countdown == 0 {
            let res = self.begin_segment(&mut active).await;
            rec.active = Some(active);
            if let Err(e) = res {
                return Err(self.fail(&mut rec, e).await);
            }
            self.set_state(&mut rec, RecorderState::Recording);
        } else {
            active.countdown = Some(self.spawn_countdown(id.clone(), countdown));
            rec.active = Some(active);
            self.set_state(&mut rec, RecorderState::Countdown);
        }
        self.spawn_ticker(id);
        Ok(self.status(&rec))
    }

    fn spawn_countdown(self: &Arc<Self>, id: String, seconds: u32) -> JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(seconds as u64)).await;
            let mut rec = me.recorder.lock().await;
            if rec.state != RecorderState::Countdown || rec.active.as_ref().is_none_or(|a| a.id != id) {
                return;
            }
            let a = rec.active.as_mut().expect("checked above");
            a.countdown = None;
            if let Err(e) = me.begin_segment(a).await {
                me.fail(&mut rec, e).await;
                return;
            }
            me.set_state(&mut rec, RecorderState::Recording);
        })
    }

    /// Ticks once a second, enforces the max duration and the disk-space floor.
    fn spawn_ticker(self: &Arc<Self>, id: String) {
        let me = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(TICK);
            let mut last_second = -1;
            let mut n: u64 = 0;
            loop {
                interval.tick().await;
                n += 1;
                let mut rec = me.recorder.lock().await;
                let Some(a) = rec.active.as_ref() else { break };
                if a.id != id {
                    break;
                }
                if rec.state != RecorderState::Recording {
                    continue;
                }
                let elapsed = a.elapsed_us();
                if elapsed / 1_000_000 != last_second {
                    last_second = elapsed / 1_000_000;
                    me.emit(events::RECORDER_TICK, json!({ "recordingId": id, "elapsedUs": elapsed }));
                }
                let mut reason = None;
                if a.max_duration_us.is_some_and(|max| elapsed >= max) {
                    reason = Some(("MAX_DURATION", "reached the maximum duration".to_string()));
                } else if n % DISK_CHECK_EVERY == 0 {
                    let min = me.settings().recording.min_free_disk_mb;
                    if let Some(free) = free_mb(&a.save_folder).filter(|&f| f < min) {
                        reason = Some(("DISK_LOW", format!("only {free} MB free; stopped to protect the recording")));
                    }
                }
                if let Some((code, message)) = reason {
                    me.emit(events::RECORDER_WARNING, json!({ "recordingId": id, "code": code, "message": message }));
                    if let Some(a) = rec.active.as_mut() {
                        a.manifest.warnings.push(Warning { at_us: elapsed, code: code.into(), message });
                    }
                    let _ = me.stop_locked(&mut rec).await;
                    break;
                }
            }
        });
    }

    pub(crate) async fn pause(&self) -> ToolResult<RecorderStatus> {
        let mut rec = self.recorder.lock().await;
        if rec.state != RecorderState::Recording {
            return Err(ToolError::invalid_state("not recording"));
        }
        let a = rec.active.as_mut().expect("recording has an active recording");
        a.manifest.status = RecordingStatus::Paused;
        if let Err(e) = self.end_segment(a).await {
            return Err(self.fail(&mut rec, e).await);
        }
        self.set_state(&mut rec, RecorderState::Paused);
        Ok(self.status(&rec))
    }

    pub(crate) async fn resume(&self) -> ToolResult<RecorderStatus> {
        let mut rec = self.recorder.lock().await;
        if rec.state != RecorderState::Paused {
            return Err(ToolError::invalid_state("not paused"));
        }
        let a = rec.active.as_mut().expect("paused has an active recording");
        a.manifest.status = RecordingStatus::Recording;
        if let Err(e) = self.begin_segment(a).await {
            return Err(self.fail(&mut rec, e).await);
        }
        self.set_state(&mut rec, RecorderState::Recording);
        Ok(self.status(&rec))
    }

    pub(crate) async fn stop(self: &Arc<Self>, wait: bool, timeout: Duration) -> ToolResult<StopResult> {
        let mut result = {
            let mut rec = self.recorder.lock().await;
            self.stop_locked(&mut rec).await?
        };
        if wait {
            let job = self.jobs.wait(&result.finalize_job_id, timeout).await?;
            match job.status {
                JobStatus::Done => {
                    if let Some(ready) = job.result.and_then(|v| serde_json::from_value::<RecordingReady>(v).ok()) {
                        result.output_path = Some(ready.output_path);
                        result.bundle_path = Some(ready.bundle_path);
                    }
                }
                JobStatus::Failed => return Err(job.error.unwrap_or_else(|| ToolError::backend("finalize failed"))),
                _ => {}
            }
        }
        Ok(result)
    }

    /// Close capture, hand the recording to a finalize job, return to idle.
    pub(crate) async fn stop_locked(self: &Arc<Self>, rec: &mut Recorder) -> ToolResult<StopResult> {
        let was = rec.state;
        if !matches!(was, RecorderState::Recording | RecorderState::Paused) {
            return Err(ToolError::invalid_state("nothing is recording"));
        }
        self.set_state(rec, RecorderState::Stopping);
        let mut a = rec.active.take().expect("active recording");
        if was == RecorderState::Recording {
            if let Err(e) = self.end_segment(&mut a).await {
                rec.active = Some(a);
                return Err(self.fail(rec, e).await);
            }
        }
        if let Some(session) = a.session.take() {
            if let Err(e) = session.close().await {
                a.manifest.warnings.push(Warning { at_us: a.closed_us, code: e.code.as_str().into(), message: e.message });
            }
        }
        a.manifest.status = RecordingStatus::Finalizing;
        let _ = a.write_manifest();

        let job = self.jobs.create(JobKind::Finalize, Some(a.id.clone()));
        let s = self.settings();
        let id = a.id.clone();
        let input = FinalizeInput {
            id: a.id,
            title: a.title,
            created_local: a.created_local,
            save_folder: a.save_folder,
            bundle_dir: a.bundle_dir,
            manifest: a.manifest,
            file_name_template: s.storage.file_name_template,
            keep_source_tracks: s.storage.keep_source_tracks,
            mic_gain_db: s.audio.mic_gain_db,
            system_gain_db: s.audio.system_gain_db,
            audio_offset_us: i64::from(s.audio.sync_offset_ms) * 1000,
        };
        tokio::spawn(finalize::run(self.clone(), job.id.clone(), input));
        self.set_state(rec, RecorderState::Idle);
        Ok(StopResult { recording_id: id, finalize_job_id: job.id, output_path: None, bundle_path: None })
    }

    pub(crate) async fn cancel(&self) -> ToolResult<CancelResult> {
        let mut rec = self.recorder.lock().await;
        let was = rec.state;
        if !matches!(was, RecorderState::Preparing | RecorderState::Countdown | RecorderState::Recording | RecorderState::Paused) {
            return Err(ToolError::invalid_state("nothing to cancel"));
        }
        self.set_state(&mut rec, RecorderState::Discarding);
        let mut a = rec.active.take().expect("active recording");
        if let Some(h) = a.countdown.take() {
            h.abort();
        }
        if let Some(mut session) = a.session.take() {
            if a.open_segment.take().is_some() {
                let _ = session.end_segment().await;
            }
            let _ = session.close().await;
        }
        let discarded = self.move_to_trash(&a);
        self.set_state(&mut rec, RecorderState::Idle);
        Ok(CancelResult { recording_id: a.id, discarded })
    }

    /// Move a bundle to the Trash (never a hard delete). Falls back to
    /// `<save>/.lumos-trash` when the Trash is on another volume.
    fn move_to_trash(&self, a: &Active) -> bool {
        let name = format!("{}.lumos", a.id);
        for dir in [self.paths.trash_dir.clone(), a.save_folder.join(".lumos-trash")] {
            if std::fs::create_dir_all(&dir).is_ok() && std::fs::rename(&a.bundle_dir, dir.join(&name)).is_ok() {
                return true;
            }
        }
        tracing::warn!(bundle = %a.bundle_dir.display(), "could not move cancelled recording to the Trash");
        false
    }

    pub(crate) async fn mark(&self, label: Option<String>) -> ToolResult<MarkResult> {
        let mut rec = self.recorder.lock().await;
        if !matches!(rec.state, RecorderState::Recording | RecorderState::Paused) {
            return Err(ToolError::invalid_state("nothing is recording"));
        }
        let a = rec.active.as_mut().expect("active recording");
        let marker = Marker { id: new_id("mrk"), at_us: a.elapsed_us(), label };
        let result = MarkResult { marker_id: marker.id.clone(), at_us: marker.at_us };
        a.manifest.markers.push(marker);
        a.write_manifest()?;
        Ok(result)
    }
}
