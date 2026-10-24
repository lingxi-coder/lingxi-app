import XCTest
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers
@testable import LingxiCode

final class LocalAppPhotoEncodingTests: XCTestCase {
    private func fixture(_ type: UTType, width: Int = 64, height: Int = 32, orientation: Int = 1) throws -> Data {
        let context = try XCTUnwrap(CGContext(data: nil, width: width, height: height,
            bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue))
        context.setFillColor(CGColor(red: 1, green: 0, blue: 0, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        let data = NSMutableData()
        let destination = try XCTUnwrap(CGImageDestinationCreateWithData(data, type.identifier as CFString, 1, nil))
        CGImageDestinationAddImage(destination, try XCTUnwrap(context.makeImage()),
            [kCGImagePropertyOrientation: orientation] as CFDictionary)
        XCTAssertTrue(CGImageDestinationFinalize(destination))
        return data as Data
    }

    func testPNGSelectionBecomesBoundedJPEGWithoutOriginalMetadata() throws {
        let data = try LocalAppPhotoEncoding.jpeg(fixture(.png, width: 4096, height: 32))
        let source = try XCTUnwrap(CGImageSourceCreateWithData(data as CFData, nil))
        XCTAssertEqual(CGImageSourceGetType(source) as String?, UTType.jpeg.identifier)
        let properties = try XCTUnwrap(CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [String: Any])
        XCTAssertLessThanOrEqual(try XCTUnwrap(properties[kCGImagePropertyPixelWidth as String] as? Int), 2048)
        XCTAssertNil(properties[kCGImagePropertyGPSDictionary as String])
    }

    func testEXIFOrientationIsAppliedToOutputPixels() throws {
        let data = try LocalAppPhotoEncoding.jpeg(fixture(.jpeg, orientation: 6))
        let source = try XCTUnwrap(CGImageSourceCreateWithData(data as CFData, nil))
        let image = try XCTUnwrap(CGImageSourceCreateImageAtIndex(source, 0, nil))
        XCTAssertEqual(image.width, 32)
        XCTAssertEqual(image.height, 64)
    }

    func testInvalidAndOversizedInputIsRejected() {
        XCTAssertThrowsError(try LocalAppPhotoEncoding.jpeg(Data([0, 1, 2, 3])))
        XCTAssertThrowsError(try LocalAppPhotoEncoding.jpeg(Data(count: 16 * 1024 * 1024 + 1)))
    }
}
