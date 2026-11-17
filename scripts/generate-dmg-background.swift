// A neutral installer canvas; the two draggable icons and their names are drawn by Finder.
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers
import Foundation

guard CommandLine.arguments.count == 2 else {
    fatalError("Usage: swift scripts/generate-dmg-background.swift <output.png>")
}
let width = 600, height = 300, scale = 2
let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
let canvas = CGContext(data: nil, width: width * scale, height: height * scale,
                       bitsPerComponent: 8, bytesPerRow: 0, space: colorSpace,
                       bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
canvas.translateBy(x: 0, y: CGFloat(height * scale))
canvas.scaleBy(x: CGFloat(scale), y: -CGFloat(scale))
canvas.setFillColor(CGColor(gray: 0.96, alpha: 1))
canvas.fill(CGRect(x: 0, y: 0, width: width, height: height))
canvas.setStrokeColor(CGColor(gray: 0.6, alpha: 1))
canvas.setLineWidth(2.5)
canvas.setLineCap(.round)
canvas.setLineJoin(.round)
canvas.move(to: CGPoint(x: 269, y: 140))
canvas.addLine(to: CGPoint(x: 331, y: 140))
canvas.move(to: CGPoint(x: 319, y: 128))
canvas.addLine(to: CGPoint(x: 331, y: 140))
canvas.addLine(to: CGPoint(x: 319, y: 152))
canvas.strokePath()

let url = URL(fileURLWithPath: CommandLine.arguments[1])
try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)!
// 144 DPI gives Finder a 600 × 300 point background with a sharp Retina arrow.
CGImageDestinationAddImage(destination, canvas.makeImage()!, [kCGImagePropertyDPIWidth: 144,
    kCGImagePropertyDPIHeight: 144] as CFDictionary)
guard CGImageDestinationFinalize(destination) else { fatalError("Failed to write DMG artwork") }
print("Generated Retina DMG background: \(url.path)")
