//! Library: list, get, update, delete and recover recordings, plus export and
//! import jobs. A recording is a `<name>.lumos` bundle next to `<name>.mp4`.
//! See docs/02-data-model.md and docs/07-import-export.md.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lumos_model::api::*;
use lumos_model::jobs::{JobKind, JobStatus};
use lumos_model::manifest::*;
use lumos_model::sources::CaptureSource;
use lumos_model::{ErrorCode, ToolError, ToolResult, new_id, now_rfc3339};
use serde_json::{Value, json};

use crate::engine::{DEFAULT_WAIT_MS, Inner, MAX_WAIT_MS};
use crate::finalize::{self, FinalizeInput};
use crate::fsutil::{expand_tilde, io_err, sanitize_file_name, unique_stem, write_json_atomic};

/// Extensions the `ffprobe` importer accepts.
const IMPORT_EXTS: &[&str] = &["mp4", "mov", "m4v", "webm", "mkv"];

fn media(e: lumos_media::MediaError) -> ToolError {
    ToolError::backend(e.to_string())
}

// ---- entries ---------------------------------------------------------------

pub(crate) struct Entry {
    pub bundle: PathBuf,
    pub manifest: Manifest,
}

impl Entry {
    fn load(bundle: PathBuf) -> Option<Self> {
        let body = std::fs::read(bundle.join("manifest.json")).ok()?;
        let manifest: Manifest = serde_json::from_slice(&body).ok()?;
        Some(Self { bundle, manifest })
    }

    fn resolve(&self, rel: &str) -> PathBuf {
        match rel.strip_prefix("../") {
            Some(rest) => self.bundle.parent().unwrap_or(Path::new(".")).join(rest),
            None => self.bundle.join(rel),
        }
    }

    /// The saved video: the `default` output, else the first one.
    fn output(&self) -> Option<PathBuf> {
        let o = self.manifest.outputs.iter().find(|o| o.preset == "default").or(self.manifest.outputs.first())?;
        Some(self.resolve(&o.path))
    }

    fn summary(&self) -> RecordingSummary {
        let m = &self.manifest;
        let output = self.output();
        let thumb = self.bundle.join("thumb.jpg");
        let bytes = output.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0) + dir_size(&self.bundle);
        RecordingSummary {
            id: m.id.clone(),
            title: m.title.clone(),
            created_at: m.created_at.clone(),
            duration_us: m.duration_us,
            status: m.status,
            origin: match m.origin {
                Origin::Recorded => "recorded".into(),
                Origin::Imported { .. } => "imported".into(),
            },
            tags: m.tags.clone(),
            bundle_path: self.bundle.display().to_string(),
            output_path: output.map(|p| p.display().to_string()),
            thumb_path: thumb.exists().then(|| thumb.display().to_string()),
            bytes,
            has_transcript: self.bundle.join("captions").join("transcript.txt").exists(),
        }
    }

    /// The transcript text, lowercased, if one exists.
    fn transcript_lower(&self) -> Option<String> {
        std::fs::read_to_string(self.bundle.join("captions").join("transcript.txt")).ok().map(|t| t.to_lowercase())
    }

    fn save(&self) -> ToolResult<()> {
        write_json_atomic(&self.bundle.join("manifest.json"), &self.manifest)
    }
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// All finished bundles in the save folder and one level of subfolders, newest first.
pub(crate) fn scan(save: &Path) -> Vec<Entry> {
    let mut dirs = vec![save.to_path_buf()];
    if let Ok(rd) = std::fs::read_dir(save) {
        for e in rd.flatten() {
            let p = e.path();
            let hidden = p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if p.is_dir() && !hidden && p.extension().is_none_or(|x| x != "lumos") {
                dirs.push(p);
            }
        }
    }
    let mut out: Vec<Entry> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .filter(|p| p.is_dir() && p.extension().is_some_and(|x| x == "lumos"))
        .filter_map(Entry::load)
        .collect();
    out.sort_by(|a, b| b.manifest.created_at.cmp(&a.manifest.created_at));
    out
}

fn find(entries: Vec<Entry>, id: &str) -> ToolResult<Entry> {
    if id == "last" {
        return entries.into_iter().next().ok_or_else(|| ToolError::not_found("the library is empty"));
    }
    let mut matches: Vec<Entry> = entries.into_iter().filter(|e| e.manifest.id == id || e.manifest.id.starts_with(id)).collect();
    if let Some(i) = matches.iter().position(|e| e.manifest.id == id) {
        return Ok(matches.swap_remove(i));
    }
    match matches.len() {
        0 => Err(ToolError::not_found(format!("no recording '{id}'"))),
        1 => Ok(matches.remove(0)),
        n => Err(ToolError::invalid_params(format!("'{id}' matches {n} recordings; use more of the id"))),
    }
}

// ---- export presets --------------------------------------------------------

pub(crate) fn presets() -> Vec<ExportPreset> {
    let p = |id: &str, name: &str, kind: &str, h: Option<u32>, fps: Option<u32>, kbps: Option<u32>| ExportPreset {
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
        max_height: h,
        fps,
        video_kbps: kbps,
    };
    vec![
        p("mp4-1080p", "MP4 1080p", "mp4", Some(1080), None, Some(6000)),
        p("mp4-720p", "MP4 720p (small)", "mp4", Some(720), Some(30), Some(2500)),
        p("gif", "GIF (480p, 12 fps)", "gif", Some(480), Some(12), None),
        p("audio", "Audio only (AAC)", "audio", None, None, None),
    ]
}

fn preset_ext(kind: &str) -> &'static str {
    match kind {
        "gif" => "gif",
        "audio" => "m4a",
        _ => "mp4",
    }
}

// ---- job plumbing ----------------------------------------------------------

impl Inner {
    /// With `wait`, block until the job ends (or `timeout_ms`) and return its result.
    async fn wait_result(&self, job_id: &str, wait: bool, timeout_ms: Option<u64>) -> ToolResult<Option<Value>> {
        if !wait {
            return Ok(None);
        }
        let timeout = Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_WAIT_MS).min(MAX_WAIT_MS));
        let job = self.jobs.wait(job_id, timeout).await?;
        match job.status {
            JobStatus::Done => Ok(job.result),
            JobStatus::Failed => Err(job.error.unwrap_or_else(|| ToolError::backend("job failed"))),
            _ => Ok(None),
        }
    }

    fn save_folder(&self) -> PathBuf {
        expand_tilde(&self.settings().storage.save_folder)
    }

    fn entries(&self) -> Vec<Entry> {
        scan(&self.save_folder())
    }

    // ---- read / update / delete -------------------------------------------

    pub(crate) fn library_list(&self, p: LibraryListParams) -> ToolResult<LibraryListResult> {
        let q = p.query.as_deref().map(str::to_lowercase);
        let mut items: Vec<RecordingSummary> = self
            .entries()
            .iter()
            .filter(|e| match &q {
                None => true,
                Some(q) => e.manifest.title.to_lowercase().contains(q) || e.transcript_lower().is_some_and(|t| t.contains(q)),
            })
            .map(Entry::summary)
            .collect();
        items.retain(|r| p.tags.iter().all(|t| r.tags.contains(t)));
        if let Some(o) = &p.origin {
            if !matches!(o.as_str(), "recorded" | "imported") {
                return Err(ToolError::invalid_params("origin must be 'recorded' or 'imported'"));
            }
            items.retain(|r| &r.origin == o);
        }
        match p.sort.as_deref().unwrap_or("newest") {
            "newest" => {}
            "oldest" => items.reverse(),
            "longest" => items.sort_by_key(|r| std::cmp::Reverse(r.duration_us)),
            "title" => items.sort_by_key(|r| r.title.to_lowercase()),
            other => return Err(ToolError::invalid_params(format!("unknown sort '{other}'"))),
        }
        let total = items.len();
        let items = items.into_iter().skip(p.offset.unwrap_or(0)).take(p.limit.unwrap_or(50)).collect();
        Ok(LibraryListResult { items, total })
    }

    pub(crate) fn library_get(&self, id: &str) -> ToolResult<RecordingDetails> {
        let e = find(self.entries(), id)?;
        Ok(RecordingDetails { summary: e.summary(), manifest: e.manifest })
    }

    pub(crate) fn library_update(&self, p: LibraryUpdateParams) -> ToolResult<RecordingSummary> {
        let mut e = find(self.entries(), &p.id)?;
        if let Some(tags) = p.tags {
            let mut seen = Vec::new();
            for t in tags.into_iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
                if !seen.contains(&t) {
                    seen.push(t);
                }
            }
            e.manifest.tags = seen;
        }
        if let Some(title) = p.title {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err(ToolError::invalid_params("title can't be empty"));
            }
            e = self.rename(e, &title)?;
        }
        e.save()?;
        Ok(e.summary())
    }

    /// Rename the bundle and its default output to match a new title.
    fn rename(&self, mut e: Entry, title: &str) -> ToolResult<Entry> {
        let dir = e.bundle.parent().unwrap_or(Path::new(".")).to_path_buf();
        let output = e.output().filter(|p| p.exists());
        let ext = output.as_ref().and_then(|p| p.extension()).map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| "mp4".into());
        let current = e.bundle.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let wanted = sanitize_file_name(title);
        e.manifest.title = title.to_string();
        if wanted == current {
            return Ok(e);
        }
        let stem = unique_stem(&dir, &wanted, &[ext.as_str(), "lumos"]);
        let new_bundle = dir.join(format!("{stem}.lumos"));
        std::fs::rename(&e.bundle, &new_bundle).map_err(|err| io_err("rename", &new_bundle, err))?;
        e.bundle = new_bundle;
        if let Some(old) = output {
            let new_out = dir.join(format!("{stem}.{ext}"));
            std::fs::rename(&old, &new_out).map_err(|err| io_err("rename", &new_out, err))?;
            if let Some(o) = e.manifest.outputs.iter_mut().find(|o| o.preset == "default") {
                o.path = format!("../{stem}.{ext}");
            }
        }
        Ok(e)
    }

    pub(crate) fn library_delete(&self, p: LibraryDeleteParams) -> ToolResult<LibraryDeleteResult> {
        if !p.confirm {
            return Err(ToolError::invalid_params("pass confirm: true to move the recording to the Trash"));
        }
        let e = find(self.entries(), &p.id)?;
        let mut trashed = Vec::new();
        if !p.keep_output {
            if let Some(out) = e.output().filter(|o| o.exists()) {
                trashed.push(self.trash(&out)?);
            }
        }
        trashed.push(self.trash(&e.bundle)?);
        self.emit("library.deleted", json!({ "recordingId": e.manifest.id }));
        Ok(LibraryDeleteResult { id: e.manifest.id, trashed })
    }

    /// Move to the Trash (never a hard delete); falls back to `<save>/.lumos-trash`
    /// when the Trash is on another volume.
    fn trash(&self, path: &Path) -> ToolResult<String> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "item".into());
        for dir in [self.paths.trash_dir.clone(), self.save_folder().join(".lumos-trash")] {
            if std::fs::create_dir_all(&dir).is_err() {
                continue;
            }
            let mut dest = dir.join(&name);
            if dest.exists() {
                dest = dir.join(format!("{}-{name}", new_id("x")));
            }
            if std::fs::rename(path, &dest).is_ok() {
                return Ok(dest.display().to_string());
            }
        }
        Err(ToolError::backend(format!("could not move {} to the Trash", path.display())))
    }

    pub(crate) fn library_reveal(&self, p: LibraryRevealParams) -> ToolResult<PathResult> {
        let e = find(self.entries(), &p.id)?;
        let path = match p.which.as_deref().unwrap_or("output") {
            "output" => e.output().ok_or_else(|| ToolError::not_found("recording has no output file"))?,
            "bundle" => e.bundle.clone(),
            other => return Err(ToolError::invalid_params(format!("which must be 'output' or 'bundle', not '{other}'"))),
        };
        if std::env::var_os("LUMOS_NO_REVEAL").is_none() {
            let mut cmd = if cfg!(target_os = "macos") {
                let mut c = std::process::Command::new("open");
                c.arg("-R").arg(&path);
                c
            } else {
                let mut c = std::process::Command::new("xdg-open");
                c.arg(path.parent().unwrap_or(Path::new(".")));
                c
            };
            let _ = cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
        }
        Ok(PathResult { path: path.display().to_string() })
    }

    // ---- recover ----------------------------------------------------------

    pub(crate) async fn library_recover(self: &Arc<Self>, p: RecoverParams) -> ToolResult<RecoverResult> {
        let job = self.jobs.create(JobKind::Recover, p.id.clone());
        tokio::spawn(run_recover(self.clone(), job.id.clone(), p.id));
        let waited = self.wait_result(&job.id, p.wait, p.timeout_ms).await?;
        let mut out = RecoverResult { job_id: job.id, recovered: None, skipped: None };
        if let Some(v) = waited {
            out.recovered = serde_json::from_value(v["recovered"].clone()).ok();
            out.skipped = serde_json::from_value(v["skipped"].clone()).ok();
        }
        Ok(out)
    }

    // ---- export -----------------------------------------------------------

    pub(crate) fn export_presets(&self) -> Vec<ExportPreset> {
        presets()
    }

    pub(crate) async fn export_create(self: &Arc<Self>, p: ExportParams) -> ToolResult<ExportResult> {
        let e = find(self.entries(), &p.id)?;
        let preset = presets().into_iter().find(|x| x.id == p.preset).ok_or_else(|| {
            let ids: Vec<String> = presets().into_iter().map(|x| x.id).collect();
            ToolError::invalid_params(format!("unknown preset '{}'; available: {}", p.preset, ids.join(", ")))
        })?;
        let src = e.output().filter(|o| o.exists()).ok_or_else(|| ToolError::invalid_state("the recording's video file is missing"))?;
        let ext = preset_ext(&preset.kind);
        let dest = match &p.output_path {
            Some(path) => {
                let dest = expand_tilde(path);
                if dest.exists() {
                    return Err(ToolError::invalid_params(format!("{} already exists", dest.display())));
                }
                dest
            }
            None => {
                let dir = src.parent().unwrap_or(Path::new(".")).to_path_buf();
                let base = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let stem = unique_stem(&dir, &format!("{base} ({})", preset.id), &[ext]);
                dir.join(format!("{stem}.{ext}"))
            }
        };
        let range = match &p.range {
            None => None,
            Some(r) => {
                let total = e.manifest.duration_us as f64 / 1e6;
                if r.start_seconds < 0.0 || r.end_seconds.is_some_and(|end| end <= r.start_seconds) || r.start_seconds >= total {
                    return Err(ToolError::invalid_params(format!(
                        "range must start inside the recording ({total:.1}s) and end after it starts"
                    )));
                }
                Some((r.start_seconds, r.end_seconds))
            }
        };
        let job = self.jobs.create(JobKind::Export, Some(e.manifest.id.clone()));
        tokio::spawn(run_export(self.clone(), job.id.clone(), e, preset, src, dest, range));
        let waited = self.wait_result(&job.id, p.wait, p.timeout_ms).await?;
        let mut out = ExportResult { job_id: job.id, path: None, bytes: None, duration_us: None };
        if let Some(v) = waited {
            out.path = v["path"].as_str().map(str::to_owned);
            out.bytes = v["bytes"].as_u64();
            out.duration_us = v["durationUs"].as_i64();
        }
        Ok(out)
    }

    // ---- transcripts ------------------------------------------------------

    pub(crate) async fn transcript_create(self: &Arc<Self>, p: TranscriptCreateParams) -> ToolResult<TranscriptCreateResult> {
        let e = find(self.entries(), &p.id)?;
        let model = p
            .model
            .clone()
            .or_else(|| self.settings().advanced.whisper_model_path.clone())
            .or_else(|| std::env::var("LUMOS_WHISPER_MODEL").ok())
            .map(|m| expand_tilde(&m))
            .ok_or_else(|| ToolError::invalid_params("no whisper model: pass `model` (a ggml-*.bin path) or set Settings > advanced.whisperModelPath"))?;
        if !model.is_file() {
            return Err(ToolError::invalid_params(format!("whisper model not found: {}", model.display())));
        }
        let src = e.output().filter(|o| o.exists()).ok_or_else(|| ToolError::invalid_state("the recording's video file is missing"))?;
        let language = p.language.clone().unwrap_or_else(|| "auto".into());
        let job = self.jobs.create(JobKind::Transcribe, Some(e.manifest.id.clone()));
        let (inner, job_id) = (self.clone(), job.id.clone());
        tokio::spawn(async move {
            inner.jobs.start(&job_id);
            inner.jobs.progress(&job_id, 0.05);
            let dir = e.bundle.join("captions");
            let r = match std::fs::create_dir_all(&dir) {
                Err(err) => Err(io_err("create", &dir, err)),
                Ok(()) => {
                    let base = dir.join("transcript");
                    lumos_media::transcribe(&src, &model, &language, &base).await.map_err(media).map(|()| {
                        json!({ "jobId": job_id, "path": base.with_extension("srt").display().to_string() })
                    })
                }
            };
            if let Ok(v) = &r {
                inner.emit("transcribe.finished", v.clone());
            }
            inner.jobs.finish(&job_id, r);
        });
        let waited = self.wait_result(&job.id, p.wait, p.timeout_ms).await?;
        Ok(TranscriptCreateResult { job_id: job.id, path: waited.and_then(|v| v["path"].as_str().map(str::to_owned)) })
    }

    pub(crate) fn transcript_get(&self, p: TranscriptGetParams) -> ToolResult<TranscriptGetResult> {
        let e = find(self.entries(), &p.id)?;
        let format = p.format.unwrap_or_else(|| "txt".into());
        if !["txt", "srt", "vtt"].contains(&format.as_str()) {
            return Err(ToolError::invalid_params("format must be txt, srt or vtt"));
        }
        let path = e.bundle.join("captions").join(format!("transcript.{format}"));
        let text = std::fs::read_to_string(&path)
            .map_err(|_| ToolError::new(ErrorCode::NotFound, "no transcript yet; run transcript.create first"))?;
        Ok(TranscriptGetResult { format, text })
    }

    /// Chapters from the user's markers, plus a new chapter wherever the transcript has a long pause.
    /// Titles of the second kind are the first words said after the pause; summaries are left to the client.
    pub(crate) fn chapters_suggest(&self, p: RecordingIdParams) -> ToolResult<ChaptersResult> {
        let e = find(self.entries(), &p.id)?;
        let mut chapters = vec![Chapter { start_seconds: 0.0, title: "Start".into(), source: "start".into() }];
        for (i, m) in e.manifest.markers.iter().enumerate() {
            chapters.push(Chapter {
                start_seconds: m.at_us as f64 / 1e6,
                title: m.label.clone().unwrap_or_else(|| format!("Marker {}", i + 1)),
                source: "marker".into(),
            });
        }
        if let Ok(srt) = std::fs::read_to_string(e.bundle.join("captions").join("transcript.srt")) {
            let mut prev_end = 0.0;
            for cue in parse_srt(&srt) {
                let near_marker = chapters.iter().any(|c| (c.start_seconds - cue.0).abs() < 5.0);
                if cue.0 - prev_end >= 6.0 && !near_marker {
                    let title: String = cue.2.split_whitespace().take(6).collect::<Vec<_>>().join(" ");
                    chapters.push(Chapter { start_seconds: cue.0, title, source: "transcript".into() });
                }
                prev_end = cue.1;
            }
        }
        chapters.sort_by(|a, b| a.start_seconds.total_cmp(&b.start_seconds));
        Ok(ChaptersResult { chapters })
    }

    // ---- import -----------------------------------------------------------

    pub(crate) fn import_formats(&self) -> Vec<ImportFormat> {
        vec![ImportFormat {
            importer: "ffprobe".into(),
            extensions: IMPORT_EXTS.iter().map(|e| e.to_string()).collect(),
            description: "Video files ffmpeg can read (MP4, MOV, M4V, WebM, MKV)".into(),
        }]
    }

    pub(crate) async fn import_create(self: &Arc<Self>, p: ImportParams) -> ToolResult<ImportResult> {
        if p.paths.is_empty() {
            return Err(ToolError::invalid_params("paths is empty"));
        }
        let job = self.jobs.create(JobKind::Import, None);
        tokio::spawn(run_import(self.clone(), job.id.clone(), p.paths.clone(), p.title.clone(), p.tags.clone()));
        let waited = self.wait_result(&job.id, p.wait, p.timeout_ms).await?;
        let mut out = ImportResult { job_id: job.id, recordings: None, skipped: None };
        if let Some(v) = waited {
            out.recordings = serde_json::from_value(v["recordings"].clone()).ok();
            out.skipped = serde_json::from_value(v["skipped"].clone()).ok();
        }
        Ok(out)
    }
}

// ---- jobs ------------------------------------------------------------------

async fn run_recover(inner: Arc<Inner>, job_id: String, only: Option<String>) {
    inner.jobs.start(&job_id);
    let r = recover_all(&inner, &job_id, only).await;
    inner.jobs.finish(&job_id, r);
}

async fn recover_all(inner: &Arc<Inner>, job_id: &str, only: Option<String>) -> ToolResult<Value> {
    let save = inner.save_folder();
    let active = inner.recorder.lock().await.active.as_ref().map(|a| a.id.clone());
    let finalizing: Vec<String> = inner
        .jobs
        .list(200)
        .into_iter()
        .filter(|j| j.kind == JobKind::Finalize && !j.status.is_terminal())
        .filter_map(|j| j.recording_id)
        .collect();
    let mut bundles: Vec<PathBuf> = std::fs::read_dir(save.join(".lumos-inprogress"))
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    bundles.retain(|p| p.extension().is_some_and(|x| x == "lumos") && p.is_dir());
    bundles.retain(|p| {
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        active.as_deref() != Some(stem.as_str())
            && !finalizing.contains(&stem)
            && only.as_deref().is_none_or(|id| stem == id || stem.starts_with(id))
    });
    bundles.sort();
    if let Some(id) = &only {
        if bundles.is_empty() {
            return Err(ToolError::not_found(format!("no unfinished recording '{id}'")));
        }
    }

    let (mut recovered, mut skipped) = (Vec::new(), Vec::new());
    let total = bundles.len().max(1) as f64;
    for (i, bundle) in bundles.iter().enumerate() {
        match recover_one(inner, job_id, &save, bundle).await {
            Ok(summary) => recovered.push(summary),
            Err(reason) => skipped.push(format!("{}: {reason}", bundle.display())),
        }
        inner.jobs.progress(job_id, (i as f64 + 1.0) / total);
    }
    Ok(json!({ "recovered": recovered, "skipped": skipped }))
}

async fn recover_one(inner: &Arc<Inner>, job_id: &str, save: &Path, bundle: &Path) -> Result<RecordingSummary, String> {
    let body = std::fs::read(bundle.join("manifest.json")).map_err(|_| "no manifest.json".to_string())?;
    let mut manifest: Manifest = serde_json::from_slice(&body).map_err(|e| format!("unreadable manifest: {e}"))?;
    if manifest.segments.is_empty() {
        return Err("no finished segments (recording stopped before the first one closed)".into());
    }
    // Keep only segment files that exist and play; the segment open at the crash is not listed.
    for t in &mut manifest.tracks {
        let mut ok = Vec::new();
        for rel in std::mem::take(&mut t.files) {
            let p = bundle.join(&rel);
            if p.exists() && lumos_media::probe_duration_us(&p).await.is_ok_and(|d| d > 0) {
                ok.push(rel);
            }
        }
        t.files = ok;
    }
    manifest.tracks.retain(|t| !t.files.is_empty());
    if !manifest.tracks.iter().any(|t| t.kind == TrackKind::Screen) {
        return Err("no playable screen video".into());
    }
    manifest.warnings.push(Warning { at_us: manifest.duration_us, code: "RECOVERED".into(), message: "recovered after the engine stopped unexpectedly".into() });
    let created_local = chrono::DateTime::parse_from_rfc3339(&manifest.created_at).map(|d| d.with_timezone(&chrono::Local)).unwrap_or_else(|_| chrono::Local::now());
    let s = inner.settings();
    let mut input = FinalizeInput {
        id: manifest.id.clone(),
        title: manifest.title.clone(),
        created_local,
        save_folder: save.to_path_buf(),
        bundle_dir: bundle.to_path_buf(),
        manifest,
        file_name_template: s.storage.file_name_template,
        keep_source_tracks: s.storage.keep_source_tracks,
        mic_gain_db: s.audio.mic_gain_db,
        system_gain_db: s.audio.system_gain_db,
    };
    let ready = finalize::finalize(inner, job_id, &mut input).await.map_err(|e| e.message)?;
    inner.emit(lumos_model::events::RECORDING_READY, &ready);

    let stem = Path::new(&ready.output_path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let new_bundle = if ready.bundle_path.is_empty() { save.join(format!("{stem}.lumos")) } else { PathBuf::from(&ready.bundle_path) };
    match Entry::load(new_bundle.clone()) {
        Some(mut e) => {
            e.manifest.status = RecordingStatus::Recovered;
            e.save().map_err(|e| e.message)?;
            Ok(e.summary())
        }
        None => {
            // keepSourceTracks = never: the bundle is gone, report from the manifest we have.
            input.manifest.status = RecordingStatus::Recovered;
            Ok(Entry { bundle: new_bundle, manifest: input.manifest }.summary())
        }
    }
}

async fn run_export(inner: Arc<Inner>, job_id: String, mut e: Entry, preset: ExportPreset, src: PathBuf, dest: PathBuf, range: Option<(f64, Option<f64>)>) {
    inner.jobs.start(&job_id);
    let r = export_one(&inner, &job_id, &mut e, &preset, &src, &dest, range).await;
    if let Ok(v) = &r {
        inner.emit("export.finished", v);
    }
    inner.jobs.finish(&job_id, r);
}

async fn export_one(inner: &Inner, job_id: &str, e: &mut Entry, preset: &ExportPreset, src: &Path, dest: &Path, range: Option<(f64, Option<f64>)>) -> ToolResult<Value> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|err| io_err("create", dir, err))?;
    }
    // Write beside the destination, then rename, so a half-written file never has the final name.
    let tmp = dest.with_file_name(format!(".{}.part.{}", dest.file_stem().unwrap_or_default().to_string_lossy(), dest.extension().unwrap_or_default().to_string_lossy()));
    inner.jobs.progress(job_id, 0.05);
    let res = lumos_media::transcode(lumos_media::Transcode {
        input: src,
        output: &tmp,
        kind: &preset.kind,
        max_height: preset.max_height,
        fps: preset.fps,
        video_kbps: preset.video_kbps,
        start_seconds: range.map(|r| r.0),
        end_seconds: range.and_then(|r| r.1),
    })
    .await;
    if let Err(err) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(media(err));
    }
    std::fs::rename(&tmp, dest).map_err(|err| io_err("move", dest, err))?;
    let bytes = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    let duration_us = lumos_media::probe_duration_us(dest).await.unwrap_or(e.manifest.duration_us);
    e.manifest.outputs.push(OutputFile {
        kind: preset.kind.clone(),
        preset: preset.id.clone(),
        path: dest.display().to_string(),
        created_at: now_rfc3339(),
        bytes,
    });
    // The manifest is the record of exports, but a missing bundle must not fail a finished export.
    if let Err(err) = e.save() {
        tracing::warn!(error = %err, "could not record the export in the manifest");
    }
    Ok(json!({ "jobId": job_id, "path": dest.display().to_string(), "bytes": bytes, "durationUs": duration_us }))
}

async fn run_import(inner: Arc<Inner>, job_id: String, paths: Vec<String>, title: Option<String>, tags: Vec<String>) {
    inner.jobs.start(&job_id);
    let save = inner.save_folder();
    let (mut recordings, mut skipped) = (Vec::new(), Vec::new());
    let total = paths.len() as f64;
    for (i, raw) in paths.iter().enumerate() {
        let path = expand_tilde(raw);
        // A shared title would make every file the same; only use it for a single file.
        let t = if paths.len() == 1 { title.as_deref() } else { None };
        match import_one(&save, &path, t, &tags).await {
            Ok(s) => {
                inner.emit("library.imported", json!({ "recordingId": s.id }));
                recordings.push(s);
            }
            Err(reason) => skipped.push(format!("{raw}: {reason}")),
        }
        inner.jobs.progress(&job_id, (i as f64 + 1.0) / total);
    }
    let result = if recordings.is_empty() {
        Err(ToolError::new(ErrorCode::InvalidParams, format!("nothing imported. {}", skipped.join("; "))))
    } else {
        Ok(json!({ "recordings": recordings, "skipped": skipped }))
    };
    inner.jobs.finish(&job_id, result);
}

async fn import_one(save: &Path, path: &Path, title: Option<&str>, tags: &[String]) -> Result<RecordingSummary, String> {
    let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !IMPORT_EXTS.contains(&ext.as_str()) {
        return Err(format!("unsupported file type '.{ext}' (supported: {})", IMPORT_EXTS.join(", ")));
    }
    if !path.is_file() {
        return Err("file not found".into());
    }
    let streams = lumos_media::probe_streams(path).await.map_err(|e| e.to_string())?;
    if !streams.iter().any(|s| s == "video") {
        return Err("no video stream".into());
    }
    let duration_us = lumos_media::probe_duration_us(path).await.map_err(|e| e.to_string())?;

    let title = title.map(str::to_owned).unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Imported".into()));
    std::fs::create_dir_all(save.join(".lumos-inprogress")).map_err(|e| e.to_string())?;
    let id = new_id("rec");
    let stem = unique_stem(save, &sanitize_file_name(&title), &[ext.as_str(), "mp4", "lumos"]);
    let output = save.join(format!("{stem}.{ext}"));
    let part = save.join(".lumos-inprogress").join(format!("{id}.import"));
    tokio::fs::copy(path, &part).await.map_err(|e| format!("copy failed: {e}"))?;
    tokio::fs::rename(&part, &output).await.map_err(|e| format!("move failed: {e}"))?;
    let bytes = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);

    let bundle = save.join(format!("{stem}.lumos"));
    std::fs::create_dir_all(&bundle).map_err(|e| e.to_string())?;
    let rel = format!("../{stem}.{ext}");
    let now = now_rfc3339();
    let manifest = Manifest {
        schema: MANIFEST_SCHEMA.into(),
        id,
        title,
        created_at: now.clone(),
        status: RecordingStatus::Ready,
        duration_us,
        origin: Origin::Imported { from: path.display().to_string(), importer: "ffprobe".into() },
        app: AppInfo { version: env!("CARGO_PKG_VERSION").into(), platform: std::env::consts::OS.into() },
        capture: CaptureInfo { source: CaptureSource::default(), source_name: "Imported file".into(), fps: 0, show_cursor: false, camera_burned_in: false },
        segments: vec![Segment { id: "seg_1".into(), start_us: 0, end_us: duration_us, wall_start: now.clone() }],
        tracks: vec![Track { id: "trk_screen".into(), kind: TrackKind::Screen, codec: "video".into(), device_name: None, offset_us: 0, files: vec![rel.clone()] }],
        markers: Vec::new(),
        edits: Edits::default(),
        outputs: vec![OutputFile { kind: ext, preset: "default".into(), path: rel, created_at: now, bytes }],
        tags: tags.to_vec(),
        warnings: Vec::new(),
    };
    let entry = Entry { bundle: bundle.clone(), manifest };
    entry.save().map_err(|e| e.message)?;
    // Best effort: a missing thumbnail never fails an import.
    if let Some(out) = entry.output() {
        let _ = lumos_media::thumbnail(&out, (duration_us / 2).min(1_000_000), &bundle.join("thumb.jpg")).await;
    }
    Ok(entry.summary())
}

/// `(start, end, text)` in seconds for each SRT cue.
fn parse_srt(srt: &str) -> Vec<(f64, f64, String)> {
    fn secs(t: &str) -> Option<f64> {
        let (hms, ms) = t.trim().split_once(',')?;
        let mut it = hms.split(':').map(|x| x.parse::<f64>().ok());
        Some(it.next()?? * 3600.0 + it.next()?? * 60.0 + it.next()?? + ms.parse::<f64>().ok()? / 1000.0)
    }
    srt.replace("\r\n", "\n")
        .split("\n\n")
        .filter_map(|block| {
            let mut lines = block.lines().filter(|l| !l.trim().is_empty());
            lines.next()?;
            let (a, b) = lines.next()?.split_once("-->")?;
            Some((secs(a)?, secs(b)?, lines.collect::<Vec<_>>().join(" ").trim().to_string()))
        })
        .collect()
}
