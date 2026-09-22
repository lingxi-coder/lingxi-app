import XCTest

final class LingxiCodeUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        super.setUp()
        continueAfterFailure = false
        app = XCUIApplication()
        // Scheme test arguments reach the UI-test runner, not every
        // XCUIApplication launch. Pin the app itself because many assertions
        // intentionally verify the zh-Hans product copy.
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)"]
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launch()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 12), app.debugDescription)
    }

    func testScheduledTaskCenterShowsStatusesAndPreservesDraft() {
        openDrawer()
        app.buttons["drawer.tab.cron"].tap()
        let create = app.buttons["cron.add"]
        XCTAssertTrue(create.waitForExistence(timeout: 12), app.debugDescription)
        XCTAssertTrue(app.buttons["All"].exists, app.debugDescription)
        XCTAssertTrue(app.buttons["Active"].exists, app.debugDescription)
        XCTAssertTrue(app.buttons["Paused"].exists, app.debugDescription)
        XCTAssertTrue(app.buttons["Completed"].exists, app.debugDescription)
        let listImage = XCTAttachment(screenshot: app.screenshot())
        listImage.name = "Scheduled task center"
        listImage.lifetime = .keepAlways
        add(listImage)
        create.tap()
        let name = app.textFields["Task name"]
        XCTAssertTrue(name.waitForExistence(timeout: 5), app.debugDescription)
        let settingsOverview = XCTAttachment(screenshot: app.screenshot())
        settingsOverview.name = "Scheduled task settings overview"
        settingsOverview.lifetime = .keepAlways
        add(settingsOverview)
        name.tap()
        name.typeText("Keep my draft")
        app.buttons["Cancel"].tap()
        XCTAssertTrue(app.buttons["Keep editing"].waitForExistence(timeout: 3))
        app.buttons["Keep editing"].tap()
        XCTAssertEqual(name.value as? String, "Keep my draft")
        let editorImage = XCTAttachment(screenshot: app.screenshot())
        editorImage.name = "Scheduled task settings"
        editorImage.lifetime = .keepAlways
        add(editorImage)
        app.buttons["Cancel"].tap()
        app.buttons["Discard"].tap()
        XCTAssertTrue(create.waitForExistence(timeout: 5))
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
        XCTAssertFalse(thought.exists, app.debugDescription)
        let shellGroup = app.buttons["conversation.timeline.tool-batch.timeline-tools:ui-shell"]
        XCTAssertTrue(shellGroup.waitForExistence(timeout: 5), app.debugDescription)
        shellGroup.tap()
        let tool = app.descendants(matching: .any)["conversation.tool-call.ui-shell"]
        XCTAssertTrue(tool.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-shell.icon.terminal"].exists)
        let batch = app.buttons["conversation.timeline.tool-batch.timeline-tools:ui-read"]
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

    func testVoiceTapRequestsPermissionThenOpensListeningAndSettings() {
        app.terminate()
        app.resetAuthorizationStatus(for: .microphone)
        app.launch()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 12), app.debugDescription)

        let voiceButton = app.buttons["composer.voice"]
        XCTAssertTrue(voiceButton.waitForExistence(timeout: 5), app.debugDescription)
        voiceButton.tap()

        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        var permissionPromptCount = 0
        for _ in 0..<2 {
            let alert = springboard.alerts.firstMatch
            guard alert.waitForExistence(timeout: 5) else { break }
            permissionPromptCount += 1
            XCTAssertTrue(allowSystemPermission(in: alert), alert.debugDescription)
            if app.staticTexts["正在聆听"].waitForExistence(timeout: 4) {
                break
            }
        }

        XCTAssertGreaterThan(permissionPromptCount, 0, springboard.debugDescription)
        XCTAssertTrue(app.staticTexts["正在聆听"].waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertFalse(app.buttons["voice.configure"].exists, app.debugDescription)

        let listeningScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        listeningScreenshot.name = "Voice-Permission-Granted-Listening"
        listeningScreenshot.lifetime = .keepAlways
        add(listeningScreenshot)

        app.buttons["voice.close"].tap()
        openDrawer()
        app.buttons["drawer.settings"].tap()
        XCTAssertTrue(app.staticTexts["设置"].waitForExistence(timeout: 8), app.debugDescription)

        let voiceSettings = app.buttons.matching(
            NSPredicate(format: "label BEGINSWITH %@", "语音 TTS")
        ).firstMatch
        XCTAssertTrue(voiceSettings.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(voiceSettings, timeout: 5), app.debugDescription)
        voiceSettings.tap()

        XCTAssertTrue(app.navigationBars["语音 TTS"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.staticTexts["听 · 语音识别"].exists, app.debugDescription)
        XCTAssertTrue(app.staticTexts["说 · 语音合成"].exists, app.debugDescription)

        let settingsTopScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        settingsTopScreenshot.name = "Voice-Settings-Top"
        settingsTopScreenshot.lifetime = .keepAlways
        add(settingsTopScreenshot)

        app.swipeUp()
        app.swipeUp()
        XCTAssertTrue(app.staticTexts["访问与可用性"].waitForExistence(timeout: 5), app.debugDescription)

        let settingsAccessScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        settingsAccessScreenshot.name = "Voice-Settings-Access"
        settingsAccessScreenshot.lifetime = .keepAlways
        add(settingsAccessScreenshot)
    }

    func testComposerShowsUnconfiguredWhenNoProviderExists() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_PROVIDER_UNCONFIGURED"] = "1"
        app.launch()

        let options = app.buttons["composer.model"]
        XCTAssertTrue(options.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(String(describing: options.value).contains("未配置"), app.debugDescription)
    }

    func testComposerConfigurationIsVisibleAndFastModeCanChange() {
        app.buttons["composer.controls"].tap()
        XCTAssertTrue(app.buttons["composer.permission.auto"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertFalse(app.descendants(matching: .any)["composer.reasoning.slider"].exists)
        XCTAssertFalse(app.switches["composer.model.fast-mode"].exists)
        let permissionShot = XCTAttachment(screenshot: app.screenshot())
        permissionShot.name = "Permission-only sheet"
        permissionShot.lifetime = .keepAlways
        add(permissionShot)
        app.buttons["composer.permission.close"].tap()
        openModelPicker()
        XCTAssertFalse(app.switches["composer.model.fast-mode"].exists)
        let opus = providerRow("anthropic/claude-opus-4-8")
        XCTAssertTrue(opus.waitForExistence(timeout: 5), app.debugDescription)
        opus.tap()
        let toggle = app.switches["composer.model.fast-mode"]
        XCTAssertTrue(toggle.waitForExistence(timeout: 5), app.debugDescription)
        let effort = app.descendants(matching: .any)["composer.reasoning.slider"].firstMatch
        XCTAssertTrue(effort.waitForExistence(timeout: 3), app.debugDescription)
        effort.coordinate(withNormalizedOffset: CGVector(dx: 0.92, dy: 0.5)).tap()
        XCTAssertTrue(String(describing: effort.value).contains("High"), app.debugDescription)
        toggle.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        let modelShot = XCTAttachment(screenshot: app.screenshot())
        modelShot.name = "Model effort and Fast sheet"
        modelShot.lifetime = .keepAlways
        add(modelShot)
        app.buttons["composer.model.close"].tap()
        let model = app.buttons["composer.model"]
        XCTAssertTrue(String(describing: model.value).contains("High"), app.debugDescription)
        XCTAssertTrue(String(describing: model.value).contains("Fast Mode On"), app.debugDescription)
        openModelPicker()
        toggle.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        providerRow("anthropic/claude-sonnet-5").tap()
        XCTAssertFalse(app.switches["composer.model.fast-mode"].exists)
        XCTAssertFalse(app.descendants(matching: .any)["composer.reasoning.slider"].exists)
        app.buttons["composer.model.close"].tap()
        XCTAssertFalse(String(describing: model.value).contains("Fast Mode"))
    }

    func testKeyboardPreservesComposerLayout() {
        let input = app.textFields["composer.input"]
        let ids = ["composer.model", "composer.controls", "composer.voice", "composer.flow"]
        let inputFrame = input.frame
        let frames = ids.map { app.buttons[$0].frame }
        input.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))
        for (id, frame) in zip(ids, frames) {
            let control = app.buttons[id]
            XCTAssertTrue(control.isHittable, id)
            XCTAssertEqual(control.frame.minX, frame.minX, accuracy: 1, id)
            XCTAssertEqual(control.frame.width, frame.width, accuracy: 1, id)
            XCTAssertEqual(control.frame.midY - input.frame.minY,
                           frame.midY - inputFrame.minY, accuracy: 1, id)
        }
        app.buttons["composer.keyboard.dismiss"].tap()
        for (id, frame) in zip(ids, frames) {
            XCTAssertEqual(app.buttons[id].frame.minX, frame.minX, accuracy: 1, id)
        }
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
        XCTAssertFalse(app.descendants(matching: .any)["conversation.llm-status"].exists, app.debugDescription)
        let thinkingRows = app.descendants(matching: .any).matching(identifier: "conversation.timeline.thinking")
        XCTAssertEqual(thinkingRows.count, 1, app.debugDescription)

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
        app.launchEnvironment["LINGXI_UI_TEST_VOICE_CONFIGURATION_REQUIRED"] = "1"
        app.launch()

        let messageList = app.scrollViews["conversation.message-list"]
        let composerInput = app.textFields["composer.input"]
        let flowButton = app.buttons["composer.flow"]
        XCTAssertTrue(flowButton.waitForExistence(timeout: 8), app.debugDescription)
        flowButton.tap()

        let panel = app.descendants(matching: .any)["conversation.voice-panel"]
        XCTAssertTrue(panel.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.staticTexts["心流模式"].exists)
        XCTAssertTrue(
            app.descendants(matching: .any)["voice.configuration-required"].exists,
            app.debugDescription
        )
        XCTAssertTrue(messageList.exists)
        XCTAssertTrue(composerInput.exists)
        // XCUI reports a ScrollView's content extent rather than its clipped
        // viewport, so its frame can overlap later VStack siblings even when
        // the rendered transcript does not. The load-bearing layout boundary
        // is the panel-to-composer edge below.
        XCTAssertTrue(waitUntilVerticallyStacked(panel, above: composerInput, timeout: 3))
        let panelScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        panelScreenshot.name = "iOS-内联心流配置面板"
        panelScreenshot.lifetime = .keepAlways
        add(panelScreenshot)

        app.buttons["voice.close"].tap()
        XCTAssertTrue(waitUntilGone(panel, timeout: 5))
        app.buttons["composer.voice"].tap()
        XCTAssertTrue(panel.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.staticTexts["语音输入"].exists)

        let configure = app.descendants(matching: .any)["voice.configure"].firstMatch
        XCTAssertTrue(configure.waitForExistence(timeout: 5), app.debugDescription)
        configure.tap()
        XCTAssertTrue(app.navigationBars["语音 TTS"].waitForExistence(timeout: 5), app.debugDescription)
        // Voice preferences persist directly from their controls; the former
        // explicit Save button no longer exists.
        XCTAssertTrue(app.staticTexts["听 · 语音识别"].exists, app.debugDescription)
        XCTAssertTrue(app.staticTexts["说 · 语音合成"].exists, app.debugDescription)
    }

    func testCancelledRunClosesEveryRunningRow() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_CANCELLED_RUN"] = "1"
        app.launch()

        XCTAssertFalse(app.descendants(matching: .any)["conversation.agent-run"].exists)
        let batch = app.buttons.matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "conversation.timeline.tool-batch.")
        ).firstMatch
        XCTAssertTrue(batch.waitForExistence(timeout: 8), app.debugDescription)
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

    /// A wire-maximum questionnaire (4 questions x 4 described options) must
    /// still present its actions. The sheet opened at the `.medium` detent with
    /// 取消/下一题/提交 inside the scrolling content, so a full-size request
    /// showed only options and nothing the user could act on.
    func testAskUserQuestionSheetKeepsActionsReachableWithMaximumOptions() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_ASK_QUESTION"] = "1"
        app.launch()

        let panel = app.descendants(matching: .any)["conversation.ask-user-question.sheet"]
        XCTAssertTrue(panel.waitForExistence(timeout: 10), app.debugDescription)
        let cancel = app.buttons["chat.ask.cancel"]
        XCTAssertTrue(cancel.waitForExistence(timeout: 5), app.debugDescription)
        let next = app.buttons["chat.ask.next"]
        XCTAssertTrue(next.waitForExistence(timeout: 5), app.debugDescription)
        // `isHittable` is what fails when a control is laid out below the
        // sheet's visible height: it exists in the hierarchy but no tap can
        // reach it.
        XCTAssertTrue(cancel.isHittable, app.debugDescription)
        XCTAssertTrue(next.isHittable, app.debugDescription)
        // The free-text row is part of every question and must be reachable
        // without first scrolling past four described options.
        XCTAssertTrue(app.descendants(matching: .any)["chat.ask.other.0"].exists, app.debugDescription)

        // Walk the whole stepper: every step must keep its actions on screen,
        // and the last one must offer an enabled 提交 once each question is
        // answered.
        for step in 0 ..< 4 {
            let option = app.buttons.matching(
                NSPredicate(format: "label BEGINSWITH %@", "Option \(step + 1).1")
            ).firstMatch
            XCTAssertTrue(option.waitForExistence(timeout: 5), app.debugDescription)
            XCTAssertTrue(option.isHittable, app.debugDescription)
            option.tap()
            guard step < 3 else { break }
            XCTAssertTrue(next.isHittable, app.debugDescription)
            next.tap()
        }
        let submit = app.buttons["chat.ask.submit"]
        XCTAssertTrue(submit.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(submit.isHittable, app.debugDescription)
        XCTAssertTrue(submit.isEnabled, app.debugDescription)
        XCTAssertTrue(app.buttons["chat.ask.prev"].isHittable, app.debugDescription)

        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = "提问表单-满配选项"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    /// A subagent's dispatch prompt is the first USER bubble of its child
    /// transcript and runs to thousands of characters. The user branch of
    /// `MessageBubble` had no fold at all — only the assistant branch did — so
    /// opening a child agent buried the transcript under a wall of text.
    func testLongSubagentPromptIsFoldedInTheChildTranscript() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_MULTI_AGENT"] = "1"
        app.launch()

        let menu = app.buttons["conversation.summary-menu"]
        XCTAssertTrue(menu.waitForExistence(timeout: 8), app.debugDescription)
        menu.tap()
        let agents = app.buttons["conversation.summary.category.agents"]
        XCTAssertTrue(agents.waitForExistence(timeout: 5), app.debugDescription)
        agents.tap()
        let childRow = app.buttons["conversation.summary.agent.ui-child"]
        XCTAssertTrue(childRow.waitForExistence(timeout: 5), app.debugDescription)
        childRow.tap()

        XCTAssertTrue(app.descendants(matching: .any)["conversation.agent-detail-sheet"].waitForExistence(timeout: 8), app.debugDescription)

        let toggle = app.buttons["conversation.message.user.toggle"]
        XCTAssertTrue(
            toggle.waitForExistence(timeout: 8),
            "a long subagent prompt must offer a fold affordance: \(app.debugDescription)"
        )

        // The suite runs with `-testLanguage zh-Hans`, so the collapsed label
        // is 展开. Asserting the CONCRETE initial label proves the bubble
        // starts folded — a label that merely "flips" would still pass with
        // the fold inert.
        XCTAssertEqual(
            toggle.label,
            "展开",
            "the bubble must START collapsed: \(app.debugDescription)"
        )

        // The real assertion: expanding must make the bubble TALLER. Without
        // it, deleting the `.lineLimit` modifier entirely leaves this test
        // green — the toggle would still render and its label would still
        // flip, testing nothing about the fold.
        // `otherElements[...]`, NOT `descendants(matching: .any)[...]`: the
        // latter walks the entire accessibility tree and stalls the runner
        // long enough for the test to be killed.
        let bubble = app.otherElements["conversation.message.user"]
        XCTAssertTrue(bubble.waitForExistence(timeout: 5), app.debugDescription)
        let collapsedHeight = bubble.frame.height

        toggle.tap()
        XCTAssertEqual(toggle.label, "收起", app.debugDescription)
        XCTAssertGreaterThan(
            bubble.frame.height,
            collapsedHeight,
            "expanding must reveal more of the prompt: \(app.debugDescription)"
        )

        // …and collapsing returns it.
        toggle.tap()
        XCTAssertEqual(toggle.label, "展开", app.debugDescription)
        XCTAssertEqual(bubble.frame.height, collapsedHeight, accuracy: 1.0)
    }

    func testMultiAgentDetailSheetDismissesBackToMain() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_MULTI_AGENT"] = "1"
        app.launch()

        // Child execution is surfaced inline in the transcript while the main
        // turn is active; it no longer relies on the removed bottom status dock.
        let inlineChild = app.buttons["conversation.agent-row.ui-child"]
        XCTAssertTrue(inlineChild.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertEqual(
            inlineChild.label,
            "UI Child · Running · Child agent checking workspace",
            app.debugDescription
        )
        XCTAssertFalse(app.descendants(matching: .any)["conversation.llm-status"].exists, app.debugDescription)
        let inlineScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        inlineScreenshot.name = "Inline child runtime row"
        inlineScreenshot.lifetime = .keepAlways
        add(inlineScreenshot)

        inlineChild.tap()
        let sheet = app.descendants(matching: .any)["conversation.agent-detail-sheet"]
        XCTAssertTrue(sheet.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(app.staticTexts["Child agent completed the requested check."].waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.agent-read-only"].exists, app.debugDescription)
        XCTAssertFalse(sheet.textFields["composer.input"].exists, app.debugDescription)
        app.buttons["conversation.agent-detail.close"].tap()
        XCTAssertTrue(sheet.waitForNonExistence(timeout: 5), app.debugDescription)

        XCTAssertTrue(app.staticTexts["Hello! I'm ready to help with your software engineering tasks."].waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(app.textFields["composer.input"].waitForExistence(timeout: 5), app.debugDescription)
    }

    /// The drawer has THREE tabs — `DrawerSection` is `chat`/`code`/`cron` —
    /// and no projects tab: a project is a workspace CARD inside the chat and
    /// code sections, and `drawer.project.create` lives in
    /// `conversationActions`, rendered for every non-cron section. This test
    /// used to drive `drawer.tab.projects`, `drawer.tab.crons`,
    /// `drawer.scope.global` and `drawer.scope.project.<name>`, none of which
    /// any `accessibilityIdentifier` in `Sources/` has produced since the
    /// workspace-card rework.
    func testDrawerTabsCreateAndSwitchProject() {
        openDrawer()
        XCTAssertTrue(app.buttons["drawer.tab.chat"].exists)
        XCTAssertTrue(app.buttons["drawer.tab.code"].exists)
        XCTAssertTrue(app.buttons["drawer.tab.cron"].exists)

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
        // A workspace card is `drawer.workspace.<ConversationScope.workspaceKey>`
        // — `project.<id>` — and the id is minted by the engine, so match the
        // prefix and read the name off the card. The card itself is the
        // group's container `VStack`, not a control, so it is matched over
        // every element type rather than `app.buttons`. The card declares
        // `.accessibilityElement(children: .contain)`, which is what keeps it
        // addressable here AND keeps the buttons inside it
        // (`drawer.workspace.new.*`, `drawer.session.*`) addressable below —
        // without it the card id overwrites every one of them.
        let projectWorkspace = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "drawer.workspace.project.")
        ).firstMatch
        XCTAssertTrue(projectWorkspace.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.staticTexts[projectName].waitForExistence(timeout: 5), app.debugDescription)

        // Leaving the project: the global card's own new-conversation button,
        // which routes through `startNewConversation(in: .global)`.
        app.buttons["drawer.workspace.new.global"].tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
        openDrawer()
        let backToProject = app.buttons.matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "drawer.workspace.new.project.")
        ).firstMatch
        XCTAssertTrue(backToProject.waitForExistence(timeout: 5), app.debugDescription)
        backToProject.tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
    }

    /// The hand-written drawer had no drag affordance at all — `DragGesture` did
    /// not appear anywhere in Sources. Collapsing the split view onto a
    /// navigation stack hands the interactive back-swipe over for free, so pin
    /// it: an edge drag on the chat must land on the sidebar.
    ///
    /// Only the opening direction is a gesture. iOS has no forward swipe, so
    /// returning to the chat is a tap — here the global workspace card's
    /// new-conversation button, which routes through
    /// `startNewConversation(in: .global)` and its `closeSidebar()`.
    func testEdgeSwipeOpensTheSidebar() {
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 8), app.debugDescription)

        let origin = app.coordinate(withNormalizedOffset: CGVector(dx: 0.01, dy: 0.5))
        let target = app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5))
        origin.press(forDuration: 0.05, thenDragTo: target)

        XCTAssertTrue(
            app.buttons["drawer.tab.chat"].waitForExistence(timeout: 8),
            app.debugDescription
        )

        app.buttons["drawer.workspace.new.global"].tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
    }

    func testCompactSidebarHasAnExplicitCloseControl() {
        openDrawer()

        let close = app.buttons["drawer.close"]
        XCTAssertTrue(close.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertEqual(close.label, "关闭侧栏")
        close.tap()

        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(waitUntilGone(close, timeout: 5), app.debugDescription)
    }

    /// Every sidebar route used to be routed through a deferred close
    /// (`closeDrawerThen` + `Task.yield()`) because dismissing the hand-written
    /// drawer and changing presentation state in one animated transaction made
    /// SwiftUI drop the presentation. `NavigationSplitView` removed that
    /// coupling and the deferral went with it, so every presentation shape the
    /// sidebar can still reach under this fixture is pinned here: a sheet and a
    /// push. Each must arrive AND leave the sidebar behind.
    ///
    /// The full-screen cover is no longer among them, and the apps leg says so
    /// with an assertion rather than a comment. The drawer no longer has an
    /// "apps" tab at all — `DrawerSection` is only `.chat`/`.code`/`.cron`
    /// (`Drawer.swift`) — and its two local-apps affordances,
    /// `drawer.apps.create` and `drawer.apps.library`, sit in
    /// `conversationActions`, rendered unconditionally whenever the drawer is
    /// not on the cron section; neither depends on the catalog being
    /// non-empty any more. `drawer.apps.create` creates an app directly
    /// instead of opening the library cover, so this leg pins THAT contract:
    /// the affordance is reachable with no tab to switch to, and tapping it
    /// leaves the sidebar behind without presenting anything.
    ///
    /// The cover itself is not left uncovered: it is mounted and asserted by
    /// `testTheDrawersViewAllMountsTheLocalAppsCover` below via
    /// `drawer.apps.library`, seeding the catalog
    /// (`LINGXI_UI_TEST_LOCAL_APPS=1`) only to have a row to assert inside the
    /// cover once it opens — the affordance itself would render either way.
    func testEverySidebarRoutePresentsAndLeavesTheSidebar() {
        // Settings — a sheet.
        openDrawer()
        app.buttons["drawer.settings"].tap()
        XCTAssertTrue(app.staticTexts["设置"].waitForExistence(timeout: 8), app.debugDescription)
        let settingsClose = app.descendants(matching: .any)["settings.close"].firstMatch
        XCTAssertTrue(settingsClose.waitForExistence(timeout: 5), app.debugDescription)
        settingsClose.tap()
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

        // Local apps — the drawer's create affordance. It does not open the
        // library cover any more: it creates an app and hands the conversation
        // over (`RootView.createLocalAppFromDrawer`), so what this leg pins is
        // the NEW contract — the sidebar is left behind and nothing is
        // presented over the chat. There is no tab to switch to first:
        // `drawer.apps.create` lives in `conversationActions`, rendered on the
        // default (chat) section like every other route above.
        openDrawer()
        app.buttons["drawer.tab.apps"].tap()
        let createApp = app.buttons["drawer.apps.create"]
        XCTAssertTrue(createApp.waitForExistence(timeout: 8), app.debugDescription)
        createApp.tap()

        // Leaves the sidebar behind, exactly like the two routes above: the row
        // that was just tapped goes away with the sidebar, and the chat returns.
        XCTAssertTrue(waitUntilGone(createApp, timeout: 10), app.debugDescription)
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)

        // …and presents nothing. A bounded wait rather than a bare `exists`:
        // the regression this guards against is a PRESENTATION, which takes time
        // to arrive, so the negative has to give it that time to mean anything.
        // `navigationBars["应用"]` (`local_apps_title`) is the very probe the
        // old assertion used POSITIVELY against this cover, which is why it is
        // known to fire when the cover mounts; `local-apps.create` is the
        // cover's toolbar button and `local-apps.create.empty-state` its
        // `ContentUnavailableView` action — the one an empty library shows — so
        // between them no state of that cover goes unnoticed.
        XCTAssertFalse(
            app.navigationBars["应用"].waitForExistence(timeout: 5),
            app.debugDescription
        )
        XCTAssertFalse(app.buttons["local-apps.create"].exists, app.debugDescription)
        XCTAssertFalse(
            app.buttons["local-apps.create.empty-state"].exists,
            app.debugDescription
        )
    }

    /// The local-apps cover, mounted through the drawer's `drawer.apps.library`
    /// affordance.
    ///
    /// `testEverySidebarRoutePresentsAndLeavesTheSidebar` above pins the
    /// SIBLING fact — that the drawer's create affordance creates an app
    /// directly rather than opening this cover. `drawer.apps.library` (in
    /// `conversationActions`, `Drawer.swift`) is rendered unconditionally, not
    /// gated on a non-empty catalog, so reaching the cover needs no seed at
    /// all; the seed below exists only so the cover has a row to assert once
    /// it is open. Under `LINGXI_UI_TESTING=1` the conversation source is
    /// `MockConversationSource`, whose `submitEngineCommand` is the no-op
    /// protocol-extension default, so `.listApps` never reaches an engine and
    /// the catalog cannot fill on its own — hence the launch below.
    ///
    /// `LINGXI_UI_TEST_LOCAL_APPS=1` is `LocalAppsStore.uiTestSeedEnvironmentKey`;
    /// `RootView.init` answers it by calling `LocalAppsStore.seedForUITesting()`,
    /// which plants exactly one app.
    ///
    /// `"ui-test-seeded-app"` below is the literal value of
    /// `LocalAppsStore.uiTestSeedAppID`. A UI test is a black box and cannot
    /// import the app module, so the constant is duplicated on purpose — and
    /// renaming it on the app side makes this test go red at the row probe
    /// rather than silently stop proving anything.
    func testAppsTabShowsTheSeededLocalAppLibrary() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_LOCAL_APPS"] = "1"
        app.launch()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 12), app.debugDescription)

        openDrawer()
        let appsTab = app.buttons["drawer.tab.apps"]
        XCTAssertTrue(appsTab.waitForExistence(timeout: 8), app.debugDescription)
        appsTab.tap()

        let appRow = app.buttons["drawer.apps.row.ui-test-seeded-app"]
        XCTAssertTrue(appRow.waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(
            app.buttons["drawer.apps.create"].exists,
            app.debugDescription
        )
    }

    /// The name is now WIDER than what the test asserts, and the name is kept
    /// only because `docs/superpowers/plans/` still cites it: scheduled tasks
    /// are no longer hidden anywhere — `Drawer.tabs` renders `tab(.cron, …)`
    /// unconditionally. What this covers is the provider-settings round trip
    /// (open Settings → LLM providers → a preset sheet → back out) and that it
    /// leaves the drawer intact. See the trailing assertions for what the two
    /// vacuous cron checks it used to end on were replaced with.
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
        // Provider presets live on this page now. Selecting one opens its
        // configuration sheet directly instead of requiring a separate + flow.
        XCTAssertTrue(app.staticTexts["Anthropic"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["OpenAI"].exists)
        XCTAssertTrue(app.staticTexts["Kimi"].exists)

        let deepSeekPreset = app.buttons["provider.preset.deepseek"]
        XCTAssertTrue(deepSeekPreset.waitForExistence(timeout: 5), app.debugDescription)
        let providerPage = app.scrollViews.firstMatch
        XCTAssertTrue(scrollUntilHittable(deepSeekPreset, in: providerPage), app.debugDescription)
        deepSeekPreset.tap()

        let keyField = app.secureTextFields["provider.api-key"]
        let visibility = app.buttons["provider.api-key.visibility"]
        let clear = app.buttons["provider.api-key.clear"]
        XCTAssertTrue(keyField.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(visibility.exists)
        XCTAssertFalse(clear.exists)

        keyField.tap()
        keyField.typeText("sk-ui-draft")
        XCTAssertTrue(visibility.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertLessThanOrEqual(keyField.frame.maxX, visibility.frame.minX + 0.5)
        XCTAssertFalse(clear.exists)

        app.keyboards.buttons["return"].tap()
        app.buttons["provider.cancel"].tap()
        XCTAssertTrue(waitUntilGone(keyField, timeout: 5), app.debugDescription)
        let savePasswordPrompt = app.sheets["Save Password?"]
        if savePasswordPrompt.waitForExistence(timeout: 1) {
            let notNow = savePasswordPrompt.buttons["Not Now"]
            XCTAssertTrue(notNow.waitForExistence(timeout: 2), app.debugDescription)
            notNow.tap()
            XCTAssertTrue(waitUntilGone(savePasswordPrompt, timeout: 5), app.debugDescription)
        }
        let settingsBack = app.navigationBars["LLM 提供商"].buttons["设置"]
        XCTAssertTrue(settingsBack.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(settingsBack, timeout: 5), app.debugDescription)
        settingsBack.tap()
        let settingsClose = app.descendants(matching: .any)["settings.close"].firstMatch
        XCTAssertTrue(settingsClose.waitForExistence(timeout: 5), app.debugDescription)
        settingsClose.tap()
        openDrawer()
        // This leg used to assert `drawer.tab.crons` and `drawer.shortcut.cron`
        // were ABSENT. Neither identifier has ever been produced by anything in
        // `Sources/` — the tab is `drawer.tab.cron` (`DrawerSection`'s rawValue
        // is singular) and there is no cron shortcut at all — so both were
        // vacuously true and asserted nothing. Scheduled tasks are an
        // unconditional drawer section now (`Drawer.tabs` renders
        // `tab(.cron, …)` with no gate), so what this leg can honestly pin is
        // that the provider round-trip leaves the drawer intact.
        XCTAssertTrue(app.buttons["drawer.tab.chat"].exists, app.debugDescription)
        XCTAssertTrue(app.buttons["drawer.tab.cron"].exists, app.debugDescription)
    }

    func testChatSummaryMenuContainsFormerToolbarActions() {
        let menu = app.buttons["conversation.summary-menu"]
        XCTAssertTrue(menu.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertGreaterThanOrEqual(menu.frame.width, 28)
        XCTAssertGreaterThanOrEqual(menu.frame.height, 32)
        XCTAssertFalse(app.buttons["conversation.session-details"].exists, app.debugDescription)
        XCTAssertFalse(app.buttons["conversation.theme-toggle"].exists, app.debugDescription)
        XCTAssertFalse(app.buttons["conversation.new-chat"].exists, app.debugDescription)

        menu.tap()
        XCTAssertTrue(app.buttons["conversation.session-details"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.buttons["conversation.theme-toggle"].exists, app.debugDescription)
        XCTAssertTrue(app.buttons["conversation.new-chat"].exists, app.debugDescription)
    }

    func testSummaryMenuOpensBottomSheetDismissesAndPreservesDraft() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_MULTI_AGENT"] = "1"
        app.launch()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 12), app.debugDescription)

        let input = app.textFields["composer.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5), app.debugDescription)
        input.tap()
        input.typeText("summary draft")
        app.buttons["composer.keyboard.dismiss"].tap()

        let chatScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        chatScreenshot.name = "Summary menu chat with draft"
        chatScreenshot.lifetime = .keepAlways
        add(chatScreenshot)

        let menu = app.buttons["conversation.summary-menu"]
        XCTAssertTrue(menu.waitForExistence(timeout: 5), app.debugDescription)
        menu.tap()
        let menuScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        menuScreenshot.name = "Summary menu"
        menuScreenshot.lifetime = .keepAlways
        add(menuScreenshot)

        let agents = app.buttons["conversation.summary.category.agents"]
        XCTAssertTrue(agents.waitForExistence(timeout: 5), app.debugDescription)
        agents.tap()
        let sheet = app.descendants(matching: .any)["conversation.summary-sheet.agents"]
        XCTAssertTrue(sheet.waitForExistence(timeout: 8), app.debugDescription)
        let sheetScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        sheetScreenshot.name = "Summary bottom sheet"
        sheetScreenshot.lifetime = .keepAlways
        add(sheetScreenshot)

        let close = app.buttons["关闭"]
        XCTAssertTrue(close.waitForExistence(timeout: 5), app.debugDescription)
        close.tap()
        XCTAssertTrue(waitUntilGone(sheet, timeout: 8), app.debugDescription)
        XCTAssertTrue(chatSurface.exists, app.debugDescription)
        XCTAssertEqual(input.value as? String, "summary draft", app.debugDescription)
        XCTAssertFalse(app.descendants(matching: .any)["conversation.agent-status"].exists, app.debugDescription)
    }

    func testSessionDetailsOpensFromChatHeader() {
        app.buttons["conversation.summary-menu"].tap()
        let details = app.buttons["conversation.session-details"]
        XCTAssertTrue(details.waitForExistence(timeout: 8), app.debugDescription)
        details.tap()

        let sheet = app.descendants(matching: .any)["conversation.session-details-sheet"]
        XCTAssertTrue(sheet.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["session-details.root"].waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(app.staticTexts["会话详情"].exists)
        XCTAssertTrue(app.staticTexts["Agents"].exists)
        XCTAssertTrue(app.staticTexts["Tasks"].exists)
        XCTAssertTrue(app.staticTexts["Plan & progress"].exists)

        app.buttons["conversation.session-details.close"].tap()
        XCTAssertTrue(waitUntilGone(sheet, timeout: 8), app.debugDescription)
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 8), app.debugDescription)
    }

    func testThemeToggleUsesCompleteSystemGlyphAndUpdatesItsAction() {
        app.buttons["conversation.summary-menu"].tap()
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
        XCTAssertTrue(waitUntilGone(themeToggle, timeout: 3), app.debugDescription)
        app.buttons["conversation.summary-menu"].tap()
        let updatedToggle = app.buttons["conversation.theme-toggle"]
        XCTAssertTrue(updatedToggle.waitForExistence(timeout: 5), app.debugDescription)
        let labelChanged = expectation(
            for: NSPredicate(format: "label == %@", expectedLabel),
            evaluatedWith: updatedToggle
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
        XCTAssertTrue(
            app.descendants(matching: .any)["onboarding.model.step"].waitForExistence(timeout: 3),
            app.debugDescription
        )
        XCTAssertFalse(app.staticTexts["我该怎么称呼你？"].exists, app.debugDescription)
        let modelScreenshot = XCTAttachment(screenshot: app.screenshot())
        modelScreenshot.name = "引导-模型设置"
        modelScreenshot.lifetime = .keepAlways
        add(modelScreenshot)

        let apiKeyPreset = app.buttons["onboarding.model.preset.anthropic"]
        XCTAssertTrue(apiKeyPreset.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(scrollUntilHittable(apiKeyPreset, in: content), app.debugDescription)
        apiKeyPreset.tap()
        let apiKeyField = app.secureTextFields["provider.api-key"]
        XCTAssertTrue(apiKeyField.waitForExistence(timeout: 5), app.debugDescription)
        let apiKeyScreenshot = XCTAttachment(screenshot: app.screenshot())
        apiKeyScreenshot.name = "引导-API密钥设置"
        apiKeyScreenshot.lifetime = .keepAlways
        add(apiKeyScreenshot)
        app.buttons["provider.cancel"].tap()
        XCTAssertTrue(waitUntilGone(apiKeyField, timeout: 5), app.debugDescription)

        let oauthPreset = app.buttons["onboarding.model.preset.openai-chatgpt"]
        XCTAssertTrue(scrollUntilHittable(oauthPreset, in: content), app.debugDescription)
        oauthPreset.tap()
        XCTAssertTrue(app.buttons["provider.oauth.login"].waitForExistence(timeout: 5), app.debugDescription)
        let oauthScreenshot = XCTAttachment(screenshot: app.screenshot())
        oauthScreenshot.name = "引导-OAuth设置"
        oauthScreenshot.lifetime = .keepAlways
        add(oauthScreenshot)
        app.buttons["provider.cancel"].tap()
        primaryAction.tap()

        XCTAssertTrue(
            app.descendants(matching: .any)["onboarding.web.step"].waitForExistence(timeout: 3),
            app.debugDescription
        )
        XCTAssertTrue(app.buttons["onboarding.web.search"].exists, app.debugDescription)
        XCTAssertTrue(app.buttons["onboarding.web.fetch"].exists, app.debugDescription)
        let webScreenshot = XCTAttachment(screenshot: app.screenshot())
        webScreenshot.name = "引导-Web设置"
        webScreenshot.lifetime = .keepAlways
        add(webScreenshot)
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
        openModelPicker()

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
            providerRow("deepseek/deepseek-flash").exists,
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
        let flash = providerRow("deepseek/deepseek-flash")
        search.typeText("flash")
        XCTAssertTrue(flash.waitForExistence(timeout: 3), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(flash, timeout: 3), app.debugDescription)
        flash.tap()
        app.buttons["composer.model.close"].tap()
        XCTAssertTrue(waitUntilGone(sheet, timeout: 5), app.debugDescription)
        let options = app.buttons["composer.model"]
        XCTAssertTrue(options.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(String(describing: options.value).contains("V4 Flash"), app.debugDescription)

        // Reopening pins that pick to the top under 最近使用 — a SECOND row for
        // the same model, distinct from the one under its provider.
        openModelPicker()
        XCTAssertTrue(sheet.waitForExistence(timeout: 5), app.debugDescription)
        let recent = app.buttons["composer.model.recent.row.deepseek/deepseek-flash"]
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

    private func allowSystemPermission(in alert: XCUIElement) -> Bool {
        for label in ["允许", "Allow", "好", "OK"] {
            let button = alert.buttons[label]
            if button.exists {
                button.tap()
                return true
            }
        }
        let buttons = alert.buttons.allElementsBoundByIndex
        guard let allowButton = buttons.last, buttons.count >= 2 else { return false }
        allowButton.tap()
        return true
    }

    /// A model can appear twice — once under 最近使用 and once under its own
    /// provider — so rows are addressed by their section.
    private func openModelPicker() {
        let model = app.buttons["composer.model"]
        XCTAssertTrue(model.waitForExistence(timeout: 3), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(model, timeout: 3), app.debugDescription)
        model.tap()
    }

    private func providerRow(_ reference: String) -> XCUIElement {
        app.buttons["composer.model.provider.row.\(reference)"]
    }

    /// The chat is the detail column of a `NavigationSplitView`. In compact width
    /// that collapses to a stack rooted at the sidebar, so "open the drawer" is
    /// the system back button — the leading navigation-bar item — not an
    /// app-drawn control any more.
    private func openDrawer() {
        let chatsTab = app.buttons["drawer.tab.chat"]
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

    private func waitUntilVerticallyStacked(
        _ upper: XCUIElement,
        above lower: XCUIElement,
        timeout: TimeInterval
    ) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            if upper.exists, lower.exists, upper.frame.maxY <= lower.frame.minY + 1 {
                return true
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        } while Date() < deadline
        return false
    }

}

/// Exercises the real engine startup path rather than the mock fixture used by
/// the rest of this file. On a clean install, engine construction may spend tens
/// of seconds installing the bundled Linux rootfs; onboarding must remain usable
/// while that work continues in the background.
final class StartupResponsivenessUITests: XCTestCase {
    func testOnboardingRemainsInteractiveDuringEngineBootstrap() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)"]

        let launchStarted = Date()
        app.launch()
        XCTAssertLessThan(
            Date().timeIntervalSince(launchStarted),
            8,
            "real engine bootstrap must not delay presentation of the setup UI"
        )

        let primaryAction = app.buttons["onboarding.primaryAction"]
        XCTAssertTrue(primaryAction.waitForExistence(timeout: 3), app.debugDescription)
        XCTAssertTrue(primaryAction.isHittable, app.debugDescription)

        let tapStarted = Date()
        primaryAction.tap()
        XCTAssertLessThan(
            Date().timeIntervalSince(tapStarted),
            2,
            "rootfs installation must not block setup interactions"
        )
        XCTAssertTrue(app.textFields.firstMatch.waitForExistence(timeout: 2), app.debugDescription)
    }
}
