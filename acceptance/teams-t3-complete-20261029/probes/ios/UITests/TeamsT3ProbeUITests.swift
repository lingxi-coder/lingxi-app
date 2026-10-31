import XCTest

/// Real SwiftUI + Firebase SDK interaction, backed only by the parent's local
/// HTTP/Auth/Firestore fixture. These tests never create or restore real users.
final class TeamsT3ProbeUITests: XCTestCase {
    private let values = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!
    override func setUpWithError() throws {
        continueAfterFailure = false
        guard let backend = values["TEAM_TEST_BACKEND"], backend.hasPrefix("http://127.0.0.1:"),
              let uid = values["TEAM_TEST_UID"], !uid.isEmpty else {
            throw XCTSkip("Requires explicit local TEAM_TEST_BACKEND and synthetic TEAM_TEST_UID")
        }
        app = XCUIApplication()
        app.launchArguments = ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        for (key, value) in values where key.hasPrefix("TEAM_TEST_") { app.launchEnvironment[key] = value }
        app.launch()
        XCTAssertTrue(app.staticTexts["teams.probe.ready"].waitForExistence(timeout: 30), "Demo Firebase bootstrap must finish before UI assertions: \(app.debugDescription)")
    }
    override func tearDownWithError() throws {
        if let app {
            let attachment = XCTAttachment(screenshot: app.screenshot())
            attachment.name = name
            attachment.lifetime = .keepAlways
            add(attachment)
            app.terminate()
        }
    }
    private var botName: String { values["TEAM_TEST_BOT_NAME"].flatMap { $0.isEmpty ? nil : $0 } ?? "Acceptance Bot" }
    private func dismissKeyboardIntro() {
        let text = "Speed up your typing by sliding your finger across the letters to compose a word."
        let intro = app.staticTexts.matching(NSPredicate(format: "label == %@", text)).firstMatch
        if intro.exists {
            let proceed = app.buttons["Continue"].firstMatch
            XCTAssertTrue(proceed.exists, "The known keyboard onboarding must expose Continue")
            proceed.tap()
        }
    }
    private func reveal(_ element: XCUIElement, down: Bool = true) {
        for _ in 0..<12 {
            dismissKeyboardIntro()
            if element.exists && element.isHittable { return }
            let frame = app.frame
            let navigation = app.navigationBars.firstMatch
            let keyboard = app.keyboards.firstMatch
            let top = navigation.exists ? navigation.frame.maxY + 12 : frame.minY + frame.height * 0.16
            let bottom = keyboard.exists ? min(frame.maxY - 24, keyboard.frame.minY - 16) : frame.maxY - 40
            guard bottom - top > 80 else { XCTFail("No visible List region above the keyboard for scrolling"); return }
            let high = (top + (bottom - top) * 0.2 - frame.minY) / frame.height
            let low = (top + (bottom - top) * 0.8 - frame.minY) / frame.height
            let start = app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: down ? low : high))
            let end = app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: down ? high : low))
            start.press(forDuration: 0.1, thenDragTo: end)
        }
    }
    private func enableToggle(_ row: XCUIElement, initialMessage: String) {
        reveal(row)
        XCTAssertTrue(row.exists)
        XCTAssertEqual(row.value as? String, "0", initialMessage)
        let thumb = row.switches.firstMatch
        XCTAssertTrue(thumb.exists, "SwiftUI exposes the interactive switch thumb inside the labelled row")
        thumb.tap()
        let enabled = XCTNSPredicateExpectation(predicate: NSPredicate(format: "value == %@", "1"), object: row)
        XCTAssertEqual(XCTWaiter.wait(for: [enabled], timeout: 5), .completed)
    }
    func testBotCardsAndPrivateDM() throws {
        let bot = app.staticTexts[botName].firstMatch
        reveal(bot)
        XCTAssertTrue(bot.exists)
        XCTAssertTrue(app.staticTexts["Bind and configure Bots on LingXi Desktop. Keep the owner's Desktop running to process requests."].exists)
        let chat = app.buttons["Chat with Bot"].firstMatch
        XCTAssertTrue(chat.isEnabled)
        chat.tap()
        let markRead = app.buttons["Mark read"]
        reveal(markRead)
        XCTAssertTrue(markRead.waitForExistence(timeout: 20))
        XCTAssertTrue(app.staticTexts["Sending here requests a reply from this Bot."].exists)
    }
    func testGroupRequiresExplicitBotTarget() throws {
        let disclosure = app.buttons["New conversation"]
        reveal(disclosure)
        XCTAssertTrue(disclosure.exists)
        disclosure.tap()
        let field = app.textFields["Title"]
        reveal(field)
        field.tap()
        dismissKeyboardIntro()
        field.typeText("Native acceptance \(UUID().uuidString.prefix(8))\n")
        let keyboardDismissed = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: app.keyboards.firstMatch)
        XCTAssertEqual(XCTWaiter.wait(for: [keyboardDismissed], timeout: 5), .completed)
        let type = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "Type")).firstMatch
        reveal(type)
        type.tap()
        app.buttons["Group"].firstMatch.tap()
        let participant = app.switches["Bot: " + botName]
        enableToggle(participant, initialMessage: "Bot participation must start unselected")
        let create = app.buttons["Create conversation"]
        reveal(create)
        XCTAssertTrue(create.isEnabled)
        create.tap()
        let targets = app.buttons["Request replies from Bots"]
        reveal(targets, down: false)
        XCTAssertTrue(targets.waitForExistence(timeout: 20))
        targets.tap()
        let target = app.switches[botName]
        XCTAssertTrue(app.staticTexts["Select Bots explicitly. Typing @name alone does not start a Bot."].exists)
        enableToggle(target, initialMessage: "Adding a Bot participant must not automatically target it")
        let messageField = NSPredicate(format: "identifier == %@ OR label == %@ OR placeholderValue == %@", "Message", "Message", "Message")
        let textView = app.textViews.matching(messageField).firstMatch
        let input = textView.exists ? textView : app.textFields.matching(messageField).firstMatch
        reveal(input)
        input.tap()
        dismissKeyboardIntro()
        let text = "Native explicit Bot request \(UUID().uuidString.prefix(8))"
        input.typeText(text)
        let send = app.buttons["Send"].firstMatch
        reveal(send)
        send.tap()
        let sent = app.staticTexts.matching(NSPredicate(format: "label == %@", text)).firstMatch
        reveal(sent, down: false)
        XCTAssertTrue(sent.waitForExistence(timeout: 20))
        let status = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", "Bot status:")).firstMatch
        reveal(status)
        XCTAssertTrue(status.waitForExistence(timeout: 20), "Original requester must see queued/offline run status")
    }
    func testSeededQuestionRoutesToOriginalHumanOnly() throws {
        guard let title = values["TEAM_TEST_CONVERSATION_TITLE"], !title.isEmpty,
              let question = values["TEAM_TEST_QUESTION_ID"], !question.isEmpty,
              let questionText = values["TEAM_TEST_QUESTION_TEXT"], !questionText.isEmpty else {
            throw XCTSkip("Question case requires seeded conversation title, question ID and body text")
        }
        let titlePattern = "^" + NSRegularExpression.escapedPattern(for: title) + "(?:, [0-9]+)?$"
        let thread = app.buttons.matching(NSPredicate(format: "label MATCHES %@", titlePattern)).firstMatch
        reveal(thread)
        XCTAssertTrue(thread.exists)
        thread.tap()
        let body = app.staticTexts.matching(NSPredicate(format: "label == %@", questionText)).firstMatch
        reveal(body)
        XCTAssertTrue(body.waitForExistence(timeout: 20), "Assert transcript loaded before checking routing")
        let rowID = body.identifier
        XCTAssertTrue(rowID.hasPrefix("teams.message.bot_"), "Question \(question) must resolve to its immutable Bot message row")
        let answerPredicate = NSPredicate(format: "identifier == %@ AND placeholderValue == %@", rowID, "Your answer")
        let textFields = app.textFields.matching(answerPredicate)
        let textViews = app.textViews.matching(answerPredicate)
        let send = app.buttons.matching(NSPredicate(format: "identifier == %@ AND label == %@", rowID, "Send answers")).firstMatch
        if values["TEAM_TEST_EXPECT_QUESTION"] == "hidden" {
            XCTAssertFalse(textFields.firstMatch.exists || textViews.firstMatch.exists || send.exists,
                           "Owner/other member/old generation must not receive answer controls in the question row")
        } else {
            reveal(send)
            XCTAssertTrue(send.waitForExistence(timeout: 15))
            let fields = textFields.allElementsBoundByIndex + textViews.allElementsBoundByIndex
            XCTAssertFalse(fields.isEmpty, "The original human must receive editable answer fields")
            for field in fields {
                reveal(field)
                field.tap()
                dismissKeyboardIntro()
                field.typeText("Current")
            }
            reveal(send)
            XCTAssertTrue(send.isEnabled)
            send.tap()
            reveal(body, down: false)

            // The form hides as soon as an intent is saved. Require the pending
            // answer to disappear AND a fresh server-derived run status, so a
            // locally queued answer alone cannot satisfy this journey.
            let pending = app.staticTexts["Bot answer"].firstMatch
            let acceptedStatus = app.staticTexts.matching(NSPredicate(format: "label IN %@", ["Bot status: Running", "Bot status: Completed"])).firstMatch
            let acknowledged = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                !textFields.firstMatch.exists && !textViews.firstMatch.exists && !send.exists
                    && !pending.exists && acceptedStatus.exists
            }, object: nil)
            XCTAssertEqual(XCTWaiter.wait(for: [acknowledged], timeout: 45), .completed,
                           "The answer must be acknowledged, leave the durable pending queue, and advance the server-derived run status: \(app.debugDescription)")
        }
    }
}
