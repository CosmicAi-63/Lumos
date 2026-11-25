# 8. Roadmap

macOS first, engine and CLI first, UI second. Each milestone ends with something a person can use and a test that proves it.

## M0: Foundations (engine + CLI records a display)

> **Status (2026-11-25): skeleton done, capture spike open.** Built and tested: `lumos-model`, the `lumos-tools` registry (23 tools), `lumos-rpc`, `lumos-core` (recorder state machine, settings, jobs, finalize, agent policy), `lumos-media` (ffmpeg CLI), the fake capture backend, `lumosd`, the `lumos` CLI and `cargo xtask schemas`. The fake backend records, pauses, resumes, stops and saves a real MP4 end to end, in Rust tests and through the CLI. **Not built:** the macOS capture backend (displays and the Screen Recording check are real; recording returns `BACKEND_ERROR`), the Tauri shell, crash recovery. **Exit criteria below are not met:** nothing is QuickTime-verified from a real display, and A/V sync is untested.

**Build**

- Monorepo, CI (fmt, clippy, tests, dependency-rule check), and the `cargo xtask schemas` step.
- `lumos-model`, `lumos-tools` registry, and `lumos-rpc` (NDJSON JSON-RPC over a Unix socket plus an in-process transport).
- Engine host on macOS as `Lumos.app --background`, a minimal Tauri shell with no windows yet. `lumosd` for the fake backend in CI.
- Fake capture backend.
- macOS backend: display capture plus mic, AVAssetWriter H.264/AAC fMP4, segments, and a finalize job with concat and mux.
- Tools: `session.hello`, `system.info`, `system.health`, `permissions.*`, `sources.displays`, `sources.microphones`, `recording.start|pause|resume|stop|cancel|status`, `settings.get|set`, `jobs.get|wait`.
- CLI: `lumos record …`, `lumos doctor`, `lumos settings`, `lumos engine`.

**Spikes to settle**

1. Can the engine run inside the app bundle while the CLI talks to it, and are the TCC prompts attributed to `Lumos.app`? If not, switch to the `SMAppService` helper fallback.
2. Swift shim or the `screencapturekit` crate.
3. The minimum macOS version for system audio and mic capture paths.
4. Real disk usage and CPU at the default bitrates.

**Exit criteria**

- `lumos record start --countdown 0 && sleep 10 && lumos record pause && sleep 2 && lumos record resume && sleep 10 && lumos record stop` prints a path inside `storage.saveFolder`.
- The MP4 plays in QuickTime and is about 20 s long with no gap.
- The fake backend's CI test shows A/V offset < 33 ms after a pause/resume cycle.
- Killing the engine mid-recording, then relaunching, recovers a playable file.

## M1: Recorder UI (the Loom experience)

**Build**

- Menu bar item.
- Source picker (display, window, area) and the area selector.
- Countdown overlay.
- Floating toolbar with start, pause, resume, stop, restart and cancel, the timer, and the mic meter, excluded from capture.
- Camera bubble with shapes, sizes, drag and snap, and burn-in.
- Settings window: General with the save folder picker, plus Recording, Camera, Audio and Shortcuts.
- Onboarding and permissions flow.
- Global shortcuts and the after-stop toast.
- Engine work: `sources.windows`, `sources.cameras`, `sources.snapshot`, `camera.*`, `audio.*`, `recording.restart|mark`, window and area sources, system audio, layout keyframes, device-loss handling.
- Camera compositing at stop for window sources (and areas too small for the bubble). Without it, window recordings would lose the camera.

**Exit criteria**

- A first-time user installs the app, grants permissions, picks a save folder, and records a 5-minute screen plus camera video with one pause, without opening a terminal.
- The file is in their folder within 3 s of pressing Stop.
- The toolbar never appears in the video, and the bubble always does.

## M2: Library, edit, export

**Build**

- SQLite index and file watcher.
- Library window with grid, search, detail player and markers.
- Rename, tags, delete to Trash, reveal.
- Trim and cut with stale-output detection.
- Export presets `default`, `hevc`, `web`, `gif`, `audio`, `frames`.
- `jobs.*`, recovery UI, disk-low handling, `keepSourceTracks` pruning.
- CLI: `ls`, `show`, `rename`, `rm`, `trim`, `cut`, `export`, `jobs`.

**Exit criteria**

- `lumos export last --preset gif --range 0-10` writes a GIF under 5 MB.
- Deleting from the UI puts both the MP4 and the bundle in the Trash.
- Changing the save folder keeps old recordings in the library.

## M3: MCP

**Build**

- `lumos mcp` (stdio) with tools generated from the registry, annotations, resources and progress.
- The `automation.*` policy, the audit log, the "Started by…" indicator and the agent-start notification.
- Settings → Automation tab with the config snippet.
- Docs for Claude Desktop, Claude Code and Cursor.

**Exit criteria**

With default settings, an MCP client can list recordings, read a manifest, export a GIF of the last recording and stop a running recording. It gets a clear `POLICY_DENIED` message when it tries to start one. After the user enables agent recording, the client can start a recording, and the indicator plus notification appear.

## M4: Import, transcripts, polish

**Build**

- Importers (`video`, `audio`, `captions`, `bundle`), drag and drop, `import.plan` and dry run.
- Local transcription with whisper.cpp: `transcript.*`, caption export, transcript search, MCP transcript resource.
- `recomposite` export, and `burnIn: false` as a user choice for display recordings, reusing the M1 compositor.
- Click highlights and cursor events (needs Accessibility permission).
- Camera background blur.
- `chapters.suggest`.

## M5: Windows, then Linux

Capture backends behind the same traits, using the encoders from the [pipeline platform notes](03-recording-pipeline.md#platform-notes). Every engine test runs per OS on the fake backend, and the real-capture checklist runs per OS.

## M6: Optional sharing (off by default)

`share.upload` to a self-hostable Lumos share server (a small server with a viewer page) or any S3-compatible bucket. It returns a link. There's no hosted service unless the project later decides to run one.

## Later ideas

Drawing on screen while recording, zoom to cursor, a teleprompter, noise suppression, a vertical (9:16) crop preset, a multi-take stitcher, and a comment-at-timestamp viewer for shared links.

---

## Open decisions

| # | Decision | Recommendation | Why it matters |
|---|---|---|---|
| 1 | **License** | Apache-2.0 (permissive, with a patent grant). Choose AGPL-3.0 instead if stopping closed SaaS forks matters more than adoption. | Hard to change after outside contributions arrive |
| 2 | **Name** | "Lumos" works as a codename. Check trademark, domain, crates.io, Homebrew and npm availability before the first public release. | Renaming later touches the bundle ID, socket paths and the `lumos://` URIs |
| 3 | **Stack** | Rust engine + Tauri UI + native macOS windows for the bubble and overlay | Decides who can contribute and how capture code is written |
| 4 | **Platform order** | macOS → Windows → Linux | macOS has the best capture API and is where the project starts |
| 5 | **Minimum macOS** | 13 Ventura, if the M0 spike confirms system audio there. Otherwise 14. | Sets which APIs are available |
| 6 | **Camera default** | Burn-in, keep the raw track (see the pipeline doc) | Instant files versus layout flexibility |
| 7 | **Sharing/cloud** | Out of scope until M6, and self-host only | Shapes privacy promises and the cost of running anything |
| 8 | **Loom import** | Treat Loom downloads as plain MP4. Get a real Loom export before building anything Loom-specific. | Avoids guessing an undocumented format |
| 9 | **Is the CLI trusted?** | `automation.cliIsTrusted` defaults to `true`. An agent with shell access (Claude Code, for example) could then run `lumos record start` and skip `agentMayStartRecording`. The alternative default, `false`, would make every terminal user enable it once. | The trade-off is between agent-friendliness and the "agents need permission to record" promise |

## Risks

| Risk | Mitigation |
|---|---|
| macOS TCC behavior: permissions tied to the wrong binary, a relaunch needed after granting Screen Recording, periodic re-prompts | M0 spike 1, onboarding designed around relaunch, `permissions.changed` reopening onboarding |
| A/V drift on long recordings | A single host clock, segment-level offsets, a fake-backend drift test over a simulated 60 min |
| Dropped frames under load | Hardware encoders only during recording, heavy jobs paused while recording, the dropped-frame counter surfaced in the UI |
| FFmpeg licensing | LGPL build, dynamic linking, no GPL encoders (no x264). OpenH264 as the software fallback. |
| Code signing and notarization | An Apple Developer account and a notarization step in CI, budgeted in M1 |
| An agent recording without the user knowing | Default-off policy, an indicator that can't be hidden, a notification and the audit log ([CLI and MCP → Agent safety](05-cli-and-mcp.md#agent-safety)) |
| Scope creep toward a full video editor | Edits stay non-destructive trim, cut and markers until after M4. Anything bigger needs its own design doc. |
