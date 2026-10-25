import XCTest

final class WorkspaceDirectoryUITests: XCTestCase {
    func testPickerAdaptsToDarkAppearanceAndLargeText() {
        continueAfterFailure = false
        for theme in ["light", "dark"] {
            let app = XCUIApplication()
            app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
            app.launchArguments = ["-AppleLanguages", "(zh-Hans)", "-theme", theme]
            if theme == "dark" {
                app.launchArguments += ["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"]
            }
            app.launch()
            XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 15))
            app.buttons["conversation.new-chat"].tap()
            let workspace = app.buttons["newchat.workspace"]
            XCTAssertTrue(workspace.waitForExistence(timeout: 8))
            workspace.tap()
            let browse = app.buttons["workspace.browse"]
            for _ in 0..<4 {
                if browse.isHittable { break }
                app.swipeUp()
            }
            XCTAssertTrue(browse.isHittable, app.debugDescription)
            capture(app, "Workspace-Picker-\(theme)-Chinese")
            browse.tap()
            let choose = app.buttons["workspace.browser.choose"]
            XCTAssertTrue(choose.waitForExistence(timeout: 5), app.debugDescription)
            XCTAssertTrue(choose.isHittable)
            XCTAssertGreaterThanOrEqual(choose.frame.height, 44)
            XCTAssertLessThanOrEqual(choose.frame.maxY, app.frame.maxY)
            capture(app, "Workspace-Browser-\(theme)-Chinese")
            app.terminate()
        }
    }

    func testCreatePickEditUnlinkAndDeleteFolder() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchArguments = ["-AppleLanguages", "(en)", "-theme", "light"]
        app.launch()
        XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 15))
        app.buttons["conversation.new-chat"].tap()
        let workspace = app.buttons["newchat.workspace"]
        XCTAssertTrue(workspace.waitForExistence(timeout: 8), app.debugDescription)
        workspace.tap()
        XCTAssertTrue(app.descendants(matching: .any)["workspace.runtime-user"].firstMatch.waitForExistence(timeout: 5), app.debugDescription)
        app.buttons["workspace.browse"].tap()
        app.buttons["workspace.browser.actions"].tap()
        app.buttons["workspace.browser.new-folder"].tap()
        let folder = "UI-Folder-" + String(UUID().uuidString.prefix(4))
        let name = app.alerts.textFields.firstMatch
        XCTAssertTrue(name.waitForExistence(timeout: 3))
        name.tap()
        name.typeText(folder)
        app.alerts.buttons["Save"].tap()
        let row = app.buttons["workspace.folder.\(folder)"]
        XCTAssertTrue(row.waitForExistence(timeout: 5), app.debugDescription)
        capture(app, "Workspace-Folder-Browser")
        row.tap()
        XCTAssertTrue(app.descendants(matching: .any)["workspace.browser.path"].firstMatch.label.contains(folder), app.debugDescription)
        app.buttons["workspace.browser.choose"].tap()
        XCTAssertTrue(workspace.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertEqual(workspace.label, folder, app.debugDescription)

        workspace.tap()
        let saved = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@ AND label CONTAINS %@", "workspace.location.", folder)).firstMatch
        XCTAssertTrue(saved.waitForExistence(timeout: 5), app.debugDescription)
        capture(app, "Workspace-Picker-Selected-Folder")
        saved.press(forDuration: 1)
        app.buttons["Edit location"].tap()
        let label = app.textFields["workspace.location.name"]
        XCTAssertTrue(label.waitForExistence(timeout: 4), app.debugDescription)
        label.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5)).tap()
        label.typeText(" alias")
        capture(app, "Workspace-Edit-Location")
        app.buttons["workspace.location.save"].tap()
        XCTAssertTrue(saved.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(saved.label.contains("alias"), app.debugDescription)
        saved.press(forDuration: 1)
        app.buttons["Remove from list"].tap()
        XCTAssertTrue(gone(saved), app.debugDescription)

        // Removing the saved entry leaves the actual selected directory intact.
        app.buttons["workspace.add-location"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["workspace.browser.path"].firstMatch.waitForExistence(timeout: 5))
        XCTAssertTrue(app.descendants(matching: .any)["workspace.browser.path"].firstMatch.label.contains(folder))
        app.buttons["workspace.browser.choose"].tap()
        XCTAssertTrue(saved.waitForExistence(timeout: 5), app.debugDescription)
        app.buttons["workspace.default"].tap()
        XCTAssertTrue(workspace.waitForExistence(timeout: 5))
        XCTAssertNotEqual(workspace.label, folder)
        workspace.tap()
        XCTAssertTrue(saved.waitForExistence(timeout: 5))
        saved.press(forDuration: 1)
        app.buttons["Delete folder"].tap()
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "Permanently delete")).firstMatch.waitForExistence(timeout: 5), app.debugDescription)
        app.buttons["Delete folder"].tap()
        XCTAssertTrue(gone(saved), app.debugDescription)
    }

    private func gone(_ element: XCUIElement) -> Bool {
        let expectation = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: element)
        return XCTWaiter.wait(for: [expectation], timeout: 6) == .completed
    }

    private func capture(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
