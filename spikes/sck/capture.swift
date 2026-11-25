// M0 capture spike: ScreenCaptureKit + AVAssetWriter, segment-level API.
//
// Mirrors lumos-capture's CaptureSession: open (streams warm) -> begin segment ->
// end segment (file complete) -> close. Streams stay open between segments, so a
// pause is just "no writer"; each resume writes a new file.
//
// Build: swiftc -O -swift-version 5 -parse-as-library capture.swift -o /tmp/lumos-sck-spike
// Run:   /tmp/lumos-sck-spike <out-dir> [--segments N] [--seconds S] [--gap G] [--no-mic] [--system-audio]
// Needs Screen Recording permission for the app that runs it (Terminal, Claude, ...).

import AVFoundation
import CoreGraphics
import CoreMedia
import Foundation
import ScreenCaptureKit

func log(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }

struct Options {
    var out = ""
    var segments = 2
    var seconds = 2.0
    var gap = 1.0
    var mic = true
    var systemAudio = false
}

func parse() -> Options {
    var o = Options()
    var a = Array(CommandLine.arguments.dropFirst())
    guard !a.isEmpty else { log("usage: capture <out-dir> [--segments N] [--seconds S] [--gap G] [--no-mic] [--system-audio]"); exit(2) }
    o.out = a.removeFirst()
    while !a.isEmpty {
        switch a.removeFirst() {
        case "--segments": o.segments = Int(a.removeFirst()) ?? 2
        case "--seconds": o.seconds = Double(a.removeFirst()) ?? 2
        case "--gap": o.gap = Double(a.removeFirst()) ?? 1
        case "--no-mic": o.mic = false
        case "--system-audio": o.systemAudio = true
        default: log("unknown option"); exit(2)
        }
    }
    return o
}

/// One segment file: H.264 video plus optional AAC mic and system audio.
final class SegmentWriter {
    let writer: AVAssetWriter
    let video: AVAssetWriterInput
    var mic: AVAssetWriterInput?
    var sys: AVAssetWriterInput?
    var started = false
    var firstVideo = CMTime.invalid
    var firstMic = CMTime.invalid
    var firstSys = CMTime.invalid
    var frames = 0
    var dropped = 0

    init(url: URL, width: Int, height: Int, fps: Int, mic: Bool, system: Bool) throws {
        writer = try AVAssetWriter(outputURL: url, fileType: .mp4)
        video = AVAssetWriterInput(mediaType: .video, outputSettings: [
            AVVideoCodecKey: AVVideoCodecType.h264,
            AVVideoWidthKey: width,
            AVVideoHeightKey: height,
            AVVideoCompressionPropertiesKey: [
                AVVideoAverageBitRateKey: 6_000_000,
                AVVideoExpectedSourceFrameRateKey: fps,
                AVVideoMaxKeyFrameIntervalKey: fps * 2,
                AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
            ],
        ])
        video.expectsMediaDataInRealTime = true
        writer.add(video)
        func audio(_ ch: Int) -> AVAssetWriterInput {
            let i = AVAssetWriterInput(mediaType: .audio, outputSettings: [
                AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 48000, AVNumberOfChannelsKey: ch, AVEncoderBitRateKey: 128_000,
            ])
            i.expectsMediaDataInRealTime = true
            writer.add(i)
            return i
        }
        if mic { self.mic = audio(1) }
        if system { sys = audio(2) }
        guard writer.startWriting() else { throw writer.error ?? NSError(domain: "spike", code: 1) }
    }

    func append(_ sb: CMSampleBuffer, _ type: SCStreamOutputType) {
        let pts = CMSampleBufferGetPresentationTimeStamp(sb)
        switch type {
        case .screen:
            // Only complete frames carry pixels.
            guard let att = (CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]])?.first,
                  let raw = att[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete else { return }
            if !started {
                writer.startSession(atSourceTime: pts)
                started = true
                firstVideo = pts
            }
            if video.isReadyForMoreMediaData { video.append(sb); frames += 1 } else { dropped += 1 }
        case .microphone:
            guard started, let mic, mic.isReadyForMoreMediaData else { return }
            if !firstMic.isValid { firstMic = pts }
            mic.append(sb)
        case .audio:
            guard started, let sys, sys.isReadyForMoreMediaData else { return }
            if !firstSys.isValid { firstSys = pts }
            sys.append(sb)
        @unknown default: break
        }
    }

    func finish() async {
        video.markAsFinished()
        mic?.markAsFinished()
        sys?.markAsFinished()
        await withCheckedContinuation { c in writer.finishWriting { c.resume() } }
    }
}

final class Session: NSObject, SCStreamOutput, SCStreamDelegate {
    let q = DispatchQueue(label: "lumos.capture")
    var stream: SCStream!
    var current: SegmentWriter?
    let width: Int, height: Int, fps = 30
    let opts: Options

    init(display: SCDisplay, opts: Options) {
        self.opts = opts
        width = display.width * 2 > 3840 ? display.width : display.width * 2  // Retina pixels, capped
        height = display.height * 2 > 2160 ? display.height : display.height * 2
        super.init()
        let cfg = SCStreamConfiguration()
        cfg.width = width
        cfg.height = height
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
        cfg.pixelFormat = kCVPixelFormatType_32BGRA
        cfg.showsCursor = true
        cfg.queueDepth = 6
        cfg.capturesAudio = opts.systemAudio
        cfg.sampleRate = 48000
        cfg.channelCount = 2
        if opts.mic { cfg.captureMicrophone = true }  // macOS 15+
        let filter = SCContentFilter(display: display, excludingWindows: [])
        stream = SCStream(filter: filter, configuration: cfg, delegate: self)
        try? stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: q)
        if opts.systemAudio { try? stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: q) }
        if opts.mic { try? stream.addStreamOutput(self, type: .microphone, sampleHandlerQueue: q) }
    }

    func open() async throws { try await stream.startCapture() }
    func close() async throws { try await stream.stopCapture() }

    func beginSegment(_ url: URL) throws {
        let w = try SegmentWriter(url: url, width: width, height: height, fps: fps, mic: opts.mic, system: opts.systemAudio)
        q.sync { current = w }
    }

    func endSegment() async -> SegmentWriter? {
        let w: SegmentWriter? = q.sync { let c = current; current = nil; return c }
        await w?.finish()
        return w
    }

    func stream(_ s: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard CMSampleBufferDataIsReady(sb) else { return }
        current?.append(sb, type)
    }

    func stream(_ s: SCStream, didStopWithError error: Error) { log("stream stopped: \(error)") }
}

func seconds(_ t: CMTime) -> Double { t.isValid ? CMTimeGetSeconds(t) : .nan }

func report(_ url: URL, _ w: SegmentWriter, wall: Double) async {
    let asset = AVURLAsset(url: url)
    let dur = (try? await asset.load(.duration)).map(seconds) ?? .nan
    let tracks = (try? await asset.load(.tracks)) ?? []
    let kinds = tracks.map { $0.mediaType.rawValue }.joined(separator: ",")
    let micLag = w.firstMic.isValid ? (seconds(w.firstMic) - seconds(w.firstVideo)) * 1000 : .nan
    let bytes = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? 0
    print(String(format: "segment %@ wall=%.2fs file=%.2fs frames=%d dropped=%d bytes=%d tracks=[%@] firstMic-firstVideo=%.1fms",
                 url.lastPathComponent, wall, dur, w.frames, w.dropped, bytes, kinds, micLag))
}

@main
struct Spike {
    static func main() async {
        let o = parse()
        let dir = URL(fileURLWithPath: o.out)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

        print("macOS \(ProcessInfo.processInfo.operatingSystemVersionString)")
        let preflight = CGPreflightScreenCaptureAccess()
        print("screen-recording preflight: \(preflight)")
        if !preflight {
            _ = CGRequestScreenCaptureAccess()  // shows the system prompt once
            log("Screen Recording permission is not granted for this process's host app. Grant it in System Settings > Privacy & Security > Screen Recording, relaunch the host app, and re-run.")
            exit(4)
        }
        do {
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
            guard let display = content.displays.first else { log("no displays"); exit(5) }
            print("display \(display.displayID) \(display.width)x\(display.height) windows=\(content.windows.count)")
            let s = Session(display: display, opts: o)
            let t0 = Date()
            try await s.open()
            print(String(format: "stream opened in %.0f ms", Date().timeIntervalSince(t0) * 1000))
            try await Task.sleep(nanoseconds: 500_000_000)  // warm-up, like the countdown
            for i in 0..<o.segments {
                let url = dir.appendingPathComponent(String(format: "%03d.mp4", i))
                try? FileManager.default.removeItem(at: url)
                try s.beginSegment(url)
                let start = Date()
                try await Task.sleep(nanoseconds: UInt64(o.seconds * 1e9))
                if let w = await s.endSegment() { await report(url, w, wall: Date().timeIntervalSince(start)) }
                if i < o.segments - 1 { try await Task.sleep(nanoseconds: UInt64(o.gap * 1e9)) }  // "paused"
            }
            try await s.close()
            print("closed")
        } catch {
            log("error: \(error)")
            exit(1)
        }
    }
}
