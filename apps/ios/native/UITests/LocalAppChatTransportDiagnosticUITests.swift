import CoreFoundation
import Foundation
import XCTest

// BEGIN DIAGNOSTIC VALUE TYPES
// Independent of UI automation so binding and redaction can be checked locally.
private enum ChatDiagnosticValueError: Error { case invalid }

private func diagnosticInteger(_ value: Any?, within range: ClosedRange<Int>) -> Int? {
    guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else { return nil }
    let raw = number.doubleValue
    guard raw.isFinite, raw.rounded() == raw, raw >= Double(range.lowerBound),
          raw <= Double(range.upperBound), raw <= 9_007_199_254_740_991 else { return nil }
    return Int(raw)
}

private func diagnosticUUID(_ value: String) -> Bool {
    value.range(of: #"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"#,
                options: .regularExpression) != nil
}

private func diagnosticTransport(_ value: [String: Any]) -> [String: Any] {
    func allowed(_ key: String, _ values: [String]) -> String {
        guard let text = value[key] as? String, values.contains(text) else { return "unavailable" }
        return text
    }
    return [
        "phase": allowed("phase", ["request", "http_response", "response_body", "host_response"]),
        "reason": allowed("reason", ["timeout", "fetch_failed", "http_error", "invalid_json", "body_failed", "host_error", "wait_cancelled"]),
        "httpStatus": diagnosticInteger(value["httpStatus"], within: 100...599).map { $0 as Any } ?? "unavailable",
        "elapsedMs": diagnosticInteger(value["elapsedMs"], within: 0...120_000).map { $0 as Any } ?? "unavailable",
        "visibility": allowed("visibility", ["visible", "hidden", "unknown"]),
    ]
}

private enum ChatDiagnosticStage: String, CaseIterable { case create, creationStatus, history }
private enum ChatDiagnosticCallState: String { case pending, returned, failed }
private enum ChatDiagnosticCreationState: String { case pending, confirmed, rejected }

private struct ChatDiagnosticCall {
    let attribution: String
    let source: String
    let sequence: Int?
    let requestRef: Int?
    let state: ChatDiagnosticCallState
    let transport: [String: Any]?

    init(_ value: [String: Any]) throws {
        let base = Set(["attribution", "storage", "source", "sequence", "requestRef", "state"])
        guard let attribution = value["attribution"] as? String,
              let source = value["source"] as? String,
              let storage = value["storage"] as? String,
              let stateText = value["state"] as? String,
              let state = ChatDiagnosticCallState(rawValue: stateText),
              Set(value.keys) == base.union(state == .failed ? ["transport"] : []) else {
            throw ChatDiagnosticValueError.invalid
        }
        if attribution == "current-page" {
            guard ["bootstrap", "create-button", "status-button", "retry-button", "open-created-button",
                   "recover-id-button", "open-chat-button", "refresh-history-button", "run-completion",
                   "automatic-recovery", "reconnect-button"].contains(source),
                  let sequence = diagnosticInteger(value["sequence"], within: 1...9_007_199_254_740_991),
                  let requestRef = diagnosticInteger(value["requestRef"], within: 1...1000),
                  state == .failed ? ["saved", "unavailable"].contains(storage) : storage == "not-saved" else {
                throw ChatDiagnosticValueError.invalid
            }
            self.sequence = sequence
            self.requestRef = requestRef
        } else {
            guard attribution == "saved-unbound", source == "unavailable", state == .failed,
                  ["saved", "unavailable"].contains(storage),
                  value["sequence"] is NSNull, value["requestRef"] is NSNull else {
                throw ChatDiagnosticValueError.invalid
            }
            sequence = nil
            requestRef = nil
        }
        if state == .failed {
            guard let transport = value["transport"] as? [String: Any] else { throw ChatDiagnosticValueError.invalid }
            self.transport = diagnosticTransport(transport)
        } else {
            transport = nil
        }
        self.attribution = attribution
        self.source = source
        self.state = state
    }
}

private struct ChatDiagnosticEnvelope {
    let pageID: String
    let creationRef: Int?
    let chatRef: Int?
    let calls: [ChatDiagnosticStage: ChatDiagnosticCall]

    init(_ value: [String: Any]) throws {
        guard Set(value.keys) == Set(["version", "pageId", "selectedCreationRef", "selectedChatRef", "create", "creationStatus", "history"]),
              diagnosticInteger(value["version"], within: 2...2) == 2,
              let pageID = value["pageId"] as? String, diagnosticUUID(pageID) else {
            throw ChatDiagnosticValueError.invalid
        }
        func reference(_ raw: Any?) throws -> Int? {
            if raw is NSNull { return nil }
            guard let result = diagnosticInteger(raw, within: 1...1000) else { throw ChatDiagnosticValueError.invalid }
            return result
        }
        self.pageID = pageID
        creationRef = try reference(value["selectedCreationRef"])
        chatRef = try reference(value["selectedChatRef"])
        var calls: [ChatDiagnosticStage: ChatDiagnosticCall] = [:]
        for stage in ChatDiagnosticStage.allCases {
            if value[stage.rawValue] is NSNull { continue }
            guard let record = value[stage.rawValue] as? [String: Any] else { throw ChatDiagnosticValueError.invalid }
            calls[stage] = try ChatDiagnosticCall(record)
        }
        let sequences = calls.values.compactMap(\.sequence)
        guard Set(sequences).count == sequences.count else { throw ChatDiagnosticValueError.invalid }
        self.calls = calls
    }

    var maximumSequence: Int { calls.values.compactMap(\.sequence).max() ?? 0 }

    func boundCall(_ stage: ChatDiagnosticStage, reference: Int) -> ChatDiagnosticCall? {
        guard (stage == .history ? chatRef : creationRef) == reference,
              let call = calls[stage], call.attribution == "current-page", call.requestRef == reference else { return nil }
        return call
    }
}

private struct ChatDiagnosticReceipt {
    let requestID: String
    let state: ChatDiagnosticCreationState

    init(_ label: String) throws {
        let prefix = "Chat creation "
        guard label.hasPrefix(prefix) else { throw ChatDiagnosticValueError.invalid }
        let rest = String(label.dropFirst(prefix.count))
        guard rest.count >= 37 else { throw ChatDiagnosticValueError.invalid }
        let requestID = String(rest.prefix(36))
        guard diagnosticUUID(requestID), rest.dropFirst(36).first == " " else { throw ChatDiagnosticValueError.invalid }
        let suffix = String(rest.dropFirst(37))
        if suffix == "is confirmed." { state = .confirmed }
        else if suffix == "is pending; its outcome is unknown. No automatic retry was sent." { state = .pending }
        else if ["chat_create_conflict", "chat_create_deleted", "sdk_rejected"].contains(where: {
            suffix == "was rejected (\($0)). Start another only by explicit action."
        }) { state = .rejected }
        else { throw ChatDiagnosticValueError.invalid }
        self.requestID = requestID
    }
}
// END DIAGNOSTIC VALUE TYPES

/// Default observe mode only reads the already foreground, explicitly bound fixture.
/// No mode launches, navigates, sends messages, invokes tools/models, retries, or reads history content.
/// The fixture itself reads empty-Chat history after successful creation; only diagnostic metadata is collected.
///
/// Always supply LINGXI_AGENT_BROWSER_APP_ID, LINGXI_AGENT_BROWSER_REVISION, and
/// LINGXI_SYSTEM_BROWSER_BUNDLE_ID observed for the system-selected browser.
/// observe / query-once also require LINGXI_EXISTING_CHAT_REQUEST_ID.
/// Creating requires mode=create-once AND LINGXI_ALLOW_SINGLE_CHAT_CREATE=<AppID>:<revision>.
/// Optional LINGXI_SINGLE_CREATE_STATUS_MODE=query-once allows one status read after failed, pending creation.
final class LocalAppChatTransportDiagnosticUITests: XCTestCase {
    private enum Mode: String { case observe, queryOnce = "query-once", createOnce = "create-once" }
    private struct Binding {
        let appID: String
        let revision: Int
        let browserID: String
        let mode: Mode
        let requestID: String?
        let statusAfterCreate: Bool

        static func read(creating: Bool) throws -> Binding {
            let env = ProcessInfo.processInfo.environment
            guard env["SIMULATOR_DEVICE_NAME"] == nil else { throw XCTSkip("physical_device_required") }
            guard let mode = Mode(rawValue: env["LINGXI_CHAT_DIAGNOSTIC_MODE"] ?? "observe"),
                  creating == (mode == .createOnce) else { throw XCTSkip("diagnostic_mode_not_selected") }
            guard let appID = env["LINGXI_AGENT_BROWSER_APP_ID"],
                  appID.range(of: #"^a[0-9a-f]{32}$"#, options: .regularExpression) != nil,
                  let revision = env["LINGXI_AGENT_BROWSER_REVISION"].flatMap(Int.init), revision > 0,
                  let browserID = env["LINGXI_SYSTEM_BROWSER_BUNDLE_ID"],
                  browserID.range(of: #"^[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+$"#, options: .regularExpression) != nil else {
                throw XCTSkip("explicit_app_revision_browser_binding_required")
            }
            let requestID = env["LINGXI_EXISTING_CHAT_REQUEST_ID"]
            if creating {
                guard browserID == "com.apple.mobilesafari", requestID == nil,
                      env["LINGXI_ALLOW_SINGLE_CHAT_CREATE"] == "\(appID):\(revision)" else {
                    throw XCTSkip("explicit_isolated_safari_creation_authorization_required")
                }
            } else {
                guard let requestID, diagnosticUUID(requestID) else { throw XCTSkip("existing_request_binding_required") }
            }
            let statusMode = env["LINGXI_SINGLE_CREATE_STATUS_MODE"] ?? "observe"
            guard ["observe", "query-once"].contains(statusMode) else { throw XCTSkip("unsupported_status_mode") }
            return Binding(appID: appID, revision: revision, browserID: browserID, mode: mode,
                           requestID: requestID, statusAfterCreate: creating && statusMode == "query-once")
        }
    }

    private struct Snapshot {
        let metadata: [String: Any]
        let diagnostic: ChatDiagnosticEnvelope
        let receipt: ChatDiagnosticReceipt?
    }

    @MainActor
    func testReadExistingChatTransportDiagnostics() throws {
        continueAfterFailure = false
        executionTimeAllowance = 35
        let binding = try Binding.read(creating: false)
        let browser = try preparedBrowser(binding)
        let initial = try snapshot(browser, binding: binding, requestID: binding.requestID)
        guard let reference = initial.diagnostic.creationRef, initial.receipt != nil else {
            throw XCTSkip("selected_existing_creation_not_bound")
        }
        let observed = emitBoundCalls(initial, reference: reference, context: "existing-request-observation", creates: 0, statusTaps: 0)
        if binding.mode == .observe {
            guard observed else { throw XCTSkip("no_current_page_bound_diagnostic_saved_records_not_attributed") }
            return
        }
        guard let requestID = binding.requestID else { throw XCTSkip("existing_request_binding_required") }
        var statusTapped = false
        try queryStatusOnce(browser, binding: binding, initial: initial, requestID: requestID,
                            reference: reference, creates: 0, statusTapped: &statusTapped)
    }

    @MainActor
    func testCreateSingleIsolatedChatTransportDiagnostics() throws {
        continueAfterFailure = false
        executionTimeAllowance = 160
        let binding = try Binding.read(creating: true)
        let browser = try preparedBrowser(binding)
        let initial = try snapshot(browser, binding: binding)
        try requireCleanFixture(initial, browser: browser)
        guard browser.staticTexts["No saved creation IDs listed."].exists else {
            throw XCTSkip("fresh_local_receipt_listing_not_available")
        }
        let list = browser.buttons["List saved creation IDs"]
        guard usable(list) else { throw XCTSkip("local_receipt_list_control_unavailable") }
        list.tap() // Reads the SDK's local receipt database, not the host or a model.
        let listingDeadline = ProcessInfo.processInfo.systemUptime + 8
        while !browser.staticTexts["No saved Chat creation IDs were found."].exists || !list.exists || !list.isEnabled {
            let current = try snapshot(browser, binding: binding, pageID: initial.diagnostic.pageID)
            try requireCleanFixture(current, browser: browser)
            guard ProcessInfo.processInfo.systemUptime < listingDeadline else {
                throw XCTSkip("empty_local_receipt_list_not_proven_no_creation_performed")
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        }
        let before = try snapshot(browser, binding: binding, pageID: initial.diagnostic.pageID)
        try requireCleanFixture(before, browser: browser)
        let create = browser.buttons["Create Chat"]
        guard usable(create), browser.staticTexts["No saved Chat creation IDs were found."].exists else {
            throw XCTSkip("clean_creation_control_not_verified")
        }
        emit(["stage": "create-click", "scope": "new-isolated-request", "pageId": before.diagnostic.pageID,
              "explicitCreateTaps": 1, "explicitStatusTaps": 0])
        create.tap() // The only creation action in this test; no retry control is used.
        let started = ProcessInfo.processInfo.systemUptime
        var requestID: String?
        var statusTapped = false
        do {
            repeat {
                let current = try snapshot(browser, binding: binding, requestID: requestID,
                                           pageID: before.diagnostic.pageID, allowUnrenderedReceipt: requestID == nil)
                if let receipt = current.receipt {
                    requestID = receipt.requestID
                    guard let reference = current.diagnostic.creationRef,
                          let call = current.diagnostic.boundCall(.create, reference: reference),
                          call.source == "create-button", call.sequence == 1 else {
                        throw XCTSkip("single_create_source_sequence_or_binding_not_proven")
                    }
                    try requireCreationCompanions(current, reference: reference)
                    if call.state != .pending, list.isEnabled {
                        _ = emitBoundCalls(current, reference: reference, context: "new-isolated-request", creates: 1, statusTaps: 0)
                        emitOutcome(current, browser: browser, context: "new-isolated-request", creates: 1, statusTaps: 0)
                        if call.state == .failed, receipt.state == .pending, binding.statusAfterCreate {
                            try queryStatusOnce(browser, binding: binding, initial: current, requestID: receipt.requestID,
                                                reference: reference, creates: 1, statusTapped: &statusTapped)
                        }
                        return
                    }
                }
                RunLoop.current.run(until: Date().addingTimeInterval(0.5))
            } while ProcessInfo.processInfo.systemUptime - started < 130
        } catch {
            emit(["stage": "observation-stopped", "scope": "new-isolated-request", "outcome": "unobserved",
                  "explicitCreateTaps": 1, "explicitStatusTaps": statusTapped ? 1 : 0])
            throw error
        }
        emit(["stage": "observation-deadline", "scope": "new-isolated-request", "outcome": "unobserved",
              "explicitCreateTaps": 1, "explicitStatusTaps": 0])
        throw XCTSkip("single_create_outcome_not_observed_no_retry_performed")
    }

    @MainActor
    private func queryStatusOnce(_ browser: XCUIApplication, binding: Binding, initial: Snapshot,
                                 requestID: String, reference: Int, creates: Int, statusTapped: inout Bool) throws {
        guard initial.receipt?.state == .pending else { throw XCTSkip("creation_not_pending_no_status_query_performed") }
        let before = try snapshot(browser, binding: binding, requestID: requestID, pageID: initial.diagnostic.pageID)
        let status = browser.buttons["Check saved Chat creation"]
        guard before.receipt?.state == .pending, before.diagnostic.creationRef == reference,
              before.diagnostic.maximumSequence == initial.diagnostic.maximumSequence,
              before.diagnostic.maximumSequence < 9_007_199_254_740_991,
              !before.diagnostic.calls.values.contains(where: { $0.attribution == "current-page" && $0.state == .pending }),
              usable(status) else { throw XCTSkip("stable_pending_status_binding_not_proven") }
        let expectedSequence = before.diagnostic.maximumSequence + 1
        emit(["stage": "status-click", "scope": creates == 0 ? "existing-request-query" : "new-isolated-request",
              "pageId": before.diagnostic.pageID, "expectedSequence": expectedSequence,
              "requestRef": reference, "explicitCreateTaps": creates, "explicitStatusTaps": 1])
        statusTapped = true
        status.tap() // The only explicit host-status action; it never creates or opens a Chat.
        let deadline = ProcessInfo.processInfo.systemUptime + 15
        repeat {
            let current = try snapshot(browser, binding: binding, requestID: requestID, pageID: before.diagnostic.pageID)
            guard current.diagnostic.creationRef == reference,
                  current.diagnostic.maximumSequence <= expectedSequence else {
                throw XCTSkip("intervening_call_or_changed_request_not_attributed")
            }
            if let call = current.diagnostic.boundCall(.creationStatus, reference: reference), call.sequence == expectedSequence {
                guard call.source == "status-button" else { throw XCTSkip("automatic_status_not_attributed_to_explicit_click") }
                if call.state != .pending, browser.buttons["List saved creation IDs"].isEnabled {
                    emitCall(call, stage: .creationStatus, pageID: current.diagnostic.pageID,
                             context: "explicit-status-once", creates: creates, statusTaps: 1)
                    emitOutcome(current, browser: browser, context: "explicit-status-once", creates: creates, statusTaps: 1)
                    return
                }
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        } while ProcessInfo.processInfo.systemUptime < deadline
        emit(["stage": "status-observation-deadline", "outcome": "unobserved",
              "explicitCreateTaps": creates, "explicitStatusTaps": 1])
        throw XCTSkip("single_status_action_not_observed_no_retry_performed")
    }

    @MainActor
    private func preparedBrowser(_ binding: Binding) throws -> XCUIApplication {
        let browser = XCUIApplication(bundleIdentifier: binding.browserID)
        guard browser.state == .runningForeground, browser.staticTexts["Agent device acceptance"].exists else {
            throw XCTSkip("bound_fixture_browser_not_foreground_no_navigation_performed")
        }
        if jsonRegion(browser, identifier: "diagnostics", label: "Safe fixture diagnostics") == nil {
            let disclosure = browser.descendants(matching: .any).matching(NSPredicate(format: "label == %@", "Safe diagnostics")).firstMatch
            guard disclosure.exists, disclosure.isHittable else { throw XCTSkip("safe_identity_region_unavailable") }
            disclosure.tap() // Presentation only; the fixture has no disclosure request handler.
        }
        return browser
    }

    @MainActor
    private func snapshot(_ browser: XCUIApplication, binding: Binding, requestID: String? = nil,
                          pageID: String? = nil, allowUnrenderedReceipt: Bool = false) throws -> Snapshot {
        guard browser.state == .runningForeground,
              let metadata = jsonRegion(browser, identifier: "diagnostics", label: "Safe fixture diagnostics"),
              let info = metadata["localAppInfo"] as? [String: Any], info["id"] as? String == binding.appID,
              diagnosticInteger(info["revision"], within: 1...9_007_199_254_740_991) == binding.revision,
              let raw = jsonRegion(browser, identifier: "chat-lookup-diagnostics", label: "Chat lookup failure diagnostics"),
              let diagnostic = try? ChatDiagnosticEnvelope(raw), pageID == nil || diagnostic.pageID == pageID else {
            throw XCTSkip("fixture_identity_or_v2_page_scope_not_verified")
        }
        let receipts = browser.staticTexts.matching(NSPredicate(format: "label MATCHES %@", #"^Chat creation [0-9a-f-]{36} .*$"#))
        guard receipts.count <= 1 else { throw XCTSkip("ambiguous_creation_receipt") }
        let receipt = receipts.count == 1 ? try? ChatDiagnosticReceipt(receipts.firstMatch.label) : nil
        guard receipts.count == 0 || receipt != nil,
              requestID == nil || receipt?.requestID == requestID,
              receipt == nil ? (diagnostic.creationRef == nil || allowUnrenderedReceipt) : diagnostic.creationRef != nil else {
            throw XCTSkip("exact_creation_receipt_and_selected_reference_not_verified")
        }
        return Snapshot(metadata: metadata, diagnostic: diagnostic, receipt: receipt)
    }

    @MainActor
    private func requireCleanFixture(_ snapshot: Snapshot, browser: XCUIApplication) throws {
        func isFalse(_ key: String) -> Bool {
            guard let value = snapshot.metadata[key] as? NSNumber,
                  CFGetTypeID(value) == CFBooleanGetTypeID() else { return false }
            return !value.boolValue
        }
        guard snapshot.receipt == nil, snapshot.diagnostic.creationRef == nil, snapshot.diagnostic.chatRef == nil,
              snapshot.diagnostic.calls.isEmpty, isFalse("readyMarkerPresent"), isFalse("localPending"),
              isFalse("fixtureStorageRecordExists"), snapshot.metadata["pendingRunId"] is NSNull,
              snapshot.metadata["lastRunId"] is NSNull, snapshot.metadata["sdkHandoffErrorCode"] is NSNull,
              let handoff = snapshot.metadata["sdkHandoff"] as? [String: Any], handoff["type"] as? String == "none",
              handoff["runId"] is NSNull, handoff["code"] is NSNull,
              browser.staticTexts["No Chat creation is awaiting recovery."].exists,
              browser.staticTexts["No Chat opened."].exists,
              browser.staticTexts["No saved task or Chat was found. Page startup made no model call."].exists else {
            throw XCTSkip("isolated_clean_fixture_not_proven_no_creation_performed")
        }
    }

    private func requireCreationCompanions(_ snapshot: Snapshot, reference: Int) throws {
        guard snapshot.diagnostic.chatRef == nil || snapshot.diagnostic.chatRef == reference else {
            throw XCTSkip("different_chat_selection_no_further_action")
        }
        for stage in [ChatDiagnosticStage.creationStatus, .history] {
            guard let call = snapshot.diagnostic.calls[stage] else { continue }
            guard let bound = snapshot.diagnostic.boundCall(stage, reference: reference), bound.sequence == call.sequence else {
                throw XCTSkip("unbound_companion_call_not_attributed")
            }
            guard (stage == .creationStatus && call.source == "bootstrap") ||
                    (stage == .history && call.source == "create-button" && snapshot.diagnostic.chatRef == reference) else {
                throw XCTSkip("unexpected_companion_source_no_further_action")
            }
        }
    }

    /// Only the two named metadata regions are read. No page text, screenshots,
    /// message/history content, results, address bars or accessibility-tree dumps.
    @MainActor
    private func jsonRegion(_ browser: XCUIApplication, identifier: String, label: String) -> [String: Any]? {
        let regions = browser.descendants(matching: .any).matching(NSPredicate(format: "identifier == %@ OR label == %@", identifier, label))
        guard regions.count <= 8 else { return nil }
        for region in regions.allElementsBoundByIndex {
            var values: [String] = []
            if let value = region.value as? String { values.append(value) }
            let leaves = region.descendants(matching: .staticText).allElementsBoundByIndex
            guard leaves.count <= 128 else { continue }
            values.append(contentsOf: leaves.map(\.label))
            guard values.reduce(0, { $0 + $1.utf8.count }) <= 8192 else { continue }
            for candidate in values + [values.joined(), values.joined(separator: "\n")] {
                guard let data = candidate.data(using: .utf8),
                      let decoded = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
                return decoded
            }
        }
        return nil
    }

    @MainActor
    private func usable(_ element: XCUIElement) -> Bool { element.exists && element.isEnabled && element.isHittable }

    @discardableResult
    private func emitBoundCalls(_ snapshot: Snapshot, reference: Int, context: String, creates: Int, statusTaps: Int) -> Bool {
        var emitted = false
        for stage in ChatDiagnosticStage.allCases {
            guard let call = snapshot.diagnostic.boundCall(stage, reference: reference) else { continue }
            emitCall(call, stage: stage, pageID: snapshot.diagnostic.pageID, context: context, creates: creates, statusTaps: statusTaps)
            emitted = true
        }
        return emitted
    }

    private func emitCall(_ call: ChatDiagnosticCall, stage: ChatDiagnosticStage, pageID: String,
                          context: String, creates: Int, statusTaps: Int) {
        guard call.attribution == "current-page", let sequence = call.sequence, let requestRef = call.requestRef else { return }
        var fields: [String: Any] = ["stage": stage.rawValue, "scope": context, "pageId": pageID,
                                     "attribution": "current-page", "source": call.source, "sequence": sequence,
                                     "requestRef": requestRef, "operationState": call.state.rawValue,
                                     "explicitCreateTaps": creates, "explicitStatusTaps": statusTaps]
        if let transport = call.transport { fields["transport"] = transport }
        emit(fields)
    }

    @MainActor
    private func emitOutcome(_ snapshot: Snapshot, browser: XCUIApplication, context: String, creates: Int, statusTaps: Int) {
        let knownCodes = ["host_unavailable", "invalid_host_response", "invalid_chat", "chat_create_conflict", "chat_create_deleted",
                          "chat_storage_uncertain", "storage_unavailable", "chat_creation_identity_mismatch", "unavailable"]
        var fields: [String: Any] = ["stage": "creation-outcome", "scope": context, "pageId": snapshot.diagnostic.pageID,
                                     "creationState": snapshot.receipt?.state.rawValue ?? "unobserved",
                                     "outcome": snapshot.receipt?.state == .confirmed ? "confirmed" : snapshot.receipt?.state == .rejected ? "rejected" : "unknown",
                                     "errorCode": "unavailable", "errorCodeAttribution": "unavailable",
                                     "explicitCreateTaps": creates, "explicitStatusTaps": statusTaps,
                                     "originalFaultCaptured": false]
        let stage: ChatDiagnosticStage = context == "explicit-status-once" ? .creationStatus : .create
        let source = stage == .create ? "create-button" : "status-button"
        // The shared error label has no provenance of its own. Only read its
        // allowlisted code when this action is the latest bound, settled call.
        if let reference = snapshot.diagnostic.creationRef,
           let call = snapshot.diagnostic.boundCall(stage, reference: reference),
           let sequence = call.sequence, call.source == source,
           sequence == snapshot.diagnostic.maximumSequence, call.state != .pending {
            let errors = browser.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", "Chat creation · "))
            if errors.count == 1 {
                let value = errors.firstMatch.label
                if value.utf8.count <= 512, let separator = value.range(of: ": ") {
                    let candidate = String(value[value.index(value.startIndex, offsetBy: "Chat creation · ".count)..<separator.lowerBound])
                    if knownCodes.contains(candidate) {
                        fields["errorCode"] = candidate
                        fields["errorSource"] = call.source
                        fields["errorSequence"] = sequence
                        fields["errorCodeAttribution"] = call.state == .failed ? "bound-operation" : "fixture-after-return"
                    }
                }
            }
        }
        emit(fields)
    }

    private func emit(_ fields: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: fields, options: [.sortedKeys]),
              let line = String(data: data, encoding: .utf8) else { return }
        print("IOS_CHAT_TRANSPORT_DIAGNOSTIC \(line)")
    }
}

