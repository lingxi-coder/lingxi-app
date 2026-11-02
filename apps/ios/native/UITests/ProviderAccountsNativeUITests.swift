import XCTest
import UIKit

/// Uses the real app and native credential store on the dedicated test simulator.
/// No model request or account-test action is submitted.
@MainActor
final class ProviderAccountsNativeUITests: XCTestCase {
    func testSyntheticAccountCreateUpdateRestartAndDelete() throws {
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "0"
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        let name = "Native account UI \(UUID().uuidString.prefix(8))"
        app.launch()
        openAccounts(app, presetID: "anthropic")
        let add = app.buttons["provider.account.add"]
        waitEnabled(add)
        add.tap()
        let submit = app.buttons["provider.account.submit"]
        waitEnabled(submit)
        submit.tap()
        XCTAssertTrue(app.staticTexts["provider.account.add.error"].waitForExistence(timeout: 5))
        let nameField = app.textFields["provider.account.name"]
        XCTAssertTrue(nameField.waitForExistence(timeout: 8), app.debugDescription)
        nameField.tap()
        nameField.typeText(name)
        let secret = app.secureTextFields["provider.account.api-key"]
        XCTAssertTrue(secret.exists, app.debugDescription)
        secret.tap()
        secret.typeText("synthetic-native-account-ui-key")
        let confirm = app.buttons["provider.account.submit"]
        waitEnabled(confirm)
        confirm.tap()
        let account = accountRow(app, name: name)
        XCTAssertTrue(account.waitForExistence(timeout: 15), app.debugDescription)
        attach(app, name: "Native-account-created")
        account.tap()
        let enabled = app.switches["Enabled"]
        XCTAssertTrue(enabled.waitForExistence(timeout: 8), app.debugDescription)
        enabled.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        waitValue(enabled, equals: "0")
        app.terminate()

        app.launch()
        openAccounts(app, presetID: "anthropic")
        let restored = accountRow(app, name: name)
        XCTAssertTrue(restored.waitForExistence(timeout: 15), app.debugDescription)
        restored.tap()
        let restoredEnabled = app.switches["Enabled"]
        XCTAssertTrue(restoredEnabled.waitForExistence(timeout: 8), app.debugDescription)
        waitValue(restoredEnabled, equals: "0")
        let delete = app.buttons["Delete account"]
        for _ in 0..<8 {
            if delete.isHittable { break }
            app.collectionViews.firstMatch.swipeUp()
        }
        XCTAssertTrue(delete.isHittable, app.debugDescription)
        delete.tap()
        let confirmation = app.sheets.buttons["Delete account"]
        XCTAssertTrue(confirmation.waitForExistence(timeout: 5), app.debugDescription)
        confirmation.tap()
        let removed = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: accountRow(app, name: name))
        XCTAssertEqual(XCTWaiter.wait(for: [removed], timeout: 15), .completed)
        attach(app, name: "Native-account-deleted")
        app.terminate()
    }

    func testOAuthFailureRetryAndDeviceCodeHandoff() {
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_PROVIDER_ACCOUNT_COMPONENT_TEST"] = "1"
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        let add = app.buttons["provider.account.add"]
        waitEnabled(add)
        add.tap()
        let submit = app.buttons["provider.account.submit"]
        waitEnabled(submit)
        submit.tap()
        XCTAssertTrue(app.staticTexts["provider.account.add.error"].waitForExistence(timeout: 5))
        let name = app.textFields["provider.account.name"]
        name.tap()
        name.typeText("OAuth fixture")
        submit.tap()
        XCTAssertTrue(app.staticTexts["Adding account"].waitForExistence(timeout: 5))
        app.buttons["provider.account.fixture.release"].tap()
        let error = app.staticTexts["provider.account.add.error"]
        XCTAssertTrue(error.waitForExistence(timeout: 10))
        XCTAssertTrue(error.label.contains("Synthetic lost add reply"))
        waitEnabled(submit)
        submit.tap()
        let failure = XCTNSPredicateExpectation(predicate: NSPredicate(format: "label CONTAINS %@", "Synthetic authorization failure"), object: error)
        XCTAssertEqual(XCTWaiter.wait(for: [failure], timeout: 10), .completed)
        XCTAssertTrue(name.exists)
        XCTAssertFalse(name.isEnabled)
        waitEnabled(submit)
        submit.tap()
        XCTAssertTrue(app.staticTexts["provider.account.device-code"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["provider.account.verify"].exists || app.links["provider.account.verify"].exists)
        app.buttons["provider.account.oauth.cancel"].tap()
        XCTAssertTrue(accountRow(app, name: "OAuth fixture").waitForExistence(timeout: 10))
        app.terminate()
    }

    func testRetryRejectsChangedAccountIdentity() {
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_PROVIDER_ACCOUNT_COMPONENT_TEST"] = "1"
        app.launchEnvironment["LINGXI_PROVIDER_ACCOUNT_IDENTITY_CHANGE"] = "1"
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        let add = app.buttons["provider.account.add"]
        waitEnabled(add)
        add.tap()
        let name = app.textFields["provider.account.name"]
        XCTAssertTrue(name.waitForExistence(timeout: 5))
        name.tap()
        name.typeText("Identity fixture")
        let submit = app.buttons["provider.account.submit"]
        submit.tap()
        XCTAssertTrue(app.staticTexts["Adding account"].waitForExistence(timeout: 5))
        app.buttons["provider.account.fixture.release"].tap()
        let error = app.staticTexts["provider.account.add.error"]
        XCTAssertTrue(error.waitForExistence(timeout: 10))
        waitEnabled(submit)
        submit.tap()
        let failure = XCTNSPredicateExpectation(predicate: NSPredicate(format: "label CONTAINS %@", "Synthetic authorization failure"), object: error)
        XCTAssertEqual(XCTWaiter.wait(for: [failure], timeout: 10), .completed)
        waitEnabled(submit)
        submit.tap()
        let changed = XCTNSPredicateExpectation(predicate: NSPredicate(format: "label CONTAINS %@", "no longer matches"), object: error)
        XCTAssertEqual(XCTWaiter.wait(for: [changed], timeout: 10), .completed)
        XCTAssertFalse(app.staticTexts["provider.account.device-code"].exists)
        app.terminate()
    }

    /// Explicitly enabled only for the authorized physical-device acceptance run.
    /// Never reads, prints, or screenshots the live device code.
    func testPhysicalChatGPTOAuthHandoff() throws {
        try XCTSkipUnless(ProcessInfo.processInfo.environment["LINGXI_PHYSICAL_OAUTH_TEST"] == "1", "Physical authorization test is opt-in")
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "0"
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        openAccounts(app, presetID: "openai-chatgpt")
        let add = app.buttons["provider.account.add"]
        waitEnabled(add)
        add.tap()
        let name = app.textFields["provider.account.name"]
        XCTAssertTrue(name.waitForExistence(timeout: 8))
        name.tap()
        name.typeText("Terra physical " + String(UUID().uuidString.prefix(8)))
        let submit = app.buttons["provider.account.submit"]
        waitEnabled(submit)
        submit.tap()
        let flow = app.staticTexts["provider.account.device-code"]
        let appeared = flow.waitForExistence(timeout: 40)
        if appeared {
            print("PHYSICAL_OAUTH_DEVICE_FLOW_PRESENT")
        } else {
            print("PHYSICAL_OAUTH_DEVICE_FLOW_NOT_PRESENT")
        }
        // Leave the real flow visible for the account owner; never capture it.
    }

    private func openAccounts(_ app: XCUIApplication, presetID: String) {
        let welcome = app.buttons["onboarding.primaryAction"]
        if welcome.waitForExistence(timeout: 3) {
            welcome.tap()
            let entered = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: welcome)
            XCTAssertEqual(XCTWaiter.wait(for: [entered], timeout: 8), .completed)
        }
        let settings = app.buttons["drawer.settings"]
        let drawer = app.buttons["drawer.open"]
        XCTAssertTrue(drawer.waitForExistence(timeout: 30), app.debugDescription)
        drawer.tap()
        XCTAssertTrue(settings.waitForExistence(timeout: 10), app.debugDescription)
        settings.tap()
        let models = app.buttons["settings.group.models"]
        XCTAssertTrue(models.waitForExistence(timeout: 8), app.debugDescription)
        models.tap()
        let addProvider = app.buttons["provider.add"]
        XCTAssertTrue(addProvider.waitForExistence(timeout: 15), app.debugDescription)
        addProvider.tap()
        let preset = app.buttons["provider.preset.\(presetID)"]
        XCTAssertTrue(preset.waitForExistence(timeout: 15), app.debugDescription)
        if !preset.isHittable { app.collectionViews.firstMatch.swipeUp() }
        preset.tap()
        let accounts = app.buttons["provider.accounts"]
        XCTAssertTrue(accounts.waitForExistence(timeout: 10), app.debugDescription)
        accounts.tap()
    }

    private func accountRow(_ app: XCUIApplication, name: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@ AND label CONTAINS %@", "provider.account.", name)).firstMatch
    }

    private func waitEnabled(_ element: XCUIElement) {
        XCTAssertTrue(element.waitForExistence(timeout: 15))
        let ready = XCTNSPredicateExpectation(predicate: NSPredicate(format: "enabled == true"), object: element)
        XCTAssertEqual(XCTWaiter.wait(for: [ready], timeout: 15), .completed)
    }

    private func waitValue(_ element: XCUIElement, equals value: String) {
        let changed = XCTNSPredicateExpectation(predicate: NSPredicate(format: "value == %@", value), object: element)
        XCTAssertEqual(XCTWaiter.wait(for: [changed], timeout: 15), .completed)
    }

    private func attach(_ app: XCUIApplication, name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
