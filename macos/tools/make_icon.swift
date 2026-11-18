// Builds the Mac app icon (an .icns) from the rustyfiles logo, the same SVG the Windows icon
// is made from (crates/rf-app/assets/rustyfiles.svg). macOS 26 puts icons that are not a
// rounded square into a grey tile of its own, so the logo is set on a dark rounded square
// (the brand's #050505 → #1c1c1f) on Apple's icon grid: an 824 px body in a 1024 px canvas.
//
// Usage: swift macos/tools/make_icon.swift <logo.svg> <out.icns>
import AppKit

let args = CommandLine.arguments
guard args.count == 3, let logo = NSImage(contentsOfFile: args[1]) else {
    FileHandle.standardError.write("usage: make_icon.swift <logo.svg> <out.icns>\n".data(using: .utf8)!)
    exit(1)
}
let out = URL(fileURLWithPath: args[2])

func render(_ px: Int) -> Data {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
        isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let s = CGFloat(px) / 1024
    let body = NSRect(x: 100 * s, y: 100 * s, width: 824 * s, height: 824 * s)
    let tile = NSBezierPath(roundedRect: body, xRadius: 185 * s, yRadius: 185 * s)
    NSGradient(starting: NSColor(srgbRed: 0x2a / 255, green: 0x2a / 255, blue: 0x2e / 255, alpha: 1),
               ending: NSColor(srgbRed: 0x05 / 255, green: 0x05 / 255, blue: 0x06 / 255, alpha: 1))!
        .draw(in: tile, angle: -90)
    // a hairline of light along the top edge, as Apple's own dark icons have
    NSColor(white: 1, alpha: 0.10).setStroke()
    tile.lineWidth = max(1, 2 * s)
    tile.stroke()
    // the logo, centred, filling about three quarters of the tile
    let side = 824 * s * 0.80
    logo.draw(in: NSRect(x: body.midX - side / 2, y: body.midY - side / 2, width: side, height: side))
    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

let set = FileManager.default.temporaryDirectory.appendingPathComponent("rustyfiles-\(UUID().uuidString).iconset")
try FileManager.default.createDirectory(at: set, withIntermediateDirectories: true)
for base in [16, 32, 128, 256, 512] {
    try render(base).write(to: set.appendingPathComponent("icon_\(base)x\(base).png"))
    try render(base * 2).write(to: set.appendingPathComponent("icon_\(base)x\(base)@2x.png"))
}
let p = Process()
p.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
p.arguments = ["-c", "icns", set.path, "-o", out.path]
try p.run()
p.waitUntilExit()
try? FileManager.default.removeItem(at: set)
exit(p.terminationStatus)
