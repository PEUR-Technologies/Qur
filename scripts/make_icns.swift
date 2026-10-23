// Builds Qur.iconset for iconutil (brand README: the 3D render at 96 px and
// up, the flat small symbol below). Both sit on Apple's rounded-tile grid
// (an 824 px tile in a 1024 px canvas); the mark itself is not altered.
// usage: swift make_icns.swift <brand dir> <out .iconset dir>
import AppKit

let args = CommandLine.arguments
let brand = URL(fileURLWithPath: args[1])
let out = URL(fileURLWithPath: args[2])
try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

guard let render = NSImage(contentsOf: brand.appendingPathComponent("icons/app-icon-3d-1024.png")),
      let flat = NSImage(contentsOf: brand.appendingPathComponent("png/qur-symbol-small-white-512.png")) else {
    fatalError("brand images missing")
}

func icon(_ px: Int) -> Data {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    NSGraphicsContext.current?.imageInterpolation = .high
    let s = CGFloat(px)
    let tile = NSRect(x: s * 100 / 1024, y: s * 100 / 1024, width: s * 824 / 1024, height: s * 824 / 1024)
    let path = NSBezierPath(roundedRect: tile, xRadius: s * 185 / 1024, yRadius: s * 185 / 1024)
    path.addClip()
    if px >= 96 {
        render.draw(in: tile)
    } else {
        NSColor.black.setFill()
        tile.fill()
        // The flat mark at the same optical size as the render's.
        let m = tile.width * 0.62
        flat.draw(in: NSRect(x: tile.midX - m / 2, y: tile.midY - m / 2, width: m, height: m))
    }
    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

for (name, px) in [("16x16", 16), ("16x16@2x", 32), ("32x32", 32), ("32x32@2x", 64), ("128x128", 128),
                   ("128x128@2x", 256), ("256x256", 256), ("256x256@2x", 512), ("512x512", 512), ("512x512@2x", 1024)] {
    try! icon(px).write(to: out.appendingPathComponent("icon_\(name).png"))
}
