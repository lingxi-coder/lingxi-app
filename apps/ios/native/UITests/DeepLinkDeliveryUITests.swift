import XCTest

/// Uses the OS URL-opening API against the existing native UI fixture. Engine
/// persistence is covered separately by EngineRoundtripTests.
final class DeepLinkDeliveryUITests: XCTestCase {
    private var app: XCUIApplication!
    private var systemOpenMonitor: NSObjectProtocol?

    override func setUp() {
        super.setUp()
        continueAfterFailure = false
        systemOpenMonitor = addUIInterruptionMonitor(withDescription: "External app URL confirmation") { alert in
            let open = alert.buttons.matching(NSPredicate(format: "label IN %@", ["Open", "打开"])).firstMatch
            guard open.exists else { return false }
            open.tap()
            return true
        }
        app = XCUIApplication()
        app.launchArguments = ["-AppleLanguages", "(zh-Hans)"]
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launch()
        dismissSystemOpenConfirmation()
        let composer = app.textViews["composer.input"]
        XCTAssertTrue(composer.waitForExistence(timeout: 15))
        composer.tap()
        if let value = composer.value as? String, !value.isEmpty {
            composer.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: value.count))
        }
    }

    override func tearDown() {
        if let systemOpenMonitor { removeUIInterruptionMonitor(systemOpenMonitor) }
        super.tearDown()
    }

    func testCustomSchemeOpensExactSettingsPage() {
        open(URL(string: "lingxi://settings/general")!)
        XCTAssertTrue(app.navigationBars["通用"].waitForExistence(timeout: 15), app.debugDescription)
        record("Custom-scheme-general-settings")
    }

    func testExternalNavigationRetainsDraftUntilConfirmed() throws {
        try XCTSkipUnless(ProcessInfo.processInfo.environment["LINGXI_EXTERNAL_URL_DRIVER"] == "1",
                          "Use scripts/run-deeplink-delivery-ui.py for warm external OS delivery")
        let composer = app.textViews["composer.input"]
        composer.tap()
        composer.typeText("Keep this external-link draft")
        XCTAssertEqual(composer.value as? String, "Keep this external-link draft")
        let originalProcess = processIdentifier()
        let marker = FileManager.default.temporaryDirectory.appendingPathComponent("lingxi-deeplink-warm-url")
        let sentMarker = marker.appendingPathExtension("sent")
        defer {
            try? FileManager.default.removeItem(at: marker)
            try? FileManager.default.removeItem(at: sentMarker)
        }
        try "lingxi://settings/general".write(to: marker, atomically: true, encoding: .utf8)
        let sent = expectation(for: NSPredicate { _, _ in
            FileManager.default.fileExists(atPath: sentMarker.path)
        }, evaluatedWith: nil)
        wait(for: [sent], timeout: 10)
        // Trigger XCTest's system interruption monitor after the OS has received
        // the link; a springboard-only alert lookup misses this system prompt.
        app.tap()
        let approval = app.alerts["Open linked destination?"]
        XCTAssertTrue(approval.waitForExistence(timeout: 30), app.debugDescription)
        XCTAssertEqual(processIdentifier(), originalProcess, "OS warm delivery must retain the process and its draft")
        record("External-link-draft-approval")
        approval.buttons["Keep editing"].tap()
        XCTAssertEqual(composer.value as? String, "Keep this external-link draft")
        XCTAssertFalse(app.navigationBars["通用"].exists)
    }

    func testImportLinkShowsInertReview() throws {
        let payload = try JSONSerialization.data(withJSONObject: [
            "name": "deeplink-review-only", "config": ["command": "review-only-command"]
        ])
        var fragment = URLComponents()
        fragment.queryItems = [URLQueryItem(name: "payload", value: String(decoding: payload, as: UTF8.self))]
        var link = URLComponents(string: "lingxi://import/mcp?scope=user")!
        link.percentEncodedFragment = fragment.percentEncodedQuery
        open(try XCTUnwrap(link.url))
        XCTAssertTrue(app.navigationBars["Import preview"].waitForExistence(timeout: 15), app.debugDescription)
        let confirm = app.buttons["deeplink.import.confirm"]
        XCTAssertTrue(confirm.exists)
        XCTAssertFalse(confirm.isEnabled, "the UI fixture has no connected engine and must never apply an import")
        record("External-link-inert-import-preview")
    }

    func testMissingRuntimeResourceShowsExplicitError() {
        open(URL(string: "lingxi://runtime/activity/deeplink-nonexistent-session")!)
        XCTAssertTrue(app.staticTexts["Local item unavailable"].waitForExistence(timeout: 15), app.debugDescription)
        record("External-link-missing-runtime-resource")
    }

    func testCurrentSessionActivityOpensItsTimeline() {
        open(URL(string: "lingxi://runtime/activity/ui-session")!)
        XCTAssertTrue(app.navigationBars["Activity"].waitForExistence(timeout: 15), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["deeplink.runtime.activity"].exists, app.debugDescription)
        XCTAssertFalse(app.staticTexts["Local item unavailable"].exists)
        record("External-link-exact-session-activity")
    }

    private func processIdentifier() -> String? {
        let description = app.debugDescription
        guard let range = description.range(of: "pid: [0-9]+", options: .regularExpression) else {
            XCTFail("Application process identifier missing")
            return nil
        }
        return String(description[range])
    }

    private func open(_ url: URL) {
        app.open(url)
        dismissSystemOpenConfirmation()
        app.activate()
    }

    private func dismissSystemOpenConfirmation(timeout: TimeInterval = 2) {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let confirmation = springboard.alerts.buttons.matching(
            NSPredicate(format: "label IN %@", ["Open", "打开"])
        ).firstMatch
        if confirmation.waitForExistence(timeout: timeout) { confirmation.tap() }
    }

    private func record(_ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
