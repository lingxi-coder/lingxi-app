import Foundation

struct DurableConversationTurnRecord: Codable, Equatable, Sendable {
    let scope: String
    let sessionID: String
    let turnID: UInt64
    var lastSequence: UInt64
}

private struct DurableConversationTurnWitness: Codable, Equatable {
    struct Owner: Codable, Equatable {
        let sessionID: String
        let turnID: UInt64
    }
    let version: Int
    var owners: [Owner]
}

enum DurableConversationRecoveryError: LocalizedError {
    case invalidStore
    case unresolvedOwner

    var errorDescription: String? {
        switch self {
        case .invalidStore: return "The conversation recovery record is missing, unreadable, or ambiguous."
        case .unresolvedOwner: return "This conversation still has a recovery record that must be resolved."
        }
    }
}

/// One non-main actor serializes checked read-modify-write across all sources
/// sharing this recovery file. A transaction contains no suspension point.
/// The witness detects a missing or incomplete counterpart. If both files are
/// externally removed across a restart, no surviving evidence distinguishes
/// that from a new store. A terminal clear whose final witness fsync fails is
/// fenced in-process; on restart, a consistently empty pair is a completed
/// terminal clear, while a surviving mismatch is rejected.
actor DurableConversationTurnLedger {
    static let shared = DurableConversationTurnLedger()
    private struct UnpublishedAttempt {
        let owner: String
        let witnessBefore: OwnerCatalogExpectation
        let ledgerBefore: OwnerCatalogExpectation
        let publishedWitness: OwnerCatalogExpectation
        let emptyWitness: Data
    }
    private struct ClearReceipt {
        let turnID: UInt64
        let ledger: OwnerCatalogExpectation
        let witness: OwnerCatalogExpectation
    }
    private let writer: any ProjectAtomicWriting
    private var blockedOwners: [String: Set<String>] = [:]
    private var unpublishedAttempts: [String: UnpublishedAttempt] = [:]
    private var clearReceipts: [String: [String: ClearReceipt]] = [:]
    private var observedStores = Set<String>()

    init(writer: any ProjectAtomicWriting = DefaultProjectAtomicWriter()) { self.writer = writer }

    private func witnessURL(for url: URL) -> URL {
        url.deletingLastPathComponent().appendingPathComponent("durable-conversation-turns.witness")
    }

    private func snapshot(_ url: URL, allowWitnessOnlyFor retryOwner: String? = nil) throws -> (OwnerCatalogExpectation, OwnerCatalogExpectation, DurableConversationTurnWitness, [DurableConversationTurnRecord]) {
        let witness = try ownerCatalogExpectation(witnessURL(for: url))
        let ledger = try ownerCatalogExpectation(url)
        let key = url.standardizedFileURL.path
        if case .present = witness { observedStores.insert(key) }
        if case .present = ledger { observedStores.insert(key) }
        switch (witness, ledger) {
        case (.absent, .absent):
            guard !observedStores.contains(key) else { throw DurableConversationRecoveryError.invalidStore }
            return (witness, ledger, DurableConversationTurnWitness(version: 1, owners: []), [])
        case let (.present(marker), .present(contents)):
            do {
                try UniqueJSONKeyGuard.validate(marker.bytes)
                let witness = try JSONDecoder().decode(DurableConversationTurnWitness.self, from: marker.bytes)
                guard witness.version == 1 else { throw DurableConversationRecoveryError.invalidStore }
                try UniqueJSONKeyGuard.validate(contents.bytes)
                let records = try JSONDecoder().decode([DurableConversationTurnRecord].self, from: contents.bytes)
                var witnessed: [String: UInt64] = [:]
                for item in witness.owners {
                    let owner = canonicalSessionID(item.sessionID)
                    guard !owner.isEmpty, witnessed.updateValue(item.turnID, forKey: owner) == nil else {
                        throw DurableConversationRecoveryError.invalidStore
                    }
                }
                var owners = Set<String>()
                for record in records {
                    let owner = canonicalSessionID(record.sessionID)
                    guard !owner.isEmpty, !record.scope.isEmpty, owners.insert(owner).inserted,
                          witnessed[owner] == record.turnID else {
                        throw DurableConversationRecoveryError.invalidStore
                    }
                }
                let witnessOnly = Set(witnessed.keys).subtracting(owners)
                guard witnessOnly.isEmpty || (retryOwner != nil && witnessOnly == Set([retryOwner!])) else {
                    throw DurableConversationRecoveryError.invalidStore
                }
                return (.present(marker), ledger, witness, records)
            } catch { throw DurableConversationRecoveryError.invalidStore }
        default: throw DurableConversationRecoveryError.invalidStore
        }
    }

    private func durableWrite(_ data: Data, to url: URL, expected: OwnerCatalogExpectation,
                              witness: (URL, OwnerCatalogExpectation)? = nil,
                              validate: (Data) throws -> Void) throws {
        try writer.writeData(data, to: url, expected: expected, validate: validate, commit: { replace in
            if let (witnessURL, witnessExpected) = witness {
                guard try ownerCatalogExpectation(witnessURL) == witnessExpected else { throw DurableConversationRecoveryError.invalidStore }
            }
            try replace()
        })
        try synchronizeOwnerCatalog(url, expectedBytes: data)
    }

    /// Undo only a begin that failed before its ledger replacement was
    /// published. Both exact file identities are checked again at each commit.
    /// This proof exists only in this actor; cold mismatches remain invalid.
    private func rollbackUnpublished(url: URL, key: String) throws {
        guard let attempt = unpublishedAttempts[key] else { return }
        let markerURL = witnessURL(for: url)
        guard try ownerCatalogExpectation(markerURL) == attempt.publishedWitness,
              try ownerCatalogExpectation(url) == attempt.ledgerBefore else {
            throw DurableConversationRecoveryError.invalidStore
        }

        var ledgerExpected = attempt.ledgerBefore
        let previousMarker: Data
        switch (attempt.witnessBefore, attempt.ledgerBefore) {
        case let (.present(snapshot), .present):
            previousMarker = snapshot.bytes
        case (.absent, .absent):
            // There was no prior pair. Publish an empty ledger first while the
            // exact owner marker remains in place. A crash between writes is
            // still an invalid mismatch, never a cold automatic recovery.
            let emptyLedger = try JSONEncoder().encode([DurableConversationTurnRecord]())
            try durableWrite(emptyLedger, to: url, expected: .absent,
                             witness: (markerURL, attempt.publishedWitness)) { bytes in
                guard bytes == emptyLedger else { throw DurableConversationRecoveryError.invalidStore }
            }
            ledgerExpected = try ownerCatalogExpectation(url)
            guard ledgerExpected.bytes == emptyLedger else { throw DurableConversationRecoveryError.invalidStore }
            previousMarker = attempt.emptyWitness
        default:
            throw DurableConversationRecoveryError.invalidStore
        }
        try durableWrite(previousMarker, to: markerURL, expected: attempt.publishedWitness,
                         witness: (url, ledgerExpected)) { bytes in
            guard bytes == previousMarker else { throw DurableConversationRecoveryError.invalidStore }
        }
        unpublishedAttempts.removeValue(forKey: key)
        blockedOwners[key]?.remove(attempt.owner)
    }

    func begin(url: URL, sessionID: String, turnID: UInt64, scope: String) throws {
        let owner = canonicalSessionID(sessionID)
        guard !owner.isEmpty, !scope.isEmpty else { throw DurableConversationRecoveryError.invalidStore }
        let key = url.standardizedFileURL.path
        // A previous pre-publication failure can be retried only using the
        // retained proof. An uncertain or changed file remains fenced.
        try rollbackUnpublished(url: url, key: key)
        guard !(blockedOwners[key]?.contains(owner) ?? false) else { throw DurableConversationRecoveryError.unresolvedOwner }
        let (witnessExpected, expected, witness, records) = try snapshot(url)
        guard !records.contains(where: { canonicalSessionID($0.sessionID) == owner }),
              !witness.owners.contains(where: { canonicalSessionID($0.sessionID) == owner }) else {
            throw DurableConversationRecoveryError.unresolvedOwner
        }
        let originalWitness = try JSONEncoder().encode(witness)
        var publishedWitness: OwnerCatalogExpectation?
        do {
            var nextWitness = witness
            nextWitness.owners.append(.init(sessionID: owner, turnID: turnID))
            let markerURL = witnessURL(for: url), marker = try JSONEncoder().encode(nextWitness)
            // Once publication is attempted its result may be uncertain. Do
            // not treat a later disappearance as a brand-new store here.
            observedStores.insert(key)
            try durableWrite(marker, to: markerURL, expected: witnessExpected) { bytes in
                try UniqueJSONKeyGuard.validate(bytes)
                guard try JSONDecoder().decode(DurableConversationTurnWitness.self, from: bytes) == nextWitness else {
                    throw DurableConversationRecoveryError.invalidStore
                }
            }
            let markerSnapshot = try ownerCatalogExpectation(markerURL)
            guard markerSnapshot.bytes == marker else { throw DurableConversationRecoveryError.invalidStore }
            publishedWitness = markerSnapshot
            let next = records + [DurableConversationTurnRecord(scope: scope, sessionID: owner, turnID: turnID, lastSequence: 0)]
            let data = try JSONEncoder().encode(next)
            try durableWrite(data, to: url, expected: expected, witness: (markerURL, markerSnapshot)) { bytes in
                try UniqueJSONKeyGuard.validate(bytes)
                guard try JSONDecoder().decode([DurableConversationTurnRecord].self, from: bytes) == next else {
                    throw DurableConversationRecoveryError.invalidStore
                }
            }
        } catch {
            blockedOwners[key, default: []].insert(owner)
            if let publishedWitness {
                unpublishedAttempts[key] = UnpublishedAttempt(owner: owner, witnessBefore: witnessExpected,
                    ledgerBefore: expected, publishedWitness: publishedWitness,
                    emptyWitness: originalWitness)
                // Failure is deliberately retained as a fence; the next
                // attempt may retry only while the exact proof still holds.
                try? rollbackUnpublished(url: url, key: key)
            }
            throw error
        }
    }

    func load(url: URL, sessionID: String) throws -> DurableConversationTurnRecord? {
        let owner = canonicalSessionID(sessionID)
        guard !owner.isEmpty else { throw DurableConversationRecoveryError.invalidStore }
        let (_, _, _, records) = try snapshot(url)
        return records.first { canonicalSessionID($0.sessionID) == owner }
    }

    func clear(url: URL, sessionID: String, turnID: UInt64) throws {
        let owner = canonicalSessionID(sessionID), key = url.standardizedFileURL.path
        guard !owner.isEmpty else { throw DurableConversationRecoveryError.invalidStore }
        do {
            let receipt = clearReceipts[key]?[owner]
            guard receipt == nil || receipt?.turnID == turnID else { throw DurableConversationRecoveryError.invalidStore }
            let retryOwner = receipt == nil ? nil : owner
            let (witnessExpected, expected, witness, records) = try snapshot(url, allowWitnessOnlyFor: retryOwner)
            if let receipt {
                guard receipt.ledger == expected, receipt.witness == witnessExpected else {
                    throw DurableConversationRecoveryError.invalidStore
                }
            }
            guard witness.owners.contains(where: { canonicalSessionID($0.sessionID) == owner && $0.turnID == turnID }) else {
                throw DurableConversationRecoveryError.invalidStore
            }
            var committedLedger = expected
            if records.contains(where: { canonicalSessionID($0.sessionID) == owner && $0.turnID == turnID }) {
                guard receipt == nil else { throw DurableConversationRecoveryError.invalidStore }
                let next = records.filter { !(canonicalSessionID($0.sessionID) == owner && $0.turnID == turnID) }
                let data = try JSONEncoder().encode(next)
                try durableWrite(data, to: url, expected: expected, witness: (witnessURL(for: url), witnessExpected)) { bytes in
                    try UniqueJSONKeyGuard.validate(bytes)
                    guard try JSONDecoder().decode([DurableConversationTurnRecord].self, from: bytes) == next else {
                        throw DurableConversationRecoveryError.invalidStore
                    }
                }
                // Only a successfully synced ledger deletion proves that a
                // later witness-only state belongs to this terminal clear.
                committedLedger = try ownerCatalogExpectation(url)
                guard committedLedger.bytes == data else { throw DurableConversationRecoveryError.invalidStore }
                clearReceipts[key, default: [:]][owner] = ClearReceipt(
                    turnID: turnID, ledger: committedLedger, witness: witnessExpected)
            } else if receipt == nil {
                throw DurableConversationRecoveryError.invalidStore
            }
            var nextWitness = witness
            nextWitness.owners.removeAll { canonicalSessionID($0.sessionID) == owner && $0.turnID == turnID }
            let marker = try JSONEncoder().encode(nextWitness)
            try durableWrite(marker, to: witnessURL(for: url), expected: witnessExpected,
                             witness: (url, committedLedger)) { bytes in
                try UniqueJSONKeyGuard.validate(bytes)
                guard try JSONDecoder().decode(DurableConversationTurnWitness.self, from: bytes) == nextWitness else {
                    throw DurableConversationRecoveryError.invalidStore
                }
            }
            clearReceipts[key]?.removeValue(forKey: owner)
            blockedOwners[key]?.remove(owner)
        } catch {
            blockedOwners[key, default: []].insert(owner)
            throw error
        }
    }
}
