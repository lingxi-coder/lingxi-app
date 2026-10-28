import XCTest

final class SettingsAXProbeUITests: XCTestCase {
    private func launch(compactClose: Bool = false, largeText: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)"]
        if largeText {
            app.launchArguments += ["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"]
        }
        if compactClose { app.launchArguments.append("--compact-close") }
        XCUIDevice.shared.orientation = .portrait
        app.launch()
        let settings = app.buttons["drawer.settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 10), app.debugDescription)
        settings.tap()
        return app
    }

    private func account(compactClose: Bool, largeText: Bool, marker: String) {
        let app = launch(compactClose: compactClose, largeText: largeText)
        let account = app.buttons["settings.page.account"]
        XCTAssertTrue(account.waitForExistence(timeout: 10), app.debugDescription)
        account.tap()
        let email = app.textFields["account.email"]
        for _ in 0..<8 where !email.isHittable { app.collectionViews.firstMatch.swipeUp() }
        XCTAssertTrue(email.isHittable, app.debugDescription)
        email.tap()
        email.typeText("native-ui@example.com")
        let password = app.secureTextFields["account.password"]
        XCTAssertTrue(password.exists, app.debugDescription)
        if !password.isHittable { app.collectionViews.firstMatch.swipeUp() }
        password.tap()
        password.typeText("synthetic-password")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.buttons["account.submit"].isEnabled, app.debugDescription)
        let done = app.buttons["settings.done"]
        let exists = done.waitForExistence(timeout: 5)
        let hittable = exists && done.isHittable
        print("\(marker) exists=\(exists) hittable=\(hittable)\n\(app.debugDescription)")
        guard exists && hittable else {
            XCTFail("\(marker)_UNREACHABLE: keyboard is open and settings.done is not hittable")
            return
        }
        if compactClose {
            XCTAssertEqual(done.label, largeText ? "关闭设置" : "完成", app.debugDescription)
            if largeText {
                XCTAssertGreaterThanOrEqual(done.frame.width, 44, app.debugDescription)
                XCTAssertGreaterThanOrEqual(done.frame.height, 44, app.debugDescription)
            }
        }
        done.tap()
        XCTAssertTrue(app.textFields["composer.input"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertFalse(app.buttons["settings.done"].exists, app.debugDescription)
    }

    func testAccountOriginalToolbar() {
        account(compactClose: false, largeText: true, marker: "AX_ACCOUNT_ORIGINAL")
    }

    func testAccountCompactToolbar() {
        account(compactClose: true, largeText: true, marker: "AX_ACCOUNT_COMPACT")
    }

    func testAccountStandardToolbar() {
        account(compactClose: true, largeText: false, marker: "AX_ACCOUNT_STANDARD")
    }

    private func modelVisibility() -> XCUIApplication {
        let app = launch()
        app.buttons["settings.group.models"].tap()
        let add = app.buttons["provider.add"]
        XCTAssertTrue(add.waitForExistence(timeout: 8), app.debugDescription)
        add.tap()
        let preset = app.buttons["provider.preset.deepseek"]
        XCTAssertTrue(preset.waitForExistence(timeout: 8), app.debugDescription)
        let search = app.searchFields.firstMatch
        search.tap()
        search.typeText("DeepSeek")
        XCTAssertTrue(preset.exists, app.debugDescription)
        preset.tap()
        let model = app.buttons["provider.model"]
        XCTAssertTrue(model.waitForExistence(timeout: 8), app.debugDescription)
        model.tap()
        let visibility = app.buttons["provider.model.visibility"]
        XCTAssertTrue(visibility.waitForExistence(timeout: 8), app.debugDescription)
        visibility.tap()
        let visible = app.switches["provider.model.visible"]
        XCTAssertTrue(visible.waitForExistence(timeout: 8), app.debugDescription)
        visible.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        XCTAssertEqual(visible.value as? String, "0", app.debugDescription)
        return app
    }

    func testModelTitleScopedBack() {
        let app = modelVisibility()
        let back = app.navigationBars["显示的模型"].buttons.firstMatch
        let exists = back.waitForExistence(timeout: 5)
        let hittable = exists && back.isHittable
        print("AX_MODEL_TITLE_BACK exists=\(exists) hittable=\(hittable)\n\(app.debugDescription)")
        guard exists && hittable else {
            XCTFail("AX_MODEL_TITLE_BACK_UNREACHABLE: title-scoped native back unavailable")
            return
        }
        back.tap()
        XCTAssertTrue(app.buttons["provider.model.visibility"].waitForExistence(timeout: 5), app.debugDescription)
    }

    func testModelNativeBackIdentifier() {
        let app = modelVisibility()
        let candidates = app.descendants(matching: .any).matching(identifier: "BackButton")
        XCTAssertTrue(candidates.firstMatch.waitForExistence(timeout: 5), app.debugDescription)
        let hittable = candidates.allElementsBoundByIndex.filter(\.isHittable)
        print("AX_MODEL_NATIVE_BACK hittableCount=\(hittable.count)\n\(app.debugDescription)")
        guard hittable.count == 1 else {
            XCTFail("AX_MODEL_NATIVE_BACK_AMBIGUOUS: expected one hittable native back control")
            return
        }
        hittable[0].tap()
        let visibility = app.buttons["provider.model.visibility"]
        XCTAssertTrue(visibility.waitForExistence(timeout: 5), app.debugDescription)
        visibility.tap()
        let visible = app.switches["provider.model.visible"]
        XCTAssertTrue(visible.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertEqual(visible.value as? String, "0", "Visibility edits must survive navigation")
    }
}
