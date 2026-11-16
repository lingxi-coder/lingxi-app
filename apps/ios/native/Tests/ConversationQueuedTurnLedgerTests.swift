import Foundation
import os
import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI
#endif

private final class QueuedLedgerFailureSwitch: Sendable {
    enum Mode: Sendable { case none, beforeLedger, afterLedger, beforeWitness }
    private let value = OSAllocatedUnfairLock(initialState: Mode.none)
    func set(_ mode: Mode) { value.withLock { $0 = mode } }
    func mode() -> Mode { value.withLock { $0 } }
}

private struct QueuedLedgerFailureWriter: ProjectAtomicWriting {
    let failure: QueuedLedgerFailureSwitch
    func write<T: Codable>(_ value: T, to url: URL, encoder: JSONEncoder, decoder: JSONDecoder) throws {
        try DefaultProjectAtomicWriter().write(value, to: url, encoder: encoder, decoder: decoder)
    }
    func writeData(_ data: Data, to url: URL, validate: (Data) throws -> Void) throws {
        try DefaultProjectAtomicWriter().writeData(data, to: url, validate: validate)
    }
    func writeData(_ data: Data, to url: URL, expected: OwnerCatalogExpectation,
                   validate: (Data) throws -> Void,
                   commit: (_ replace: () throws -> Void) throws -> Void) throws {
        let mode = failure.mode()
        let isLedger = url.lastPathComponent == "durable-conversation-turns.json"
        if isLedger, mode == .beforeLedger { throw POSIXError(.EACCES) }
        if url.lastPathComponent == "durable-conversation-turns.witness", mode == .beforeWitness {
            throw POSIXError(.EIO)
        }
        try DefaultProjectAtomicWriter().writeData(data, to: url, expected: expected,
                                                   validate: validate, commit: commit)
        if isLedger, mode == .afterLedger { throw POSIXError(.EIO) }
    }
}

final class ConversationQueuedTurnLedgerTests: XCTestCase {
    private let a = "11111111-1111-4111-8111-111111111111"
    private let b = "22222222-2222-4222-8222-222222222222"

    private func root() throws -> URL {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("queued-ledger-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        return root
    }

    private func ledgerURL(_ root: URL) -> URL { root.appendingPathComponent("durable-conversation-turns.json") }
    private func witnessURL(_ root: URL) -> URL { root.appendingPathComponent("durable-conversation-turns.witness") }

    private func assertFails<T>(_ operation: () async throws -> T,
                                file: StaticString = #filePath, line: UInt = #line) async {
        do { _ = try await operation(); XCTFail("expected ledger operation to fail", file: file, line: line) }
        catch { }
    }

    private func record(_ ledger: DurableConversationTurnLedger, url: URL,
                        sessionID: String) async throws -> DurableConversationTurnRecord {
        let found = try await ledger.state(url: url, sessionID: sessionID)
        return try XCTUnwrap(found)
    }

    func testQueuedTurnsCoexistWithActiveOwnerAndOtherSessions() async throws {
        let directory = try root(), url = ledgerURL(directory), ledger = DurableConversationTurnLedger()
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try await ledger.begin(url: url, sessionID: b, turnID: 21, scope: "/b")
        let marker = try Data(contentsOf: witnessURL(directory))
        try await ledger.enqueue(url: url, sessionID: a.uppercased(), ownerTurnID: 11, turnID: 12)
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 13)
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
        let active = try await record(ledger, url: url, sessionID: a)
        let other = try await record(ledger, url: url, sessionID: b)
        XCTAssertEqual(active, .init(scope: "/a", sessionID: a, turnID: 11, lastSequence: 0,
                                     queuedTurnIDs: [12, 13]))
        XCTAssertEqual(other.turnID, 21)
        XCTAssertTrue(other.queuedTurnIDs.isEmpty)
        let contents = try Data(contentsOf: url)
        await assertFails { try await ledger.enqueue(url: url, sessionID: self.a, ownerTurnID: 21, turnID: 14) }
        await assertFails { try await ledger.enqueue(url: url, sessionID: self.a, ownerTurnID: 11, turnID: 11) }
        XCTAssertEqual(try Data(contentsOf: url), contents)
        XCTAssertEqual(try Data(contentsOf: witnessURL(directory)), marker)
    }

    func testTerminalHandoffKeepsAnchorAndRejectsStalePredecessorClear() async throws {
        let directory = try root(), url = ledgerURL(directory), ledger = DurableConversationTurnLedger()
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 13)
        let marker = try Data(contentsOf: witnessURL(directory))
        await assertFails { try await ledger.begin(url: url, sessionID: self.a, turnID: 12, scope: "/a") }
        try await ledger.clear(url: url, sessionID: a, turnID: 11)
        let inactive = try await record(ledger, url: url, sessionID: a)
        let loaded = try await ledger.load(url: url, sessionID: a)
        XCTAssertFalse(inactive.isActive)
        XCTAssertEqual(inactive.queuedTurnIDs, [12, 13])
        XCTAssertNil(loaded)
        let waitingBytes = try Data(contentsOf: url)
        await assertFails { try await ledger.enqueue(url: url, sessionID: self.a, ownerTurnID: 11, turnID: 14) }
        XCTAssertEqual(try Data(contentsOf: url), waitingBytes)
        await assertFails { try await ledger.begin(url: url, sessionID: self.a, turnID: 99, scope: "/a") }
        try await ledger.begin(url: url, sessionID: a, turnID: 12, scope: "/a")
        let successor = try await record(ledger, url: url, sessionID: a)
        XCTAssertEqual(successor.anchorTurnID, 11)
        XCTAssertTrue(successor.isActive)
        XCTAssertEqual(successor.turnID, 12)
        XCTAssertEqual(successor.queuedTurnIDs, [13])
        let contents = try Data(contentsOf: url)
        try await ledger.removeQueued(url: url, sessionID: a, turnID: 12)
        try await ledger.removeQueued(url: url, sessionID: a, turnID: 12)
        await assertFails { try await ledger.clear(url: url, sessionID: self.a, turnID: 11) }
        XCTAssertEqual(try Data(contentsOf: url), contents)
        XCTAssertEqual(try Data(contentsOf: witnessURL(directory)), marker)
        try await ledger.clear(url: url, sessionID: a, turnID: 12)
        try await ledger.begin(url: url, sessionID: a, turnID: 13, scope: "/a")
        try await ledger.clear(url: url, sessionID: a, turnID: 13)
        let empty = try await ledger.state(url: url, sessionID: a)
        XCTAssertNil(empty)
        try await ledger.begin(url: url, sessionID: a, turnID: 14, scope: "/a")
        let next = try await record(ledger, url: url, sessionID: a)
        XCTAssertEqual(next.anchorTurnID, 14)
    }

    func testColdReloadRetainsActiveAndWaitingSuccessors() async throws {
        let directory = try root(), url = ledgerURL(directory), initial = DurableConversationTurnLedger()
        try await initial.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try await initial.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
        let reopened = DurableConversationTurnLedger()
        let active = try await record(reopened, url: url, sessionID: a)
        XCTAssertTrue(active.isActive)
        XCTAssertEqual(active.queuedTurnIDs, [12])
        try await reopened.clear(url: url, sessionID: a, turnID: 11)
        let waitingLedger = DurableConversationTurnLedger()
        let waiting = try await record(waitingLedger, url: url, sessionID: a)
        XCTAssertFalse(waiting.isActive)
        XCTAssertEqual(waiting.anchorTurnID, 11)
        let loaded = try await waitingLedger.load(url: url, sessionID: a)
        XCTAssertNil(loaded)
        try await waitingLedger.begin(url: url, sessionID: a, turnID: 12, scope: "/a")
        let current = try await record(DurableConversationTurnLedger(), url: url, sessionID: a)
        XCTAssertEqual(current.turnID, 12)
        XCTAssertEqual(current.anchorTurnID, 11)
        XCTAssertTrue(current.isActive)
        XCTAssertTrue(current.queuedTurnIDs.isEmpty)
    }

    func testDeferredActiveOwnerSurvivesColdSuccessorAndReturnsToQueue() async throws {
        let directory = try root(), url = ledgerURL(directory), ledger = DurableConversationTurnLedger()
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
        let marker = try Data(contentsOf: witnessURL(directory))
        try await ledger.deferActive(url: url, sessionID: a, turnID: 11)
        let contents = try Data(contentsOf: url)
        try await ledger.deferActive(url: url, sessionID: a, turnID: 11)
        XCTAssertEqual(try Data(contentsOf: url), contents)
        let cold = DurableConversationTurnLedger()
        let waiting = try await record(cold, url: url, sessionID: a)
        let loaded = try await cold.load(url: url, sessionID: a)
        XCTAssertFalse(waiting.isActive)
        XCTAssertEqual(waiting.queuedTurnIDs, [11, 12])
        XCTAssertNil(loaded)
        try await cold.begin(url: url, sessionID: a, turnID: 12, scope: "/a")
        let successor = try await record(cold, url: url, sessionID: a)
        XCTAssertTrue(successor.isActive)
        XCTAssertEqual(successor.turnID, 12)
        XCTAssertEqual(successor.anchorTurnID, 11)
        XCTAssertEqual(successor.queuedTurnIDs, [11])
        try await cold.clear(url: url, sessionID: a, turnID: 12)
        try await cold.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        let resumed = try await record(DurableConversationTurnLedger(), url: url, sessionID: a)
        XCTAssertTrue(resumed.isActive)
        XCTAssertEqual(resumed.turnID, 11)
        XCTAssertTrue(resumed.queuedTurnIDs.isEmpty)
        XCTAssertEqual(try Data(contentsOf: witnessURL(directory)), marker)
    }

    func testDeferActiveRejectsStaleOrTerminalOwnerWithoutMutation() async throws {
        let directory = try root(), url = ledgerURL(directory), ledger = DurableConversationTurnLedger()
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
        let active = try Data(contentsOf: url)
        await assertFails { try await ledger.deferActive(url: url, sessionID: self.a, turnID: 99) }
        XCTAssertEqual(try Data(contentsOf: url), active)
        try await ledger.clear(url: url, sessionID: a, turnID: 11)
        let terminal = try Data(contentsOf: url)
        await assertFails { try await ledger.deferActive(url: url, sessionID: self.a, turnID: 11) }
        XCTAssertEqual(try Data(contentsOf: url), terminal)
        try await ledger.begin(url: url, sessionID: a, turnID: 12, scope: "/a")
        let successor = try Data(contentsOf: url)
        await assertFails { try await ledger.deferActive(url: url, sessionID: self.a, turnID: 11) }
        XCTAssertEqual(try Data(contentsOf: url), successor)
    }

    func testDeferredPublicationFailureKeepsOneRecoverableQueueOwner() async throws {
        for mode in [QueuedLedgerFailureSwitch.Mode.beforeLedger, .afterLedger] {
            let directory = try root(), url = ledgerURL(directory), failure = QueuedLedgerFailureSwitch()
            let ledger = DurableConversationTurnLedger(writer: QueuedLedgerFailureWriter(failure: failure))
            try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
            try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
            failure.set(mode)
            await assertFails { try await ledger.deferActive(url: url, sessionID: self.a, turnID: 11) }
            let current = try await record(DurableConversationTurnLedger(), url: url, sessionID: a)
            XCTAssertEqual(current.isActive, mode == .beforeLedger)
            XCTAssertEqual(current.queuedTurnIDs, mode == .beforeLedger ? [12] : [11, 12])
            failure.set(.none)
            try await ledger.deferActive(url: url, sessionID: a, turnID: 11)
            try await ledger.deferActive(url: url, sessionID: a, turnID: 11)
            try await ledger.begin(url: url, sessionID: a, turnID: 12, scope: "/a")
            let successor = try await record(ledger, url: url, sessionID: a)
            XCTAssertEqual(successor.queuedTurnIDs, [11])
        }
    }

    func testKnownCoalescedMemberRestoresItsOwnerBeforeColdClaim() async throws {
        let directory = try root(), url = ledgerURL(directory), ledger = DurableConversationTurnLedger()
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 13)
        try await ledger.clear(url: url, sessionID: a, turnID: 11)
        let marker = try Data(contentsOf: witnessURL(directory))
        let memberOnly = try Data(contentsOf: url)
        try await ledger.adoptQueuedOwner(url: url, sessionID: a, memberTurnID: 13, ownerTurnID: 13)
        XCTAssertEqual(try Data(contentsOf: url), memberOnly)
        try await ledger.adoptQueuedOwner(url: url, sessionID: a, memberTurnID: 13, ownerTurnID: 12)
        let contents = try Data(contentsOf: url)
        try await ledger.adoptQueuedOwner(url: url, sessionID: a, memberTurnID: 13, ownerTurnID: 12)
        XCTAssertEqual(try Data(contentsOf: url), contents)
        let cold = DurableConversationTurnLedger()
        let waiting = try await record(cold, url: url, sessionID: a)
        XCTAssertFalse(waiting.isActive)
        XCTAssertEqual(waiting.anchorTurnID, 11)
        XCTAssertEqual(waiting.queuedTurnIDs, [12, 13])
        try await cold.begin(url: url, sessionID: a, turnID: 12, scope: "/a")
        let claimed = try await record(cold, url: url, sessionID: a)
        XCTAssertTrue(claimed.isActive)
        XCTAssertEqual(claimed.turnID, 12)
        XCTAssertEqual(claimed.queuedTurnIDs, [13])
        XCTAssertEqual(try Data(contentsOf: witnessURL(directory)), marker)
    }

    func testCoalescedOwnerAdoptionPreservesActiveOwnerAndRejectsStaleMembers() async throws {
        let directory = try root(), url = ledgerURL(directory), ledger = DurableConversationTurnLedger()
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try await ledger.begin(url: url, sessionID: b, turnID: 21, scope: "/b")
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 13)
        try await ledger.adoptQueuedOwner(url: url, sessionID: a, memberTurnID: 13, ownerTurnID: 12)
        let active = try await record(ledger, url: url, sessionID: a)
        let other = try await record(ledger, url: url, sessionID: b)
        XCTAssertEqual(active, .init(scope: "/a", sessionID: a, turnID: 11, lastSequence: 0,
                                     queuedTurnIDs: [12, 13]))
        XCTAssertEqual(other, .init(scope: "/b", sessionID: b, turnID: 21, lastSequence: 0))
        let contents = try Data(contentsOf: url), marker = try Data(contentsOf: witnessURL(directory))
        await assertFails { try await ledger.adoptQueuedOwner(url: url, sessionID: self.a, memberTurnID: 13, ownerTurnID: 11) }
        await assertFails { try await ledger.adoptQueuedOwner(url: url, sessionID: self.a, memberTurnID: 99, ownerTurnID: 12) }
        await assertFails { try await ledger.adoptQueuedOwner(url: url, sessionID: self.b, memberTurnID: 13, ownerTurnID: 12) }
        XCTAssertEqual(try Data(contentsOf: url), contents)
        XCTAssertEqual(try Data(contentsOf: witnessURL(directory)), marker)
        try await ledger.removeQueued(url: url, sessionID: a, turnID: 13)
        let retired = try Data(contentsOf: url)
        await assertFails { try await ledger.adoptQueuedOwner(url: url, sessionID: self.a, memberTurnID: 13, ownerTurnID: 14) }
        XCTAssertEqual(try Data(contentsOf: url), retired)
    }

    func testCoalescedOwnerAdoptionRetriesUncertainPublicationWithoutDuplicateIDs() async throws {
        for mode in [QueuedLedgerFailureSwitch.Mode.beforeLedger, .afterLedger] {
            let directory = try root(), url = ledgerURL(directory), failure = QueuedLedgerFailureSwitch()
            let ledger = DurableConversationTurnLedger(writer: QueuedLedgerFailureWriter(failure: failure))
            try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
            try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 13)
            try await ledger.clear(url: url, sessionID: a, turnID: 11)
            let marker = try Data(contentsOf: witnessURL(directory))
            failure.set(mode)
            await assertFails { try await ledger.adoptQueuedOwner(url: url, sessionID: self.a, memberTurnID: 13, ownerTurnID: 12) }
            let found = try await record(DurableConversationTurnLedger(), url: url, sessionID: a)
            XCTAssertEqual(found.queuedTurnIDs, mode == .beforeLedger ? [13] : [12, 13])
            XCTAssertFalse(found.isActive)
            XCTAssertEqual(try Data(contentsOf: witnessURL(directory)), marker)
            failure.set(.none)
            try await ledger.adoptQueuedOwner(url: url, sessionID: a, memberTurnID: 13, ownerTurnID: 12)
            let contents = try Data(contentsOf: url)
            try await ledger.adoptQueuedOwner(url: url, sessionID: a, memberTurnID: 13, ownerTurnID: 12)
            XCTAssertEqual(try Data(contentsOf: url), contents)
            try await ledger.begin(url: url, sessionID: a, turnID: 12, scope: "/a")
            let claimed = try await record(ledger, url: url, sessionID: a)
            XCTAssertEqual(claimed.queuedTurnIDs, [13])
        }
    }

    func testPhantomRemovalIsIdempotentAndRetiresOnlyEmptyInactiveOwner() async throws {
        let directory = try root(), url = ledgerURL(directory), ledger = DurableConversationTurnLedger()
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try await ledger.begin(url: url, sessionID: b, turnID: 21, scope: "/b")
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 13)
        try await ledger.removeQueued(url: url, sessionID: a, turnID: 12)
        try await ledger.removeQueued(url: url, sessionID: a, turnID: 12)
        try await ledger.removeQueued(url: url, sessionID: a, turnID: 11)
        let active = try await record(ledger, url: url, sessionID: a)
        XCTAssertTrue(active.isActive)
        XCTAssertEqual(active.queuedTurnIDs, [13])
        try await ledger.clear(url: url, sessionID: a, turnID: 11)
        try await ledger.removeQueued(url: url, sessionID: a, turnID: 13)
        try await ledger.removeQueued(url: url, sessionID: a, turnID: 13)
        let retired = try await ledger.state(url: url, sessionID: a)
        let other = try await record(ledger, url: url, sessionID: b)
        XCTAssertNil(retired)
        XCTAssertEqual(other.turnID, 21)
        try await ledger.begin(url: url, sessionID: a, turnID: 14, scope: "/a")
    }

    func testQueueWriteFailurePreservesOwnerAndUncertainPublishCanBePruned() async throws {
        for mode in [QueuedLedgerFailureSwitch.Mode.beforeLedger, .afterLedger] {
            let directory = try root(), url = ledgerURL(directory), failure = QueuedLedgerFailureSwitch()
            let ledger = DurableConversationTurnLedger(writer: QueuedLedgerFailureWriter(failure: failure))
            try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
            let marker = try Data(contentsOf: witnessURL(directory))
            failure.set(mode)
            await assertFails { try await ledger.enqueue(url: url, sessionID: self.a, ownerTurnID: 11, turnID: 12) }
            let current = try await record(DurableConversationTurnLedger(), url: url, sessionID: a)
            XCTAssertTrue(current.isActive)
            XCTAssertEqual(current.turnID, 11)
            XCTAssertEqual(current.queuedTurnIDs, mode == .afterLedger ? [12] : [])
            XCTAssertEqual(try Data(contentsOf: witnessURL(directory)), marker)
            failure.set(.none)
            try await ledger.removeQueued(url: url, sessionID: a, turnID: 12)
            try await ledger.removeQueued(url: url, sessionID: a, turnID: 12)
            try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
            try await ledger.clear(url: url, sessionID: a, turnID: 11)
            try await ledger.begin(url: url, sessionID: a, turnID: 12, scope: "/a")
        }
    }

    func testSuccessorClaimFailureKeepsOneOwnerWithoutColdDuplicate() async throws {
        for mode in [QueuedLedgerFailureSwitch.Mode.beforeLedger, .afterLedger] {
            let directory = try root(), url = ledgerURL(directory), failure = QueuedLedgerFailureSwitch()
            let ledger = DurableConversationTurnLedger(writer: QueuedLedgerFailureWriter(failure: failure))
            try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
            try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
            try await ledger.clear(url: url, sessionID: a, turnID: 11)
            let marker = try Data(contentsOf: witnessURL(directory))
            failure.set(mode)
            await assertFails { try await ledger.begin(url: url, sessionID: self.a, turnID: 12, scope: "/a") }
            failure.set(.none)
            let found = try await record(DurableConversationTurnLedger(), url: url, sessionID: a)
            if mode == .beforeLedger {
                XCTAssertFalse(found.isActive)
                XCTAssertEqual(found.queuedTurnIDs, [12])
                try await ledger.begin(url: url, sessionID: a, turnID: 12, scope: "/a")
            } else {
                XCTAssertTrue(found.isActive)
                XCTAssertEqual(found.turnID, 12)
                XCTAssertTrue(found.queuedTurnIDs.isEmpty)
                await assertFails { try await ledger.begin(url: url, sessionID: self.a, turnID: 12, scope: "/a") }
            }
            XCTAssertEqual(try Data(contentsOf: witnessURL(directory)), marker)
            try await ledger.clear(url: url, sessionID: a, turnID: 12)
            try await ledger.begin(url: url, sessionID: a, turnID: 13, scope: "/a")
        }
    }

    func testFailedTerminalHandoffMustRetryBeforeSuccessorClaim() async throws {
        for mode in [QueuedLedgerFailureSwitch.Mode.beforeLedger, .afterLedger] {
            let directory = try root(), url = ledgerURL(directory), failure = QueuedLedgerFailureSwitch()
            let ledger = DurableConversationTurnLedger(writer: QueuedLedgerFailureWriter(failure: failure))
            try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
            try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
            failure.set(mode)
            await assertFails { try await ledger.clear(url: url, sessionID: self.a, turnID: 11) }
            failure.set(.none)
            await assertFails { try await ledger.begin(url: url, sessionID: self.a, turnID: 12, scope: "/a") }
            try await ledger.clear(url: url, sessionID: a, turnID: 11)
            try await ledger.begin(url: url, sessionID: a, turnID: 12, scope: "/a")
            let current = try await record(ledger, url: url, sessionID: a)
            XCTAssertEqual(current.turnID, 12)
            XCTAssertEqual(current.anchorTurnID, 11)
        }
    }

    func testLastPhantomRemovalWitnessFailureRequiresExactClearReceipt() async throws {
        let directory = try root(), url = ledgerURL(directory), failure = QueuedLedgerFailureSwitch()
        let ledger = DurableConversationTurnLedger(writer: QueuedLedgerFailureWriter(failure: failure))
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try await ledger.begin(url: url, sessionID: b, turnID: 21, scope: "/b")
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
        try await ledger.clear(url: url, sessionID: a, turnID: 11)
        failure.set(.beforeWitness)
        await assertFails { try await ledger.removeQueued(url: url, sessionID: self.a, turnID: 12) }
        await assertFails { try await DurableConversationTurnLedger().state(url: url, sessionID: self.a) }
        failure.set(.none)
        try await ledger.removeQueued(url: url, sessionID: a, turnID: 12)
        try await ledger.removeQueued(url: url, sessionID: a, turnID: 12)
        let retired = try await ledger.state(url: url, sessionID: a)
        let other = try await record(ledger, url: url, sessionID: b)
        XCTAssertNil(retired)
        XCTAssertEqual(other.turnID, 21)
    }

    func testUncertainLastPhantomDeletionCannotBecomeAColdRetry() async throws {
        let directory = try root(), url = ledgerURL(directory), failure = QueuedLedgerFailureSwitch()
        let ledger = DurableConversationTurnLedger(writer: QueuedLedgerFailureWriter(failure: failure))
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: 12)
        try await ledger.clear(url: url, sessionID: a, turnID: 11)
        failure.set(.afterLedger)
        await assertFails { try await ledger.removeQueued(url: url, sessionID: self.a, turnID: 12) }
        let contents = try Data(contentsOf: url), marker = try Data(contentsOf: witnessURL(directory))
        failure.set(.none)
        await assertFails { try await ledger.removeQueued(url: url, sessionID: self.a, turnID: 12) }
        await assertFails { try await DurableConversationTurnLedger().state(url: url, sessionID: self.a) }
        XCTAssertEqual(try Data(contentsOf: url), contents)
        XCTAssertEqual(try Data(contentsOf: witnessURL(directory)), marker)
    }

    func testConcurrentQueueAndSuccessorClaimsPreserveSessionIsolation() async throws {
        let directory = try root(), url = ledgerURL(directory), ledger = DurableConversationTurnLedger()
        let a = self.a, b = self.b
        async let first: Void = ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        async let second: Void = ledger.begin(url: url, sessionID: b, turnID: 21, scope: "/b")
        _ = try await (first, second)
        try await withThrowingTaskGroup(of: Void.self) { group in
            for turnID in UInt64(100)..<120 {
                group.addTask { try await ledger.enqueue(url: url, sessionID: a, ownerTurnID: 11, turnID: turnID) }
                group.addTask { try await ledger.enqueue(url: url, sessionID: b, ownerTurnID: 21, turnID: turnID + 100) }
            }
            try await group.waitForAll()
        }
        let active = try await record(ledger, url: url, sessionID: a)
        let other = try await record(ledger, url: url, sessionID: b)
        XCTAssertEqual(Set(active.queuedTurnIDs), Set(UInt64(100)..<120))
        XCTAssertEqual(Set(other.queuedTurnIDs), Set(UInt64(200)..<220))
        try await ledger.clear(url: url, sessionID: a, turnID: 11)
        let claims = await withTaskGroup(of: Bool.self, returning: Int.self) { group in
            for _ in 0..<2 {
                group.addTask {
                    do { try await ledger.begin(url: url, sessionID: a, turnID: 100, scope: "/a"); return true }
                    catch { return false }
                }
            }
            var successes = 0
            for await claimed in group { if claimed { successes += 1 } }
            return successes
        }
        XCTAssertEqual(claims, 1)
        let unchanged = try await record(ledger, url: url, sessionID: b)
        XCTAssertEqual(unchanged, other)
    }

    func testMalformedQueueOrAnchorRemainsFailClosed() async throws {
        for damage in ["anchor", "duplicate", "inactive_duplicate", "active_in_queue", "empty_inactive", "null_queue", "invalid_active", "null_anchor"] {
            let directory = try root(), url = ledgerURL(directory), ledger = DurableConversationTurnLedger()
            try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
            let marker = try Data(contentsOf: witnessURL(directory))
            var records = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [[String: Any]])
            switch damage {
            case "anchor": records[0]["anchorTurnID"] = 999
            case "duplicate": records[0]["queuedTurnIDs"] = [12, 12]
            case "inactive_duplicate":
                records[0]["isActive"] = false
                records[0]["queuedTurnIDs"] = [11, 11]
            case "active_in_queue": records[0]["queuedTurnIDs"] = [11]
            case "empty_inactive": records[0]["isActive"] = false
            case "null_queue": records[0]["queuedTurnIDs"] = NSNull()
            case "invalid_active": records[0]["isActive"] = "false"
            default: records[0]["anchorTurnID"] = NSNull()
            }
            let contents = try JSONSerialization.data(withJSONObject: records)
            try contents.write(to: url, options: .atomic)
            let cold = DurableConversationTurnLedger()
            await assertFails { try await cold.state(url: url, sessionID: self.a) }
            await assertFails { try await cold.enqueue(url: url, sessionID: self.a, ownerTurnID: 11, turnID: 12) }
            await assertFails { try await cold.begin(url: url, sessionID: self.b, turnID: 21, scope: "/b") }
            XCTAssertEqual(try Data(contentsOf: url), contents)
            XCTAssertEqual(try Data(contentsOf: witnessURL(directory)), marker)
        }
    }

    func testExistingActiveRecordDefaultsRemainReadable() async throws {
        let directory = try root(), url = ledgerURL(directory), ledger = DurableConversationTurnLedger()
        try await ledger.begin(url: url, sessionID: a, turnID: 11, scope: "/a")
        let oldFields: [[String: Any]] = [["scope": "/a", "sessionID": a, "turnID": 11, "lastSequence": 0]]
        try JSONSerialization.data(withJSONObject: oldFields).write(to: url, options: .atomic)
        let current = try await record(DurableConversationTurnLedger(), url: url, sessionID: a)
        XCTAssertEqual(current, .init(scope: "/a", sessionID: a, turnID: 11, lastSequence: 0))
        XCTAssertEqual(current.anchorTurnID, 11)
        XCTAssertTrue(current.isActive)
        XCTAssertTrue(current.queuedTurnIDs.isEmpty)
    }

    #if canImport(harness_runtimeFFI)
    @MainActor
    func testClientStoreExposesInactiveQueueAndOnlyWarmActiveCursor() async throws {
        let directory = try root(), ledger = DurableConversationTurnLedger()
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
                                  sessionMode: .code, visionDelegationEnabled: false, initialSessionId: a)
        let store = DurableConversationTurnClientStore(config: config, ledger: ledger)
        try await store.begin(sessionID: a, turnID: 11)
        store.updateSequence(turnID: 11, sequence: 7)
        try await store.enqueue(sessionID: a, ownerTurnID: 11, turnID: 12)
        let active = try await store.load()
        XCTAssertEqual(active?.lastSequence, 7)
        try await store.clear(turnID: 11, sessionID: a)
        let inactive = try await store.state()
        let loaded = try await store.load()
        let queued = try await store.queuedTurnIDs()
        XCTAssertNil(loaded)
        XCTAssertEqual(inactive?.isActive, false)
        XCTAssertEqual(inactive?.lastSequence, 0)
        XCTAssertEqual(queued, [12])
        try await store.begin(sessionID: a, turnID: 12)
        store.updateSequence(turnID: 12, sequence: 3)
        let successor = try await store.load()
        let cold = DurableConversationTurnClientStore(config: config, ledger: DurableConversationTurnLedger())
        let coldRecord = try await cold.load()
        XCTAssertEqual(successor?.lastSequence, 3)
        XCTAssertEqual(successor?.anchorTurnID, 11)
        XCTAssertEqual(coldRecord?.turnID, 12)
        XCTAssertEqual(coldRecord?.lastSequence, 0)
    }

    @MainActor
    func testClientStoreExplicitQueueOwnerDoesNotRebindSelectedSession() async throws {
        let directory = try root(), ledger = DurableConversationTurnLedger()
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
                                  sessionMode: .code, visionDelegationEnabled: false, initialSessionId: a)
        let store = DurableConversationTurnClientStore(config: config, ledger: ledger)
        try await store.begin(sessionID: a, turnID: 11)
        try await store.begin(sessionID: b, turnID: 21)
        store.updateSequence(turnID: 21, sequence: 5)
        try await store.enqueue(sessionID: a, ownerTurnID: 11, turnID: 12)
        try await store.adoptQueuedOwner(sessionID: a, memberTurnID: 12, ownerTurnID: 13)
        let queued = try await store.queuedTurnIDs(sessionID: a)
        let inactiveBinding = try await store.state(sessionID: a)
        XCTAssertEqual(queued, [13, 12])
        XCTAssertEqual(inactiveBinding?.lastSequence, 0)
        try await store.removeQueued(turnID: 13, sessionID: a)
        try await store.removeQueued(turnID: 12, sessionID: a)
        try await store.clear(turnID: 11, sessionID: a)
        let selected = try await store.load()
        XCTAssertEqual(selected?.sessionID, b)
        XCTAssertEqual(selected?.turnID, 21)
        XCTAssertEqual(selected?.lastSequence, 5)
    }

    @MainActor
    func testClientStoreDeferredOwnerStaysQueuedWithoutWarmCursor() async throws {
        let directory = try root(), ledger = DurableConversationTurnLedger()
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: directory.path,
                                  sessionMode: .code, visionDelegationEnabled: false, initialSessionId: a)
        let store = DurableConversationTurnClientStore(config: config, ledger: ledger)
        try await store.begin(sessionID: a, turnID: 11)
        store.updateSequence(turnID: 11, sequence: 7)
        try await store.enqueue(sessionID: a, ownerTurnID: 11, turnID: 12)
        try await store.deferActive(turnID: 11, sessionID: a)
        let waiting = try await store.state()
        let loaded = try await store.load()
        XCTAssertNil(loaded)
        XCTAssertEqual(waiting?.queuedTurnIDs, [11, 12])
        XCTAssertEqual(waiting?.lastSequence, 0)
        try await store.begin(sessionID: a, turnID: 12)
        try await store.clear(turnID: 12, sessionID: a)
        try await store.begin(sessionID: a, turnID: 11)
        let resumed = try await store.load()
        XCTAssertEqual(resumed?.turnID, 11)
        XCTAssertEqual(resumed?.lastSequence, 0)
    }
    #endif
}
