import XCTest

/// Opt-in live tests. Never use an existing person's account or send account emails.
@MainActor
final class FirebaseAccountUITests: XCTestCase {
    override func setUpWithError() throws { continueAfterFailure = false }

    private func openAccount(_ app: XCUIApplication) {
        let settings = app.buttons["drawer.settings"]
        if !settings.isHittable {
            let drawer = app.navigationBars.firstMatch.buttons.element(boundBy: 0)
            XCTAssertTrue(drawer.waitForExistence(timeout: 20))
            drawer.tap()
        }
        XCTAssertTrue(settings.waitForExistence(timeout: 10))
        settings.tap()
        let account = app.buttons["settings.page.account"]
        XCTAssertTrue(account.waitForExistence(timeout: 10))
        account.tap()
        XCTAssertTrue(app.staticTexts["LingXi 账户"].waitForExistence(timeout: 10))
    }

    private func launch() throws -> XCUIApplication {
        try XCTSkipUnless(ProcessInfo.processInfo.environment["LINGXI_FIREBASE_LIVE_TESTS"] == "1")
        let app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1" // Only conversation data is mocked; Auth stays live.
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)", "-theme", "light"]
        app.launch()
        openAccount(app)
        return app
    }

    func testEmailRegistrationLoginAndProcessRestoration() throws {
        let app = try launch()
        let key = try XCTUnwrap(ProcessInfo.processInfo.environment["LINGXI_FIREBASE_API_KEY"])
        let email = "lingxi-ios-ui-\(UUID().uuidString.lowercased())@example.com"
        let password = UUID().uuidString + "aA9!"
        let api = FirebaseTestAPI(key: key, email: email, password: password)
        addTeardownBlock {
            if let token = try? await api.token() { try await api.delete(token: token) }
        }
        defer { app.terminate() }
        if app.buttons["account.signout"].exists { app.buttons["account.signout"].tap() }
        let mode = app.buttons["account.mode"]
        XCTAssertTrue(mode.waitForExistence(timeout: 10))
        mode.tap()
        enterCredentials(app, email: email, password: password)
        let profile = app.staticTexts["account.profile"]
        XCTAssertTrue(profile.waitForExistence(timeout: 40), app.debugDescription)
        XCTAssertEqual(profile.label, email)
        attach(app, name: "iOS-Firebase-registered")

        app.terminate()
        app.launch()
        openAccount(app)
        XCTAssertTrue(profile.waitForExistence(timeout: 15))
        XCTAssertEqual(profile.label, email)
        attach(app, name: "iOS-Firebase-restored")

        dismissPasswordSave(app)
        waitForEnabled(app.buttons["account.signout"])
        app.buttons["account.signout"].tap()
        XCTAssertTrue(app.textFields["account.email"].waitForExistence(timeout: 10))
        if app.buttons["account.submit"].label == "创建账户" { app.buttons["account.mode"].tap() }
        enterCredentials(app, email: email, password: "wrong-password-123")
        XCTAssertTrue(app.staticTexts["account.message"].waitForExistence(timeout: 30))
        XCTAssertEqual(app.staticTexts["account.message"].label, "邮箱或密码不正确。")
        XCTAssertFalse(profile.exists)
        app.buttons["account.mode"].tap()
        app.buttons["account.mode"].tap()
        app.secureTextFields["account.password"].tap()
        typePassword(app, password: password)
        XCTAssertTrue(profile.waitForExistence(timeout: 40), app.debugDescription)
        XCTAssertEqual(profile.label, email)
        dismissPasswordSave(app)
        waitForEnabled(app.buttons["account.signout"])
        app.buttons["account.signout"].tap()
        XCTAssertTrue(app.buttons["account.google"].waitForExistence(timeout: 10))
    }

    func testGoogleAuthorizationPageAndCancellation() throws {
        let app = try launch()
        defer { app.terminate() }
        if app.buttons["account.signout"].exists { app.buttons["account.signout"].tap() }
        let google = app.buttons["account.google"]
        XCTAssertTrue(google.waitForExistence(timeout: 10))
        if !google.isHittable { app.scrollViews.firstMatch.swipeUp() }
        google.tap()
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        if springboard.alerts.firstMatch.waitForExistence(timeout: 5) {
            let proceed = springboard.alerts.buttons.matching(NSPredicate(format: "label IN %@", ["继续", "Continue"])).firstMatch
            if proceed.exists { proceed.tap() }
        }
        XCTAssertTrue(app.webViews.firstMatch.waitForExistence(timeout: 30), app.debugDescription)
        XCTAssertTrue(app.webViews.textFields.firstMatch.waitForExistence(timeout: 45), app.debugDescription)
        attach(app, name: "iOS-Google-authorization")
        let cancel = app.buttons.matching(NSPredicate(format: "label IN %@", ["取消", "Cancel", "完成", "Done"])).allElementsBoundByIndex.first(where: { $0.isHittable }) ?? app.buttons["Cancel"]
        XCTAssertTrue(cancel.waitForExistence(timeout: 10))
        cancel.tap()
        XCTAssertTrue(app.staticTexts["account.message"].waitForExistence(timeout: 15))
        XCTAssertEqual(app.staticTexts["account.message"].label, "登录已取消。")
        XCTAssertFalse(app.staticTexts["account.profile"].exists)
    }

    private func enterCredentials(_ app: XCUIApplication, email: String, password: String) {
        let field = app.textFields["account.email"]
        field.tap()
        if let existing = field.value as? String, existing != "邮箱", !existing.isEmpty {
            field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: existing.count))
        }
        field.typeText(email)
        app.secureTextFields["account.password"].tap()
        typePassword(app, password: password)
    }
    private func typePassword(_ app: XCUIApplication, password: String) {
        // Paste the synthetic credential atomically: the simulator's strong-password
        // suggestion can intercept individual synthesized keystrokes.
        defer { UIPasteboard.general.items = [] }
        UIPasteboard.general.string = password
        let field = app.secureTextFields["account.password"]
        field.press(forDuration: 1)
        let paste = app.menuItems.matching(NSPredicate(format: "label IN %@", ["粘贴", "Paste"])).firstMatch
        if paste.waitForExistence(timeout: 3) { paste.tap() }
        else {
            let button = app.buttons.matching(NSPredicate(format: "label IN %@", ["粘贴", "Paste"])).firstMatch
            XCTAssertTrue(button.waitForExistence(timeout: 3), app.debugDescription)
            button.tap()
        }
        app.buttons["account.submit"].tap()
    }
    private func dismissPasswordSave(_ app: XCUIApplication) {
        // Synthetic accounts must not be saved to the simulator's Passwords app.
        for owner in [app, XCUIApplication(bundleIdentifier: "com.apple.springboard")] {
            let notNow = owner.buttons.matching(NSPredicate(format: "label IN %@", ["Not Now", "以后", "暂不"])).firstMatch
            if notNow.waitForExistence(timeout: 3) { notNow.tap(); return }
        }
    }

    private func waitForEnabled(_ element: XCUIElement) {
        let ready = XCTNSPredicateExpectation(predicate: NSPredicate(format: "enabled == true AND hittable == true"), object: element)
        XCTAssertEqual(XCTWaiter.wait(for: [ready], timeout: 15), .completed)
    }

    private func attach(_ app: XCUIApplication, name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}

private struct FirebaseTestAPI: Sendable {
    let key: String
    let email: String
    let password: String
    func token() async throws -> String {
        let result = try await request("signInWithPassword", body: ["email": email, "password": password, "returnSecureToken": true])
        return try XCTUnwrap(result["idToken"] as? String)
    }
    func delete(token: String) async throws { _ = try await request("delete", body: ["idToken": token]) }
    private func request(_ operation: String, body: [String: Any]) async throws -> [String: Any] {
        var request = URLRequest(url: URL(string: "https://identitytoolkit.googleapis.com/v1/accounts:\(operation)?key=\(key)")!)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await URLSession.shared.data(for: request)
        guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw URLError(.badServerResponse) }
        return try JSONSerialization.jsonObject(with: data) as? [String: Any] ?? [:]
    }
}
