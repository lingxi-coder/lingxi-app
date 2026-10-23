import XCTest

final class TranscriptInteractionUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        super.setUp()
        continueAfterFailure = false
        app = XCUIApplication()
        app.launchArguments += ["-AppleLanguages", "(en)"]
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchEnvironment["LINGXI_UI_TEST_TRANSCRIPT_SCROLL"] = "1"
        app.launch()
        XCTAssertTrue(transcript.waitForExistence(timeout: 15), app.debugDescription)
    }

    private var transcript: XCUIElement { app.scrollViews["conversation.message-list"] }
    private var input: XCUIElement { app.textViews["composer.input"] }
    private var jump: XCUIElement { app.buttons["conversation.jump-to-latest"] }
    private var tail: XCUIElement {
        app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "Scroll stream tail")).firstMatch
    }

    private func waitUntil(_ predicate: @escaping () -> Bool, timeout: TimeInterval = 8) -> Bool {
        let expectation = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in predicate() }, object: nil)
        return XCTWaiter.wait(for: [expectation], timeout: timeout) == .completed
    }

    private func send(_ text: String) {
        input.tap()
        input.typeText(text)
        app.buttons["composer.send"].tap()
    }

    private func startStreaming() {
        send("Stream scroll probe")
        XCTAssertTrue(tail.waitForExistence(timeout: 5), app.debugDescription)
    }

    private func assertTailVisible(file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(waitUntil {
            self.tail.isHittable && self.tail.frame.maxY <= self.transcript.frame.maxY + 2
        }, app.debugDescription, file: file, line: line)
        XCTAssertFalse(jump.exists, file: file, line: line)
    }

    func testStreamingReplyFollowsThroughKeyboardOpenAndCloseWithoutStealingDraft() {
        startStreaming()
        assertTailVisible()
        app.buttons["composer.keyboard.dismiss"].tap()
        XCTAssertTrue(waitUntil { !self.app.keyboards.firstMatch.exists })
        let before = tail.label
        XCTAssertTrue(waitUntil { self.tail.label != before })
        assertTailVisible()
        input.tap()
        input.typeText("Unsent scroll draft")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 4))
        let withKeyboard = tail.label
        XCTAssertTrue(waitUntil { self.tail.label != withKeyboard })
        assertTailVisible()
        XCTAssertEqual(input.value as? String, "Unsent scroll draft")
        XCTAssertTrue(app.keyboards.firstMatch.exists)
        app.buttons["composer.keyboard.dismiss"].tap()
        XCTAssertTrue(waitUntil { !self.app.keyboards.firstMatch.exists })
        assertTailVisible()
    }

    func testHistoryReadingSurvivesStreamingAndKeyboardThenQueuedSendRejoins() {
        startStreaming()
        transcript.swipeDown()
        XCTAssertTrue(jump.waitForExistence(timeout: 5), app.debugDescription)
        let markers = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", "Scroll history "))
            .allElementsBoundByIndex.filter {
                $0.isHittable && $0.frame.minY > self.transcript.frame.minY + 15
                    && $0.frame.maxY < self.transcript.frame.midY
            }
        XCTAssertFalse(markers.isEmpty, app.debugDescription)
        let marker = markers[0]
        let offset = marker.frame.minY - transcript.frame.minY
        let settle = expectation(description: "more streaming deltas")
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { settle.fulfill() }
        wait(for: [settle], timeout: 3)
        XCTAssertEqual(marker.frame.minY - transcript.frame.minY, offset, accuracy: 2)
        XCTAssertTrue(jump.exists)
        input.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 4))
        XCTAssertEqual(marker.frame.minY - transcript.frame.minY, offset, accuracy: 2)
        input.typeText("Queued scroll prompt")
        app.buttons["composer.send"].tap()
        XCTAssertTrue(waitUntil { self.app.staticTexts["Queued scroll prompt"].isHittable && !self.jump.exists })
        XCTAssertTrue(app.keyboards.firstMatch.exists)
    }

    func testSendingFromHistoryReturnsToNewestMessage() {
        transcript.swipeDown()
        XCTAssertTrue(jump.waitForExistence(timeout: 5))
        send("Send after browsing history")
        XCTAssertTrue(waitUntil { self.app.staticTexts["Send after browsing history"].isHittable && !self.jump.exists })
        XCTAssertTrue(app.staticTexts["已记入。继续追问。"].waitForExistence(timeout: 5))
        XCTAssertFalse(jump.exists)
    }

    func testManualAssistantCollapseSurvivesStreamingCompletionAndCanReopen() {
        startStreaming()
        let toggle = app.buttons["conversation.message.assistant.toggle"]
        let stop = app.buttons["composer.stop"]
        // The production policy exposes the toggle after 80 explicit lines.
        // Wait for that threshold while the 24-second fixture is still live.
        XCTAssertTrue(toggle.waitForExistence(timeout: 20), app.debugDescription)
        XCTAssertEqual(toggle.label, "Collapse")
        XCTAssertTrue(stop.exists, "Collapse must happen before the reply finishes")
        toggle.tap()
        XCTAssertTrue(waitUntil { toggle.label == "Expand" }, app.debugDescription)

        let whileCollapsed = tail.label
        XCTAssertTrue(waitUntil { self.tail.label != whileCollapsed },
            "More deltas must arrive while the assistant reply is collapsed")
        XCTAssertEqual(toggle.label, "Expand")
        XCTAssertTrue(stop.exists)
        capture("assistant-collapsed-during-stream")
        XCTAssertTrue(waitUntil({ !stop.exists }, timeout: 30),
            "Wait for the actual streaming-to-idle transition", file: #filePath, line: #line)
        XCTAssertEqual(toggle.label, "Expand", "Completion must preserve the user's explicit collapse")
        XCTAssertTrue(tail.label.contains("Scroll stream tail 80"), app.debugDescription)
        capture("assistant-still-collapsed-after-completion")

        toggle.tap()
        XCTAssertTrue(waitUntil { toggle.label == "Collapse" }, app.debugDescription)
        assertTailVisible()
        XCTAssertTrue(tail.label.contains("Scroll stream tail 80"))
        XCTAssertFalse(stop.exists)
    }

    private func capture(_ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
