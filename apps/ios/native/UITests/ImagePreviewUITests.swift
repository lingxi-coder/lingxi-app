import XCTest

final class ImagePreviewUITests: XCTestCase {
    private var app: XCUIApplication!
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
        XCUIDevice.shared.orientation = .portrait
        app = XCUIApplication()
        app.launchArguments = ["-AppleLanguages", "(en)"]
        app.launchEnvironment[ProductNamespace.environmentKey("UI_TESTING")] = "1"
        app.launchEnvironment[ProductNamespace.environmentKey("UI_TEST_IMAGE_PREVIEW")] = "1"
        app.launch()
        let open = app.buttons["image-preview.open"].firstMatch
        XCTAssertTrue(open.waitForExistence(timeout: 15), app.debugDescription)
        open.tap()
        XCTAssertTrue(app.buttons["image-preview.zoom-in"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(waitUntil { self.app.buttons["image-preview.zoom-in"].isEnabled })
    }
    override func tearDown() {
        XCUIDevice.shared.orientation = .portrait
        app.terminate()
        super.tearDown()
    }

    private var percent: String { app.buttons["image-preview.fit"].value as? String ?? "" }
    private func waitUntil(_ predicate: @escaping () -> Bool) -> Bool {
        XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in predicate() }, object: nil)], timeout: 8) == .completed
    }
    private func capture(_ name: String) {
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = name
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testZoomDragAndClose() {
        let initial = percent
        capture("image-preview-ios-portrait")
        app.buttons["image-preview.zoom-in"].tap()
        XCTAssertTrue(waitUntil { self.percent != initial })
        app.buttons["image-preview.zoom-out"].tap()
        app.buttons["image-preview.fit"].tap()
        XCTAssertTrue(waitUntil { self.percent == initial })
        let canvas = app.scrollViews["image-preview.canvas"]
        canvas.doubleTap()
        XCTAssertTrue(waitUntil { self.percent != initial })
        canvas.swipeUp()
        XCTAssertTrue(app.buttons["image-preview.close"].isHittable)
        // The product's Info.plist restricts iPhone/iPad to portrait. Native
        // viewport resize/fit behavior is covered by ImagePreviewTests.
        capture("image-preview-ios-zoomed")
        XCTAssertTrue(app.buttons["image-preview.zoom-in"].isHittable)
        app.buttons["image-preview.close"].tap()
        XCTAssertTrue(app.buttons["image-preview.open"].firstMatch.waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["image-preview.close"].exists)
    }

    func testSavingOpensTheSystemExporterAndCancelReturnsToPreview() {
        app.buttons["image-preview.save"].tap()
        let filename = app.textFields["DOCPicker.filenameTextField"]
        XCTAssertTrue(filename.waitForExistence(timeout: 8), app.debugDescription)
        capture("image-preview-ios-exporter")
        let start = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.09))
        start.press(forDuration: 0.1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.85)))
        XCTAssertTrue(app.buttons["image-preview.close"].waitForExistence(timeout: 5))
        app.buttons["image-preview.close"].tap()
        XCTAssertTrue(app.buttons["image-preview.open"].firstMatch.waitForExistence(timeout: 5))
    }
}
