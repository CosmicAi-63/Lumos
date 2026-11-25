# Lumos

An open-source, local-first screen and camera recorder in the spirit of Loom.

Pick a screen, window or area. A floating toolbar lets you start, pause, resume, restart, cancel and stop. A camera bubble shows your face. When you stop, the video goes into the folder you chose in Settings. No account, no cloud and no telemetry.

Every feature is a **tool**, and you can call it three ways:

| Surface | Who uses it | Example |
|---|---|---|
| Desktop app | People | Click ● in the toolbar |
| `lumos` CLI | Scripts, terminals | `lumos record start --window-title Figma` |
| MCP server (`lumos mcp`) | AI agents | `lumos_recording_start` |

## Principles

1. **The core has no UI.** The desktop app is one client among three. It calls the same tools the CLI and MCP do.
2. **One tool registry.** Each operation is defined once, with its name, JSON Schema input and output, side effects and allowed states. The CLI commands, MCP tools and the UI's typed client are all generated from it.
3. **Local-first, open formats.** The output is a plain MP4 in your folder. The project data is a folder holding a versioned JSON manifest and one file per track.
4. **Recording is never silent.** Whenever capture is running, a visible indicator is on screen, no matter which client started it.
5. **UI parity rule.** Anything the UI can do, a tool can do. A PR that adds a button names the tool the button calls.

## Planning docs

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

## Status

**M0 skeleton.** The engine, tool registry, JSON-RPC socket and `lumos` CLI work end to end with a fake capture backend: `LUMOS_BACKEND=fake lumos record start --wait` saves a real MP4 plus a `.lumos` bundle. Real screen capture on macOS is the next step (the M0 spike). See the [roadmap](docs/08-roadmap.md).

```bash
cargo build --workspace
cargo test --workspace            # needs ffmpeg + ffprobe on PATH
cargo xtask schemas               # writes docs/schemas/
LUMOS_BACKEND=fake ./target/debug/lumos record start --countdown 0 --max-duration 3 --wait
./target/debug/lumos engine stop
```

Environment overrides: `LUMOS_HOME`, `LUMOS_SOCKET`, `LUMOS_BACKEND` (`fake` or `macos`), `LUMOS_TRASH_DIR`, `LUMOS_FFMPEG`, `LUMOS_FFPROBE`, `LUMOS_DAEMON`, `LUMOS_LOG`.
