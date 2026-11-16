import Foundation
import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

@MainActor
final class ConversationQueuedOwnershipTests: XCTestCase {
    private let session = "11111111-1111-4111-8111-111111111111"
    private let other = "22222222-2222-4222-8222-222222222222"
    private let zeroCost = CostDto(totalUsd: 0, inputTokens: 0, outputTokens: 0, apiCalls: 0,
                                  sessionDurationSecs: 0, formatted: "$0.00", byAccount: [])

    private func root() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("queued-owner-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        return root
    }

    private func source(_ root: URL, ledger: DurableConversationTurnLedger = .init()) -> EngineConversationSource {
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "", appSandboxRoot: root.path,
            sessionMode: .code, visionDelegationEnabled: false), recoveryLedger: ledger)
        source.model.activeSessionId = session
        source.setCommandSubmitterForTesting { _ in }
        return source
    }

    private func url(_ root: URL) -> URL { root.appendingPathComponent("durable-conversation-turns.json") }
    private func snapshot(_ id: UInt64, _ state: TurnRecoveryStateDto) -> TurnRecoverySnapshotDto {
        .init(sessionId: session, turnId: id, state: state, firstSequence: 0, lastSequence: 7,
              safeToResume: state == .pausedRecoverable || state == .running, reason: nil)
    }
    private func fact(_ id: UInt64, queued: Bool = false, state: TurnRecoveryStateDto? = nil,
                      settled: Bool = false, active: Bool = false, owner: UInt64? = nil,
                      admitted: Bool = true) -> MobileTurnAdmissionDto {
        .init(turnId: id, admitted: admitted, queued: queued, settled: settled, executorActive: active,
              recovery: state.map { snapshot(owner ?? id, $0) })
    }
    private func flush() async { for _ in 0..<12 { await Task.yield() } }
    private func end(_ source: EngineConversationSource) {
        source.applyForTesting(.turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))
    }
    private func coldQueue(_ ledger: DurableConversationTurnLedger, _ root: URL,
                           active: Bool = false, queued: [UInt64] = [12]) async throws {
        try await ledger.begin(url: url(root), sessionID: session, turnID: 11, scope: root.path)
        for id in queued { try await ledger.enqueue(url: url(root), sessionID: session, ownerTurnID: 11, turnID: id) }
        if !active { try await ledger.clear(url: url(root), sessionID: session, turnID: 11) }
    }
    private func resume(_ source: EngineConversationSource, _ listener: EngineListener) {
        source.expectSessionResumeForTesting(session)
        listener.enqueueForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
    }

    func testDefinitiveBusyRejectionRetiresIDAndRestoresExactUnsentInput() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger(), source = source(root, ledger: ledger)
        var ids: [UInt64] = []
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(text, _, _, id?, _) = command {
                ids.append(id)
                if text.displayText == "queued 🌙" { throw ClientError.Rejected(message: "turn_cancelled_before_admission") }
            }
        }
        let sent = await source.send("first")
        let first = try XCTUnwrap(sent)
        await flush()
        source.applyForTesting(.turnStarted(turnId: first.clientTurnId))
        let images = [ImageRefDto(mediaType: "image/png", base64: "cA==")]
        _ = await source.send("queued 🌙", sourceCodeUnits: Array("queued 🌙".utf16), images: images,
                              selectedSlashCommand: nil)
        await flush()
        await source.waitForDurableCleanupForTesting()
        XCTAssertEqual(source.model.activeTurnToken, first)
        XCTAssertFalse(source.model.messages.contains { $0.text == "queued 🌙" })
        let unsent = try XCTUnwrap(source.model.unsentPrompts.last)
        XCTAssertEqual(unsent.text, "queued 🌙")
        XCTAssertEqual(unsent.sourceCodeUnits, Array("queued 🌙".utf16))
        XCTAssertEqual(unsent.images, images)
        let record = try await ledger.state(url: url(root), sessionID: session)
        XCTAssertEqual(record?.queuedTurnIDs, [])
        end(source)
        await source.waitForDurableCleanupForTesting()
        let continued = await source.send("continue")
        XCTAssertNotNil(continued)
        XCTAssertEqual(ids.count, 3)
    }

    func testLateDefinitiveRejectionAfterPredecessorTerminalReleasesIdleFence() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger(), source = source(root, ledger: ledger)
        let entered = expectation(description: "queued native submission is in flight")
        var release: CheckedContinuation<Void, Never>?
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(text, _, _, _, _) = command, text.displayText == "late rejected" {
                await withCheckedContinuation { release = $0; entered.fulfill() }
                throw ClientError.Rejected(message: "turn_cancelled_before_admission")
            }
        }
        _ = await source.send("first")
        await flush()
        _ = await source.send("late rejected")
        await fulfillment(of: [entered], timeout: 2)
        end(source)
        await source.waitForDurableCleanupForTesting()
        let inactive = try await ledger.state(url: url(root), sessionID: session)
        XCTAssertFalse(try XCTUnwrap(inactive).isActive)
        XCTAssertEqual(inactive?.queuedTurnIDs.count, 1)
        release?.resume()
        await flush()
        await source.waitForDurableCleanupForTesting()
        let continued = await source.send("continue")
        XCTAssertNotNil(continued)
        XCTAssertEqual(source.model.unsentPrompts.filter { $0.text == "late rejected" }.count, 1)
    }

    func testAmbiguousFailureUsesNativeMissingOnlyAfterReceiptAndNeverResendsPrompt() async throws {
        let root = try root(), source = source(root)
        var promptIDs: [UInt64] = []
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(text, _, _, id?, _) = command {
                promptIDs.append(id)
                if text.displayText == "uncertain" { throw POSIXError(.EIO) }
            }
        }
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { self.fact($0, admitted: false) } }
        _ = await source.send("first")
        await flush()
        _ = await source.send("uncertain")
        await flush()
        end(source)
        await source.waitForDurableCleanupForTesting()
        await source.waitForDurableRecoveryForTesting()
        await source.waitForDurableCleanupForTesting()
        XCTAssertEqual(promptIDs.count, 2)
        XCTAssertEqual(source.model.unsentPrompts.filter { $0.text == "uncertain" }.count, 1)
        let continued = await source.send("continue")
        XCTAssertNotNil(continued)
    }

    func testLateAmbiguousFailureCannotRetireAnAlreadyClaimedSuccessor() async throws {
        let root = try root(), source = source(root)
        let entered = expectation(description: "successor receipt parked")
        var release: CheckedContinuation<Void, Never>?, queuedID: UInt64?
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(text, _, _, id?, _) = command, text.displayText == "queued" {
                queuedID = id
                await withCheckedContinuation { release = $0; entered.fulfill() }
                throw POSIXError(.EIO)
            }
        }
        _ = await source.send("first")
        await flush()
        _ = await source.send("queued")
        await fulfillment(of: [entered], timeout: 2)
        let id = try XCTUnwrap(queuedID), listener = source.makeEventListenerForTesting()
        end(source)
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(id, .running)))
        listener.enqueueForTesting(.turnStarted(turnId: id))
        listener.enqueueForTesting(.textDelta(text: "owned output".clientTextDto))
        await listener.waitUntilIdle()
        release?.resume()
        await flush()
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, id)
        XCTAssertTrue(source.model.streaming)
        XCTAssertEqual(source.model.messages.last?.text, "owned output")
        XCTAssertTrue(source.model.unsentPrompts.isEmpty)
        XCTAssertNil(source.model.error)
    }

    func testPausedPredecessorRejectsForeignClaimUntilNativeTerminalProof() async throws {
        let root = try root(), source = source(root)
        var ids: [UInt64] = [], predecessorTerminal = false
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(_, _, _, id?, _) = command { ids.append(id) }
            if case let .cancel(id?) = command {
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(id, .cancelled)))
            }
        }
        let sent = await source.send("first")
        let first = try XCTUnwrap(sent)
        await flush()
        _ = await source.send("!printf queued")
        await flush()
        let queued = try XCTUnwrap(ids.last), listener = source.makeEventListenerForTesting()
        source.setTurnAdmissionQueryForTesting { _, requested in
            requested.map { id in id == first.clientTurnId
                ? self.fact(id, state: predecessorTerminal ? .cancelled : .pausedRecoverable, settled: predecessorTerminal)
                : self.fact(id, state: .running, active: true) }
        }
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(first.clientTurnId, .pausedRecoverable)))
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(queued, .running)))
        listener.enqueueForTesting(.turnStarted(turnId: queued))
        listener.enqueueForTesting(.textDelta(text: "unverified output".clientTextDto))
        await listener.waitUntilIdle()
        XCTAssertEqual(source.model.activeTurnToken, first)
        XCTAssertTrue(source.model.hasInactiveDurableRecovery)
        XCTAssertFalse(source.model.messages.contains { $0.text == "unverified output" })
        predecessorTerminal = true
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(first.clientTurnId, .cancelled)))
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(queued, .running)))
        listener.enqueueForTesting(.turnStarted(turnId: queued))
        listener.enqueueForTesting(.textDelta(text: "verified output".clientTextDto))
        await listener.waitUntilIdle()
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, queued)
        XCTAssertEqual(source.model.messages.last?.text, "verified output")
        try await source.cancelAndWait()
        XCTAssertFalse(source.model.streaming)
    }

    func testColdInactiveOwnerRestoresClaimedSuccessorWithoutAnotherRunningEvent() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root)
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        var commands: [ClientCommand] = []
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { self.fact($0, state: .pausedRecoverable) } }
        source.setCommandSubmitterForTesting { command in
            commands.append(command)
            if case let .attachTurn(id, _) = command {
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(id, .pausedRecoverable)))
            }
            if case let .resumeTurn(id) = command {
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(id, .running)))
            }
        }
        resume(source, listener)
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 12)
        XCTAssertTrue(source.model.streaming)
        XCTAssertTrue(commands.contains { if case .attachTurn(12, _) = $0 { return true }; return false })
        XCTAssertTrue(commands.contains { if case .resumeTurn(12) = $0 { return true }; return false })
        XCTAssertFalse(commands.contains { if case .sendPrompt = $0 { return true }; return false })
    }

    func testColdSettledSuccessorProjectsItsPostSnapshotFIFOOutputBeforeRetiringID() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root)
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { self.fact($0, state: .completed, settled: true) } }
        resume(source, listener)
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(12, .running)))
        listener.enqueueForTesting(.turnStarted(turnId: 12))
        listener.enqueueForTesting(.textDelta(text: "completed during restore".clientTextDto))
        listener.enqueueForTesting(.turnEventReplay(sessionId: session, turnId: 12, sequence: 7,
            eventJson: "{\"type\":\"text_delta\",\"text\":\"completed during restore\"}"))
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(12, .completed)))
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        await source.waitForDurableCleanupForTesting()
        XCTAssertEqual(source.model.messages.filter { $0.text == "completed during restore" }.count, 1)
        XCTAssertNil(source.model.activeTurnToken)
        let remaining = try await ledger.state(url: url(root), sessionID: session)
        XCTAssertNil(remaining)
    }

    func testColdMissingPhantomIDRetiresWithoutNativePromptOrResume() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root)
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        var commands: [ClientCommand] = []
        source.setCommandSubmitterForTesting { commands.append($0) }
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { self.fact($0, admitted: false) } }
        resume(source, listener)
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        await source.waitForDurableCleanupForTesting()
        XCTAssertFalse(commands.contains { if case .sendPrompt = $0 { return true }; if case .resumeTurn = $0 { return true }; return false })
        let continued = await source.send("new input")
        XCTAssertNotNil(continued)
    }

    func testColdPrivateCurrentOwnerRepairsMarkerInsteadOfAttachingStateNone() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root, active: true, queued: [])
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        var commands: [ClientCommand] = []
        source.setCommandSubmitterForTesting { commands.append($0) }
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { self.fact($0, queued: true) } }
        resume(source, listener)
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        XCTAssertTrue(commands.contains { if case .resumeTurn(11) = $0 { return true }; return false })
        XCTAssertFalse(commands.contains { if case .attachTurn = $0 { return true }; if case .sendPrompt = $0 { return true }; return false })
        let state = try await ledger.state(url: url(root), sessionID: session)
        XCTAssertEqual(state?.queuedTurnIDs, [11])
        XCTAssertEqual(state?.isActive, false)
    }

    func testColdLiveSuccessorUsesNativeExecutorProofAndDoesNotResumeBusyOwner() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root)
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        var commands: [ClientCommand] = []
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { self.fact($0, state: .running, active: true) } }
        source.setCommandSubmitterForTesting { command in
            commands.append(command)
            if case let .attachTurn(id, _) = command {
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(id, .running)))
            }
            if case .resumeTurn = command { throw ClientError.Rejected(message: "a turn is already in flight") }
        }
        resume(source, listener)
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 12)
        XCTAssertTrue(source.model.streaming)
        XCTAssertFalse(source.model.hasInactiveDurableRecovery)
        XCTAssertFalse(commands.contains { if case .resumeTurn = $0 { return true }; return false })
        XCTAssertNil(source.model.error)
    }

    func testBusySendWaitsForDirectPredecessorNativeReceiptBeforeDispatch() async throws {
        let root = try root(), source = source(root)
        let entered = expectation(description: "direct native submission parked")
        var release: CheckedContinuation<Void, Never>?, promptCount = 0
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(text, _, _, _, _) = command {
                promptCount += 1
                if text.displayText == "first" {
                    await withCheckedContinuation { release = $0; entered.fulfill() }
                }
            }
        }
        let first = await source.send("first")
        await fulfillment(of: [entered], timeout: 2)
        let busy = Task { await source.send("second") }
        await flush()
        XCTAssertEqual(promptCount, 1)
        release?.resume()
        let queued = await busy.value
        await flush()
        XCTAssertEqual(queued, first)
        XCTAssertEqual(promptCount, 2)
    }

    func testLateFailureFromOldSessionCannotMutateNewSessionOrUnsentPrompts() async throws {
        let root = try root(), source = source(root)
        let entered = expectation(description: "old queued receipt parked")
        var release: CheckedContinuation<Void, Never>?
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(text, _, _, _, _) = command, text.displayText == "old queued" {
                await withCheckedContinuation { release = $0; entered.fulfill() }
                throw ClientError.Rejected(message: "turn_cancelled_before_admission")
            }
        }
        _ = await source.send("first")
        await flush()
        _ = await source.send("old queued")
        await fulfillment(of: [entered], timeout: 2)
        source.expectSessionResumeForTesting(other)
        source.applyForTesting(.sessionResumed(sessionId: other, mode: .code, messages: []))
        await source.waitForDurableRecoveryForTesting()
        release?.resume()
        await flush()
        XCTAssertEqual(source.model.activeSessionId, other)
        XCTAssertTrue(source.model.unsentPrompts.isEmpty)
        XCTAssertTrue(source.model.messages.isEmpty)
        XCTAssertNil(source.model.error)
    }

    func testStopWhileBusySendWaitsForPredecessorReceiptLeavesItsDraftUndispatched() async throws {
        let root = try root(), source = source(root)
        let entered = expectation(description: "predecessor native receipt parked")
        var release: CheckedContinuation<Void, Never>?, prompts: [String] = []
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(text, _, _, _, _) = command {
                prompts.append(text.displayText)
                await withCheckedContinuation { release = $0; entered.fulfill() }
            }
            if case let .cancel(id?) = command {
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(id, .cancelled)))
            }
        }
        _ = await source.send("first")
        await fulfillment(of: [entered], timeout: 2)
        let pending = Task { await source.send("keep this draft") }
        await flush()
        try await source.cancelAndWait()
        release?.resume()
        let token = await pending.value
        XCTAssertNil(token)
        XCTAssertEqual(prompts, ["first"])
        XCTAssertFalse(source.model.messages.contains { $0.text == "keep this draft" })
    }

    func testMidTurnAppendAndQueuedCancellationRetirePersistentIDsBeforeContinue() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger(), source = source(root, ledger: ledger)
        var ids: [UInt64] = []
        source.setCommandSubmitterForTesting { command in
            if case let .sendPrompt(_, _, _, id?, _) = command { ids.append(id) }
        }
        let first = await source.send("first")
        await flush()
        _ = await source.send("appended")
        _ = await source.send("cancelled queued")
        await flush()
        XCTAssertEqual(ids.count, 3)
        guard ids.count == 3 else { return }
        source.applyForTesting(.userTranscriptRowIdentity(rowToken: "client-turn:\(ids[1])", uuid: UUID().uuidString))
        source.applyForTesting(.turnRecoveryState(snapshot: snapshot(ids[2], .cancelled)))
        await source.waitForDurableCleanupForTesting()
        let live = try await ledger.state(url: url(root), sessionID: session)
        XCTAssertEqual(live?.queuedTurnIDs, [])
        XCTAssertEqual(source.model.activeTurnToken, first)
        end(source)
        await source.waitForDurableCleanupForTesting()
        let continued = await source.send("continue")
        XCTAssertNotNil(continued)
    }

    func testIdleActivationGatesLiveFIFOThroughLedgerBeginAndNextSuccessor() async throws {
        let root = try root(), gate = RecoveryWriteGate()
        gate.setEnabled(false)
        let ledger = DurableConversationTurnLedger(writer: RecoveryGateWriter(gate: gate))
        try await coldQueue(ledger, root, queued: [12, 13])
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        var queryCount = 0
        source.setTurnAdmissionQueryForTesting { _, ids in
            queryCount += 1
            if queryCount == 2 { gate.setEnabled(true) }
            return ids.map { id in id == 12 ? self.fact(id, state: .running, active: queryCount > 1) : self.fact(id, queued: true) }
        }
        resume(source, listener)
        await listener.waitUntilIdle()
        guard await gate.waitForEntry() else { gate.release(); return XCTFail("successor begin did not reach the ledger gate") }
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(12, .running)))
        listener.enqueueForTesting(.turnStarted(turnId: 12))
        listener.enqueueForTesting(.textDelta(text: "first successor".clientTextDto))
        listener.enqueueForTesting(.turnEventReplay(sessionId: session, turnId: 12, sequence: 7,
            eventJson: "{\"type\":\"text_delta\",\"text\":\"first successor\"}"))
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(12, .completed)))
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(13, .running)))
        listener.enqueueForTesting(.turnStarted(turnId: 13))
        listener.enqueueForTesting(.textDelta(text: "second successor".clientTextDto))
        listener.enqueueForTesting(.turnEventReplay(sessionId: session, turnId: 13, sequence: 9,
            eventJson: "{\"type\":\"text_delta\",\"text\":\"second successor\"}"))
        await flush()
        XCTAssertTrue(source.model.messages.isEmpty, "raw output must remain behind the pending ledger claim")
        gate.setEnabled(false)
        gate.release()
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 13)
        XCTAssertEqual(source.model.messages.map(\.text), ["first successor", "second successor"])
        let cursor = try await source.durableAttachCursorForTesting()
        XCTAssertEqual(cursor, 9)
    }

    func testColdLiveExecutorRawAndAcknowledgementEnvelopeProjectOnlyOnce() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root)
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        var attachCount = 0
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { self.fact($0, state: .running, active: true) } }
        source.setCommandSubmitterForTesting { command in if case .attachTurn = command { attachCount += 1 } }
        resume(source, listener)
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        listener.enqueueForTesting(.textDelta(text: "live suffix".clientTextDto))
        listener.enqueueForTesting(.turnEventReplay(sessionId: session, turnId: 12, sequence: 1,
            eventJson: "{\"type\":\"text_delta\",\"text\":\"live suffix\"}"))
        await listener.waitUntilIdle()
        XCTAssertEqual(attachCount, 0, "an authoritative live owner uses the SessionResumed snapshot and live FIFO")
        XCTAssertEqual(source.model.messages.map(\.text), ["live suffix"])
        let cursor = try await source.durableAttachCursorForTesting()
        XCTAssertEqual(cursor, 1)
    }

    func testColdCoalescedMemberNeverAdoptsItsDifferentExecutorOwner() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root, queued: [13])
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map {
            self.fact($0, state: .completed, settled: true, owner: 12)
        } }
        resume(source, listener)
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        await source.waitForDurableCleanupForTesting()
        XCTAssertNil(source.model.activeTurnToken)
        XCTAssertFalse(source.model.streaming)
        let continued = await source.send("continue")
        XCTAssertNotNil(continued)
    }

    func testColdPrivatePredecessorSurvivesLiveSuccessorThenResumesItsExactID() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root, active: true)
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        var commands: [ClientCommand] = []
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { id in
            id == 11 ? self.fact(id, queued: true) : self.fact(id, state: .running, active: true)
        } }
        source.setCommandSubmitterForTesting { command in
            commands.append(command)
            if case .resumeTurn(11) = command {
                listener.enqueueForTesting(.turnRecoveryState(snapshot: self.snapshot(11, .running)))
                listener.enqueueForTesting(.turnStarted(turnId: 11))
                listener.enqueueForTesting(.textDelta(text: "original input resumed".clientTextDto))
            }
        }
        resume(source, listener)
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        let successor = try await ledger.state(url: url(root), sessionID: session)
        XCTAssertEqual(successor?.turnID, 12)
        XCTAssertEqual(successor?.queuedTurnIDs, [11])
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 12)
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(12, .completed)))
        await listener.waitUntilIdle()
        await source.waitForDurableCleanupForTesting()
        await source.waitForDurableRecoveryForTesting()
        await listener.waitUntilIdle()
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 11)
        XCTAssertEqual(source.model.messages.map(\.text), ["original input resumed"])
        XCTAssertTrue(commands.contains { if case .resumeTurn(11) = $0 { return true }; return false })
        XCTAssertFalse(commands.contains { if case .sendPrompt = $0 { return true }; return false })
    }

    func testColdRetainedCoalescedMemberProvesLiveOwnerAndStopTargetsThatOwner() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root, queued: [13])
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        var cancelled = false, commands: [ClientCommand] = []
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { id in
            self.fact(id, state: cancelled ? .cancelled : .running, settled: cancelled, active: !cancelled, owner: 12)
        } }
        source.setCommandSubmitterForTesting { command in
            commands.append(command)
            if case .cancel(12?) = command {
                cancelled = true
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(12, .cancelled)))
            }
        }
        resume(source, listener)
        // Running B can predate SessionResumed's transcript snapshot. The
        // trusted member proof must establish B before its raw suffix drains.
        listener.enqueueForTesting(.textDelta(text: "coalesced owner suffix".clientTextDto))
        listener.enqueueForTesting(.turnEventReplay(sessionId: session, turnId: 12, sequence: 7,
            eventJson: "{\"type\":\"text_delta\",\"text\":\"coalesced owner suffix\"}"))
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 12)
        XCTAssertEqual(source.model.messages.map(\.text), ["coalesced owner suffix"])
        let live = try await ledger.state(url: url(root), sessionID: session)
        XCTAssertEqual(live?.turnID, 12)
        XCTAssertEqual(live?.queuedTurnIDs, [13])
        try await source.cancelAndWait()
        await source.waitForDurableCleanupForTesting()
        await source.waitForDurableRecoveryForTesting()
        await source.waitForDurableCleanupForTesting()
        XCTAssertTrue(commands.contains { if case .cancel(12?) = $0 { return true }; return false })
        XCTAssertFalse(commands.contains { if case .sendPrompt = $0 { return true }; return false })
        let remaining = try await ledger.state(url: url(root), sessionID: session)
        XCTAssertNil(remaining)
    }

    func testColdCoalescedStaleRunningOwnerAttachesAndResumesActualOwner() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root, queued: [13])
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        var commands: [ClientCommand] = []
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { self.fact($0, state: .running, owner: 12) } }
        source.setCommandSubmitterForTesting { command in
            commands.append(command)
            if case .attachTurn(12, _) = command {
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(12, .pausedRecoverable)))
            }
            if case .resumeTurn(12) = command {
                source.applyForTesting(.turnRecoveryState(snapshot: self.snapshot(12, .running)))
            }
        }
        resume(source, listener)
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 12)
        XCTAssertTrue(source.model.streaming)
        XCTAssertTrue(commands.contains { if case .attachTurn(12, _) = $0 { return true }; return false })
        XCTAssertTrue(commands.contains { if case .resumeTurn(12) = $0 { return true }; return false })
        XCTAssertFalse(commands.contains { if case .attachTurn(13, _) = $0 { return true }; if case .resumeTurn(13) = $0 { return true }; return false })
    }

    func testPrivateCoalescedMemberLaterProvesUnknownRunningOwnerBeforeRawOutput() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root, queued: [13])
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        var claimed = false
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { id in
            claimed ? self.fact(id, state: .running, active: true, owner: 12) : self.fact(id, queued: true)
        } }
        resume(source, listener)
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        claimed = true
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(99, .running)))
        listener.enqueueForTesting(.textDelta(text: "unrelated output".clientTextDto))
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(12, .running)))
        listener.enqueueForTesting(.turnStarted(turnId: 12))
        listener.enqueueForTesting(.textDelta(text: "actual owner output".clientTextDto))
        listener.enqueueForTesting(.turnEventReplay(sessionId: session, turnId: 12, sequence: 7,
            eventJson: "{\"type\":\"text_delta\",\"text\":\"actual owner output\"}"))
        await listener.waitUntilIdle()
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 12)
        XCTAssertEqual(source.model.messages.map(\.text), ["actual owner output"])
        let cursor = try await source.durableAttachCursorForTesting()
        XCTAssertEqual(cursor, 7)
    }

    func testColdKnownActiveOwnerTerminalDuringQueryKeepsItsPostSnapshotSuffix() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root, active: true, queued: [])
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { self.fact($0, state: .completed, settled: true) } }
        resume(source, listener)
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(11, .running)))
        listener.enqueueForTesting(.turnStarted(turnId: 11))
        listener.enqueueForTesting(.textDelta(text: "original finished during restore".clientTextDto))
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(11, .completed)))
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        await source.waitForDurableCleanupForTesting()
        XCTAssertEqual(source.model.messages.map(\.text), ["original finished during restore"])
        XCTAssertNil(source.model.activeTurnToken)
        let remaining = try await ledger.state(url: url(root), sessionID: session)
        XCTAssertNil(remaining)
    }

    func testColdTerminalPredecessorPrefixDrainsBeforeAlreadyLiveSuccessor() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root, active: true)
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { id in
            id == 11 ? self.fact(id, state: .completed, settled: true) : self.fact(id, state: .running, active: true)
        } }
        resume(source, listener)
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(11, .running)))
        listener.enqueueForTesting(.turnStarted(turnId: 11))
        listener.enqueueForTesting(.textDelta(text: "predecessor prefix".clientTextDto))
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(11, .completed)))
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(12, .running)))
        listener.enqueueForTesting(.turnStarted(turnId: 12))
        listener.enqueueForTesting(.textDelta(text: "successor suffix".clientTextDto))
        listener.enqueueForTesting(.turnEventReplay(sessionId: session, turnId: 12, sequence: 9,
            eventJson: "{\"type\":\"text_delta\",\"text\":\"successor suffix\"}"))
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        XCTAssertEqual(source.model.messages.map(\.text), ["predecessor prefix", "successor suffix"])
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 12)
        let cursor = try await source.durableAttachCursorForTesting()
        XCTAssertEqual(cursor, 9)
    }

    func testColdKnownActiveOwnerRunningFactWithReleasedSlotStillDrainsItsPrefix() async throws {
        let root = try root(), ledger = DurableConversationTurnLedger()
        try await coldQueue(ledger, root, active: true, queued: [])
        let source = source(root, ledger: ledger), listener = source.makeEventListenerForTesting()
        // The metadata snapshot can precede completion while the query's
        // active-slot snapshot observes its subsequent release.
        source.setTurnAdmissionQueryForTesting { _, ids in ids.map { self.fact($0, state: .running) } }
        resume(source, listener)
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(11, .running)))
        listener.enqueueForTesting(.turnStarted(turnId: 11))
        listener.enqueueForTesting(.textDelta(text: "prefix before slot release".clientTextDto))
        listener.enqueueForTesting(.turnRecoveryState(snapshot: snapshot(11, .completed)))
        await listener.waitUntilIdle()
        await source.waitForDurableRecoveryForTesting()
        await source.waitForDurableCleanupForTesting()
        XCTAssertEqual(source.model.messages.map(\.text), ["prefix before slot release"])
        XCTAssertNil(source.model.activeTurnToken)
        let remaining = try await ledger.state(url: url(root), sessionID: session)
        XCTAssertNil(remaining)
    }
}
#endif
