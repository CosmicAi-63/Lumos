# 9. Onboarding

**Status: plan.** Nothing here is built yet. This replaces the short first-run list that was in [UI/UX](06-ui-ux.md#onboarding-first-run).

The reference is Loom's first run. It opens on a calm, polished welcome screen with soft ambient sound. The next thing you see is one clear permissions screen with a row per permission and live status. Lumos keeps that shape. It changes it in two ways: it asks only for what Lumos uses today, and it plans around how macOS really handles permissions (Screen Recording needs a relaunch, and unsigned builds can lose permissions).

## Goals

1. **Feel premium in the first 5 seconds.** Motion, sound and typography do the work. The welcome has no forms and no wall of text.
2. **Get to a working first recording.** Done means a recording can start *and* save. That needs Screen Recording plus ffmpeg, not only permissions.
3. **Never strand the user.** Every blocked state has one obvious next action, and quitting halfway resumes at the same step.
4. **Respect the principles.** No Lumos account and no telemetry. Nothing is uploaded unless the user connects Google Drive and asks for a share link (onboarding never asks). Every button calls a tool (parity rule).

## The sequence

```
 ┌──────────────┐   ┌──────────────────┐   ┌───────────────┐   ┌──────────────┐   ┌───────────────┐
 │ 1 Welcome    │──▶│ 2 Permissions    │──▶│ 3 ffmpeg      │──▶│ 4 Save folder│──▶│ 5 Ready       │
 │ ambient intro│   │ screen*, cam, mic│   │ (only if      │   │ ~/Movies/    │   │ shortcut tip, │
 │ "Get started"│   │ relaunch handled │   │  missing)     │   │ Lumos        │   │ Record first  │
 └──────────────┘   └──────────────────┘   └───────────────┘   └──────────────┘   └───────────────┘
                     * required
```

One window with a fixed size of about 720 × 1000 pt, portrait like Loom's. Steps slide horizontally inside it, and a small step indicator (5 dots) sits at the bottom. The window is not resizable and the traffic lights stay visible, so closing it is always possible. Closing early keeps the step you reached (see [State](#state-and-resume)).

### 1. Welcome

**What the user sees:**
- A full-bleed canvas. A slow sunset "bloom" fades up from the bottom: a blurred radial gradient of `--sun-yellow` → `--accent` → `--sun-coral` that drifts and breathes on a 12–16 s loop.
- At ~400 ms the Lumos icon scales in (0.92 → 1, `--ease-smooth-out`, `--duration-very-slow`) with a soft glow behind it.
- At ~900 ms the headline staggers in: **"Record your screen. Keep it yours."**
- At ~1200 ms the sub-line appears: *"Lumos records your screen, camera and voice to an MP4 on this Mac. Nothing leaves your computer."*
- At ~1600 ms the primary button fades up: **Get started** (sunset gradient, same as `.btn.primary`, larger).
- A small speaker icon sits in the top-right corner: the ambient sound toggle.

**Sound:** a soft ambient pad (warm, low, no melody, no beat) fades in over 2 s. It keeps playing quietly under the permissions step, then fades out over 1.5 s when the user reaches step 5 or closes the window. See [Ambient sound](#ambient-sound).

**Tools:** none. This step is presentation only.

### 2. Permissions

This is the Loom screen. It shows the icon (smaller), the title **"Allow access"**, one line of copy and a list of rows. Each row has an icon, a name, a one-line reason, and a status control on the right.

| Row | Required? | Reason line | Control → tool |
|---|---|---|---|
| Screen Recording | **Yes** | "To see what you record." | **Allow** → `permissions.request { permission: "screen" }` |
| Microphone | Optional | "To record your voice." | **Allow** → `permissions.request { permission: "microphone" }` |
| Camera | Optional | "For the face bubble." | **Allow** → `permissions.request { permission: "camera" }` |

Each row's status control has these states. State is never shown by color alone.

| State | Control |
|---|---|
| `notRequested` | **Allow** button |
| `granted` | ✓ green check + "Allowed" (muted text), like Loom's "Enabled" |
| `denied` / `restricted` | **Open Settings** → `open_privacy_pane(kind)` (it already opens the pane and reveals Lumos.app to drag in) |
| Screen granted but not active yet | **Quit & Reopen** (see below) |

**Rows not shown at first:**
- **Accessibility** is only needed for click highlights and cursor events, which aren't built yet (roadmap M4). Ask for it then, from inside that feature, not here.
- **Notifications:** Lumos doesn't send native notifications yet (the after-stop "toast" is in-app). Add this row only once native notifications ship.

**Live status:** poll `permissions.status` every 1 s while this step is visible and the window has focus. Also poll when the window regains focus, because the user comes back from System Settings. Rows update in place with the check-mark transition.

**The Screen Recording relaunch.** macOS doesn't let a running app use Screen Recording right after the user grants it. `CGPreflightScreenCaptureAccess()` keeps returning false until Lumos restarts. So:
1. **Allow** calls `CGRequestScreenCaptureAccess()`. The first time, macOS shows its own prompt and opens System Settings.
2. After the click, the row changes to: *"Turn on Lumos in System Settings, then come back."* with **Open Settings** and **Quit & Reopen** buttons.
3. **Quit & Reopen** saves `onboarding.step = "permissions"`, then relaunches the app (new Tauri command `relaunch`, with the same shutdown path as `quit`).
4. On relaunch, onboarding reopens at step 2 directly, with no welcome animation and no sound restart, and the Screen row shows ✓.

**Continue** is enabled once Screen Recording is granted. Camera and Mic rows that are still not granted get a quiet **Skip for now** link. Skipping them is fine: the camera and mic rows in the menu popover already handle denied states later.

### 3. ffmpeg (shown only if missing)

Without ffmpeg, the first recording fails when it saves. `system.health` already reports this as the `FFMPEG_MISSING` problem.

- Call `system.health` when entering this step. If `ffmpeg` is present, skip the step entirely: no flash, the dots just advance.
- If it's missing, show:
  - **"One more thing: Lumos needs ffmpeg to save videos."**
  - A copyable command box: `brew install ffmpeg` with a **Copy** button.
  - A link: "Don't have Homebrew? brew.sh"
  - **Check again** → `system.health`. On success, ✓ and auto-advance after 600 ms.
- Longer term (not in this plan): bundle ffmpeg in the .app so this step goes away.

### 4. Save folder

- Shows the current folder, default `~/Movies/Lumos`, as a path chip with a folder icon.
- **Choose…** → `pick_folder`, then `settings.set { patch: { saveFolder } }`.
- **Continue** keeps the default. Nothing to decide for most people.

### 5. Ready

- The bloom from step 1 returns, brighter, and the ambient sound resolves and fades out.
- Headline: **"You're all set."**
- A shortcut tip with keycaps: **⌘ ⇧ L** starts and stops a recording from anywhere.
- A hint that points up to the menu bar: *"Lumos lives in your menu bar."* A small arrow animates toward the top-right of the screen.
- **Record your first video** (primary) → closes onboarding, sets `onboarding.completed = true`, and opens the menu popover under the tray icon (the same code as a tray click).
- **Done** (secondary) → closes onboarding, sets `onboarding.completed = true`.

## Re-entry

| Trigger | What opens |
|---|---|
| First launch (`onboarding.completed` is false) | The full sequence from step 1, or from the saved step |
| Screen Recording was granted and is now denied (common after updating an unsigned build) | A short **"Lumos lost access to Screen Recording"** sheet: just step 2, with no welcome and no sound |
| User picks "Show welcome again" (Settings → General) | The full sequence, for demos and testing |

The engine has no `permissions.changed` event yet (the UI/UX doc assumes one). Until it does, the app checks `permissions.status` at launch and whenever the menu popover opens. Add the event to `events.rs` as part of this work: the engine compares status on a slow timer (5 s) and emits on change.

## Ambient sound

- **Source:** it must be original or CC0, because the repo is open source. Two options:
  - **A. Generated in Web Audio (recommended to try first).** Two or three detuned sine/triangle oscillators through a low-pass filter, with slow LFOs on filter cutoff and gain. It needs no asset, weighs zero bytes, and loops forever without a seam.
  - **B. A short recorded loop.** A 20–30 s seamless loop, AAC or Opus, under 300 KB, in `ui/sounds/`.
- **Volume:** quiet by default (about −24 dBFS), with fades in and out, never abrupt.
- **Mute:** a speaker toggle in the corner on every step, remembered in `localStorage` (UI-only state, so no tool is needed).
- **Autoplay risk:** WKWebView may block audio until the user clicks. **Spike this first.** Fallbacks, in order:
  1. Start the sound on the first click anywhere, or on **Get started**.
  2. Play it natively from Rust (`rodio`) when the onboarding window opens.
- **Reduced motion / accessibility:** `prefers-reduced-motion` turns off the drifting bloom and uses fades instead of slides. It doesn't affect sound. Sound is always muteable and never carries information.

## Visual direction

- The brand is the sunset palette from `style.css`. Typography is SF Pro Display for headlines (28–34 pt, semibold, tight tracking) and SF Pro Text for body.
- **Light, with frosted glass** (decided). It matches [UI/UX → Visual direction](06-ui-ux.md#visual-direction) ("always light"). It's built in two layers:
  1. **The window itself is frosted.** macOS vibrancy (`NSVisualEffectView`) sits behind a transparent webview, so a blurred hint of the desktop shows through the whole window. The light material is `popover` or `underWindowBackground`, and the window theme is forced to Light so the glass never turns dark when the system is in Dark Mode. On top of that sits a warm white wash: `--bg` at about 55–65% opacity, so it reads as warm and branded, not grey.
  2. **Frosted cards inside it.** The sunset bloom drifts *behind* the content. The content sits on glass panels: the permission list, the ffmpeg command box and the Ready card. Each panel uses `backdrop-filter: blur(24px) saturate(160%)` with white at about 55% opacity, a 1 px inner highlight (`rgba(255,255,255,.7)` on the top edge), a hairline border (`rgba(28,28,30,.06)`), a soft long shadow and a 16–20 px radius. The bloom's color softly bleeds through the glass, which gives the premium feel.
  - **The welcome step stays mostly open:** the icon and headline sit straight on the frosted window, not in a card, so the bloom shows fully. Cards appear from step 2.
  - **Readability:** text always sits on the glass at ≥ 55% white, never on bare bloom, so contrast stays at least 4.5:1. If the system's **Reduce transparency** setting is on (checked through an app command, since CSS can't detect it reliably in WKWebView), use solid `--panel` and `--bg` and no blur.
- Motion uses the existing tokens (`--duration-*`, `--ease-smooth-out`). Step transitions use a 24 px slide plus fade, at 350 ms.
- Contrast is at least 4.5:1 everywhere, including text placed over the bloom.

## Mechanics (what has to be built)

### Engine and helper

1. **`permissions.request` tool** (`lumos-tools`, `lumos-core`, `lumos-capture`). It's in `docs/04-tools.md` but not implemented.
   - Screen: `CGRequestScreenCaptureAccess()` (Rust FFI next to the existing `CGPreflightScreenCaptureAccess`). Returns `{ state, needsRelaunch: true }` when it isn't granted yet.
   - Camera and Mic: a new helper command `{"cmd":"requestAccess","media":"video"|"audio"}` that calls `AVCaptureDevice.requestAccess`. Today that call only happens inside the capture-start path (`helper.swift` around line 271).
   - Already denied: return `settingsUrl` and let the UI call `open_privacy_pane`.
   - Agents can't call it (`U L` clients only, as the tools doc says).
2. **`permissions.changed` event** (`events.rs`). The engine polls on a slow timer and emits on change.
3. **Onboarding settings** (`lumos-model/settings.rs`, `settings.schema.json`): `onboarding: { completed: bool, step: string|null, version: u32 }`. The `version` field lets a future change re-run onboarding once.

### Desktop app

1. **`onboarding` window** in `tauri.conf.json`: `onboarding.html`, about 720 × 1000, centered, not resizable, `visible: false`, `titleBarStyle: "Overlay"` with hidden title (traffic lights over the canvas like Loom). For the frosted glass: `transparent: true`, `theme: "Light"`, and `windowEffects: { effects: ["popover"], state: "active", radius: 12 }`. Tauri 2 supports this natively, and `macOSPrivateApi` is already on. Use `state: "active"` so the glass doesn't go flat grey when the window loses focus while the user is in System Settings. **Add `"onboarding"` to `capabilities/default.json` `windows`,** or its IPC calls will fail.
2. **Coming to the front.** Lumos runs with `ActivationPolicy::Accessory`, so a new window doesn't take focus on its own, and the macOS permission prompts can appear behind other apps. While onboarding is open, switch to `ActivationPolicy::Regular` and activate the app. Switch back to `Accessory` when it closes. (This also briefly shows a Dock icon during onboarding, which is fine and even helpful.)
3. **`relaunch` command:** the same shutdown path as `quit`, then `app.restart()`.
4. **Launch logic in `setup`:** read `settings.onboarding` and `permissions.status`, then open the full sequence, the "lost access" sheet, or nothing.
5. **New UI files:** `ui/onboarding.html`, `ui/onboarding.js`, and onboarding styles (a separate `onboarding.css`, or a section in `style.css`).

### Parity check

| Button | Tool / command |
|---|---|
| Allow (screen / mic / camera) | `permissions.request` |
| Open Settings | `open_privacy_pane` (app command, UI-only) |
| Quit & Reopen | `relaunch` (app command) |
| Check again (ffmpeg) | `system.health` |
| Choose… (folder) | `pick_folder` + `settings.set` |
| Continue / Done / Skip | `settings.set { onboarding }` |
| Record your first video | `settings.set` + open the popover |
| Sound toggle | UI-only (`localStorage`) |

## Build order

1. **Spike (½ day):** an onboarding window with Regular activation, plus a test of Web Audio autoplay in WKWebView. This decides the sound approach. In the same spike, check the frosted glass: the window vibrancy combined with `backdrop-filter` cards over the moving bloom, its frame rate on an M1, and how it looks over a dark desktop wallpaper and a busy one.
2. **Engine:** `permissions.request`, the helper `requestAccess` command, onboarding settings, and the `permissions.changed` event. Add tests with the fake backend.
3. **Permissions step and relaunch.** This is the part that has to work; ship it even if the welcome is plain.
4. **Welcome and Ready visuals, plus ambient sound.**
5. **ffmpeg and folder steps, and re-entry ("lost access" sheet, "Show welcome again").**
6. **QA on a clean macOS user account:** a first install, deny everything, grant from Settings, relaunch, update the app (unsigned) to check the lost-access path, reduced motion, and muted sound.

## Open questions

1. ~~Light or dark welcome?~~ **Decided: light, with frosted glass** (see [Visual direction](#visual-direction)).
2. **Sound source:** generated Web Audio, or a recorded loop (and who makes it)?
3. **Accessibility and Notifications rows:** leave them out until the features exist (recommended), or show them now as "optional" to match Loom?
4. **Bundle ffmpeg** so step 3 goes away? That's a larger packaging change (licensing, about 20–80 MB).
