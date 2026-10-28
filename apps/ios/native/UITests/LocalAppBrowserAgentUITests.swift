import XCTest

private struct BrowserRunInputEvidence {
    let input: [String: Any]
    let runID: String
    let threadID: String
}

private struct BrowserRunResultEvidence {
    let runAgentResult: [String: Any]
    let reconnect: [String: Any]?
}

/// Verifies the app-scoped foreground handoff UI without assuming which
/// external browser the person selected in iOS Settings.
final class LocalAppBrowserAgentUITests: XCTestCase {
    private let publicImagePrompt = "Describe the main food visible in this public image and estimate how many pieces are visible."
    private let documentPrompt = "Read the attached text file and return its item and count."
    private var targetFixtureAppID: String?
    private var targetFixtureRevision: Int?
    private var targetFixtureIdentityVerified = false

    private func stagedAppID() throws -> String {
        let id = ProcessInfo.processInfo.environment["LINGXI_AGENT_BROWSER_APP_ID"] ?? ""
        try XCTSkipUnless(id.range(of: #"^a[0-9a-f]{32}$"#, options: .regularExpression) != nil,
                          "Stage an isolated Local App and pass LINGXI_AGENT_BROWSER_APP_ID")
        try XCTSkipUnless(ProcessInfo.processInfo.environment["SIMULATOR_DEVICE_NAME"] == nil,
                          "Requires an iOS 26 physical device")
        let revisionText = ProcessInfo.processInfo.environment["LINGXI_AGENT_BROWSER_REVISION"] ?? "1"
        guard let revision = Int(revisionText), revision > 0 else {
            throw XCTSkip("LINGXI_AGENT_BROWSER_REVISION must be a positive integer")
        }
        targetFixtureAppID = id
        targetFixtureRevision = revision
        targetFixtureIdentityVerified = false
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
            URLQueryItem(name: "revision", value: String(targetFixtureRevision ?? 1))
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

    private func accessibleLabels(_ app: XCUIApplication) -> [String] {
        let leaves = app.otherElements.allElementsBoundByIndex.filter {
            $0.children(matching: .any).count == 0
        }
        let elements = app.staticTexts.allElementsBoundByIndex
            + app.textViews.allElementsBoundByIndex
            + app.textFields.allElementsBoundByIndex
            + app.buttons.allElementsBoundByIndex
            + leaves
        return elements.flatMap { [$0.label, $0.value as? String ?? ""] }.filter { !$0.isEmpty }
    }

    private func hasTargetFixtureTitle(_ app: XCUIApplication) -> Bool {
        accessibleLabels(app).contains("Agent device acceptance")
    }

    private func localAppInfo(in app: XCUIApplication) -> (id: String, revision: Int)? {
        guard hasTargetFixtureTitle(app),
              let payload = protocolPayload(in: app, accepting: { payload in
                  payload["localAppInfo"] as? [String: Any] != nil
                      && payload["sdkPendingDatabase"] != nil
                      && payload["fixtureStorageBytes"] != nil
              }),
              let info = payload["localAppInfo"] as? [String: Any],
              let id = info["id"] as? String,
              id.range(of: #"^[A-Za-z0-9._-]{1,128}$"#, options: .regularExpression) != nil,
              let revision = info["revision"] as? Int else {
            return nil
        }
        return (id, revision)
    }

    private func isTargetFixture(_ app: XCUIApplication) -> Bool {
        guard hasTargetFixtureTitle(app),
              let expectedAppID = targetFixtureAppID,
              let expectedRevision = targetFixtureRevision,
              let info = localAppInfo(in: app) else {
            return false
        }
        return info.id == expectedAppID && info.revision == expectedRevision
    }

    private func waitForTargetFixtureIdentity(
        _ app: XCUIApplication,
        timeout: TimeInterval = 30
    ) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        var nextSwipe = Date().addingTimeInterval(1)
        var diagnosticsOpened = false
        let disclosure = app.descendants(matching: .any).matching(
            NSPredicate(format: "label CONTAINS %@", "Safe diagnostics")
        ).firstMatch

        while Date() < deadline {
            if isTargetFixture(app) {
                targetFixtureIdentityVerified = true
                return true
            }
            if hasTargetFixtureTitle(app), !diagnosticsOpened {
                if disclosure.exists && disclosure.isHittable {
                    disclosure.tap()
                    diagnosticsOpened = true
                } else if Date() >= nextSwipe {
                    app.swipeUp()
                    nextSwipe = Date().addingTimeInterval(1)
                }
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.25))
        }
        XCTFail("The browser page did not report the staged Local App id and revision. \(safePageSummary(app))")
        return false
    }

    private func safeStatusBucket(_ app: XCUIApplication) -> String {
        let labels = accessibleLabels(app)
        let known: [(String, String)] = [
            ("Host Agent ready", "host-ready"),
            ("Read-only reconnect succeeded", "read-only-reconnect-succeeded"),
            ("Read-only reconnect to saved run", "read-only-reconnect-pending"),
            ("Read-only recovery for accepted run", "read-only-recovery-pending"),
            ("image run succeeded", "image-run-succeeded"),
            ("document run succeeded", "document-run-succeeded"),
            ("chat run succeeded", "chat-run-succeeded"),
            ("Run failed or was interrupted", "run-failed-or-interrupted"),
            ("Submitting standard AG-UI input", "submitting-single-run"),
            ("Submitting the new Chat message", "submitting-chat-run"),
            ("SDK handoff failed", "sdk-handoff-failed"),
            ("The SDK accepted saved run", "sdk-handoff-accepted"),
            ("Cancellation requested for public run", "cancel-requested"),
            ("Host Agent unavailable", "host-unavailable"),
        ]
        for label in labels {
            if let match = known.first(where: { label.hasPrefix($0.0) }) { return match.1 }
        }
        return "unavailable"
    }

    private func safeHandoffErrorCode(_ app: XCUIApplication) -> String {
        guard let diagnostics = protocolPayload(in: app, accepting: { payload in
            payload["sdkHandoffErrorCode"] != nil
                && payload["sdkPendingDatabase"] != nil
                && payload["fixtureStorageBytes"] != nil
        }) else { return "unavailable" }
        let handoff = diagnostics["sdkHandoff"] as? [String: Any]
        let code = diagnostics["sdkHandoffErrorCode"] as? String ?? handoff?["code"] as? String
        guard let code,
              code.range(of: #"^[A-Za-z0-9._-]{1,80}$"#, options: .regularExpression) != nil else {
            return "none"
        }
        return code
    }

    private func safeTerminal(_ app: XCUIApplication, runID: String?) -> String {
        guard let runID, let events = eventRecords(app, runID: runID) else { return "unavailable" }
        let runEvents = events.filter { $0["runId"] as? String == runID }
        if runEvents.contains(where: { $0["type"] as? String == "RUN_ERROR" }) { return "RUN_ERROR" }
        if let finish = runEvents.first(where: { $0["type"] as? String == "RUN_FINISHED" }) {
            let outcome = (finish["outcome"] as? [String: Any])?["type"] as? String
            if let outcome, ["success", "cancelled"].contains(outcome) {
                return "RUN_FINISHED/\(outcome)"
            }
            return "RUN_FINISHED/other"
        }
        return runEvents.contains(where: { $0["type"] as? String == "RUN_STARTED" }) ? "RUN_STARTED" : "none"
    }

    private func safePublicID(_ value: String?) -> String {
        guard let value,
              value.range(of: #"^[A-Za-z0-9_-]{1,128}$"#, options: .regularExpression) != nil else {
            return "unavailable"
        }
        return value
    }

    private func safePageSummary(
        _ app: XCUIApplication,
        input suppliedInput: BrowserRunInputEvidence? = nil
    ) -> String {
        guard isTargetFixture(app),
              let fixtureAppID = targetFixtureAppID,
              let fixtureRevision = targetFixtureRevision,
              let info = localAppInfo(in: app) else {
            return "fixtureIdentity=unverified/wrong-app"
        }
        let input = suppliedInput ?? runInputEvidence(in: app)
        let messages = input.flatMap { $0.input["messages"] as? [[String: Any]] } ?? []
        let allowedTypes = Set(["text", "image", "document", "file"])
        let partTypes = Set(messages.flatMap { message -> [String] in
            guard let parts = message["content"] as? [[String: Any]] else {
                return (message["content"] as? String).map { _ in ["text"] } ?? []
            }
            return parts.compactMap { $0["type"] as? String }.filter { allowedTypes.contains($0) }
        }).sorted().joined(separator: ",")
        let messageCount = input == nil ? "unavailable" : String(messages.count)
        return "fixtureIdentity=verified title=Agent-device-acceptance appId=\(safePublicID(fixtureAppID)) revision=\(fixtureRevision) sdkId=\(safePublicID(info.id)) sdkRevision=\(info.revision) runId=\(safePublicID(input?.runID)) threadId=\(safePublicID(input?.threadID)) messageCount=\(messageCount) partTypes=[\(partTypes)] status=\(safeStatusBucket(app)) terminal=\(safeTerminal(app, runID: input?.runID)) handoffErrorCode=\(safeHandoffErrorCode(app))"
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

    private func protocolPayload(
        in app: XCUIApplication,
        accepting predicate: ([String: Any]) -> Bool
    ) -> [String: Any]? {
        func decode(_ text: String) -> [String: Any]? {
            guard let data = text.data(using: .utf8),
                  let json = try? JSONSerialization.jsonObject(with: data),
                  let payload = json as? [String: Any],
                  predicate(payload) else {
                return nil
            }
            return payload
        }

        let values = textValues(app, deduplicating: false)
        for value in values {
            if let payload = decode(value) { return payload }
        }

        // WebKit can expose a <pre> as ordered leaf text nodes. Stitch only
        // candidate fragments from those nodes, and accept them only if the
        // caller's protocol-shape predicate matches the decoded JSON object.
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
                    if let payload = decode(fragments.joined(separator: separator)) {
                        return payload
                    }
                }
            }
        }
        return nil
    }

    private func runResultValue(_ value: Any?) -> [String: Any]? {
        guard let runAgentResult = value as? [String: Any],
              runAgentResult["result"] as? [String: Any] != nil,
              runAgentResult["newMessages"] as? [[String: Any]] != nil else {
            return nil
        }
        return runAgentResult
    }

    private func runInputEvidence(from payload: [String: Any]) -> BrowserRunInputEvidence? {
        guard let input = payload["RunAgentInput"] as? [String: Any],
              let runID = input["runId"] as? String, !runID.isEmpty,
              let threadID = input["threadId"] as? String, !threadID.isEmpty,
              input["messages"] as? [[String: Any]] != nil else {
            return nil
        }
        return BrowserRunInputEvidence(input: input, runID: runID, threadID: threadID)
    }

    private func runInputPayload(in app: XCUIApplication) -> [String: Any]? {
        guard isTargetFixture(app) else { return nil }
        return protocolPayload(in: app) { payload in
            self.runInputEvidence(from: payload) != nil
        }
    }

    private func runInputEvidence(in app: XCUIApplication) -> BrowserRunInputEvidence? {
        guard let payload = runInputPayload(in: app) else { return nil }
        return runInputEvidence(from: payload)
    }

    private func sameRunInput(_ lhs: BrowserRunInputEvidence, _ rhs: BrowserRunInputEvidence) -> Bool {
        guard lhs.runID == rhs.runID, lhs.threadID == rhs.threadID,
              let left = try? JSONSerialization.data(withJSONObject: lhs.input, options: [.sortedKeys]),
              let right = try? JSONSerialization.data(withJSONObject: rhs.input, options: [.sortedKeys]) else {
            return false
        }
        return left == right
    }

    private func waitForRunInput(
        in app: XCUIApplication,
        excludingRunID: String? = nil,
        expectedUserText: String? = nil,
        timeout: TimeInterval = 30
    ) -> BrowserRunInputEvidence? {
        let deadline = Date().addingTimeInterval(timeout)
        var lastText = ""
        var nextSwipe = Date().addingTimeInterval(2)
        while Date() < deadline {
            lastText = pageText(app)
            if let evidence = runInputEvidence(in: app) {
                let excluded = excludingRunID.map { evidence.runID == $0 } ?? false
                let expectedTextMatches = expectedUserText.map {
                    hasUserMessage(evidence.input, exactText: $0)
                } ?? true
                if !excluded && expectedTextMatches {
                    return evidence
                }
            }
            if Date() >= nextSwipe {
                app.swipeUp()
                nextSwipe = Date().addingTimeInterval(1.5)
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        }
        XCTFail("Could not observe the expected current RunAgentInput. \(safePageSummary(app))")
        return nil
    }

    private func hasUserMessage(_ input: [String: Any], exactText: String) -> Bool {
        guard let messages = input["messages"] as? [[String: Any]] else { return false }
        return messages.contains {
            $0["role"] as? String == "user" && textContent(in: $0) == exactText
        }
    }

    private func runResultEvidence(
        in app: XCUIApplication,
        input: BrowserRunInputEvidence,
        requireReconnect: Bool = false
    ) -> BrowserRunResultEvidence? {
        guard isTargetFixture(app) else { return nil }
        if !requireReconnect,
           let direct = protocolPayload(in: app, accepting: { payload in
               guard let shownInput = self.runInputEvidence(from: payload),
                     self.sameRunInput(shownInput, input),
                     self.runResultValue(payload["RunAgentResult"]) != nil else {
                   return false
               }
               return true
           }),
           let result = runResultValue(direct["RunAgentResult"]) {
            return BrowserRunResultEvidence(runAgentResult: result, reconnect: nil)
        }

        guard let recovered = protocolPayload(in: app, accepting: { payload in
            guard let reconnect = payload["reconnect"] as? [String: Any],
                  reconnect["mode"] as? String == "read-only",
                  reconnect["runId"] as? String == input.runID,
                  reconnect["threadId"] as? String == input.threadID,
                  self.runResultValue(payload["RunAgentResult"]) != nil,
                  let callbackTypes = payload["callbackEventTypes"] as? [String],
                  callbackTypes.contains("RUN_FINISHED"),
                  !callbackTypes.contains("RUN_ERROR") else {
                return false
            }
            return true
        }),
              let reconnect = recovered["reconnect"] as? [String: Any],
              let result = runResultValue(recovered["RunAgentResult"]) else {
            return nil
        }

        if let visibleInput = runInputEvidence(in: app), !sameRunInput(visibleInput, input) {
            return nil
        }
        // The read-only result view omits input; use only the input captured
        // before handoff and only after reconnect metadata matches it exactly.
        return BrowserRunResultEvidence(runAgentResult: result, reconnect: reconnect)
    }

    private func inputMessages(in input: [String: Any]) -> [[String: Any]]? {
        input["messages"] as? [[String: Any]]
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

    private func answer(in runAgentResult: [String: Any]) -> String? {
        ((runAgentResult["result"] as? [String: Any])?["answer"] as? String)
    }

    private func publicImageRunIDs(in payload: [String: Any]) -> (runID: String, threadID: String)? {
        guard let evidence = runInputEvidence(from: payload),
              UUID(uuidString: evidence.runID) != nil,
              UUID(uuidString: evidence.threadID) != nil,
              hasExactFileInput(
                  evidence,
                  prompt: publicImagePrompt,
                  partType: "image",
                  filename: "public-apple.jpg",
                  mimeType: "image/jpeg",
                  byteCount: 576216
              ) else {
            return nil
        }
        return (evidence.runID, evidence.threadID)
    }

    private func hasExactFileInput(
        _ evidence: BrowserRunInputEvidence,
        prompt: String,
        partType expectedPartType: String,
        filename: String,
        mimeType: String,
        byteCount: Int
    ) -> Bool {
        guard let messages = evidence.input["messages"] as? [[String: Any]],
              messages.count == 1,
              let message = messages.first,
              message["role"] as? String == "user",
              let parts = message["content"] as? [[String: Any]],
              parts.count == 2 else {
            return false
        }

        guard let textPart = parts.first,
              textPart["type"] as? String == "text",
              textPart["text"] as? String == prompt,
              let filePart = parts.last,
              filePart["type"] as? String == expectedPartType,
              let source = filePart["source"] as? [String: Any],
              source["type"] as? String == "data",
              source["mimeType"] as? String == mimeType,
              source["value"] as? String == "[base64 omitted; \(byteCount) bytes]",
              let metadata = filePart["metadata"] as? [String: Any],
              let localAppMetadata = metadata["com.lingxi.localApp"] as? [String: Any],
              localAppMetadata["version"] as? Int == 1,
              localAppMetadata["filename"] as? String == filename else {
            return false
        }
        return true
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
        XCTFail("Timed out waiting for the expected browser state. \(safePageSummary(app))")
        return lastText
    }

    private func requireTargetFixtureIdentity(_ app: XCUIApplication) -> Bool {
        guard isTargetFixture(app) else {
            XCTFail("The browser page identity no longer matches the staged Local App id and revision; refusing the action. \(safePageSummary(app))")
            return false
        }
        return true
    }

    private func tapPageButton(_ title: String, in app: XCUIApplication, timeout: TimeInterval = 20) -> Bool {
        guard requireTargetFixtureIdentity(app) else { return false }
        let button = app.buttons.matching(NSPredicate(format: "label CONTAINS %@", title)).firstMatch
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if button.exists && button.isHittable && button.isEnabled {
                guard requireTargetFixtureIdentity(app) else { return false }
                button.tap()
                return true
            }
            if button.exists, button.frame.maxY < app.frame.minY + 100 {
                app.swipeDown()
            } else {
                app.swipeUp()
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        }
        XCTFail("The expected fixture control was not enabled and hittable. \(safePageSummary(app))")
        return false
    }

    private func enterChatText(_ text: String, in app: XCUIApplication) -> Bool {
        guard requireTargetFixtureIdentity(app) else { return false }
        _ = waitForPageText("Message to send in this Chat", in: app, timeout: 20, scroll: true)
        let field = app.textViews.matching(NSPredicate(format: "label CONTAINS %@", "Message to send in this Chat")).firstMatch
        guard field.waitForExistence(timeout: 10) else {
            XCTFail("Chat message field did not appear. \(safePageSummary(app))")
            return false
        }
        guard requireTargetFixtureIdentity(app) else { return false }
        field.tap()
        field.typeText(text)
        return true
    }

    private func regexMatches(_ pattern: String, in text: String) -> [String] {
        guard let regex = try? NSRegularExpression(pattern: pattern, options: [.dotMatchesLineSeparators]) else { return [] }
        let range = NSRange(text.startIndex..<text.endIndex, in: text)
        return regex.matches(in: text, range: range).compactMap { match in
            guard match.numberOfRanges > 1, let capture = Range(match.range(at: 1), in: text) else { return nil }
            return String(text[capture])
        }
    }

    private func eventRecords(_ app: XCUIApplication, runID: String? = nil) -> [[String: Any]]? {
        guard isTargetFixture(app) else { return nil }
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
                  records.contains(where: {
                      $0["type"] as? String == "RUN_STARTED"
                          && (runID == nil || $0["runId"] as? String == runID)
                  }) else { continue }
            return records
        }
        return nil
    }

    private func hasLifecycleOutcome(
        _ app: XCUIApplication,
        runID: String,
        threadID: String,
        outcome expectedOutcome: String
    ) -> Bool {
        guard let events = eventRecords(app, runID: runID) else { return false }
        let runEvents = events.filter { $0["runId"] as? String == runID }
        let starts = runEvents.filter { $0["type"] as? String == "RUN_STARTED" }
        let finishes = runEvents.filter { $0["type"] as? String == "RUN_FINISHED" }
        let errors = events.filter { $0["type"] as? String == "RUN_ERROR" }
        guard starts.count == 1, finishes.count == 1, errors.isEmpty,
              starts.first?["threadId"] as? String == threadID,
              finishes.first?["threadId"] as? String == threadID else {
            return false
        }
        return (finishes.first?["outcome"] as? [String: Any])?["type"] as? String == expectedOutcome
    }

    private func waitForSuccessfulRunResult(
        in app: XCUIApplication,
        input: BrowserRunInputEvidence,
        kind: String,
        timeout: TimeInterval = 240,
        requireReconnect: Bool = false
    ) -> BrowserRunResultEvidence? {
        let deadline = Date().addingTimeInterval(timeout)
        var lastText = ""
        var nextSwipe = Date().addingTimeInterval(2)
        while Date() < deadline {
            lastText = pageText(app)
            if let evidence = runResultEvidence(in: app, input: input, requireReconnect: requireReconnect),
               hasLifecycleOutcome(app, runID: input.runID, threadID: input.threadID, outcome: "success") {
                let directStatus = lastText.contains(
                    "\(kind) run succeeded · thread \(input.threadID) · run \(input.runID)"
                )
                let reconnectStatus = evidence.reconnect != nil
                    && lastText.contains("Read-only reconnect succeeded")
                    && lastText.contains("no new run started")
                if requireReconnect ? reconnectStatus : (directStatus || reconnectStatus) {
                    return evidence
                }
            }
            if Date() >= nextSwipe {
                app.swipeUp()
                nextSwipe = Date().addingTimeInterval(1.5)
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        }
        XCTFail("The current run did not expose its typed result, exact IDs, successful terminal lifecycle, and success status. \(safePageSummary(app, input: input))")
        return nil
    }

    private func waitForCancelledRun(
        in app: XCUIApplication,
        input: BrowserRunInputEvidence,
        timeout: TimeInterval = 120
    ) -> String {
        let deadline = Date().addingTimeInterval(timeout)
        var lastText = ""
        var nextSwipe = Date().addingTimeInterval(2)
        while Date() < deadline {
            lastText = pageText(app)
            if hasLifecycleOutcome(app, runID: input.runID, threadID: input.threadID, outcome: "cancelled") {
                return lastText
            }
            if Date() >= nextSwipe {
                app.swipeUp()
                nextSwipe = Date().addingTimeInterval(1.5)
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        }
        XCTFail("The current run did not finish with a standard cancelled outcome. \(safePageSummary(app, input: input))")
        return lastText
    }

    private func assertLifecycle(
        _ app: XCUIApplication,
        outcome expectedOutcome: String,
        expectedRunID: String,
        nativeRunPrefix: String? = nil
    ) -> String {
        guard let events = eventRecords(app, runID: expectedRunID) else {
            XCTFail("Could not decode the accessible standard BaseEvent JSON. \(safePageSummary(app))")
            return ""
        }
        let runEvents = events.filter { $0["runId"] as? String == expectedRunID }
        let starts = runEvents.filter { $0["type"] as? String == "RUN_STARTED" }
        let finishes = runEvents.filter { $0["type"] as? String == "RUN_FINISHED" }
        let errors = events.filter { $0["type"] as? String == "RUN_ERROR" }
        XCTAssertEqual(starts.count, 1, "Expected one standard RUN_STARTED event.")
        XCTAssertEqual(finishes.count, 1, "Expected one standard RUN_FINISHED event.")
        XCTAssertTrue(errors.isEmpty, "A successful or cancelled run must not also report RUN_ERROR.")
        guard let startedID = starts.first?["runId"] as? String,
              let finishedID = finishes.first?["runId"] as? String else {
            XCTFail("RUN_STARTED and RUN_FINISHED must each echo the public runId.")
            return ""
        }
        XCTAssertEqual(startedID, expectedRunID, "The lifecycle must belong to the exact captured public runId.")
        XCTAssertEqual(startedID, finishedID, "RUN_STARTED and RUN_FINISHED must echo the same public runId.")
        if let nativeRunPrefix {
            XCTAssertTrue(startedID.hasPrefix(nativeRunPrefix), "The public event runId must match the native handoff prefix.")
        }
        let actualOutcome = (finishes.first?["outcome"] as? [String: Any])?["type"] as? String
        XCTAssertEqual(actualOutcome, expectedOutcome, "Unexpected RUN_FINISHED outcome.")
        return startedID
    }

    private func startBrowserRun(
        _ browser: XCUIApplication,
        host: XCUIApplication,
        expectedRunID: String
    ) -> String? {
        // The page was verified before the browser action. Once the wake
        // opens LingXi, Safari's background accessibility tree may disappear.
        guard targetFixtureIdentityVerified else {
            XCTFail("The staged browser identity was not verified before handoff.")
            return nil
        }
        if browser.state == .runningForeground {
            guard requireTargetFixtureIdentity(browser) else { return nil }
        }
        guard waitForNativeStartButton(browser: browser, host: host) else { return nil }
        let start = host.buttons["local-app-agent-continue-default"]
        let waitingCopy = "浏览器 Agent 请求等待后台任务许可。运行："
        let waitingLabel = host.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", waitingCopy)).firstMatch
        let nativeText = waitingLabel.exists ? waitingLabel.label : ""
        guard nativeText.contains(expectedRunID) else {
            XCTFail("The trusted native handoff must identify the full captured public run ID.")
            return nil
        }
        let nativeRunPrefix = regexMatches(#"运行：([0-9a-fA-F]{8})"#, in: nativeText).first
        XCTAssertNotNil(nativeRunPrefix, "The trusted Apps page must identify the pending public run. \(safePageSummary(browser))")
        if let nativeRunPrefix {
            XCTAssertTrue(expectedRunID.lowercased().hasPrefix(nativeRunPrefix.lowercased()), "The native handoff prefix must match the existing pending public run ID.")
        }
        guard start.exists && start.isHittable else {
            XCTFail("The explicit native Start Agent action must be available. \(safePageSummary(browser))")
            return nil
        }
        attachScreen("native-agent-handoff-\(nativeRunPrefix ?? "run")")
        print("IOS_AGUI_NATIVE_HANDOFF_READY runPrefix=\(nativeRunPrefix ?? "unreadable") source=system-browser-deep-link")
        start.tap()
        guard browser.wait(for: .runningForeground, timeout: 60) else {
            XCTFail("iOS did not return to the observed system-selected browser after the native lease action.")
            return nil
        }
        guard waitForTargetFixtureIdentity(browser) else { return nil }
        return nativeRunPrefix
    }

    private func matchesLingxiPageOpenPrompt(_ text: String) -> Bool {
        let normalized = text.lowercased()
        let namesLingxi = normalized.contains("lingxi") || normalized.contains("灵犀")
        let englishPrompt = normalized.contains("open this page") && namesLingxi
        let chinesePrompt = normalized.contains("打开")
            && (normalized.contains("页面") || normalized.contains("网页") || normalized.contains("此页"))
            && namesLingxi
        return englishPrompt || chinesePrompt
    }

    private func promptText(in container: XCUIElement) -> String {
        ([container.label]
            + container.staticTexts.allElementsBoundByIndex.map(\.label)
            + container.textViews.allElementsBoundByIndex.map(\.label))
            .filter { !$0.isEmpty }
            .joined(separator: " ")
    }

    private func exactWakePromptButton(in container: XCUIElement, opening: Bool) -> XCUIElement? {
        let expected = opening ? ["open", "打开"] : ["cancel", "取消"]
        return container.buttons.allElementsBoundByIndex.first { expected.contains($0.label.lowercased()) }
    }

    private func tapLingxiPageOpenPrompt(
        in container: XCUIElement,
        prompt: String,
        source: String,
        promptSeen: inout Bool
    ) -> Bool {
        guard matchesLingxiPageOpenPrompt(prompt) else { return false }
        promptSeen = true
        guard container.exists,
              let open = exactWakePromptButton(in: container, opening: true),
              let cancel = exactWakePromptButton(in: container, opening: false),
              open.exists,
              open.isEnabled,
              open.isHittable,
              cancel.exists,
              cancel.isHittable else {
            return false
        }
        attachScreen("browser-lingxi-open-prompt-\(source)")
        print("IOS_AGUI_LINGXI_OPEN_PROMPT_MATCHED source=\(source) buttons=exact-open-cancel")
        open.tap()
        print("IOS_AGUI_LINGXI_WAKE_OPEN_CONFIRMED source=\(source)")
        return true
    }

    private func acceptLingxiPageOpenPromptIfPresent(
        browser: XCUIApplication,
        springBoard: XCUIApplication,
        promptSeen: inout Bool
    ) -> Bool {
        let containers: [(XCUIElement, String)] =
            browser.alerts.allElementsBoundByIndex.map { ($0, "browser-alert") }
            + browser.sheets.allElementsBoundByIndex.map { ($0, "browser-sheet") }
            + springBoard.alerts.allElementsBoundByIndex.map { ($0, "springboard-alert") }
        for (container, source) in containers {
            if tapLingxiPageOpenPrompt(
                in: container,
                prompt: promptText(in: container),
                source: source,
                promptSeen: &promptSeen
            ) {
                return true
            }
        }

        // Some iOS/browser combinations expose this system confirmation as a
        // generic browser web UI instead of an alert or sheet. Require the
        // explicit LingXi page-open text and exact scoped action buttons.
        guard let promptElement = browser.staticTexts.allElementsBoundByIndex.first(where: {
            $0.isHittable && matchesLingxiPageOpenPrompt($0.label)
        }) else {
            return false
        }
        return tapLingxiPageOpenPrompt(
            in: browser,
            prompt: promptElement.label,
            source: "browser-static-text",
            promptSeen: &promptSeen
        )
    }

    private func waitForNativeStartButton(
        browser: XCUIApplication,
        host: XCUIApplication,
        timeout: TimeInterval = 45
    ) -> Bool {
        let start = host.buttons["local-app-agent-continue-default"]
        let springBoard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let deadline = Date().addingTimeInterval(timeout)
        var matchingPromptSeen = false
        var openPromptTapped = false
        while Date() < deadline {
            if start.exists {
                print("IOS_AGUI_NATIVE_START_BUTTON_VISIBLE promptSeen=\(matchingPromptSeen) openPromptTapped=\(openPromptTapped)")
                return true
            }
            if !openPromptTapped {
                if browser.state == .runningForeground {
                    guard requireTargetFixtureIdentity(browser) else { return false }
                }
                openPromptTapped = acceptLingxiPageOpenPromptIfPresent(
                    browser: browser,
                    springBoard: springBoard,
                    promptSeen: &matchingPromptSeen
                )
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        }
        print("IOS_AGUI_NATIVE_HANDOFF_TIMEOUT promptSeen=\(matchingPromptSeen) openPromptTapped=\(openPromptTapped) hostStartVisible=false")
        print("IOS_AGUI_BROWSER_SAFE_SUMMARY \(safePageSummary(browser))")
        XCTFail("Timed out waiting for the trusted native Start Agent action. \(safePageSummary(browser))")
        return false
    }

    private func runPageButton(
        _ title: String,
        kind: String,
        expectedPrompt: String,
        expectedPartType: String,
        expectedFilename: String,
        expectedMimeType: String,
        expectedByteCount: Int,
        browser: XCUIApplication,
        host: XCUIApplication
    ) -> String {
        let previousRunID = runInputEvidence(in: browser)?.runID
        guard tapPageButton(title, in: browser) else { return "" }
        guard let input = waitForRunInput(in: browser, excludingRunID: previousRunID) else { return "" }
        return finishPageRun(
            kind: kind,
            expectedPrompt: expectedPrompt,
            expectedPartType: expectedPartType,
            expectedFilename: expectedFilename,
            expectedMimeType: expectedMimeType,
            expectedByteCount: expectedByteCount,
            input: input,
            browser: browser,
            host: host
        )
    }

    private func finishPageRun(
        kind: String,
        expectedPrompt: String,
        expectedPartType: String,
        expectedFilename: String,
        expectedMimeType: String,
        expectedByteCount: Int,
        input: BrowserRunInputEvidence,
        browser: XCUIApplication,
        host: XCUIApplication
    ) -> String {
        guard let runIDBeforeHandoff = startBrowserRun(browser, host: host, expectedRunID: input.runID) else {
            return ""
        }
        guard let evidence = waitForSuccessfulRunResult(in: browser, input: input, kind: kind) else { return "" }
        XCTAssertTrue(
            hasExactFileInput(
                input,
                prompt: expectedPrompt,
                partType: expectedPartType,
                filename: expectedFilename,
                mimeType: expectedMimeType,
                byteCount: expectedByteCount
            ),
            "RunAgentInput did not match the exact text, file type, filename, MIME type, and redacted byte metadata. \(safePageSummary(browser, input: input))"
        )
        let runID = assertLifecycle(
            browser,
            outcome: "success",
            expectedRunID: input.runID,
            nativeRunPrefix: runIDBeforeHandoff
        )
        if let reconnect = evidence.reconnect {
            XCTAssertEqual(reconnect["runId"] as? String, input.runID)
            XCTAssertEqual(reconnect["threadId"] as? String, input.threadID)
        }
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
        guard waitForTargetFixtureIdentity(browser) else { return }
        attachScreen("system-selected-browser-agent-page")
        print("IOS_AGUI_BROWSER_PAGE_READY appId=\(appID) browserBundleId=\(browserID) resolver=system-openURL")

        let retryPending = browser.buttons["Retry the same pending request"]
        let isSubmittingImage = pageText(browser).contains("Submitting standard AG-UI input for image")
        if isSubmittingImage {
            guard let payload = runInputPayload(in: browser),
                  let pendingImage = publicImageRunIDs(in: payload),
                  let pendingInput = runInputEvidence(from: payload) else {
                XCTFail("The active image submission did not match the exact public fixture; no replacement was started. \(safePageSummary(browser))")
                return
            }
            XCTAssertEqual(pendingInput.runID, pendingImage.runID)
            XCTAssertEqual(pendingInput.threadID, pendingImage.threadID)
            print("IOS_AGUI_PENDING_IMAGE_CONTINUE_READY runId=\(pendingImage.runID) threadId=\(pendingImage.threadID) file=public-apple.jpg")
            _ = finishPageRun(
                kind: "image",
                expectedPrompt: publicImagePrompt,
                expectedPartType: "image",
                expectedFilename: "public-apple.jpg",
                expectedMimeType: "image/jpeg",
                expectedByteCount: 576216,
                input: pendingInput,
                browser: browser,
                host: host
            )
        } else if retryPending.exists && retryPending.isEnabled {
            guard let payload = runInputPayload(in: browser),
                  let pendingImage = publicImageRunIDs(in: payload),
                  let pendingInput = runInputEvidence(from: payload) else {
                XCTFail("The visible retry was not the exact public fixture input; no replacement was submitted. \(safePageSummary(browser))")
                return
            }
            XCTAssertEqual(pendingInput.runID, pendingImage.runID)
            XCTAssertEqual(pendingInput.threadID, pendingImage.threadID)
            attachScreen("browser-pending-public-image-retry-\(pendingImage.runID.prefix(8))")
            print("IOS_AGUI_PENDING_IMAGE_RETRY_READY runId=\(pendingImage.runID) threadId=\(pendingImage.threadID) file=public-apple.jpg")
            guard tapPageButton("Retry the same pending request", in: browser) else { return }
            _ = finishPageRun(
                kind: "image",
                expectedPrompt: publicImagePrompt,
                expectedPartType: "image",
                expectedFilename: "public-apple.jpg",
                expectedMimeType: "image/jpeg",
                expectedByteCount: 576216,
                input: pendingInput,
                browser: browser,
                host: host
            )
        } else {
            let runImage = browser.buttons["Run public image"]
            guard runImage.exists && runImage.isEnabled else {
                XCTFail("No validated pending retry or enabled new-image action is available; preserving existing input. \(safePageSummary(browser))")
                return
            }
            _ = runPageButton(
                "Run public image",
                kind: "image",
                expectedPrompt: publicImagePrompt,
                expectedPartType: "image",
                expectedFilename: "public-apple.jpg",
                expectedMimeType: "image/jpeg",
                expectedByteCount: 576216,
                browser: browser,
                host: host
            )
        }
        _ = runPageButton(
            "Run text file",
            kind: "document",
            expectedPrompt: documentPrompt,
            expectedPartType: "document",
            expectedFilename: "acceptance-stationery.txt",
            expectedMimeType: "text/plain;charset=utf-8",
            expectedByteCount: 26,
            browser: browser,
            host: host
        )

        guard tapPageButton("Create Chat", in: browser) else { return }
        let createdChat = waitForPageText("Created host-owned persistent Chat", in: browser, timeout: 30, scroll: true)
        XCTAssertTrue(createdChat.contains("threadId"), "Chat creation must expose its host-owned threadId.")
        let markerSuffix = String(UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(12))
        let contextMarker = "IOS_CHAT_CONTEXT_\(markerSuffix)"
        let firstChatPrompt = "Remember this marker for my next message: \(contextMarker). For this turn, reply with exactly context-seed."
        let previousChatRunID = runInputEvidence(in: browser)?.runID
        guard enterChatText(firstChatPrompt, in: browser),
              tapPageButton("Send Chat turn", in: browser) else { return }
        let firstInput = try XCTUnwrap(
            waitForRunInput(in: browser, excludingRunID: previousChatRunID, expectedUserText: firstChatPrompt),
            "The first Chat turn did not expose its new RunAgentInput before native handoff."
        )
        guard let firstChatRun = startBrowserRun(browser, host: host, expectedRunID: firstInput.runID) else { return }
        let firstEvidence = try XCTUnwrap(
            waitForSuccessfulRunResult(in: browser, input: firstInput, kind: "chat"),
            "The first Chat run did not complete with its own typed result and terminal lifecycle."
        )
        let firstChatRunID = assertLifecycle(
            browser,
            outcome: "success",
            expectedRunID: firstInput.runID,
            nativeRunPrefix: firstChatRun
        )
        XCTAssertEqual(firstInput.runID, firstChatRunID, "The first RunAgentInput must match the public run reported by lifecycle events.")
        let firstMessages = try XCTUnwrap(inputMessages(in: firstInput.input), "The first RunAgentInput must expose messages.")
        XCTAssertEqual(firstMessages.count, 1, "The first Chat RunAgentInput must contain exactly its new user turn.")
        let firstMessage = try XCTUnwrap(firstMessages.first, "The first RunAgentInput has no user message.")
        XCTAssertEqual(firstMessage["role"] as? String, "user")
        let firstInputText = try XCTUnwrap(textContent(in: firstMessage), "The first user message must include text content.")
        XCTAssertTrue(firstInputText.contains(contextMarker), "The first user prompt must contain the unique synthetic context marker.")
        let firstAnswer = try XCTUnwrap(answer(in: firstEvidence.runAgentResult), "RunAgentResult.result.answer was missing from the first Chat turn.")
        XCTAssertTrue(firstAnswer == "context-seed", "The first Chat turn should answer exactly context-seed. \(safePageSummary(browser, input: firstInput))")
        attachScreen("browser-chat-first-run-\(firstChatRunID.prefix(8))")

        guard tapPageButton("Read-only reconnect to last run", in: browser) else { return }
        _ = waitForPageText(
            "Read-only reconnect to saved run \(firstInput.runID)",
            in: browser,
            timeout: 20,
            scroll: true
        )
        let reconnectEvidence = try XCTUnwrap(
            waitForSuccessfulRunResult(in: browser, input: firstInput, kind: "chat", timeout: 60, requireReconnect: true),
            "Read-only reconnect did not return the current run's typed result and exact saved IDs."
        )
        let reconnect = pageText(browser)
        XCTAssertTrue(reconnect.contains("no new run started"), "connectAgent must replay the existing run without submitting another run.")
        XCTAssertTrue(reconnect.contains(firstChatRunID), "Read-only replay must refer to the same public runId.")
        XCTAssertEqual(reconnectEvidence.reconnect?["runId"] as? String, firstInput.runID)
        XCTAssertEqual(reconnectEvidence.reconnect?["threadId"] as? String, firstInput.threadID)
        XCTAssertTrue(answer(in: reconnectEvidence.runAgentResult) == "context-seed", "Read-only reconnect must restore the same typed result. \(safePageSummary(browser, input: firstInput))")
        guard tapPageButton("Refresh Chat history", in: browser) else { return }
        let history = waitForPageText(contextMarker, in: browser, timeout: 30, scroll: true)
        XCTAssertTrue(history.contains("context-seed"), "Canonical Chat history should preserve the first assistant turn separately from the next RunAgentInput.")
        XCTAssertTrue(history.contains("assistant"), "Canonical Chat history should include the assistant reply.")

        let followupPrompt = "What exact marker did I ask you to remember in the previous message? Reply with only the marker."
        XCTAssertFalse(followupPrompt.contains(contextMarker), "The second user prompt must not reveal the marker.")
        let previousFollowupRunID = runInputEvidence(in: browser)?.runID
        guard enterChatText(followupPrompt, in: browser),
              tapPageButton("Send Chat turn", in: browser) else { return }
        let followupInput = try XCTUnwrap(
            waitForRunInput(in: browser, excludingRunID: previousFollowupRunID, expectedUserText: followupPrompt),
            "The follow-up Chat turn did not expose its new RunAgentInput before native handoff."
        )
        guard let followupRun = startBrowserRun(browser, host: host, expectedRunID: followupInput.runID) else { return }
        let followupEvidence = try XCTUnwrap(
            waitForSuccessfulRunResult(in: browser, input: followupInput, kind: "chat"),
            "The follow-up Chat run did not complete with its own typed result and terminal lifecycle."
        )
        let followupRunID = assertLifecycle(
            browser,
            outcome: "success",
            expectedRunID: followupInput.runID,
            nativeRunPrefix: followupRun
        )
        XCTAssertEqual(followupInput.runID, followupRunID, "The follow-up RunAgentInput must match the public run reported by lifecycle events.")
        let followupMessages = try XCTUnwrap(inputMessages(in: followupInput.input), "The second RunAgentInput must expose messages.")
        XCTAssertEqual(followupMessages.count, 1, "The second RunAgentInput must send only the new user turn, not replay prior Chat history.")
        let followupMessage = try XCTUnwrap(followupMessages.first, "The second RunAgentInput has no user message.")
        XCTAssertEqual(followupMessage["role"] as? String, "user")
        let followupInputText = try XCTUnwrap(textContent(in: followupMessage), "The second user message must include text content.")
        XCTAssertTrue(followupInputText == followupPrompt, "The second RunAgentInput should contain only the new prompt. \(safePageSummary(browser, input: followupInput))")
        XCTAssertFalse(followupInputText.contains(contextMarker), "The second RunAgentInput must not include the marker as prompt text or replayed history.")
        let followupAnswer = try XCTUnwrap(answer(in: followupEvidence.runAgentResult), "RunAgentResult.result.answer was missing from the follow-up Chat turn.")
        XCTAssertTrue(followupAnswer == contextMarker, "The second Chat turn must recall the marker from the host-persisted prior turn. \(safePageSummary(browser, input: followupInput))")
        attachScreen("browser-chat-followup-\(followupRunID.prefix(8))")

        let cancellationPrompt = "Write a detailed 1500 word explanation of how a paper notebook is made. Continue until complete."
        let previousCancellationRunID = runInputEvidence(in: browser)?.runID
        guard enterChatText(cancellationPrompt, in: browser),
              tapPageButton("Send Chat turn", in: browser) else { return }
        let cancelledInput = try XCTUnwrap(
            waitForRunInput(in: browser, excludingRunID: previousCancellationRunID, expectedUserText: cancellationPrompt),
            "The cancellation run did not expose its new RunAgentInput before native handoff."
        )
        guard let cancelledRunPrefix = startBrowserRun(browser, host: host, expectedRunID: cancelledInput.runID) else { return }
        let cancel = browser.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Cancel active run")).firstMatch
        let cancelDeadline = Date().addingTimeInterval(45)
        while Date() < cancelDeadline && (!cancel.exists || !cancel.isEnabled || !cancel.isHittable) {
            if cancel.exists, cancel.frame.maxY < browser.frame.minY + 100 {
                browser.swipeDown()
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.25))
        }
        XCTAssertTrue(cancel.exists && cancel.isEnabled, "The running task must expose its cancel action before completion. \(safePageSummary(browser, input: cancelledInput))")
        guard requireTargetFixtureIdentity(browser) else { return }
        cancel.tap()
        attachScreen("browser-chat-cancel-requested")
        let cancelled = waitForCancelledRun(in: browser, input: cancelledInput)
        let cancelledRunID = assertLifecycle(
            browser,
            outcome: "cancelled",
            expectedRunID: cancelledInput.runID,
            nativeRunPrefix: cancelledRunPrefix
        )
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

        let start = host.buttons["local-app-agent-continue-default"]
        XCTAssertTrue(start.waitForExistence(timeout: 20), "The app should require a foreground action before it returns to the external browser.")
        XCTAssertTrue(host.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", String(runID.prefix(8)))).firstMatch.exists)
        print("IOS_BROWSER_AGENT_HANDOFF_UI PASS ag-ui-run-id explicit-native-action system-default-browser-neutral")
    }
}
