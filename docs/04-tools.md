# 4. Tool catalog

Every engine capability, defined once in `lumos-tools`. Each tool is a JSON-RPC method, and most are also a CLI command and an MCP tool (see [CLI and MCP](05-cli-and-mcp.md) for how they map).

**Legend**

- **Effect:** `R` read-only · `M` mutating · `D` destructive · `C` starts or affects capture (privacy-sensitive)
- **Surfaces:** `U` UI · `L` CLI · `A` agent (MCP). An `A*` means the tool is off for agents until the user enables it under Settings → Automation.
- **States:** recorder states the tool is valid in. "any" means no restriction.
- **⏳:** long-running. The tool returns `{ jobId }` and accepts `wait: true` (plus `timeoutMs`) to block until it finishes.

Shared error codes are listed in [Architecture → Wire protocol](01-architecture.md#wire-protocol). Only codes specific to a tool are listed below.

---

## `session.*`: connection

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `session.hello` | R | internal | `{ client: { kind: "ui"\|"cli"\|"mcp", name, version }, protocol: 1 }` | `{ engineVersion, protocol, capabilities: string[] }` |
| `events.subscribe` | R | U L | `{ topics: string[] }` (prefixes, e.g. `"recorder."`) | `{ subscribed: string[] }` |
| `events.unsubscribe` | R | U L | `{ topics: string[] }` | `{ subscribed: string[] }` |

`capabilities` lists optional features, for example `"systemAudio"`, `"windowExclusion"`, `"transcribe"`, `"ui"` (a UI is attached). Clients hide what's missing.

---

## `system.*`: health and info

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `system.info` | R | U L A | — | `{ version, platform, osVersion, arch, engineHost: "app"\|"lumosd", settingsPath, logsPath }` |
| `system.health` | R | U L A | — | `{ permissions, devices: { cameras, microphones, displays }, saveFolder: { path, writable, freeMB }, encoder: { hardware: bool, codecs } , problems: Problem[] }` |
| `system.clients` | R | U L | — | `[{ kind, name, version, connectedAt }]` |

`system.health` powers `lumos doctor` and the Settings diagnostics. Each `Problem` is `{ code, message, fix }`, where `fix` is a human hint or the tool to run.

---

## `permissions.*`

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `permissions.status` | R | U L A | — | `{ screen, camera, microphone, accessibility }`, each `granted \| denied \| notRequested \| restricted` |
| `permissions.request` | M | U L | `{ permission: "screen"\|"camera"\|"microphone"\|"accessibility" }` | `{ permission, state, needsRelaunch: bool, settingsUrl? }` |

- `request` triggers the OS prompt the first time. If the permission was already denied, it opens the right System Settings pane and returns `settingsUrl`.
- Agents can't request permissions. The user does that.

---

## `sources.*`: what can be recorded

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `sources.displays` | R | U L A | — | `Display[]` |
| `sources.windows` | R | U L A | `{ app?: string, titleContains?: string, onScreenOnly?: bool = true }` | `Window[]` (Lumos's own windows excluded) |
| `sources.cameras` | R | U L A | — | `Device[]` |
| `sources.microphones` | R | U L A | — | `Device[]` |
| `sources.snapshot` | R·C | U L A* | `{ source: CaptureSource, maxWidth?: int = 1280 }` | `{ mimeType: "image/png", path, width, height }` |

`sources.snapshot` captures one still. The UI uses it for picker thumbnails, and agents can use it to choose a window. It shows screen contents, so agents get it only with `automation.agentMaySnapshotScreen`.

---

## `recording.*`: the recorder

| Tool | Effect | States | Surfaces | Params | Returns |
|---|---|---|---|---|---|
| `recording.start` | C | idle | U L A* | see below | `RecorderStatus` |
| `recording.pause` | C | recording | U L A | — | `RecorderStatus` |
| `recording.resume` | C | paused | U L A* | — | `RecorderStatus` |
| `recording.stop` | C | recording, paused | U L A | `{ wait?: bool = false, timeoutMs?: int = 55000 }` | `{ recordingId, finalizeJobId, outputPath?, bundlePath? }` (paths only if `wait`) |
| `recording.cancel` | D | preparing, countdown, recording, paused | U L A | `{ confirm: true }` | `{ recordingId, discarded: true }` |
| `recording.restart` | D·C | countdown, recording, paused | U L A* | `{ confirm: true }` | `RecorderStatus` (new `recordingId`) |
| `recording.status` | R | any | U L A | — | `RecorderStatus` |
| `recording.mark` | M | recording, paused | U L A | `{ label?: string }` | `{ markerId, atUs }` |
| `recording.dismissError` | M | failed | U L | — | `RecorderStatus` (idle) |

Agents may always **pause, stop and cancel**, because those reduce capture. Starting or resuming needs `agentMayStartRecording`.

### `recording.start` params

```jsonc
{
  "source": { "kind": "display", "displayId": "main" },      // default: settings.capture.defaultSource
            // | { "kind": "window", "windowId": 4127 }
            // | { "kind": "window", "app": "Figma", "titleContains": "Onboarding" }  (resolved to one window or NOT_FOUND / AMBIGUOUS)
            // | { "kind": "area", "displayId": "main", "rect": { "x": 0, "y": 0, "w": 1280, "h": 720 } }
  "camera":  { "enabled": true, "deviceId": null },          // default: settings.camera
  "mic":     { "enabled": true, "deviceId": null },          // default: settings.audio
  "systemAudio": false,
  "countdownSeconds": 3,                                      // 0 = none; for agents the minimum is 3, even if 0 is passed
  "title": "Product demo",
  "maxDurationSeconds": null,
  "saveFolder": null                                          // one-off override of settings.storage.saveFolder
}
```

- **Errors:** `INVALID_STATE` (already recording), `PERMISSION_DENIED { permission }`, `NOT_FOUND` / `AMBIGUOUS_SOURCE { candidates }`, `DEVICE_BUSY`, `DISK_LOW`, `POLICY_DENIED`.
- **Events:** `recorder.stateChanged` (preparing → countdown → recording), then `recorder.tick` once a second.
- **Returns** as soon as the recorder enters `countdown`. Don't wait for it to reach recording.

### `recording.stop` flow

1. Return immediately with `finalizeJobId`. The recorder is already back to `idle`.
2. `job.progress` events follow, and `recording.ready` fires when the file is in the save folder.
3. With `wait: true`, the call blocks until `recording.ready` and returns the output paths. The CLI uses that by default for `lumos record stop`.

---

## `camera.*` and `audio.*`: live controls

| Tool | Effect | States | Surfaces | Params | Returns |
|---|---|---|---|---|---|
| `camera.status` | R | any | U L A | — | `{ enabled, visible, deviceId, shape, size, position, mirror, previewing }` |
| `camera.update` | M | any | U L A | patch: `{ enabled?, visible?, deviceId?, shape?, size?, position?: { anchor?, x?, y? }, mirror? }` | `camera.status` result |
| `audio.status` | R | any | U L A | — | `{ micEnabled, micMuted, micDeviceId, systemAudio, micGainDb, systemGainDb }` |
| `audio.update` | M | any | U L A | patch: `{ micEnabled?, micMuted?, micDeviceId?, systemAudio?, micGainDb?, systemGainDb? }` | `audio.status` result |
| `audio.levels` | R | any | U L | — | `{ mic: { rms, peak }, system: { rms, peak } }` (one snapshot; subscribe to `audio.levels` events for live meters) |

- **Outside a recording**, updates change the *session defaults*: what the next recording uses and what the bubble preview shows. They don't write to Settings. To persist a default, call `settings.set`.
- **During a recording**, `camera.update` writes a layout keyframe. `micEnabled` and `systemAudio` can't change mid-recording (`INVALID_STATE`). Use `micMuted` instead.
- `position.anchor` is one of `topLeft`, `topRight`, `bottomLeft`, `bottomRight`, `free`.

---

## `library.*`: recordings on disk

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `library.list` | R | U L A | `{ query?, tags?, origin?: "recorded"\|"imported", status?, since?, until?, sort?: "newest"\|"oldest"\|"longest"\|"title", limit?: int = 50, cursor? }` | `{ items: RecordingSummary[], nextCursor? }` |
| `library.get` | R | U L A | `{ id }` (id, prefix or `last`) | `Manifest` + `{ bundlePath, outputPath, bytes }` |
| `library.update` | M | U L A | `{ id, title?, tags?: string[] }` | `RecordingSummary` (renaming moves the MP4 and the bundle) |
| `library.delete` | D | U L A* | `{ id, confirm: true, keepOutput?: bool = false }` | `{ id, trashed: string[] }` |
| `library.reveal` | R | U L | `{ id, which?: "output"\|"bundle" }` | `{ path }` (opens Finder/Explorer) |
| `library.roots` | R | U L A | — | `[{ path, recordings, bytes, available }]` |
| `library.addRoot` | M | U L | `{ path }` | `{ path, found: int }` |
| `library.removeRoot` | M | U L | `{ path }` | `{ path }` (forgets the root, deletes nothing) |
| `library.reindex` ⏳ | M | U L A | `{ root? }` | `{ jobId }` → `{ scanned, added, updated, removed }` |
| `library.recover` ⏳ | M | U L A | `{ id? }` (default: all in-progress leftovers) | `{ jobId }` → `{ recovered: RecordingSummary[] }` |

```jsonc
// RecordingSummary
{ "id": "rec_01JDX8Q3M7K2", "title": "Product demo", "createdAt": "2026-11-25T22:03:11.204Z",
  "durationUs": 184233000, "status": "ready", "origin": "recorded", "bytes": 48211392,
  "outputPath": "/Users/me/Movies/Lumos/Product demo — 2026-11-25 14.03.mp4",
  "thumbPath": "/Users/me/Movies/Lumos/Product demo — 2026-11-25 14.03.lumos/thumb.jpg",
  "tags": [], "markers": 1, "hasTranscript": false }
```

`library.delete` always moves files to the OS Trash. Nothing is ever hard-deleted.

---

## `edit.*`: non-destructive edits

Edits change `manifest.edits` only. The source tracks are never touched. The edits apply at the next export.

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `edit.trim` | M | U L A | `{ id, startUs?, endUs? }` | `{ edits, effectiveDurationUs }` |
| `edit.cut` | M | U L A | `{ id, startUs, endUs }` (adds a removed range) | `{ edits, effectiveDurationUs }` |
| `edit.uncut` | M | U L A | `{ id, index }` | `{ edits, effectiveDurationUs }` |
| `edit.reset` | M | U L A | `{ id }` | `{ edits: {} }` |
| `edit.setMarkers` | M | U L A | `{ id, markers: [{ atUs, label }] }` | `{ markers }` |

After an edit, the output MP4 is stale. The library shows "Edited, re-export?". If `output.autoExport` is on, the UI offers a one-click re-export. The engine doesn't re-export silently.

---

## `export.*`

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `export.presets` | R | U L A | — | `ExportPreset[]` (built-in + user) |
| `export.create` ⏳ | M | U L A | see below | `{ jobId }` → `{ path, bytes, durationUs }` |
| `export.savePreset` | M | U L | `ExportPreset` | `ExportPreset` |
| `export.deletePreset` | D | U L | `{ id }` | `{ id }` |

```jsonc
// export.create params
{
  "id": "last",
  "preset": "gif",                                // see 07-import-export.md
  "range": { "startUs": 5000000, "endUs": 30000000 },   // on top of edits; optional
  "overrides": { "maxWidth": 960, "fps": 15 },    // any preset field
  "outPath": null,                                // default: next to the recording, "<title>.<ext>"
  "applyEdits": true,
  "wait": false
}
```

**Errors:** `NOT_FOUND`, `UNSUPPORTED_PRESET` (for example re-composite on a burn-in recording), `DISK_LOW`. If `outPath` already exists, the job writes `"<title> (2).<ext>"` unless `overwrite: true` is passed.

---

## `import.*` and `media.*`

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `media.probe` | R | U L A | `{ path }` | `Probe` (container, duration, streams with codec/size/fps/rate, chapters, tags) |
| `import.formats` | R | U L A | — | `[{ importer, extensions, description }]` |
| `import.plan` | R | U L A | `{ paths: string[], options? }` | `ImportPlan[]` (what would happen, no side effects) |
| `import.create` ⏳ | M | U L A | `{ paths: string[], mode?: "copy"\|"move"\|"reference" = "copy", title?, tags?, captions?: path, wait? }` | `{ jobId }` → `{ recordings: RecordingSummary[], skipped: [{ path, reason }] }` |
| `media.thumbnail` ⏳ | M | U L A | `{ id, atUs?, width? = 640 }` | `{ jobId }` → `{ path }` |

See [Import and export](07-import-export.md) for the importer interface and format matrix.

---

## `jobs.*`

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `jobs.list` | R | U L A | `{ status?, kind?, recordingId?, limit? = 50 }` | `Job[]` |
| `jobs.get` | R | U L A | `{ jobId }` | `Job` |
| `jobs.wait` | R | U L A | `{ jobId, timeoutMs?: int = 55000 }` | `Job` (returns early when done. If it times out the job keeps running, and the result has `status: "running"`) |
| `jobs.cancel` | M | U L A | `{ jobId }` | `Job` (`status: "cancelled"`. Partial output removed) |

The 55 s default timeout stays under common MCP client limits. Agents loop on `jobs.wait` for long exports.

---

## `settings.*`

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `settings.get` | R | U L A | `{ path?: "storage.saveFolder" }` | the whole object or the value at `path` |
| `settings.set` | M | U L A | `{ patch: { "storage.saveFolder": "~/Videos/Lumos", … } }` (dotted paths) | `{ changed: string[], settings }` |
| `settings.reset` | M | U L | `{ path? }` | `{ changed: string[], settings }` |
| `settings.schema` | R | U L A | — | JSON Schema of `settings.json` (with titles and descriptions for UIs and agents) |

**Validation:** `saveFolder` must exist or be creatable and writable. Shortcuts must parse and not conflict. Agents get `POLICY_DENIED` for `automation.*`.

---

## `share.*`: public links through Google Drive

Each person connects their own Google Drive. Lumos uploads only when asked, into a "Lumos" folder in that Drive, and turns on "anyone with the link can view". Lumos runs no server. Setup and design: [Sharing](10-sharing.md).

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `share.status` | R | U L A | — | `{ provider, configured, connected, account?, folderName, hint? }`. Never includes tokens. |
| `share.connect` | M | U L | `{ clientJson?, noBrowser?, timeoutMs? }` | `ShareStatus`. Opens Google sign-in in the browser and waits (default 5 min). Emits `share.signInUrl { url }` first. |
| `share.disconnect` | M | U L | — | `ShareStatus`. Revokes the token at Google and deletes it here. Shared files stay in Drive. |
| `share.upload` ⏳ | M | U L A* | `{ id, force?, wait?, timeoutMs? }` | `{ jobId }` → `{ url, fileId }`. If the recording already has a link and `force` is not set: `{ url, fileId, existing: true }` with no job. |

- `share.upload` is `A*`: agents need **Settings → Automation → Let AI agents share recordings by link** (`automation.agentMayShare`), because a link publishes the video.
- Agents can never call `share.connect`. Only a person links an account.
- The link is saved in the manifest (`shares[]`) and in `library.list` as `shareUrl`.
- Events: `share.signInUrl`, `share.connected`, `share.disconnected`, `share.uploaded { recordingId, url, fileId, bytes }`.

**Errors:** `AUTH_REQUIRED` (not connected, or the sign-in expired or was revoked: connect again), `INVALID_STATE` (no OAuth client set up, or Google rejected it), `BACKEND_ERROR` (Drive full, network, or a Workspace admin blocks public links; in that case `data.fileId` names the uploaded file).

---

## `ui.*`: asks an attached UI to show something

These tools exist so the CLI and agents can bring up the app. The engine forwards them to the UI as `ui.command` events. With no UI attached they return `UI_UNAVAILABLE`.

| Tool | Effect | Surfaces | Params | Returns |
|---|---|---|---|---|
| `ui.show` | R | L A | `{ view: "sourcePicker"\|"library"\|"settings"\|"recording", id? }` | `{ shown: true }` |

---

## Later milestones

| Tool | Milestone | Notes |
|---|---|---|
| `transcript.create` ⏳ | M4 | `{ id, language?: "auto", model?: "base"\|"small"\|"medium" }`, local whisper.cpp → `captions/<lang>.vtt` and the search index |
| `transcript.get` | M4 | `{ id, format: "vtt"\|"srt"\|"txt"\|"json" }`. The main way agents "watch" a recording. |
| `chapters.suggest` | M4 | Built from markers, silence detection and transcript sections. Summaries are left to the MCP client's own model, so the engine needs no LLM. |

## Adding a tool (checklist)

1. Define the params and result types in `lumos-model` with `#[derive(Serialize, Deserialize, JsonSchema)]` and doc comments. The comments become CLI help and MCP descriptions.
2. Register a `ToolDef` in `lumos-tools` with effect, states, surfaces and `CliSpec`.
3. Implement `ToolHandler` in `lumos-core`.
4. Run `cargo xtask schemas`, which regenerates `schemas/` and `packages/api`.
5. Add a contract test (fake backend) and update this page.
