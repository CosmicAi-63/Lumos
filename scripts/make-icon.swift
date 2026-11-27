// Regenerate the app icon: xcrun swiftc scripts/make-icon.swift -o /tmp/make-icon && /tmp/make-icon crates/lumos-app/icons/icon.png 512
// (If swiftc fails with "redefinition of module SwiftBridging", use the -vfsoverlay workaround in crates/lumos-capture/build.rs.)
// A flat, faceted sunset ring on a light macOS-style tile.
import CoreGraphics
import Foundation
import ImageIO

let out = CommandLine.arguments[1]
let S = CGFloat(Double(CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "512")!)
let cs = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: Int(S), height: Int(S), bitsPerComponent: 8, bytesPerRow: 0, space: cs,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
func c(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: a)
}
func grad(_ a: UInt32, _ b: UInt32) -> CGGradient {
    CGGradient(colorsSpace: cs, colors: [c(a), c(b)] as CFArray, locations: [0, 1])!
}

// Apple's icon grid: an 824/1024 body with ~22% corner radius. CoreGraphics is y-up.
let body = S * 824 / 1024
let o = (S - body) / 2
let tile = CGPath(roundedRect: CGRect(x: o, y: o + S * 0.008, width: body, height: body),
                  cornerWidth: body * 0.225, cornerHeight: body * 0.225, transform: nil)

ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -S * 0.01), blur: S * 0.025, color: c(0x5A3A20, 0.22))
ctx.addPath(tile); ctx.setFillColor(c(0xFFFFFF)); ctx.fillPath()
ctx.restoreGState()
ctx.saveGState()
ctx.addPath(tile); ctx.clip()
ctx.drawLinearGradient(grad(0xFFFFFF, 0xF7F0E8), start: CGPoint(x: 0, y: o + body), end: CGPoint(x: 0, y: o), options: [])
ctx.restoreGState()

let mid = CGPoint(x: S / 2, y: S / 2 + S * 0.008)
let R = body * 0.34, r = body * 0.14
let ring = CGMutablePath()
ring.addEllipse(in: CGRect(x: mid.x - R, y: mid.y - R, width: 2 * R, height: 2 * R))
ring.addEllipse(in: CGRect(x: mid.x - r, y: mid.y - r, width: 2 * r, height: 2 * r))

// Two flat planes split on the diagonal (top-right to bottom-left), like a cut facet.
let ul = CGMutablePath()
ul.addLines(between: [CGPoint(x: mid.x + 2 * R, y: mid.y + 2 * R), CGPoint(x: mid.x - 2 * R, y: mid.y - 2 * R),
                      CGPoint(x: mid.x - 2 * R, y: mid.y + 2 * R)])
ul.closeSubpath()
let lr = CGMutablePath()
lr.addLines(between: [CGPoint(x: mid.x + 2 * R, y: mid.y + 2 * R), CGPoint(x: mid.x - 2 * R, y: mid.y - 2 * R),
                      CGPoint(x: mid.x + 2 * R, y: mid.y - 2 * R)])
lr.closeSubpath()
let tl = CGPoint(x: mid.x - R, y: mid.y + R), br = CGPoint(x: mid.x + R, y: mid.y - R)

ctx.saveGState()
ctx.addPath(ring); ctx.clip(using: .evenOdd)
ctx.saveGState(); ctx.addPath(ul); ctx.clip()
ctx.drawLinearGradient(grad(0xFFD251, 0xFF9A2C), start: tl, end: mid, options: [.drawsAfterEndLocation])
ctx.restoreGState()
ctx.saveGState(); ctx.addPath(lr); ctx.clip()
ctx.drawLinearGradient(grad(0xFF8323, 0xE6452B), start: mid, end: br, options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
ctx.restoreGState()
ctx.restoreGState()

let img = ctx.makeImage()!
let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: out) as CFURL, "public.png" as CFString, 1, nil)!
CGImageDestinationAddImage(dest, img, nil)
CGImageDestinationFinalize(dest)
print("wrote \(out)")
