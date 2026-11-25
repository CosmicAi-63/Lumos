# 5. CLI and MCP

Both are generated from the [tool registry](01-architecture.md#tool-registry-the-single-source-of-truth) and ship in **one binary**, `lumos`. `lumos mcp` runs the MCP server, so users install one thing.

## CLI

### Command tree

Tool names map to CLI paths through `CliSpec`. `recording.*` becomes `lumos record …` for ergonomics, and every other namespace keeps its name.

```
lumos record start   [--display main|<id>] [--window <id> | --app Figma [--title-contains Onboarding]]
                     [--area x,y,w,h [--on-display <id>]] [--no-camera] [--camera <device>]
                     [--no-mic] [--mic <device>] [--system-audio] [--countdown 3] [--title "…"]
                     [--max-duration 10m] [--save-folder <dir>]
lumos record pause | resume
lumos record stop    [--no-wait]                     # waits for the saved file by default, prints its path
lumos record cancel  [--yes]
lumos record restart [--yes]
lumos record status  [--watch]                       # --watch streams state + ticks
lumos record mark    ["label"]

lumos sources displays | windows [--app X] | cameras | mics
lumos sources snapshot --window <id> -o shot.png

lumos camera status | set [--show|--hide] [--shape circle|rounded|rect] [--size s|m|l]
                          [--position bottom-left|bottom-right|top-left|top-right] [--mirror|--no-mirror] [--device X]
lumos audio  status | set [--mute|--unmute] [--mic X] [--system-audio|--no-system-audio] | levels [--watch]

lumos ls             [query] [--tag t] [--since 7d] [--limit 20]      # alias of: lumos library list
lumos show <id|last>                                                   # library get
lumos rename <id> "New title"
lumos rm <id> [--yes] [--keep-output]                                  # moves to Trash
lumos reveal <id> [--bundle]
lumos library roots | add-root <dir> | remove-root <dir> | reindex | recover

lumos trim <id> [--start 00:02.5] [--end 03:01]
lumos cut  <id> 01:10-01:14.5
lumos export <id> [--preset default|hevc|web|gif|audio|prores|frames|captions|bundle]
                  [--range 00:05-00:30] [--max-width 960] [--fps 15] [-o out.gif] [--no-wait]
lumos import <files…> [--move|--reference] [--title "…"] [--captions subs.vtt] [--dry-run]
lumos probe <file>

lumos jobs [ls] | show <job> | wait <job> | cancel <job>
lumos settings get [path] | set path=value … | reset [path] | schema | edit   # edit opens $EDITOR, validates on save
lumos permissions [status] | request screen|camera|microphone
lumos doctor                                                            # system.health, human-readable
lumos open [library|settings|picker]                                    # ui.show
lumos engine start | stop | status | logs [-f]
lumos mcp                                                               # MCP server over stdio
lumos completions zsh|bash|fish
```

### Conventions

- **Times** are accepted as `90`, `1:30`, `00:01:30.5` or `1m30s`, and converted to microseconds.
- **IDs:** a full ID, a unique prefix, `last` or `last~1`. Titles work with `--by-title "Product demo"` (an error if more than one matches).
- **Output:** human-readable by default. `--json` prints the tool's result exactly as the schema defines it. `--watch` and `--json` together print NDJSON events. With `--quiet`, only the essential value is printed: the output path for `record stop` and `export`, the ID for `import`.
- **Confirmation:** destructive commands prompt on a TTY. On a non-TTY they need `--yes`, or they exit with code 2.
- **Exit codes:**

  | Code | Meaning |
  |---|---|
  | 0 | Success |
  | 1 | Backend or generic error |
  | 2 | Usage error |
  | 3 | Invalid state |
  | 4 | Permission denied |
  | 5 | Not found |
  | 6 | Policy denied |
  | 7 | Engine unavailable |
  | 8 | Device busy |
  | 9 | Disk low |
  | 130 | Cancelled |

- **Engine auto-start:** if the socket is missing, start the engine host and wait up to 5 s. `--no-start` turns this off.

### Examples

```bash
# 30-second window capture of Figma with no camera, saved path printed at the end
lumos record start --app Figma --no-camera --countdown 0 && sleep 30 && lumos record stop --quiet

# GIF of the last recording's first 20 seconds
lumos export last --preset gif --range 0-20 -o ~/Desktop/demo.gif

# Everything recorded this week, as JSON
lumos ls --since 7d --json | jq '.items[] | {title, outputPath}'

# Point recordings at a new folder
lumos settings set storage.saveFolder=~/Videos/Lumos
```

## MCP server

### Transport and setup

- `lumos mcp` speaks MCP over **stdio**. It's a client of the engine socket, the same as the CLI, and starts the engine if needed.
- A local Streamable HTTP transport (`lumos mcp --http 127.0.0.1:7341`) is optional, comes later, and needs a bearer token.
- Client config is the same for Claude Desktop, Claude Code, Cursor and others:

```json
{ "mcpServers": { "lumos": { "command": "lumos", "args": ["mcp"] } } }
```

Settings → Automation shows this snippet with a copy button.

### Tools

- Every registry tool whose surfaces include `A` is listed. MCP names use underscores: `recording.start` becomes `lumos_recording_start`.
- `inputSchema` and `outputSchema` come straight from the registry. Results return as `structuredContent` with a one-line text summary, for example "Recording started on Built-in Retina Display (countdown 3 s)".
- Annotations come from `effect`:

| Effect | `readOnlyHint` | `destructiveHint` | `idempotentHint` | `openWorldHint` |
|---|---|---|---|---|
| R | true | false | true | false |
| M | false | false | per tool | false |
| D | false | true | false | false |
| C | false | false | false | false |

- Tools that are blocked by policy are **still listed**. Calling one returns an error result whose text names the exact setting to enable, for example "Allow agents to start recordings: Settings → Automation". The agent can tell the user instead of guessing.
- **Long-running tools:** return `{ jobId }` right away. If the client sent a `progressToken`, the server forwards `job.progress` as MCP progress notifications. Otherwise the agent calls `lumos_jobs_wait`, which blocks for at most 55 s.
- **Not exposed to agents:** `events.*`, `permissions.request`, `settings.reset`, `library.addRoot`/`removeRoot`, `export.savePreset`/`deletePreset`, `recording.dismissError`, and any `settings.set` patch that touches `automation.*`.

### Resources

| URI | Content |
|---|---|
| `lumos://recordings` | `RecordingSummary[]` (newest 50) |
| `lumos://recordings/{id}` | Manifest JSON |
| `lumos://recordings/{id}/thumbnail` | `image/jpeg` |
| `lumos://recordings/{id}/transcript` | `text/vtt` (M4) |
| `lumos://recorder/status` | `RecorderStatus`, subscribable. The server sends `notifications/resources/updated` on each state change. |
| `lumos://settings` | Settings JSON |

Video bytes are never sent over MCP. Agents get file paths and can pass them to other tools.

### Prompts (M3+, optional)

| Prompt | Does |
|---|---|
| `record-a-demo` | Walks the agent through choosing a source, confirming with the user and starting a recording |
| `summarize-recording` | Pulls the transcript and markers for `{id}` and asks the client's model for a summary and chapters |
| `release-notes-from-recording` | Turns the transcript into release notes |

## Agent safety

Recording someone's screen is sensitive. These rules apply to clients with `kind: "mcp"`, and to `kind: "cli"` when `automation.cliIsTrusted` is false. Turn that setting off if agents on this machine run the CLI through a shell.

| Rule | Default |
|---|---|
| Start or resume recording | **Off.** `automation.agentMayStartRecording` |
| Snapshot screen or window contents | **Off.** `automation.agentMaySnapshotScreen` |
| Delete recordings (always to the Trash) | **Off.** `automation.agentMayDelete` |
| Pause, stop, cancel | Always allowed, since they reduce capture |
| Change `automation.*` settings | Never. UI only. |
| Countdown for agent-started recordings | At least 3 s, even if 0 is passed |
| Indicator | Menu bar dot plus toolbar, labeled "Started by <client name>". Can't be hidden. |
| Notification | A system notification when an agent starts a recording |
| Audit | Every capture-effect call is logged with client kind and name in `~/Library/Logs/Lumos/audit.log` |

`client.kind` is self-reported, so these rules are guardrails against agents acting on their own, not a defense against malware running as the same user. The indicator and the OS permission prompts remain the real protection.
