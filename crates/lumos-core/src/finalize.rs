//! Finalize job: segments → one MP4 in the save folder, bundle moved next to it.
//! See docs/03-recording-pipeline.md §5.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lumos_model::api::RecordingReady;
use lumos_model::events;
use lumos_model::manifest::{Manifest, OutputFile, RecordingStatus, TrackKind, Warning};
use lumos_model::{ToolError, ToolResult, now_rfc3339};
use serde_json::json;

use crate::engine::Inner;
use crate::fsutil::{io_err, render_file_name, unique_stem, write_json_atomic};

pub(crate) struct FinalizeInput {
    pub id: String,
    pub title: String,
    pub created_local: chrono::DateTime<chrono::Local>,
    pub save_folder: PathBuf,
    pub bundle_dir: PathBuf,
    pub manifest: Manifest,
    pub file_name_template: String,
    pub keep_source_tracks: String,
    pub mic_gain_db: f64,
    pub system_gain_db: f64,
    /// Added to every audio segment's start, in microseconds (the `audio.syncOffsetMs` setting).
    pub audio_offset_us: i64,
}

pub(crate) async fn run(inner: Arc<Inner>, job_id: String, mut input: FinalizeInput) {
    inner.jobs.start(&job_id);
    match finalize(&inner, &job_id, &mut input).await {
        Ok(ready) => {
            tracing::info!(recording = %ready.recording_id, output = %ready.output_path, "recording saved");
            // Mark the job done first, so a client that reacts to the event and reads the recorder
            // status no longer sees this recording as "still saving".
            inner.jobs.finish(&job_id, serde_json::to_value(&ready).map_err(|e| ToolError::backend(e.to_string())));
            inner.emit(events::RECORDING_READY, &ready);
        }
        Err(e) => {
            tracing::error!(recording = %input.id, error = %e, "finalize failed");
            // The bundle stays in .lumos-inprogress so recovery can retry it.
            input.manifest.status = RecordingStatus::Failed;
            input.manifest.warnings.push(Warning { at_us: input.manifest.duration_us, code: e.code.as_str().into(), message: e.message.clone() });
            let _ = write_json_atomic(&input.bundle_dir.join("manifest.json"), &input.manifest);
            inner.jobs.finish(&job_id, Err(e.clone()));
            inner.emit(events::RECORDING_FAILED, json!({ "recordingId": input.id, "error": &e }));
        }
    }
}

fn media(e: lumos_media::MediaError) -> ToolError {
    ToolError::backend(e.to_string())
}

pub(crate) async fn finalize(inner: &Inner, job_id: &str, f: &mut FinalizeInput) -> ToolResult<RecordingReady> {
    let bundle = f.bundle_dir.clone();
    let work = bundle.join("work");
    std::fs::create_dir_all(&work).map_err(|e| io_err("create", &work, e))?;

    // 1. Join each track's segments without re-encoding.
    let tracks = f.manifest.tracks.clone();
    let steps = tracks.len() as f64 + 3.0;
    let mut joined: HashMap<TrackKind, PathBuf> = HashMap::new();
    for (i, t) in tracks.iter().enumerate() {
        if t.files.is_empty() {
            continue;
        }
        let inputs: Vec<PathBuf> = t.files.iter().map(|rel| bundle.join(rel)).collect();
        let segs = &f.manifest.segments;
        let audio = matches!(t.kind, TrackKind::Mic | TrackKind::SystemAudio);
        let out = if audio && !segs.is_empty() && segs.len() == inputs.len() {
            // Put every audio segment back on the video's clock: it starts where its first sample really
            // began and is exactly as long as its video segment. Joining raw PCM avoids encoder gaps.
            let mut parts = Vec::new();
            for (i, (input, seg)) in inputs.iter().zip(segs).enumerate() {
                let part = work.join(format!("{}-{i:03}.wav", t.kind.dir()));
                lumos_media::fit_audio(input, &part, (seg.end_us - seg.start_us).max(0), f.audio_offset_us).await.map_err(media)?;
                parts.push(part);
            }
            let out = work.join(format!("{}.wav", t.kind.dir()));
            lumos_media::concat(&parts, &out).await.map_err(media)?;
            out
        } else {
            let out = work.join(format!("{}.{}", t.kind.dir(), t.kind.ext()));
            lumos_media::concat(&inputs, &out).await.map_err(media)?;
            out
        };
        joined.insert(t.kind, out);
        inner.jobs.progress(job_id, (i as f64 + 1.0) / steps);
    }
    let screen = joined.get(&TrackKind::Screen).ok_or_else(|| ToolError::backend("recording has no screen video"))?;

    // 2. Mix microphone and system audio. The camera track stays in the bundle;
    //    on display sources the bubble is already part of the screen video.
    let mut audio_in = Vec::new();
    let mut gains = Vec::new();
    for (kind, gain) in [(TrackKind::Mic, f.mic_gain_db), (TrackKind::SystemAudio, f.system_gain_db)] {
        if let Some(p) = joined.get(&kind) {
            audio_in.push(p.clone());
            gains.push(gain);
        }
    }
    let audio = if audio_in.is_empty() {
        None
    } else {
        let out = work.join("audio.m4a");
        lumos_media::mix_audio(&audio_in, &gains, &out).await.map_err(media)?;
        Some(out)
    };
    inner.jobs.progress(job_id, (steps - 2.0) / steps);

    // 3. Mux into a temp file on the same volume, then rename into place.
    let tmp_out = f.save_folder.join(".lumos-inprogress").join(format!("{}.out.mp4", f.id));
    lumos_media::mux(screen, audio.as_deref(), &tmp_out).await.map_err(media)?;
    let thumb = bundle.join("thumb.jpg");
    let at = (f.manifest.duration_us / 2).min(1_000_000);
    if let Err(e) = lumos_media::thumbnail(&tmp_out, at, &thumb).await {
        f.manifest.warnings.push(Warning { at_us: at, code: "THUMBNAIL".into(), message: e.to_string() });
    }
    inner.jobs.progress(job_id, (steps - 1.0) / steps);

    let base = render_file_name(&f.file_name_template, &f.title, f.created_local);
    let stem = unique_stem(&f.save_folder, &base, &["mp4", "lumos"]);
    let output = f.save_folder.join(format!("{stem}.mp4"));
    std::fs::rename(&tmp_out, &output).map_err(|e| io_err("move", &output, e))?;
    let bytes = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);

    f.manifest.status = RecordingStatus::Ready;
    f.manifest.outputs.push(OutputFile {
        kind: "mp4".into(),
        preset: "default".into(),
        path: format!("../{stem}.mp4"),
        created_at: now_rfc3339(),
        bytes,
    });
    let _ = std::fs::remove_dir_all(&work);
    // 4. If the user keeps only MP4s, drop the source tracks (nearly all of the bundle's size), but
    //    keep the manifest and thumbnail so the recording still shows up in the library.
    if f.keep_source_tracks == "never" {
        remove_tracks(&bundle.join("tracks"));
        f.manifest.tracks.clear();
    }
    write_json_atomic(&bundle.join("manifest.json"), &f.manifest)?;

    // 5. Move the bundle next to the MP4.
    let dest = f.save_folder.join(format!("{stem}.lumos"));
    std::fs::rename(&bundle, &dest).map_err(|e| io_err("move", &dest, e))?;
    let bundle_path = dest.display().to_string();

    Ok(RecordingReady {
        recording_id: f.id.clone(),
        output_path: output.display().to_string(),
        bundle_path,
        duration_us: f.manifest.duration_us,
    })
}

fn remove_tracks(tracks: &Path) {
    if let Err(e) = std::fs::remove_dir_all(tracks) {
        tracing::warn!(tracks = %tracks.display(), error = %e, "could not remove source tracks");
    }
}
