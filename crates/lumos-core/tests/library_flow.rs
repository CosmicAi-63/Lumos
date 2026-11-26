//! Library, export, import and crash recovery, end to end over a real socket
//! with the fake capture backend. Needs `ffmpeg` and `ffprobe`.
//!
//! Callers: the engine's `Inner::call` dispatch for library.*, export.*, import.*.
//! Data: `lumos.recording/1` manifests inside `.lumos` bundles (unchanged schema).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lumos_capture::fake::FakeBackend;
use lumos_core::{Engine, EngineOptions};
use lumos_model::ErrorCode;
use lumos_rpc::paths::Paths;
use lumos_rpc::{Client, Dispatch};
use serde_json::{Value, json};

struct Harness {
    _home: tempfile::TempDir,
    save: PathBuf,
    trash: PathBuf,
    socket: PathBuf,
    server: tokio::task::JoinHandle<()>,
}

/// The socket sits under /tmp: macOS limits socket paths to 104 bytes.
async fn harness() -> Harness {
    let home = tempfile::Builder::new().prefix("ll").tempdir_in("/tmp").unwrap();
    let paths = Paths::under(home.path().to_path_buf());
    let engine = Engine::new(EngineOptions { paths: paths.clone(), host: "test".into(), backend: Some(Arc::new(FakeBackend)) }).unwrap();
    let listener = lumos_rpc::bind(&paths.socket).unwrap();
    let server = tokio::spawn(lumos_rpc::serve(listener, engine as Arc<dyn Dispatch>));
    Harness { socket: paths.socket.clone(), trash: paths.trash_dir.clone(), save: home.path().join("save"), _home: home, server }
}

async fn connect(h: &Harness) -> Client {
    let c = Client::connect(&h.socket).await.unwrap();
    c.call("session.hello", json!({ "client": { "kind": "cli", "name": "test", "version": "0" }, "protocol": 1 })).await.unwrap();
    c.call("settings.set", json!({ "patch": { "storage.saveFolder": h.save.to_str().unwrap(), "recording.minFreeDiskMB": 1 } })).await.unwrap();
    c
}

/// Record `title` for about a second and return the recording id.
async fn record(c: &Client, title: &str) -> String {
    let st = c.call("recording.start", json!({ "countdownSeconds": 0, "title": title })).await.unwrap();
    let id = st["recordingId"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    c.call("recording.stop", json!({ "wait": true, "timeoutMs": 30000 })).await.unwrap();
    id
}

#[tokio::test]
async fn list_get_update_and_delete() {
    let h = harness().await;
    let c = connect(&h).await;
    let a = record(&c, "Alpha").await;
    let b = record(&c, "Bravo").await;

    let list = c.call("library.list", json!({})).await.unwrap();
    assert_eq!(list["total"], 2);
    assert_eq!(list["items"][0]["id"], b.as_str(), "newest first");
    assert_eq!(list["items"][0]["origin"], "recorded");
    assert!(Path::new(list["items"][0]["outputPath"].as_str().unwrap()).exists());

    let q = c.call("library.list", json!({ "query": "alp" })).await.unwrap();
    assert_eq!(q["total"], 1);
    assert_eq!(c.call("library.list", json!({ "sort": "title", "limit": 1 })).await.unwrap()["items"][0]["title"], "Alpha");
    assert_eq!(c.call("library.list", json!({ "sort": "bogus" })).await.unwrap_err().code, ErrorCode::InvalidParams);

    // `last`, exact id and a unique prefix all resolve; unknown ids are NOT_FOUND.
    assert_eq!(c.call("library.get", json!({ "id": "last" })).await.unwrap()["summary"]["id"], b.as_str());
    assert_eq!(c.call("library.get", json!({ "id": a })).await.unwrap()["manifest"]["title"], "Alpha");
    assert_eq!(c.call("library.get", json!({ "id": "rec_nope" })).await.unwrap_err().code, ErrorCode::NotFound);

    // Renaming moves the MP4 and the bundle and keeps the manifest in step.
    let old_out = PathBuf::from(c.call("library.get", json!({ "id": a })).await.unwrap()["summary"]["outputPath"].as_str().unwrap());
    let up = c.call("library.update", json!({ "id": a, "title": "Renamed: take 2", "tags": ["demo", " demo ", "x"] })).await.unwrap();
    let new_out = PathBuf::from(up["outputPath"].as_str().unwrap());
    assert!(!old_out.exists() && new_out.exists());
    assert_eq!(new_out.file_name().unwrap(), "Renamed- take 2.mp4");
    assert_eq!(up["tags"], json!(["demo", "x"]));
    assert!(Path::new(up["bundlePath"].as_str().unwrap()).ends_with("Renamed- take 2.lumos"));
    let tagged = c.call("library.list", json!({ "tags": ["demo"] })).await.unwrap();
    assert_eq!(tagged["total"], 1);
    assert_eq!(c.call("library.update", json!({ "id": a, "title": "  " })).await.unwrap_err().code, ErrorCode::InvalidParams);

    // Delete needs confirm and goes to the Trash, never a hard delete.
    assert_eq!(c.call("library.delete", json!({ "id": a })).await.unwrap_err().code, ErrorCode::InvalidParams);
    assert!(new_out.exists());
    let del = c.call("library.delete", json!({ "id": a, "confirm": true })).await.unwrap();
    assert_eq!(del["trashed"].as_array().unwrap().len(), 2);
    assert!(!new_out.exists());
    assert!(h.trash.join("Renamed- take 2.mp4").exists());
    assert_eq!(c.call("library.list", json!({})).await.unwrap()["total"], 1);
    h.server.abort();
}

#[tokio::test]
async fn export_presets_make_playable_files_and_get_recorded() {
    let h = harness().await;
    let c = connect(&h).await;
    let id = record(&c, "Exportable").await;

    let presets = c.call("export.presets", json!({})).await.unwrap();
    let ids: Vec<&str> = presets.as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"mp4-720p") && ids.contains(&"gif") && ids.contains(&"audio"));
    assert_eq!(c.call("export.create", json!({ "id": id, "preset": "nope" })).await.unwrap_err().code, ErrorCode::InvalidParams);

    let small = c.call("export.create", json!({ "id": id, "preset": "mp4-720p", "wait": true, "timeoutMs": 60000 })).await.unwrap();
    let path = PathBuf::from(small["path"].as_str().expect("wait returns the path"));
    let name = path.file_name().unwrap().to_string_lossy();
    assert!(name.starts_with("Exportable") && name.ends_with(" (mp4-720p).mp4"), "{name}");
    let dur = lumos_media::probe_duration_us(&path).await.unwrap();
    assert!((800_000..2_500_000).contains(&dur), "{dur}");
    assert!(small["bytes"].as_u64().unwrap() > 0);

    let gif = c.call("export.create", json!({ "id": id, "preset": "gif", "wait": true, "timeoutMs": 60000 })).await.unwrap();
    assert!(Path::new(gif["path"].as_str().unwrap()).extension().unwrap() == "gif");
    let audio = c.call("export.create", json!({ "id": id, "preset": "audio", "wait": true, "timeoutMs": 60000 })).await.unwrap();
    let streams = lumos_media::probe_streams(Path::new(audio["path"].as_str().unwrap())).await.unwrap();
    assert_eq!(streams, vec!["audio"]);

    // Without wait the call returns a job; an existing target is refused.
    let job = c.call("export.create", json!({ "id": id, "preset": "mp4-1080p" })).await.unwrap();
    assert!(job["jobId"].as_str().unwrap().starts_with("job_"));
    assert!(job.get("path").is_none());
    let done = c.call("jobs.wait", json!({ "jobId": job["jobId"], "timeoutMs": 60000 })).await.unwrap();
    assert_eq!(done["status"], "done");
    assert_eq!(c.call("export.create", json!({ "id": id, "preset": "gif", "outputPath": gif["path"] })).await.unwrap_err().code, ErrorCode::InvalidParams);

    // Exports are listed in the manifest next to the original.
    let outputs = &c.call("library.get", json!({ "id": id })).await.unwrap()["manifest"]["outputs"];
    assert_eq!(outputs.as_array().unwrap().len(), 5);
    h.server.abort();
}

#[tokio::test]
async fn import_brings_files_into_the_library() {
    let h = harness().await;
    let c = connect(&h).await;
    let src = tempfile::Builder::new().prefix("ls").tempdir_in("/tmp").unwrap();
    let video = src.path().join("Screen Capture.mp4");
    lumos_media::generate_test_video(&video, 1_500_000, 320, 180, 15).await.unwrap();
    let text = src.path().join("notes.txt");
    std::fs::write(&text, "hello").unwrap();
    let silent = src.path().join("tone.mp4");
    lumos_media::generate_test_tone(&silent, 500_000, 440, 1).await.unwrap();

    let formats = c.call("import.formats", json!({})).await.unwrap();
    assert!(formats[0]["extensions"].as_array().unwrap().iter().any(|e| e == "webm"));

    let r = c
        .call("import.create", json!({ "paths": [video, text, silent, src.path().join("missing.mp4")], "tags": ["imported-test"], "wait": true, "timeoutMs": 60000 }))
        .await
        .unwrap();
    let recs = r["recordings"].as_array().unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["origin"], "imported");
    assert_eq!(recs[0]["title"], "Screen Capture");
    assert_eq!(recs[0]["tags"], json!(["imported-test"]));
    let skipped = r["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 3, "{skipped:?}");
    let reasons = skipped.iter().map(|s| s.as_str().unwrap()).collect::<Vec<_>>().join("|");
    assert!(reasons.contains("unsupported file type") && reasons.contains("no video stream") && reasons.contains("file not found"), "{reasons}");

    // The original stays put; the library holds a copy next to a bundle with a thumbnail.
    assert!(video.exists());
    let out = Path::new(recs[0]["outputPath"].as_str().unwrap());
    assert_eq!(out.parent().unwrap(), h.save);
    let dur = recs[0]["durationUs"].as_i64().unwrap();
    assert!((1_300_000..1_800_000).contains(&dur), "{dur}");
    assert!(recs[0]["thumbPath"].is_string());
    assert_eq!(c.call("library.list", json!({ "origin": "imported" })).await.unwrap()["total"], 1);
    assert_eq!(c.call("library.list", json!({ "origin": "recorded" })).await.unwrap()["total"], 0);

    // Importing the same name again gets a new name instead of overwriting.
    let again = c.call("import.create", json!({ "paths": [video], "wait": true, "timeoutMs": 60000 })).await.unwrap();
    assert!(again["recordings"][0]["outputPath"].as_str().unwrap().ends_with("Screen Capture 2.mp4"));

    // Nothing importable is an error, not a silent success.
    let e = c.call("import.create", json!({ "paths": [text], "wait": true, "timeoutMs": 60000 })).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    h.server.abort();
}

#[tokio::test]
async fn recover_saves_what_a_crash_left_behind() {
    let h = harness().await;
    let c = connect(&h).await;

    // Simulate a crash: a recording that stopped after one segment closed, never finalized.
    // Its in-progress bundle still holds the source tracks, so keep them for this setup.
    c.call("settings.set", json!({ "patch": { "storage.keepSourceTracks": "always" } })).await.unwrap();
    let id = record(&c, "Crashed").await;
    let rec = c.call("library.get", json!({ "id": id })).await.unwrap();
    let bundle = PathBuf::from(rec["summary"]["bundlePath"].as_str().unwrap());
    let output = PathBuf::from(rec["summary"]["outputPath"].as_str().unwrap());
    let progress = h.save.join(".lumos-inprogress");
    std::fs::create_dir_all(&progress).unwrap();
    let stuck = progress.join(format!("{id}.lumos"));
    std::fs::rename(&bundle, &stuck).unwrap();
    std::fs::remove_file(&output).unwrap();

    // Rewind the manifest to what the engine would have written before the crash.
    let mut m: Value = serde_json::from_slice(&std::fs::read(stuck.join("manifest.json")).unwrap()).unwrap();
    m["status"] = json!("recording");
    m["outputs"] = json!([]);
    std::fs::write(stuck.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    assert_eq!(c.call("library.list", json!({})).await.unwrap()["total"], 0);
    let health = c.call("system.health", json!({})).await.unwrap();
    assert!(health["problems"].as_array().unwrap().iter().any(|p| p["code"] == "RECOVERY_PENDING"));

    // A bundle with no manifest is reported, not fatal.
    std::fs::create_dir_all(progress.join("rec_empty.lumos")).unwrap();
    let r = c.call("library.recover", json!({ "wait": true, "timeoutMs": 60000 })).await.unwrap();
    let recovered = r["recovered"].as_array().unwrap();
    assert_eq!(recovered.len(), 1, "{r}");
    assert_eq!(recovered[0]["status"], "recovered");
    assert!(Path::new(recovered[0]["outputPath"].as_str().unwrap()).exists());
    let skipped = r["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].as_str().unwrap().contains("no manifest.json"));

    let list = c.call("library.list", json!({})).await.unwrap();
    assert_eq!(list["total"], 1);
    let warnings = &c.call("library.get", json!({ "id": id })).await.unwrap()["manifest"]["warnings"];
    assert!(warnings.as_array().unwrap().iter().any(|w| w["code"] == "RECOVERED"));
    assert!(!stuck.exists());

    // Asking for something that isn't there is an error.
    assert_eq!(c.call("library.recover", json!({ "id": "rec_zzz", "wait": true })).await.unwrap_err().code, ErrorCode::NotFound);
    h.server.abort();
}

#[tokio::test]
async fn export_range_keeps_only_that_part_and_rejects_bad_ranges() {
    let h = harness().await;
    let c = connect(&h).await;
    let id = record(&c, "Ranged").await;

    let part = c
        .call("export.create", json!({ "id": id, "preset": "mp4-720p", "range": { "startSeconds": 0.2, "endSeconds": 0.7 }, "wait": true, "timeoutMs": 60000 }))
        .await
        .unwrap();
    let dur = lumos_media::probe_duration_us(Path::new(part["path"].as_str().unwrap())).await.unwrap();
    assert!((350_000..700_000).contains(&dur), "range 0.2-0.7 should give about 0.5s, got {dur}us");

    for bad in [json!({ "startSeconds": 5.0 }), json!({ "startSeconds": 0.5, "endSeconds": 0.5 }), json!({ "startSeconds": -1.0 })] {
        let err = c.call("export.create", json!({ "id": id, "preset": "mp4-720p", "range": bad })).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams, "{bad}");
    }
    // The original is untouched.
    let original = c.call("library.get", json!({ "id": id })).await.unwrap();
    let full = original["manifest"]["durationUs"].as_i64().unwrap();
    assert!(full > 800_000, "{full}");
    h.server.abort();
}

#[tokio::test]
async fn search_matches_transcript_text_and_flags_has_transcript() {
    let h = harness().await;
    let c = connect(&h).await;
    let id = record(&c, "Standup").await;
    let _other = record(&c, "Planning").await;

    let list = c.call("library.list", json!({})).await.unwrap();
    assert!(list["items"].as_array().unwrap().iter().all(|r| r["hasTranscript"] == false));
    let bundle = list["items"].as_array().unwrap().iter().find(|r| r["id"] == id.as_str()).unwrap()["bundlePath"].as_str().unwrap().to_string();

    // A transcript is just text files in captions/; transcript.create writes them with whisper.
    let dir = Path::new(&bundle).join("captions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("transcript.txt"), "We should migrate the Kubernetes cluster on Friday.\n").unwrap();

    let hit = c.call("library.list", json!({ "query": "kubernetes" })).await.unwrap();
    let items = hit["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{hit}");
    assert_eq!(items[0]["id"], id.as_str());
    assert_eq!(items[0]["hasTranscript"], true);
    assert_eq!(c.call("library.list", json!({ "query": "no such words" })).await.unwrap()["items"].as_array().unwrap().len(), 0);
    let got = c.call("transcript.get", json!({ "id": id })).await.unwrap();
    assert!(got["text"].as_str().unwrap().contains("Kubernetes"));
    h.server.abort();
}

#[tokio::test]
async fn chapters_come_from_markers_and_transcript_pauses() {
    let h = harness().await;
    let c = connect(&h).await;
    let st = c.call("recording.start", json!({ "countdownSeconds": 0, "title": "Chaptered" })).await.unwrap();
    let id = st["recordingId"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    c.call("recording.mark", json!({ "label": "Demo begins" })).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    c.call("recording.stop", json!({ "wait": true, "timeoutMs": 30000 })).await.unwrap();

    let list = c.call("library.list", json!({})).await.unwrap();
    let bundle = list["items"][0]["bundlePath"].as_str().unwrap().to_string();
    let dir = Path::new(&bundle).join("captions");
    std::fs::create_dir_all(&dir).unwrap();
    // Cue 2 starts 20 s after cue 1 ends: a long pause, far from the marker.
    std::fs::write(
        dir.join("transcript.srt"),
        "1\n00:00:00,000 --> 00:00:03,000\n Welcome everyone.\n\n2\n00:00:23,000 --> 00:00:27,000\n Now let us talk about pricing and plans today.\n\n",
    )
    .unwrap();

    let r = c.call("chapters.suggest", json!({ "id": id })).await.unwrap();
    let ch = r["chapters"].as_array().unwrap();
    let sources: Vec<&str> = ch.iter().map(|x| x["source"].as_str().unwrap()).collect();
    assert_eq!(sources, ["start", "marker", "transcript"], "{r}");
    assert_eq!(ch[1]["title"], "Demo begins");
    assert!((0.3..1.0).contains(&ch[1]["startSeconds"].as_f64().unwrap()), "{r}");
    assert_eq!(ch[2]["title"], "Now let us talk about pricing");
    assert_eq!(ch[2]["startSeconds"].as_f64().unwrap(), 23.0);
    h.server.abort();
}
