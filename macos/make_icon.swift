import AppKit
import Foundation

// Generates a simple original app icon (coral rounded square + white R).
let out = URL(fileURLWithPath: CommandLine.arguments[1])
try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

let sizes = [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)]

for (pointSize, scale) in sizes {
    let pixels = pointSize * scale
    let px: CGFloat = CGFloat(pixels)
    guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
                                     bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
                                     isPlanar: false, colorSpaceName: .deviceRGB,
                                     bytesPerRow: 0, bitsPerPixel: 0) else { continue }
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

    let square = NSRect(x: 0, y: 0, width: px, height: px).insetBy(dx: px * 0.04, dy: px * 0.04)
    let path = NSBezierPath(roundedRect: square, xRadius: px * 0.23, yRadius: px * 0.23)
    NSColor(calibratedRed: 0.94, green: 0.35, blue: 0.16, alpha: 1).setFill()
    path.fill()

    let attrs: [NSAttributedString.Key: Any] = [
        .font: NSFont.boldSystemFont(ofSize: px * 0.62),
        .foregroundColor: NSColor.white,
    ]
    let letter = NSAttributedString(string: "R", attributes: attrs)
    let size = letter.size()
    letter.draw(at: NSPoint(x: (px - size.width) / 2, y: (px - size.height) / 2))

    NSGraphicsContext.restoreGraphicsState()
    if let data = rep.representation(using: .png, properties: [:]) {
        let suffix = scale == 2 ? "@2x" : ""
        try? data.write(to: out.appendingPathComponent("icon_\(pointSize)x\(pointSize)\(suffix).png"))
    }
}
