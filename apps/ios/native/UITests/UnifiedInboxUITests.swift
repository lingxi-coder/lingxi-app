import XCTest

final class UnifiedInboxUITests: XCTestCase {
    func testChatsStartsInInboxAndCanOpenAndReturnFromCompose() {
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchEnvironment["LINGXI_UI_TEST_INBOX"] = "1"
        app.launchEnvironment["LINGXI_ISOLATED_SIM_TEST"] = "1"
        app.launch()
        let new = app.buttons["inbox.new-conversation"]
        XCTAssertTrue(new.waitForExistence(timeout: 20))
        XCTAssertFalse(app.textViews["composer.input"].exists)
        new.tap()
        XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 10))
        let back = app.buttons["inbox.back"]
        XCTAssertTrue(back.waitForExistence(timeout: 5))
        back.tap()
        XCTAssertTrue(new.waitForExistence(timeout: 5))
    }
}
