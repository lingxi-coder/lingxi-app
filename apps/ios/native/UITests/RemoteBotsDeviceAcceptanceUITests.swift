import XCTest

/// Opt-in acceptance against an isolated Desktop Bot using the phone's existing account.
final class RemoteBotsDeviceAcceptanceUITests: XCTestCase {
    func testRealBotConversationSurvivesBackAndColdLaunch() throws {
        let environment = ProcessInfo.processInfo.environment
        let hostID = environment["LINGXI_BOT_ACCEPTANCE_HOST_ID"] ?? ""
        let botID = environment["LINGXI_BOT_ACCEPTANCE_BOT_ID"] ?? ""
        try XCTSkipUnless(!hostID.isEmpty && !botID.isEmpty,
                          "Pass the host and Bot IDs of an isolated acceptance Desktop")
        try XCTSkipUnless(environment["SIMULATOR_DEVICE_NAME"] == nil,
                          "Requires a physical iPhone and its existing signed-in account")
        continueAfterFailure = false

        let app = XCUIApplication()
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)"]
        let marker = "BOT_E2E_IOS_" + UUID().uuidString.replacingOccurrences(of: "-", with: "")

        func openBot() {
            // Remote Control is the Bots tab now (it left the drawer); the bar button appears once the real app has started.
            app.openBotsTab(timeout: 30)
            let host = app.buttons["bots.host.\(hostID)"]
            XCTAssertTrue(host.waitForExistence(timeout: 45), "Acceptance host must be available to the existing account")
            // Sections start expanded; connect only if presence has not auto-connected the paired host.
            let connect = app.buttons["bots.connect.\(hostID)"]
            if connect.waitForExistence(timeout: 3) { connect.tap() }
            let bot = app.buttons["bots.bot.\(hostID).\(botID)"]
            XCTAssertTrue(bot.waitForExistence(timeout: 45), "Fresh native FFI must deliver the host's Bot list")
            if !app.buttons["bots.new-chat.\(hostID).\(botID)"].exists { bot.tap() }
            XCTAssertTrue(app.buttons["bots.new-chat.\(hostID).\(botID)"].waitForExistence(timeout: 20))
        }

        func waitForAssistant() {
            let reply = NSPredicate { _, _ in
                app.descendants(matching: .any)
                    .matching(identifier: "conversation.message.assistant")
                    .allElementsBoundByIndex.contains { element in
                        element.label.contains(marker) || element.staticTexts.allElementsBoundByIndex.contains {
                            $0.label.contains(marker)
                        }
                    }
            }
            XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: reply, object: nil)], timeout: 120), .completed,
                           "The assistant response must arrive through the real Remote Control connection")
        }

        app.terminate()
        app.launch()
        openBot()
        let priorThreads = Set(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@", "remote.session."))
            .allElementsBoundByIndex.map(\.identifier))
        app.buttons["bots.new-chat.\(hostID).\(botID)"].tap()
        let input = app.textViews["composer.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 30))
        let enabled = XCTNSPredicateExpectation(predicate: NSPredicate(format: "enabled == true"), object: input)
        XCTAssertEqual(XCTWaiter.wait(for: [enabled], timeout: 30), .completed)
        input.tap()
        input.typeText("Reply with exactly this text: \(marker). Do not use tools.")
        let send = app.buttons["composer.send"]
        XCTAssertTrue(send.waitForExistence(timeout: 10))
        send.tap()
        waitForAssistant()

        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = "real-remote-bot-assistant-response"
        attachment.lifetime = .keepAlways
        add(attachment)

        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.buttons["bots.new-chat.\(hostID).\(botID)"].waitForExistence(timeout: 20), "Back must reactivate the Bot page")
        let newThread = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@", "remote.session."))
            .allElementsBoundByIndex.first { !priorThreads.contains($0.identifier) }
        let threadID = try XCTUnwrap(newThread?.identifier, "The newly created chat must appear in this Bot's thread list")

        app.terminate()
        app.launch()
        openBot()
        let restored = app.buttons[threadID]
        XCTAssertTrue(restored.waitForExistence(timeout: 20), "The Bot's chat must survive a real cold launch")
        restored.tap()
        waitForAssistant()
    }
}
