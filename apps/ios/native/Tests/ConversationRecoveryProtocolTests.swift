import Foundation
import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

// Dispatch workers mutate only this lock-protected collection.
private final class LedgerAttemptResults: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [String] = []
    func append(_ value: String) { lock.lock(); defer { lock.unlock() }; storage.append(value) }
    func values() -> [String] { lock.lock(); defer { lock.unlock() }; return storage }
}

@MainActor
final class ConversationRecoveryProtocolTests: XCTestCase {
    private func makeSource(root: String, storeRoot: String? = nil, project: String? = nil) -> EngineConversationSource {
        let config = EngineConfig(
            apiBase: "https://api.anthropic.com", apiKey: "", model: "", appSandboxRoot: root,
            projectCwd: project, sessionMode: .code, visionDelegationEnabled: true
        )
        return EngineConversationSource(config: config, recoveryStoreRootForTesting: storeRoot ?? root)
    }

    private func root() -> String {
        FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).path
    }

    private func seedAdmittedQueue(root: String, session: String, turns: [UInt64]) throws {
        let config = EngineConfig(apiBase: "https://api.anthropic.com", apiKey: "", model: "",
            appSandboxRoot: root, projectCwd: nil, sessionMode: .code, visionDelegationEnabled: true)
        let store = DurableConversationTurnClientStore(config: config)
        for turn in turns {
            let attempt = "attempt-\(turn)"
            try store.beginSubmission(sessionID: session, turnID: turn, kind: .prompt, attemptID: attempt)
            try store.markQueuedAdmitted(sessionID: session, turnID: turn, attemptID: attempt)
        }
    }

    private func snapshot(_ session: String, _ turn: UInt64, _ revision: UInt64,
                          _ state: TurnRecoveryStateDto, reason: String? = nil,
                          last: UInt64 = 0, uncertain: Bool = false) -> TurnRecoverySnapshotDto {
        TurnRecoverySnapshotDto(sessionId: session, turnId: turn, revision: revision, state: state,
                                firstSequence: 0, lastSequence: last, safeToResume: false,
                                projectionUncertain: uncertain, reason: reason)
    }

    private func flush(_ count: Int = 24) async {
        for _ in 0..<count { await Task.yield() }
    }

    private func pausedSource(session: String, root: String) -> EngineConversationSource {
        let source = makeSource(root: root)
        source.beginTurnForTesting(turnId: 71, sessionId: session)
        source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
        source.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, 1, .pausedRecoverable)))
        return source
    }

    func testAttachWaitsForItsFinishedAfterTheLocalFIFOProjectsReplay() async {
        let session = "fifo-\(UUID().uuidString)"
        let source = pausedSource(session: session, root: root())
        var request: String?
        var resumedWithText: [String] = []
        source.setCommandSubmitterForTesting { command in
            switch command {
            case let .attachTurn(owner, turn, _, requestID):
                XCTAssertEqual(owner, session)
                XCTAssertEqual(turn, 71)
                request = requestID
            case let .resumeTurn(owner, turn):
                XCTAssertEqual(owner, session)
                XCTAssertEqual(turn, 71)
                resumedWithText = source.model.messages.map(\.text)
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(session, 71, 3, .running)))
            default: break
            }
        }
        source.handleForeground()
        await flush()
        guard let request else { return XCTFail("expected an Attach request") }
        XCTAssertFalse(request.isEmpty)
        source.applyForTesting(.turnAttachFinished(requestId: request, sessionId: session, turnId: 71))
        source.applyForTesting(.turnAttachStarted(requestId: "stale", snapshot: snapshot("other", 71, 1, .pausedRecoverable)))
        source.applyForTesting(.turnAttachFinished(requestId: "stale", sessionId: "other", turnId: 71))
        XCTAssertTrue(resumedWithText.isEmpty)

        let listener = source.makeEventListenerForTesting()
        listener.enqueueForTesting(.turnAttachStarted(requestId: request, snapshot: snapshot(session, 71, 1, .pausedRecoverable)))
        listener.enqueueForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
            eventJson: #"{"type":"text_delta","text":"replayed"}"#))
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, 2, .pausedRecoverable)))
        listener.enqueueForTesting(.turnAttachFinished(requestId: "stale", sessionId: session, turnId: 71))
        await listener.waitUntilIdle()
        await flush()
        XCTAssertTrue(resumedWithText.isEmpty, "another request's finish cannot release Resume")
        listener.enqueueForTesting(.turnAttachFinished(requestId: request, sessionId: session, turnId: 71))
        await listener.waitUntilIdle()
        await flush()
        XCTAssertEqual(resumedWithText, ["replayed"])
        XCTAssertEqual(source.durableAttachCursorForTesting(), 1)
        source.retireRecoveryIngress()
    }

    func testTimedOutBatchCannotReplayAsLiveWhileTheNextAttemptStarts() async {
        let session = "late-batch-\(UUID().uuidString)"
        let source = pausedSource(session: session, root: root())
        var requests: [String] = []
        var resumeCount = 0
        source.setCommandSubmitterForTesting { command in
            switch command {
            case let .attachTurn(_, _, _, request): requests.append(request)
            case .resumeTurn: resumeCount += 1
            default: break
            }
        }
        source.handleForeground()
        await flush()
        guard let first = requests.first else { return XCTFail("expected first attach") }
        source.applyForTesting(.turnAttachStarted(requestId: first, snapshot: snapshot(session, 71, 1, .pausedRecoverable)))
        source.expireAttachAttemptForTesting()
        await flush()
        source.handleForeground()
        await flush()
        guard requests.count == 2 else { return XCTFail("expected a retry") }
        let second = requests[1]
        XCTAssertNotEqual(first, second)
        source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
            eventJson: #"{"type":"text_delta","text":"old"}"#))
        XCTAssertEqual(source.durableAttachCursorForTesting(), 0)
        XCTAssertFalse(source.model.messages.contains { $0.text.contains("old") })
        source.applyForTesting(.turnAttachFinished(requestId: first, sessionId: session, turnId: 71))
        source.applyForTesting(.turnAttachStarted(requestId: second, snapshot: snapshot(session, 71, 2, .pausedRecoverable)))
        source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
            eventJson: #"{"type":"text_delta","text":"new"}"#))
        source.applyForTesting(.turnAttachFinished(requestId: second, sessionId: session, turnId: 71))
        await flush()
        XCTAssertEqual(resumeCount, 1)
        XCTAssertEqual(source.durableAttachCursorForTesting(), 1)
        XCTAssertTrue(source.model.messages.contains { $0.text == "new" })
        source.retireRecoveryIngress()
    }

    func testTimedOutUnseededSourceBuffersOldStatesUntilRetrySnapshot() async {
        let root = root()
        let session = "seed-\(UUID().uuidString)"
        let previous = makeSource(root: root)
        previous.beginTurnForTesting(turnId: 71, sessionId: session)
        previous.beginDurableTurnForTesting(turnId: 71, sessionId: session)
        previous.retireRecoveryIngress()
        let source = makeSource(root: root)
        var requests: [String] = []
        var forwarded: [UInt64] = []
        source.setExternalEventHandler { event in
            if case let .turnRecoveryState(state) = event { forwarded.append(state.revision) }
        }
        source.setCommandSubmitterForTesting { command in
            if case let .attachTurn(_, _, _, request) = command { requests.append(request) }
        }
        source.expectSessionResumeForTesting(session)
        source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        await flush()
        guard requests.count == 1 else { return XCTFail("expected first attach") }
        source.expireAttachAttemptForTesting()
        await flush()
        source.applyForTesting(.turnAttachStarted(requestId: requests[0], snapshot: snapshot(session, 71, 6, .pausedRecoverable)))
        source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
            eventJson: #"{"type":"text_delta","text":"obsolete"}"#))
        source.applyForTesting(.turnAttachFinished(requestId: requests[0], sessionId: session, turnId: 71))
        XCTAssertEqual(source.durableAttachCursorForTesting(), 0)
        source.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, 7, .pausedRecoverable)))
        XCTAssertTrue(forwarded.isEmpty)
        source.handleForeground()
        await flush()
        guard requests.count == 2 else { return XCTFail("expected retry") }
        source.applyForTesting(.turnAttachStarted(requestId: requests[1], snapshot: snapshot(session, 71, 8, .cancelled)))
        XCTAssertEqual(forwarded, [8], "the lower stale state must not reach the external handler")
        source.applyForTesting(.turnAttachFinished(requestId: requests[1], sessionId: session, turnId: 71))
        await flush()
        source.retireRecoveryIngress()
    }

    func testNewerSharedWatermarkRejectsAnOlderStartedBatch() async {
        let root = root()
        let session = "peer-\(UUID().uuidString)"
        let source = pausedSource(session: session, root: root)
        let peer = makeSource(root: root)
        peer.beginTurnForTesting(turnId: 71, sessionId: session)
        var request: String?
        var resumeCount = 0
        source.setCommandSubmitterForTesting { command in
            if case let .attachTurn(_, _, _, id) = command { request = id }
            if case .resumeTurn = command { resumeCount += 1 }
        }
        source.handleForeground()
        await flush()
        guard let request else { return XCTFail("expected attach") }
        peer.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, 8, .pausedRecoverable)))
        source.applyForTesting(.turnAttachStarted(requestId: request, snapshot: snapshot(session, 71, 7, .pausedRecoverable)))
        source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
            eventJson: #"{"type":"text_delta","text":"stale"}"#))
        source.applyForTesting(.turnAttachFinished(requestId: request, sessionId: session, turnId: 71))
        await flush()
        XCTAssertEqual(resumeCount, 0)
        XCTAssertEqual(source.durableAttachCursorForTesting(), 0)
        XCTAssertFalse(source.model.messages.contains { $0.text.contains("stale") })
        source.retireRecoveryIngress()
        peer.retireRecoveryIngress()
    }

    func testBootstrapOverflowRemainsIncompleteAndNeverResumes() async {
        let root = root()
        let session = "overflow-bootstrap-\(UUID().uuidString)"
        let previous = makeSource(root: root)
        previous.beginTurnForTesting(turnId: 71, sessionId: session)
        previous.beginDurableTurnForTesting(turnId: 71, sessionId: session)
        previous.retireRecoveryIngress()
        let source = makeSource(root: root)
        var requests: [String] = []
        var resumeCount = 0
        source.setCommandSubmitterForTesting { command in
            if case let .attachTurn(_, _, _, request) = command { requests.append(request) }
            if case .resumeTurn = command { resumeCount += 1 }
        }
        source.expectSessionResumeForTesting(session)
        source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        await flush()
        XCTAssertEqual(requests.count, 1)
        for revision in 1...513 {
            source.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, UInt64(revision), .pausedRecoverable)))
        }
        await flush()
        source.handleForeground()
        await flush()
        guard requests.count == 2 else { return XCTFail("overflow must leave the turn retryable") }
        source.applyForTesting(.turnAttachStarted(requestId: requests[1], snapshot: snapshot(session, 71, 514, .pausedRecoverable)))
        source.applyForTesting(.turnAttachFinished(requestId: requests[1], sessionId: session, turnId: 71))
        await flush()
        XCTAssertEqual(resumeCount, 0)
        XCTAssertEqual(source.durableAttachCursorForTesting(), 0)
        XCTAssertNotNil(source.model.error)
        source.retireRecoveryIngress()
    }

    func testGapSnapshotsKeepTranscriptAndNeverAdvanceCursor() async {
        for reason in ["replay_cursor_ahead", "replay_history_truncated"] {
            let session = "gap-\(reason)-\(UUID().uuidString)"
            let source = pausedSource(session: session, root: root())
            source.recordDurableSequenceForTesting(turnId: 71, sequence: 2)
            source.model.messages = [Message(role: .ai, text: "existing transcript")]
            var request: String?
            var resumeCount = 0
            source.setCommandSubmitterForTesting { command in
                if case let .attachTurn(_, _, _, id) = command { request = id }
                if case .resumeTurn = command { resumeCount += 1 }
            }
            source.handleForeground()
            await flush()
            guard let request else { return XCTFail("expected attach for \(reason)") }
            source.applyForTesting(.turnAttachStarted(requestId: request,
                snapshot: snapshot(session, 71, 2, .waitingForUser, reason: reason, last: 99)))
            source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 3,
                eventJson: #"{"type":"text_delta","text":"must not append"}"#))
            source.applyForTesting(.turnAttachFinished(requestId: request, sessionId: session, turnId: 71))
            await flush()
            XCTAssertEqual(source.model.messages.map(\.text), ["existing transcript"])
            XCTAssertEqual(source.durableAttachCursorForTesting(), 2)
            XCTAssertNotNil(source.model.error, "\(reason) must remain visibly incomplete")
            XCTAssertEqual(resumeCount, 0)
            source.retireRecoveryIngress()
        }
    }

    func testWatermarkLivesUntilLastInterestedSourceRetires() {
        let root = root()
        let session = "interest-\(UUID().uuidString)"
        let first = makeSource(root: root, project: "/project-a")
        let second = makeSource(root: root, project: "/project-b")
        first.beginTurnForTesting(turnId: 71, sessionId: session)
        second.beginTurnForTesting(turnId: 71, sessionId: session)
        first.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, 8, .pausedRecoverable)))
        XCTAssertEqual(first.recoveryWatermarkForTesting(sessionID: session, turnID: 71)?.1, 2)
        first.retireRecoveryIngress()
        XCTAssertEqual(second.recoveryWatermarkForTesting(sessionID: session, turnID: 71)?.0, 8)
        XCTAssertEqual(second.recoveryWatermarkForTesting(sessionID: session, turnID: 71)?.1, 1)
        second.retireRecoveryIngress()
        XCTAssertNil(second.recoveryWatermarkForTesting(sessionID: session, turnID: 71))
    }

    func testDistinctSandboxConfigsShareActualHostStoreByDefault() {
        func source(_ path: String, _ project: String) -> EngineConversationSource {
            EngineConversationSource(config: EngineConfig(
                apiBase: "https://api.anthropic.com", apiKey: "", model: "", appSandboxRoot: path,
                projectCwd: project, sessionMode: .code, visionDelegationEnabled: true
            ), memoryOnly: true)
        }
        let a = source(root(), "/project-a")
        let b = source(root(), "/project-b")
        let session = "actual-home-\(UUID().uuidString)"
        a.beginTurnForTesting(turnId: 71, sessionId: session)
        b.beginTurnForTesting(turnId: 71, sessionId: session)
        a.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, 8, .pausedRecoverable)))
        XCTAssertEqual(b.recoveryWatermarkForTesting(sessionID: session, turnID: 71)?.0, 8)
        XCTAssertEqual(b.recoveryWatermarkForTesting(sessionID: session, turnID: 71)?.1, 2)
        a.retireRecoveryIngress()
        b.retireRecoveryIngress()
    }

    func testQueuedStopCannotRetargetSameTurnIDInAnotherSession() async {
        enum StopFailure: Error { case rejected }
        let source = makeSource(root: root())
        var cancelled: [(String?, UInt64?)] = []
        source.setCommandSubmitterForTesting { command in
            if case let .cancel(session, turn) = command {
                cancelled.append((session, turn))
                throw StopFailure.rejected
            }
        }
        source.beginTurnForTesting(turnId: 71, sessionId: "session-a")
        source.cancel()
        source.beginTurnForTesting(turnId: 71, sessionId: "session-b")
        await flush()
        XCTAssertFalse(cancelled.contains { $0.0 == "session-b" })
        XCTAssertTrue(source.model.streaming)
        source.retireRecoveryIngress()
    }

    func testImmediateStopWaitsForFirstSendOwnerAndHostAdmission() async {
        let source = makeSource(root: root())
        var ownerWaiter: CheckedContinuation<String, Never>?
        var admissionWaiter: CheckedContinuation<Void, Never>?
        var sent: [(String?, UInt64?)] = []
        var cancelled: [(String?, UInt64?)] = []
        source.setWorkspaceOwnerResolverForTesting {
            await withCheckedContinuation { ownerWaiter = $0 }
        }
        source.setCommandSubmitterForTesting { command in
            switch command {
            case let .sendPrompt(owner, _, _, _, turn, _):
                sent.append((owner, turn))
                await withCheckedContinuation { admissionWaiter = $0 }
            case let .cancel(owner, turn):
                cancelled.append((owner, turn))
                if let owner, let turn {
                    source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(owner, turn, 1, .cancelled)))
                }
            default: break
            }
        }
        XCTAssertTrue(source.model.activeSessionId.isEmpty)
        guard let token = source.send("first") else { return XCTFail("expected first turn") }
        await flush()
        XCTAssertNotNil(ownerWaiter)
        source.cancel()
        await flush()
        XCTAssertTrue(sent.isEmpty)
        XCTAssertTrue(cancelled.isEmpty)
        ownerWaiter?.resume(returning: "first-session")
        ownerWaiter = nil
        await flush()
        XCTAssertEqual(sent.count, 1)
        XCTAssertEqual(sent.first?.0, "first-session")
        XCTAssertEqual(sent.first?.1, token.clientTurnId)
        XCTAssertTrue(cancelled.isEmpty, "Cancel must wait until the host has accepted the send")
        admissionWaiter?.resume()
        admissionWaiter = nil
        await flush()
        XCTAssertEqual(cancelled.count, 1)
        XCTAssertEqual(cancelled.first?.0, "first-session")
        XCTAssertEqual(cancelled.first?.1, token.clientTurnId)
        XCTAssertFalse(source.model.streaming)
        XCTAssertEqual(source.model.notice, .cancelled)
        source.retireRecoveryIngress()
    }

    func testImmediatePauseWaitsForFirstSendOwnerAndHostAdmission() async throws {
        let source = makeSource(root: root())
        var ownerWaiter: CheckedContinuation<String, Never>?
        var admissionWaiter: CheckedContinuation<Void, Never>?
        var paused: [(String, UInt64)] = []
        source.setWorkspaceOwnerResolverForTesting {
            await withCheckedContinuation { ownerWaiter = $0 }
        }
        source.setCommandSubmitterForTesting { command in
            switch command {
            case .sendPrompt:
                await withCheckedContinuation { admissionWaiter = $0 }
            case let .pauseTurn(owner, turn, _):
                paused.append((owner, turn))
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(owner, turn, 1, .pausedRecoverable)))
            default: break
            }
        }
        guard let token = source.send("first") else { return XCTFail("expected first turn") }
        await flush()
        let pause = Task { try await source.markActiveTurnPausedRecoverable(token) }
        await flush()
        XCTAssertTrue(paused.isEmpty)
        ownerWaiter?.resume(returning: "first-session")
        ownerWaiter = nil
        await flush()
        XCTAssertNotNil(admissionWaiter)
        XCTAssertTrue(paused.isEmpty)
        admissionWaiter?.resume()
        admissionWaiter = nil
        try await pause.value
        XCTAssertEqual(paused.count, 1)
        XCTAssertEqual(paused.first?.0, "first-session")
        XCTAssertEqual(paused.first?.1, token.clientTurnId)
        source.retireRecoveryIngress()
    }

    func testTwoStopsShareOneSubmissionAndBothObserveItsTerminal() async throws {
        let source = makeSource(root: root())
        let session = "double-stop-\(UUID().uuidString)"
        source.beginTurnForTesting(turnId: 71, sessionId: session)
        source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
        var cancelCount = 0
        var release: CheckedContinuation<Void, Never>?
        source.setCommandSubmitterForTesting { command in
            if case .cancel = command {
                cancelCount += 1
                await withCheckedContinuation { release = $0 }
            }
        }
        let first = Task { try await source.cancelAndWait() }
        let second = Task { try await source.cancelAndWait() }
        await flush()
        XCTAssertEqual(cancelCount, 1)
        source.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, 1, .cancelled)))
        release?.resume()
        release = nil
        try await first.value
        try await second.value
        XCTAssertFalse(source.model.isCancelling)
        XCTAssertEqual(source.model.notice, .cancelled)
        source.retireRecoveryIngress()
    }

    func testStopDoesNotReplaceNaturalCompletionWithCancelled() async throws {
        let source = makeSource(root: root())
        let session = "natural-stop-\(UUID().uuidString)"
        source.beginTurnForTesting(turnId: 71, sessionId: session)
        source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
        var release: CheckedContinuation<Void, Never>?
        source.setCommandSubmitterForTesting { command in
            if case .cancel = command { await withCheckedContinuation { release = $0 } }
        }
        let stop = Task { try await source.cancelAndWait() }
        await flush()
        source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
            eventJson: #"{"type":"turn_ended","outcome":{"type":"end_turn"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#))
        source.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, 1, .completed)))
        release?.resume()
        release = nil
        try await stop.value
        XCTAssertNotEqual(source.model.notice, .cancelled)
        XCTAssertEqual(source.model.turnCompletion?.outcome, .completed)
        XCTAssertFalse(source.model.isCancelling)
        source.retireRecoveryIngress()
    }

    func testStopPreservesMaxTurnsWhenHostReportsFailedCheckpoint() async throws {
        let source = makeSource(root: root())
        let session = "limit-stop-\(UUID().uuidString)"
        source.beginTurnForTesting(turnId: 71, sessionId: session)
        source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
        var release: CheckedContinuation<Void, Never>?
        source.setCommandSubmitterForTesting { command in
            if case .cancel = command { await withCheckedContinuation { release = $0 } }
        }
        let stop = Task { try await source.cancelAndWait() }
        await flush()
        source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
            eventJson: #"{"type":"turn_ended","outcome":{"type":"max_turns"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#))
        source.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, 1, .failed, reason: "max_turns")))
        release?.resume()
        release = nil
        try await stop.value
        XCTAssertEqual(source.model.notice, .maxTurns)
        XCTAssertEqual(source.model.turnCompletion?.outcome, .maxTurns)
        XCTAssertNil(source.model.error)
        XCTAssertFalse(source.model.isCancelling)
        source.retireRecoveryIngress()
    }

    func testNaturalMaxTurnsDoesNotBecomeABackgroundFailure() {
        let source = makeSource(root: root())
        let session = "limit-natural-\(UUID().uuidString)"
        source.beginTurnForTesting(turnId: 71, sessionId: session)
        source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
        source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
            eventJson: #"{"type":"turn_ended","outcome":{"type":"max_turns"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#))
        source.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, 1, .failed, reason: "max_turns")))
        XCTAssertEqual(source.model.notice, .maxTurns)
        XCTAssertEqual(source.model.turnCompletion?.outcome, .maxTurns)
        XCTAssertNil(source.model.error)
        source.retireRecoveryIngress()
    }

    func testColdMaxTurnsSnapshotRestoresNoticeWithoutInventingTranscript() async {
        let root = root()
        let session = "limit-cold-\(UUID().uuidString)"
        let previous = makeSource(root: root)
        previous.beginDurableTurnForTesting(turnId: 71, sessionId: session)
        previous.retireRecoveryIngress()
        let source = makeSource(root: root)
        source.setCommandSubmitterForTesting { command in
            switch command {
            case let .attachTurn(_, _, _, request):
                source.applyForTesting(.turnAttachStarted(requestId: request,
                    snapshot: self.snapshot(session, 71, 1, .failed, reason: "max_turns")))
                source.applyForTesting(.turnAttachFinished(requestId: request, sessionId: session, turnId: 71))
            case .resumeTurn:
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(session, 71, 1, .failed, reason: "max_turns")))
            default: break
            }
        }
        source.expectSessionResumeForTesting(session)
        source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: [
            MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "persisted".clientTextDto)])
        ]))
        await flush()
        XCTAssertEqual(source.model.notice, .maxTurns)
        XCTAssertEqual(source.model.messages.map(\.text), ["persisted"])
        XCTAssertFalse(source.model.items.contains { if case .run = $0 { return true }; return false })
        XCTAssertNil(source.model.error)
        source.retireRecoveryIngress()
    }

    func testPendingFirstSendDoesNotAdoptAnotherSessionAfterSwitch() async {
        let source = makeSource(root: root())
        var ownerWaiter: CheckedContinuation<String, Never>?
        var sends: [String?] = []
        source.setWorkspaceOwnerResolverForTesting {
            await withCheckedContinuation { ownerWaiter = $0 }
        }
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(owner, _, _, _, _, _) = command { sends.append(owner) }
        }
        XCTAssertNotNil(source.send("first"))
        await flush()
        source.expectSessionResumeForTesting("session-b")
        source.applyForTesting(.sessionResumed(sessionId: "session-b", mode: .code, messages: []))
        ownerWaiter?.resume(returning: "session-b")
        ownerWaiter = nil
        await flush()
        XCTAssertTrue(sends.isEmpty, "an old epoch cannot submit into the newly active session")
        source.retireRecoveryIngress()
    }

    func testAlreadyPreparedSendRetainsOriginAcrossSessionSwitch() async {
        let source = makeSource(root: root())
        source.model.activeSessionId = "session-a"
        var release: CheckedContinuation<Void, Never>?
        var submittedOwner: String?
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(owner, _, _, _, _, _) = command {
                submittedOwner = owner
                await withCheckedContinuation { release = $0 }
            }
        }
        XCTAssertNotNil(source.send("origin a"))
        await flush()
        XCTAssertEqual(submittedOwner, "session-a")
        source.expectSessionResumeForTesting("session-b")
        source.applyForTesting(.sessionResumed(sessionId: "session-b", mode: .code, messages: []))
        release?.resume()
        release = nil
        await flush()
        XCTAssertEqual(submittedOwner, "session-a", "the command remains fenced to its original owner")
        XCTAssertEqual(source.model.activeSessionId, "session-b")
        source.retireRecoveryIngress()
    }

    func testInFlightStopCannotUseOtherSessionsTerminalAcknowledgement() async {
        let source = makeSource(root: root())
        var submitted: [(String?, UInt64?)] = []
        var release: CheckedContinuation<Void, Never>?
        var succeeded = false
        source.setCommandSubmitterForTesting { command in
            if case let .cancel(session, turn) = command {
                submitted.append((session, turn))
                await withCheckedContinuation { release = $0 }
            }
        }
        source.beginTurnForTesting(turnId: 71, sessionId: "session-a")
        source.beginDurableTurnForTesting(turnId: 71, sessionId: "session-a")
        let stop = Task { @MainActor in
            do { try await source.cancelAndWait(); succeeded = true }
            catch { }
        }
        await flush()
        guard submitted.count == 1 else { stop.cancel(); return XCTFail("expected A's Stop") }
        XCTAssertEqual(submitted[0].0, "session-a")
        XCTAssertEqual(submitted[0].1, 71)
        source.beginTurnForTesting(turnId: 71, sessionId: "session-b")
        source.beginDurableTurnForTesting(turnId: 71, sessionId: "session-b")
        source.applyForTesting(.turnRecoveryState(snapshot: snapshot("session-b", 71, 1, .cancelled)))
        release?.resume()
        release = nil
        await flush(64)
        XCTAssertFalse(succeeded, "B's same-numbered terminal must not acknowledge A's Stop")
        stop.cancel()
        await stop.value
        source.retireRecoveryIngress()
    }

    func testStopWithoutAnOwnerNeverSendsAnUnscopedMainCancellation() async {
        let source = makeSource(root: root())
        var cancelCount = 0
        source.setCommandSubmitterForTesting { command in
            if case .cancel = command { cancelCount += 1 }
        }
        try? await source.cancelAndWait()
        source.beginTurnForTesting(turnId: 71, sessionId: "")
        do {
            try await source.cancelAndWait()
            XCTFail("a turn without an established session cannot be cancelled safely")
        } catch { }
        XCTAssertEqual(cancelCount, 0)
        XCTAssertNotNil(source.model.error)
        source.retireRecoveryIngress()
    }

    func testTypedNotAdmittedClearsOnlyTheMatchingSubmission() async throws {
        let root = root()
        let session = "not-admitted-\(UUID().uuidString)"
        let source = makeSource(root: root)
        source.model.activeSessionId = session
        var attachCount = 0
        source.setCommandSubmitterForTesting { command in
            if case .sendPrompt = command { throw ClientError.SubmissionNotAdmitted(message: "any text") }
            if case .attachTurn = command { attachCount += 1 }
        }
        XCTAssertNotNil(source.send("not admitted"))
        await flush(48)
        XCTAssertTrue(try source.pendingSubmissionsForTesting().isEmpty)
        XCTAssertNil(source.durableAttachCursorForTesting())
        XCTAssertFalse(source.model.hasUnresolvedTurnRecovery)
        XCTAssertEqual(attachCount, 0)
        XCTAssertNotNil(source.send("can retry"))
        source.retireRecoveryIngress()
    }

    func testLedgerPublicationSyncFailureBeforeDispatchCleansOnlyOwnAttempt() async throws {
        let source = makeSource(root: root())
        source.model.activeSessionId = "sync-failure"
        var sent = 0
        source.setCommandSubmitterForTesting { command in
            if case .sendPrompt = command { sent += 1 }
        }
        source.failNextSubmissionSyncForTesting()
        XCTAssertNotNil(source.send("never dispatched"))
        await flush(64)
        XCTAssertEqual(sent, 0)
        XCTAssertTrue(try source.pendingSubmissionsForTesting().isEmpty)
        XCTAssertNil(source.durableAttachCursorForTesting())
        XCTAssertNotNil(source.model.error)
        source.retireRecoveryIngress()
    }

    func testConcurrentSubmissionOwnersCannotDeleteEachOthersAttempt() throws {
        let config = EngineConfig(apiBase: "https://api.anthropic.com", apiKey: "", model: "",
            appSandboxRoot: root(), projectCwd: nil, sessionMode: .code, visionDelegationEnabled: true)
        let stores = [DurableConversationTurnClientStore(config: config), DurableConversationTurnClientStore(config: config)]
        let results = LedgerAttemptResults()
        DispatchQueue.concurrentPerform(iterations: 2) { index in
            let attempt = "owner-\(index)"
            do {
                try stores[index].beginSubmission(sessionID: "same-session", turnID: 17, kind: .prompt, attemptID: attempt)
                results.append(attempt)
            } catch { }
        }
        let winners = results.values()
        XCTAssertEqual(winners.count, 1)
        guard let winner = winners.first else { return }
        let loser = winner == "owner-0" ? "owner-1" : "owner-0"
        try stores[1].clearSubmission(sessionID: "same-session", turnID: 17, attemptID: loser)
        XCTAssertEqual(try stores[0].pendingSubmissions().first?.attemptID, winner)
        try stores[0].clearSubmission(sessionID: "same-session", turnID: 17, attemptID: winner)
        XCTAssertTrue(try stores[1].pendingSubmissions().isEmpty)
    }

    func testAcceptedPromptKeepsLedgerWhenCheckpointCannotBeSynced() async throws {
        let path = root()
        let session = "checkpoint-failure-\(UUID().uuidString)"
        let first = makeSource(root: path)
        first.model.activeSessionId = session
        first.setFailMainCheckpointSyncForTesting(true)
        var accepted = 0
        first.setCommandSubmitterForTesting { command in
            if case .sendPrompt = command { accepted += 1 }
            if case .attachTurn = command { throw ClientError.NotFound(message: "not yet visible") }
        }
        guard let token = first.send("accepted") else { return XCTFail("expected send") }
        await flush(64)
        XCTAssertEqual(accepted, 1)
        XCTAssertEqual(try first.pendingSubmissionsForTesting().map(\.turnID), [token.clientTurnId])
        XCTAssertTrue(first.model.hasUnresolvedTurnRecovery)
        first.retireRecoveryIngress()

        let reopened = makeSource(root: path)
        var resumes = 0
        reopened.setCommandSubmitterForTesting { command in
            if case .attachTurn = command { throw ClientError.NotFound(message: "not yet visible") }
            if case .resumeTurn = command { resumes += 1 }
        }
        reopened.expectSessionResumeForTesting(session)
        reopened.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        await flush(64)
        XCTAssertEqual(try reopened.pendingSubmissionsForTesting().map(\.turnID), [token.clientTurnId])
        XCTAssertEqual(resumes, 0)
        reopened.retireRecoveryIngress()
    }

    func testFailedNewOrResumeCancellationKeepsOriginalCheckpointAcrossRestart() async throws {
        for newSession in [true, false] {
            let path = root()
            let session = "cancel-failed-\(UUID().uuidString)"
            let source = makeSource(root: path)
            source.model.activeSessionId = session
            var release: CheckedContinuation<Void, Never>?
            var transitionCommands = 0
            source.setCommandSubmitterForTesting { command in
                switch command {
                case .cancel:
                    await withCheckedContinuation { release = $0 }
                    throw ClientError.Transport(message: "cancel failed")
                case .newSession, .resumeSession: transitionCommands += 1
                default: break
                }
            }
            guard let turn = source.send("A") else { return XCTFail("expected A") }
            await flush(48)
            XCTAssertEqual(source.durableAttachCursorForTesting(), 0)
            if newSession { source.startNewConversation() }
            else { source.resumeSession("session-B", emptySessionTitle: nil) }
            await flush(48)
            XCTAssertTrue(source.model.sessionTransitionPending)
            XCTAssertNotNil(release)
            XCTAssertEqual(source.durableAttachCursorForTesting(), 0)
            release?.resume()
            release = nil
            await flush(64)
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertTrue(source.model.streaming)
            XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, turn.clientTurnId)
            XCTAssertEqual(source.durableAttachCursorForTesting(), 0)
            XCTAssertEqual(transitionCommands, 0)
            source.retireRecoveryIngress()

            let reopened = makeSource(root: path)
            reopened.setCommandSubmitterForTesting { command in
                if case .attachTurn = command { throw ClientError.NotFound(message: "retry later") }
            }
            reopened.expectSessionResumeForTesting(session)
            reopened.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
            await flush(48)
            XCTAssertEqual(reopened.durableAttachCursorForTesting(), 0)
            XCTAssertEqual(reopened.model.activeTurnToken?.clientTurnId, turn.clientTurnId)
            reopened.retireRecoveryIngress()
        }
    }

    func testLocalAppAuthoringCannotBypassQueuedOrUnknownSubmission() async throws {
        for admitted in [true, false] {
            let path = root()
            let session = "authoring-guard-\(UUID().uuidString)"
            if admitted {
                try seedAdmittedQueue(root: path, session: session, turns: [81])
            } else {
                let config = EngineConfig(apiBase: "https://api.anthropic.com", apiKey: "", model: "",
                    appSandboxRoot: path, projectCwd: nil, sessionMode: .code, visionDelegationEnabled: true)
                try DurableConversationTurnClientStore(config: config).beginSubmission(
                    sessionID: session, turnID: 81, kind: .prompt, attemptID: "unknown-attempt")
            }
            let source = makeSource(root: path)
            source.model.activeSessionId = session
            var newSessions = 0
            source.setCommandSubmitterForTesting { command in
                if case .newSession = command { newSessions += 1 }
            }
            do {
                _ = try await source.startLocalAppAuthoringSession(cwd: "/project")
                XCTFail("authoring must not abandon a pending submission")
            } catch { }
            XCTAssertEqual(newSessions, 0)
            XCTAssertEqual(source.model.activeSessionId, session)
            XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [81])
            source.retireRecoveryIngress()
        }
    }

    func testGenericSubmissionFailuresRemainUnknownEvenWhenTheirTextLooksDefinitive() async throws {
        let failures: [Error] = [
            ClientError.Rejected(message: "submission not admitted"),
            ClientError.Internal(message: "submission not admitted"),
            ClientError.Transport(message: "submission not admitted"),
        ]
        for failure in failures {
            let session = "unknown-\(UUID().uuidString)"
            let source = makeSource(root: root())
            source.model.activeSessionId = session
            var attachCount = 0
            var resumeCount = 0
            var newSessionCount = 0
            source.setCommandSubmitterForTesting { command in
                switch command {
                case .sendPrompt: throw failure
                case .attachTurn:
                    attachCount += 1
                    throw ClientError.NotFound(message: "no checkpoint")
                case .resumeTurn: resumeCount += 1
                case .newSession: newSessionCount += 1
                default: break
                }
            }
            guard let token = source.send("uncertain") else { return XCTFail("expected initial send") }
            await flush(64)
            let pending = try source.pendingSubmissionsForTesting()
            XCTAssertEqual(pending.map(\.turnID), [token.clientTurnId])
            XCTAssertEqual(pending.first?.sessionID, session)
            XCTAssertEqual(source.durableAttachCursorForTesting(), 0)
            XCTAssertTrue(source.model.hasUnresolvedTurnRecovery)
            XCTAssertNil(source.send("must remain blocked"))
            source.startNewConversation()
            XCTAssertEqual(newSessionCount, 0)
            XCTAssertGreaterThanOrEqual(attachCount, 1)
            XCTAssertEqual(resumeCount, 0)
            XCTAssertEqual(source.model.error?.message, String(localized: "chat_submission_not_found_unknown"))
            source.handleForeground()
            await flush(48)
            XCTAssertGreaterThanOrEqual(attachCount, 2)
            XCTAssertEqual(try source.pendingSubmissionsForTesting().count, 1)
            XCTAssertEqual(resumeCount, 0)
            source.retireRecoveryIngress()
        }
    }

    func testStopWaitingForGenericSubmissionFailureKeepsTheUnknownRecord() async throws {
        let source = makeSource(root: root())
        source.model.activeSessionId = "stop-unknown"
        var release: CheckedContinuation<Void, Never>?
        var cancels = 0
        var resumes = 0
        source.setCommandSubmitterForTesting { command in
            switch command {
            case .sendPrompt:
                await withCheckedContinuation { release = $0 }
                throw ClientError.Transport(message: "response lost")
            case .cancel: cancels += 1
            case .attachTurn: throw ClientError.NotFound(message: "no checkpoint")
            case .resumeTurn: resumes += 1
            default: break
            }
        }
        guard let token = source.send("pending") else { return XCTFail("expected turn") }
        await flush()
        let stop = Task { try await source.cancelAndWait() }
        await flush()
        release?.resume()
        release = nil
        do { try await stop.value; XCTFail("unknown submission must not count as a successful Stop") }
        catch { }
        await flush(48)
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [token.clientTurnId])
        XCTAssertEqual(source.durableAttachCursorForTesting(), 0)
        XCTAssertTrue(source.model.hasUnresolvedTurnRecovery)
        XCTAssertEqual(cancels, 0)
        XCTAssertEqual(resumes, 0)
        source.retireRecoveryIngress()
    }

    func testStopWaitingForTypedNotAdmittedDoesNotKeepAFalseCheckpoint() async throws {
        let source = makeSource(root: root())
        source.model.activeSessionId = "stop-not-admitted"
        var release: CheckedContinuation<Void, Never>?
        var cancels = 0
        source.setCommandSubmitterForTesting { command in
            if case .sendPrompt = command {
                await withCheckedContinuation { release = $0 }
                throw ClientError.SubmissionNotAdmitted(message: "denied")
            }
            if case .cancel = command { cancels += 1 }
        }
        XCTAssertNotNil(source.send("pending"))
        await flush()
        let stop = Task { try await source.cancelAndWait() }
        await flush()
        release?.resume()
        release = nil
        do { try await stop.value; XCTFail("the send was not admitted") } catch { }
        await flush()
        XCTAssertTrue(try source.pendingSubmissionsForTesting().isEmpty)
        XCTAssertNil(source.durableAttachCursorForTesting())
        XCTAssertFalse(source.model.hasUnresolvedTurnRecovery)
        XCTAssertEqual(cancels, 0)
        source.retireRecoveryIngress()
    }

    func testColdUnknownSubmissionProbesWithoutResumeAndWaitsForTerminal() async throws {
        let root = root()
        let session = "cold-unknown-\(UUID().uuidString)"
        let first = makeSource(root: root)
        first.model.activeSessionId = session
        first.setCommandSubmitterForTesting { command in
            if case .sendPrompt = command { throw ClientError.Internal(message: "reply lost") }
            if case .attachTurn = command { throw ClientError.NotFound(message: "no record yet") }
        }
        guard let token = first.send("unknown") else { return XCTFail("expected initial send") }
        await flush(64)
        XCTAssertEqual(try first.pendingSubmissionsForTesting().count, 1)
        first.retireRecoveryIngress()

        let reopened = makeSource(root: root)
        var attempts = 0
        var resumes = 0
        reopened.setCommandSubmitterForTesting { command in
            switch command {
            case let .attachTurn(owner, turn, _, request):
                attempts += 1
                let state: TurnRecoveryStateDto = attempts == 1 ? .running : .completed
                reopened.applyForTesting(.turnAttachStarted(requestId: request,
                    snapshot: self.snapshot(owner, turn, UInt64(attempts + 1), state)))
                reopened.applyForTesting(.turnAttachFinished(requestId: request, sessionId: owner, turnId: turn))
            case .resumeTurn: resumes += 1
            default: break
            }
        }
        reopened.expectSessionResumeForTesting(session)
        reopened.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: [
            MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "previous".clientTextDto)])
        ]))
        await flush(64)
        XCTAssertEqual(attempts, 1)
        XCTAssertEqual(resumes, 0)
        XCTAssertEqual(try reopened.pendingSubmissionsForTesting().map(\.turnID), [token.clientTurnId])
        XCTAssertEqual(reopened.model.messages.map(\.text), ["previous"])
        XCTAssertTrue(reopened.model.hasUnresolvedTurnRecovery)
        reopened.handleForeground()
        await flush(64)
        XCTAssertEqual(attempts, 2)
        XCTAssertEqual(resumes, 0)
        XCTAssertTrue(try reopened.pendingSubmissionsForTesting().isEmpty)
        XCTAssertNil(reopened.durableAttachCursorForTesting())
        reopened.retireRecoveryIngress()
    }

    func testUnknownQueuedSubmissionCannotOverwriteRunningTurn() async throws {
        let source = makeSource(root: root())
        let session = "queued-unknown-\(UUID().uuidString)"
        source.model.activeSessionId = session
        var submitted: [UInt64] = []
        var attaches: [UInt64] = []
        var resumes = 0
        source.setCommandSubmitterForTesting { command in
            switch command {
            case let .sendPrompt(_, _, _, _, turn?, _):
                submitted.append(turn)
                if submitted.count == 2 { throw ClientError.Transport(message: "queue response lost") }
            case let .attachTurn(_, turn, _, _):
                attaches.append(turn)
                throw ClientError.NotFound(message: "not yet claimed")
            case .resumeTurn: resumes += 1
            default: break
            }
        }
        guard let first = source.send("first") else { return XCTFail("expected first") }
        await flush()
        XCTAssertNotNil(source.send("queued"))
        await flush(64)
        guard submitted.count == 2 else { return XCTFail("expected two submissions") }
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, first.clientTurnId)
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [submitted[1]])
        XCTAssertEqual(source.durableAttachCursorForTesting(), 0)
        XCTAssertTrue(attaches.isEmpty, "queued B must wait while A owns the slot")
        source.applyForTesting(.turnEventReplay(sessionId: session, turnId: first.clientTurnId, sequence: 1,
            eventJson: #"{"type":"turn_ended","outcome":{"type":"end_turn"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#))
        source.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, first.clientTurnId, 1, .completed)))
        await flush(64)
        XCTAssertEqual(attaches, [submitted[1]])
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [submitted[1]])
        XCTAssertEqual(resumes, 0)
        source.retireRecoveryIngress()
    }

    func testColdDeliveredBatchRequiresCorrelatedQueryAndProjectedUserRow() async throws {
        let path = root()
        let session = "batch-\(UUID().uuidString)"
        let uuid = UUID().uuidString
        try seedAdmittedQueue(root: path, session: session, turns: [11, 12])
        let source = makeSource(root: path)
        var requests: [UInt64: [String]] = [:]
        var newSessions = 0
        source.setCommandSubmitterForTesting { command in
            if case let .queryQueuedTurn(owner, turn, request) = command {
                XCTAssertEqual(owner, session)
                requests[turn, default: []].append(request)
            }
            if case .newSession = command { newSessions += 1 }
        }
        source.expectSessionResumeForTesting(session)
        source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: [
            MessageDto(transcriptUuid: uuid, role: "user", blocks: [.text(text: "queued A and B".clientTextDto)])
        ]))
        await flush(64)
        guard let oldRequest = requests[11]?.first, let secondRequest = requests[12]?.first else {
            return XCTFail("expected a correlated query for each queued member")
        }
        let delivered = QueuedTurnStatusDto.delivered(batchOwnerTurnId: 11, memberTurnIds: [11, 12], transcriptUuid: uuid)
        source.applyForTesting(.queuedTurnStatus(sessionId: "another-session", turnId: 11, requestId: oldRequest, status: delivered))
        source.handleForeground()
        await flush(64)
        guard let freshRequest = requests[11]?.last, freshRequest != oldRequest,
              let freshSecondRequest = requests[12]?.last, freshSecondRequest != secondRequest else {
            return XCTFail("foreground retry must replace old request IDs")
        }
        source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 11, requestId: oldRequest, status: delivered))
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [11, 12])
        source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 11, requestId: freshRequest, status: delivered))
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [12])
        source.startNewConversation()
        await flush()
        XCTAssertEqual(newSessions, 0)
        source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 12, requestId: freshSecondRequest, status: delivered))
        XCTAssertTrue(try source.pendingSubmissionsForTesting().isEmpty)
        XCTAssertEqual(source.model.messages.map(\.text), ["queued A and B"])
        source.startNewConversation()
        await flush()
        XCTAssertEqual(newSessions, 1)
        source.retireRecoveryIngress()
    }

    func testDeliveredBatchWithoutCompleteProjectedRowRemainsRecoverable() async throws {
        for variant in 0..<4 {
            let path = root()
            let session = "incomplete-batch-\(UUID().uuidString)"
            let uuid = UUID().uuidString
            try seedAdmittedQueue(root: path, session: session, turns: [21])
            let source = makeSource(root: path)
            var request: String?
            source.setCommandSubmitterForTesting { command in
                if case let .queryQueuedTurn(_, _, requestID) = command { request = requestID }
            }
            let messages: [MessageDto]
            switch variant {
            case 0: messages = []
            case 1: messages = [MessageDto(transcriptUuid: uuid, role: "assistant", blocks: [.text(text: "not user".clientTextDto)])]
            case 2: messages = [MessageDto(transcriptUuid: uuid, role: "user", blocks: [.text(text: "   ".clientTextDto)])]
            default:
                messages = [MessageDto(transcriptUuid: uuid, role: "user", blocks: [.text(text: ClientTextDto(codeUnits: [0xd800]))])]
            }
            source.expectSessionResumeForTesting(session)
            source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: messages))
            await flush(48)
            guard let request else { return XCTFail("expected query") }
            source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 21, requestId: request,
                status: .delivered(batchOwnerTurnId: 21, memberTurnIds: [21], transcriptUuid: uuid)))
            XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [21])
            XCTAssertNotNil(source.model.error, "missing, filtered, or lossy rows must be visible")
            source.retireRecoveryIngress()
        }
    }

    func testFailedSessionTransitionRetriesQueriesDiscardedDuringTransition() async throws {
        let path = root()
        let session = "transition-query-\(UUID().uuidString)"
        let uuid = UUID().uuidString
        try seedAdmittedQueue(root: path, session: session, turns: [22])
        let source = makeSource(root: path)
        var requests: [String] = []
        source.setCommandSubmitterForTesting { command in
            if case let .queryQueuedTurn(_, _, request) = command { requests.append(request) }
        }
        source.expectSessionResumeForTesting(session)
        source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: [
            MessageDto(transcriptUuid: uuid, role: "user", blocks: [.text(text: "queued".clientTextDto)])
        ]))
        await flush(48)
        guard let old = requests.first else { return XCTFail("expected query") }
        source.beginSessionTransitionForTesting()
        let delivered = QueuedTurnStatusDto.delivered(batchOwnerTurnId: 22, memberTurnIds: [22], transcriptUuid: uuid)
        source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 22, requestId: old, status: delivered))
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [22])
        source.failSessionTransitionForTesting()
        await flush(48)
        guard let fresh = requests.last, fresh != old else { return XCTFail("failed transition must requery") }
        source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 22, requestId: old, status: delivered))
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [22])
        source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 22, requestId: fresh, status: delivered))
        XCTAssertTrue(try source.pendingSubmissionsForTesting().isEmpty)
        source.retireRecoveryIngress()
    }

    func testRunningQueuedStatusUsesProbeWithoutAutomaticResume() async throws {
        let path = root()
        let session = "running-queued-\(UUID().uuidString)"
        try seedAdmittedQueue(root: path, session: session, turns: [31])
        let source = makeSource(root: path)
        var queries: [String] = []
        var attaches = 0
        var resumes = 0
        source.setCommandSubmitterForTesting { command in
            switch command {
            case let .queryQueuedTurn(_, _, request): queries.append(request)
            case let .attachTurn(owner, turn, _, request):
                attaches += 1
                source.applyForTesting(.turnAttachStarted(requestId: request, snapshot: self.snapshot(owner, turn, 1, .running)))
                source.applyForTesting(.turnAttachFinished(requestId: request, sessionId: owner, turnId: turn))
            case .resumeTurn: resumes += 1
            default: break
            }
        }
        source.expectSessionResumeForTesting(session)
        source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        await flush(48)
        guard let request = queries.first else { return XCTFail("expected query") }
        source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 31, requestId: request, status: .running))
        await flush(64)
        XCTAssertEqual(attaches, 1)
        XCTAssertEqual(resumes, 0)
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [31])
        XCTAssertTrue(source.model.hasUnresolvedTurnRecovery)
        source.retireRecoveryIngress()
    }

    func testFoldedQueuedUserRowCannotClearDeliveredEvidence() async throws {
        let path = root()
        let session = "folded-\(UUID().uuidString)"
        let uuid = UUID().uuidString
        try seedAdmittedQueue(root: path, session: session, turns: [41])
        let source = makeSource(root: path)
        var request: String?
        source.setCommandSubmitterForTesting { command in
            if case let .queryQueuedTurn(_, _, requestID) = command { request = requestID }
        }
        source.expectSessionResumeForTesting(session)
        source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: [
            MessageDto(transcriptUuid: nil, role: "system", blocks: [],
                loopWakeup: LoopWakeupDto(message: "first", companion: nil, streak: 0, sinceMs: 0)),
            MessageDto(transcriptUuid: uuid, role: "user", blocks: [.text(text: "hidden input".clientTextDto)]),
            MessageDto(transcriptUuid: nil, role: "system", blocks: [],
                loopWakeup: LoopWakeupDto(message: "second", companion: nil, streak: 1, sinceMs: 1))
        ]))
        await flush(48)
        guard let request else { return XCTFail("expected query") }
        XCTAssertFalse(source.model.selectedAgentItems.contains { item in
            if case let .message(message) = item { return message.transcriptUUID == UUID(uuidString: uuid) }
            return false
        })
        source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 41, requestId: request,
            status: .delivered(batchOwnerTurnId: 41, memberTurnIds: [41], transcriptUuid: uuid)))
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [41])
        XCTAssertNotNil(source.model.error)
        source.retireRecoveryIngress()
    }

    func testPrivateQueuedCancellationIsProjectedBeforeLedgerRelease() async throws {
        let path = root()
        let session = "cancelled-queue-\(UUID().uuidString)"
        try seedAdmittedQueue(root: path, session: session, turns: [51])
        let source = makeSource(root: path)
        var request: String?
        source.setCommandSubmitterForTesting { command in
            if case let .queryQueuedTurn(_, _, requestID) = command { request = requestID }
        }
        source.expectSessionResumeForTesting(session)
        source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        await flush(48)
        guard let request else { return XCTFail("expected query") }
        source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 51, requestId: request, status: .cancelled))
        XCTAssertTrue(source.model.items.contains {
            if case let .notice(notice) = $0 { return notice.text == String(localized: "chat_queued_cancelled") }
            return false
        })
        XCTAssertTrue(try source.pendingSubmissionsForTesting().isEmpty)
        XCTAssertEqual(try source.terminalEvidenceForTesting(sessionID: session).first?.queuedCancelled, true)
        source.retireRecoveryIngress()
        let reopened = makeSource(root: path)
        reopened.expectSessionResumeForTesting(session)
        reopened.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        XCTAssertEqual(reopened.model.recoveryEvidenceForReview.first?.queuedCancelled, true)
        XCTAssertEqual(reopened.model.error?.message, String(localized: "chat_queued_cancelled"))
        reopened.dismissError()
        XCTAssertFalse(try reopened.terminalEvidenceForTesting(sessionID: session).isEmpty)
        reopened.acknowledgeRecoveryEvidence(reopened.model.recoveryEvidenceForReview)
        XCTAssertTrue(try reopened.terminalEvidenceForTesting(sessionID: session).isEmpty)
        reopened.retireRecoveryIngress()
    }

    func testQueuedCancellationReceiptFailureKeepsLedgerUntilRetry() async throws {
        let path = root()
        let session = "cancelled-receipt-failure-\(UUID().uuidString)"
        try seedAdmittedQueue(root: path, session: session, turns: [52])
        let source = makeSource(root: path)
        var requests: [String] = []
        source.setCommandSubmitterForTesting { command in
            if case let .queryQueuedTurn(_, _, request) = command { requests.append(request) }
        }
        source.expectSessionResumeForTesting(session)
        source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        await flush(48)
        guard let first = requests.first else { return XCTFail("expected query") }
        source.failNextTerminalEvidenceSyncForTesting()
        source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 52, requestId: first, status: .cancelled))
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [52])
        XCTAssertFalse(source.model.items.contains {
            if case let .notice(notice) = $0 { return notice.text == String(localized: "chat_queued_cancelled") }
            return false
        })
        source.handleForeground()
        await flush(48)
        guard let retry = requests.last, retry != first else { return XCTFail("expected retry") }
        source.applyForTesting(.queuedTurnStatus(sessionId: session, turnId: 52, requestId: retry, status: .cancelled))
        XCTAssertTrue(try source.pendingSubmissionsForTesting().isEmpty)
        XCTAssertEqual(try source.terminalEvidenceForTesting(sessionID: session).first?.queuedCancelled, true)
        source.retireRecoveryIngress()
    }

    func testAcceptedQueuedBRemainsInterestedAfterSourceReplacement() async throws {
        let path = root()
        let session = "cold-queued-\(UUID().uuidString)"
        let first = makeSource(root: path)
        first.model.activeSessionId = session
        var turns: [UInt64] = []
        first.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(_, _, _, _, turn?, _) = command { turns.append(turn) }
        }
        guard let a = first.send("A") else { return XCTFail("expected A") }
        await flush(24)
        XCTAssertNotNil(first.send("!queued B"))
        await flush(64)
        guard turns.count == 2 else { return XCTFail("expected accepted A and B") }
        let b = turns[1]
        XCTAssertEqual(try first.pendingSubmissionsForTesting().map(\.turnID), [b])
        first.retireRecoveryIngress()

        let reopened = makeSource(root: path)
        reopened.setCommandSubmitterForTesting { command in
            switch command {
            case let .attachTurn(owner, turn, _, request):
                reopened.applyForTesting(.turnAttachStarted(requestId: request, snapshot: self.snapshot(owner, turn, 1, .pausedRecoverable)))
                reopened.applyForTesting(.turnAttachFinished(requestId: request, sessionId: owner, turnId: turn))
            case let .resumeTurn(owner, turn):
                reopened.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(owner, turn, 2, .running)))
            case let .queryQueuedTurn(owner, turn, request):
                reopened.applyForTesting(.queuedTurnStatus(sessionId: owner, turnId: turn, requestId: request, status: .pending))
            default: break
            }
        }
        reopened.expectSessionResumeForTesting(session)
        reopened.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        await flush(64)
        XCTAssertEqual(reopened.model.activeTurnToken?.clientTurnId, a.clientTurnId)
        reopened.applyForTesting(.turnEventReplay(sessionId: session, turnId: a.clientTurnId, sequence: 1,
            eventJson: #"{"type":"turn_ended","outcome":{"type":"end_turn"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#))
        reopened.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, a.clientTurnId, 3, .completed)))
        reopened.applyForTesting(.turnRecoveryState(snapshot: snapshot(session, b, 1, .running)))
        reopened.applyForTesting(.turnEventReplay(sessionId: session, turnId: b, sequence: 1,
            eventJson: "{\"type\":\"turn_started\",\"turn_id\":\(b)}"))
        XCTAssertEqual(reopened.model.activeTurnToken?.clientTurnId, b)
        XCTAssertTrue(reopened.model.streaming)
        XCTAssertTrue(try reopened.pendingSubmissionsForTesting().isEmpty)
        reopened.retireRecoveryIngress()
    }

    func testTerminalEvidenceRequiresExplicitReviewOfExactVersion() throws {
        let source = makeSource(root: root())
        let session = "evidence-\(UUID().uuidString)"
        source.beginTurnForTesting(turnId: 71, sessionId: session)
        source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
        let event = ClientEvent.turnEventLiveUnretained(sessionId: session, turnId: 71,
            eventJson: #"{"type":"text_delta","text":"unretained"}"#)
        source.applyForTesting(event)
        let displayed = source.model.recoveryEvidenceForReview
        XCTAssertEqual(displayed.count, 1)
        source.dismissError()
        XCTAssertEqual(try source.terminalEvidenceForTesting(sessionID: session).count, 1)
        source.applyForTesting(event)
        XCTAssertNotEqual(source.model.recoveryEvidenceForReview.first?.version, displayed.first?.version)
        source.acknowledgeRecoveryEvidence(displayed)
        XCTAssertEqual(try source.terminalEvidenceForTesting(sessionID: session).count, 1)
        source.acknowledgeRecoveryEvidence(source.model.recoveryEvidenceForReview)
        XCTAssertTrue(try source.terminalEvidenceForTesting(sessionID: session).isEmpty)
        source.retireRecoveryIngress()
    }

    func testTerminalEvidenceSyncFailureKeepsRecoveryGateUntilRetry() {
        let source = makeSource(root: root())
        let session = "evidence-sync-\(UUID().uuidString)"
        source.beginTurnForTesting(turnId: 71, sessionId: session)
        source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
        source.failNextTerminalEvidenceSyncForTesting()
        let terminal = snapshot(session, 71, 1, .completed, uncertain: true)
        source.applyForTesting(.turnRecoveryState(snapshot: terminal))
        XCTAssertNotNil(source.durableAttachCursorForTesting())
        XCTAssertTrue(source.model.hasUnresolvedTurnRecovery)
        XCTAssertTrue(source.model.hasInactiveDurableRecovery)
        source.startNewConversation()
        XCTAssertNotNil(source.durableAttachCursorForTesting())
        source.applyForTesting(.turnRecoveryState(snapshot: terminal))
        XCTAssertNil(source.durableAttachCursorForTesting())
        XCTAssertFalse(source.model.hasUnresolvedTurnRecovery)
        source.retireRecoveryIngress()
    }

    func testCrashAfterTerminalCallbackBeforeSnapshotKeepsIdentity() async throws {
        let path = root()
        let session = "terminal-crash-\(UUID().uuidString)"
        let first = makeSource(root: path)
        first.beginTurnForTesting(turnId: 71, sessionId: session)
        first.beginDurableTurnForTesting(turnId: 71, sessionId: session)
        first.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
            eventJson: #"{"type":"turn_ended","outcome":{"type":"end_turn"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#))
        XCTAssertNotNil(first.durableAttachCursorForTesting())
        let oldListener = first.makeEventListenerForTesting()
        oldListener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(session, 71, 1, .completed, uncertain: true)))
        first.retireRecoveryIngress()

        let reopened = makeSource(root: path)
        reopened.setCommandSubmitterForTesting { command in
            switch command {
            case let .attachTurn(owner, turn, _, request):
                reopened.applyForTesting(.turnAttachStarted(requestId: request,
                    snapshot: self.snapshot(owner, turn, 2, .completed, uncertain: true)))
                reopened.applyForTesting(.turnAttachFinished(requestId: request, sessionId: owner, turnId: turn))
            case let .resumeTurn(owner, turn):
                reopened.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(owner, turn, 2, .completed, uncertain: true)))
            default: break
            }
        }
        reopened.expectSessionResumeForTesting(session)
        reopened.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        await oldListener.waitUntilIdle()
        await flush(64)
        XCTAssertEqual(try reopened.terminalEvidenceForTesting(sessionID: session).map(\.turnID), [71])
        XCTAssertNil(reopened.durableAttachCursorForTesting())
        reopened.retireRecoveryIngress()
        let third = makeSource(root: path)
        third.expectSessionResumeForTesting(session)
        third.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        XCTAssertEqual(third.model.recoveryEvidenceForReview.map(\.turnID), [71])
        XCTAssertNotNil(third.model.error)
        third.retireRecoveryIngress()
    }

    func testSlashResultPersistsEvidenceBeforeReleasingIdentity() async throws {
        let path = root()
        let session = "slash-receipt-\(UUID().uuidString)"
        let first = makeSource(root: path)
        first.model.activeSessionId = session
        first.model.slashCommandsLoaded = true
        first.model.slashCommands = [ConversationSlashCommand(name: "help", description: "help", aliases: [],
            argumentHint: nil, menuDescription: nil, source: "builtin", hidden: false)]
        var release: CheckedContinuation<Void, Never>?
        first.setCommandSubmitterForTesting { command in
            if case .runSlashCommand = command { await withCheckedContinuation { release = $0 } }
        }
        guard let token = first.send("/help") else { return XCTFail("expected slash") }
        await flush(48)
        first.applyForTesting(.slashCommandResult(turnId: token.clientTurnId, display: "help output".clientTextDto, isError: false))
        XCTAssertTrue(try first.pendingSubmissionsForTesting().isEmpty)
        XCTAssertEqual(try first.terminalEvidenceForTesting(sessionID: session).map(\.turnID), [token.clientTurnId])
        release?.resume()
        release = nil
        await flush()
        first.retireRecoveryIngress()
        let reopened = makeSource(root: path)
        reopened.expectSessionResumeForTesting(session)
        reopened.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        XCTAssertEqual(reopened.model.recoveryEvidenceForReview.map(\.turnID), [token.clientTurnId])
        XCTAssertNotNil(reopened.model.error)
        reopened.retireRecoveryIngress()
    }

    func testSlashEvidenceWriteFailureKeepsLedgerUntilForegroundRetry() async throws {
        let source = makeSource(root: root())
        let session = "slash-evidence-failure-\(UUID().uuidString)"
        source.model.activeSessionId = session
        source.model.slashCommandsLoaded = true
        source.model.slashCommands = [ConversationSlashCommand(name: "help", description: "help", aliases: [],
            argumentHint: nil, menuDescription: nil, source: "builtin", hidden: false)]
        var release: CheckedContinuation<Void, Never>?
        source.setCommandSubmitterForTesting { command in
            if case .runSlashCommand = command { await withCheckedContinuation { release = $0 } }
        }
        guard let token = source.send("/help") else { return XCTFail("expected slash") }
        await flush(48)
        source.failNextTerminalEvidenceSyncForTesting()
        source.applyForTesting(.slashCommandResult(turnId: token.clientTurnId, display: "result".clientTextDto, isError: false))
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [token.clientTurnId])
        XCTAssertTrue(source.model.hasUnresolvedTurnRecovery)
        release?.resume()
        release = nil
        await flush(48)
        source.handleForeground()
        await flush(48)
        XCTAssertTrue(try source.pendingSubmissionsForTesting().isEmpty)
        XCTAssertFalse(source.model.hasUnresolvedTurnRecovery)
        XCTAssertEqual(try source.terminalEvidenceForTesting(sessionID: session).map(\.turnID), [token.clientTurnId])
        source.retireRecoveryIngress()
    }

    func testSlashCallbackNotYetConsumedRetainsLedgerAcrossReplacement() async throws {
        let path = root()
        let session = "slash-callback-\(UUID().uuidString)"
        let first = makeSource(root: path)
        first.model.activeSessionId = session
        first.model.slashCommandsLoaded = true
        first.model.slashCommands = [ConversationSlashCommand(name: "help", description: "help", aliases: [],
            argumentHint: nil, menuDescription: nil, source: "builtin", hidden: false)]
        var release: CheckedContinuation<Void, Never>?
        first.setCommandSubmitterForTesting { command in
            if case .runSlashCommand = command { await withCheckedContinuation { release = $0 } }
        }
        guard let token = first.send("/help") else { return XCTFail("expected slash") }
        await flush(48)
        let listener = first.makeEventListenerForTesting()
        listener.enqueueForTesting(.slashCommandResult(turnId: token.clientTurnId, display: "queued result".clientTextDto, isError: false))
        first.retireRecoveryIngress()
        await listener.waitUntilIdle()

        let reopened = makeSource(root: path)
        var releaseProbe: CheckedContinuation<Void, Never>?
        reopened.setCommandSubmitterForTesting { command in
            if case .attachTurn = command {
                await withCheckedContinuation { releaseProbe = $0 }
                throw ClientError.NotFound(message: "no durable local slash")
            }
        }
        reopened.expectSessionResumeForTesting(session)
        reopened.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
        await flush(64)
        XCTAssertNotNil(releaseProbe)
        XCTAssertEqual(reopened.durableAttachCursorForTesting(), 0, "the new source has already created the shared checkpoint")
        release?.resume()
        release = nil
        await flush(48)
        XCTAssertEqual(try reopened.pendingSubmissionsForTesting().map(\.turnID), [token.clientTurnId])
        releaseProbe?.resume()
        releaseProbe = nil
        await flush(48)
        XCTAssertTrue(reopened.model.hasUnresolvedTurnRecovery)
        XCTAssertNotNil(reopened.model.error)
        reopened.retireRecoveryIngress()
    }

    func testSharedPostSubmitFenceRejectsRetiredMainSource() throws {
        let source = makeSource(root: root())
        XCTAssertNoThrow(try source.checkCommandSubmissionAfterAwaitForTesting())
        source.retireRecoveryIngress()
        XCTAssertThrowsError(try source.checkCommandSubmissionAfterAwaitForTesting()) { error in
            XCTAssertTrue(error is CancellationError)
        }
    }

    func testStopLateLiveOutputRetainsEvidenceAndAllowsQueuedTurnToAdopt() async throws {
        let source = makeSource(root: root())
        let session = "stop-live-queue-\(UUID().uuidString)"
        source.model.activeSessionId = session
        var turns: [UInt64] = []
        var release: CheckedContinuation<Void, Never>?
        source.setCommandSubmitterForTesting { command in
            switch command {
            case let .sendPrompt(_, _, _, _, turn?, _): turns.append(turn)
            case let .cancel(owner?, turn?):
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(owner, turn, 1, .cancelled)))
                await withCheckedContinuation { release = $0 }
            default: break
            }
        }
        guard let first = source.send("A") else { return XCTFail("expected A") }
        await flush()
        XCTAssertNotNil(source.send("!B"))
        await flush(48)
        guard turns.count == 2 else { return XCTFail("expected B") }
        let second = turns[1]
        let stop = Task { try await source.cancelAndWait() }
        await flush(48)
        XCTAssertNotNil(release)
        source.applyForTesting(.turnEventLiveUnretained(sessionId: session, turnId: first.clientTurnId,
            eventJson: #"{"type":"tool_use_result","id":"block-1","tool":"Read","result_json":"{}","is_error":false,"display":{"body_lines":0}}"#))
        source.applyForTesting(.turnEventReplay(sessionId: session, turnId: second, sequence: 1,
            eventJson: "{\"type\":\"turn_started\",\"turn_id\":\(second)}"))
        release?.resume()
        release = nil
        try await stop.value
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, second)
        XCTAssertTrue(source.model.streaming)
        XCTAssertEqual(try source.terminalEvidenceForTesting(sessionID: session).first?.turnID, first.clientTurnId)
        XCTAssertEqual(try source.terminalEvidenceForTesting(sessionID: session).first?.liveOnlyOutput, true)
        source.retireRecoveryIngress()
    }

    func testLateSlashResultSettlesUnknownWithoutProbeResurrection() async throws {
        let source = makeSource(root: root())
        let session = "slash-unknown-\(UUID().uuidString)"
        source.model.activeSessionId = session
        source.model.slashCommandsLoaded = true
        source.model.slashCommands = [ConversationSlashCommand(
            name: "skill", description: "skill", aliases: [], argumentHint: nil,
            menuDescription: nil, source: "builtin", hidden: false
        )]
        var releaseProbe: CheckedContinuation<Void, Never>?
        var resumes = 0
        source.setCommandSubmitterForTesting { command in
            switch command {
            case .runSlashCommand: throw ClientError.Transport(message: "response lost")
            case .attachTurn:
                await withCheckedContinuation { releaseProbe = $0 }
                throw ClientError.NotFound(message: "no checkpoint")
            case .resumeTurn: resumes += 1
            default: break
            }
        }
        guard let token = source.send("/skill") else { return XCTFail("expected slash") }
        await flush(64)
        XCTAssertEqual(try source.pendingSubmissionsForTesting().map(\.turnID), [token.clientTurnId])
        XCTAssertNotNil(releaseProbe)
        source.applyForTesting(.slashCommandResult(turnId: token.clientTurnId, display: "done".clientTextDto, isError: false))
        releaseProbe?.resume()
        releaseProbe = nil
        await flush(64)
        XCTAssertTrue(try source.pendingSubmissionsForTesting().isEmpty)
        XCTAssertNil(source.durableAttachCursorForTesting())
        XCTAssertFalse(source.model.hasUnresolvedTurnRecovery)
        XCTAssertFalse(source.model.slashCommandPending)
        XCTAssertEqual(resumes, 0)
        source.retireRecoveryIngress()
    }

    func testSessionSwitchWaitsForPendingSlashCancelAcknowledgement() async {
        let source = makeSource(root: root())
        let session = "slash-stop-\(UUID().uuidString)"
        source.model.activeSessionId = session
        source.model.slashCommandsLoaded = true
        source.model.slashCommands = [ConversationSlashCommand(
            name: "skill", description: "skill", aliases: [], argumentHint: nil,
            menuDescription: nil, source: "builtin", hidden: false
        )]
        var release: CheckedContinuation<Void, Never>?
        var cancelled: [(String?, UInt64?)] = []
        var newSessions = 0
        source.setCommandSubmitterForTesting { command in
            switch command {
            case .runSlashCommand:
                await withCheckedContinuation { release = $0 }
            case let .cancel(owner, turn):
                cancelled.append((owner, turn))
                if let owner, let turn {
                    source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(owner, turn, 1, .cancelled)))
                }
            case .newSession: newSessions += 1
            default: break
            }
        }
        guard let token = source.send("/skill") else { return XCTFail("expected slash") }
        await flush()
        source.startNewConversation()
        await flush()
        XCTAssertTrue(source.model.slashCommandPending)
        XCTAssertTrue(cancelled.isEmpty)
        XCTAssertEqual(newSessions, 0)
        release?.resume()
        release = nil
        await flush(64)
        XCTAssertEqual(cancelled.count, 1)
        XCTAssertEqual(cancelled.first?.0, session)
        XCTAssertEqual(cancelled.first?.1, token.clientTurnId)
        XCTAssertFalse(source.model.slashCommandPending)
        XCTAssertEqual(newSessions, 1)
        source.retireRecoveryIngress()
    }

    func testTurnIdentifierSkipsLiveInterestAndFailsClosedOnOverflow() async {
        let source = makeSource(root: root())
        source.model.activeSessionId = "turn-id-session"
        var submitted: [UInt64] = []
        var releases: [CheckedContinuation<Void, Never>] = []
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(_, _, _, _, turn?, _) = command {
                submitted.append(turn)
                await withCheckedContinuation { releases.append($0) }
            }
        }
        source.setNextTurnIDForTesting(41)
        XCTAssertNotNil(source.send("first"))
        await flush()
        source.setNextTurnIDForTesting(41)
        XCTAssertNotNil(source.send("queued"))
        await flush()
        XCTAssertEqual(submitted, [41, 42])
        source.setNextTurnIDForTesting(UInt64.max)
        XCTAssertNotNil(source.send("last possible id"))
        await flush()
        let count = source.model.messages.count
        XCTAssertNil(source.send("must not wrap"))
        XCTAssertEqual(source.model.messages.count, count)
        XCTAssertEqual(submitted.last, UInt64.max)
        for release in releases { release.resume() }
        await flush()
        source.retireRecoveryIngress()
    }
}
#endif
