import Foundation
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

/// Native photo selections use a bounded, oriented JPEG rather than image/* or
/// a device-only codec. File selection remains available for original bytes.
enum LocalAppPhotoEncoding {
    enum Failure: Error, LocalizedError, Sendable {
        case tooLarge, invalidImage
        nonisolated var errorDescription: String? {
            switch self {
            case .tooLarge: "Select a photo smaller than 16 MB and 64 megapixels."
            case .invalidImage: "The selected photo could not be decoded."
            }
        }
    }
    nonisolated static func jpeg(_ data: Data) throws -> Data {
        guard data.count <= 16 * 1024 * 1024 else { throw Failure.tooLarge }
        guard let source = CGImageSourceCreateWithData(data as CFData, [kCGImageSourceShouldCache: false] as CFDictionary),
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [String: Any],
              let width = (properties[kCGImagePropertyPixelWidth as String] as? NSNumber)?.doubleValue,
              let height = (properties[kCGImagePropertyPixelHeight as String] as? NSNumber)?.doubleValue,
              width > 0, height > 0 else { throw Failure.invalidImage }
        guard width <= 32768, height <= 32768, width * height <= 64_000_000 else { throw Failure.tooLarge }
        guard let thumbnail = CGImageSourceCreateThumbnailAtIndex(source, 0, [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: 2048,
            kCGImageSourceShouldCacheImmediately: true,
        ] as CFDictionary),
              let context = CGContext(data: nil, width: thumbnail.width, height: thumbnail.height,
                                      bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { throw Failure.invalidImage }
        context.setFillColor(CGColor(gray: 1, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: thumbnail.width, height: thumbnail.height))
        context.draw(thumbnail, in: CGRect(x: 0, y: 0, width: thumbnail.width, height: thumbnail.height))
        let output = NSMutableData()
        guard let image = context.makeImage(),
              let destination = CGImageDestinationCreateWithData(output, UTType.jpeg.identifier as CFString, 1, nil) else { throw Failure.invalidImage }
        CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: 0.85] as CFDictionary)
        guard CGImageDestinationFinalize(destination) else { throw Failure.invalidImage }
        return output as Data
    }
}
