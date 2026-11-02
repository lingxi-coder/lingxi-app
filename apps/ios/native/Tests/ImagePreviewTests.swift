import UIKit
import XCTest
@testable import LingxiCode

@MainActor
final class ImagePreviewTests: XCTestCase {
    private func makeView(width: CGFloat = 780, height: CGFloat = 1688) -> PreviewImageScrollView {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let image = UIGraphicsImageRenderer(size: CGSize(width: width, height: height), format: format).image { context in
            UIColor.systemBlue.setFill()
            context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        }
        let view = PreviewImageScrollView(image: image)
        view.reduceMotion = true
        view.frame = CGRect(x: 0, y: 0, width: 360, height: 600)
        view.layoutIfNeeded()
        return view
    }

    func testPortraitFitsAndCentersWithoutCropping() {
        let view = makeView()
        XCTAssertEqual(view.zoomScale, 600 / 1688, accuracy: 0.0001)
        XCTAssertEqual(view.imageView.frame.height, 600, accuracy: 0.1)
        XCTAssertEqual(view.contentInset.left, (360 - view.imageView.frame.width) / 2, accuracy: 0.1)
    }
    func testNativeZoomCommandsAndResizeReturnToFit() {
        let view = makeView()
        view.apply(PreviewZoomCommand(sequence: 1, action: .in))
        XCTAssertEqual(view.zoomScale, 0.5, accuracy: 0.0001)
        view.apply(PreviewZoomCommand(sequence: 2, action: .out))
        XCTAssertEqual(view.zoomScale, 0.25, accuracy: 0.0001)
        view.apply(PreviewZoomCommand(sequence: 3, action: .fit))
        XCTAssertEqual(view.zoomScale, view.fitScale, accuracy: 0.0001)
        view.frame.size = CGSize(width: 700, height: 300)
        view.layoutIfNeeded()
        XCTAssertEqual(view.zoomScale, 300 / 1688, accuracy: 0.0001)
    }
    func testSmallImagesStayAtOriginalSizeAndLongImagesRemainViewable() {
        let small = makeView(width: 40, height: 40)
        XCTAssertEqual(small.zoomScale, 1, accuracy: 0.0001)
        XCTAssertEqual(small.contentInset.left, 160, accuracy: 0.1)
        let long = makeView(width: 80, height: 8000)
        XCTAssertEqual(long.zoomScale, 0.075, accuracy: 0.0001)
        XCTAssertEqual(long.minimumZoomScale, long.fitScale, accuracy: 0.0001)
    }
}
