import XCTest
import UIKit

final class DesktopSettingsUITests: XCTestCase {
    private func launch(theme: String = "dark") -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)", "-theme", theme]
        if UIDevice.current.userInterfaceIdiom == .pad {
            XCUIDevice.shared.orientation = .landscapeLeft
        }
        app.launch()
        let settings = app.buttons["drawer.settings"]
        if !settings.isHittable {
            let drawer = app.navigationBars.firstMatch.buttons.element(boundBy: 0)
            XCTAssertTrue(drawer.waitForExistence(timeout: 12), app.debugDescription)
            drawer.tap()
        }
        XCTAssertTrue(settings.waitForExistence(timeout: 5), app.debugDescription)
        settings.tap()
        XCTAssertTrue(app.textFields["settings.search"].waitForExistence(timeout: 5), app.debugDescription)
        return app
    }

    func testSearchKeepsDisconnectedEngineSettingsAccessible() {
        let app = launch()
        let search = app.textFields["settings.search"]
        search.tap()
        search.typeText("routing")
        let providers = app.buttons["settings.page.custom-providers"]
        XCTAssertTrue(providers.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertFalse(app.buttons["settings.page.general"].exists)
        providers.tap()
        let disconnected = app.descendants(matching: .any)["settings.engine-disconnected"]
        XCTAssertTrue(disconnected.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertFalse(app.buttons["settings.providers.save"].isEnabled)
        attach("Settings-Providers-disconnected")
    }

    func testSettingsGroupsInLightAndDark() {
        for theme in ["light", "dark"] {
            let app = launch(theme: theme)
            XCTAssertTrue(app.staticTexts["个人"].exists)
            attach("Settings-\(theme)-top")
            app.scrollViews.firstMatch.swipeUp()
            XCTAssertTrue(app.staticTexts["模型与服务"].exists || app.staticTexts["编码"].exists)
            app.scrollViews.firstMatch.swipeUp()
            XCTAssertTrue(app.staticTexts["高级"].exists)
            attach("Settings-\(theme)")
            app.terminate()
        }
    }

    func testTabletSessionInspectorPreservesTranscriptInBothThemes() throws {
        try XCTSkipUnless(UIDevice.current.userInterfaceIdiom == .pad)
        for theme in ["light", "dark"] {
            let app = launch(theme: theme)
            app.buttons["settings.close"].tap()
            let transcript = app.scrollViews["conversation.message-list"]
            XCTAssertTrue(transcript.waitForExistence(timeout: 5), app.debugDescription)
            let details = app.buttons["conversation.session-details"]
            XCTAssertTrue(details.waitForExistence(timeout: 5), app.debugDescription)
            details.tap()
            let inspector = app.descendants(matching: .any)["session-details.root"]
            XCTAssertTrue(inspector.waitForExistence(timeout: 5), app.debugDescription)
            XCTAssertTrue(transcript.isHittable, app.debugDescription)
            XCTAssertGreaterThan(inspector.frame.minX, transcript.frame.minX)
            attach("Tablet-Chat-Inspector-\(theme)")
            app.buttons["session-inspector-close"].tap()
            XCTAssertFalse(inspector.exists)
            XCTAssertTrue(app.textFields["composer.input"].isHittable)
            attach("Tablet-Chat-\(theme)")
            app.terminate()
        }
    }

    func testPhoneLargeTextKeepsComposerAndModelPickerAccessible() throws {
        try XCTSkipUnless(UIDevice.current.userInterfaceIdiom == .phone)
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)", "-theme", "light",
                                "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"]
        XCUIDevice.shared.orientation = .portrait
        app.launch()
        let input = app.textFields["composer.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 12), app.debugDescription)
        XCTAssertGreaterThanOrEqual(input.frame.height, 40)
        for id in ["composer.model", "composer.controls", "composer.voice", "composer.flow"] {
            let button = app.buttons[id]
            XCTAssertTrue(button.isHittable, id)
            XCTAssertGreaterThanOrEqual(button.frame.height, 44, id)
            XCTAssertGreaterThanOrEqual(button.frame.width, 44, id)
            XCTAssertFalse(button.label.isEmpty, id)
            XCTAssertNotEqual(button.label, id, id)
        }
        attach("Phone-LargeText-Composer")
        app.buttons["composer.model"].tap()
        let chooseModel = app.buttons["composer.model.choose"]
        XCTAssertTrue(chooseModel.waitForExistence(timeout: 3), app.debugDescription)
        chooseModel.tap()
        XCTAssertTrue(app.descendants(matching: .any)["composer.model.menu"].waitForExistence(timeout: 5), app.debugDescription)
        let search = app.searchFields.firstMatch
        XCTAssertTrue(search.isHittable, app.debugDescription)
        search.tap()
        search.typeText("sonnet")
        let result = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@ AND identifier CONTAINS %@", "composer.model.", ".row.")).firstMatch
        XCTAssertTrue(result.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(result.isHittable, app.debugDescription)
        attach("Phone-LargeText-ModelSearch")
        result.tap()
        XCTAssertTrue(app.textFields["composer.input"].waitForExistence(timeout: 5), app.debugDescription)
    }

    func testPhoneReducedMotionKeepsRunningControlsAccessible() throws {
        try XCTSkipUnless(UIDevice.current.userInterfaceIdiom == .phone)
        try XCTSkipUnless(UIAccessibility.isReduceMotionEnabled, "Enable system Reduce Motion before this scenario")
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchEnvironment["LINGXI_UI_TEST_HOLD_TURN"] = "1"
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)", "-theme", "dark"]
        app.launch()
        let input = app.textFields["composer.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 12), app.debugDescription)
        input.tap()
        input.typeText("Check reduced motion")
        let send = app.buttons["composer.send"]
        XCTAssertTrue(send.isHittable, app.debugDescription)
        send.tap()
        let stop = app.buttons["composer.stop"]
        XCTAssertTrue(stop.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(stop.isHittable)
        XCTAssertFalse(stop.label.isEmpty)
        XCTAssertGreaterThanOrEqual(stop.frame.height, 44)
        attach("Phone-ReducedMotion-Running")
        stop.tap()
    }

    private func attach(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
