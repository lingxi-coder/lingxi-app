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
        XCTAssertFalse(app.staticTexts["理解需求"].exists)
        let userMessage = app.descendants(matching: .any)["conversation.message.user"]
        let agentRun = app.descendants(matching: .any)["conversation.agent-run"]
        let assistantMessage = app.descendants(matching: .any)["conversation.message.assistant"]
        XCTAssertTrue(userMessage.waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Agent 运行"].waitForExistence(timeout: 5))
        XCTAssertTrue(agentRun.exists)
        XCTAssertTrue(assistantMessage.exists)
        XCTAssertLessThanOrEqual(userMessage.frame.maxY, agentRun.frame.minY + 0.5)
        XCTAssertLessThanOrEqual(agentRun.frame.maxY, assistantMessage.frame.minY + 0.5)
        XCTAssertEqual(agentRun.frame.minX, assistantMessage.frame.minX, accuracy: 1)
        XCTAssertTrue(app.staticTexts["/workspace/ui-test"].exists)

        let openTerminal = app.buttons["在终端打开"]
        XCTAssertTrue(openTerminal.waitForExistence(timeout: 5))
        openTerminal.tap()

        XCTAssertTrue(app.navigationBars["终端"].waitForExistence(timeout: 8))
        app.buttons["关闭"].tap()
        XCTAssertTrue(app.buttons["打开抽屉"].waitForExistence(timeout: 5))
    }

    func testComposerTracksKeyboardAndKeepsVoiceModesSeparate() {
        let input = app.textFields["composer.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.buttons["composer.voice"].exists)
        XCTAssertTrue(app.buttons["composer.flow"].exists)

        input.tap()
        input.typeText("keyboard draft")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))

        XCTAssertTrue(app.scrollViews["conversation.message-list"].exists)
        let dismissKeyboard = app.buttons["composer.keyboard.dismiss"]
        XCTAssertTrue(dismissKeyboard.waitForExistence(timeout: 2), app.debugDescription)
        dismissKeyboard.tap()
        XCTAssertFalse(app.keyboards.firstMatch.waitForExistence(timeout: 2))
        XCTAssertEqual(input.value as? String, "keyboard draft")
    }

    func testVoiceModesShareInlinePanelAndConfigurationDeepLink() {
        app.terminate()
        app.launchArguments += [
            "-voiceSpeechConfigurationVersion", "0",
            "-voiceTTSConfigurationVersion", "0",
        ]
        app.launch()

        let messageList = app.scrollViews["conversation.message-list"]
        let composerInput = app.textFields["composer.input"]
        let flowButton = app.buttons["composer.flow"]
        XCTAssertTrue(flowButton.waitForExistence(timeout: 8), app.debugDescription)
        flowButton.tap()

        let panel = app.descendants(matching: .any)["conversation.voice-panel"]
        XCTAssertTrue(panel.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.staticTexts["心流模式"].exists)
        XCTAssertTrue(app.staticTexts["需要配置语音能力"].exists)
        XCTAssertTrue(messageList.exists)
        XCTAssertTrue(composerInput.exists)
        XCTAssertLessThanOrEqual(messageList.frame.maxY, panel.frame.minY + 1)
        XCTAssertLessThanOrEqual(panel.frame.maxY, composerInput.frame.minY + 1)
        let panelScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        panelScreenshot.name = "iOS-内联心流配置面板"
        panelScreenshot.lifetime = .keepAlways
        add(panelScreenshot)

        app.buttons["voice.close"].tap()
        XCTAssertTrue(waitUntilGone(panel, timeout: 5))
        app.buttons["composer.voice"].tap()
        XCTAssertTrue(panel.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.staticTexts["语音输入"].exists)

        let configure = app.buttons["voice.configure"]
        XCTAssertTrue(configure.exists)
        configure.tap()
        XCTAssertTrue(app.navigationBars["语音 TTS"].waitForExistence(timeout: 5), app.debugDescription)
        let saveVoiceConfiguration = app.buttons["settings.voice.save"]
        XCTAssertTrue(saveVoiceConfiguration.exists)
        XCTAssertTrue(saveVoiceConfiguration.isEnabled)
    }

    func testCancelledRunClosesEveryRunningRow() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_CANCELLED_RUN"] = "1"
        app.launch()

        XCTAssertTrue(app.staticTexts["WebSearch"].waitForExistence(timeout: 8), app.debugDescription)
        let cancelledLabels = app.staticTexts.matching(
            NSPredicate(format: "label == %@", "已取消")
        )
        XCTAssertGreaterThan(cancelledLabels.count, 0)
        XCTAssertFalse(app.staticTexts["运行中"].exists, app.debugDescription)

        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = "取消后工具终态"
        screenshot.lifetime = .keepAlways
        add(screenshot)
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

        let deepSeekPreset = app.buttons.matching(
            NSPredicate(format: "label CONTAINS %@", "DeepSeek")
        ).firstMatch
        XCTAssertTrue(deepSeekPreset.waitForExistence(timeout: 5), app.debugDescription)
        deepSeekPreset.tap()

        let keyField = app.secureTextFields["provider.api-key"]
        let visibility = app.buttons["provider.api-key.visibility"]
        let clear = app.buttons["provider.api-key.clear"]
        XCTAssertTrue(keyField.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(visibility.exists)
        XCTAssertTrue(clear.exists)
        XCTAssertLessThanOrEqual(keyField.frame.maxX, visibility.frame.minX + 0.5)
        XCTAssertLessThanOrEqual(visibility.frame.maxX, clear.frame.minX + 0.5)

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

    func testAppIntegrationExposesRealSystemActions() {
        openDrawer()
        app.buttons["drawer.settings"].tap()
        XCTAssertTrue(app.staticTexts["设置"].waitForExistence(timeout: 5))
        Thread.sleep(forTimeInterval: 1)

        let integration = app.buttons["settings.appIntegration"]
        if !integration.exists || !integration.isHittable {
            app.swipeUp()
        }
        XCTAssertTrue(integration.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(integration, timeout: 5), app.debugDescription)
        integration.tap()

        XCTAssertTrue(app.navigationBars["应用接入"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.staticTexts["打开灵犀"].exists)
        XCTAssertTrue(app.staticTexts["新建对话"].exists)
        XCTAssertTrue(app.staticTexts["向灵犀提问"].exists)
        XCTAssertTrue(app.staticTexts["打开终端"].exists)
        let topScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        topScreenshot.name = "应用接入-顶部"
        topScreenshot.lifetime = .keepAlways
        add(topScreenshot)

        app.swipeUp()
        let openShortcuts = app.descendants(matching: .any)["settings.appIntegration.openShortcuts"]
        XCTAssertTrue(openShortcuts.waitForExistence(timeout: 5), app.debugDescription)

        app.swipeUp()
        let platformBoundary = app.staticTexts["不会读取、点击或控制其他 App 的界面。"]
        XCTAssertTrue(platformBoundary.waitForExistence(timeout: 5), app.debugDescription)
        let lowerScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        lowerScreenshot.name = "应用接入-下半部"
        lowerScreenshot.lifetime = .keepAlways
        add(lowerScreenshot)
    }

    func testOnboardingHeaderStaysAtTopAndPrimaryActionIsVisible() {
        app.terminate()
        app.launchEnvironment["LINGXI_FORCE_ONBOARDING"] = "1"
        app.launch()

        let header = app.descendants(matching: .any)["onboarding.header"]
        let content = app.descendants(matching: .any)["onboarding.content"]
        let primaryAction = app.buttons["onboarding.primaryAction"]
        XCTAssertTrue(header.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(content.exists)
        XCTAssertTrue(primaryAction.exists)
        XCTAssertTrue(primaryAction.isHittable)

        let screen = app.frame
        XCTAssertLessThan(header.frame.minY, screen.height * 0.18)
        XCTAssertGreaterThan(primaryAction.frame.minY, content.frame.minY)
        XCTAssertLessThanOrEqual(primaryAction.frame.maxY, screen.maxY)

        primaryAction.tap()
        XCTAssertTrue(app.staticTexts["给你的灵犀起个名字"].waitForExistence(timeout: 3))
        primaryAction.tap()
        XCTAssertTrue(app.staticTexts["我该怎么称呼你？"].waitForExistence(timeout: 3))
        let nameField = app.textFields["你的名字"]
        XCTAssertTrue(nameField.waitForExistence(timeout: 3), app.debugDescription)
        nameField.tap()
        nameField.typeText("测试用户")
        primaryAction.tap()

        let voiceTitle = app.staticTexts["配置语音能力"]
        XCTAssertTrue(voiceTitle.waitForExistence(timeout: 3), app.debugDescription)
        XCTAssertTrue(voiceTitle.isHittable)
        XCTAssertTrue(primaryAction.isHittable)
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

    private func waitUntilGone(_ element: XCUIElement, timeout: TimeInterval) -> Bool {
        let gone = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: element)
        return XCTWaiter.wait(for: [gone], timeout: timeout) == .completed
    }

}
