import XCTest

final class WorkspaceFilesUITests: XCTestCase {
    private func launchFiles(_ theme: String = "light") -> XCUIApplication {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchEnvironment["LINGXI_UI_TEST_FILES"] = "ui-files-" + UUID().uuidString
        app.launchArguments = ["-AppleLanguages", "(en)", "-theme", theme]
        app.launch()
        XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 15))
        app.buttons["conversation.summary-menu"].tap()
        app.buttons["conversation.files"].tap()
        XCTAssertTrue(app.buttons["workspace.folder.note2.txt"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertFalse(app.buttons["workspace.browser.choose"].exists)
        return app
    }
    func testFilesEntryPreviewSearchAndGrid() {
        let app = launchFiles()
        app.buttons["workspace.folder.note2.txt"].tap()
        XCTAssertTrue(app.textViews["files.text-preview"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue((app.textViews["files.text-preview"].value as? String ?? "").contains("Read-only file preview"))
        app.buttons["files.preview.close"].tap()
        let search = app.searchFields.firstMatch
        search.tap(); search.typeText("note2")
        XCTAssertTrue(app.buttons["workspace.folder.note2.txt"].exists)
        XCTAssertFalse(app.buttons["workspace.folder.note10.txt"].exists)
        search.buttons["Clear text"].tap()
        app.buttons["close"].tap()
        app.buttons["workspace.browser.actions"].tap(); app.buttons["Grid"].tap()
        capture(app, "ios-files-grid")
        app.buttons["workspace.folder.large.txt"].tap()
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "2 MiB")).firstMatch.waitForExistence(timeout: 5))
    }
    func testCopyRenameAndDeleteConfirm() {
        let app = launchFiles("dark")
        let note = app.buttons["workspace.folder.note2.txt"]
        note.press(forDuration: 1)
        XCTAssertTrue(app.buttons["Copy"].isEnabled, app.debugDescription)
        app.buttons["Copy"].tap()
        let choose = app.buttons["workspace.browser.choose"]
        capture(app, "ios-files-copy-destination")
        XCTAssertTrue(choose.waitForExistence(timeout: 5), app.debugDescription)
        let ready = XCTNSPredicateExpectation(predicate: NSPredicate(format: "enabled == true"), object: choose)
        XCTAssertEqual(XCTWaiter.wait(for: [ready], timeout: 5), .completed)
        let targetReady = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            app.buttons.matching(identifier: "workspace.folder.target").allElementsBoundByIndex.contains { $0.isHittable && $0.isEnabled }
        }, object: nil)
        XCTAssertEqual(XCTWaiter.wait(for: [targetReady], timeout: 5), .completed)
        guard let target = app.buttons.matching(identifier: "workspace.folder.target").allElementsBoundByIndex.first(where: { $0.isHittable && $0.isEnabled }) else { XCTFail(app.debugDescription); return }
        target.tap()
        XCTAssertTrue(app.navigationBars["target"].waitForExistence(timeout: 5), app.debugDescription)
        app.buttons["workspace.browser.choose"].tap()
        app.buttons["Keep both"].tap()
        XCTAssertTrue(note.waitForExistence(timeout: 5))
        app.buttons["workspace.folder.target"].tap()
        let targetPath = XCTNSPredicateExpectation(predicate: NSPredicate(format: "label ENDSWITH %@", "/target"), object: app.scrollViews["workspace.browser.path"])
        XCTAssertEqual(XCTWaiter.wait(for: [targetPath], timeout: 5), .completed, app.debugDescription)
        XCTAssertTrue(app.buttons["workspace.folder.note2.txt"].waitForExistence(timeout: 5))
        app.buttons["workspace.browser.parent"].tap()
        let parentPath = XCTNSPredicateExpectation(predicate: NSPredicate(format: "NOT (label ENDSWITH %@)", "/target"), object: app.scrollViews["workspace.browser.path"])
        XCTAssertEqual(XCTWaiter.wait(for: [parentPath], timeout: 5), .completed, app.debugDescription)
        XCTAssertTrue(note.waitForExistence(timeout: 5), app.debugDescription)
        note.press(forDuration: 1); app.buttons["Rename"].tap()
        let field = app.alerts.textFields.firstMatch
        field.tap()
        let old = field.value as? String ?? ""
        field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: old.count) + "renamed.txt")
        app.alerts.buttons["Save"].tap()
        XCTAssertTrue(app.buttons["workspace.folder.renamed.txt"].waitForExistence(timeout: 5), app.debugDescription)
        capture(app, "ios-files-list-dark")
        let other = app.buttons["workspace.folder.note10.txt"]
        other.press(forDuration: 1); app.buttons["Delete"].tap()
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "cannot be undone")).firstMatch.waitForExistence(timeout: 3))
        app.buttons["Delete"].tap()
        let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: other)
        XCTAssertEqual(XCTWaiter.wait(for: [gone], timeout: 5), .completed)
    }
    func testNativePreviewsAndSystemImportExportPanels() {
        let app = launchFiles()
        for name in ["image.png", "document.pdf", "sound.wav", "unknown.bin"] {
            let entry = app.buttons["workspace.folder.\(name)"]
            if !entry.isHittable { app.swipeUp() }
            let ready = XCTNSPredicateExpectation(predicate: NSPredicate(format: "enabled == true"), object: entry)
            XCTAssertEqual(XCTWaiter.wait(for: [ready], timeout: 5), .completed)
            entry.tap()
            XCTAssertTrue(app.buttons["files.preview.close"].waitForExistence(timeout: 5), app.debugDescription)
            capture(app, "ios-file-preview-" + name)
            app.buttons["files.preview.close"].tap()
        }
        app.buttons["workspace.folder.note2.txt"].press(forDuration: 1)
        app.buttons["Export"].tap()
        let cancel = app.buttons["Cancel"].firstMatch
        XCTAssertTrue(cancel.waitForExistence(timeout: 8), app.debugDescription)
        cancel.tap()
        app.buttons["workspace.browser.actions"].tap()
        app.buttons["Import files"].tap()
        XCTAssertTrue(cancel.waitForExistence(timeout: 8), app.debugDescription)
        cancel.tap()
        app.buttons["workspace.folder.note2.txt"].press(forDuration: 1)
        app.buttons["Share"].tap()
        XCTAssertTrue(app.navigationBars["UIActivityContentView"].waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(app.staticTexts["Save to Files"].exists || app.staticTexts["Copy"].exists, app.debugDescription)
        capture(app, "ios-files-native-share")
    }
    private func capture(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot()); attachment.name = name; attachment.lifetime = .keepAlways; add(attachment)
    }
}
