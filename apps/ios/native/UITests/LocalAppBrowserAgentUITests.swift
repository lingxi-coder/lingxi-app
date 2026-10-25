import XCTest

/// Verifies the app-scoped foreground handoff UI without assuming which
/// external browser the person selected in iOS Settings.
final class LocalAppBrowserAgentUITests: XCTestCase {
    private func stagedAppID() throws -> String {
        let id = ProcessInfo.processInfo.environment["LINGXI_AGENT_BROWSER_APP_ID"] ?? ""
        try XCTSkipUnless(id.range(of: #"^a[0-9a-f]{32}$"#, options: .regularExpression) != nil,
                          "Stage an isolated Local App and pass LINGXI_AGENT_BROWSER_APP_ID")
        try XCTSkipUnless(ProcessInfo.processInfo.environment["SIMULATOR_DEVICE_NAME"] == nil,
                          "Requires an iOS 26 physical device")
        return id
    }

    private func agentReviewURL(appID: String, runID: String) throws -> URL {
        var parts = URLComponents()
        parts.scheme = "lingxi"
        parts.host = "apps"
        parts.path = "/\(appID)"
        parts.queryItems = [
            URLQueryItem(name: "review", value: "agent"),
            URLQueryItem(name: "runId", value: runID),
            URLQueryItem(name: "revision", value: ProcessInfo.processInfo.environment["LINGXI_AGENT_BROWSER_REVISION"] ?? "1")
        ]
        return try XCTUnwrap(parts.url)
    }

    func testNativeForegroundActionIsScopedToAGUIRun() throws {
        continueAfterFailure = false
        let id = try stagedAppID()
        let runID = "shortcuts+browser/\(UUID().uuidString)"
        let host = XCUIApplication()
        host.activate()
        host.open(try agentReviewURL(appID: id, runID: runID))

        let start = host.buttons["开始 Agent 并返回应用"]
        XCTAssertTrue(start.waitForExistence(timeout: 20), "The app should require a foreground action before it returns to the external browser.")
        XCTAssertTrue(host.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", String(runID.prefix(8)))).firstMatch.exists)
        print("IOS_BROWSER_AGENT_HANDOFF_UI PASS ag-ui-run-id explicit-native-action system-default-browser-neutral")
    }
}
