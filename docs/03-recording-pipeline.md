# 3. Recording pipeline

How one recording moves from `recording.start` to an MP4 in the save folder. The macOS backend is described concretely. Windows and Linux implement the same traits.

## Overview

```
 sources (per track)                 writers (per track, per segment)        finalize job
 ┌───────────────────────┐          ┌─────────────────────────────┐        ┌─────────────────────────────┐
 │ SCStream  (screen)    │──frames─▶│ TrackWriter: H.264 HW, fMP4 │──┐     │ 1 concat segments (copy)    │
 │ SCStream  (system aud)│──pcm────▶│ TrackWriter: AAC            │──┤     │ 2 mix mic+system → AAC      │
 │ AVCapture (camera)    │──frames─▶│ TrackWriter: H.264 HW, fMP4 │──┼────▶│ 3 mux screen + mix → MP4    │
 │ AVCapture (mic)       │──pcm────▶│ TrackWriter: AAC            │──┘     │ 4 thumbnail, manifest       │
 └──────────┬────────────┘          └─────────────────────────────┘        │ 5 atomic rename into folder │
            │ camera preview (same session)                                 │ 6 index + recording.ready   │
            ▼                                                               └─────────────────────────────┘
   Camera bubble window (native, captured by the screen stream)
```

## Traits

```rust
// As built in M0 (crates/lumos-capture). The boundary is a *session* with
// segment-level calls: the native backend runs its streams and its writer together.
#[async_trait]
pub trait CaptureBackend: Send + Sync {
    async fn permissions(&self) -> PermissionStatus;
    async fn displays(&self) -> ToolResult<Vec<Display>>;
    async fn cameras(&self) -> ToolResult<Vec<Device>>;
    async fn microphones(&self) -> ToolResult<Vec<Device>>;
    /// Open all streams; they warm up during the countdown. Nothing is written yet.
    async fn open(&self, config: SessionConfig) -> ToolResult<Box<dyn CaptureSession>>;
}

#[async_trait]
pub trait CaptureSession: Send {
    fn tracks(&self) -> Vec<TrackInfo>;
    async fn begin_segment(&mut self, index: u32) -> ToolResult<()>;
    async fn end_segment(&mut self) -> ToolResult<SegmentOutput>; // every file complete and playable
    async fn close(self: Box<Self>) -> ToolResult<()>;
}
```

The frame-level design (`VideoStream`, `AudioStream`, `TrackWriter`) now lives *inside* each backend, for example the Swift shim around `SCStream` and `AVAssetWriter`. Rust only drives the lifecycle. The fake backend writes synthetic media (test pattern and a sine tone) with the `ffmpeg` command-line tool when a segment ends. M0 finalize also calls `ffmpeg` (concat demuxer, stream copy) instead of linking the libraries; `lumos-media` hides that.

Planned for the macOS backend: `ScreenOpts` holds `fps`, `max_resolution`, `show_cursor` and **`exclude_windows`**, the IDs of the toolbar, the source picker and any other Lumos control windows. ScreenCaptureKit's content filter excludes those windows. The camera bubble is deliberately *not* excluded.

## Phases

### 1. Preflight (`preparing`)

1. Check permissions. If any are missing, fail with `PERMISSION_DENIED { permission }` and the UI opens onboarding.
2. Check that the save folder exists and is writable, and that free space ≥ `recording.minFreeDiskMB`. Otherwise fail with `DISK_LOW`.
3. Create `.lumos-inprogress/<rec_id>.lumos/` and write an initial manifest with `status: "recording"`.
4. Open all streams: screen, camera, mic, system audio. They warm up **during the countdown**, so the first frame is never black and the mic never clips at the start.

### 2. Countdown (`countdown`)

The UI shows 3-2-1. Samples arriving now are dropped. Esc cancels, and clicking the countdown skips it.

### 3. Recording (`recording`)

- **Clock:** a single monotonic host clock (`mach_absolute_time` on macOS, QPC on Windows, `CLOCK_MONOTONIC` on Linux). `t0` is the first host time after the countdown. Every sample's PTS is rebased as `pts - t0 - pausedTotal`.
- **Writers:** each track writes a **fragmented MP4** with a 1 s fragment interval (audio goes in `.m4a`). If the process dies, the file is playable up to the last fragment.
- **Encoders:** screen uses H.264 through VideoToolbox, VBR at about 6 Mbps for 1080p30 and scaled by pixel count. Camera uses H.264 at about 2.5 Mbps for 720p. Audio is AAC-LC at 48 kHz (mono 96 kbps for mic, stereo 160 kbps for system).
- **Camera skew:** the camera's first frame arrives later than the screen's. The writer records `offsetUs` instead of padding.
- **Health:** a 1 Hz tick checks free disk space, dropped frames and the bytes written so far, then emits `recorder.tick`.

### 4. Pause and resume (`paused`)

- **Pause:** call `end_segment()` on every writer, which closes the file cleanly. The streams **stay open**, so resume is instant and the bubble preview keeps running. Samples are dropped while paused. The segment `endUs` is written to the manifest.
- **Resume:** call `begin_segment()` with the next index (`001.mp4`) and add the paused duration to `pausedTotal`. The timeline continues where it left off.
- A **segment per pause** keeps every file valid on its own and avoids PTS gaps. Finalize joins the segments with a stream copy, without re-encoding.

### 5. Stop (`stopping` → finalize job)

`recording.stop` ends the segments, closes the streams, enqueues a **finalize job** and sets the recorder back to `idle`. The finalize job then runs these steps:

1. **Concat** each track's segments with FFmpeg's concat demuxer as a stream copy. This takes seconds, not minutes.
2. **Mix audio:** mic + system audio, using `micGainDb`/`systemGainDb` and a limiter, encoded to AAC. Only the audio is re-encoded.
3. **Mux** the screen video and mixed audio into the output MP4 with `+faststart` so it starts playing quickly on the web. Markers are written as MP4 chapters.
4. **Thumbnail** from the frame at 1 s, or the middle frame for clips shorter than 2 s.
5. **Finish the manifest:** `status: "ready"`, `durationUs`, `outputs[]`.
6. **Atomic rename** of the bundle and the MP4 into `saveFolder`, using `fileNameTemplate` and resolving name collisions.
7. **Index upsert**, then emit `recording.ready { recordingId, outputPath, bundlePath, durationUs }`.

The UI reacts to `recording.ready`. Depending on `output.afterStop` it shows a toast, reveals the file in Finder or opens the library. The engine itself never shows UI.

**Target:** a 10-minute recording is ready in under 3 s after stop on Apple silicon.

Because the recorder is already `idle`, a new `recording.start` can begin while the previous recording is still finalizing. `RecorderStatus.pendingFinalize` lists those jobs, and the toolbar keeps showing "Saving…" for them.

### 6. Cancel and restart

- **Cancel:** end the segments, close the streams and delete the in-progress bundle. The UI asks for confirmation first, and so does the CLI unless `--yes` is passed. The bundle goes to the OS Trash, not a hard delete, so mistakes can be undone.
- **Restart:** cancel, then `start` again with the same resolved config. One click.

## Camera bubble

**Decision: burn-in by default, raw track always kept.**

- The bubble is a borderless, always-on-top native window (`NSPanel`) owned by the UI layer and rendered from the engine's camera session, using an `AVSampleBufferDisplayLayer` with a shape mask.
- The screen stream captures it like any other window, so **what you see is what you get**. Stop needs no compositing step, which keeps "stop → file in folder" near-instant.
- The camera is also written raw to `tracks/camera/`, and every move, resize, shape change or hide is logged as a keyframe in `layout.camera.keyframes`. That enables later **re-composite exports**: record with `camera.burnIn: false` (bubble excluded from capture), then choose the layout at export time, such as a different corner, camera-only or side by side. Re-composite needs a full re-encode, so it's an export preset, not the default (M4).
- **Window source (M1):** when recording a single window, the bubble isn't part of that window. In that mode the engine automatically switches to `burnIn: false`, and the finalize job composites the camera using the layout keyframes: an FFmpeg `overlay` with a shape mask, positioned relative to the window. That one case pays the re-encode cost at stop, about real-time ÷ 4 on Apple silicon (to be measured). The picker warns "Saving takes longer for window recordings with camera".
- **Area source:** while an area is being recorded, the bubble's drag bounds are clamped to inside the area rectangle, so the bubble is always in the video. If the area is too small for the smallest bubble, the engine treats the recording like a window source and composites the camera at stop.
- Mirroring applies to the preview and, if `camera.mirror` is on, to the burned-in pixels. The raw track is never mirrored.

## Failure handling

| Situation | Behavior |
|---|---|
| Mic unplugged | Keep recording, write silence, emit `recorder.warning MIC_DISCONNECTED`, and switch to the system default mic if one appears. Add a manifest warning. |
| Camera unplugged | Hide the bubble, stop the camera track segment, emit a warning, and continue. |
| Recorded window closes | Auto-pause and emit `WINDOW_CLOSED`. The user resumes on another source or stops. |
| Display disconnected | Auto-pause and emit a warning. On resume, the user picks a new source. |
| Disk below `minFreeDiskMB` | Emit `DISK_LOW` at 2× the threshold, then auto-stop and finalize at 1×. |
| Encoder error | Try once to restart the writer in a new segment. If that fails too, stop, finalize what exists and mark the recording `failed` (recoverable). |
| Crash or power loss | On the next launch, the engine scans `.lumos-inprogress/`, trims each fMP4 to its last complete fragment, runs finalize with `status: "recovered"` and emits `recording.ready`. The UI shows "Recovered recording from 14:03". |
| Screen permission revoked mid-recording | Stop and finalize what was captured, then emit `permissions.changed`. |
| `maxDurationMinutes` reached | Stop normally. |

## Performance budget (Apple silicon, 1080p30 screen + 720p camera + mic)

| Metric | Target |
|---|---|
| Engine CPU while recording | < 15% of one performance core |
| Dropped frames over 30 min | 0 |
| Click → first recorded frame (after countdown) | < 200 ms |
| A/V drift over 60 min | < 1 frame (33 ms) |
| Stop → `recording.ready` (10 min, burn-in) | < 3 s |
| Disk usage at defaults | roughly 3 GB/hour (screen + camera + audio, bundle and MP4 together). Tune in M0. |

## Platform notes

| | macOS | Windows | Linux |
|---|---|---|---|
| Screen | ScreenCaptureKit `SCStream` + `SCContentFilter` (exclude our windows) | Windows.Graphics.Capture (`SetExcludedFromCapture` via `WDA_EXCLUDEFROMCAPTURE`) | Portal ScreenCast → PipeWire (window exclusion not available; the toolbar auto-hides or moves to another display) |
| System audio | ScreenCaptureKit audio (macOS 13+; verify in M0) | WASAPI loopback | PipeWire monitor source |
| Camera / mic | AVFoundation | Media Foundation | PipeWire / V4L2 |
| HW encode | VideoToolbox via AVAssetWriter | Media Foundation (NVENC/QSV/AMF) | VAAPI via GStreamer |
| Permissions | TCC: Screen Recording, Camera, Microphone. Screen Recording may need an app relaunch after granting. Newer macOS versions re-confirm screen access periodically (verify the cadence). | Privacy settings for camera/mic | Portal dialog on each session (restore tokens remember the choice) |
