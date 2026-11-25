// lumos-capture-helper: native capture for the macOS backend.
//
// Rust (lumos-capture/src/macos.rs) drives this process with one JSON object per
// line on stdin and reads one JSON reply per line on stdout. Logs go to stderr.
//
//   {"cmd":"permissions"}
//   {"cmd":"devices"}
//   {"cmd":"open","dir":"<bundle>","source":{...},"fps":30,"mic":null|{"deviceId":..},"system":bool,"camera":null|{"deviceId":..}}
//   {"cmd":"begin","index":0}
//   {"cmd":"end"}
//   {"cmd":"close"}
//
// Every track of a segment is its own file, and all files of a segment share one
// time origin (the host-clock time at "begin"), so they line up without offsets.

import AVFoundation
import CoreGraphics
import CoreMedia
import Foundation
import ScreenCaptureKit

func eprint(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }

func send(_ o: [String: Any]) {
    guard let d = try? JSONSerialization.data(withJSONObject: o) else { return }
    FileHandle.standardOutput.write(d + Data("\n".utf8))
}

struct Fail: Error {
    let code: String
    let message: String
}

func hostNow() -> CMTime { CMClockGetTime(CMClockGetHostTimeClock()) }
func micros(_ t: CMTime) -> Int64 { Int64((CMTimeGetSeconds(t) * 1_000_000).rounded()) }
func even(_ v: Double) -> Int { max(2, Int(v.rounded()) / 2 * 2) }

func restamp(_ sb: CMSampleBuffer, to t: CMTime) -> CMSampleBuffer? {
    var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: t, decodeTimeStamp: .invalid)
    var out: CMSampleBuffer?
    CMSampleBufferCreateCopyWithNewTiming(allocator: nil, sampleBuffer: sb, sampleTimingEntryCount: 1, sampleTimingArray: &timing, sampleBufferOut: &out)
    return out
}

// MARK: - One track file

final class TrackFile {
    let kind: String
    let rel: String
    let url: URL
    let writer: AVAssetWriter
    let input: AVAssetWriterInput
    let t0: CMTime

    init(kind: String, rel: String, url: URL, video: Bool, width: Int, height: Int, fps: Int, channels: Int, bitrate: Int, t0: CMTime) throws {
        self.kind = kind
        self.rel = rel
        self.url = url
        self.t0 = t0
        try? FileManager.default.removeItem(at: url)
        writer = try AVAssetWriter(outputURL: url, fileType: video ? .mp4 : .m4a)
        if video {
            input = AVAssetWriterInput(mediaType: .video, outputSettings: [
                AVVideoCodecKey: AVVideoCodecType.h264,
                AVVideoWidthKey: width,
                AVVideoHeightKey: height,
                AVVideoCompressionPropertiesKey: [
                    AVVideoAverageBitRateKey: bitrate,
                    AVVideoExpectedSourceFrameRateKey: fps,
                    AVVideoMaxKeyFrameIntervalKey: fps * 2,
                    AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
                ],
            ])
        } else {
            input = AVAssetWriterInput(mediaType: .audio, outputSettings: [
                AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 48000, AVNumberOfChannelsKey: channels, AVEncoderBitRateKey: 128_000,
            ])
        }
        input.expectsMediaDataInRealTime = true
        writer.add(input)
        guard writer.startWriting() else { throw writer.error ?? Fail(code: "BACKEND_ERROR", message: "cannot start writing \(rel)") }
        writer.startSession(atSourceTime: t0)
    }

    func append(_ sb: CMSampleBuffer) {
        guard CMSampleBufferGetPresentationTimeStamp(sb) >= t0, writer.status == .writing, input.isReadyForMoreMediaData else { return }
        input.append(sb)
    }

    func finish(at end: CMTime) async {
        input.markAsFinished()
        writer.endSession(atSourceTime: end)
        await withCheckedContinuation { c in writer.finishWriting { c.resume() } }
    }

    var bytes: Int { (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? 0 }
}

final class Segment {
    let t0: CMTime
    let screen: TrackFile
    var mic: TrackFile?
    var sys: TrackFile?
    var cam: TrackFile?
    init(t0: CMTime, screen: TrackFile) {
        self.t0 = t0
        self.screen = screen
    }
    var all: [TrackFile] { [screen] + [mic, sys, cam].compactMap { $0 } }
}

// MARK: - Capture session

final class Session: NSObject, SCStreamOutput, SCStreamDelegate, AVCaptureVideoDataOutputSampleBufferDelegate {
    let q = DispatchQueue(label: "lumos.capture")
    let dir: URL
    let fps: Int
    var width = 0
    var height = 0
    var wantMic = false
    var wantSystem = false
    var wantCamera = false
    var stream: SCStream!
    var cam: AVCaptureSession?
    var current: Segment?
    var lastScreen: CMSampleBuffer?
    var streamError: String?

    init(dir: URL, fps: Int) {
        self.dir = dir
        self.fps = fps
    }

    func stream(_ s: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard sb.isValid, CMSampleBufferDataIsReady(sb) else { return }
        switch type {
        case .screen:
            guard let att = (CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]])?.first,
                  let raw = att[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete else { return }
            lastScreen = sb
            current?.screen.append(sb)
        case .microphone: current?.mic?.append(sb)
        case .audio: current?.sys?.append(sb)
        @unknown default: break
        }
    }

    func captureOutput(_ output: AVCaptureOutput, didOutput sb: CMSampleBuffer, from connection: AVCaptureConnection) {
        current?.cam?.append(sb)
    }

    func stream(_ s: SCStream, didStopWithError error: Error) {
        streamError = "\(error)"
        eprint("stream stopped: \(error)")
    }

    func path(_ kind: String, _ index: Int, _ ext: String) -> (rel: String, url: URL) {
        let rel = String(format: "tracks/%@/%03d.%@", kind, index, ext)
        return (rel, dir.appendingPathComponent(rel))
    }

    func begin(index: Int) throws {
        if let e = streamError { throw Fail(code: "BACKEND_ERROR", message: "capture stream stopped: \(e)") }
        let t0 = hostNow()
        func file(_ kind: String, _ dirName: String, video: Bool, w: Int = 0, h: Int = 0, ch: Int = 1, rate: Int = 0) throws -> TrackFile {
            let p = path(dirName, index, video ? "mp4" : "m4a")
            try FileManager.default.createDirectory(at: p.url.deletingLastPathComponent(), withIntermediateDirectories: true)
            return try TrackFile(kind: kind, rel: p.rel, url: p.url, video: video, width: w, height: h, fps: fps, channels: ch, bitrate: rate, t0: t0)
        }
        let seg = Segment(t0: t0, screen: try file("screen", "screen", video: true, w: width, h: height, rate: bitrate(width, height, fps)))
        if wantMic { seg.mic = try file("mic", "mic", video: false, ch: 1) }
        if wantSystem { seg.sys = try file("systemAudio", "system", video: false, ch: 2) }
        if wantCamera { seg.cam = try file("camera", "camera", video: true, w: 1280, h: 720, rate: 3_000_000) }
        q.sync {
            current = seg
            // A still screen sends no frames; seed the segment with the last one so it never starts empty.
            if let last = lastScreen, let r = restamp(last, to: t0) { seg.screen.append(r) }
        }
    }

    func end() async throws -> [String: Any] {
        let (segOpt, tEnd): (Segment?, CMTime) = q.sync {
            let s = current
            current = nil
            return (s, hostNow())
        }
        guard let seg = segOpt else { throw Fail(code: "INVALID_STATE", message: "no open segment") }
        for t in seg.all { await t.finish(at: tEnd) }
        for t in seg.all where t.writer.status == .failed {
            throw Fail(code: "BACKEND_ERROR", message: "writing \(t.rel) failed: \(t.writer.error.map { "\($0)" } ?? "unknown")")
        }
        return [
            "ok": true,
            "durationUs": micros(tEnd - seg.t0),
            "files": seg.all.map { [$0.kind, $0.rel] },
            "bytes": seg.all.reduce(0) { $0 + $1.bytes },
        ]
    }

    func close() async {
        cam?.stopRunning()
        try? await stream?.stopCapture()
    }
}

func bitrate(_ w: Int, _ h: Int, _ fps: Int) -> Int {
    // About 0.07 bits per pixel per frame, clamped: 1440p@30 is about 8 Mbit/s.
    min(40_000_000, max(2_000_000, Int(Double(w * h * fps) * 0.07)))
}

// MARK: - Commands

func authState(_ s: AVAuthorizationStatus) -> String {
    switch s {
    case .authorized: return "granted"
    case .denied: return "denied"
    case .restricted: return "restricted"
    case .notDetermined: return "notRequested"
    @unknown default: return "unknown"
    }
}

func permissions() -> [String: Any] {
    [
        "ok": true,
        "screen": CGPreflightScreenCaptureAccess() ? "granted" : "denied",
        "camera": authState(AVCaptureDevice.authorizationStatus(for: .video)),
        "microphone": authState(AVCaptureDevice.authorizationStatus(for: .audio)),
    ]
}

func devices() -> [String: Any] {
    func list(_ types: [AVCaptureDevice.DeviceType], _ media: AVMediaType) -> [[String: Any]] {
        let def = AVCaptureDevice.default(for: media)?.uniqueID
        return AVCaptureDevice.DiscoverySession(deviceTypes: types, mediaType: media, position: .unspecified).devices.map {
            ["id": $0.uniqueID, "name": $0.localizedName, "default": $0.uniqueID == def]
        }
    }
    return [
        "ok": true,
        "cameras": list([.builtInWideAngleCamera, .external, .continuityCamera], .video),
        "mics": list([.microphone, .external], .audio),
    ]
}

func ensureAccess(_ media: AVMediaType, _ what: String) async throws {
    switch AVCaptureDevice.authorizationStatus(for: media) {
    case .authorized: return
    case .notDetermined:
        if await AVCaptureDevice.requestAccess(for: media) { return }
        throw Fail(code: "PERMISSION_DENIED", message: "\(what) permission was not granted")
    default:
        throw Fail(code: "PERMISSION_DENIED", message: "\(what) permission is not granted (System Settings > Privacy & Security)")
    }
}

func resolveFilter(_ source: [String: Any], _ content: SCShareableContent) throws -> (SCContentFilter, String, CGRect?) {
    func display(_ id: String?) throws -> SCDisplay {
        let wanted: UInt32 = (id == nil || id == "main") ? CGMainDisplayID() : (UInt32(id!) ?? 0)
        guard let d = content.displays.first(where: { $0.displayID == wanted }) else {
            throw Fail(code: "NOT_FOUND", message: "no display '\(id ?? "main")'")
        }
        return d
    }
    switch source["kind"] as? String ?? "display" {
    case "display":
        let d = try display(source["displayId"] as? String)
        return (SCContentFilter(display: d, excludingWindows: []), "Display \(d.displayID)", nil)
    case "area":
        let d = try display(source["displayId"] as? String)
        guard let r = source["rect"] as? [String: Any], let x = r["x"] as? Double, let y = r["y"] as? Double,
              let w = r["w"] as? Double, let h = r["h"] as? Double, w > 0, h > 0 else {
            throw Fail(code: "INVALID_PARAMS", message: "area source needs rect {x,y,w,h}")
        }
        return (SCContentFilter(display: d, excludingWindows: []), "Area on display \(d.displayID)", CGRect(x: x, y: y, width: w, height: h))
    case "window":
        let id = (source["windowId"] as? NSNumber)?.uint32Value
        let app = (source["app"] as? String)?.lowercased()
        let title = (source["titleContains"] as? String)?.lowercased()
        let match = content.windows.first { w in
            if let id { return w.windowID == id }
            let appOK = app.map { w.owningApplication?.applicationName.lowercased().contains($0) ?? false } ?? true
            let titleOK = title.map { w.title?.lowercased().contains($0) ?? false } ?? true
            return (app != nil || title != nil) && appOK && titleOK && w.isOnScreen
        }
        guard let w = match else { throw Fail(code: "NOT_FOUND", message: "no matching window") }
        let name = [w.owningApplication?.applicationName, w.title].compactMap { $0 }.joined(separator: " — ")
        return (SCContentFilter(desktopIndependentWindow: w), name.isEmpty ? "Window \(w.windowID)" : name, nil)
    default:
        throw Fail(code: "INVALID_PARAMS", message: "unknown source kind")
    }
}

func open(_ cmd: [String: Any]) async throws -> (Session, [String: Any]) {
    guard CGPreflightScreenCaptureAccess() else {
        throw Fail(code: "PERMISSION_DENIED", message: "Screen Recording permission is not granted")
    }
    let dir = URL(fileURLWithPath: cmd["dir"] as? String ?? "")
    let fps = cmd["fps"] as? Int ?? 30
    let source = cmd["source"] as? [String: Any] ?? ["kind": "display", "displayId": "main"]
    let mic = cmd["mic"] as? [String: Any]
    let camera = cmd["camera"] as? [String: Any]
    let system = cmd["system"] as? Bool ?? false

    if mic != nil { try await ensureAccess(.audio, "Microphone") }
    if camera != nil { try await ensureAccess(.video, "Camera") }

    let content: SCShareableContent
    do {
        content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
    } catch {
        throw Fail(code: "PERMISSION_DENIED", message: "cannot list screen content: \(error.localizedDescription)")
    }
    let (filter, name, area) = try resolveFilter(source, content)

    let info = SCShareableContent.info(for: filter)
    let scale = Double(info.pointPixelScale)
    var wpx = (area?.width ?? info.contentRect.width) * scale
    var hpx = (area?.height ?? info.contentRect.height) * scale
    let cap = 3840.0
    if wpx > cap { hpx *= cap / wpx; wpx = cap }

    let s = Session(dir: dir, fps: fps)
    s.width = even(wpx)
    s.height = even(hpx)
    s.wantMic = mic != nil
    s.wantSystem = system
    s.wantCamera = camera != nil

    let cfg = SCStreamConfiguration()
    cfg.width = s.width
    cfg.height = s.height
    if let area { cfg.sourceRect = area }
    cfg.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
    cfg.pixelFormat = kCVPixelFormatType_32BGRA
    cfg.showsCursor = true
    cfg.queueDepth = 6
    cfg.capturesAudio = system
    cfg.sampleRate = 48000
    cfg.channelCount = 2
    if mic != nil {
        guard #available(macOS 15.0, *) else {
            throw Fail(code: "BACKEND_ERROR", message: "microphone capture needs macOS 15 (older systems: not implemented yet)")
        }
        cfg.captureMicrophone = true
        if let id = mic?["deviceId"] as? String { cfg.microphoneCaptureDeviceID = id }
    }

    s.stream = SCStream(filter: filter, configuration: cfg, delegate: s)
    try s.stream.addStreamOutput(s, type: .screen, sampleHandlerQueue: s.q)
    if system { try s.stream.addStreamOutput(s, type: .audio, sampleHandlerQueue: s.q) }
    if mic != nil, #available(macOS 15.0, *) { try s.stream.addStreamOutput(s, type: .microphone, sampleHandlerQueue: s.q) }

    var tracks: [[String: Any]] = [["kind": "screen", "codec": "h264"]]
    if mic != nil { tracks.append(["kind": "mic", "codec": "aac"]) }
    if system { tracks.append(["kind": "systemAudio", "codec": "aac"]) }

    if camera != nil {
        let wanted = camera?["deviceId"] as? String
        let dev = wanted.flatMap { AVCaptureDevice(uniqueID: $0) } ?? AVCaptureDevice.default(for: .video)
        guard let dev else { throw Fail(code: "NOT_FOUND", message: "no camera found") }
        let c = AVCaptureSession()
        c.sessionPreset = .hd1280x720
        let input = try AVCaptureDeviceInput(device: dev)
        guard c.canAddInput(input) else { throw Fail(code: "DEVICE_BUSY", message: "cannot use camera \(dev.localizedName)") }
        c.addInput(input)
        let out = AVCaptureVideoDataOutput()
        out.alwaysDiscardsLateVideoFrames = true
        out.setSampleBufferDelegate(s, queue: s.q)
        guard c.canAddOutput(out) else { throw Fail(code: "BACKEND_ERROR", message: "cannot read camera frames") }
        c.addOutput(out)
        s.cam = c
        tracks.append(["kind": "camera", "codec": "h264", "deviceName": dev.localizedName])
    }

    do {
        try await s.stream.startCapture()
    } catch {
        throw Fail(code: "BACKEND_ERROR", message: "cannot start capture: \(error.localizedDescription)")
    }
    s.cam?.startRunning()
    return (s, ["ok": true, "sourceName": name, "tracks": tracks, "width": s.width, "height": s.height])
}

@main
struct Helper {
    static func main() async {
        var session: Session?
        while let line = readLine() {
            guard let cmd = (try? JSONSerialization.jsonObject(with: Data(line.utf8))) as? [String: Any], let name = cmd["cmd"] as? String else {
                send(["ok": false, "code": "INVALID_PARAMS", "message": "not a JSON command"])
                continue
            }
            do {
                switch name {
                case "permissions": send(permissions())
                case "devices": send(devices())
                case "open":
                    if session != nil { throw Fail(code: "INVALID_STATE", message: "already open") }
                    let (s, reply) = try await open(cmd)
                    session = s
                    send(reply)
                case "begin":
                    guard let s = session else { throw Fail(code: "INVALID_STATE", message: "not open") }
                    try s.begin(index: cmd["index"] as? Int ?? 0)
                    send(["ok": true])
                case "end":
                    guard let s = session else { throw Fail(code: "INVALID_STATE", message: "not open") }
                    send(try await s.end())
                case "close", "quit":
                    await session?.close()
                    send(["ok": true])
                    exit(0)
                default:
                    send(["ok": false, "code": "METHOD_NOT_FOUND", "message": "unknown command \(name)"])
                }
            } catch let f as Fail {
                send(["ok": false, "code": f.code, "message": f.message])
            } catch {
                send(["ok": false, "code": "BACKEND_ERROR", "message": "\(error)"])
            }
        }
        // The parent went away: stop capturing and exit.
        await session?.close()
        exit(0)
    }
}
