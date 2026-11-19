import XCTest

final class UnifiedInboxUITests: XCTestCase {
    func testMessagesCanOpenAndReturnFromProjects() {
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchEnvironment["LINGXI_UI_TEST_INBOX"] = "1"
        app.launchEnvironment["LINGXI_ISOLATED_SIM_TEST"] = "1"
        app.launch()
        let projects = app.buttons["inbox.open-projects"]
        XCTAssertTrue(projects.waitForExistence(timeout: 20))
        XCTAssertFalse(app.textViews["composer.input"].exists)
        projects.tap()
        XCTAssertTrue(app.buttons["drawer.entry.projects"].waitForExistence(timeout: 10))
        let close = app.buttons["drawer.close"]
        if close.isHittable { close.tap() }
        let back = app.buttons["inbox.back"]
        XCTAssertTrue(back.waitForExistence(timeout: 5))
        back.tap()
        XCTAssertTrue(projects.waitForExistence(timeout: 5))
    }
}
