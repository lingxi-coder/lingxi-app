import XCTest

final class LingxiCodeUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        super.setUp()
        continueAfterFailure = false
        app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launch()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 12), app.debugDescription)
    }

    func testTimelineHidesAgentRunAndShowsCompactExecutionRows() {
        XCTAssertFalse(app.staticTexts["理解需求"].exists)
        let llmStatus = app.descendants(matching: .any)["conversation.llm-status"]
        XCTAssertFalse(app.staticTexts["已暂停"].exists, app.debugDescription)
        let userMessage = app.descendants(matching: .any)["conversation.message.user"]
        let agentRun = app.descendants(matching: .any)["conversation.agent-run"]
        let assistantMessage = app.descendants(matching: .any)["conversation.message.assistant"]
        XCTAssertTrue(userMessage.waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["Agent 运行"].exists)
        XCTAssertFalse(agentRun.exists)
        XCTAssertFalse(app.buttons["conversation.agent-picker"].exists)
        XCTAssertTrue(assistantMessage.exists)
        let thought = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "conversation.timeline.thought.")
        ).firstMatch
        XCTAssertTrue(thought.waitForExistence(timeout: 5), app.debugDescription)
        let tool = app.descendants(matching: .any)["conversation.tool-call.ui-shell"]
        XCTAssertTrue(tool.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-shell.icon.terminal"].exists)
        let batch = app.buttons.matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "conversation.timeline.tool-batch.")
        ).firstMatch
        XCTAssertTrue(batch.exists, app.debugDescription)
        XCTAssertFalse(app.descendants(matching: .any)["conversation.tool-call.ui-read"].exists)
        batch.tap()
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-read"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-search"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-read.icon.bookOpen"].exists)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-search.icon.search"].exists)

        // A normal completed turn has no persistent runtime footer.
        XCTAssertTrue(waitUntilGone(llmStatus, timeout: 5), app.debugDescription)
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = "iOS-CodexTimeline"
        screenshot.lifetime = .keepAlways
        add(screenshot)
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

    func testComposerSendTransitionsToMatchingStopControl() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_HOLD_TURN"] = "1"
        app.launchArguments += ["-theme", "light"]
        app.launch()

        let input = app.textFields["composer.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5), app.debugDescription)
        input.tap()
        input.typeText("composer action icon")

        let send = app.buttons["composer.send"]
        XCTAssertTrue(send.waitForExistence(timeout: 3), app.debugDescription)
        XCTAssertEqual(send.frame.width, send.frame.height, accuracy: 1)

        let sendScreenshot = XCTAttachment(screenshot: app.screenshot())
        sendScreenshot.name = "Composer-Send-Action"
        sendScreenshot.lifetime = .keepAlways
        add(sendScreenshot)

        send.tap()
        let stop = app.buttons["composer.stop"]
        XCTAssertTrue(stop.waitForExistence(timeout: 1), app.debugDescription)
        XCTAssertEqual(stop.frame.width, stop.frame.height, accuracy: 1)

        let stopScreenshot = XCTAttachment(screenshot: app.screenshot())
        stopScreenshot.name = "Composer-Stop-Action"
        stopScreenshot.lifetime = .keepAlways
        add(stopScreenshot)

        stop.tap()
        XCTAssertTrue(waitUntilGone(stop, timeout: 3), app.debugDescription)
    }

    func testSlashCommandSuggestionsFilterSelectAndSubmit() {
        let input = app.textFields["composer.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5), app.debugDescription)

        input.tap()
        input.typeText("/")
        let help = app.buttons["composer.slash-command.help"]
        let review = app.buttons["composer.slash-command.review"]
        XCTAssertTrue(help.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(review.exists, app.debugDescription)
        let suggestionList = app.scrollViews["composer.slash-command-list"]
        XCTAssertTrue(suggestionList.exists, app.debugDescription)
        XCTAssertGreaterThanOrEqual(suggestionList.frame.height, 220)
        XCTAssertGreaterThan(help.frame.height, 40)

        input.typeText("re")
        XCTAssertTrue(review.waitForExistence(timeout: 2), app.debugDescription)
        XCTAssertFalse(help.exists, app.debugDescription)
        review.tap()

        XCTAssertEqual(input.value as? String, "/review")
        XCTAssertFalse(review.exists, app.debugDescription)

        input.typeText(" [instructions]")
        XCTAssertTrue(app.staticTexts["[instructions]"].exists, app.debugDescription)
        let send = app.buttons["composer.send"]
        XCTAssertTrue(send.isHittable, app.debugDescription)
        send.tap()
        XCTAssertTrue(
            app.staticTexts["/review [instructions]"].waitForExistence(timeout: 5),
            app.debugDescription
        )
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

        XCTAssertFalse(app.descendants(matching: .any)["conversation.agent-run"].exists)
        XCTAssertTrue(app.staticTexts["WebSearch"].waitForExistence(timeout: 8), app.debugDescription)
        let batch = app.buttons.matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "conversation.timeline.tool-batch.")
        ).firstMatch
        XCTAssertTrue(batch.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertFalse(app.descendants(matching: .any)["conversation.tool-call.ui-web-search"].exists)
        batch.tap()
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-web-search"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-shell"].waitForExistence(timeout: 5), app.debugDescription)
        let webSearchStatus = app.staticTexts["conversation.tool-call.ui-web-search.status"]
        XCTAssertTrue(webSearchStatus.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertEqual(webSearchStatus.label, "已取消")
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

    func testMultiAgentDockSwitchesTranscriptAndRestoresMain() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_MULTI_AGENT"] = "1"
        app.launch()

        let picker = app.buttons["conversation.agent-picker"]
        XCTAssertTrue(picker.waitForExistence(timeout: 8), app.debugDescription)
        picker.tap()
        let childRow = app.buttons["conversation.agent-row.ui-child"]
        XCTAssertTrue(childRow.waitForExistence(timeout: 5), app.debugDescription)
        childRow.tap()

        XCTAssertTrue(app.staticTexts["Child agent completed the requested check."].waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.agent-read-only"].exists, app.debugDescription)
        XCTAssertFalse(app.textFields["composer.input"].exists, app.debugDescription)

        picker.tap()
        let mainRow = app.buttons["conversation.agent-row.main"]
        XCTAssertTrue(mainRow.waitForExistence(timeout: 5), app.debugDescription)
        mainRow.tap()
        XCTAssertTrue(app.staticTexts["Hello! I'm ready to help with your software engineering tasks."].waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(app.textFields["composer.input"].waitForExistence(timeout: 5), app.debugDescription)
    }

    func testDrawerTabsCreateAndSwitchProject() {
        openDrawer()
        XCTAssertTrue(app.buttons["drawer.tab.chats"].exists)
        XCTAssertTrue(app.buttons["drawer.tab.projects"].exists)
        XCTAssertFalse(app.buttons["drawer.tab.crons"].exists)

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

        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
        openDrawer()
        let projectScope = app.buttons["drawer.scope.project.\(projectName)"]
        XCTAssertTrue(projectScope.waitForExistence(timeout: 5))
        XCTAssertEqual(projectScope.value as? String, "当前项目")

        app.buttons["drawer.scope.global"].tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
        openDrawer()
        let persistedProjectScope = app.buttons["drawer.scope.project.\(projectName)"]
        XCTAssertTrue(persistedProjectScope.waitForExistence(timeout: 5))
        persistedProjectScope.tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
    }

    /// The hand-written drawer had no drag affordance at all — `DragGesture` did
    /// not appear anywhere in Sources. Collapsing the split view onto a
    /// navigation stack hands the interactive back-swipe over for free, so pin
    /// it: an edge drag on the chat must land on the sidebar.
    ///
    /// Only the opening direction is a gesture. iOS has no forward swipe, so
    /// returning to the chat is a tap — here the already-active scope pill,
    /// which routes through `switchProject` and its `closeSidebar()`.
    func testEdgeSwipeOpensTheSidebar() {
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 8), app.debugDescription)

        let origin = app.coordinate(withNormalizedOffset: CGVector(dx: 0.01, dy: 0.5))
        let target = app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5))
        origin.press(forDuration: 0.05, thenDragTo: target)

        XCTAssertTrue(
            app.buttons["drawer.tab.chats"].waitForExistence(timeout: 8),
            app.debugDescription
        )

        app.buttons["drawer.scope.global"].tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
    }

    /// Every sidebar route used to be routed through a deferred close
    /// (`closeDrawerThen` + `Task.yield()`) because dismissing the hand-written
    /// drawer and changing presentation state in one animated transaction made
    /// SwiftUI drop the presentation. `NavigationSplitView` removed that
    /// coupling and the deferral went with it, so all three remaining presentation shapes
    /// are pinned here: a sheet, a push, and two full-screen covers. Each must
    /// arrive AND leave the sidebar behind.
    func testEverySidebarRoutePresentsAndLeavesTheSidebar() {
        // Settings — a sheet.
        openDrawer()
        app.buttons["drawer.settings"].tap()
        XCTAssertTrue(app.staticTexts["设置"].waitForExistence(timeout: 8), app.debugDescription)
        app.buttons["关闭设置"].tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)

        // Terminal — a push onto the detail column's stack, i.e. the one route
        // that changes the split view's column and the stack's path together.
        openDrawer()
        app.buttons["drawer.terminal.top"].tap()
        XCTAssertTrue(
            app.descendants(matching: .any)["terminal.root"].waitForExistence(timeout: 15),
            app.debugDescription
        )
        app.navigationBars.firstMatch.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)

        // Local apps — a full-screen cover reached from the drawer's apps tab.
        openDrawer()
        app.buttons["drawer.tab.apps"].tap()
        let createApp = app.buttons["drawer.apps.create"]
        XCTAssertTrue(createApp.waitForExistence(timeout: 8), app.debugDescription)
        createApp.tap()
        XCTAssertTrue(app.navigationBars["应用"].waitForExistence(timeout: 15), app.debugDescription)
        app.buttons["关闭"].tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
    }

    func testProviderManagementDoesNotExposeScheduledTasksInDrawer() {
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
        XCTAssertFalse(visibility.exists)
        XCTAssertTrue(clear.exists)

        keyField.tap()
        keyField.typeText("sk-ui-draft")
        XCTAssertTrue(visibility.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertLessThanOrEqual(keyField.frame.maxX, visibility.frame.minX + 0.5)
        XCTAssertLessThanOrEqual(visibility.frame.maxX, clear.frame.minX + 0.5)

        app.buttons["完成"].tap()
        let discardKeyChanges = app.buttons["放弃密钥修改"]
        XCTAssertTrue(discardKeyChanges.waitForExistence(timeout: 5), app.debugDescription)
        discardKeyChanges.tap()
        app.buttons["关闭设置"].tap()
        openDrawer()
        XCTAssertFalse(app.buttons["drawer.tab.crons"].exists)
        XCTAssertFalse(app.buttons["drawer.shortcut.cron"].exists)
    }

    func testChatToolbarIconsShareSizeAndSpacing() {
        let sessionDetails = app.buttons["conversation.session-details"]
        let themeToggle = app.buttons["conversation.theme-toggle"]
        let newChat = app.buttons["conversation.new-chat"]

        XCTAssertTrue(sessionDetails.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(themeToggle.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(newChat.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertGreaterThanOrEqual(themeToggle.frame.width, 28)
        XCTAssertGreaterThanOrEqual(themeToggle.frame.height, 32)
        XCTAssertEqual(sessionDetails.frame.width, themeToggle.frame.width, accuracy: 1)
        XCTAssertEqual(themeToggle.frame.width, newChat.frame.width, accuracy: 1)
        XCTAssertEqual(sessionDetails.frame.height, themeToggle.frame.height, accuracy: 1)
        XCTAssertEqual(themeToggle.frame.height, newChat.frame.height, accuracy: 1)
        XCTAssertEqual(
            themeToggle.frame.midX - sessionDetails.frame.midX,
            newChat.frame.midX - themeToggle.frame.midX,
            accuracy: 1.5
        )
    }

    func testSessionDetailsOpensFromChatHeader() {
        let details = app.buttons["conversation.session-details"]
        XCTAssertTrue(details.waitForExistence(timeout: 8), app.debugDescription)
        details.tap()

        XCTAssertTrue(app.descendants(matching: .any)["session-details.root"].waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(app.staticTexts["会话详情"].exists)
        XCTAssertTrue(app.staticTexts["Agents"].exists)
        XCTAssertTrue(app.staticTexts["Tasks"].exists)
        XCTAssertTrue(app.staticTexts["Plan & progress"].exists)

        app.navigationBars.firstMatch.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 8), app.debugDescription)
    }

    func testThemeToggleUsesCompleteSystemGlyphAndUpdatesItsAction() {
        let themeToggle = app.buttons["conversation.theme-toggle"]
        XCTAssertTrue(themeToggle.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertGreaterThanOrEqual(themeToggle.frame.width, 28)
        XCTAssertGreaterThanOrEqual(themeToggle.frame.height, 32)

        let initialLabel = themeToggle.label
        XCTAssertTrue(
            initialLabel == "切换到深色主题" || initialLabel == "切换到浅色主题",
            app.debugDescription
        )

        themeToggle.tap()
        let expectedLabel = initialLabel == "切换到深色主题" ? "切换到浅色主题" : "切换到深色主题"
        let labelChanged = expectation(
            for: NSPredicate(format: "label == %@", expectedLabel),
            evaluatedWith: themeToggle
        )
        XCTAssertEqual(XCTWaiter.wait(for: [labelChanged], timeout: 5), .completed, app.debugDescription)
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

    /// The picker: grouped by provider, searchable, and recently-picked models
    /// pinned above the provider sections.
    ///
    /// Replaces an earlier test that asserted the popover's measured HEIGHT. The
    /// picker is a sheet now — the system sizes it, so height is no longer a
    /// property this layer can meaningfully assert; what matters is that every
    /// provider is reachable, the search box narrows the list, and a pick
    /// resurfaces at the top next time.
    func testModelPickerSearchesGroupsAndPinsRecentPicks() {
        let chip = app.buttons["composer.model"]
        XCTAssertTrue(chip.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(chip, timeout: 5), app.debugDescription)
        chip.tap()

        let sheet = app.descendants(matching: .any)["composer.model.menu"]
        XCTAssertTrue(sheet.waitForExistence(timeout: 5), app.debugDescription)

        // Grouped by provider: headers for the providers the fixture spans.
        XCTAssertTrue(app.staticTexts["Anthropic"].waitForExistence(timeout: 3), app.debugDescription)
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = "模型选择器"
        shot.lifetime = .keepAlways
        add(shot)

        // Every provider is reachable — the last one in the fixture sits well
        // below the fold, so scroll in a bounded loop rather than exactly once.
        XCTAssertTrue(
            scrollUntilHittable(providerRow("zai/glm-5.1"), in: sheet),
            app.debugDescription)

        // Search narrows the list: "sonnet" keeps the Claude row and drops
        // DeepSeek entirely.
        let search = app.searchFields.firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: 3), app.debugDescription)
        search.tap()
        search.typeText("sonnet")
        let sonnet = providerRow("anthropic/claude-sonnet-5")
        XCTAssertTrue(sonnet.waitForExistence(timeout: 3), app.debugDescription)
        // `List` is lazy, so an off-screen row was never materialised either —
        // scroll the filtered list to the bottom and confirm DeepSeek is absent
        // from it rather than merely undrawn.
        for _ in 0..<4 { sheet.swipeUp() }
        XCTAssertFalse(
            providerRow("deepseek/deepseek-v4-flash").exists,
            app.debugDescription)

        // A query matching nothing shows the empty state rather than a blank list.
        search.typeText("zzzz")
        XCTAssertTrue(app.staticTexts["没有匹配的模型"].waitForExistence(timeout: 3), app.debugDescription)

        // Clear with the keyboard rather than the field's clear button: the
        // button's label is locale- and version-dependent, and the keyboard
        // covers part of the sheet while it is up.
        search.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: "sonnetzzzz".count))
        XCTAssertTrue(
            providerRow("anthropic/claude-opus-4-8").waitForExistence(timeout: 3),
            "clearing the query must restore the full list: " + app.debugDescription)

        // Search for a model and pick it straight out of the filtered list —
        // no scrolling needed, which is the point of having search at all.
        let flash = providerRow("deepseek/deepseek-v4-flash")
        search.typeText("flash")
        XCTAssertTrue(flash.waitForExistence(timeout: 3), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(flash, timeout: 3), app.debugDescription)
        flash.tap()
        XCTAssertTrue(waitUntilGone(sheet, timeout: 5), app.debugDescription)
        XCTAssertEqual(chip.label, "V4 Flash", app.debugDescription)

        // Reopening pins that pick to the top under 最近使用 — a SECOND row for
        // the same model, distinct from the one under its provider.
        chip.tap()
        XCTAssertTrue(sheet.waitForExistence(timeout: 5), app.debugDescription)
        let recent = app.buttons["composer.model.recent.row.deepseek/deepseek-v4-flash"]
        XCTAssertTrue(recent.waitForExistence(timeout: 3), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(recent, timeout: 3), app.debugDescription)
        XCTAssertTrue(app.staticTexts["最近使用"].exists, app.debugDescription)
        // It sits ABOVE every provider section.
        XCTAssertLessThan(recent.frame.minY, app.staticTexts["Anthropic"].frame.minY)
    }

    /// `List` only materialises visible rows, so a row far down the picker does
    /// not merely fail `isHittable` — it does not exist yet. Scroll until it
    /// does, bounded so a genuinely missing row fails instead of spinning.
    @discardableResult
    private func scrollUntilHittable(
        _ element: XCUIElement,
        in container: XCUIElement,
        swipes: Int = 8
    ) -> Bool {
        for _ in 0..<swipes {
            if element.exists && element.isHittable { return true }
            container.swipeUp()
        }
        return element.exists && element.isHittable
    }

    /// A model can appear twice — once under 最近使用 and once under its own
    /// provider — so rows are addressed by their section.
    private func providerRow(_ reference: String) -> XCUIElement {
        app.buttons["composer.model.provider.row.\(reference)"]
    }

    /// The chat is the detail column of a `NavigationSplitView`. In compact width
    /// that collapses to a stack rooted at the sidebar, so "open the drawer" is
    /// the system back button — the leading navigation-bar item — not an
    /// app-drawn control any more.
    private func openDrawer() {
        let chatsTab = app.buttons["drawer.tab.chats"]
        if chatsTab.exists { return }

        let trigger = app.navigationBars.firstMatch.buttons.element(boundBy: 0)
        XCTAssertTrue(trigger.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(trigger, timeout: 5), app.debugDescription)
        trigger.tap()

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

    /// The transcript identifies the chat column without depending on chrome.
    private var chatSurface: XCUIElement {
        app.scrollViews["conversation.message-list"]
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
