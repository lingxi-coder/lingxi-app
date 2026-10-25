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

    func testCompactTranscriptSpacingAndIconOnlyCopy() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_COMPACT_TRANSCRIPT"] = "1"
        for theme in ["light", "dark"] {
            app.launchArguments = ["-AppleLanguages", "(en)", "-theme", theme]
            app.launch()
            let copies = app.buttons.matching(identifier: "conversation.message.user.copy")
            let copy = copies.element(boundBy: 1)
            XCTAssertTrue(copy.waitForExistence(timeout: 10), app.debugDescription)
            let interrupted = app.staticTexts["[Request interrupted by user]"]
            // Text is inset 12pt; the accessible copy target extends 8pt below
            // the compact footer so its visual gap stays distinct from hit padding.
            let footerBottom = copies.element(boundBy: 0).frame.maxY - 8
            XCTAssertEqual(interrupted.frame.minY - 12 - footerBottom, 16, accuracy: 1)
            XCTAssertTrue(copy.isHittable)
            XCTAssertFalse(copy.staticTexts["Copy"].exists)
            XCTAssertEqual(copy.frame.height, 44, accuracy: 1)
            let screenshot = XCTAttachment(screenshot: app.screenshot())
            screenshot.name = "Compact-transcript-\(theme)"
            screenshot.lifetime = .keepAlways
            add(screenshot)
            copy.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.05)).tap()
            // Inspect immediately: the native success feedback lasts 1.5s.
            XCTAssertEqual(copy.label, "Copied")
            XCTAssertFalse(copy.staticTexts["Copied"].exists)
            let feedback = XCTAttachment(screenshot: app.screenshot())
            feedback.name = "Compact-transcript-copied-\(theme)"
            feedback.lifetime = .keepAlways
            add(feedback)
            app.terminate()
        }
    }

    func testSideChatOpensWithComposerAndPreservesDrafts() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_SIDE_CHAT"] = "1"
        app.launch()
        let sideButton = app.buttons["conversation.side-chats"]
        XCTAssertTrue(sideButton.waitForExistence(timeout: 15), app.debugDescription)
        sideButton.tap()
        let hide = app.buttons["side-chat.hide"]
        XCTAssertTrue(hide.waitForExistence(timeout: 10), app.debugDescription)
        let input = app.otherElements["side-chat.sheet"].textViews["composer.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 15), app.debugDescription)
        input.tap()
        input.typeText("First side draft")
        XCTAssertEqual(input.value as? String, "First side draft")
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Side-chat-composer-keyboard"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        hide.tap()
        XCTAssertTrue(sideButton.waitForExistence(timeout: 5))
        sideButton.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        XCTAssertEqual(input.value as? String, "First side draft")
        app.buttons["side-chat.create"].tap()
        let deadline = Date().addingTimeInterval(15)
        while (input.value as? String) == "First side draft" && Date() < deadline { Thread.sleep(forTimeInterval: 0.1) }
        XCTAssertNotEqual(input.value as? String, "First side draft", app.debugDescription)
        input.tap()
        input.typeText("Second side draft")
        app.buttons["side-chat.select"].tap()
        let first = app.buttons["旁聊 1"]
        XCTAssertTrue(first.waitForExistence(timeout: 3), app.debugDescription)
        first.tap()
        XCTAssertEqual(input.value as? String, "First side draft", app.debugDescription)
    }

    func testAnsweredQuestionDisclosureMatchesReference() {
        let baseArguments = app.launchArguments
        for theme in ["light", "dark"] {
            app.terminate()
            app.launchArguments = baseArguments + ["-theme", theme]
            app.launchEnvironment["LINGXI_UI_TEST_ANSWERED_QUESTION"] = "1"
            app.launch()
            let answer = app.staticTexts["同时纳入原生实时对话"]
            XCTAssertTrue(answer.waitForExistence(timeout: 10), app.debugDescription)
            let toggle = app.buttons["conversation.tool-call.ui-answered-question"]
            XCTAssertTrue(toggle.isHittable, app.debugDescription)
            let image = XCTAttachment(screenshot: app.screenshot())
            image.name = "iOS-answered-question-\(theme)"
            image.lifetime = .keepAlways
            add(image)
            toggle.tap()
            XCTAssertFalse(answer.exists)
            toggle.tap()
            XCTAssertTrue(answer.waitForExistence(timeout: 3))
        }
    }

    func testScheduledTaskCenterShowsStatusesAndPreservesDraft() {
        openDrawer()
        app.buttons["drawer.entry.cron"].tap()
        let create = app.buttons["cron.add"]
        XCTAssertTrue(create.waitForExistence(timeout: 12), app.debugDescription)
        XCTAssertEqual(app.navigationBars.matching(identifier: "Scheduled").count, 1, app.debugDescription)
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

    func testTimelineKeepsTranscriptAndOpensToolResultsInRuntimeCenter() {
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
        XCTAssertFalse(app.buttons["conversation.timeline.tool-batch.timeline-tools:ui-shell"].exists)
        app.buttons["conversation.summary-menu"].tap()
        app.buttons["conversation.summary.category.tools"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["conversation.summary-sheet.tools"].firstMatch.waitForExistence(timeout: 5))
        let shellGroup = app.buttons["conversation.timeline.tool-batch.timeline-tools:ui-shell"]
        XCTAssertTrue(shellGroup.waitForExistence(timeout: 5), app.debugDescription)
        shellGroup.tap()
        let tool = app.descendants(matching: .any)["conversation.tool-call.ui-shell"]
        XCTAssertTrue(tool.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-shell.icon.terminal"].exists)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-read"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-search"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-read.icon.bookOpen"].exists)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-search.icon.search"].exists)
        XCTAssertFalse(app.staticTexts["Read fixture body"].exists)
        let readDisclosure = app.buttons["conversation.tool-call.ui-read.toggle"]
        XCTAssertTrue(readDisclosure.waitForExistence(timeout: 5), app.debugDescription)
        readDisclosure.tap()
        XCTAssertTrue(app.staticTexts["Read fixture body"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.staticTexts["Read completed"].exists, app.debugDescription)

        // A normal completed turn has no persistent runtime footer.
        XCTAssertTrue(waitUntilGone(llmStatus, timeout: 5), app.debugDescription)
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = "iOS-CodexTimeline"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testComposerTracksKeyboardAndKeepsVoiceModesSeparate() {
        let input = app.textViews["composer.input"]
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

    func testDetachedTranscriptStaysDetachedWhenViewportResizes() {
        defer { XCUIDevice.shared.orientation = .portrait }
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_MULTI_AGENT"] = "1"
        app.launch()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 12), app.debugDescription)

        let inlineChild = app.buttons["conversation.agent-row.ui-child"]
        XCTAssertTrue(inlineChild.waitForExistence(timeout: 8), app.debugDescription)
        inlineChild.tap()
        XCTAssertTrue(
            app.descendants(matching: .any)["conversation.agent-detail-sheet"].waitForExistence(timeout: 8),
            app.debugDescription
        )

        let transcript = app.scrollViews["conversation.agent-message-list"]
        XCTAssertTrue(transcript.waitForExistence(timeout: 5), app.debugDescription)
        let expandPrompt = app.buttons["conversation.message.user.toggle"]
        XCTAssertTrue(expandPrompt.waitForExistence(timeout: 5), app.debugDescription)
        expandPrompt.tap()

        // The expanded child prompt makes this transcript scrollable. Move into
        // its history, then verify a rotation keeps it detached from the tail.
        transcript.swipeDown()
        let jump = app.buttons["conversation.jump-to-latest"]
        XCTAssertTrue(jump.waitForExistence(timeout: 5), app.debugDescription)

        XCUIDevice.shared.orientation = .landscapeLeft
        XCTAssertTrue(jump.waitForExistence(timeout: 5), app.debugDescription)

        jump.tap()
        XCTAssertTrue(waitUntilGone(jump, timeout: 5), app.debugDescription)
    }

    func testVoiceTapRequestsPermissionThenShowsNativeBackendStateAndSettings() {
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
        let listening = app.staticTexts["正在聆听"]
        let unavailable = app.staticTexts["语音暂不可用"]
        XCTAssertTrue(listening.waitForExistence(timeout: 8) || unavailable.waitForExistence(timeout: 2), app.debugDescription)
        if unavailable.exists {
            XCTAssertTrue(app.buttons["voice.retry"].isHittable, app.debugDescription)
            XCTAssertTrue(app.buttons["voice.close"].isHittable, app.debugDescription)
        }
        XCTAssertFalse(app.buttons["voice.configure"].exists, app.debugDescription)

        let listeningScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        listeningScreenshot.name = "Voice-Permission-Granted-Listening"
        listeningScreenshot.lifetime = .keepAlways
        add(listeningScreenshot)

        app.buttons["voice.close"].tap()
        openDrawer()
        app.buttons["drawer.settings"].tap()
        XCTAssertTrue(app.staticTexts["设置"].waitForExistence(timeout: 8), app.debugDescription)

        // Voice has its own row on the grouped settings home.
        let voiceGroup = app.buttons["settings.group.voice"]
        XCTAssertTrue(voiceGroup.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(voiceGroup, timeout: 5), app.debugDescription)
        voiceGroup.tap()

        XCTAssertTrue(app.navigationBars["语音"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.staticTexts["语音输入"].exists, app.debugDescription)
        XCTAssertTrue(app.staticTexts["朗读"].exists, app.debugDescription)

        let settingsTopScreenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        settingsTopScreenshot.name = "Voice-Settings-Top"
        settingsTopScreenshot.lifetime = .keepAlways
        add(settingsTopScreenshot)

        let advanced = app.buttons["高级设置与权限"]
        XCTAssertTrue(scrollUntilHittable(advanced, in: app.collectionViews.firstMatch), app.debugDescription)
        if !app.staticTexts["麦克风"].exists { advanced.tap() }
        XCTAssertTrue(scrollUntilHittable(app.staticTexts["麦克风"], in: app.collectionViews.firstMatch), app.debugDescription)

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
        let modelOptions = app.buttons["composer.model.options"]
        XCTAssertTrue(modelOptions.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertFalse(app.switches["composer.model.fast-mode"].exists)
        modelOptions.tap()
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
        app.buttons["composer.model.options"].tap()
        XCTAssertTrue(toggle.waitForExistence(timeout: 5), app.debugDescription)
        toggle.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        app.buttons["composer.model.close"].tap()
        openModelPicker()
        providerRow("anthropic/claude-sonnet-5").tap()
        XCTAssertFalse(app.switches["composer.model.fast-mode"].exists)
        XCTAssertFalse(app.descendants(matching: .any)["composer.reasoning.slider"].exists)
        app.buttons["composer.model.close"].tap()
        XCTAssertFalse(String(describing: model.value).contains("Fast Mode"))
    }

    func testKeyboardPreservesComposerLayout() {
        let input = app.textViews["composer.input"]
        let ids = ["composer.attachment", "composer.model", "composer.controls", "composer.voice", "composer.flow"]
        let inputFrame = input.frame
        let frames = ids.map { app.buttons[$0].frame }
        for frame in frames {
            XCTAssertEqual(frame.midY, app.buttons["composer.voice"].frame.midY, accuracy: 1)
        }
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

        let input = app.textViews["composer.input"]
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
        let input = app.textViews["composer.input"]
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

        XCTAssertEqual(input.value as? String, "/review ")
        XCTAssertFalse(review.exists, app.debugDescription)

        let hint = app.staticTexts["composer.slash-argument-hint"]
        XCTAssertTrue(hint.waitForExistence(timeout: 2), app.debugDescription)
        XCTAssertEqual(hint.value as? String, "[instructions]")
        XCTAssertEqual(hint.frame.minY, input.frame.minY, accuracy: 4)
        let hintScreenshot = XCTAttachment(screenshot: app.screenshot())
        hintScreenshot.name = "iOS-slash-command-inline-argument-hint"
        hintScreenshot.lifetime = .keepAlways
        add(hintScreenshot)

        input.typeText("staged")
        XCTAssertFalse(hint.exists, app.debugDescription)
        XCTAssertEqual(input.value as? String, "/review staged")
        let send = app.buttons["composer.send"]
        XCTAssertTrue(send.isHittable, app.debugDescription)
        send.tap()
        XCTAssertTrue(
            app.staticTexts["/review staged"].waitForExistence(timeout: 5),
            app.debugDescription
        )
    }

    func testVoiceConfigurationGuidanceKeepsComposerAndSettingsReachable() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_VOICE_CONFIGURATION_REQUIRED"] = "1"
        app.launch()

        let input = app.textViews["composer.input"]
        let guidance = app.buttons["conversation.audio-setup.configure"]
        XCTAssertTrue(app.buttons["composer.flow"].waitForExistence(timeout: 8))
        app.buttons["composer.flow"].tap()
        XCTAssertTrue(guidance.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(input.exists)
        XCTAssertTrue(app.scrollViews["conversation.message-list"].exists)
        let image = XCTAttachment(screenshot: app.screenshot())
        image.name = "Voice-configuration-guidance"
        image.lifetime = .keepAlways
        add(image)
        app.buttons["conversation.audio-setup.dismiss"].tap()
        XCTAssertTrue(waitUntilGone(guidance, timeout: 5))
        XCTAssertTrue(app.buttons["composer.voice"].isEnabled)
        app.buttons["composer.voice"].tap()
        XCTAssertTrue(guidance.waitForExistence(timeout: 5), app.debugDescription)
        guidance.tap()
        XCTAssertTrue(app.navigationBars["语音"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.staticTexts["语音输入"].exists, app.debugDescription)
        XCTAssertTrue(app.staticTexts["朗读"].exists, app.debugDescription)
    }

    func testCancelledRunClosesEveryRunningRow() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_CANCELLED_RUN"] = "1"
        app.launch()

        XCTAssertFalse(app.descendants(matching: .any)["conversation.agent-run"].exists)
        XCTAssertTrue(app.buttons["conversation.summary-menu"].waitForExistence(timeout: 8))
        app.buttons["conversation.summary-menu"].tap()
        app.buttons["conversation.summary.category.tools"].tap()
        let batch = app.buttons.matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "conversation.timeline.tool-batch.")
        ).firstMatch
        XCTAssertTrue(batch.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertFalse(app.descendants(matching: .any)["conversation.tool-call.ui-web-search"].exists)
        batch.tap()
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-web-search"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.descendants(matching: .any)["conversation.tool-call.ui-shell"].waitForExistence(timeout: 5), app.debugDescription)
        for toolID in ["ui-web-search", "ui-shell"] {
            let row = app.descendants(matching: .any)["conversation.tool-call.\(toolID)"]
            XCTAssertEqual(row.value as? String, "已取消", app.debugDescription)
        }
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
        let submitInitially = app.buttons["chat.ask.submit"]
        XCTAssertTrue(submitInitially.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(cancel.isHittable, app.debugDescription)
        XCTAssertTrue(submitInitially.isHittable, app.debugDescription)
        XCTAssertFalse(submitInitially.isEnabled)
        let scroll = panel.scrollViews.firstMatch
        XCTAssertTrue(app.descendants(matching: .any)["chat.ask.other.0"].exists, app.debugDescription)
        // Every accordion row keeps its answer while the next row is opened;
        // the fixed submit footer stays reachable throughout the full form.
        for step in 0 ..< 4 {
            let option = app.buttons["chat.ask.option.\(step).0"]
            if !option.exists {
                let row = app.buttons["chat.ask.question.\(step)"]
                XCTAssertTrue(scrollUntilHittable(row, in: scroll), app.debugDescription)
                row.tap()
            }
            XCTAssertTrue(option.waitForExistence(timeout: 5), app.debugDescription)
            XCTAssertTrue(scrollUntilHittable(option, in: scroll), app.debugDescription)
            option.tap()
            XCTAssertTrue(cancel.isHittable, app.debugDescription)
            XCTAssertTrue(submitInitially.isHittable, app.debugDescription)
        }
        let submit = app.buttons["chat.ask.submit"]
        XCTAssertTrue(submit.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(submit.isHittable, app.debugDescription)
        XCTAssertTrue(submit.isEnabled, app.debugDescription)
        XCTAssertTrue(app.staticTexts["4 of 4 answered"].exists, app.debugDescription)

        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = "提问表单-满配选项"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        submit.tap()
        XCTAssertTrue(waitUntilGone(panel, timeout: 5), app.debugDescription)
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

        let detail = app.otherElements["conversation.agent-detail-sheet"].firstMatch
        let toggle = detail.buttons["conversation.message.user.toggle"].firstMatch
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
        let bubble = detail.otherElements["conversation.message.user"].firstMatch
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
        let collapsed = NSPredicate { _, _ in
            abs(bubble.frame.height - collapsedHeight) <= 1
        }
        expectation(for: collapsed, evaluatedWith: bubble)
        waitForExpectations(timeout: 5)
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
        XCTAssertFalse(sheet.textViews["composer.input"].exists, app.debugDescription)
        app.buttons["conversation.agent-detail.close"].tap()
        XCTAssertTrue(sheet.waitForNonExistence(timeout: 5), app.debugDescription)

        XCTAssertTrue(app.staticTexts["Hello! I'm ready to help with your software engineering tasks."].waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 5), app.debugDescription)
    }

    /// The sidebar has two entries (scheduled tasks, projects) and one recents
    /// list with a mode filter — no per-mode tabs and no per-project cards. A
    /// project is created on the projects page and starts its sessions from
    /// its own page.
    func testSidebarEntriesCreateProjectAndStartSession() {
        openDrawer()
        XCTAssertTrue(app.buttons["drawer.entry.cron"].exists)
        XCTAssertTrue(app.buttons["drawer.entry.projects"].exists)
        XCTAssertTrue(app.buttons["drawer.filter.all"].exists)
        XCTAssertTrue(app.buttons["drawer.filter.chat"].exists)
        XCTAssertTrue(app.buttons["drawer.filter.code"].exists)

        app.buttons["drawer.entry.projects"].tap()
        let create = app.buttons["projects.create"]
        XCTAssertTrue(create.waitForExistence(timeout: 8), app.debugDescription)
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

        // Creating a project lands on its page, where the name is the title.
        XCTAssertTrue(
            app.descendants(matching: .any)["project.page"].waitForExistence(timeout: 10),
            app.debugDescription
        )
        XCTAssertTrue(app.staticTexts[projectName].waitForExistence(timeout: 5), app.debugDescription)

        app.buttons["project.new-session"].tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
    }

    /// The drawer supports edge opening while leaving the chat visible.
    func testEdgeSwipeOpensTheSidebar() {
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 8), app.debugDescription)

        let origin = app.coordinate(withNormalizedOffset: CGVector(dx: 0.01, dy: 0.5))
        let target = app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5))
        origin.press(forDuration: 0.05, thenDragTo: target)

        XCTAssertTrue(
            app.buttons["drawer.filter.chat"].waitForExistence(timeout: 8),
            app.debugDescription
        )

        app.buttons["drawer.close"].tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
    }

    func testCompactSidebarLeavesContentVisibleAndDismissesBySwipeAndBackdrop() {
        openDrawer()
        let search = app.textFields["drawer.search"]
        XCTAssertTrue(search.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertLessThan(search.frame.maxX, app.frame.maxX - 44)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "iPhone-SwiftUI-drawer-open"
        screenshot.lifetime = .keepAlways
        add(screenshot)

        let origin = app.coordinate(withNormalizedOffset: CGVector(dx: 0.7, dy: 0.5))
        let target = app.coordinate(withNormalizedOffset: CGVector(dx: 0.03, dy: 0.5))
        origin.press(forDuration: 0.05, thenDragTo: target)
        XCTAssertTrue(waitUntilGone(app.buttons["drawer.close"], timeout: 5), app.debugDescription)
        XCTAssertTrue(app.buttons["drawer.open"].isHittable)

        openDrawer()
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.97, dy: 0.5)).tap()
        XCTAssertTrue(waitUntilGone(app.buttons["drawer.close"], timeout: 5), app.debugDescription)
        XCTAssertTrue(app.buttons["drawer.open"].isHittable)
    }

    func testCompactSidebarVerticalScrollDoesNotDismiss() {
        openDrawer()
        let origin = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.72))
        let target = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.35))
        origin.press(forDuration: 0.05, thenDragTo: target)
        XCTAssertTrue(app.buttons["drawer.close"].isHittable, app.debugDescription)
    }

    func testCompactSidebarIgnoresSwipesStartingInsideTheConversation() {
        let origin = app.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.5))
        let target = app.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5))
        origin.press(forDuration: 0.05, thenDragTo: target)
        XCTAssertFalse(app.buttons["drawer.close"].exists, app.debugDescription)
        XCTAssertTrue(app.buttons["drawer.open"].isHittable)
    }

    func testCompactSidebarShortDragSettlesBackOpen() {
        openDrawer()
        let origin = app.coordinate(withNormalizedOffset: CGVector(dx: 0.7, dy: 0.5))
        let target = app.coordinate(withNormalizedOffset: CGVector(dx: 0.6, dy: 0.5))
        origin.press(forDuration: 0.05, thenDragTo: target, withVelocity: .slow, thenHoldForDuration: 0.4)
        XCTAssertTrue(app.buttons["drawer.close"].isHittable, app.debugDescription)
        app.buttons["drawer.close"].tap()
        XCTAssertTrue(waitUntilGone(app.buttons["drawer.close"], timeout: 5))
    }

    func testCompactSidebarClosingDismissesSearchKeyboard() {
        openDrawer()
        let search = app.textFields["drawer.search"]
        search.tap()
        search.typeText("drawer search")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))
        app.buttons["drawer.close"].tap()
        XCTAssertTrue(waitUntilGone(app.keyboards.firstMatch, timeout: 5), app.debugDescription)
        XCTAssertTrue(app.buttons["drawer.open"].isHittable)
    }

    func testCompactSidebarAdaptsToRotationWhileOpen() {
        openDrawer()
        defer { XCUIDevice.shared.orientation = .portrait }
        XCUIDevice.shared.orientation = .landscapeLeft
        let search = app.textFields["drawer.search"]
        XCTAssertTrue(search.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertLessThan(search.frame.maxX, app.frame.maxX - 44)
        XCTAssertTrue(app.buttons["drawer.close"].isHittable)
        app.buttons["drawer.close"].tap()
        XCTAssertTrue(waitUntilGone(app.buttons["drawer.close"], timeout: 5))
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

    /// Closing the container and navigating in the same update must retain
    /// both sheet and pushed-page presentations.
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
        app.buttons["drawer.terminal"].tap()
        XCTAssertTrue(
            app.descendants(matching: .any)["terminal.root"].waitForExistence(timeout: 15),
            app.debugDescription
        )
        app.navigationBars.firstMatch.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 10), app.debugDescription)
    }

    /// The name is now WIDER than what the test asserts, and the name is kept
    /// only because `docs/superpowers/plans/` still cites it: scheduled tasks
    /// are no longer hidden anywhere — the sidebar renders its cron entry
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

        let modelsGroup = app.buttons["settings.group.models"]
        XCTAssertTrue(modelsGroup.waitForExistence(timeout: 5))
        modelsGroup.tap()
        // The models page is the provider list: no intermediate credentials page.
        XCTAssertTrue(app.buttons["provider.add"].waitForExistence(timeout: 5), app.debugDescription)
        let hubShot = XCTAttachment(screenshot: app.screenshot())
        hubShot.name = "模型设置-首页"
        hubShot.lifetime = .keepAlways
        add(hubShot)
        // Adding a provider is a modal: pick a preset, then fill in the editor.
        let addProvider = app.buttons["provider.add"]
        XCTAssertTrue(scrollUntilHittable(addProvider, in: app.collectionViews.firstMatch), app.debugDescription)
        addProvider.tap()
        XCTAssertTrue(app.staticTexts["Anthropic"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["OpenAI"].exists)
        XCTAssertTrue(app.staticTexts["Kimi"].exists)
        let presetShot = XCTAttachment(screenshot: app.screenshot())
        presetShot.name = "模型设置-选择提供商"
        presetShot.lifetime = .keepAlways
        add(presetShot)

        let deepSeekPreset = app.buttons["provider.preset.deepseek"]
        XCTAssertTrue(deepSeekPreset.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(scrollUntilHittable(deepSeekPreset, in: app.collectionViews.firstMatch), app.debugDescription)
        deepSeekPreset.tap()

        let keyField = app.secureTextFields["provider.api-key"]
        let visibility = app.buttons["provider.api-key.visibility"]
        let clear = app.buttons["provider.api-key.clear"]
        XCTAssertTrue(keyField.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(visibility.exists)
        XCTAssertFalse(clear.exists)
        let editorShot = XCTAttachment(screenshot: app.screenshot())
        editorShot.name = "模型设置-编辑提供商"
        editorShot.lifetime = .keepAlways
        add(editorShot)

        keyField.tap()
        keyField.typeText("sk-ui-draft")
        XCTAssertTrue(visibility.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertLessThanOrEqual(keyField.frame.maxX, visibility.frame.minX + 0.5)
        XCTAssertFalse(clear.exists)

        app.keyboards.buttons["return"].tap()
        // The editor is pushed inside the add modal: back to the preset list,
        // then cancel the modal.
        app.navigationBars["DeepSeek"].buttons["添加 LLM"].tap()
        let cancelAdd = app.buttons["provider.cancel"]
        XCTAssertTrue(cancelAdd.waitForExistence(timeout: 5), app.debugDescription)
        cancelAdd.tap()
        XCTAssertTrue(waitUntilGone(keyField, timeout: 5), app.debugDescription)
        let passwordAlert = app.alerts["Save Password?"]
        let passwordSheet = app.sheets["Save Password?"]
        if passwordAlert.waitForExistence(timeout: 1) || passwordSheet.waitForExistence(timeout: 1) {
            let savePasswordPrompt = passwordAlert.exists ? passwordAlert : passwordSheet
            let notNow = savePasswordPrompt.buttons["Not Now"]
            XCTAssertTrue(notNow.waitForExistence(timeout: 2), app.debugDescription)
            notNow.tap()
            XCTAssertTrue(waitUntilGone(savePasswordPrompt, timeout: 5), app.debugDescription)
        }
        let settingsBack = app.navigationBars["模型"].buttons["设置"]
        XCTAssertTrue(settingsBack.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(waitUntilHittable(settingsBack, timeout: 5), app.debugDescription)
        settingsBack.tap()
        let settingsClose = app.descendants(matching: .any)["settings.close"].firstMatch
        XCTAssertTrue(settingsClose.waitForExistence(timeout: 5), app.debugDescription)
        settingsClose.tap()
        openDrawer()
        // Scheduled tasks are an unconditional sidebar entry, so what this leg
        // can honestly pin is that the provider round-trip leaves the sidebar
        // intact.
        XCTAssertTrue(app.buttons["drawer.filter.chat"].exists, app.debugDescription)
        XCTAssertTrue(app.buttons["drawer.entry.cron"].exists, app.debugDescription)
    }

    /// The chat's top bar keeps the summary menu and a new-chat button; the
    /// theme toggle moved into Appearance and the session details into the
    /// session panel that the title opens.
    func testChatTopBarUsesSessionNameEvenForAnEmptyNamedSession() {
        app.terminate()
        let title = "修复 iOS 导航栏与会话标题同步"
        app.launchEnvironment["LINGXI_UI_TEST_SESSION_TITLE"] = title
        app.launchArguments += ["-conversation.global.code.active-session", "ui-named-session"]
        app.launch()
        let header = app.buttons["conversation.project-context"]
        XCTAssertTrue(header.waitForExistence(timeout: 12), app.debugDescription)
        XCTAssertEqual(header.label, title)
        XCTAssertTrue(app.buttons["conversation.new-chat"].isHittable)
        XCTAssertTrue(app.buttons["conversation.summary-menu"].isHittable)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Chat-Session-Title"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        header.tap()
        let panel = app.descendants(matching: .any)["session-panel"].firstMatch
        XCTAssertTrue(panel.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(panel.staticTexts[title].exists, app.debugDescription)
    }

    func testChatTopBarHasNewChatAndTitleButOnlySummariesInTheMenu() {
        let menu = app.buttons["conversation.summary-menu"]
        XCTAssertTrue(menu.waitForExistence(timeout: 8), app.debugDescription)
        XCTAssertGreaterThanOrEqual(menu.frame.width, 28)
        XCTAssertGreaterThanOrEqual(menu.frame.height, 32)
        XCTAssertTrue(app.buttons["conversation.new-chat"].exists, app.debugDescription)
        XCTAssertTrue(app.buttons["conversation.project-context"].exists, app.debugDescription)

        menu.tap()
        XCTAssertTrue(
            app.buttons["conversation.summary.category.tools"].waitForExistence(timeout: 5),
            app.debugDescription
        )
        XCTAssertFalse(app.buttons["conversation.session-details"].exists, app.debugDescription)
        XCTAssertFalse(app.buttons["conversation.theme-toggle"].exists, app.debugDescription)
    }

    func testSummaryMenuOpensBottomSheetDismissesAndPreservesDraft() {
        app.terminate()
        app.launchEnvironment["LINGXI_UI_TEST_MULTI_AGENT"] = "1"
        app.launch()
        XCTAssertTrue(chatSurface.waitForExistence(timeout: 12), app.debugDescription)

        let input = app.textViews["composer.input"]
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

    func testSessionDetailsOpensFromSessionPanel() {
        app.buttons["conversation.project-context"].tap()
        XCTAssertTrue(
            app.descendants(matching: .any)["session-panel"].waitForExistence(timeout: 8),
            app.debugDescription
        )
        let details = app.buttons["session-panel.details"]
        if !details.isHittable { app.collectionViews.allElementsBoundByIndex.last!.swipeUp() }
        XCTAssertTrue(details.waitForExistence(timeout: 5), app.debugDescription)
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

    func testAppIntegrationExposesRealSystemActions() {
        openDrawer()
        app.buttons["drawer.settings"].tap()
        XCTAssertTrue(app.staticTexts["设置"].waitForExistence(timeout: 5))
        Thread.sleep(forTimeInterval: 1)

        let phoneGroup = app.buttons["settings.group.phone"]
        XCTAssertTrue(phoneGroup.waitForExistence(timeout: 5), app.debugDescription)
        phoneGroup.tap()
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
        let passwordAlert = app.alerts["Save Password?"]
        let passwordSheet = app.sheets["Save Password?"]
        if passwordAlert.waitForExistence(timeout: 1) || passwordSheet.waitForExistence(timeout: 1) {
            let savePasswordPrompt = passwordAlert.exists ? passwordAlert : passwordSheet
            savePasswordPrompt.buttons["Not Now"].tap()
            XCTAssertTrue(waitUntilGone(savePasswordPrompt, timeout: 5), app.debugDescription)
        }

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
        XCTAssertTrue(String(describing: options.value).contains("Flash"), app.debugDescription)

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

    /// Compact layout has an explicit trigger; iPad keeps its system control.
    private func openDrawer() {
        let chatsTab = app.buttons["drawer.filter.chat"]
        if chatsTab.exists && chatsTab.isHittable { return }

        let compactTrigger = app.buttons["drawer.open"]
        let trigger = compactTrigger.exists ? compactTrigger : app.navigationBars.firstMatch.buttons.element(boundBy: 0)
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
        XCTAssertTrue(waitUntilHittable(chatsTab, timeout: 5), app.debugDescription)
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
        app.launchEnvironment["LINGXI_FORCE_ONBOARDING"] = "1"
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
