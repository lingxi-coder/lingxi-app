import Foundation
import SwiftUI
import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

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
    func block() {
        condition.lock(); defer { condition.unlock() }
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
        let zero = CostDto(totalUsd: 0, inputTokens: 0, outputTokens: 0, apiCalls: 0, sessionDurationSecs: 0, formatted: "$0.00")
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
