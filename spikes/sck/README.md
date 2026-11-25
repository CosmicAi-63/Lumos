# M0 capture spike: ScreenCaptureKit + AVAssetWriter

Standalone Swift tool that mirrors `lumos-capture`'s `CaptureSession`: open (streams warm up), begin segment, end segment (file complete), close. Streams stay open between segments, so a pause is just "no writer" and each resume writes a new file.

## Build and run

Command Line Tools 16 ships a duplicated `SwiftBridging` modulemap that breaks every `swiftc` run. The compile below hides the duplicate through a VFS overlay without touching system files.

```bash
: > /tmp/empty.modulemap
cat > /tmp/vfs.yaml <<'EOF'
{ "version": 0, "case-sensitive": "false", "roots": [ { "name": "/Library/Developer/CommandLineTools/usr/include/swift/module.modulemap", "type": "file", "external-contents": "/tmp/empty.modulemap" } ] }
EOF
xcrun swiftc -O -swift-version 5 -parse-as-library -vfsoverlay /tmp/vfs.yaml -Xcc -ivfsoverlay -Xcc /tmp/vfs.yaml capture.swift -o /tmp/lumos-sck-spike
/tmp/lumos-sck-spike /tmp/out --segments 2 --seconds 3 --gap 1 --system-audio
```

Flags: `--segments N`, `--seconds S`, `--gap G` (the simulated pause), `--no-mic`, `--system-audio`. The host app (Terminal, Claude, ...) needs Screen Recording, and Microphone for mic capture.

## Findings (macOS 15.1.1, Command Line Tools only, no Xcode)

| Question | Result |
|---|---|
| Does a Swift shim build without Xcode? | Yes, with the VFS workaround above. |
| Display capture to H.264 MP4 via `SCStream` + `AVAssetWriter` | Works. 1440x900 display captured at 2x. Segments play in ffprobe; file duration is within 50 ms of wall time. |
| Pause/resume as segments | Works. A second writer on the same open stream gives a clean second file, no dropped frames. |
| Mic (`captureMicrophone`, macOS 15+) and system audio in the same stream | Both work: `h264` + mono `aac` + stereo `aac` per segment. |
| First mic sample vs first video frame | Mic leads video by 10 to 22 ms in 2 runs, inside the 33 ms target. Only 2 runs, so this is not yet proof of sync. |
| Stream start latency | 100 ms without mic; 8 s once with mic (likely the first-time microphone prompt). Needs a second look. |
| Frame delivery | Variable: SCK sends frames when the screen changes (41 frames in 2 s on a still desktop). The writer takes that as is, but the final MP4 should be checked for players that expect constant frame rate. |

## Not yet answered

- Does the engine inside `Lumos.app` get the Screen Recording and Microphone prompts attributed to the app (spike 1)? This run used the host app's permission.
- Real audio content on the mic and system tracks, not just their presence.
- Sync after a long recording and across many pauses.
- macOS 13/14: `captureMicrophone` needs 15; older systems need `AVCaptureSession` for the mic.
- CPU and disk use at the default bitrate.
- Window and area sources, camera track, excluding Lumos' own windows.
