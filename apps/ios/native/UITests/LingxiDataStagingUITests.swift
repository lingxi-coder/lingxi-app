import XCTest

final class LingxiDataStagingUITests: XCTestCase {
    @MainActor
    func testReaderOpensTeamRecordAndTemplate() throws {
        continueAfterFailure = false
        let app = try openRecord(account: "reader")
        selectTemplate(app)
        XCTAssertTrue(app.staticTexts["Prepare shipment"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["teams.data.save"].exists)
    }

    @MainActor
    func testLocalAppWritesAndRendersInSimulatorBrowser() throws {
        #if targetEnvironment(simulator)
        guard let marker = ProcessInfo.processInfo.environment["LINGXI_DATA_STAGING_BROWSER_MARKER"],
            marker.range(of: "^[0-9a-f]{8}$", options: .regularExpression) != nil else {
            throw XCTSkip("Requires a Local App browser staging session")
        }
        continueAfterFailure = false
        // The staging driver opens the URL through the simulator's system resolver.
        let browser = XCUIApplication(bundleIdentifier: "com.apple.mobilesafari")
        let save = browser.buttons["Save staging record"]
        XCTAssertTrue(save.waitForExistence(timeout: 35), browser.debugDescription)
        save.tap()
        XCTAssertTrue(browser.staticTexts["Saved revision 1"].waitForExistence(timeout: 10), browser.debugDescription)
        XCTAssertTrue(browser.staticTexts["Packed on mobile " + marker].waitForExistence(timeout: 10), browser.debugDescription)
        #else
        throw XCTSkip("Simulator loopback staging is required")
        #endif
    }

    @MainActor
    private func openRecord(account: String) throws -> XCUIApplication {
        let env = ProcessInfo.processInfo.environment
        let names = ["LINGXI_DATA_STAGING_AUTH", "LINGXI_DATA_STAGING_FIRESTORE", "LINGXI_DATA_STAGING_API"]
        guard names.allSatisfy({ env[$0].flatMap(Int.init).map { (1024...65535).contains($0) } == true }) else {
            throw XCTSkip("Requires a running loopback Data staging instance")
        }
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_ISOLATED_SIM_TEST"] = "1"
        app.launchEnvironment["LINGXI_DATA_STAGING_ACCOUNT"] = account
        for name in names { app.launchEnvironment[name] = env[name] }
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()

        let record = app.buttons["teams.message.data.staging-message"]
        let entry = app.buttons.matching(NSPredicate(format: "identifier IN %@",
            ["teams.row.orders-chat", "teams.message.data.staging-message"])).firstMatch
        XCTAssertTrue(entry.waitForExistence(timeout: 30), app.debugDescription)
        if !record.exists { app.buttons["teams.row.orders-chat"].tap() }
        XCTAssertTrue(record.waitForExistence(timeout: 20), app.debugDescription)
        record.tap()
        XCTAssertTrue(app.buttons["teams.data.close"].waitForExistence(timeout: 10))
        return app
    }

    @MainActor
    private func selectTemplate(_ app: XCUIApplication) {
        let picker = app.descendants(matching: .any).matching(identifier: "teams.data.view").firstMatch
        XCTAssertTrue(picker.waitForExistence(timeout: 20), app.debugDescription)
        picker.tap()
        let template = app.descendants(matching: .any).matching(NSPredicate(format: "label == %@", "order-summary")).firstMatch
        XCTAssertTrue(template.waitForExistence(timeout: 10), app.debugDescription)
        template.tap()
    }
}
