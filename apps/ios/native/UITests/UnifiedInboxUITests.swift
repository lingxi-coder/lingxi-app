import XCTest

final class UnifiedInboxUITests: XCTestCase {
    func testSearchOpensOnTapAndClosingRestoresAllMessages() {
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchEnvironment["LINGXI_UI_TEST_INBOX"] = "1"
        app.launchEnvironment["LINGXI_ISOLATED_SIM_TEST"] = "1"
        app.launchEnvironment["LINGXI_UI_TEST_INBOX_SEARCH"] = "1"
        app.launch()

        let toggle = app.buttons["inbox.toggle-search"]
        let search = app.textFields["inbox.search"]
        let alpha = app.buttons["inbox.row.team:qa:alpha"]
        let beta = app.buttons["inbox.row.team:qa:beta"]
        XCTAssertTrue(alpha.waitForExistence(timeout: 20))
        XCTAssertTrue(beta.exists)
        XCTAssertTrue(toggle.exists)
        XCTAssertFalse(search.exists)

        toggle.tap()
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        search.typeText("Alpha")
        XCTAssertTrue(alpha.exists)
        XCTAssertFalse(beta.exists)

        toggle.tap()
        XCTAssertTrue(beta.waitForExistence(timeout: 5))
        XCTAssertTrue(alpha.exists)
        XCTAssertFalse(search.exists)
    }

    func testMessagesCanOpenAndReturnFromProjects() {
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchEnvironment["LINGXI_UI_TEST_INBOX"] = "1"
        app.launchEnvironment["LINGXI_ISOLATED_SIM_TEST"] = "1"
        app.launch()
        let options = app.buttons["inbox.options"]
        let searchToggle = app.buttons["inbox.toggle-search"]
        let search = app.textFields["inbox.search"]
        XCTAssertTrue(searchToggle.waitForExistence(timeout: 20))
        XCTAssertFalse(search.exists)
        searchToggle.tap()
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        search.typeText("Example")
        searchToggle.tap()
        XCTAssertFalse(search.exists)
        let projects = app.buttons["inbox.open-projects"]
        XCTAssertTrue(options.exists)
        XCTAssertFalse(app.textViews["composer.input"].exists)
        options.tap()
        XCTAssertTrue(projects.waitForExistence(timeout: 5))
        projects.tap()
        XCTAssertTrue(app.buttons["drawer.entry.projects"].waitForExistence(timeout: 10))
        let close = app.buttons["drawer.close"]
        if close.isHittable { close.tap() }
        let back = app.buttons["inbox.back"]
        XCTAssertTrue(back.waitForExistence(timeout: 5))
        back.tap()
        XCTAssertTrue(options.waitForExistence(timeout: 5))
        XCTAssertFalse(search.exists)
    }
}
