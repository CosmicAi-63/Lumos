# Lumos

An open-source, local-first screen and camera recorder in the spirit of Loom.

Pick a screen, window or area, and record it with your mic, system audio and a camera bubble. When you stop, an MP4 lands in your folder. No account, no cloud, no telemetry.

**[⬇ Download for Mac (Apple Silicon)](https://github.com/CosmicAi-63/Lumos/releases/latest/download/Lumos-macos-arm64.dmg)** · [All releases](https://github.com/CosmicAi-63/Lumos/releases)

## Quick start

Lumos runs on macOS 13 or later on Apple Silicon (M1 and newer). Windows, Linux and Intel Macs are not supported yet.

1. **Install ffmpeg.** Lumos uses it to put the final video together. With [Homebrew](https://brew.sh):
   ```bash
   brew install ffmpeg
   ```
2. **Download** [Lumos-macos-arm64.dmg](https://github.com/CosmicAi-63/Lumos/releases/latest/download/Lumos-macos-arm64.dmg), open it and drag **Lumos** into **Applications**.
3. **Open it the first time.** The app isn't notarized by Apple yet, so macOS blocks the first launch. Double-click Lumos and close the warning, then go to **System Settings → Privacy & Security**, scroll down and click **Open Anyway**. Or run this once in Terminal:
   ```bash
   xattr -dr com.apple.quarantine /Applications/Lumos.app
   ```
4. **Find it in the menu bar.** Lumos has no Dock icon. Click its icon at the top right of the screen.
5. **Allow access.** On your first recording, macOS asks for Screen Recording, Camera and Microphone access. After you turn on Screen Recording, quit and reopen Lumos.
6. **Record.** Pick a screen, window or area, then use the floating toolbar to start, pause, resume, restart, cancel or stop. Recordings go to `~/Movies/Lumos`. You can change the folder in Settings.

## One set of tools, three ways in

Every feature is a **tool**, and you can call it three ways:

| Surface | Who uses it | Example |
|---|---|---|
| Desktop app | People | Click ● in the toolbar |
| `lumos` CLI | Scripts, terminals | `lumos record start --window-title Figma` |
| MCP server (`lumos mcp`) | AI agents | `lumos_recording_start` |

The download contains the desktop app. The CLI and MCP server are built from source for now (see below).

## Principles

1. **The core has no UI.** The desktop app is one client among three. It calls the same tools the CLI and MCP do.
2. **One tool registry.** Each operation is defined once, with its name, JSON Schema input and output, side effects and allowed states. The CLI commands, MCP tools and the UI's typed client are all generated from it.
3. **Local-first, open formats.** The output is a plain MP4 in your folder. The project data is a folder holding a versioned JSON manifest and one file per track.
4. **Recording is never silent.** Whenever capture is running, a visible indicator is on screen, no matter which client started it.
5. **UI parity rule.** Anything the UI can do, a tool can do. A PR that adds a button names the tool the button calls.

## Status

**Milestones M0–M3 are built on macOS. M4 and later have not started.**

Working:
- Engine, tool registry, JSON-RPC socket, `lumos` CLI and `lumos mcp` (generated from the same registry).
- macOS capture through a Swift helper (ScreenCaptureKit): display, window and area sources, mic and system audio, pause and resume.
- Library, export, import and recover tools.
- Desktop app (Tauri): menu-bar popover, floating toolbar, camera bubble and main window.

Not done yet: Windows and Linux, Intel Mac builds, signed and notarized builds, transcription, trim UI polish, testing on macOS 13 and 14, and long-recording sync. So far it has been tested on one machine running macOS 15.

See the [roadmap](docs/08-roadmap.md).

## Build from source

You need Rust (stable), the Xcode Command Line Tools (`xcode-select --install`) for the Swift capture helper, and ffmpeg and ffprobe on your PATH.

```bash
git clone https://github.com/CosmicAi-63/Lumos.git
cd Lumos
cargo build --workspace
```

Build the app bundle into `~/Applications/Lumos.app` (add `RELEASE=1` for an optimized build):

```bash
scripts/make-app.sh
```

Build the downloadable DMG into `dist/`:

```bash
scripts/make-dmg.sh
```

Run the tests and try the CLI with the fake capture backend:

```bash
cargo test --workspace
LUMOS_BACKEND=fake ./target/debug/lumos record start --countdown 0 --max-duration 3 --wait
./target/debug/lumos engine stop
```

Regenerate the JSON Schemas in `docs/schemas/`:

```bash
cargo run -p xtask -- schemas
```

Environment overrides: `LUMOS_HOME`, `LUMOS_SOCKET`, `LUMOS_BACKEND` (`fake` or `macos`), `LUMOS_TRASH_DIR`, `LUMOS_FFMPEG`, `LUMOS_FFPROBE`, `LUMOS_DAEMON`, `LUMOS_LOG`.

### Publishing a release

Push a version tag. The [release workflow](.github/workflows/release.yml) builds the DMG on GitHub Actions and attaches it to a GitHub release, which is what the download link above points to.

```bash
git tag v0.1.0 && git push origin v0.1.0
```

## Design docs

| # | Doc | What it covers |
|---|---|---|
| 1 | [Architecture](docs/01-architecture.md) | Process model, engine host, IPC, stack, monorepo layout, testing |
| 2 | [Data model](docs/02-data-model.md) | IDs, on-disk bundle, manifest, settings, state machine, events, jobs, index |
| 3 | [Recording pipeline](docs/03-recording-pipeline.md) | Capture → encode → segments → pause → finalize, camera bubble, failure handling |
| 4 | [Tool catalog](docs/04-tools.md) | Every tool: params, results, errors, states, surfaces |
| 5 | [CLI and MCP](docs/05-cli-and-mcp.md) | Command tree, output and exit codes, MCP tools/resources, agent safety |
| 6 | [UI/UX](docs/06-ui-ux.md) | Toolbar, camera bubble, pickers, library, settings, onboarding, shortcuts |
| 7 | [Import and export](docs/07-import-export.md) | Importer interface, format matrix, export presets, non-destructive edits |
| 8 | [Roadmap](docs/08-roadmap.md) | Milestones with exit criteria, open decisions, risks |

## License

[Apache-2.0](LICENSE)
