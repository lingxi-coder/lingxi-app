import XCTest

final class LingxiCodeUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        super.setUp()
        continueAfterFailure = false
        app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launch()
        XCTAssertTrue(app.buttons["打开抽屉"].waitForExistence(timeout: 12))
    }

    func testStructuredShellCardOpensProjectTerminal() {
        XCTAssertTrue(app.staticTexts["Agent 运行"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["/workspace/ui-test"].exists)

        let openTerminal = app.buttons["在终端打开"]
        XCTAssertTrue(openTerminal.waitForExistence(timeout: 5))
        openTerminal.tap()

        XCTAssertTrue(app.navigationBars["终端"].waitForExistence(timeout: 8))
        app.buttons["关闭"].tap()
        XCTAssertTrue(app.buttons["打开抽屉"].waitForExistence(timeout: 5))
    }

    func testDrawerTabsCreateAndSwitchProject() {
        openDrawer()
        XCTAssertTrue(app.buttons["drawer.tab.chats"].exists)
        XCTAssertTrue(app.buttons["drawer.tab.projects"].exists)
        XCTAssertTrue(app.buttons["drawer.tab.crons"].exists)

        app.buttons["drawer.tab.projects"].tap()
        let create = app.buttons["drawer.project.create"]
        XCTAssertTrue(create.waitForExistence(timeout: 5))
        create.tap()
        let createLocalProject = app.buttons["新建本地项目"]
        XCTAssertTrue(createLocalProject.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(createLocalProject, timeout: 5), app.debugDescription)
        createLocalProject.tap()

        let field = app.textFields["项目名称"]
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        let projectName = "UI-项目-\(UUID().uuidString.prefix(6))"
        field.tap()
        field.typeText(projectName)
        app.buttons["创建"].tap()

        XCTAssertTrue(app.buttons["打开抽屉"].waitForExistence(timeout: 10))
        openDrawer()
        let projectScope = app.buttons["drawer.scope.project.\(projectName)"]
        XCTAssertTrue(projectScope.waitForExistence(timeout: 5))
        XCTAssertEqual(projectScope.value as? String, "当前项目")

        app.buttons["drawer.scope.global"].tap()
        XCTAssertTrue(app.buttons["打开抽屉"].waitForExistence(timeout: 10))
        openDrawer()
        let persistedProjectScope = app.buttons["drawer.scope.project.\(projectName)"]
        XCTAssertTrue(persistedProjectScope.waitForExistence(timeout: 5))
        persistedProjectScope.tap()
        XCTAssertTrue(app.buttons["打开抽屉"].waitForExistence(timeout: 10))
    }

    func testProviderAndCronManagementEntryPoints() {
        openDrawer()
        app.buttons["drawer.settings"].tap()
        XCTAssertTrue(app.staticTexts["设置"].waitForExistence(timeout: 5))
        // Let the drawer removal and settings sheet presentation finish before
        // targeting a control at the same screen coordinates.
        Thread.sleep(forTimeInterval: 1)

        let llmProvider = app.buttons["settings.provider.llm"]
        XCTAssertTrue(llmProvider.waitForExistence(timeout: 5))
        XCTAssertTrue(waitUntilHittable(llmProvider, timeout: 5))
        llmProvider.tap()
        let providerBlurb = app.staticTexts.matching(
            NSPredicate(format: "label CONTAINS %@", "密钥仅本机加密")
        ).firstMatch
        XCTAssertTrue(providerBlurb.waitForExistence(timeout: 5), app.debugDescription)
        let addProvider = app.buttons["provider.add"]
        XCTAssertTrue(addProvider.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(addProvider, timeout: 5), app.debugDescription)
        addProvider.tap()
        XCTAssertTrue(app.staticTexts["Anthropic"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["OpenAI"].exists)
        XCTAssertTrue(app.staticTexts["Kimi"].exists)

        app.buttons["完成"].tap()
        app.buttons["关闭设置"].tap()
        openDrawer()
        app.buttons["drawer.shortcut.cron"].tap()
        let newCron = app.buttons["cron.add"]
        XCTAssertTrue(newCron.waitForExistence(timeout: 8), app.debugDescription)
        let schedulingNote = app.staticTexts.matching(
            NSPredicate(format: "label CONTAINS %@", "系统调度")
        ).firstMatch
        XCTAssertTrue(schedulingNote.exists)
    }

    private func openDrawer() {
        let trigger = app.buttons["打开抽屉"]
        XCTAssertTrue(trigger.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(trigger, timeout: 5), app.debugDescription)
        trigger.tap()

        let chatsTab = app.buttons["drawer.tab.chats"]
        if !chatsTab.waitForExistence(timeout: 5),
           trigger.exists,
           waitUntilHittable(trigger, timeout: 2) {
            // The UI-test fixture can finish its initial source refresh between
            // hit-testing and event delivery, replacing the chat surface once.
            // Retry only while the original trigger is still actually hittable.
            trigger.tap()
        }
        XCTAssertTrue(chatsTab.waitForExistence(timeout: 8), app.debugDescription)
    }

    private func waitUntilHittable(_ element: XCUIElement, timeout: TimeInterval) -> Bool {
        let ready = expectation(for: NSPredicate(format: "hittable == true"), evaluatedWith: element)
        return XCTWaiter.wait(for: [ready], timeout: timeout) == .completed
    }

}
