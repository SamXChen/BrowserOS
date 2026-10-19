import AppKit
import CoreText

// Generates the 1024x1024 source PNG for the NeoB icon.
//
// The glyph is positioned from its own path bounds rather than from a text
// layout rect: "NB" has no descenders, so a centred line box sits visibly
// high. Measuring the drawn glyphs and centring that box keeps the mark
// optically centred at any font size.
//
// Rendering goes through an explicit bitmap rep instead of NSImage.lockFocus,
// which would double the output to 2048 px on a Retina backing store.

let side = 1024

guard
  let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: side,
    pixelsHigh: side,
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
  )
else {
  FileHandle.standardError.write("failed to allocate bitmap\n".data(using: .utf8)!)
  exit(1)
}
bitmap.size = NSSize(width: side, height: side)

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
guard let context = NSGraphicsContext.current?.cgContext else {
  FileHandle.standardError.write("failed to create graphics context\n".data(using: .utf8)!)
  exit(1)
}

let tile = CGRect(x: 0, y: 0, width: side, height: side)
context.setFillColor(red: 0.09, green: 0.36, blue: 0.88, alpha: 1)
context.addPath(CGPath(roundedRect: tile, cornerWidth: 224, cornerHeight: 224, transform: nil))
context.fillPath()

let attributes: [NSAttributedString.Key: Any] = [
  .font: NSFont.boldSystemFont(ofSize: 440),
  .foregroundColor: NSColor.white,
]
let line = CTLineCreateWithAttributedString(
  NSAttributedString(string: "NB", attributes: attributes)
)
let glyphs = CTLineGetBoundsWithOptions(line, .useGlyphPathBounds)

context.textMatrix = .identity
context.textPosition = CGPoint(
  x: (CGFloat(side) - glyphs.width) / 2 - glyphs.minX,
  y: (CGFloat(side) - glyphs.height) / 2 - glyphs.minY
)
CTLineDraw(line, context)

NSGraphicsContext.restoreGraphicsState()

guard let data = bitmap.representation(using: .png, properties: [:]) else {
  FileHandle.standardError.write("failed to render icon\n".data(using: .utf8)!)
  exit(1)
}

try data.write(to: URL(fileURLWithPath: "/tmp/NeoBIcon_1024.png"))
print("wrote /tmp/NeoBIcon_1024.png (\(side)x\(side))")
