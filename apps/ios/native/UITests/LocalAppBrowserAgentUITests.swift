import XCTest

/// Verifies the app-scoped foreground handoff UI without assuming which
/// external browser the person selected in iOS Settings.
final class LocalAppBrowserAgentUITests: XCTestCase {
    private func stagedAppID() throws -> String {
        let id = ProcessInfo.processInfo.environment["LINGXI_AGENT_BROWSER_APP_ID"] ?? ""
        try XCTSkipUnless(id.range(of: #"^a[0-9a-f]{32}$"#, options: .regularExpression) != nil,
                          "Stage an isolated Local App and pass LINGXI_AGENT_BROWSER_APP_ID")
        try XCTSkipUnless(ProcessInfo.processInfo.environment["SIMULATOR_DEVICE_NAME"] == nil,
                          "Requires an iOS 26 physical device")
        return id
    }

    private func agentReviewURL(appID: String, runID: String) throws -> URL {
        var parts = URLComponents()
        parts.scheme = "lingxi"
        parts.host = "apps"
        parts.path = "/\(appID)"
        parts.queryItems = [
            URLQueryItem(name: "review", value: "agent"),
            URLQueryItem(name: "runId", value: runID),
            URLQueryItem(name: "revision", value: ProcessInfo.processInfo.environment["LINGXI_AGENT_BROWSER_REVISION"] ?? "1")
        ]
        return try XCTUnwrap(parts.url)
    }

    private func selectedBrowserBundleID() throws -> String {
        let bundleID = ProcessInfo.processInfo.environment["LINGXI_SYSTEM_BROWSER_BUNDLE_ID"] ?? ""
        try XCTSkipUnless(!bundleID.isEmpty && bundleID.contains("."),
                          "Pass the bundle ID observed for the iOS system-selected browser.")
        return bundleID
    }

    private func pageText(_ app: XCUIApplication) -> String {
        textValues(app).joined(separator: "\n")
    }

    private func textValues(_ app: XCUIApplication, deduplicating: Bool = true) -> [String] {
        let leaves = app.otherElements.allElementsBoundByIndex.filter {
            $0.children(matching: .any).count == 0
        }
        let elements = app.staticTexts.allElementsBoundByIndex
            + app.textViews.allElementsBoundByIndex
            + app.textFields.allElementsBoundByIndex
            + app.buttons.allElementsBoundByIndex
            + leaves
        var seen = Set<String>()
        var values: [String] = []
        for element in elements {
            var elementValues = Set<String>()
            let candidates = [element.label, element.value as? String ?? ""]
            for candidate in candidates where !candidate.isEmpty && elementValues.insert(candidate).inserted {
                if deduplicating && !seen.insert(candidate).inserted { continue }
                values.append(candidate)
            }
        }
        return values
    }

    private func decodeRunResultPayload(_ text: String) -> [String: Any]? {
        guard let data = text.data(using: .utf8),
              let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let input = payload["RunAgentInput"] as? [String: Any],
              input["messages"] as? [[String: Any]] != nil,
              let runAgentResult = payload["RunAgentResult"] as? [String: Any],
              let result = runAgentResult["result"] as? [String: Any],
              result["answer"] as? String != nil else {
            return nil
        }
        return payload
    }

    private func runResultPayload(in app: XCUIApplication) -> [String: Any]? {
        let values = textValues(app, deduplicating: false)
        for value in values {
            if let payload = decodeRunResultPayload(value) { return payload }
        }

        // WebKit can expose a <pre> as ordered leaf text nodes. Stitch only
        // candidate fragments from those nodes, and accept them only if they
        // decode as the fixture's RunAgentInput/RunAgentResult JSON object.
        let starts = values.indices.filter { index in
            let value = values[index].trimmingCharacters(in: .whitespacesAndNewlines)
            return value.hasPrefix("{") || value.contains("\"RunAgentInput\"")
        }
        for start in starts {
            var fragments: [String] = []
            let endLimit = min(values.count, start + 160)
            for end in start..<endLimit {
                fragments.append(values[end])
                guard fragments.count > 1 else { continue }
                for separator in ["", "\n"] {
                    if let payload = decodeRunResultPayload(fragments.joined(separator: separator)) {
                        return payload
                    }
                }
            }
        }
        return nil
    }

    private func inputMessages(in payload: [String: Any]) -> [[String: Any]]? {
        (payload["RunAgentInput"] as? [String: Any])?["messages"] as? [[String: Any]]
    }

    private func textContent(in message: [String: Any]) -> String? {
        if let content = message["content"] as? String { return content }
        guard let parts = message["content"] as? [[String: Any]] else { return nil }
        let textParts = parts.compactMap { part -> String? in
            guard part["type"] as? String == "text" else { return nil }
            return part["text"] as? String
        }
        return textParts.isEmpty ? nil : textParts.joined()
    }

    private func answer(in payload: [String: Any]) -> String? {
        (((payload["RunAgentResult"] as? [String: Any])?["result"] as? [String: Any])?["answer"] as? String)
    }

    private func attachScreen(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func waitForPageText(
        _ marker: String,
        in app: XCUIApplication,
        timeout: TimeInterval,
        scroll: Bool = false
    ) -> String {
        let deadline = Date().addingTimeInterval(timeout)
        var lastText = ""
        var nextSwipe = Date().addingTimeInterval(2)
        while Date() < deadline {
            lastText = pageText(app)
            if lastText.localizedCaseInsensitiveContains(marker) { return lastText }
            if scroll, Date() >= nextSwipe {
                app.swipeUp()
                nextSwipe = Date().addingTimeInterval(1.5)
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        }
        XCTFail("Timed out waiting for browser text \(marker.debugDescription). Snapshot: \(lastText.prefix(3000))")
        return lastText
    }

    private func tapPageButton(_ title: String, in app: XCUIApplication, timeout: TimeInterval = 20) {
        let button = app.buttons.matching(NSPredicate(format: "label CONTAINS %@", title)).firstMatch
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if button.exists && button.isHittable && button.isEnabled {
                button.tap()
                return
            }
            if button.exists, button.frame.maxY < app.frame.minY + 100 {
                app.swipeDown()
            } else {
                app.swipeUp()
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        }
        XCTFail("Browser button \(title.debugDescription) was not enabled and hittable. Snapshot: \(pageText(app).prefix(3000))")
    }

    private func enterChatText(_ text: String, in app: XCUIApplication) {
        _ = waitForPageText("Message to send in this Chat", in: app, timeout: 20, scroll: true)
        let field = app.textViews.matching(NSPredicate(format: "label CONTAINS %@", "Message to send in this Chat")).firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 10), "Chat message field did not appear. Snapshot: \(pageText(app).prefix(2000))")
        field.tap()
        field.typeText(text)
    }

    private func regexMatches(_ pattern: String, in text: String) -> [String] {
        guard let regex = try? NSRegularExpression(pattern: pattern, options: [.dotMatchesLineSeparators]) else { return [] }
        let range = NSRange(text.startIndex..<text.endIndex, in: text)
        return regex.matches(in: text, range: range).compactMap { match in
            guard match.numberOfRanges > 1, let capture = Range(match.range(at: 1), in: text) else { return nil }
            return String(text[capture])
        }
    }

    private func eventRecords(_ app: XCUIApplication) -> [[String: Any]]? {
        let candidates = textValues(app) + [pageText(app)]
        for candidate in candidates {
            let eventText: String
            if let heading = candidate.range(of: "Standard BaseEvents") {
                eventText = String(candidate[heading.upperBound...])
            } else {
                eventText = candidate
            }
            guard let start = eventText.firstIndex(of: "["),
                  let end = eventText.lastIndex(of: "]"), start <= end else { continue }
            let jsonText = String(eventText[start...end])
            guard let data = jsonText.data(using: .utf8),
                  let records = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]],
                  records.contains(where: { $0["type"] as? String == "RUN_STARTED" }) else { continue }
            return records
        }
        return nil
    }

    private func assertLifecycle(
        _ app: XCUIApplication,
        outcome expectedOutcome: String,
        nativeRunPrefix: String? = nil
    ) -> String {
        guard let events = eventRecords(app) else {
            XCTFail("Could not decode the accessible standard BaseEvent JSON. Snapshot: \(pageText(app).prefix(5000))")
            return ""
        }
        let starts = events.filter { $0["type"] as? String == "RUN_STARTED" }
        let finishes = events.filter { $0["type"] as? String == "RUN_FINISHED" }
        XCTAssertEqual(starts.count, 1, "Expected one standard RUN_STARTED event.")
        XCTAssertEqual(finishes.count, 1, "Expected one standard RUN_FINISHED event.")
        guard let startedID = starts.first?["runId"] as? String,
              let finishedID = finishes.first?["runId"] as? String else {
            XCTFail("RUN_STARTED and RUN_FINISHED must each echo the public runId.")
            return ""
        }
        XCTAssertEqual(startedID, finishedID, "RUN_STARTED and RUN_FINISHED must echo the same public runId.")
        if let nativeRunPrefix {
            XCTAssertTrue(startedID.hasPrefix(nativeRunPrefix), "The public event runId must match the native handoff prefix.")
        }
        let actualOutcome = (finishes.first?["outcome"] as? [String: Any])?["type"] as? String
        if let actualOutcome {
            XCTAssertEqual(actualOutcome, expectedOutcome, "Unexpected RUN_FINISHED outcome.")
        } else if expectedOutcome == "cancelled" {
            XCTFail("Cancellation must be reported as RUN_FINISHED outcome=cancelled.")
        }
        return startedID
    }

    private func startBrowserRun(_ browser: XCUIApplication, host: XCUIApplication) -> String? {
        confirmLingxiWakeOpenPromptIfPresent(browser: browser, host: host)
        let start = host.buttons["开始 Agent 并返回应用"]
        XCTAssertTrue(start.waitForExistence(timeout: 45), "The browser run should reach the trusted native handoff. Host UI: \(host.debugDescription)")
        let waitingCopy = "浏览器 Agent 请求等待后台任务许可。运行："
        let waitingLabel = host.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", waitingCopy)).firstMatch
        let nativeText = waitingLabel.exists ? waitingLabel.label : pageText(host)
        let nativeRunPrefix = regexMatches(#"运行：([0-9a-fA-F]{8})"#, in: nativeText).first
        XCTAssertNotNil(nativeRunPrefix, "The trusted Apps page must identify the pending public run. Host UI: \(host.debugDescription)")
        attachScreen("native-agent-handoff-\(nativeRunPrefix ?? "run")")
        print("IOS_AGUI_NATIVE_HANDOFF_READY runPrefix=\(nativeRunPrefix ?? "unreadable") source=system-browser-deep-link")
        XCTAssertTrue(start.isHittable, "The explicit native Start Agent action must be available to the user.")
        start.tap()
        XCTAssertTrue(browser.wait(for: .runningForeground, timeout: 60), "iOS did not return to the observed system-selected browser after the native lease action.")
        return nativeRunPrefix
    }

    private func confirmLingxiWakeOpenPromptIfPresent(
        browser: XCUIApplication,
        host: XCUIApplication,
        timeout: TimeInterval = 8
    ) {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let alert = browser.alerts.firstMatch
            if alert.exists {
                let alertText = ([alert.label] + alert.staticTexts.allElementsBoundByIndex.map(\.label))
                    .joined(separator: " ").lowercased()
                let identifiesLingxi = alertText.contains("lingxi") || alertText.contains("灵犀")
                let buttons = alert.buttons.allElementsBoundByIndex
                let buttonLabels = buttons.map { $0.label.lowercased() }
                let openIndex = buttonLabels.firstIndex { $0 == "open" || $0 == "打开" }
                let cancelIndex = buttonLabels.firstIndex { $0 == "cancel" || $0 == "取消" }
                guard identifiesLingxi,
                      buttonLabels.count == 2,
                      let openIndex,
                      let cancelIndex,
                      openIndex != cancelIndex else {
                    XCTFail("Refusing to accept an unrelated browser alert during the Local App Agent wake. Alert: \(alertText) Buttons: \(buttonLabels)")
                    return
                }
                buttons[openIndex].tap()
                print("IOS_AGUI_LINGXI_WAKE_OPEN_CONFIRMED source=browser-alert")
                return
            }
            if host.state == .runningForeground { return }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        }
    }

    private func runPageButton(
        _ title: String,
        expectedSuccess: String,
        expectedAttachment: String,
        expectedPartType: String,
        browser: XCUIApplication,
        host: XCUIApplication
    ) -> String {
        tapPageButton(title, in: browser)
        let runIDBeforeHandoff = startBrowserRun(browser, host: host)
        var text = waitForPageText(expectedSuccess, in: browser, timeout: 240, scroll: true)
        if !text.contains("RUN_FINISHED") {
            text = waitForPageText("RUN_FINISHED", in: browser, timeout: 20, scroll: true)
        }
        XCTAssertTrue(text.localizedCaseInsensitiveContains(expectedAttachment), "Expected the file to appear in redacted RunAgentInput. Snapshot: \(text.prefix(3000))")
        XCTAssertTrue(text.localizedCaseInsensitiveContains(expectedPartType), "Expected the official ContentPart type in RunAgentInput. Snapshot: \(text.prefix(3000))")
        XCTAssertTrue(text.contains("[base64 omitted"), "The browser evidence should show redacted inline bytes, not raw file data.")
        let runID = assertLifecycle(browser, outcome: "success", nativeRunPrefix: runIDBeforeHandoff)
        XCTAssertTrue(text.contains("RunAgentResult"), "Successful AG-UI run did not expose the official RunAgentResult.")
        attachScreen("browser-\(expectedPartType)-run-finished-\(runID.prefix(8))")
        print("IOS_AGUI_BROWSER_RUN_PASS kind=\(expectedPartType) runId=\(runID) terminal=RUN_FINISHED outcome=success")
        return runID
    }

    func testSystemSelectedBrowserRunsAGUIFileChatAndCancellation() throws {
        continueAfterFailure = false
        try XCTSkipUnless(ProcessInfo.processInfo.environment["SIMULATOR_DEVICE_NAME"] == nil,
                          "Requires the physical FullDebug iPhone and a real BGContinuedProcessingTask.")
        let appID = try stagedAppID()
        let browserID = try selectedBrowserBundleID()
        let browser = XCUIApplication(bundleIdentifier: browserID)
        let host = XCUIApplication()

        host.activate()
        host.open(try XCTUnwrap(URL(string: "lingxi://apps/\(appID)")))
        XCTAssertTrue(browser.wait(for: .runningForeground, timeout: 60), "The normal Local App launch must open iOS's system-selected browser.")
        _ = waitForPageText("Host Agent ready", in: browser, timeout: 60)
        attachScreen("system-selected-browser-agent-page")
        print("IOS_AGUI_BROWSER_PAGE_READY appId=\(appID) browserBundleId=\(browserID) resolver=system-openURL")

        _ = runPageButton("Run public image", expectedSuccess: "image run succeeded", expectedAttachment: "public-apple.jpg", expectedPartType: "image", browser: browser, host: host)
        _ = runPageButton("Run text file", expectedSuccess: "document run succeeded", expectedAttachment: "acceptance-stationery.txt", expectedPartType: "document", browser: browser, host: host)

        tapPageButton("Create Chat", in: browser)
        let createdChat = waitForPageText("Created host-owned persistent Chat", in: browser, timeout: 30, scroll: true)
        XCTAssertTrue(createdChat.contains("threadId"), "Chat creation must expose its host-owned threadId.")
        let markerSuffix = String(UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(12))
        let contextMarker = "IOS_CHAT_CONTEXT_\(markerSuffix)"
        let firstChatPrompt = "Remember this marker for my next message: \(contextMarker). For this turn, reply with exactly context-seed."
        enterChatText(firstChatPrompt, in: browser)
        tapPageButton("Send Chat turn", in: browser)
        let firstChatRun = startBrowserRun(browser, host: host)
        var firstChatEvidence = waitForPageText("chat run succeeded", in: browser, timeout: 240, scroll: true)
        if !firstChatEvidence.contains("RUN_FINISHED") {
            firstChatEvidence = waitForPageText("RUN_FINISHED", in: browser, timeout: 20, scroll: true)
        }
        let firstChatRunID = assertLifecycle(browser, outcome: "success", nativeRunPrefix: firstChatRun)
        let firstPayload = try XCTUnwrap(runResultPayload(in: browser), "Could not decode the official RunAgentResult JSON from the page result element. Snapshot: \(pageText(browser).prefix(3000))")
        let firstInput = try XCTUnwrap(firstPayload["RunAgentInput"] as? [String: Any], "The first RunAgentInput was missing.")
        XCTAssertEqual(firstInput["runId"] as? String, firstChatRunID, "The first RunAgentInput must match the public run reported by lifecycle events.")
        let firstMessages = try XCTUnwrap(inputMessages(in: firstPayload), "The first RunAgentInput must expose messages.")
        XCTAssertEqual(firstMessages.count, 1, "The first Chat RunAgentInput must contain exactly its new user turn.")
        let firstMessage = try XCTUnwrap(firstMessages.first, "The first RunAgentInput has no user message.")
        XCTAssertEqual(firstMessage["role"] as? String, "user")
        let firstInputText = try XCTUnwrap(textContent(in: firstMessage), "The first user message must include text content.")
        XCTAssertTrue(firstInputText.contains(contextMarker), "The first user prompt must contain the unique synthetic context marker.")
        let firstAnswer = try XCTUnwrap(answer(in: firstPayload), "RunAgentResult.result.answer was missing from the first Chat turn.")
        XCTAssertEqual(firstAnswer, "context-seed", "The first Chat turn should answer exactly context-seed.")
        attachScreen("browser-chat-first-run-\(firstChatRunID.prefix(8))")

        tapPageButton("Read-only reconnect to last run", in: browser)
        let reconnect = waitForPageText("Read-only reconnect succeeded", in: browser, timeout: 60, scroll: true)
        XCTAssertTrue(reconnect.contains("no new run started"), "connectAgent must replay the existing run without submitting another run.")
        XCTAssertTrue(reconnect.contains(firstChatRunID), "Read-only replay must refer to the same public runId.")
        tapPageButton("Refresh Chat history", in: browser)
        let history = waitForPageText(contextMarker, in: browser, timeout: 30, scroll: true)
        XCTAssertTrue(history.contains("context-seed"), "Canonical Chat history should preserve the first assistant turn separately from the next RunAgentInput.")
        XCTAssertTrue(history.contains("assistant"), "Canonical Chat history should include the assistant reply.")

        let followupPrompt = "What exact marker did I ask you to remember in the previous message? Reply with only the marker."
        XCTAssertFalse(followupPrompt.contains(contextMarker), "The second user prompt must not reveal the marker.")
        enterChatText(followupPrompt, in: browser)
        tapPageButton("Send Chat turn", in: browser)
        let followupRun = startBrowserRun(browser, host: host)
        var followup = waitForPageText("chat run succeeded", in: browser, timeout: 240, scroll: true)
        if !followup.contains("RUN_FINISHED") {
            followup = waitForPageText("RUN_FINISHED", in: browser, timeout: 20, scroll: true)
        }
        let followupRunID = assertLifecycle(browser, outcome: "success", nativeRunPrefix: followupRun)
        let followupPayload = try XCTUnwrap(runResultPayload(in: browser), "Could not decode the official RunAgentResult JSON from the follow-up page result element. Snapshot: \(pageText(browser).prefix(3000))")
        let followupInput = try XCTUnwrap(followupPayload["RunAgentInput"] as? [String: Any], "The follow-up RunAgentInput was missing.")
        XCTAssertEqual(followupInput["runId"] as? String, followupRunID, "The follow-up RunAgentInput must match the public run reported by lifecycle events.")
        let followupMessages = try XCTUnwrap(inputMessages(in: followupPayload), "The second RunAgentInput must expose messages.")
        XCTAssertEqual(followupMessages.count, 1, "The second RunAgentInput must send only the new user turn, not replay prior Chat history.")
        let followupMessage = try XCTUnwrap(followupMessages.first, "The second RunAgentInput has no user message.")
        XCTAssertEqual(followupMessage["role"] as? String, "user")
        let followupInputText = try XCTUnwrap(textContent(in: followupMessage), "The second user message must include text content.")
        XCTAssertEqual(followupInputText, followupPrompt, "The second RunAgentInput should contain only the new prompt.")
        XCTAssertFalse(followupInputText.contains(contextMarker), "The second RunAgentInput must not include the marker as prompt text or replayed history.")
        let followupAnswer = try XCTUnwrap(answer(in: followupPayload), "RunAgentResult.result.answer was missing from the follow-up Chat turn.")
        XCTAssertEqual(followupAnswer, contextMarker, "The second Chat turn must recall the marker from the host-persisted prior turn.")
        attachScreen("browser-chat-followup-\(followupRunID.prefix(8))")

        enterChatText("Write a detailed 1500 word explanation of how a paper notebook is made. Continue until complete.", in: browser)
        tapPageButton("Send Chat turn", in: browser)
        let cancelledRunPrefix = startBrowserRun(browser, host: host)
        let cancel = browser.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Cancel active run")).firstMatch
        let cancelDeadline = Date().addingTimeInterval(45)
        while Date() < cancelDeadline && (!cancel.exists || !cancel.isEnabled || !cancel.isHittable) {
            if cancel.exists, cancel.frame.maxY < browser.frame.minY + 100 {
                browser.swipeDown()
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.25))
        }
        XCTAssertTrue(cancel.exists && cancel.isEnabled, "The running task must expose its cancel action before completion. Snapshot: \(pageText(browser).prefix(3000))")
        cancel.tap()
        attachScreen("browser-chat-cancel-requested")
        var cancelled = waitForPageText("RUN_FINISHED", in: browser, timeout: 120, scroll: true)
        if !cancelled.localizedCaseInsensitiveContains("cancelled") {
            cancelled = waitForPageText("cancelled", in: browser, timeout: 30, scroll: true)
        }
        let cancelledRunID = assertLifecycle(browser, outcome: "cancelled", nativeRunPrefix: cancelledRunPrefix)
        XCTAssertFalse(cancelled.contains("RUN_ERROR"), "Cancellation should be a RUN_FINISHED cancelled outcome, not a success or error masquerading as completion.")
        attachScreen("browser-chat-cancelled-\(cancelledRunID.prefix(8))")
        print("IOS_AGUI_BROWSER_CANCEL_PASS runId=\(cancelledRunID) terminal=RUN_FINISHED outcome=cancelled")
    }

    func testNativeForegroundActionIsScopedToAGUIRun() throws {
        continueAfterFailure = false
        let id = try stagedAppID()
        let runID = "shortcuts+browser/\(UUID().uuidString)"
        let host = XCUIApplication()
        host.activate()
        host.open(try agentReviewURL(appID: id, runID: runID))

        let start = host.buttons["开始 Agent 并返回应用"]
        XCTAssertTrue(start.waitForExistence(timeout: 20), "The app should require a foreground action before it returns to the external browser.")
        XCTAssertTrue(host.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", String(runID.prefix(8)))).firstMatch.exists)
        print("IOS_BROWSER_AGENT_HANDOFF_UI PASS ag-ui-run-id explicit-native-action system-default-browser-neutral")
    }
}
