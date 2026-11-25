# 6. UI/UX

The desktop app is a **client**. It renders engine state and calls tools, and it never touches devices or files directly. Its whole state is a projection of `recorder.stateChanged`, `settings.changed`, `library.changed` and `job.*` events.

**Parity rule:** every control maps to a tool from the [catalog](04-tools.md). Only purely visual state (window positions, which tab is open) lives in the UI alone.

## Surfaces

| Surface | Tech | Excluded from capture? |
|---|---|---|
| Menu bar item | Native tray (Tauri) | n/a |
| Source picker panel | Webview | Yes |
| Area selector overlay | Native full-screen transparent window | Yes |
| Countdown overlay | Native | Yes |
| Recording toolbar | Webview in a floating `NSPanel` | **Yes** |
| Camera bubble | Native `NSPanel` + `AVSampleBufferDisplayLayer` | **No** (burned in, see the pipeline doc) |
| Saved toast | Native notification + in-app toast | Yes |
| Library window | Webview | n/a |
| Settings window | Webview | n/a |
| Onboarding | Webview | n/a |

## Main flow

```
Menu bar ● / ⌘⇧L ─▶ Source picker ─▶ (Area? drag rectangle) ─▶ Countdown 3-2-1 ─▶ Recording
                                                                                   │
           Toolbar: ⏸ pause / ▶ resume · ■ stop · ⟲ restart · 🗑 cancel · cam · mic
                                                                                   │ stop
                                                        "Saving…" in toolbar ◀─────┘
                                                                   │ recording.ready
                                              Toast: "Saved to Lumos — Product demo.mp4"
                                              [Show in Finder] [Copy path] [Rename] [Open]
```

## Source picker

A compact panel near the menu bar.

- **Source tabs:** Full screen (display list with thumbnails), Window (searchable list with app icons), Area (opens the selector).
  Tools: `sources.displays`, `sources.windows`.
  - With Window chosen and the camera on, a note reads "Saving takes longer for window recordings with camera", because the camera is composited at stop.
- **Camera:** a device dropdown with a "None" option, and a live preview in the bubble, which appears as soon as the picker opens so you can frame yourself.
  Tools: `sources.cameras`, `camera.update`.
- **Mic:** a device dropdown with "None", and a live level meter.
  Tools: `sources.microphones`, `audio.update`, `audio.levels` subscription.
- **System audio toggle.**
- **Start recording** button. It shows the shortcut, and its tooltip shows the save folder ("Saves to ~/Movies/Lumos").
- The picker remembers the last choices by writing settings defaults.

## Area selector

- Dims every display. Drag a rectangle and its live W × H appears.
- Handles resize the rectangle, and dragging the inside moves it.
- Shift locks the aspect ratio. Presets: 16:9 1080p, 4:3, 1:1, 9:16 (vertical).
- Enter or "Record" confirms, and Esc cancels.
- The result becomes `source: { kind: "area", displayId, rect }`.

## Recording toolbar

```
╭──────────────────────────────────────────────────────────────────╮
│ ⠿  ● 00:42   ⏸   ■ Stop   ⟲   🗑   │  📷  🎙▁▃▅   │  ⌃           │
╰──────────────────────────────────────────────────────────────────╯
  drag  timer  pause stop  restart cancel  cam  mic+level   collapse
```

| Control | Tool | Notes |
|---|---|---|
| Timer | `recorder.tick` | Shows `elapsedUs`, which excludes paused time |
| Pause / Resume | `recording.pause` / `recording.resume` | While paused: amber "Paused" label, the timer stops, and the icon becomes ▶ |
| Stop | `recording.stop` | Primary action. The button turns into "Saving…" with progress until `recording.ready` |
| Restart | `recording.restart` | Asks inline in the toolbar: "Start over? This deletes the current take. [Start over] [Keep]" |
| Cancel | `recording.cancel` | Same inline confirmation: "Discard recording? [Discard] [Keep]" |
| Camera | `camera.update { visible }` | Hides or shows the bubble without stopping the camera track |
| Mic | `audio.update { micMuted }` | Mute keeps the track (silence) so sync stays intact. A live level meter shows input. |
| Marker | `recording.mark` | Via shortcut (⌘⇧M) or the overflow menu |
| Collapse | UI only | Shrinks to a pill showing "● 00:42" and Stop |

- Floating and always on top, it **never appears in the recording**. You can drag it anywhere, it snaps to screen edges, and it remembers its position (`ui.toolbarPosition`).
- Confirmations happen **inside the toolbar**. Never open a modal over the content being recorded.
- **States:** recording (red dot plus the text "Recording"), paused (amber plus the text "Paused"), saving (spinner plus "Saving…"), error (red banner with "Recovered/Failed — details"). State is never shown by color alone.
- If the toolbar is hidden or collapsed, the menu bar icon still shows a red dot and the elapsed time. That's the "never silent" indicator, and it's also present when recording was started from the CLI or MCP. When an agent started the recording, the toolbar names the client ("Started by Claude via MCP").

## Camera bubble

- **Shapes:** circle (default), rounded square, rectangle (16:9).
- **Sizes:** S, M and L are about 160, 240 and 360 pt in diameter. Double-click cycles the size.
- **Drag:** anywhere on screen. It snaps to the corners and edge midpoints with a 24 pt margin when released near them, and moves freely otherwise. When an area is being recorded, dragging is limited to inside that area, and snapping uses the area's corners.
- **Hover controls:** shape, size, hide, and flip (mirror).
- **Tools:** `camera.update { visible, shape, size, position, mirror, deviceId }`. Every change during recording is logged as a layout keyframe.
- **No camera permission or "None" selected:** no bubble at all. Don't show an empty circle.
- **Later (M4):** background blur, and a "camera only" full-frame mode toggled from the toolbar.

## After stop

- **Toast:** the title (editable inline, calls `library.update`), file name, size and duration, plus buttons for Show in Finder (`library.reveal`), Copy path, Open and Open in library.
- The output MP4 is already in the save folder when the toast appears. Rename moves both the MP4 and its bundle.

## Library window

- **Grid** of thumbnails with title, date, duration and a status chip (Ready, Recovered, Imported). It has search (title, markers, transcript), sort, and filters by tag or origin.
  Tools: `library.list`, `library.get`.
- **Detail view:**
  - Player.
  - Timeline with markers and trim handles. Cuts come in M2+. Tools: `edit.trim`, `edit.cut`.
  - Export menu. Tools: `export.presets`, `export.create`. The job's progress shows in the window.
  - Show in Finder, Rename, Tags, Delete. Delete goes to the Trash after confirmation (`library.delete`).
- **Drag and drop** video files onto the window to import them (`import.create`).
- **Empty state:** "No recordings yet. Press ⌘⇧L to record your screen." with a Record button.

## Settings window

| Tab | Contents | Tool |
|---|---|---|
| General | **Save folder** (path plus "Choose…", which opens the OS folder picker), file name template with live preview, keep source tracks, subfolders, after-stop behavior, launch at login | `settings.set` |
| Recording | FPS, max resolution, countdown, show cursor, highlight clicks, max duration | `settings.set` |
| Camera | Device, shape, size, mirror, resolution, live preview | `settings.set`, `camera.update` |
| Audio | Mic device with meter, system audio, gains | `settings.set` |
| Shortcuts | Rebindable global shortcuts, with conflict detection | `settings.set` |
| Automation | CLI trusted toggle; the agent switches (start recording, delete, snapshot screen); a copyable MCP config snippet; connected clients right now | `settings.set` (UI only for `automation.*`), `system.clients` |
| Storage | Library roots, disk usage, prune old bundles, rebuild index | `library.roots`, `library.reindex` |
| Advanced | Encoder (hardware or software), bitrate override, open logs, reset settings | `settings.reset` |

## Onboarding (first run)

1. **Welcome.** One sentence about what Lumos does and that recordings stay on this computer.
2. **Screen Recording.** Request it and explain that macOS may require restarting Lumos. Show the live status with a "Open System Settings" button. Tool: `permissions.request`.
3. **Camera and Microphone.** Each can be skipped. Show the live status.
4. **Save folder.** Default `~/Movies/Lumos`, with a "Choose…" button.
5. **Shortcut tip,** then a "Record your first video" button.

The onboarding comes back if a permission is later revoked, triggered by `permissions.changed`.

## Global shortcuts (defaults)

| Action | macOS | Windows/Linux |
|---|---|---|
| Start / stop | ⌘⇧L | Ctrl+Shift+L |
| Pause / resume | ⌘⇧P | Ctrl+Shift+P |
| Cancel | ⌘⇧⌫ | Ctrl+Shift+Backspace |
| Toggle camera | ⌘⇧C | Ctrl+Shift+C |
| Add marker | ⌘⇧M | Ctrl+Shift+M |

## Accessibility

- Every toolbar and bubble control can be reached with the keyboard and has a VoiceOver or Narrator label. The timer is announced on pause and resume, not every second.
- Contrast is at least 4.5:1 on the translucent toolbar in both light and dark mode.
- `prefers-reduced-motion` turns off the pulsing record dot and the countdown animation.
- State always pairs an icon with text, never color alone.

## Visual direction

- Quiet and native. The toolbar is a dark translucent pill with system fonts.
- Red is used only for record and stop. Amber is used only for paused.
- No branding inside recordings, ever.
