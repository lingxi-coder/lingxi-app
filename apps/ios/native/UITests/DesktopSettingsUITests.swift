import XCTest
import UIKit

final class DesktopSettingsUITests: XCTestCase {
    private func launch(theme: String = "dark", largeText: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)", "-theme", theme]
        if largeText { app.launchArguments += ["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"] }
        if UIDevice.current.userInterfaceIdiom == .pad {
            XCUIDevice.shared.orientation = .landscapeLeft
        }
        app.launch()
        let settings = app.buttons["drawer.settings"]
        if !settings.isHittable {
            let drawer = app.buttons["drawer.open"]
            XCTAssertTrue(drawer.waitForExistence(timeout: 12), app.debugDescription)
            drawer.tap()
        }
        XCTAssertTrue(settings.waitForExistence(timeout: 5), app.debugDescription)
        settings.tap()
        XCTAssertTrue(app.searchFields.firstMatch.waitForExistence(timeout: 5), app.debugDescription)
        return app
    }

    func testSearchKeepsDisconnectedEngineSettingsAccessible() {
        let app = launch()
        let search = app.searchFields.firstMatch
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

    /// The settings home is an account card and six groups — it fits one screen.
    func testSettingsGroupsInLightAndDark() {
        for theme in ["light", "dark"] {
            let app = launch(theme: theme)
            XCTAssertTrue(app.buttons["settings.page.account"].exists, app.debugDescription)
            for group in ["models", "modes", "phone", "general", "data", "about"] {
                XCTAssertTrue(app.buttons["settings.group.\(group)"].exists, "\(group): \(app.debugDescription)")
            }
            attach("Settings-\(theme)")
            app.terminate()
        }
    }

    func testNativeModelConfigurationInLightDarkAndLargeText() {
        for (theme, largeText) in [("light", false), ("dark", false), ("light", true)] {
            let app = launch(theme: theme, largeText: largeText)
            app.buttons["settings.group.models"].tap()
            let addProvider = app.buttons["provider.add"]
            XCTAssertTrue(addProvider.waitForExistence(timeout: 8), app.debugDescription)
            XCTAssertFalse(app.buttons["settings.page.web-search"].exists)
            XCTAssertFalse(app.buttons["settings.page.web-fetch"].exists)
            XCTAssertFalse(app.staticTexts["路由与重试"].exists)
            XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "密钥仅本机加密")).firstMatch.exists)
            let advanced = app.buttons["provider.advanced"]
            XCTAssertTrue(advanced.exists)
            attach("Models-\(theme)-\(largeText ? "large" : "standard")")
            if !addProvider.isHittable { app.collectionViews.firstMatch.swipeUp() }
            addProvider.tap()
            let preset = app.buttons["provider.preset.deepseek"]
            XCTAssertTrue(preset.waitForExistence(timeout: 8), app.debugDescription)
            if !preset.isHittable { app.collectionViews.firstMatch.swipeUp() }
            attach("Providers-\(theme)-\(largeText ? "large" : "standard")")
            let presetSearch = app.searchFields.firstMatch
            presetSearch.tap()
            presetSearch.typeText("DeepSeek")
            XCTAssertTrue(preset.exists)
            XCTAssertFalse(app.buttons["provider.preset.anthropic"].exists)
            preset.tap()
            let key = app.secureTextFields["provider.api-key"]
            for _ in 0..<8 {
                if key.isHittable { break }
                app.collectionViews.allElementsBoundByIndex.last!.swipeUp()
            }
            XCTAssertTrue(key.exists, app.debugDescription)
            let visibility = app.buttons["provider.api-key.visibility"]
            XCTAssertGreaterThanOrEqual(visibility.frame.height, 44)
            attach("Provider-editor-\(theme)-\(largeText ? "large" : "standard")")
            visibility.tap()
            XCTAssertTrue(app.textFields["provider.api-key"].exists)
            visibility.tap()
            XCTAssertTrue(app.secureTextFields["provider.api-key"].exists)
            let model = app.buttons["provider.model"]
            for _ in 0..<8 {
                if model.isHittable { break }
                app.collectionViews.allElementsBoundByIndex.last!.swipeUp()
            }
            model.tap()
            XCTAssertTrue(app.searchFields.firstMatch.waitForExistence(timeout: 5))
            let modelVisibility = app.buttons["provider.model.visibility"]
            for _ in 0..<8 {
                if modelVisibility.isHittable { break }
                app.collectionViews.allElementsBoundByIndex.last!.swipeUp()
            }
            XCTAssertTrue(modelVisibility.isHittable, app.debugDescription)
            XCTAssertFalse(app.switches["provider.model.visible"].exists)
            modelVisibility.tap()
            let visible = app.switches["provider.model.visible"]
            XCTAssertTrue(visible.waitForExistence(timeout: 5), app.debugDescription)
            visible.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
            XCTAssertEqual(visible.value as? String, "0")
            app.navigationBars["显示的模型"].buttons.firstMatch.tap()
            modelVisibility.tap()
            XCTAssertEqual(visible.value as? String, "0", "Visibility edits must survive navigation")
            visible.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
            app.navigationBars["显示的模型"].buttons.firstMatch.tap()
            let disclosure = app.buttons["自定义模型 ID"]
            let custom = app.textFields["provider.model.custom"]
            for _ in 0..<8 {
                if disclosure.isHittable || custom.isHittable { break }
                app.collectionViews.allElementsBoundByIndex.last!.swipeDown()
            }
            if disclosure.isHittable { disclosure.tap() }
            for _ in 0..<8 {
                if custom.exists { break }
                app.collectionViews.allElementsBoundByIndex.last!.swipeUp()
            }
            XCTAssertTrue(custom.exists, app.debugDescription)
            attach("Model-selection-\(theme)-\(largeText ? "large" : "standard")")
            let modelSearch = app.searchFields.firstMatch
            modelSearch.tap()
            modelSearch.typeText("no-such-model-native-ui")
            XCTAssertFalse(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@", "provider.model.select.")).firstMatch.exists)
            XCTAssertTrue(custom.exists)
            app.terminate()
        }
    }

    func testModelRoutingIsInAdvancedSettings() {
        let app = launch()
        app.buttons["settings.group.models"].tap()
        let advanced = app.buttons["provider.advanced"]
        XCTAssertTrue(advanced.waitForExistence(timeout: 8))
        advanced.tap()
        XCTAssertTrue(app.staticTexts["路由与重试"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.textFields.count >= 2)
        attach("Models-advanced")
        app.terminate()
    }

    func testRemovedWebProviderSettingsAreNotSearchable() {
        let app = launch()
        let search = app.searchFields.firstMatch
        search.tap()
        search.typeText("web")
        XCTAssertFalse(app.buttons["settings.page.web-search"].exists)
        XCTAssertFalse(app.buttons["settings.page.web-fetch"].exists)
        app.terminate()
    }

    func testNativeAccountFormInLightDarkAndLargeText() {
        for (theme, largeText) in [("light", false), ("dark", false), ("light", true)] {
            let app = launch(theme: theme, largeText: largeText)
            let account = app.buttons["settings.page.account"]
            if !account.isHittable { app.collectionViews.firstMatch.swipeUp() }
            account.tap()
            XCTAssertTrue(app.staticTexts["LingXi 账户"].waitForExistence(timeout: 8), app.debugDescription)
            XCTAssertFalse(app.buttons["settings.apply-saved"].exists)
            XCTAssertFalse(app.descendants(matching: .any)["settings.engine-disconnected"].exists)
            if app.buttons["account.signout"].exists { app.buttons["account.signout"].tap() }
            let google = app.buttons["account.google"]
            XCTAssertTrue(google.waitForExistence(timeout: 8), app.debugDescription)
            XCTAssertGreaterThanOrEqual(google.frame.height, 44)
            XCTAssertFalse(app.buttons["account.apple"].exists, "Disabled provider must not appear as a working login option")
            attach("Account-\(theme)-\(largeText ? "large" : "standard")")
            let email = app.textFields["account.email"]
            if !email.isHittable { app.collectionViews.firstMatch.swipeUp() }
            XCTAssertTrue(email.exists, app.debugDescription)
            email.tap()
            email.typeText("native-ui@example.com")
            let password = app.secureTextFields["account.password"]
            XCTAssertTrue(password.exists)
            password.tap()
            password.typeText("synthetic-password")
            let submit = app.buttons["account.submit"]
            XCTAssertTrue(submit.isEnabled)
            attach("Account-keyboard-\(theme)-\(largeText ? "large" : "standard")")
            app.buttons["settings.done"].tap()
            XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 8))
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
            app.buttons["conversation.project-context"].tap()
            let details = app.buttons["session-panel.details"]
            if !details.isHittable { app.collectionViews.allElementsBoundByIndex.last!.swipeUp() }
            XCTAssertTrue(details.waitForExistence(timeout: 5), app.debugDescription)
            details.tap()
            let inspector = app.descendants(matching: .any)["session-details.root"]
            XCTAssertTrue(inspector.waitForExistence(timeout: 5), app.debugDescription)
            XCTAssertTrue(transcript.isHittable, app.debugDescription)
            XCTAssertGreaterThan(inspector.frame.minX, transcript.frame.minX)
            let userBubble = app.otherElements["conversation.message.user"].firstMatch
            XCTAssertTrue(userBubble.exists)
            XCTAssertLessThanOrEqual(userBubble.frame.maxX, inspector.frame.minX + 1)
            attach("Tablet-Chat-Inspector-\(theme)")
            app.buttons["session-inspector-close"].tap()
            XCTAssertFalse(inspector.exists)
            XCTAssertTrue(app.textViews["composer.input"].isHittable)
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
        let input = app.textViews["composer.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 12), app.debugDescription)
        XCTAssertGreaterThanOrEqual(input.frame.height, 40)
        for id in ["composer.attachment", "composer.model", "composer.controls", "composer.voice", "composer.flow"] {
            let button = app.buttons[id]
            XCTAssertTrue(button.isHittable, id)
            XCTAssertEqual(button.frame.midY, app.buttons["composer.voice"].frame.midY, accuracy: 1, id)
            XCTAssertGreaterThanOrEqual(button.frame.height, 44, id)
            XCTAssertGreaterThanOrEqual(button.frame.width, 44, id)
            // Hittability alone also passes for a partially clipped control.
            XCTAssertGreaterThanOrEqual(button.frame.minX, app.frame.minX, id)
            XCTAssertLessThanOrEqual(button.frame.maxX, app.frame.maxX, id)
            XCTAssertLessThanOrEqual(button.frame.maxY, app.frame.maxY, id)
            XCTAssertFalse(button.label.isEmpty, id)
            XCTAssertNotEqual(button.label, id, id)
        }
        // The complete toolbar stays on one row by using the icon-only model control.
        XCTAssertEqual(app.buttons["composer.model"].frame.width, 44, accuracy: 1)
        attach("Phone-LargeText-Composer")
        app.buttons["composer.model"].tap()
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
        app.buttons["composer.model.close"].tap()
        XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 5), app.debugDescription)
    }

    func testPhoneReducedMotionKeepsRunningControlsAccessible() throws {
        try XCTSkipUnless(UIDevice.current.userInterfaceIdiom == .phone)
        try XCTSkipUnless(UIAccessibility.isReduceMotionEnabled, "Enable system Reduce Motion before this scenario")
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchEnvironment["LINGXI_UI_TEST_HOLD_TURN"] = "1"
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)", "-theme", "dark"]
        app.launch()
        let input = app.textViews["composer.input"]
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
