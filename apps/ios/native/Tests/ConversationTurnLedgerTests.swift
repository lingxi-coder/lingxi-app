import Foundation
import XCTest
@testable import LingxiCode

private final class LedgerFailureSwitch: @unchecked Sendable {
    enum Mode { case none, beforeLedger, afterLedger, beforeRollbackWitness }
    private let lock = NSLock()
    private var current: Mode = .none
    func set(_ mode: Mode) { lock.lock(); defer { lock.unlock() }; current = mode }
    func mode() -> Mode { lock.lock(); defer { lock.unlock() }; return current }
}

private struct LedgerFailureWriter: ProjectAtomicWriting {
    let failure: LedgerFailureSwitch
    func write<T: Codable>(_ value: T, to url: URL, encoder: JSONEncoder, decoder: JSONDecoder) throws {
        try DefaultProjectAtomicWriter().write(value, to: url, encoder: encoder, decoder: decoder)
    }
    func writeData(_ data: Data, to url: URL, validate: (Data) throws -> Void) throws {
        try DefaultProjectAtomicWriter().writeData(data, to: url, validate: validate)
    }
    func writeData(_ data: Data, to url: URL, expected: OwnerCatalogExpectation,
                   validate: (Data) throws -> Void,
                   commit: (_ replace: () throws -> Void) throws -> Void) throws {
        let isLedger = url.lastPathComponent == "durable-conversation-turns.json"
        let isMarker = url.lastPathComponent == "durable-conversation-turns.witness"
        let mode = failure.mode()
        if isLedger, mode == .beforeLedger { throw POSIXError(.EACCES) }
        if isMarker, mode == .beforeRollbackWitness { throw POSIXError(.EIO) }
        try DefaultProjectAtomicWriter().writeData(data, to: url, expected: expected, validate: validate, commit: commit)
        if isLedger, mode == .afterLedger { throw POSIXError(.EIO) }
    }
}

final class ConversationTurnLedgerTests: XCTestCase {
    private let a = "11111111-1111-4111-8111-111111111111"
    private let b = "22222222-2222-4222-8222-222222222222"

    private func makeRoot() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("ledger-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        return root
    }

    private func assertFails(_ operation: () async throws -> Void, file: StaticString = #filePath, line: UInt = #line) async {
        do { try await operation(); XCTFail("expected operation to fail", file: file, line: line) }
        catch { }
    }

    func testFirstUnpublishedAttemptCanBeReconciledBeforeAnotherOwner() async throws {
        let root = try makeRoot(), url = root.appendingPathComponent("durable-conversation-turns.json")
        let failure = LedgerFailureSwitch()
        let ledger = DurableConversationTurnLedger(writer: LedgerFailureWriter(failure: failure))
        failure.set(.beforeLedger)
        await assertFails { try await ledger.begin(url: url, sessionID: self.a, turnID: 1, scope: "/a") }
        let reopened = DurableConversationTurnLedger()
        await assertFails { _ = try await reopened.load(url: url, sessionID: self.a) }
        failure.set(.none)
        try await ledger.begin(url: url, sessionID: b, turnID: 2, scope: "/b")
        let first = try await ledger.load(url: url, sessionID: a)
        let second = try await ledger.load(url: url, sessionID: b)
        XCTAssertNil(first)
        XCTAssertEqual(second?.turnID, 2)
    }

    func testExistingOwnerSurvivesUnpublishedAttempt() async throws {
        let root = try makeRoot(), url = root.appendingPathComponent("durable-conversation-turns.json")
        let failure = LedgerFailureSwitch()
        let ledger = DurableConversationTurnLedger(writer: LedgerFailureWriter(failure: failure))
        try await ledger.begin(url: url, sessionID: b, turnID: 2, scope: "/b")
        failure.set(.beforeLedger)
        await assertFails { try await ledger.begin(url: url, sessionID: self.a, turnID: 1, scope: "/a") }
        let surviving = try await ledger.load(url: url, sessionID: b)
        XCTAssertEqual(surviving?.turnID, 2)
        failure.set(.none)
        try await ledger.begin(url: url, sessionID: a, turnID: 1, scope: "/a")
        let retried = try await ledger.load(url: url, sessionID: a)
        XCTAssertEqual(retried?.turnID, 1)
    }

    func testUncertainPublicationOrRollbackDoesNotBecomeAColdRetry() async throws {
        for failureDuringRollback in [false, true] {
            let root = try makeRoot(), url = root.appendingPathComponent("durable-conversation-turns.json")
            let marker = root.appendingPathComponent("durable-conversation-turns.witness")
            let failure = LedgerFailureSwitch()
            let ledger = DurableConversationTurnLedger(writer: LedgerFailureWriter(failure: failure))
            failure.set(failureDuringRollback ? .beforeLedger : .afterLedger)
            await assertFails { try await ledger.begin(url: url, sessionID: self.a, turnID: 1, scope: "/a") }
            if failureDuringRollback {
                failure.set(.beforeRollbackWitness)
                await assertFails { try await ledger.begin(url: url, sessionID: self.b, turnID: 2, scope: "/b") }
            }
            let ledgerBytes = try Data(contentsOf: url), markerBytes = try Data(contentsOf: marker)
            failure.set(.none)
            await assertFails { try await ledger.begin(url: url, sessionID: self.b, turnID: 2, scope: "/b") }
            XCTAssertEqual(try Data(contentsOf: url), ledgerBytes)
            XCTAssertEqual(try Data(contentsOf: marker), markerBytes)
            if failureDuringRollback {
                let reopened = DurableConversationTurnLedger()
                await assertFails { _ = try await reopened.load(url: url, sessionID: self.a) }
            }
        }
    }

    func testUnexpectedWitnessOnlyCannotBeLaunderedByRepeatedClear() async throws {
        let root = try makeRoot(), url = root.appendingPathComponent("durable-conversation-turns.json")
        let marker = root.appendingPathComponent("durable-conversation-turns.witness")
        let ledger = DurableConversationTurnLedger()
        try await ledger.begin(url: url, sessionID: a, turnID: 1, scope: "/a")
        try await ledger.begin(url: url, sessionID: b, turnID: 2, scope: "/b")
        let records = try JSONDecoder().decode([DurableConversationTurnRecord].self, from: Data(contentsOf: url))
        let damaged = try JSONEncoder().encode(records.filter { $0.sessionID != a })
        try damaged.write(to: url, options: .atomic)
        let witness = try Data(contentsOf: marker)
        for _ in 0..<2 {
            await assertFails { try await ledger.clear(url: url, sessionID: self.a, turnID: 1) }
        }
        XCTAssertEqual(try Data(contentsOf: url), damaged)
        XCTAssertEqual(try Data(contentsOf: marker), witness)
    }

    func testOnlySyncedLedgerClearCanRetryWitnessPublication() async throws {
        for uncertainLedger in [false, true] {
            let root = try makeRoot(), url = root.appendingPathComponent("durable-conversation-turns.json")
            let marker = root.appendingPathComponent("durable-conversation-turns.witness")
            let failure = LedgerFailureSwitch()
            let ledger = DurableConversationTurnLedger(writer: LedgerFailureWriter(failure: failure))
            try await ledger.begin(url: url, sessionID: a, turnID: 1, scope: "/a")
            try await ledger.begin(url: url, sessionID: b, turnID: 2, scope: "/b")
            failure.set(uncertainLedger ? .afterLedger : .beforeRollbackWitness)
            await assertFails { try await ledger.clear(url: url, sessionID: self.a, turnID: 1) }
            let ledgerBytes = try Data(contentsOf: url), witnessBytes = try Data(contentsOf: marker)
            failure.set(.none)
            if uncertainLedger {
                await assertFails { try await ledger.clear(url: url, sessionID: self.a, turnID: 1) }
                XCTAssertEqual(try Data(contentsOf: url), ledgerBytes)
                XCTAssertEqual(try Data(contentsOf: marker), witnessBytes)
            } else {
                try await ledger.clear(url: url, sessionID: a, turnID: 1)
                let first = try await ledger.load(url: url, sessionID: a)
                let second = try await ledger.load(url: url, sessionID: b)
                XCTAssertNil(first)
                XCTAssertEqual(second?.turnID, 2)
            }
        }
    }
}
