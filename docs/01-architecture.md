# 1. Architecture

## Goals that shape it

- The UI is separate from the engine. The desktop app can be replaced, or not run at all.
- CLI and MCP get the same features as the UI with no duplicated logic.
- A recording outlives any single command: `lumos record start` returns right away, and `lumos record stop` comes minutes later.
- On macOS, Screen Recording, Camera and Microphone permissions (TCC) belong to the process that asks for them. A CLI launched from Terminal would otherwise borrow Terminal's permissions. An MCP server launched by an agent app would borrow that app's.

These goals lead to one long-lived **engine** that owns all capture and state. Every surface is a thin **client** that talks to it over JSON-RPC.

## Process model

```
 ┌─────────────── Desktop UI (Tauri webview) ───────────────┐  ┌── lumos CLI ──┐  ┌── lumos mcp ──┐
 │ menu bar · source picker · toolbar · library · settings  │  │ generated     │  │ stdio MCP     │
 │ (native windows: camera bubble, area selector overlay)   │  │ from registry │  │ server        │
 └───────────────────────────┬──────────────────────────────┘  └──────┬────────┘  └──────┬────────┘
                             │ in-process transport                   │                  │
                             │                      JSON-RPC 2.0 over local socket / named pipe
                             ▼                                        ▼                  ▼
 ┌──────────────────────────────────────── Engine ────────────────────────────────────────────┐
 │  Tool registry + dispatcher  →  policy check (client kind, state)  →  handler              │
 │  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────────┐  │
 │  │ Recorder FSM │ │ Library +    │ │ Settings     │ │ Job queue    │ │ Event bus        │  │
 │  │ + pipeline   │ │ SQLite index │ │ store        │ │ (export,     │ │ (state, levels,  │  │
 │  │              │ │              │ │              │ │ import, …)   │ │ progress)        │  │
 │  └──────┬───────┘ └──────────────┘ └──────────────┘ └──────────────┘ └──────────────────┘  │
 └─────────┼──────────────────────────────────────────────────────────────────────────────────┘
           ▼
   Capture backends (trait per OS)                Media layer
   macOS:   ScreenCaptureKit + AVFoundation       recording-time: native HW writers
   Windows: Windows.Graphics.Capture + WASAPI     post-processing: FFmpeg (LGPL, dynamic)
   Linux:   xdg-desktop-portal ScreenCast + PipeWire
```

### Engine host

**macOS (decision, to be confirmed in the M0 spike):** the engine runs inside the `Lumos.app` process.

- The app can start without any windows (`--background`), so it behaves like a daemon.
- One bundle identity holds all three TCC permissions. The user grants each one once.
- The camera bubble preview and the camera encoder share one `AVCaptureSession` in the same process. No frames cross a process boundary.
- The Tauri webview already runs in a separate WebKit process, so a UI crash does not stop a recording.
- The separation is enforced in the code, not by process boundaries. The UI crate cannot depend on engine crates. It reaches the engine only through the dispatcher, exactly like the CLI. See *Dependency rules* below.

**Fallback if the spike shows problems:** a separate signed helper (`LumosHelper.app`) registered with `SMAppService` as a LaunchAgent. In that case the camera preview frames would need an IOSurface hand-off to the UI, which costs more work.

**Linux, Windows and headless:** a standalone `lumosd` binary hosts the same engine crate.

### Client → engine connection

| OS | Endpoint | Access |
|---|---|---|
| macOS | `~/Library/Application Support/Lumos/engine.sock` | Unix socket, mode `0600` |
| Linux | `$XDG_RUNTIME_DIR/lumos/engine.sock` | Unix socket, mode `0600` |
| Windows | `\\.\pipe\lumos-engine-<user SID>` | Named pipe, current user ACL |

- If the socket is missing, the CLI or MCP server launches the host and waits up to 5 s for the socket. On macOS the launch is `open -g -j -b app.lumos.Lumos --args --background`; elsewhere it spawns `lumosd`.
- The first call on every connection is `session.hello { client: { kind: "ui"|"cli"|"mcp", name, version } }`. The engine uses `kind` for policy, such as whether an agent may start a recording. **This is a guardrail, not a security boundary.** The real boundary is the OS user: only the current user can open the socket. The protection that always holds is the on-screen recording indicator.

### Wire protocol

- JSON-RPC 2.0, one message per line (NDJSON). Each tool name is a method, for example `recording.start`.
- Server → client notifications use the method `event` with `{ type, ts, data }`. Clients choose topics with `events.subscribe`.
- Errors are `{ code: <int>, message, data: { code: "<STABLE_STRING>", ... } }`. The stable string is the contract. Integers follow JSON-RPC conventions.

| Stable error code | Meaning | CLI exit |
|---|---|---|
| `INVALID_PARAMS` | Schema validation failed | 2 |
| `INVALID_STATE` | Tool not allowed in the current recorder state | 3 |
| `PERMISSION_DENIED` | An OS permission is missing (`data.permission`) | 4 |
| `NOT_FOUND` | Unknown recording, job, device or source | 5 |
| `POLICY_DENIED` | Blocked by `automation.*` settings | 6 |
| `ENGINE_UNAVAILABLE` | The client could not reach or launch the engine | 7 |
| `DEVICE_BUSY` | A device is in exclusive use elsewhere | 8 |
| `DISK_LOW` | Free space is below `recording.minFreeDiskMB` | 9 |
| `BACKEND_ERROR` | An OS or codec failure (`data.detail`) | 1 |
| `CANCELLED` | A job or wait was cancelled | 130 |

## Tool registry (the single source of truth)

Each tool is declared once in `lumos-tools`:

```rust
pub struct ToolDef {
    pub name: &'static str,                 // "recording.start"
    pub summary: &'static str,              // one line, used in --help and MCP description
    pub params: fn() -> schemars::Schema,   // JSON Schema of input
    pub result: fn() -> schemars::Schema,   // JSON Schema of output
    pub effect: Effect,                     // ReadOnly | Mutating | Destructive | Capture
    pub long_running: bool,                 // returns { jobId } and emits job.* events
    pub idempotent: bool,
    pub surfaces: Surfaces,                 // bitflags: UI | CLI | MCP
    pub states: Option<&'static [RecorderState]>, // allowed recorder states, None = any
    pub cli: CliSpec,                       // path ["record","start"], positionals, aliases
}

#[async_trait]
pub trait ToolHandler {
    async fn call(&self, ctx: &CallCtx, params: serde_json::Value) -> Result<serde_json::Value, ToolError>;
}
```

The registry generates:

| Output | How |
|---|---|
| `schemas/*.json` | Build step that dumps each tool's params and result schemas. Checked in, and CI fails on drift. |
| `packages/api` (TypeScript) | `json-schema-to-typescript`, plus a thin typed client (`api.recording.start({...})`). |
| CLI tree | `clap` built from `CliSpec` at startup. Flags come from top-level schema properties. |
| MCP tools | `name.replace('.', '_')` prefixed with `lumos_`, with inputSchema/outputSchema and annotations from `effect` and `idempotent`. |
| Docs | `docs/04-tools.md` is the human version. A generated reference page sits next to it later. |

## Stack

| Layer | Choice | Why |
|---|---|---|
| Engine | Rust (tokio) | Cross-platform, a single binary, safe for a long-running media process |
| macOS capture | ScreenCaptureKit (screen, system audio) + AVFoundation (camera, mic) through a small Swift shim with a C ABI, or the `screencapturekit` crate (decide in M0) | Official API, can exclude our own windows, has system audio |
| Windows capture | Windows.Graphics.Capture + WASAPI loopback, Media Foundation camera | Native and supported |
| Linux capture | xdg-desktop-portal ScreenCast + PipeWire | Works on Wayland |
| Recording-time encode | Native hardware writers behind a `TrackWriter` trait: AVAssetWriter (VideoToolbox) on macOS, Media Foundation SinkWriter on Windows, GStreamer/VAAPI on Linux | Zero-copy from capture buffers, low CPU |
| Post-processing (concat, mix, export, import, thumbnails) | FFmpeg libraries, LGPL build, dynamically linked | One mature path for every format |
| Software encoder fallback | OpenH264 (BSD) and SVT-AV1. **Not x264**, which is GPL. | Keeps the license permissive |
| Desktop UI | Tauri 2 + React + TypeScript. Native NSWindow/NSPanel for the camera bubble and area overlay. | Small footprint, Rust host, and the webview uses the same API |
| Library index | SQLite (`rusqlite`) with FTS5 | A cache that can always be rebuilt from manifests |
| Schemas | `serde` + `schemars` → JSON Schema → TypeScript | One source of truth |

**Rejected: Electron.** Capture would live in the renderer (`getDisplayMedia`/`MediaRecorder`). That ties recording to a window, can't run headless for the CLI or MCP, and gives little control over segments, pause and hardware encoders.

**Prior art:** Cap is an open-source Loom alternative also built on Tauri and Rust. Read its capture code before M0, and check its license before reusing any of it.

## Monorepo layout

```
lumos/
├── crates/
│   ├── lumos-model/     # Pure data: manifest, settings, events, errors, IDs. No I/O.
│   ├── lumos-tools/     # ToolDef registry, params/result types, schema export
│   ├── lumos-core/      # Recorder FSM, library, settings store, jobs, event bus, policy
│   ├── lumos-capture/   # Capture traits + backends (cfg-gated: macos/windows/linux/fake)
│   ├── lumos-media/     # TrackWriter impls, concat, audio mix, compositor, import/export
│   ├── lumos-rpc/       # JSON-RPC codec, socket server, client, in-process transport
│   ├── lumosd/          # Standalone engine host binary
│   └── lumos-cli/       # `lumos` binary: CLI tree + `lumos mcp` subcommand
├── native/macos/        # Swift shim: ScreenCaptureKit, AVFoundation, bubble/overlay windows
├── apps/desktop/        # Tauri app (UI only) + macOS engine host bootstrap
├── packages/api/        # Generated TS types + typed client
├── schemas/             # Generated JSON Schemas (checked in)
└── docs/
```

### Dependency rules (CI-enforced)

- `apps/desktop` frontend → `packages/api` only.
- `apps/desktop` Rust host → `lumos-core`. This is the only place outside `lumosd` that may link the engine, and it does so only to *host* it. UI commands still go through `lumos-rpc`'s in-process transport.
- `lumos-cli` → `lumos-rpc` (client) + `lumos-tools` (definitions). **Never** `lumos-core`.
- `lumos-model` has no dependencies on any other Lumos crate.

## Testing strategy

- **Fake capture backend** (`lumos-capture/fake`): synthetic frames with a burned-in frame counter and timestamp, plus a 1 kHz tone. CI runs the whole engine headless on it. That covers the state machine, segments, pause/resume, finalize, A/V sync (counter vs. tone onset), crash recovery and disk-low.
- **Contract tests:** snapshot the generated schemas, CLI `--help` tree and MCP `tools/list`. Any surface change shows up in review.
- **Golden manifests:** fixtures for every manifest version, so migrations can't regress.
- **Real-capture checklist:** manual and local, run per release on macOS. CI runners can't be granted Screen Recording permission.

## Privacy and security

- No network access by default. Sharing (M6) is opt-in and off by default.
- The socket is readable only by the user who owns it.
- The recording indicator (toolbar or menu bar dot) is always visible while capture runs, and no setting turns it off.
- Agents (MCP clients) cannot start recordings, delete recordings, take screen snapshots or change `automation.*` settings unless the user enables each in Settings. See [CLI and MCP](05-cli-and-mcp.md).
