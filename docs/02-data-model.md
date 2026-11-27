# 2. Data model

All types live in `lumos-model`, a crate with no I/O. Rust is the source of truth, and the JSON Schema and TypeScript are generated from it. JSON is `camelCase`.

## Conventions

| Thing | Format | Example |
|---|---|---|
| IDs | Prefixed ULID (sortable by creation time) | `rec_01JDX8Q3M7K2`, `trk_…`, `seg_…`, `job_…`, `mrk_…` |
| Wall-clock time | RFC 3339, UTC | `"2026-11-25T22:03:11.204Z"` |
| Media time | `i64` microseconds on the **recording timeline** (paused time excluded), field suffix `Us` | `"durationUs": 184233000` |
| Normalized positions | `0.0–1.0` of the output frame, origin top-left | `{ "x": 0.04, "y": 0.92 }` |
| Paths in manifests | Relative to the bundle directory | `"tracks/screen/000.mp4"` |
| Schema versions | `"schema": "lumos.<kind>/<major>"` | `"lumos.recording/1"` |

The CLI and MCP accept ID prefixes when unambiguous (`rec_01JDX`) and the alias `last` (the most recent recording).

## On-disk layout

The user picks `storage.saveFolder` in Settings. A finished recording is two siblings: the MP4 people share, and the project bundle that holds the source tracks.

```
~/Movies/Lumos/                                   ← storage.saveFolder
├── Product demo — 2026-11-25 14.03.mp4           ← output (what users open and share)
├── Product demo — 2026-11-25 14.03.lumos/        ← bundle (macOS shows it as one item via a UTI)
│   ├── manifest.json                             ← source of truth for this recording
│   ├── tracks/
│   │   ├── screen/000.mp4  001.mp4               ← one file per segment (pause = new segment)
│   │   ├── camera/000.mp4  001.mp4
│   │   ├── mic/000.m4a     001.m4a
│   │   └── system/000.m4a  001.m4a
│   ├── thumb.jpg
│   ├── events.jsonl                              ← cursor/click/key-marker stream (M4+)
│   └── captions/en.vtt                           ← transcript (M4)
└── .lumos-inprogress/                            ← active and crashed recordings
    └── rec_01JDX8Q3M7K2.lumos/
```

- **In progress:** files are written under `.lumos-inprogress/` *inside the save folder*, which keeps them on the same volume. On finalize, both the bundle and the MP4 are moved into place with an atomic `rename`.
- **Name collisions:** append ` (2)`, ` (3)`, and so on.
- **`storage.keepSourceTracks`:** `"always"` (default), `"never"` (delete the bundle after export and keep only the MP4), or `"days:30"` (prune bundles older than N days, never the MP4).
- **`storage.subfolders`:** `"none"` (default) or `"yearMonth"` (`2026/11/…`).
- **Changing the save folder** affects new recordings only. Old recordings stay where they are, and the library keeps every folder it has seen in `library.roots`.

## Recording manifest (`manifest.json`)

```jsonc
{
  "schema": "lumos.recording/1",
  "id": "rec_01JDX8Q3M7K2",
  "title": "Product demo",
  "createdAt": "2026-11-25T22:03:11.204Z",
  "status": "ready",                       // recording | paused | finalizing | ready | failed | recovered
  "durationUs": 184233000,
  "origin": { "kind": "recorded" },        // or { "kind": "imported", "from": "/…/demo.mov", "importer": "video" }
  "app": { "version": "0.1.0", "platform": "macos-15.1-arm64" },

  "capture": {
    "source": {
      "kind": "display",                   // display | window | area
      "displayId": "69733382",
      "name": "Built-in Retina Display",
      "bounds": { "x": 0, "y": 0, "w": 1512, "h": 982 },  // points
      "scale": 2.0
    },
    "fps": 30,
    "showCursor": true,
    "cameraBurnedIn": true                 // the bubble window is part of the screen track
  },

  "segments": [                            // one per uninterrupted run; pauses split segments
    { "id": "seg_0", "startUs": 0,        "endUs": 61000000,  "wallStart": "2026-11-25T22:03:14.210Z" },
    { "id": "seg_1", "startUs": 61000000, "endUs": 184233000, "wallStart": "2026-11-25T22:05:02.877Z" }
  ],

  "tracks": [
    { "id": "trk_screen", "kind": "screen", "codec": "h264", "width": 3024, "height": 1964, "fps": 30,
      "files": ["tracks/screen/000.mp4", "tracks/screen/001.mp4"] },
    { "id": "trk_camera", "kind": "camera", "codec": "h264", "width": 1280, "height": 720, "fps": 30,
      "device": { "id": "0x8020000005ac8514", "name": "FaceTime HD Camera" },
      "offsetUs": 12000,                   // measured start skew vs. timeline zero
      "files": ["tracks/camera/000.mp4", "tracks/camera/001.mp4"] },
    { "id": "trk_mic", "kind": "mic", "codec": "aac", "sampleRate": 48000, "channels": 1,
      "device": { "id": "BuiltInMicrophoneDevice", "name": "MacBook Pro Microphone" },
      "files": ["tracks/mic/000.m4a", "tracks/mic/001.m4a"] },
    { "id": "trk_system", "kind": "systemAudio", "codec": "aac", "sampleRate": 48000, "channels": 2,
      "files": ["tracks/system/000.m4a", "tracks/system/001.m4a"] }
  ],

  "layout": {                              // camera placement over time; used by re-composite exports
    "camera": {
      "keyframes": [
        { "atUs": 0, "visible": true, "shape": "circle", "size": "m", "x": 0.04, "y": 0.78 },
        { "atUs": 95000000, "visible": false }
      ]
    }
  },

  "markers": [ { "id": "mrk_01JDX9…", "atUs": 30500000, "label": "Intro done" } ],

  "edits": {                               // non-destructive; applied at export
    "trim": { "startUs": 1200000, "endUs": 182000000 },
    "cuts": [ { "startUs": 70000000, "endUs": 74500000 } ]
  },

  "outputs": [
    { "kind": "export", "preset": "default", "path": "../Product demo — 2026-11-25 14.03.mp4",
      "createdAt": "2026-11-25T22:06:20.010Z", "bytes": 48211392, "editsHash": "b3:9f1c…" }
  ],

  "tags": [],
  "warnings": [ { "atUs": 120000000, "code": "MIC_DISCONNECTED", "message": "Mic lost for 2.1 s" } ]
}
```

### Rust sketch

```rust
pub enum CaptureSource {
    Display { display_id: String },
    Window  { window_id: u64, app_bundle_id: Option<String>, title: Option<String> },
    Area    { display_id: String, rect: Rect },          // rect in display points
}

pub enum TrackKind { Screen, Camera, Mic, SystemAudio }
pub enum RecordingStatus { Recording, Paused, Finalizing, Ready, Failed, Recovered }

pub struct Track {
    pub id: TrackId, pub kind: TrackKind, pub codec: Codec,
    pub video: Option<VideoParams>, pub audio: Option<AudioParams>,
    pub device: Option<DeviceRef>, pub offset_us: i64, pub files: Vec<RelPath>,
}
```

### Versioning and migration

- `schema` carries a major version. Readers migrate older majors in memory (`v1 → v2 → …`) and write the current major only when the user edits the recording.
- Unknown fields are kept on round-trip, so newer apps don't destroy data written by older ones and the reverse.
- If a manifest's major version is newer than the app supports, the recording opens read-only with a warning.

## Settings (`settings.json`)

Stored at `~/Library/Application Support/Lumos/settings.json` (Linux: `$XDG_CONFIG_HOME/lumos/`, Windows: `%APPDATA%\Lumos\`). Writes are validated against the schema and atomic (temp file + rename). A change emits `settings.changed`.

```jsonc
{
  "schema": "lumos.settings/1",
  "storage": {
    "saveFolder": "~/Movies/Lumos",
    "fileNameTemplate": "{title} — {date:%Y-%m-%d %H.%M}",
    "defaultTitle": "Recording",
    "keepSourceTracks": "always",          // always | never | "days:N"
    "subfolders": "none"                   // none | yearMonth
  },
  "capture": {
    "defaultSource": { "kind": "display", "displayId": "main" },
    "fps": 30,                             // 24 | 30 | 60
    "maxResolution": "native",             // native | 1080p | 1440p | 4k
    "showCursor": true,
    "highlightClicks": false
  },
  "camera": {
    "enabled": true, "deviceId": null,     // null = system default
    "shape": "circle",                     // circle | roundedRect | rect
    "size": "m",                           // s | m | l
    "position": { "anchor": "bottomLeft", "x": 0.04, "y": 0.78 },
    "mirror": true,
    "resolution": "720p",                  // 720p | 1080p
    "burnIn": true                         // bubble captured with the screen; false = composite at export (forced false for window sources)
  },
  "audio": {
    "micEnabled": true, "micDeviceId": null,
    "systemAudio": false,
    "micGainDb": 0, "systemGainDb": 0
  },
  "recording": {
    "countdownSeconds": 3,
    "maxDurationMinutes": null,
    "minFreeDiskMB": 2048
  },
  "output": {
    "autoExport": true,                    // write the MP4 on stop
    "preset": "default",
    "afterStop": "notify"                  // notify | reveal | openLibrary | nothing
  },
  "shortcuts": {
    "startStop": "CmdOrCtrl+Shift+L",
    "pauseResume": "CmdOrCtrl+Shift+P",
    "cancel": "CmdOrCtrl+Shift+Backspace",
    "toggleCamera": "CmdOrCtrl+Shift+C",
    "addMarker": "CmdOrCtrl+Shift+M"
  },
  "automation": {                          // see 05-cli-and-mcp.md
    "cliIsTrusted": true,
    "agentMayStartRecording": false,
    "agentMayDelete": false,
    "agentMaySnapshotScreen": false
  },
  "ui": {
    "theme": "system",
    "toolbarPosition": { "anchor": "bottomCenter", "dx": 0, "dy": -24 },
    "toolbarCollapsed": false,
    "launchAtLogin": false
  },
  "jobs": {
    "maxConcurrent": 1                     // heavy jobs (export/import/transcribe) at once; paused while recording
  },
  "advanced": {
    "encoder": "hardware",                 // hardware | software
    "screenBitrateKbps": null,             // null = automatic (about 6000 at 1080p30, scaled by pixel count)
    "cameraBitrateKbps": null,
    "logLevel": "info"
  }
}
```

Agents can't change `automation.*` at all. `settings.set` rejects those keys from MCP clients with `POLICY_DENIED`.

## Recorder state machine

There is one recorder per engine and one active recording at a time.

```
              start                 devices ready         countdown done
   idle ───────────────▶ preparing ─────────────▶ countdown ─────────────▶ recording ◀──┐
    ▲  ▲                    │ error                  │ cancel                │  │ pause  │ resume
    │  │                    ▼                        ▼                       │  ▼        │
    │  │                 failed ──(dismiss)──▶ idle  idle                    │ paused ───┘
    │  │                                                                     │  │
    │  └── stopping: writers close, finalize job queued (< 1 s) ◀── stop ────┴──┘
    │
    └──── cancel / restart (from countdown/recording/paused) → discarding → idle (restart → preparing)
```

**Finalize is a job, not a recorder state.** The recorder goes back to `idle` as soon as the writers close. That means a new recording can start while the previous one is still being finalized. Until finalize is done, the recording's manifest `status` is `finalizing`, and `RecorderStatus.pendingFinalize` lists the finalize job IDs that are still running.

| State | Allowed tools |
|---|---|
| `idle` | `recording.start`, all read/library/settings/export tools |
| `preparing`, `countdown` | `recording.cancel`, `recording.status`, `camera.update`, `audio.update` |
| `recording` | `pause`, `stop`, `cancel`, `restart`, `mark`, `camera.update`, `audio.update` (mute/unmute) |
| `paused` | `resume`, `stop`, `cancel`, `restart`, `mark`, `camera.update`, `audio.update` |
| `stopping`, `discarding` | `recording.status` and read tools (lasts under 1 s) |
| `failed` | `recording.status`, `recording.dismissError`, `library.recover` |

`RecorderStatus` is what `recording.status` returns and what `recorder.stateChanged` carries:

```jsonc
{
  "state": "recording",
  "recordingId": "rec_01JDX8Q3M7K2",
  "elapsedUs": 42100000,
  "segmentIndex": 1,
  "source": { "kind": "display", "displayId": "69733382", "name": "Built-in Retina Display" },
  "camera": { "enabled": true, "visible": true },
  "mic": { "enabled": true, "muted": false },
  "systemAudio": false,
  "startedBy": { "kind": "ui", "name": "Lumos Desktop" },
  "stats": { "bytesWritten": 11482113, "droppedFrames": 0, "freeDiskMB": 182330 },
  "pendingFinalize": []                    // finalize job ids of earlier recordings still being saved
}
```

## Events

These are delivered as JSON-RPC notifications `event { type, ts, data }`. Topics are subscribed by prefix, such as `recorder.*`.

| Type | Rate | Data |
|---|---|---|
| `recorder.stateChanged` | On change | `RecorderStatus` |
| `recorder.tick` | 1 Hz while recording | `{ elapsedUs, bytesWritten, droppedFrames }` |
| `recorder.warning` | On occurrence | `{ code, message }`. Codes: `MIC_DISCONNECTED`, `CAMERA_DISCONNECTED`, `WINDOW_CLOSED`, `FRAMES_DROPPED`, `DISK_LOW` |
| `audio.levels` | 15 Hz, only if subscribed | `{ mic: { rms, peak }, system: { rms, peak } }` in dBFS |
| `recording.ready` | Once per recording | `{ recordingId, outputPath, bundlePath, durationUs }` |
| `recording.failed` | Once | `{ recordingId, code, message, recoverable }` |
| `devices.changed` | On hot-plug | `{ cameras, microphones, displays }` |
| `permissions.changed` | On change | `PermissionStatus` |
| `job.progress` | ≤ 4 Hz | `{ jobId, progress, etaMs }` |
| `job.finished` | Once | `{ jobId, status, result \| error }` |
| `library.changed` | On change | `{ added: [], updated: [], removed: [] }` |
| `settings.changed` | On change | `{ paths: ["storage.saveFolder"] }` |
| `ui.command` | On request | Engine → UI: `{ command: "showLibrary" \| "showSettings" \| … }` |

## Jobs

Exports, imports, finalize, thumbnails and transcription all run as jobs.

```jsonc
{
  "id": "job_01JDXA…",
  "kind": "export",              // finalize | export | import | thumbnail | transcribe | reindex
  "status": "running",           // queued | running | done | failed | cancelled
  "recordingId": "rec_01JDX8Q3M7K2",
  "params": { "preset": "gif", "range": { "startUs": 5000000, "endUs": 30000000 } },
  "progress": 0.42, "etaMs": 6100,
  "createdAt": "…", "startedAt": "…", "finishedAt": null,
  "result": null,                // e.g. { "path": "/…/demo.gif", "bytes": 3211001 }
  "error": null                  // { code, message }
}
```

- While recording, heavy jobs (export, import, transcribe) are **paused** so they don't cause dropped frames. Finalize and thumbnail jobs still run.
- Concurrency: one heavy job at a time by default (`jobs.maxConcurrent`).
- Job history is kept in the index for 7 days.

## Library index (SQLite)

The index is a cache. Manifests are the truth, and `library.reindex` rebuilds the index by scanning each root for `*.lumos/manifest.json`.

```sql
CREATE TABLE roots       (path TEXT PRIMARY KEY, added_at TEXT, last_scan_at TEXT);
CREATE TABLE recordings  (id TEXT PRIMARY KEY, root TEXT, bundle_path TEXT, output_path TEXT,
                          title TEXT, created_at TEXT, duration_us INTEGER, status TEXT,
                          origin TEXT, bytes INTEGER, thumb_path TEXT, manifest_mtime INTEGER);
CREATE TABLE tags        (recording_id TEXT, tag TEXT, PRIMARY KEY (recording_id, tag));
CREATE TABLE jobs        (id TEXT PRIMARY KEY, kind TEXT, status TEXT, recording_id TEXT,
                          json TEXT, created_at TEXT);
CREATE VIRTUAL TABLE search USING fts5(recording_id UNINDEXED, title, transcript, markers);
```

A file watcher on each root picks up manifests that were edited, moved or deleted outside the app.

## Devices and sources

```jsonc
// sources.displays
[{ "id": "69733382", "name": "Built-in Retina Display", "main": true,
   "bounds": { "x": 0, "y": 0, "w": 1512, "h": 982 }, "scale": 2.0, "refreshHz": 120 }]

// sources.windows
[{ "id": 4127, "app": "Figma", "appBundleId": "com.figma.Desktop", "title": "Onboarding v3",
   "bounds": { "x": 120, "y": 80, "w": 1280, "h": 820 }, "displayId": "69733382", "onScreen": true }]

// sources.cameras / sources.microphones
[{ "id": "0x8020000005ac8514", "name": "FaceTime HD Camera", "default": true,
   "formats": [{ "width": 1280, "height": 720, "fps": [15, 30] }] }]
```

```jsonc
// permissions.status
{ "screen": "granted", "camera": "granted", "microphone": "denied",
  "accessibility": "notRequested" }        // accessibility is needed only for click highlights / key markers
```
