//! End-to-end through a real Unix socket with the fake capture backend:
//! start → pause → resume → stop → wait for the saved MP4.
//!
//! Needs `ffmpeg` and `ffprobe` on PATH (or LUMOS_FFMPEG / LUMOS_FFPROBE).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lumos_capture::fake::FakeBackend;
use lumos_core::{Engine, EngineOptions};
use lumos_model::ErrorCode;
use lumos_model::manifest::Manifest;
use lumos_rpc::paths::Paths;
use lumos_rpc::{Client, Dispatch};
use serde_json::{Value, json};
use tokio::time::sleep;

struct Harness {
    _home: tempfile::TempDir,
    save: PathBuf,
    socket: PathBuf,
    engine: Arc<Engine>,
    server: tokio::task::JoinHandle<()>,
}

/// The socket sits under /tmp: macOS limits socket paths to 104 bytes.
async fn harness() -> Harness {
    let home = tempfile::Builder::new().prefix("lm").tempdir_in("/tmp").unwrap();
    let paths = Paths::under(home.path().to_path_buf());
    assert!(paths.socket.as_os_str().len() < lumos_rpc::MAX_SOCKET_PATH, "socket path too long: {}", paths.socket.display());
    let save = home.path().join("save");
    let engine = Engine::new(EngineOptions { paths: paths.clone(), host: "test".into(), backend: Some(Arc::new(FakeBackend)) }).unwrap();
    let listener = lumos_rpc::bind(&paths.socket).unwrap();
    let server = tokio::spawn(lumos_rpc::serve(listener, engine.clone() as Arc<dyn Dispatch>));
    Harness { socket: paths.socket.clone(), _home: home, save, engine, server }
}

async fn connect(h: &Harness, kind: &str) -> Client {
    let c = Client::connect(&h.socket).await.unwrap();
    c.call("session.hello", json!({ "client": { "kind": kind, "name": "test", "version": "0" }, "protocol": 1 })).await.unwrap();
    c
}

async fn set_save_folder(c: &Client, folder: &Path) {
    c.call("settings.set", json!({ "patch": { "storage.saveFolder": folder.to_str().unwrap(), "recording.minFreeDiskMB": 1 } })).await.unwrap();
}

async fn probe_streams(path: &Path) -> Vec<String> {
    let out = tokio::process::Command::new(std::env::var("LUMOS_FFPROBE").unwrap_or_else(|_| "ffprobe".into()))
        .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .await
        .unwrap();
    String::from_utf8_lossy(&out.stdout).lines().map(str::to_owned).collect()
}

#[tokio::test]
async fn record_pause_resume_stop_saves_an_mp4() {
    let h = harness().await;
    let c = connect(&h, "cli").await;
    set_save_folder(&c, &h.save).await;
    // This test checks the source track layout, which only the "always" setting keeps.
    c.call("settings.set", json!({ "patch": { "storage.keepSourceTracks": "always" } })).await.unwrap();

    c.call("events.subscribe", json!({ "topics": ["recorder.", "recording."] })).await.unwrap();
    let mut events = c.take_events().unwrap();

    let st = c.call("recording.start", json!({ "countdownSeconds": 0, "title": "Flow Test" })).await.unwrap();
    assert_eq!(st["state"], "recording");
    let id = st["recordingId"].as_str().unwrap().to_string();

    // Active recordings live under .lumos-inprogress until finalize.
    assert!(h.save.join(".lumos-inprogress").join(format!("{id}.lumos")).is_dir());

    sleep(Duration::from_millis(1200)).await;
    let paused = c.call("recording.pause", json!({})).await.unwrap();
    assert_eq!(paused["state"], "paused");
    let at_pause = paused["elapsedUs"].as_i64().unwrap();
    assert!((1_100_000..2_500_000).contains(&at_pause), "elapsed at pause: {at_pause}");

    // Paused time must not count.
    sleep(Duration::from_millis(1500)).await;
    let still = c.call("recording.status", json!({})).await.unwrap();
    assert_eq!(still["state"], "paused");
    assert_eq!(still["elapsedUs"].as_i64().unwrap(), at_pause);

    // Markers work in both active states.
    let mark = c.call("recording.mark", json!({ "label": "midpoint" })).await.unwrap();
    assert_eq!(mark["atUs"].as_i64().unwrap(), at_pause);

    let resumed = c.call("recording.resume", json!({})).await.unwrap();
    assert_eq!(resumed["state"], "recording");
    sleep(Duration::from_millis(1200)).await;

    let stop = c.call("recording.stop", json!({ "wait": true, "timeoutMs": 30000 })).await.unwrap();
    let output = PathBuf::from(stop["outputPath"].as_str().expect("wait returns the output path"));
    let bundle = PathBuf::from(stop["bundlePath"].as_str().unwrap());

    // The MP4 is in the save folder with a templated name; nothing is left in progress.
    assert_eq!(output.parent().unwrap(), h.save);
    assert!(output.file_name().unwrap().to_string_lossy().starts_with("Flow Test — "));
    assert_eq!(output.extension().unwrap(), "mp4");
    assert_eq!(std::fs::read_dir(h.save.join(".lumos-inprogress")).unwrap().count(), 0);

    // Duration ≈ recorded time (about 2.4 s), not wall time (about 3.9 s).
    let dur = lumos_media::probe_duration_us(&output).await.unwrap();
    assert!((1_800_000..3_400_000).contains(&dur), "mp4 duration {dur}us");
    let streams = probe_streams(&output).await;
    assert!(streams.contains(&"video".to_string()) && streams.contains(&"audio".to_string()), "{streams:?}");

    // The bundle has the manifest: two segments, track files, marker, ready.
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest.segments.len(), 2);
    assert_eq!(manifest.segments[1].start_us, manifest.segments[0].end_us);
    assert_eq!(manifest.markers.len(), 1);
    assert_eq!(manifest.markers[0].label.as_deref(), Some("midpoint"));
    assert_eq!(serde_json::to_value(manifest.status).unwrap(), "ready");
    assert_eq!(manifest.tracks.iter().find(|t| t.id == "trk_screen").unwrap().files.len(), 2);
    for t in &manifest.tracks {
        for f in &t.files {
            assert!(bundle.join(f).is_file(), "missing {f}");
        }
    }
    assert!(bundle.join("thumb.jpg").is_file());
    assert_eq!(manifest.outputs.len(), 1);

    // Recorder is idle again, and the finalize job is done.
    assert_eq!(c.call("recording.status", json!({})).await.unwrap()["state"], "idle");
    let job = c.call("jobs.get", json!({ "jobId": stop["finalizeJobId"] })).await.unwrap();
    assert_eq!(job["status"], "done");

    // Events arrived over the socket, in order.
    let mut seen = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(300), events.recv()).await {
        seen.push((ev.kind.clone(), ev.data["state"].as_str().unwrap_or("").to_string()));
    }
    let states: Vec<&str> = seen.iter().filter(|(k, _)| k == "recorder.stateChanged").map(|(_, s)| s.as_str()).collect();
    assert_eq!(states, ["preparing", "recording", "paused", "recording", "stopping", "idle"]);
    assert!(seen.iter().any(|(k, _)| k == "recording.ready"));

    h.server.abort();
}

/// By default only the MP4 keeps the video: the source tracks are dropped, and the small bundle
/// left behind keeps the recording in the library with its screen track pointing at the MP4.
#[tokio::test]
async fn mp4_only_drops_source_tracks_but_stays_in_the_library() {
    let h = harness().await;
    let c = connect(&h, "cli").await;
    set_save_folder(&c, &h.save).await;
    assert_eq!(c.call("settings.get", json!({ "path": "storage.keepSourceTracks" })).await.unwrap(), "never");

    let st = c.call("recording.start", json!({ "countdownSeconds": 0, "title": "Slim", "camera": { "enabled": true } })).await.unwrap();
    let id = st["recordingId"].as_str().unwrap().to_string();
    sleep(Duration::from_millis(1100)).await;
    let stop = c.call("recording.stop", json!({ "wait": true, "timeoutMs": 30000 })).await.unwrap();
    let output = PathBuf::from(stop["outputPath"].as_str().unwrap());
    let bundle = PathBuf::from(stop["bundlePath"].as_str().unwrap());

    assert!(output.is_file());
    assert!(!bundle.join("tracks").exists(), "source tracks should be gone");
    assert!(bundle.join("thumb.jpg").is_file());
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    let kinds: Vec<_> = manifest.tracks.iter().map(|t| t.kind).collect();
    assert_eq!(kinds, [lumos_model::manifest::TrackKind::Screen]);
    let stem = output.file_stem().unwrap().to_string_lossy();
    assert_eq!(manifest.tracks[0].files, [format!("../{stem}.mp4")]);

    let got = c.call("library.get", json!({ "id": id })).await.unwrap();
    assert_eq!(got["summary"]["outputPath"], output.display().to_string());
    h.server.abort();
}

/// The toolbar shows "Saving…" while the recorder lists a pending save. When the ready event
/// arrives, that list must already be empty, or "Saving…" would stay on screen.
#[tokio::test]
async fn ready_event_arrives_when_the_recorder_no_longer_lists_a_pending_save() {
    let h = harness().await;
    let c = connect(&h, "cli").await;
    set_save_folder(&c, &h.save).await;
    c.call("events.subscribe", json!({ "topics": ["recording."] })).await.unwrap();
    let mut events = c.take_events().unwrap();

    c.call("recording.start", json!({ "countdownSeconds": 0 })).await.unwrap();
    sleep(Duration::from_millis(600)).await;
    c.call("recording.stop", json!({})).await.unwrap();

    loop {
        let ev = tokio::time::timeout(Duration::from_secs(30), events.recv()).await.expect("ready event in time").expect("event stream open");
        if ev.kind == "recording.ready" {
            break;
        }
    }
    let after = c.call("recording.status", json!({})).await.unwrap();
    assert!(after["pendingFinalize"].as_array().unwrap().is_empty(), "still listed as saving after the ready event: {after}");
    h.server.abort();
}

#[tokio::test]
async fn stop_without_wait_returns_a_job_and_recorder_is_free_at_once() {
    let h = harness().await;
    let c = connect(&h, "cli").await;
    set_save_folder(&c, &h.save).await;

    c.call("recording.start", json!({ "countdownSeconds": 0 })).await.unwrap();
    sleep(Duration::from_millis(600)).await;
    let stop = c.call("recording.stop", json!({})).await.unwrap();
    assert!(stop["outputPath"].is_null());
    let job_id = stop["finalizeJobId"].as_str().unwrap().to_string();

    // The recorder is idle immediately; a second recording can start while the first saves.
    let again = c.call("recording.start", json!({ "countdownSeconds": 0 })).await.unwrap();
    assert_eq!(again["state"], "recording");
    let c2 = connect(&h, "ui").await;
    let cancelled = c2.call("recording.cancel", json!({ "confirm": true })).await.unwrap();
    assert_eq!(cancelled["discarded"], true);

    let job = c.call("jobs.wait", json!({ "jobId": job_id, "timeoutMs": 30000 })).await.unwrap();
    assert_eq!(job["status"], "done");
    assert!(PathBuf::from(job["result"]["outputPath"].as_str().unwrap()).is_file());
    h.engine.drain(Duration::from_secs(10)).await;
    h.server.abort();
}

#[tokio::test]
async fn errors_carry_stable_codes() {
    let h = harness().await;
    let c = connect(&h, "cli").await;
    set_save_folder(&c, &h.save).await;

    let err = c.call("recording.pause", json!({})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidState);
    let err = c.call("nope.nothing", json!({})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::MethodNotFound);
    let err = c.call("recording.cancel", json!({ "confirm": false })).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidState);
    let err = c.call("settings.set", json!({ "patch": { "capture.fps": 7 } })).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let err = c.call("jobs.get", json!({ "jobId": "job_missing" })).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = c.call("recording.start", json!({ "bogusField": 1 })).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);

    // A start that fails leaves the recorder idle and the in-progress folder clean.
    let err = c
        .call("recording.start", json!({ "countdownSeconds": 0, "source": { "kind": "display", "displayId": "nope" } }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(c.call("recording.status", json!({})).await.unwrap()["state"], "idle");
    assert_eq!(std::fs::read_dir(h.save.join(".lumos-inprogress")).map(|d| d.count()).unwrap_or(0), 0);

    // Calls before hello are refused.
    let raw = Client::connect(&h.socket).await.unwrap();
    assert_eq!(raw.call("system.info", json!({})).await.unwrap_err().code, ErrorCode::InvalidState);
    h.server.abort();
}

#[tokio::test]
async fn agents_are_gated_and_given_a_countdown() {
    let h = harness().await;
    let ui = connect(&h, "ui").await;
    set_save_folder(&ui, &h.save).await;
    let agent = connect(&h, "mcp").await;

    let err = agent.call("recording.start", json!({})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::PolicyDenied);
    // Agents can't open the gate themselves, only the app can.
    let err = agent.call("settings.set", json!({ "patch": { "automation.agentMayStartRecording": true } })).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::PolicyDenied);
    assert_eq!(agent.call("system.shutdown", json!({})).await.unwrap_err().code, ErrorCode::PolicyDenied);

    ui.call("settings.set", json!({ "patch": { "automation.agentMayStartRecording": true } })).await.unwrap();
    let st = agent.call("recording.start", json!({ "countdownSeconds": 0 })).await.unwrap();
    // Even with 0 requested, an agent gets the minimum countdown.
    assert_eq!(st["state"], "countdown");
    assert_eq!(st["startedBy"]["kind"], "mcp");
    agent.call("recording.cancel", json!({ "confirm": true })).await.unwrap();
    assert_eq!(agent.call("recording.status", json!({})).await.unwrap()["state"], "idle");
    h.server.abort();
}

#[tokio::test]
async fn max_duration_stops_and_saves_by_itself() {
    let h = harness().await;
    let c = connect(&h, "cli").await;
    set_save_folder(&c, &h.save).await;

    c.call("recording.start", json!({ "countdownSeconds": 0, "maxDurationSeconds": 1 })).await.unwrap();
    sleep(Duration::from_millis(1800)).await;
    let st = c.call("recording.status", json!({})).await.unwrap();
    assert_eq!(st["state"], "idle");
    // Either still saving or already saved; the file must exist in the end.
    h.engine.drain(Duration::from_secs(30)).await;
    let jobs: Value = c.call("jobs.list", json!({})).await.unwrap();
    assert_eq!(jobs[0]["status"], "done");
    let out = jobs[0]["result"]["outputPath"].as_str().unwrap();
    let dur = lumos_media::probe_duration_us(Path::new(out)).await.unwrap();
    assert!((800_000..1_800_000).contains(&dur), "{dur}");
    h.server.abort();
}
