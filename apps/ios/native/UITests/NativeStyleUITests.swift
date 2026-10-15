import XCTest

/// Exercise the real app's complete settings navigation, with deterministic
/// conversation data and without modifying credentials or remote accounts.
final class NativeStyleUITests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    func testEverySettingsPageInLightAppearance() { auditSettings(theme: "light") }
    func testEverySettingsPageInDarkAppearance() { auditSettings(theme: "dark") }

    func testPrimaryScreensInBothAppearances() {
        for theme in ["light", "dark"] {
            let app = XCUIApplication()
            app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
            app.launchArguments = ["-AppleLanguages", "(zh-Hans)", "-theme", theme]
            app.launch()
            XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 15))
            capture(app, "Chat-\(theme)")
            for (entry, root) in [
                ("drawer.entry.projects", "projects.page"),
                ("drawer.entry.apps", "apps.page"),
                ("drawer.entry.cron", "cron.root"),
                ("drawer.terminal", "terminal.root"),
            ] {
                if !app.buttons[entry].isHittable {
                    app.navigationBars.allElementsBoundByIndex.last!.buttons.element(boundBy: 0).tap()
                }
                XCTAssertTrue(app.buttons[entry].waitForExistence(timeout: 8), app.debugDescription)
                capture(app, "Sidebar-\(theme)")
                app.buttons[entry].tap()
                XCTAssertTrue(app.descendants(matching: .any)[root].firstMatch.waitForExistence(timeout: 10), app.debugDescription)
                capture(app, "\(root)-\(theme)")
                if root == "cron.root" {
                    app.buttons["Close"].tap()
                } else {
                    app.navigationBars.buttons["BackButton"].firstMatch.tap()
                }
                XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 10), app.debugDescription)
            }
            app.terminate()
        }
    }

    private func auditSettings(theme: String) {
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchArguments = ["-AppleLanguages", "(zh-Hans)", "-theme", theme]
        app.launch()
        defer { app.terminate() }
        let settings = app.buttons["drawer.settings"]
        if !settings.isHittable {
            let drawer = app.navigationBars.allElementsBoundByIndex.last!.buttons.element(boundBy: 0)
            XCTAssertTrue(drawer.waitForExistence(timeout: 15), app.debugDescription)
            drawer.tap()
        }
        XCTAssertTrue(settings.waitForExistence(timeout: 8), app.debugDescription)
        settings.tap()
        let search = app.textFields["settings.search"]
        XCTAssertTrue(search.exists || search.waitForExistence(timeout: 8), app.debugDescription)
        capture(app, "Settings-home-\(theme)")

        let pages = [
            "account", "appearance", "language", "notifications", "input",
            "archived-chats", "projects", "provider-credentials", "custom-providers",
            "voice", "permissions", "tools-agent", "skills", "mcp", "hooks", "plugins",
            "web-search", "web-fetch", "permission-mode", "dream", "linux-runtime",
            "typescript-lsp", "app-integration", "privacy", "diagnostics", "about",
        ]
        var previousQuery = ""
        for page in pages {
            XCTAssertTrue(search.exists || search.waitForExistence(timeout: 8), "Returned from \(previousQuery): \(app.debugDescription)")
            search.tap()
            if !previousQuery.isEmpty {
                search.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: previousQuery.count))
            }
            search.typeText(page)
            let identifier = page == "provider-credentials" ? "settings.provider.llm"
                : page == "app-integration" ? "settings.appIntegration" : "settings.page.\(page)"
            let row = app.buttons[identifier]
            XCTAssertTrue(row.exists || row.waitForExistence(timeout: 5), "\(page): \(app.debugDescription)")
            row.tap()
            XCTAssertTrue(app.navigationBars.allElementsBoundByIndex.last!.exists)
            XCTAssertFalse(search.exists && search.isHittable, "\(page) did not navigate")
            capture(app, "Settings-\(page)-\(theme)")
            if page == "appearance" {
                let selected = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", theme == "light" ? "浅色" : "深色")).firstMatch
                XCTAssertTrue(selected.exists, app.debugDescription)
                XCTAssertTrue(selected.isSelected, app.debugDescription)
            }
            let back = app.otherElements["settings.root"].navigationBars.buttons["BackButton"].firstMatch
            if !back.isHittable {
                let ready = XCTNSPredicateExpectation(predicate: NSPredicate(format: "hittable == true"), object: back)
                XCTAssertEqual(XCTWaiter.wait(for: [ready], timeout: 5), .completed, "\(page): \(app.debugDescription)")
            }
            back.tap()
            previousQuery = page
        }
        app.buttons["settings.close"].tap()
        XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 8))
    }

    private func capture(_ app: XCUIApplication, _ name: String) {
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = name
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }
}
