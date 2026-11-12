import Combine
import Foundation
import SwiftUI
import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI
import UIKit

#if DEBUG
private final class ExpiredSideTestHandle: MobileEngineHandle, @unchecked Sendable {
    required init(unsafeFromHandle handle: UInt64) { super.init(unsafeFromHandle: handle) }
    init() { super.init(noHandle: .init()) }
}
#endif

/// The lock makes the injected failure switch safe when the ledger actor writes off-main.
final class RecoveryWriterFailure: @unchecked Sendable {
    enum Mode: Equatable { case none, beforeLedger, afterLedger, beforeWitness }
    private let lock = NSLock()
    private var value: Mode = .none
    func set(_ next: Mode) { lock.lock(); defer { lock.unlock() }; value = next }
    func mode() -> Mode { lock.lock(); defer { lock.unlock() }; return value }
}

struct RecoveryFailureWriter: ProjectAtomicWriting {
    let failure: RecoveryWriterFailure
    func write<T: Codable>(_ value: T, to url: URL, encoder: JSONEncoder, decoder: JSONDecoder) throws {
        try DefaultProjectAtomicWriter().write(value, to: url, encoder: encoder, decoder: decoder)
    }
    func writeData(_ data: Data, to url: URL, validate: (Data) throws -> Void) throws {
        try DefaultProjectAtomicWriter().writeData(data, to: url, validate: validate)
    }
    func writeData(_ data: Data, to url: URL, expected: OwnerCatalogExpectation, validate: (Data) throws -> Void,
                   commit: (_ replace: () throws -> Void) throws -> Void) throws {
        let isLedger = url.lastPathComponent == "durable-conversation-turns.json"
        let isWitness = url.lastPathComponent == "durable-conversation-turns.witness"
        if isLedger, failure.mode() == .beforeLedger { throw POSIXError(.EACCES) }
        if isWitness, failure.mode() == .beforeWitness { throw POSIXError(.EIO) }
        try DefaultProjectAtomicWriter().writeData(data, to: url, expected: expected, validate: validate, commit: commit)
        if isLedger, failure.mode() == .afterLedger { throw POSIXError(.EIO) }
    }
}

/// The condition only blocks the ledger actor's writer, never the main actor.
final class RecoveryWriteGate: @unchecked Sendable {
    private let condition = NSCondition()
    private var entered = false
    private var released = false
    private var enabled = true
    func setEnabled(_ next: Bool) {
        condition.lock(); defer { condition.unlock() }
        enabled = next
    }
    func block() {
        condition.lock(); defer { condition.unlock() }
        guard enabled else { return }
        entered = true
        condition.broadcast()
        while !released { condition.wait() }
    }
    private func waitForEntryBlocking() -> Bool {
        condition.lock(); defer { condition.unlock() }
        let deadline = Date(timeIntervalSinceNow: 5)
        while !entered && condition.wait(until: deadline) { }
        return entered
    }
    func waitForEntry() async -> Bool {
        await Task.detached { [self] in waitForEntryBlocking() }.value
    }
    func release() {
        condition.lock(); defer { condition.unlock() }
        released = true
        condition.broadcast()
    }
}

struct RecoveryGateWriter: ProjectAtomicWriting {
    let gate: RecoveryWriteGate
    func write<T: Codable>(_ value: T, to url: URL, encoder: JSONEncoder, decoder: JSONDecoder) throws {
        try DefaultProjectAtomicWriter().write(value, to: url, encoder: encoder, decoder: decoder)
    }
    func writeData(_ data: Data, to url: URL, validate: (Data) throws -> Void) throws {
        try DefaultProjectAtomicWriter().writeData(data, to: url, validate: validate)
    }
    func writeData(_ data: Data, to url: URL, expected: OwnerCatalogExpectation, validate: (Data) throws -> Void,
                   commit: (_ replace: () throws -> Void) throws -> Void) throws {
        if url.lastPathComponent == "durable-conversation-turns.json" { gate.block() }
        try DefaultProjectAtomicWriter().writeData(data, to: url, expected: expected, validate: validate, commit: commit)
    }
}

@MainActor
final class ConversationRecoveryTests: XCTestCase {
    private let a = "11111111-1111-4111-8111-111111111111"
    private let b = "22222222-2222-4222-8222-222222222222"

    private func root() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("lingxi-recovery-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        return root
    }
    private func ledgerURL(_ root: URL) -> URL { root.appendingPathComponent("durable-conversation-turns.json") }
    private func witnessURL(_ root: URL) -> URL { root.appendingPathComponent("durable-conversation-turns.witness") }
    private func assertFails<T>(_ operation: () async throws -> T, file: StaticString = #filePath, line: UInt = #line) async {
        do { _ = try await operation(); XCTFail("expected recovery operation to fail", file: file, line: line) }
        catch { }
    }

    func testConcurrentOwnersKeepEachOthersRecordsAndClearOnlyTheirOwnTurn() async throws {
        let url = ledgerURL(try root()), ledger = DurableConversationTurnLedger()
        async let first: Void = ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        async let second: Void = ledger.begin(url: url, sessionID: b, turnID: 22, scope: "/b")
        _ = try await (first, second)
        let firstRecord = try await ledger.load(url: url, sessionID: a)
        let secondRecord = try await ledger.load(url: url, sessionID: b)
        XCTAssertEqual(firstRecord?.turnID, 11)
        XCTAssertEqual(secondRecord?.turnID, 22)
        try await ledger.clear(url: url, sessionID: a, turnID: 11)
        let remaining = try await ledger.load(url: url, sessionID: b)
        XCTAssertEqual(remaining?.turnID, 22)
    }

    func testCorruptionAndDuplicateCanonicalOwnerNeverOverwriteAnotherRecord() async throws {
        for duplicate in [false, true] {
            let url = ledgerURL(try root()), ledger = DurableConversationTurnLedger()
            try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
            let data: Data
            if duplicate {
                let existing = try JSONDecoder().decode([DurableConversationTurnRecord].self, from: Data(contentsOf: url))
                data = try JSONEncoder().encode(existing + [.init(scope: "/a", sessionID: a.uppercased(), turnID: 12, lastSequence: 0)])
            } else { data = Data("{corrupt".utf8) }
            try data.write(to: url, options: .atomic)
            await assertFails { try await ledger.begin(url: url, sessionID: b, turnID: 22, scope: "/b") }
            XCTAssertEqual(try Data(contentsOf: url), data)
        }
    }

    func testColdReopenRejectsMissingWitnessOrPreviouslyObservedLedger() async throws {
        for missingWitness in [false, true] {
            let directory = try root(), url = ledgerURL(directory), first = DurableConversationTurnLedger()
            try await first.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
            let retained = try Data(contentsOf: missingWitness ? url : witnessURL(directory))
            try FileManager.default.removeItem(at: missingWitness ? witnessURL(directory) : url)
            let reopened = DurableConversationTurnLedger()
            await assertFails { try await reopened.load(url: url, sessionID: a) }
            await assertFails { try await reopened.begin(url: url, sessionID: b, turnID: 22, scope: "/b") }
            XCTAssertEqual(try Data(contentsOf: missingWitness ? url : witnessURL(directory)), retained)
        }
    }

    func testObservedStoreCannotBeReinitializedAfterBothFilesDisappear() async throws {
        let directory = try root(), url = ledgerURL(directory), ledger = DurableConversationTurnLedger()
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try FileManager.default.removeItem(at: url)
        try FileManager.default.removeItem(at: witnessURL(directory))
        await assertFails { try await ledger.begin(url: url, sessionID: b, turnID: 22, scope: "/b") }
        XCTAssertFalse(FileManager.default.fileExists(atPath: url.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: witnessURL(directory).path))
    }

    func testUncertainClearKeepsWitnessAndExplicitRetryPreservesOtherOwner() async throws {
        let directory = try root(), url = ledgerURL(directory), failure = RecoveryWriterFailure()
        let ledger = DurableConversationTurnLedger(writer: RecoveryFailureWriter(failure: failure))
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try await ledger.begin(url: url, sessionID: b, turnID: 22, scope: "/b")
        let oldWitness = try Data(contentsOf: witnessURL(directory))
        failure.set(.beforeWitness)
        await assertFails { try await ledger.clear(url: url, sessionID: a, turnID: 11) }
        XCTAssertEqual(try Data(contentsOf: witnessURL(directory)), oldWitness)
        await assertFails { try await ledger.begin(url: url, sessionID: b, turnID: 23, scope: "/b") }
        let reopened = DurableConversationTurnLedger()
        await assertFails { try await reopened.load(url: url, sessionID: a) }
        failure.set(.none)
        try await ledger.clear(url: url, sessionID: a, turnID: 11)
        let remaining = try await ledger.load(url: url, sessionID: b)
        XCTAssertEqual(remaining?.turnID, 22)
    }

    func testFailedCheckpointDoesNotAdmitPromptOrConsumeItsContents() async throws {
        let directory = try root(), failure = RecoveryWriterFailure()
        let ledger = DurableConversationTurnLedger(writer: RecoveryFailureWriter(failure: failure))
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
            sessionMode: .code, visionDelegationEnabled: false), recoveryLedger: ledger)
        source.model.activeSessionId = a
        var commands: [ClientCommand] = []
        source.setCommandSubmitterForTesting { commands.append($0) }
        failure.set(.beforeLedger)
        let token = await source.send("retain this draft")
        XCTAssertNil(token)
        XCTAssertTrue(source.model.messages.isEmpty)
        XCTAssertTrue(commands.isEmpty)
        XCTAssertNotNil(source.model.error)
        XCTAssertTrue(FileManager.default.fileExists(atPath: witnessURL(directory).path))

        failure.set(.none)
        let retry = await source.send("retain this draft")
        XCTAssertNotNil(retry, "an exactly proven pre-publication failure can be retried")
        XCTAssertEqual(source.model.messages.map(\.text), ["retain this draft"])
        try await ledger.begin(url: ledgerURL(directory), sessionID: b, turnID: 22, scope: "/b")
        let unrelated = try await ledger.load(url: ledgerURL(directory), sessionID: b)
        XCTAssertEqual(unrelated?.turnID, 22)
    }

    func testPrepublicationFailureRestoresExistingOwnersButUncertainWriteStaysFenced() async throws {
        let directory = try root(), url = ledgerURL(directory), failure = RecoveryWriterFailure()
        let ledger = DurableConversationTurnLedger(writer: RecoveryFailureWriter(failure: failure))
        try await ledger.begin(url: url, sessionID: b, turnID: 22, scope: "/b")
        failure.set(.beforeLedger)
        await assertFails { try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a") }
        let surviving = try await ledger.load(url: url, sessionID: b)
        XCTAssertEqual(surviving?.turnID, 22)
        failure.set(.none)
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        let retried = try await ledger.load(url: url, sessionID: a)
        XCTAssertEqual(retried?.turnID, 11)

        let uncertainDirectory = try root(), uncertainURL = ledgerURL(uncertainDirectory)
        let uncertainFailure = RecoveryWriterFailure()
        let uncertain = DurableConversationTurnLedger(writer: RecoveryFailureWriter(failure: uncertainFailure))
        uncertainFailure.set(.afterLedger)
        await assertFails { try await uncertain.begin(url: uncertainURL, sessionID: a, turnID: 11, scope: "/a") }
        let ledgerBytes = try Data(contentsOf: uncertainURL)
        let markerBytes = try Data(contentsOf: witnessURL(uncertainDirectory))
        uncertainFailure.set(.none)
        await assertFails { try await uncertain.begin(url: uncertainURL, sessionID: b, turnID: 22, scope: "/b") }
        XCTAssertEqual(try Data(contentsOf: uncertainURL), ledgerBytes)
        XCTAssertEqual(try Data(contentsOf: witnessURL(uncertainDirectory)), markerBytes)
    }

    func testTerminalClearFailureKeepsOtherTurnRunningAndCanRetry() async throws {
        let directory = try root(), failure = RecoveryWriterFailure()
        let ledger = DurableConversationTurnLedger(writer: RecoveryFailureWriter(failure: failure))
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let source = EngineConversationSource(config: config, recoveryLedger: ledger)
        source.model.activeSessionId = a
        var commands: [ClientCommand] = []
        source.setCommandSubmitterForTesting { commands.append($0) }
        let accepted = await source.send("first")
        let token = try XCTUnwrap(accepted)
        let other = EngineConversationSource(config: config, recoveryLedger: ledger)
        other.model.activeSessionId = b
        other.setCommandSubmitterForTesting { commands.append($0) }
        let otherAccepted = await other.send("already admitted")
        XCTAssertNotNil(otherAccepted)
        source.applyForTesting(.turnStarted(turnId: token.clientTurnId))
        failure.set(.beforeWitness)
        let zero = CostDto(totalUsd: 0, inputTokens: 0, outputTokens: 0, apiCalls: 0, sessionDurationSecs: 0, formatted: "$0.00", byAccount: [])
        source.applyForTesting(.turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zero))
        let blockedWhileClearing = await source.send("keep this draft")
        XCTAssertNil(blockedWhileClearing)
        await source.waitForDurableCleanupForTesting()
        XCTAssertFalse(source.model.streaming, "the confirmed terminal result remains terminal")
        XCTAssertNotNil(source.model.error)

        XCTAssertTrue(other.model.streaming, "a storage failure cannot cancel another admitted turn")
        let otherRejected = await other.send("other draft")
        XCTAssertNil(otherRejected, "an inconsistent shared ledger cannot admit another input")
        XCTAssertEqual(other.model.messages.map(\.text), ["already admitted"])

        failure.set(.none)
        let retry = await source.send("keep this draft")
        XCTAssertNil(retry, "the first retry clears the old checkpoint without consuming the draft")
        await source.waitForDurableCleanupForTesting()
        let next = await source.send("keep this draft")
        XCTAssertNotNil(next)
    }

    func testDelayedAdmissionKeepsOriginalOwnerAndDetectsDraftABA() async throws {
        for editAndRestore in [false, true] {
            let directory = try root(), gate = RecoveryWriteGate()
            let ledger = DurableConversationTurnLedger(writer: RecoveryGateWriter(gate: gate))
            let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
                                      sessionMode: .code, visionDelegationEnabled: false)
            let source = EngineConversationSource(config: config, recoveryLedger: ledger)
            source.model.activeSessionId = a
            source.setCommandSubmitterForTesting { _ in }
            let other = MockConversationSource(sessionMode: .code)
            other.model.activeSessionId = b
            let host = SessionHost()
            let first = try host.retain(source: source, scope: .global, mode: .code,
                                        sessionID: a, workspace: "/a", draft: "original")
            let second = try host.retain(source: other, scope: .global, mode: .code,
                                         sessionID: b, workspace: "/b", draft: "other")
            host.select(first)
            let captured = CapturedComposerDraft(
                draft: Binding(get: { first.draft }, set: { first.draft = $0 }),
                units: Binding(get: { first.draftSourceCodeUnits }, set: { first.draftSourceCodeUnits = $0 }),
                attachment: Binding(get: { first.attachment }, set: { first.attachment = $0 }),
                submittedUnits: nil, revision: { first.draftRevision })
            let task = Task { await source.send("original") }
            guard await gate.waitForEntry() else {
                gate.release()
                _ = await task.value
                XCTFail("checkpoint did not reach the controlled write")
                continue
            }
            host.select(second)
            if editAndRestore {
                first.draft = "intervening voice transcript"
                first.draft = "original"
            } else {
                first.draft = "original" // Navigation's unchanged save is not an edit.
            }
            gate.release()
            let token = await task.value
            XCTAssertNotNil(token, "the admitted prompt remains owned by the original source")
            XCTAssertEqual(source.model.messages.last?.text, "original")
            XCTAssertEqual(captured.clearIfUnchanged(), !editAndRestore)
            XCTAssertEqual(first.draft, editAndRestore ? "original" : "")
            XCTAssertEqual(second.draft, "other", "completion must not clear the selected owner's draft")
            XCTAssertEqual(host.selectedEntryID, second.id)
        }
    }

    #if DEBUG
    func testStopDuringFollowupCheckpointLoadKeepsOriginalOwnerDraftAndChip() async throws {
        let directory = try root(), gate = RecoveryWriteGate()
        gate.setEnabled(false)
        let ledger = DurableConversationTurnLedger(writer: RecoveryGateWriter(gate: gate))
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let source = EngineConversationSource(config: config, recoveryLedger: ledger)
        source.beginTurnForTesting(turnId: 11, sessionId: a)
        try await source.beginDurableTurnForTesting(turnId: 11, sessionId: a)
        let host = SessionHost()
        let entry = try host.retain(source: source, scope: .global, mode: .code,
                                    sessionID: a, workspace: "/a", draft: "follow up")
        let other = MockConversationSource(sessionMode: .chat)
        other.model.activeSessionId = b
        let otherEntry = try host.retain(source: other, scope: .global, mode: .chat,
                                         sessionID: b, workspace: "/b", draft: "other")
        let attachment = ComposerAttachment(image: UIImage(), width: 1, height: 1, base64: "cA==")
        entry.attachment = attachment
        entry.draftSourceCodeUnits = Array("follow up".utf16)
        let chip = VisualizationContextChip(id: "widget", revision: 3, title: "Chart")
        source.model.visualizationChip = chip
        let loadStarted = expectation(description: "follow-up reached the ledger load")
        source.setDurableLoadWillStartForTesting { loadStarted.fulfill() }
        let cancelStarted = expectation(description: "old turn cancel entered the test transport")
        var resumeCancel: CheckedContinuation<Void, Never>?
        var promptCount = 0
        source.setCommandSubmitterForTesting { command in
            switch command {
            case .cancel:
                await withCheckedContinuation { resumeCancel = $0; cancelStarted.fulfill() }
                throw POSIXError(.ECANCELED)
            case .sendPrompt: promptCount += 1
            default: break
            }
        }
        gate.setEnabled(true)
        let blockingWrite = Task { try await ledger.begin(url: ledgerURL(directory), sessionID: b, turnID: 22, scope: "/b") }
        guard await gate.waitForEntry() else {
            gate.release()
            _ = try? await blockingWrite.value
            return XCTFail("the controlled ledger writer did not start")
        }
        let pending = Task { await source.send("follow up", sourceCodeUnits: Array("follow up".utf16),
                                               images: [ImageRefDto(mediaType: attachment.mediaType, base64: attachment.base64)],
                                               selectedSlashCommand: nil) }
        await fulfillment(of: [loadStarted], timeout: 2)
        host.select(otherEntry)
        let stop = Task { try? await source.cancelAndWait() }
        await fulfillment(of: [cancelStarted], timeout: 2)
        gate.release()
        let token = await pending.value
        XCTAssertNil(token)
        XCTAssertTrue(source.model.messages.isEmpty)
        XCTAssertEqual(promptCount, 0)
        XCTAssertEqual(entry.draft, "follow up")
        XCTAssertEqual(entry.draftSourceCodeUnits, Array("follow up".utf16))
        XCTAssertEqual(entry.attachment, attachment)
        XCTAssertEqual(source.model.visualizationChip, chip)
        XCTAssertEqual(otherEntry.draft, "other")
        resumeCancel?.resume()
        _ = await stop.value
        try await blockingWrite.value
    }

    func testStopBeforeFirstPromptDispatchDoesNotConsumeDraftOrCheckpoint() async throws {
        let directory = try root(), ledger = DurableConversationTurnLedger()
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let source = EngineConversationSource(config: config, recoveryLedger: ledger)
        source.model.activeSessionId = a
        let chip = VisualizationContextChip(id: "widget", revision: 1, title: "Chart")
        source.model.visualizationChip = chip
        let started = expectation(description: "prompt reached the last pre-dispatch barrier")
        var resumeDispatch: CheckedContinuation<Void, Never>?
        source.setPromptDispatchBarrierForTesting {
            await withCheckedContinuation { resumeDispatch = $0; started.fulfill() }
        }
        var promptCount = 0
        source.setCommandSubmitterForTesting { command in
            if case .sendPrompt = command { promptCount += 1 }
        }
        let pending = Task { await source.send("keep me") }
        await fulfillment(of: [started], timeout: 2)
        source.cancel()
        resumeDispatch?.resume()
        let token = await pending.value
        XCTAssertNil(token)
        XCTAssertTrue(source.model.messages.isEmpty)
        XCTAssertFalse(source.model.streaming)
        XCTAssertEqual(source.model.visualizationChip, chip)
        XCTAssertEqual(promptCount, 0)
        await source.waitForDurableCleanupForTesting()
        let checkpoint = try await ledger.load(url: ledgerURL(directory), sessionID: a)
        XCTAssertNil(checkpoint)
    }

    func testPromptDispatchDoesNotConsumeChipEditedDuringAdmission() async throws {
        let directory = try root()
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
            sessionMode: .code, visionDelegationEnabled: false), recoveryLedger: DurableConversationTurnLedger())
        source.model.activeSessionId = a
        let chip = VisualizationContextChip(id: "widget", revision: 1, title: "Chart")
        source.model.visualizationChip = chip
        let started = expectation(description: "prompt reached the pre-dispatch barrier")
        var resumeDispatch: CheckedContinuation<Void, Never>?
        source.setPromptDispatchBarrierForTesting {
            await withCheckedContinuation { resumeDispatch = $0; started.fulfill() }
        }
        source.setCommandSubmitterForTesting { _ in }
        let pending = Task { await source.send("ask about this") }
        await fulfillment(of: [started], timeout: 2)
        source.model.visualizationChip = nil
        source.model.visualizationChip = chip
        resumeDispatch?.resume()
        let token = await pending.value
        XCTAssertNotNil(token)
        XCTAssertEqual(source.model.visualizationChip, chip, "an intervening edit returning to the same chip must survive")
        XCTAssertEqual(source.model.messages.last?.visualizationContext, chip)
    }

    func testRemovedChipBeforeAdmissionIsNotAutomaticallyRestoredAfterRejectedStop() async throws {
        let directory = try root()
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
            sessionMode: .code, visionDelegationEnabled: false), recoveryLedger: DurableConversationTurnLedger())
        source.model.activeSessionId = a
        let host = SessionHost()
        let entry = try host.retain(source: source, scope: .global, mode: .code,
                                    sessionID: a, workspace: "/a", draft: "input")
        let captured = CapturedComposerDraft(
            draft: Binding(get: { entry.draft }, set: { entry.draft = $0 }),
            units: Binding(get: { entry.draftSourceCodeUnits }, set: { entry.draftSourceCodeUnits = $0 }),
            attachment: Binding(get: { entry.attachment }, set: { entry.attachment = $0 }),
            submittedUnits: nil, revision: { entry.draftRevision })
        let chip = VisualizationContextChip(id: "chart", revision: 1, title: "Chart")
        source.model.visualizationChip = chip
        let chipRevision = source.model.visualizationChipRevision
        let barrier = expectation(description: "waiting before prompt admission")
        let dispatched = expectation(description: "original prompt dispatched")
        let cancelAck = expectation(description: "cancel acknowledged")
        var resumeDispatch: CheckedContinuation<Void, Never>?
        var resumeSend: CheckedContinuation<Void, Never>?
        source.setPromptDispatchBarrierForTesting {
            await withCheckedContinuation { resumeDispatch = $0; barrier.fulfill() }
        }
        source.setCancelSubmissionObserverForTesting { _ in cancelAck.fulfill() }
        source.setCommandSubmitterForTesting { command in
            if case .sendPrompt = command {
                await withCheckedContinuation { resumeSend = $0; dispatched.fulfill() }
                throw ClientError.Rejected(message: "turn_cancelled_before_admission")
            }
        }
        let send = Task { await source.send("input") }
        await fulfillment(of: [barrier], timeout: 2)
        source.model.visualizationChip = nil
        resumeDispatch?.resume()
        await fulfillment(of: [dispatched], timeout: 2)
        let submitted = await send.value
        let token = try XCTUnwrap(submitted)
        XCTAssertTrue(captured.clearIfUnchanged())
        let clearRevision = entry.draftRevision
        source.registerRejectedPromptRestorer(for: token) { submitted in
            captured.restoreIfUnchanged(afterClearRevision: clearRevision, model: source.model,
                                        chip: chip, chipRevision: chipRevision, submitted: submitted)
        }
        let stop = Task { try await source.cancelAndWait() }
        await fulfillment(of: [cancelAck], timeout: 2)
        resumeSend?.resume()
        try await stop.value
        XCTAssertNil(source.model.visualizationChip)
        XCTAssertEqual(entry.draft, "")
        let unsent = try XCTUnwrap(source.model.unsentPrompts.first)
        XCTAssertFalse(unsent.mayRestoreCapturedDraft)
        XCTAssertEqual(unsent.text, "input")
        XCTAssertEqual(unsent.visualizationContext, chip)
    }

    func testRemovedChipBeforeSendTaskStartsIsNotAutomaticallyRestored() async throws {
        let directory = try root()
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
            sessionMode: .code, visionDelegationEnabled: false), recoveryLedger: DurableConversationTurnLedger())
        source.model.activeSessionId = a
        let host = SessionHost()
        let entry = try host.retain(source: source, scope: .global, mode: .code,
                                    sessionID: a, workspace: "/a", draft: "input")
        let captured = CapturedComposerDraft(
            draft: Binding(get: { entry.draft }, set: { entry.draft = $0 }),
            units: Binding(get: { entry.draftSourceCodeUnits }, set: { entry.draftSourceCodeUnits = $0 }),
            attachment: Binding(get: { entry.attachment }, set: { entry.attachment = $0 }),
            submittedUnits: nil, revision: { entry.draftRevision })
        let chip = VisualizationContextChip(id: "chart", revision: 1, title: "Chart")
        source.model.visualizationChip = chip
        let capturedChipRevision = source.model.visualizationChipRevision
        let dispatched = expectation(description: "queued send reached the controlled transport")
        let cancelAck = expectation(description: "cancel acknowledged")
        var resumeSend: CheckedContinuation<Void, Never>?
        source.setCancelSubmissionObserverForTesting { _ in cancelAck.fulfill() }
        source.setCommandSubmitterForTesting { command in
            if case .sendPrompt = command {
                await withCheckedContinuation { resumeSend = $0; dispatched.fulfill() }
                throw ClientError.Rejected(message: "turn_cancelled_before_admission")
            }
        }
        let send = Task { await source.send("input") }
        // This runs before the MainActor task can call source.send.
        source.model.visualizationChip = nil
        await fulfillment(of: [dispatched], timeout: 2)
        let submitted = await send.value
        let token = try XCTUnwrap(submitted)
        XCTAssertTrue(captured.clearIfUnchanged())
        let clearRevision = entry.draftRevision
        source.registerRejectedPromptRestorer(for: token) { actual in
            captured.restoreIfUnchanged(afterClearRevision: clearRevision, model: source.model,
                chip: chip, chipRevision: capturedChipRevision, submitted: actual)
        }
        let stop = Task { try await source.cancelAndWait() }
        await fulfillment(of: [cancelAck], timeout: 2)
        resumeSend?.resume()
        try await stop.value
        XCTAssertNil(source.model.visualizationChip)
        XCTAssertEqual(entry.draft, "")
        let unsent = try XCTUnwrap(source.model.unsentPrompts.first)
        XCTAssertNil(unsent.visualizationContext)
        XCTAssertEqual(unsent.text, "input")
    }

    func testStopAfterPromptHandoffDoesNotRestoreOrResubmitUnknownInput() async throws {
        let directory = try root()
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
            sessionMode: .code, visionDelegationEnabled: false), recoveryLedger: DurableConversationTurnLedger())
        source.model.activeSessionId = a
        let dispatched = expectation(description: "prompt entered the test transport")
        var resumePrompt: CheckedContinuation<Void, Never>?
        var promptCount = 0
        source.setCommandSubmitterForTesting { command in
            switch command {
            case .sendPrompt:
                promptCount += 1
                await withCheckedContinuation { resumePrompt = $0; dispatched.fulfill() }
            case .cancel: throw POSIXError(.ECANCELED)
            default: break
            }
        }
        let pending = Task { await source.send("already handed off") }
        await fulfillment(of: [dispatched], timeout: 2)
        let token = await pending.value
        XCTAssertNotNil(token)
        do { try await source.cancelAndWait(); XCTFail("the controlled cancel should fail") }
        catch { }
        resumePrompt?.resume()
        XCTAssertEqual(promptCount, 1)
        XCTAssertEqual(source.model.messages.filter { $0.role == .user }.map(\.text), ["already handed off"])
    }

    func testCancelledBeforeAdmissionNeedsBothReceiptsInEitherOrder() async throws {
        for side in [false, true] {
            for rejectionFirst in [false, true] {
                for interveningEdit in [false, true] {
                    try await exerciseCancelledBeforeAdmission(side: side,
                        rejectionFirst: rejectionFirst, interveningEdit: interveningEdit)
                }
            }
        }
    }

    func testRejectedSendDoesNotSettleUntilItsExactCancelSucceeds() async throws {
        for side in [false, true] {
            let directory = try root(), ledger = DurableConversationTurnLedger()
            let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "",
                appSandboxRoot: directory.path, sessionMode: .code, visionDelegationEnabled: false),
                recoveryLedger: ledger, memoryOnly: side)
            if side { source.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a") }
            else { source.model.activeSessionId = a }
            let dispatched = expectation(description: "original send entered")
            let proof = expectation(description: "original send was rejected")
            var releaseSend: CheckedContinuation<Void, Never>?
            var cancels = 0
            source.setCommandSubmitterForTesting { command in
                switch command {
                case .sendPrompt:
                    await withCheckedContinuation { releaseSend = $0; dispatched.fulfill() }
                    throw ClientError.Rejected(message: "turn_cancelled_before_admission")
                case .cancel:
                    cancels += 1
                    if cancels == 1 { throw POSIXError(.EIO) }
                default: break
                }
            }
            source.setNoAdmissionProofObserverForTesting { _ in proof.fulfill() }
            let send = Task { await source.send("retain") }
            await fulfillment(of: [dispatched], timeout: 2)
            let submitted = await send.value
            XCTAssertNotNil(submitted)
            do { try await source.cancelAndWait(); XCTFail("the first cancel must fail") }
            catch { }
            releaseSend?.resume()
            await fulfillment(of: [proof], timeout: 2)
            XCTAssertTrue(source.model.unsentPrompts.isEmpty)
            XCTAssertEqual(source.model.messages.filter { $0.role == .user }.map(\.text), ["retain"])
            if !side {
                let checkpoint = try await ledger.load(url: ledgerURL(directory), sessionID: a)
                XCTAssertNotNil(checkpoint)
            }
            try await source.cancelAndWait()
            XCTAssertEqual(source.model.unsentPrompts.map(\.text), ["retain"])
            XCTAssertTrue(source.model.messages.filter { $0.role == .user }.isEmpty)
            XCTAssertEqual(cancels, 2)
        }
    }

    func testDifferentRejectedErrorDoesNotCreateUnsentPrompt() async throws {
        for side in [false, true] {
            let directory = try root()
            let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "",
                appSandboxRoot: directory.path, sessionMode: .code, visionDelegationEnabled: false),
                recoveryLedger: DurableConversationTurnLedger(), memoryOnly: side)
            if side { source.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a") }
            else { source.model.activeSessionId = a }
            let dispatched = expectation(description: "original send entered")
            let failureShown = expectation(description: "unknown failure is shown")
            var releaseSend: CheckedContinuation<Void, Never>?
            let observation = source.model.$error.dropFirst().sink { value in
                if value != nil { failureShown.fulfill() }
            }
            source.setCommandSubmitterForTesting { command in
                if case .sendPrompt = command {
                    await withCheckedContinuation { releaseSend = $0; dispatched.fulfill() }
                    throw ClientError.Rejected(message: "turn_cancelled_before_admission_extra")
                }
            }
            let send = Task { await source.send("unknown") }
            await fulfillment(of: [dispatched], timeout: 2)
            _ = await send.value
            releaseSend?.resume()
            await fulfillment(of: [failureShown], timeout: 2)
            XCTAssertTrue(source.model.unsentPrompts.isEmpty)
            XCTAssertEqual(source.model.messages.filter { $0.role == .user }.map(\.text), ["unknown"])
            withExtendedLifetime(observation) {}
        }
    }

    func testLateRejectionCannotSettleAnotherOwnerOrTurn() async throws {
        // Main/turn, Side/side ID, Side/parent and Side/turn are independent
        // identity changes; none may inherit the old Send's proof.
        for scenario in 0..<4 {
            let side = scenario != 0
            let directory = try root()
            let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "",
                appSandboxRoot: directory.path, sessionMode: .code, visionDelegationEnabled: false),
                recoveryLedger: DurableConversationTurnLedger(), memoryOnly: side)
            if side { source.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a") }
            else { source.model.activeSessionId = a }
            let dispatched = expectation(description: "old owner's send entered")
            let cancelStarted = expectation(description: "old owner's cancel entered")
            let cancelAck = expectation(description: "old owner's cancel returned")
            var releaseSend: CheckedContinuation<Void, Never>?
            var releaseCancel: CheckedContinuation<Void, Never>?
            source.setNoAdmissionProofObserverForTesting { _ in XCTFail("stale send must not create proof") }
            source.setCancelSubmissionObserverForTesting { _ in cancelAck.fulfill() }
            source.setCommandSubmitterForTesting { command in
                switch command {
                case .sendPrompt:
                    await withCheckedContinuation { releaseSend = $0; dispatched.fulfill() }
                    throw ClientError.Rejected(message: "turn_cancelled_before_admission")
                case .cancel:
                    await withCheckedContinuation { releaseCancel = $0; cancelStarted.fulfill() }
                default: break
                }
            }
            let send = Task { await source.send("old") }
            await fulfillment(of: [dispatched], timeout: 2)
            let submitted = await send.value
            let oldToken = try XCTUnwrap(submitted)
            let stop = Task { try await source.cancelAndWait() }
            await fulfillment(of: [cancelStarted], timeout: 2)
            if scenario == 1 { source.setSideOwnerForTesting(parentSessionID: a, sideID: "side-b") }
            if scenario == 2 { source.setSideOwnerForTesting(parentSessionID: b, sideID: "side-a") }
            let nextID = scenario == 1 || scenario == 2 ? oldToken.clientTurnId :
                (oldToken.clientTurnId == UInt64.max ? 1 : oldToken.clientTurnId + 1)
            let nextSessionID = scenario == 1 || scenario == 3 ? a : b
            source.beginTurnForTesting(turnId: nextID, sessionId: nextSessionID)
            source.model.isCancelling = true
            releaseSend?.resume()
            releaseCancel?.resume()
            await fulfillment(of: [cancelAck], timeout: 2)
            stop.cancel()
            _ = try? await stop.value
            XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, nextID)
            XCTAssertEqual(source.model.activeSessionId, nextSessionID)
            XCTAssertTrue(source.model.isCancelling, "the old completion must not unlock the new owner")
            XCTAssertTrue(source.model.unsentPrompts.isEmpty)
            XCTAssertNil(source.model.error)
        }
    }

    func testUnsentInputSurvivesOnlySameOwnerReplacement() async throws {
        let host = SessionHost()
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: try root().path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let old = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        old.model.activeSessionId = a
        let state = SessionWorkspaceState(sessionId: a, cwd: "/a", projectRoot: "/a",
                                          additionalDirectories: [], transcriptPath: "/a/session.jsonl")
        old.model.workspaceState = state
        let entry = try host.retain(source: old, scope: .global, mode: .code,
                                    sessionID: a, workspace: "/a", draft: "new draft")
        let token = ConversationTurnToken(clientTurnId: 3, sessionEpoch: 1)
        let item = ConversationUnsentPrompt(token: token, sessionID: a, sideID: nil,
            text: "not admitted", sourceCodeUnits: [65, 0xd800],
            images: [ImageRefDto(mediaType: "image/png", base64: "cA==")],
            visualizationContext: VisualizationContextChip(id: "chart", revision: 1, title: "Chart"),
            mayRestoreCapturedDraft: true, chipRevisionAtCapture: 1, chipRevisionAfterAdmission: 2)
        old.model.unsentPrompts = [item]
        host.releaseIdleSource(old)
        XCTAssertTrue(host.entry(for: old) === entry)

        let replacement = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        replacement.model.activeSessionId = a
        replacement.model.workspaceState = state
        var catalogPath = state.transcriptPath
        let preparationStarted = expectation(description: "replacement prepare is suspended")
        var resumePreparation: CheckedContinuation<Void, Never>?
        let staleReplacement = Task { @MainActor in
            await withCheckedContinuation { resumePreparation = $0; preparationStarted.fulfill() }
            return try host.replaceIdleSource(old, with: replacement, scope: .global, mode: .code,
                sessionID: a, workspace: "/a", draft: "new draft", validateUnsentOwner: {
                    guard catalogPath == state.transcriptPath else { throw SessionHost.OwnershipError.sourceIdentityMismatch }
                })
        }
        await fulfillment(of: [preparationStarted], timeout: 2)
        catalogPath = "/a/other.jsonl"
        resumePreparation?.resume()
        do { _ = try await staleReplacement.value; XCTFail("the changed catalog must reject the swap") }
        catch { }
        XCTAssertTrue(host.entry(for: old) === entry)
        XCTAssertEqual(old.model.unsentPrompts.first?.id, item.id)
        catalogPath = state.transcriptPath
        let next = try host.replaceIdleSource(old, with: replacement, scope: .global,
            mode: .code, sessionID: a, workspace: "/a", draft: "new draft", validateUnsentOwner: {
                guard catalogPath == state.transcriptPath else { throw SessionHost.OwnershipError.sourceIdentityMismatch }
            })
        XCTAssertTrue(host.entry(for: replacement) === next)
        XCTAssertTrue(old.model.unsentPrompts.isEmpty)
        XCTAssertEqual(replacement.model.unsentPrompts.first?.id, item.id)
        XCTAssertEqual(replacement.model.unsentPrompts.first?.sourceCodeUnits, [65, 0xd800])
        XCTAssertEqual(replacement.model.unsentPrompts.first?.images, item.images)
        XCTAssertEqual(replacement.model.unsentPrompts.first?.visualizationContext, item.visualizationContext)

        let wrong = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        wrong.model.activeSessionId = a
        wrong.model.workspaceState = SessionWorkspaceState(sessionId: a, cwd: "/a", projectRoot: "/a",
            additionalDirectories: [], transcriptPath: "/a/other.jsonl")
        replacement.model.unsentPrompts = [item]
        XCTAssertThrowsError(try host.replaceIdleSource(replacement, with: wrong, scope: .global,
            mode: .code, sessionID: a, workspace: "/a", draft: "new draft"))
        XCTAssertTrue(host.entry(for: replacement) === next)
        wrong.model.workspaceState = state
        replacement.model.unsentPrompts = [ConversationUnsentPrompt(token: token, sessionID: b,
            sideID: nil, text: "wrong owner", sourceCodeUnits: nil, images: [], visualizationContext: nil,
            mayRestoreCapturedDraft: true, chipRevisionAtCapture: 0, chipRevisionAfterAdmission: 0)]
        XCTAssertThrowsError(try host.replaceIdleSource(replacement, with: wrong, scope: .global,
            mode: .code, sessionID: a, workspace: "/a", draft: "new draft"))
        XCTAssertTrue(host.entry(for: replacement) === next)
        XCTAssertEqual(replacement.model.unsentPrompts.first?.text, "wrong owner")
    }

    func testIdleExpiredSideArchivesOwnerPayloadAndReleasesSharedHandle() async throws {
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: try root().path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let parent = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        parent.model.activeSessionId = a
        let child = EngineConversationSource(config: config, memoryOnly: true)
        child.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a")
        var handle: ExpiredSideTestHandle? = ExpiredSideTestHandle()
        weak var weakHandle = handle
        child.setSideHandleForTesting(try XCTUnwrap(handle))
        handle = nil
        let entry = SideChatEntry(source: child, id: "side-a")
        entry.draft = "draft"
        entry.draftSourceCodeUnits = [65, 0xd800]
        let photo = ComposerAttachment(image: UIImage(), width: 1, height: 1, base64: "cA==")
        entry.attachment = photo
        let chip = VisualizationContextChip(id: "chart", revision: 1, title: "Chart")
        child.model.visualizationChip = chip
        let image = ImageRefDto(mediaType: photo.mediaType, base64: photo.base64)
        child.onSideSubmissionFailure?("previous failure", [66, 0xd800], [image])
        let token = ConversationTurnToken(clientTurnId: 7, sessionEpoch: 1)
        child.model.unsentPrompts = [ConversationUnsentPrompt(token: token, sessionID: a, sideID: "side-a",
            text: "rejected", sourceCodeUnits: [67, 0xd800], images: [image], visualizationContext: chip,
            mayRestoreCapturedDraft: false, chipRevisionAtCapture: 1, chipRevisionAfterAdmission: 2)]
        parent.sideChats.entries.append(entry)
        parent.sideChats.selectedID = "side-a"
        parent.sideChats.removeEnded("side-a")

        XCTAssertNil(weakHandle, "an archived side must not retain the shared native handle")
        XCTAssertFalse(parent.hasLiveSideChats)
        XCTAssertTrue(parent.sideChats.entries.isEmpty)
        let archive = try XCTUnwrap(parent.sideChats.expiredArchives.first)
        XCTAssertEqual(archive.id, "side-a")
        XCTAssertEqual(archive.parentSessionID, a)
        XCTAssertEqual(archive.parentOwnerEpoch, 1)
        XCTAssertEqual(archive.draft, "draft")
        XCTAssertEqual(archive.draftSourceCodeUnits, [65, 0xd800])
        XCTAssertEqual(archive.attachment, photo)
        XCTAssertEqual(archive.visualizationContext, chip)
        XCTAssertEqual(archive.unsent.first?.sourceCodeUnits, [66, 0xd800])
        XCTAssertEqual(archive.rejected.first?.sourceCodeUnits, [67, 0xd800])
        XCTAssertEqual(archive.rejected.first?.images, [image])
        let denied = await child.send("must not send")
        XCTAssertNil(denied)
        try await child.cancelAndWait()
        XCTAssertTrue(parent.model.unsentPrompts.isEmpty)

        let explicitlyEnded = EngineConversationSource(config: config, memoryOnly: true)
        explicitlyEnded.setSideOwnerForTesting(parentSessionID: a, sideID: "side-b")
        let explicitEntry = SideChatEntry(source: explicitlyEnded, id: "side-b")
        explicitEntry.draft = "user chose to end"
        explicitEntry.closing = true
        parent.sideChats.entries.append(explicitEntry)
        parent.sideChats.removeEnded("side-b")
        XCTAssertEqual(parent.sideChats.expiredArchives.map(\.id), ["side-a"])
        parent.sideChats.discardExpired("side-a")
        XCTAssertTrue(parent.sideChats.expiredArchives.isEmpty)
    }

    func testSideExpiryBeforeEitherReceiptKeepsOneUnconfirmedOwnerPayload() async throws {
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: try root().path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let parent = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        parent.model.activeSessionId = a
        let child = EngineConversationSource(config: config, memoryOnly: true)
        child.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a")
        let entry = SideChatEntry(source: child, id: "side-a")
        parent.sideChats.entries.append(entry)
        parent.sideChats.selectedID = "side-a"
        let chip = VisualizationContextChip(id: "chart", revision: 1, title: "Chart")
        child.model.visualizationChip = chip
        let image = ImageRefDto(mediaType: "image/png", base64: "cA==")
        let units: [UInt16] = [65, 0xd800]
        let dispatched = expectation(description: "side send is in flight")
        let cancelStarted = expectation(description: "side cancel is in flight")
        var releaseSend: CheckedContinuation<Void, Never>?
        var releaseCancel: CheckedContinuation<Void, Never>?
        var sends = 0
        child.setCommandSubmitterForTesting { command in
            switch command {
            case .sendPrompt:
                sends += 1
                await withCheckedContinuation { releaseSend = $0; dispatched.fulfill() }
                throw ClientError.Rejected(message: "turn_cancelled_before_admission")
            case .cancel:
                await withCheckedContinuation { releaseCancel = $0; cancelStarted.fulfill() }
            default: break
            }
        }
        let send = Task { await child.send("input", sourceCodeUnits: units, images: [image], selectedSlashCommand: nil) }
        await fulfillment(of: [dispatched], timeout: 2)
        let submitted = await send.value
        XCTAssertNotNil(submitted)
        let stop = Task { try? await child.cancelAndWait() }
        await fulfillment(of: [cancelStarted], timeout: 2)
        parent.sideChats.removeEnded("side-a")
        let archive = try XCTUnwrap(parent.sideChats.expiredArchives.first)
        XCTAssertTrue(archive.rejected.isEmpty)
        XCTAssertEqual(archive.outstanding.count, 1)
        XCTAssertEqual(archive.outstanding.first?.parentSessionID, a)
        XCTAssertEqual(archive.outstanding.first?.sideID, "side-a")
        XCTAssertEqual(archive.outstanding.first?.text, "input")
        XCTAssertEqual(archive.outstanding.first?.sourceCodeUnits, units)
        XCTAssertEqual(archive.outstanding.first?.images, [image])
        XCTAssertEqual(archive.outstanding.first?.visualizationContext, chip)
        releaseSend?.resume()
        releaseCancel?.resume()
        _ = await stop.value
        XCTAssertEqual(parent.sideChats.expiredArchives.first?.outstanding.count, 1)
        XCTAssertTrue(parent.sideChats.expiredArchives.first?.rejected.isEmpty == true)
        XCTAssertEqual(sends, 1)
        XCTAssertTrue(parent.model.unsentPrompts.isEmpty)
    }

    func testSideExpiryPreservesEachUnknownSubmissionAndItsCapturedChip() async throws {
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: try root().path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let parent = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        parent.model.activeSessionId = a
        let child = EngineConversationSource(config: config, memoryOnly: true)
        child.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a")
        let entry = SideChatEntry(source: child, id: "side-a")
        parent.sideChats.entries.append(entry)
        let firstChip = VisualizationContextChip(id: "first", revision: 1, title: "First")
        let secondChip = VisualizationContextChip(id: "second", revision: 2, title: "Second")
        let draftChip = VisualizationContextChip(id: "draft", revision: 3, title: "Draft")
        let images = [ImageRefDto(mediaType: "image/png", base64: "YQ=="),
                      ImageRefDto(mediaType: "image/png", base64: "Yg==")]

        for (index, chip) in [firstChip, secondChip].enumerated() {
            child.dismissError()
            child.model.visualizationChip = chip
            let dispatched = expectation(description: "submission \(index) reached the transport")
            let failed = expectation(description: "submission \(index) returned an unknown error")
            var releaseSend: CheckedContinuation<Void, Never>?
            let observation = child.model.$error.dropFirst().sink { if $0 != nil { failed.fulfill() } }
            child.setCommandSubmitterForTesting { command in
                if case .sendPrompt = command {
                    await withCheckedContinuation { releaseSend = $0; dispatched.fulfill() }
                    throw POSIXError(.ECONNRESET)
                }
            }
            let text = "input \(index)"
            let send = Task { await child.send(text, sourceCodeUnits: Array(text.utf16),
                                                images: [images[index]], selectedSlashCommand: nil) }
            await fulfillment(of: [dispatched], timeout: 2)
            let token = await send.value
            XCTAssertNotNil(token)
            XCTAssertNil(child.model.visualizationChip)
            releaseSend?.resume()
            await fulfillment(of: [failed], timeout: 2)
            observation.cancel()
        }

        entry.draft = "new draft"
        child.model.visualizationChip = draftChip
        parent.sideChats.removeEnded("side-a")
        let archive = try XCTUnwrap(parent.sideChats.expiredArchives.first)
        XCTAssertEqual(archive.parentSessionID, a)
        XCTAssertEqual(archive.id, "side-a")
        XCTAssertEqual(archive.draft, "new draft")
        XCTAssertEqual(archive.visualizationContext, draftChip)
        XCTAssertEqual(archive.outstanding.count, 2)
        for (index, input) in archive.outstanding.enumerated() {
            XCTAssertEqual(input.parentSessionID, a)
            XCTAssertEqual(input.sideID, "side-a")
            XCTAssertEqual(input.sideOwnerEpoch, archive.sideOwnerEpoch)
            XCTAssertEqual(input.text, "input \(index)")
            XCTAssertEqual(input.sourceCodeUnits, Array("input \(index)".utf16))
            XCTAssertEqual(input.images, [images[index]])
            XCTAssertEqual(input.visualizationContext, [firstChip, secondChip][index])
        }
        XCTAssertTrue(archive.rejected.isEmpty)
        XCTAssertTrue(parent.model.unsentPrompts.isEmpty)
    }

    func testSideErrorBeforeSendReturnsKeepsOriginalPayloadThroughNextTurn() async throws {
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: try root().path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let parent = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        parent.model.activeSessionId = a
        let child = EngineConversationSource(config: config, memoryOnly: true)
        child.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a")
        let entry = SideChatEntry(source: child, id: "side-a")
        parent.sideChats.entries.append(entry)
        let firstChip = VisualizationContextChip(id: "first", revision: 1, title: "First")
        let nextChip = VisualizationContextChip(id: "next", revision: 2, title: "Next")
        let firstDispatched = expectation(description: "first send is suspended")
        let nextDispatched = expectation(description: "next send is suspended")
        let firstReturned = expectation(description: "old send returns after next admission")
        var firstContinuation: CheckedContinuation<Void, Never>?
        var nextContinuation: CheckedContinuation<Void, Never>?
        var sends = 0
        child.setCommandSubmitterForTesting { command in
            guard case .sendPrompt = command else { return }
            sends += 1
            if sends == 1 {
                await withCheckedContinuation { firstContinuation = $0; firstDispatched.fulfill() }
                throw POSIXError(.ECONNRESET)
            }
            await withCheckedContinuation { nextContinuation = $0; nextDispatched.fulfill() }
        }
        child.model.visualizationChip = firstChip
        let first = Task { await child.send("first") }
        await fulfillment(of: [firstDispatched], timeout: 2)
        let firstToken = await first.value
        XCTAssertNotNil(firstToken)
        child.setPromptFailureHandledForTesting { if $0 == firstToken { firstReturned.fulfill() } }
        child.applyForTesting(.error(kind: .rejected, message: "unconfirmed".clientTextDto))
        XCTAssertTrue(entry.unsentPrompts.isEmpty)

        child.model.visualizationChip = nextChip
        let next = Task { await child.send("next") }
        await fulfillment(of: [nextDispatched], timeout: 2)
        let nextToken = await next.value
        XCTAssertNotNil(nextToken)
        firstContinuation?.resume()
        await fulfillment(of: [firstReturned], timeout: 2)
        XCTAssertEqual(child.model.activeTurnToken, nextToken, "the old FFI failure must not settle the new turn")

        parent.sideChats.removeEnded("side-a")
        let archive = try XCTUnwrap(parent.sideChats.expiredArchives.first)
        XCTAssertEqual(archive.outstanding.map(\.text), ["first", "next"])
        XCTAssertEqual(archive.outstanding.map(\.visualizationContext), [firstChip, nextChip])
        XCTAssertTrue(archive.outstanding.allSatisfy { $0.parentSessionID == a && $0.sideID == "side-a" })
        XCTAssertTrue(archive.rejected.isEmpty)
        nextContinuation?.resume()
    }

    func testSideExpiryDuringInitialAdmissionKeepsParentDraft() async throws {
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: try root().path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let parent = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        parent.model.activeSessionId = a
        let host = SessionHost()
        let owner = try host.retain(source: parent, scope: .global, mode: .code,
                                    sessionID: a, workspace: "/a", draft: "/side keep")
        let units = Array("/side keep".utf16)
        owner.draftSourceCodeUnits = units
        let captured = CapturedComposerDraft(
            draft: Binding(get: { owner.draft }, set: { owner.draft = $0 }),
            units: Binding(get: { owner.draftSourceCodeUnits }, set: { owner.draftSourceCodeUnits = $0 }),
            attachment: Binding(get: { owner.attachment }, set: { owner.attachment = $0 }),
            submittedUnits: units, revision: { owner.draftRevision })
        let child = EngineConversationSource(config: config, memoryOnly: true)
        child.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a")
        let entry = SideChatEntry(source: child, id: "side-a")
        let collection = SideChatCollection(parent: parent, createEntry: { _ in entry })
        let atBarrier = expectation(description: "initial side send is awaiting admission")
        let completed = expectation(description: "creation reports its result")
        var resume: CheckedContinuation<Void, Never>?
        var accepted: Bool?
        var sends = 0
        child.setPromptDispatchBarrierForTesting {
            await withCheckedContinuation { resume = $0; atBarrier.fulfill() }
        }
        child.setCommandSubmitterForTesting { command in
            if case .sendPrompt = command { sends += 1 }
        }
        collection.create(input: "keep", inputSourceCodeUnits: Array("keep".utf16)) { result in
            accepted = result
            if result { captured.clearIfUnchanged() }
            completed.fulfill()
        }
        await fulfillment(of: [atBarrier], timeout: 2)
        collection.removeEnded("side-a")
        resume?.resume()
        await fulfillment(of: [completed], timeout: 2)
        XCTAssertEqual(accepted, false)
        XCTAssertEqual(owner.draft, "/side keep")
        XCTAssertEqual(owner.draftSourceCodeUnits, units)
        XCTAssertEqual(sends, 0)
        XCTAssertTrue(collection.expiredArchives.isEmpty)
    }

    func testSideExpiryAfterInitialHandoffArchivesOriginalWithoutRestoringParent() async throws {
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: try root().path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let parent = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        parent.model.activeSessionId = a
        let host = SessionHost()
        let owner = try host.retain(source: parent, scope: .global, mode: .code,
                                    sessionID: a, workspace: "/a", draft: "/side keep")
        let units = Array("/side keep".utf16)
        owner.draftSourceCodeUnits = units
        let captured = CapturedComposerDraft(
            draft: Binding(get: { owner.draft }, set: { owner.draft = $0 }),
            units: Binding(get: { owner.draftSourceCodeUnits }, set: { owner.draftSourceCodeUnits = $0 }),
            attachment: Binding(get: { owner.attachment }, set: { owner.attachment = $0 }),
            submittedUnits: units, revision: { owner.draftRevision })
        let child = EngineConversationSource(config: config, memoryOnly: true)
        child.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a")
        let entry = SideChatEntry(source: child, id: "side-a")
        let collection = SideChatCollection(parent: parent, createEntry: { _ in entry })
        let atTransport = expectation(description: "side send crossed the handoff")
        let completed = expectation(description: "creation reports its result")
        var resumeSend: CheckedContinuation<Void, Never>?
        var accepted: Bool?
        child.setCommandSubmitterForTesting { command in
            if case .sendPrompt = command {
                collection.removeEnded("side-a")
                await withCheckedContinuation { resumeSend = $0; atTransport.fulfill() }
                throw ClientError.Rejected(message: "turn_cancelled_before_admission")
            }
        }
        collection.create(input: "keep", inputSourceCodeUnits: Array("keep".utf16)) { result in
            accepted = result
            if result { captured.clearIfUnchanged() }
            completed.fulfill()
        }
        await fulfillment(of: [atTransport, completed], timeout: 2)
        XCTAssertEqual(accepted, true)
        XCTAssertEqual(owner.draft, "")
        let archive = try XCTUnwrap(collection.expiredArchives.first)
        XCTAssertEqual(archive.id, "side-a")
        XCTAssertEqual(archive.parentSessionID, a)
        XCTAssertEqual(archive.outstanding.count, 1)
        XCTAssertEqual(archive.outstanding.first?.text, "keep")
        XCTAssertEqual(archive.outstanding.first?.sourceCodeUnits, Array("keep".utf16))
        XCTAssertTrue(archive.rejected.isEmpty)
        resumeSend?.resume()
    }

    func testSideEndedBeforeCreationReturnsDoesNotClearParentDraft() async throws {
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: try root().path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let parent = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        parent.model.activeSessionId = a
        let child = EngineConversationSource(config: config, memoryOnly: true)
        child.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a")
        let entry = SideChatEntry(source: child, id: "side-a")
        let factoryWaiting = expectation(description: "side creation has not returned")
        let completed = expectation(description: "creation rejects the expired side")
        var resumeFactory: CheckedContinuation<SideChatEntry, Never>?
        let collection = SideChatCollection(parent: parent, createEntry: { _ in
            await withCheckedContinuation { resumeFactory = $0; factoryWaiting.fulfill() }
        })
        var accepted: Bool?
        collection.create(input: "keep") { accepted = $0; completed.fulfill() }
        await fulfillment(of: [factoryWaiting], timeout: 2)
        collection.removeEnded("side-a")
        resumeFactory?.resume(returning: entry)
        await fulfillment(of: [completed], timeout: 2)
        XCTAssertEqual(accepted, false)
        XCTAssertTrue(collection.entries.isEmpty)
        XCTAssertTrue(collection.expiredArchives.isEmpty)
        XCTAssertFalse(child.isAvailableForPrompts)
    }

    func testExpiredSideArchiveMovesOnlyWithExactParentAndFreshCatalog() throws {
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: try root().path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let parent = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        parent.model.activeSessionId = a
        let state = SessionWorkspaceState(sessionId: a, cwd: "/a", projectRoot: "/a",
                                          additionalDirectories: [], transcriptPath: "/a/session.jsonl")
        parent.model.workspaceState = state
        let child = EngineConversationSource(config: config, memoryOnly: true)
        child.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a")
        let entry = SideChatEntry(source: child, id: "side-a")
        entry.draft = "keep side"
        parent.sideChats.entries.append(entry)
        parent.sideChats.selectedID = "side-a"
        parent.sideChats.removeEnded("side-a")
        let host = SessionHost()
        let old = try host.retain(source: parent, scope: .global, mode: .code,
                                  sessionID: a, workspace: "/a", draft: "")
        host.releaseIdleSource(parent)
        XCTAssertTrue(host.entry(for: parent) === old)
        let replacement = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        replacement.model.activeSessionId = a
        replacement.model.workspaceState = state
        XCTAssertThrowsError(try host.replaceIdleSource(parent, with: replacement, scope: .global,
            mode: .code, sessionID: a, workspace: "/a", draft: ""), "archive-only replacement needs a fresh catalog check")
        XCTAssertThrowsError(try host.replaceIdleSource(parent, with: replacement, scope: .global,
            mode: .code, sessionID: a, workspace: "/a", draft: "", validateUnsentOwner: {
                throw SessionHost.OwnershipError.sourceIdentityMismatch
            }))
        XCTAssertTrue(host.entry(for: parent) === old)
        let next = try host.replaceIdleSource(parent, with: replacement, scope: .global,
            mode: .code, sessionID: a, workspace: "/a", draft: "", validateUnsentOwner: {})
        XCTAssertTrue(host.entry(for: replacement) === next)
        XCTAssertTrue(parent.sideChats.expiredArchives.isEmpty)
        XCTAssertEqual(replacement.sideChats.expiredArchives.first?.draft, "keep side")
        XCTAssertEqual(replacement.sideChats.selectedID, "side-a")

        let wrong = ExpiredSideChatArchive(id: "side-b", parentSessionID: b, parentOwnerEpoch: 1, sideOwnerEpoch: 1,
            draft: "other owner", draftSourceCodeUnits: nil, attachment: nil, visualizationContext: nil,
            unsent: [], rejected: [], outstanding: [])
        replacement.sideChats.adoptExpiredArchives([wrong])
        let third = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
        third.model.activeSessionId = a
        third.model.workspaceState = state
        XCTAssertThrowsError(try host.replaceIdleSource(replacement, with: third, scope: .global,
            mode: .code, sessionID: a, workspace: "/a", draft: "", validateUnsentOwner: {}))
        XCTAssertTrue(host.entry(for: replacement) === next)
        XCTAssertEqual(replacement.sideChats.expiredArchives.count, 2)
    }

    func testExpiredSideKeepsDuplicatePayloadButShowsItOnceAsUnconfirmed() {
        let units: [UInt16] = [65, 0xd800]
        let image = ImageRefDto(mediaType: "image/jpeg", base64: "cA==")
        let photo = ComposerAttachment(image: UIImage(), width: 1, height: 1, base64: "cA==")
        let chip = VisualizationContextChip(id: "chart", revision: 1, title: "Chart")
        let pending = ExpiredSideChatInput(parentSessionID: a, sideID: "side-a", sideOwnerEpoch: 1, text: "same",
            sourceCodeUnits: units, images: [image], visualizationContext: chip)
        let archive = ExpiredSideChatArchive(id: "side-a", parentSessionID: a, parentOwnerEpoch: 1, sideOwnerEpoch: 1,
            draft: "same", draftSourceCodeUnits: units, attachment: photo, visualizationContext: chip,
            unsent: [], rejected: [], outstanding: [pending])
        XCTAssertTrue(archive.draftIsUnconfirmed)
        XCTAssertTrue(archive.visibleOutstanding.isEmpty)
        XCTAssertEqual(archive.outstanding.count, 1, "the original in-flight payload remains retained")
        XCTAssertEqual(archive.draftSourceCodeUnits, units)
        XCTAssertEqual(archive.outstanding.first?.images, [image])
    }

    private func exerciseCancelledBeforeAdmission(side: Bool, rejectionFirst: Bool,
                                                   interveningEdit: Bool) async throws {
        let directory = try root(), ledger = DurableConversationTurnLedger()
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
                                  sessionMode: .code, visionDelegationEnabled: false)
        let source = EngineConversationSource(config: config, recoveryLedger: ledger, memoryOnly: side)
        if side { source.setSideOwnerForTesting(parentSessionID: a, sideID: "side-a") }
        else { source.model.activeSessionId = a }
        let host = SessionHost()
        let firstSource: any ConversationSource = side ? MockConversationSource(sessionMode: .code) : source
        firstSource.model.activeSessionId = a
        let first = try host.retain(source: firstSource, scope: .global, mode: .code,
                                    sessionID: a, workspace: "/a", draft: "input")
        let otherSource = MockConversationSource(sessionMode: .chat)
        otherSource.model.activeSessionId = b
        let other = try host.retain(source: otherSource, scope: .global, mode: .chat,
                                    sessionID: b, workspace: "/b", draft: "other")
        host.select(first)
        let sideEntry = side ? SideChatEntry(source: source, id: "side-a") : nil
        let draft = Binding<String>(get: { sideEntry?.draft ?? first.draft }, set: {
            if let sideEntry { sideEntry.draft = $0 } else { first.draft = $0 }
        })
        let draftUnits = Binding<[UInt16]?>(get: {
            if let sideEntry { return sideEntry.draftSourceCodeUnits }
            return first.draftSourceCodeUnits
        }, set: {
            if let sideEntry { sideEntry.draftSourceCodeUnits = $0 } else { first.draftSourceCodeUnits = $0 }
        })
        let draftAttachment = Binding<ComposerAttachment?>(get: {
            if let sideEntry { return sideEntry.attachment }
            return first.attachment
        }, set: {
            if let sideEntry { sideEntry.attachment = $0 } else { first.attachment = $0 }
        })
        let revision: () -> UInt64? = { sideEntry?.draftRevision ?? first.draftRevision }
        draft.wrappedValue = "input"
        let units: [UInt16] = [0x0069, 0x006e, 0x0070, 0x0075, 0x0074, 0xd800]
        draftUnits.wrappedValue = units
        let photo = ComposerAttachment(image: UIImage(), width: 1, height: 1, base64: "cA==")
        draftAttachment.wrappedValue = photo
        let image = ImageRefDto(mediaType: photo.mediaType, base64: photo.base64)
        let chip = VisualizationContextChip(id: "chart", revision: 1, title: "Chart")
        source.model.visualizationChip = chip
        let chipRevision = source.model.visualizationChipRevision
        let captured = CapturedComposerDraft(
            draft: draft, units: draftUnits, attachment: draftAttachment,
            submittedUnits: units, revision: revision)
        let dispatched = expectation(description: "original send entered the controlled transport")
        let cancelStarted = expectation(description: "exact cancel entered the controlled transport")
        let proofArrived = expectation(description: "original send returned its exact rejection")
        let cancelAcknowledged = expectation(description: "exact cancel returned successfully")
        var releaseSend: CheckedContinuation<Void, Never>?
        var releaseCancel: CheckedContinuation<Void, Never>?
        var promptCount = 0
        source.setCommandSubmitterForTesting { command in
            switch command {
            case .sendPrompt:
                promptCount += 1
                await withCheckedContinuation { releaseSend = $0; dispatched.fulfill() }
                throw ClientError.Rejected(message: "turn_cancelled_before_admission")
            case .cancel:
                await withCheckedContinuation { releaseCancel = $0; cancelStarted.fulfill() }
            default: break
            }
        }
        source.setNoAdmissionProofObserverForTesting { _ in proofArrived.fulfill() }
        source.setCancelSubmissionObserverForTesting { _ in cancelAcknowledged.fulfill() }
        let pending = Task { await source.send("input", sourceCodeUnits: units, images: [image], selectedSlashCommand: nil) }
        await fulfillment(of: [dispatched], timeout: 2)
        let submitted = await pending.value
        let token = try XCTUnwrap(submitted)
        XCTAssertTrue(captured.clearIfUnchanged())
        let clearRevision = revision()
        source.registerRejectedPromptRestorer(for: token) { submitted in
            captured.restoreIfUnchanged(afterClearRevision: clearRevision, model: source.model,
                                        chip: chip, chipRevision: chipRevision, submitted: submitted)
        }
        host.select(other)
        if interveningEdit {
            draft.wrappedValue = "another"
            draft.wrappedValue = ""
            source.model.visualizationChip = chip
            source.model.visualizationChip = nil
        }
        var stopCompleted = false
        let stop = Task { try await source.cancelAndWait(); stopCompleted = true }
        await fulfillment(of: [cancelStarted], timeout: 2)
        if rejectionFirst {
            releaseSend?.resume()
            await fulfillment(of: [proofArrived], timeout: 2)
            XCTAssertFalse(stopCompleted)
            XCTAssertTrue(source.model.unsentPrompts.isEmpty)
            XCTAssertEqual(source.model.messages.filter { $0.role == .user }.count, 1)
            releaseCancel?.resume()
            await fulfillment(of: [cancelAcknowledged], timeout: 2)
        } else {
            releaseCancel?.resume()
            await fulfillment(of: [cancelAcknowledged], timeout: 2)
            XCTAssertFalse(stopCompleted)
            XCTAssertTrue(source.model.unsentPrompts.isEmpty)
            XCTAssertEqual(source.model.messages.filter { $0.role == .user }.count, 1)
            releaseSend?.resume()
            await fulfillment(of: [proofArrived], timeout: 2)
        }
        try await stop.value
        XCTAssertTrue(stopCompleted)
        XCTAssertEqual(promptCount, 1)
        XCTAssertTrue(source.model.messages.filter { $0.role == .user }.isEmpty)
        XCTAssertFalse(source.model.streaming)
        XCTAssertEqual(host.selectedEntryID, other.id)
        XCTAssertEqual(other.draft, "other")
        if interveningEdit {
            XCTAssertEqual(draft.wrappedValue, "")
            XCTAssertNil(draftAttachment.wrappedValue)
            XCTAssertNil(source.model.visualizationChip)
            let unsent = try XCTUnwrap(source.model.unsentPrompts.first)
            XCTAssertEqual(source.model.unsentPrompts.count, 1)
            XCTAssertEqual(unsent.token, token)
            XCTAssertEqual(unsent.sessionID, a.lowercased())
            XCTAssertEqual(unsent.sideID, side ? "side-a" : nil)
            XCTAssertEqual(unsent.text, "input")
            XCTAssertEqual(unsent.sourceCodeUnits, units)
            XCTAssertEqual(unsent.images, [image])
            XCTAssertEqual(unsent.visualizationContext, chip)
        } else {
            XCTAssertEqual(draft.wrappedValue, "input")
            XCTAssertEqual(draftUnits.wrappedValue, units)
            XCTAssertEqual(draftAttachment.wrappedValue, photo)
            XCTAssertEqual(source.model.visualizationChip, chip)
            XCTAssertTrue(source.model.unsentPrompts.isEmpty)
        }
        if !side {
            await source.waitForDurableCleanupForTesting()
            let checkpoint = try await ledger.load(url: ledgerURL(directory), sessionID: a)
            XCTAssertNil(checkpoint)
        }
    }
    #endif

    func testRemoteAndSideDraftRevisionsDetectRestoredValues() {
        let remote = RemoteDraftOwner()
        let remoteRevision = remote.draftRevision
        remote.draft.text = "new"
        remote.draft.text = ""
        XCTAssertNotEqual(remote.draftRevision, remoteRevision)
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "", appSandboxRoot: "",
            sessionMode: .code, visionDelegationEnabled: false), memoryOnly: true)
        let side = SideChatEntry(source: source, id: "side")
        let sideRevision = side.draftRevision
        side.draftSourceCodeUnits = [65]
        side.draftSourceCodeUnits = nil
        XCTAssertNotEqual(side.draftRevision, sideRevision)
    }
}
#endif
