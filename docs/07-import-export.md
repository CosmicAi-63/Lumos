# 7. Import and export

Both run as jobs in `lumos-media` and use FFmpeg (LGPL build, dynamically linked) for demuxing, decoding and muxing. Recording-time encoding is separate. See the [pipeline doc](03-recording-pipeline.md).

## Importers

### Goal

Bring existing media into the library as normal recordings, so they can be trimmed, transcribed, exported and searched like anything recorded in Lumos. An imported file becomes a `.lumos` bundle with `origin: { kind: "imported", from, importer }`.

### Interface

```rust
pub trait Importer: Send + Sync {
    /// Stable id, e.g. "video", "audio", "captions", "bundle".
    fn id(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];

    /// How well this importer fits the probed file. Highest wins; ties go to registration order.
    fn score(&self, probe: &Probe) -> Option<u8>;           // None = can't handle

    /// Pure: decide what will happen. Shown by `import.plan` / `lumos import --dry-run`.
    fn plan(&self, input: &Path, probe: &Probe, opts: &ImportOptions) -> Result<ImportPlan>;

    /// Do it. Reports progress through ctx; must honor ctx.cancelled().
    async fn run(&self, plan: ImportPlan, ctx: &JobCtx) -> Result<RecordingId>;
}

pub struct ImportPlan {
    pub input: PathBuf,
    pub importer: &'static str,
    pub title: String,                     // from metadata title, else file stem
    pub created_at: DateTime<Utc>,         // from container creation_time, else file mtime
    pub tracks: Vec<PlannedTrack>,         // which input stream → which TrackKind, Copy or Transcode
    pub captions: Vec<PathBuf>,            // sidecars found next to the file (same stem .srt/.vtt)
    pub mode: ImportMode,                  // Copy | Move | Reference
    pub warnings: Vec<String>,             // e.g. "VFR source; will be normalized to 30 fps"
}
```

### Flow

```
paths ─▶ media.probe (ffprobe) ─▶ score all importers ─▶ plan ─▶ (dry run? stop) ─▶ job: run
                                                                                        │
                    create bundle under saveFolder/.lumos-inprogress/ ◀─────────────────┘
                    copy or transcode streams into tracks/ · write manifest · thumbnail
                    atomic rename into saveFolder · index · library.changed
```

### Modes

| Mode | Effect |
|---|---|
| `copy` (default) | The original stays put. The bundle gets its own copy, remuxed when possible. |
| `move` | Same as copy, then the original goes to the Trash once the import succeeds. |
| `reference` | The bundle points at the original path with no media copied. The recording is marked `offline` if the file disappears. Good for huge files, fragile when files move. |

### Stream mapping rules

- First video stream → `screen` track. A second video stream, if present, → `camera` (some recorders write two video streams).
- First audio stream → `mic` track. A second audio stream → `systemAudio`.
- **Stream copy** when the codec fits the track container: H.264, HEVC or AV1 in MP4, and AAC, Opus or ALAC in M4A/MP4. **Transcode** otherwise, to H.264 + AAC with the hardware encoder.
- Variable frame rate sources get a constant frame rate at the nearest of 24, 30 or 60 fps only when transcoding. A stream copy keeps the original timing.
- Embedded chapters → `markers`. Embedded subtitle streams → `captions/<lang>.vtt`.

### Format matrix

| Input | Importer | Default action | Notes |
|---|---|---|---|
| `.mp4` `.m4v` `.mov` (H.264/HEVC + AAC) | `video` | Remux (copy) | The most common case: QuickTime, phone videos, Loom downloads |
| `.mov` (ProRes) | `video` | Transcode to H.264 | ProRes is kept if `--keep-codec` is passed |
| `.webm` (VP8/VP9/AV1 + Opus/Vorbis) | `video` | VP8/VP9/Vorbis transcode, AV1/Opus copy | Browser recordings |
| `.mkv` | `video` | Copy if the codecs fit, else transcode | OBS's default container |
| `.avi` `.wmv` `.flv` `.ts` `.mts` `.3gp` | `video` | Transcode | Legacy formats |
| `.gif` `.apng` `.webp` (animated) | `video` | Transcode to H.264 (no audio) | |
| `.m4a` `.mp3` `.wav` `.aac` `.flac` `.ogg` `.opus` `.aiff` | `audio` | Copy AAC/Opus, transcode others | Audio-only recording, no screen track. The library shows a waveform thumbnail. |
| `.srt` `.vtt` | `captions` | Attach to an existing recording (`--to <id>`) or to a sibling video in the same import | Converted to VTT |
| `.lumos` (bundle or `.lumos.zip`) | `bundle` | Validate, migrate the manifest, copy | Moves recordings between machines |
| Folder | — | Expanded to its files (non-recursive unless `--recursive`) | |

**Not planned until real samples exist:**

- **Loom metadata:** Loom downloads are plain MP4s, which the `video` importer already handles. Titles, transcripts and comments from Loom's export would need a real export sample to scope. That's an open question in the [roadmap](08-roadmap.md).
- **Project formats from other recorders** (Screen Studio, Cap, OBS scene data): same rule, sample first.

### Errors and edge cases

| Case | Behavior |
|---|---|
| No video and no audio streams | Skipped with the reason "no media streams" |
| DRM or encrypted | Skipped with the reason "protected content" |
| Duration unknown or a broken index | Try a remux to rebuild the index. If that fails, transcode with error tolerance and add a warning. |
| Duplicate (same SHA-256 as an earlier import) | Skipped unless `--allow-duplicates`. The result names the existing recording ID. |
| Not enough disk space for the copy | `DISK_LOW` before starting, estimated from probe size × 1.1 |

## Exporters

### Built-in presets

| Preset id | Container / codecs | Defaults | Use |
|---|---|---|---|
| `default` | MP4, H.264 High + AAC, `+faststart`, chapters | Source resolution, 30 fps, about 6 Mbps at 1080p | The file written on stop. Plays everywhere. |
| `hevc` | MP4, HEVC (hvc1 tag) + AAC | About 40% smaller than `default` | Archiving, Apple devices |
| `web` | WebM, VP9 (or AV1 if a hardware encoder exists) + Opus | Max 1080p | Embeds on the web |
| `gif` | GIF, two-pass palette (`palettegen`/`paletteuse`), dithering | 960 px wide, 12 fps, no audio | READMEs, issues, chat |
| `audio` | M4A (AAC 160 kbps) or MP3 (LAME is LGPL; verify the build) | Mixed mic + system | Podcasts, notes |
| `prores` | MOV, ProRes 422 + PCM | Source resolution | Handing off to an editor |
| `frames` | PNG sequence or a single PNG (`atUs`) | Every frame, or a range at N fps | Docs, thumbnails |
| `captions` | SRT, VTT or TXT | Needs a transcript (M4) | Subtitles, notes |
| `bundle` | `.lumos.zip` (manifest + tracks) | Stored, no compression for video | Sharing a project |
| `recomposite` (M4) | MP4 | Camera layout re-rendered from the raw camera track and layout keyframes | Only for recordings made with `burnIn: false` |

### Preset schema

```jsonc
{
  "schema": "lumos.exportPreset/1",
  "id": "gif",
  "label": "GIF (960 px)",
  "builtIn": true,
  "container": "gif",                 // mp4 | mov | webm | gif | m4a | mp3 | png | srt | vtt | txt | zip
  "video": { "codec": "gif", "maxWidth": 960, "maxHeight": null, "fps": 12, "bitrateKbps": null,
             "crf": null, "hardware": "prefer" },        // prefer | require | never
  "audio": null,                      // or { "codec": "aac", "bitrateKbps": 160, "mix": { "micGainDb": 0, "systemGainDb": -6 } }
  "camera": { "layout": "asRecorded" },                  // asRecorded | hidden | corner | sideBySide | cameraOnly (recomposite only)
  "chapters": false,
  "fileNameTemplate": "{title}.gif"
}
```

Users can save their own presets with `export.savePreset`, for example "Twitter 720p".

### How an export runs

1. Resolve the preset, apply `overrides`, and validate it against the recording (`UNSUPPORTED_PRESET` if the recording can't satisfy it).
2. Build the edit list: `trim` minus `cuts`, intersected with the requested `range`. The result is a list of kept ranges.
3. **Fast path:** if the preset matches the source codecs and every kept range starts on a keyframe, use a stream copy with concat, which takes seconds. Otherwise re-encode with an FFmpeg filter graph (`trim`/`atrim` per range → `concat` → `scale`/`fps` → encoder).
4. Write to `<out>.partial`, then `rename` when done. A cancelled export removes the partial file.
5. Add an entry to `manifest.outputs`, emit `job.finished` with `{ path, bytes, durationUs }`.

## Non-destructive edits

- Edits live in `manifest.edits` (see the [data model](02-data-model.md)). The tracks are never rewritten.
- `outputs[].editsHash` records which edit state produced each output, which is how the library knows an MP4 is stale.
- Undo and redo live in the UI: it keeps a history of `edits` snapshots and calls `edit.*` to restore one. The engine stays stateless about history.
