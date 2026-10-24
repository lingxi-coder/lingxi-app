import XCTest

/// Opt-in physical Safari acceptance, with the real host and previously staged app.
/// Does not use fixture conversation flags, install keys or replace Agent output.
final class LocalAppBrowserAgentUITests: XCTestCase {
    private func stagedAppID() throws -> String {
        let id = ProcessInfo.processInfo.environment["LINGXI_AGENT_BROWSER_APP_ID"] ?? ""
        try XCTSkipUnless(id.range(of: #"^a[0-9a-f]{32}$"#, options: .regularExpression) != nil,
                          "Stage an isolated calorie app and pass LINGXI_AGENT_BROWSER_APP_ID")
        try XCTSkipUnless(ProcessInfo.processInfo.environment["SIMULATOR_DEVICE_NAME"] == nil,
                          "Requires physical iPhone")
        return id
    }
    private func record(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name; attachment.lifetime = .keepAlways; add(attachment)
    }
    func testExternalBrowserEntryAndPhotoPicker() throws {
        continueAfterFailure = false
        let id = try stagedAppID()
        let host = XCUIApplication()
        host.activate()
        host.open(try XCTUnwrap(URL(string: "lingxi://apps/" + id)))
        let open = host.buttons["返回浏览器"]
        XCTAssertTrue(open.waitForExistence(timeout: 20))
        record(host, "browser-agent-trusted-native-controls")
        open.tap()
        let safari = XCUIApplication(bundleIdentifier: "com.apple.mobilesafari")
        XCTAssertTrue(safari.wait(for: .runningForeground, timeout: 15))
        let choose = safari.buttons["选择照片"]
        XCTAssertTrue(choose.waitForExistence(timeout: 15), "Safari must show the actual Local App")
        record(safari, "browser-agent-real-safari-page")
        choose.tap()
        let photoLibrary = safari.buttons.matching(NSPredicate(format: "label IN %@", ["Photo Library", "照片图库", "图库"])).firstMatch
        let chooseFile = safari.buttons.matching(NSPredicate(format: "label IN %@", ["Choose File", "Choose Files", "选取文件", "选择文件"])).firstMatch
        XCTAssertTrue(photoLibrary.waitForExistence(timeout: 5) || chooseFile.exists,
                      "The external browser must own photo selection")
        record(safari, "browser-agent-system-photo-source-chooser")
        let cancel = safari.buttons.matching(NSPredicate(format: "label IN %@", ["Cancel", "取消"])).firstMatch
        if cancel.exists { cancel.tap() }
        XCTAssertTrue(choose.waitForExistence(timeout: 5))
        print("IOS_BROWSER_UI_ENTRY PASS native-controls external-Safari system-photo-chooser cancel-return")
    }
}
