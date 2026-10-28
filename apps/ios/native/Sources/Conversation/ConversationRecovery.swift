import Combine
import CryptoKit
import Foundation
import OSLog
import SwiftUI

#if canImport(harness_runtimeFFI)
import AuthenticationServices
import Darwin
import UIKit
import harness_runtimeFFI
#endif

#if canImport(harness_runtimeFFI)
struct DurableConversationTurnRecord: Codable, Equatable {
    let scope: String
    let sessionID: String
    let turnID: UInt64
    var lastSequence: UInt64
}

enum DurableSubmissionKind: String, Codable {
    case prompt
    case slash
}

struct DurableSubmissionRecord: Codable, Equatable {
    let sessionID: String
    let turnID: UInt64
    let kind: DurableSubmissionKind
    var attemptID: String? = nil
    var admittedQueued: Bool? = nil
    var projectionIncomplete: Bool? = nil
}

struct TerminalProjectionEvidence: Codable, Equatable {
    let sessionID: String
    let turnID: UInt64
    var projectionUncertain: Bool
    var liveOnlyOutput: Bool
    var queuedCancelled: Bool? = nil
    var version: String? = nil
}
#endif

#if canImport(harness_runtimeFFI)
final class DurableConversationTurnClientStore: @unchecked Sendable {
    private static let fileLock = NSRecursiveLock()
    private let scope: String
    private let legacyScope: String
    private let canMigrateLegacy: Bool
    private let url: URL
    private let legacyURL: URL
    private let submissionsURL: URL
    private let terminalEvidenceURL: URL
    private struct SubmissionLedger: Codable {
        let scope: String
        var entries: [DurableSubmissionRecord]
    }
    private struct TerminalEvidenceLedger: Codable {
        let scope: String
        var entries: [TerminalProjectionEvidence]
    }
    private struct TurnIdentity: Hashable {
        let sessionID: String
        let turnID: UInt64
    }
    private struct ProjectedCursor {
        var contiguous: UInt64 = 0
        var later: Set<UInt64> = []
    }
    private var liveAckSequences: [TurnIdentity: ProjectedCursor] = [:]
    private var failNextSubmissionSync = false
    private var failNextTerminalEvidenceSync = false
    private var failMainCheckpointSync = false

    init(config: EngineConfig) {
        // Use the exact configured project path: do not infer equivalence
        // between relative paths or symbolic links. Global is a distinct kind.
        scope = config.projectCwd.map { "project:\($0)" } ?? "global:"
        legacyScope = config.projectCwd ?? "__global__"
        canMigrateLegacy = config.projectCwd != "__global__"
        let directory = URL(fileURLWithPath: config.appSandboxRoot, isDirectory: true)
        legacyURL = directory.appendingPathComponent("durable-conversation-turn.json")
        let digest = SHA256.hash(data: Data(scope.utf8)).map { String(format: "%02x", $0) }.joined()
        url = directory.appendingPathComponent("durable-conversation-turn-\(digest).json")
        submissionsURL = directory.appendingPathComponent("durable-submissions-\(digest).json")
        terminalEvidenceURL = directory.appendingPathComponent("terminal-evidence-\(digest).json")
    }

    func terminalEvidence(sessionID: String) throws -> [TerminalProjectionEvidence] {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        return try readTerminalEvidence().entries.filter { $0.sessionID == sessionID }
    }

    func recordTerminalEvidence(sessionID: String, turnID: UInt64,
                                projectionUncertain: Bool, liveOnlyOutput: Bool,
                                queuedCancelled: Bool = false) throws {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        var ledger = try readTerminalEvidence()
        if let index = ledger.entries.firstIndex(where: { $0.sessionID == sessionID && $0.turnID == turnID }) {
            ledger.entries[index].projectionUncertain = ledger.entries[index].projectionUncertain || projectionUncertain
            ledger.entries[index].liveOnlyOutput = ledger.entries[index].liveOnlyOutput || liveOnlyOutput
            if queuedCancelled { ledger.entries[index].queuedCancelled = true }
            ledger.entries[index].version = UUID().uuidString
        } else {
            ledger.entries.append(TerminalProjectionEvidence(sessionID: sessionID, turnID: turnID,
                projectionUncertain: projectionUncertain, liveOnlyOutput: liveOnlyOutput,
                queuedCancelled: queuedCancelled ? true : nil, version: UUID().uuidString))
        }
        try writeTerminalEvidence(ledger)
    }

    func acknowledgeTerminalEvidence(_ reviewed: [TerminalProjectionEvidence], sessionID: String) throws {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        var ledger = try readTerminalEvidence()
        let oldCount = ledger.entries.count
        ledger.entries.removeAll { entry in entry.sessionID == sessionID && reviewed.contains(entry) }
        if ledger.entries.count != oldCount { try writeTerminalEvidence(ledger) }
    }

    private func writeTerminalEvidence(_ ledger: TerminalEvidenceLedger) throws {
        try prepareDirectory(terminalEvidenceURL.deletingLastPathComponent())
        try JSONEncoder().encode(ledger).write(to: terminalEvidenceURL, options: .atomic)
        if failNextTerminalEvidenceSync {
            failNextTerminalEvidenceSync = false
            throw CocoaError(.fileWriteUnknown)
        }
        try synchronizeFileAndParent(terminalEvidenceURL)
    }

    private func readTerminalEvidence() throws -> TerminalEvidenceLedger {
        guard FileManager.default.fileExists(atPath: terminalEvidenceURL.path) else {
            return TerminalEvidenceLedger(scope: scope, entries: [])
        }
        let ledger = try JSONDecoder().decode(TerminalEvidenceLedger.self, from: Data(contentsOf: terminalEvidenceURL))
        guard ledger.scope == scope else { throw CocoaError(.fileReadCorruptFile) }
        return ledger
    }

    func pendingSubmissions() throws -> [DurableSubmissionRecord] {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        return try readSubmissions().entries
    }

    func beginSubmission(sessionID: String, turnID: UInt64, kind: DurableSubmissionKind, attemptID: String) throws {
        guard !sessionID.isEmpty else { throw CocoaError(.fileWriteInvalidFileName) }
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        var ledger = try readSubmissions()
        guard !ledger.entries.contains(where: { $0.sessionID == sessionID && $0.turnID == turnID }) else {
            throw CocoaError(.fileWriteFileExists)
        }
        ledger.entries.append(DurableSubmissionRecord(sessionID: sessionID, turnID: turnID, kind: kind, attemptID: attemptID))
        try writeSubmissions(ledger)
    }

    func clearSubmission(sessionID: String, turnID: UInt64, attemptID: String? = nil) throws {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        var ledger = try readSubmissions()
        let originalCount = ledger.entries.count
        ledger.entries.removeAll {
            $0.sessionID == sessionID && $0.turnID == turnID && (attemptID == nil || $0.attemptID == attemptID)
        }
        if ledger.entries.count != originalCount { try writeSubmissions(ledger) }
    }

    func markQueuedAdmitted(sessionID: String, turnID: UInt64, attemptID: String) throws {
        try updateSubmission(sessionID: sessionID, turnID: turnID, attemptID: attemptID) { $0.admittedQueued = true }
    }

    func markSubmissionIncomplete(sessionID: String, turnID: UInt64) throws {
        try updateSubmission(sessionID: sessionID, turnID: turnID, attemptID: nil) { $0.projectionIncomplete = true }
    }


    private func updateSubmission(sessionID: String, turnID: UInt64, attemptID: String?,
                                  mutate: (inout DurableSubmissionRecord) -> Void) throws {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        var ledger = try readSubmissions()
        guard let index = ledger.entries.firstIndex(where: {
            $0.sessionID == sessionID && $0.turnID == turnID && (attemptID == nil || $0.attemptID == attemptID)
        }) else { return }
        mutate(&ledger.entries[index])
        try writeSubmissions(ledger)
    }

    private func readSubmissions() throws -> SubmissionLedger {
        guard FileManager.default.fileExists(atPath: submissionsURL.path) else {
            return SubmissionLedger(scope: scope, entries: [])
        }
        let ledger = try JSONDecoder().decode(SubmissionLedger.self, from: Data(contentsOf: submissionsURL))
        guard ledger.scope == scope else { throw CocoaError(.fileReadCorruptFile) }
        return ledger
    }

    private func writeSubmissions(_ ledger: SubmissionLedger) throws {
        try prepareDirectory(submissionsURL.deletingLastPathComponent())
        try JSONEncoder().encode(ledger).write(to: submissionsURL, options: .atomic)
        if failNextSubmissionSync {
            failNextSubmissionSync = false
            throw CocoaError(.fileWriteUnknown)
        }
        try synchronizeFileAndParent(submissionsURL)
    }

    func failNextSubmissionSyncForTesting() {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        failNextSubmissionSync = true
    }

    func failNextTerminalEvidenceSyncForTesting() {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        failNextTerminalEvidenceSync = true
    }

    @discardableResult
    func begin(sessionID: String, turnID: UInt64) -> Bool {
        guard !sessionID.isEmpty else { return false }
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        migrateLegacyIfNeeded()
        // If an exact-scope legacy record could not be moved, keep it intact
        // rather than shadowing it with a newly created checkpoint.
        if !FileManager.default.fileExists(atPath: url.path), matchingLegacyRecord() != nil { return false }
        // A store instance projects one visible turn at a time. Starting the
        // next one must not retain every previously acknowledged cursor.
        liveAckSequences.removeAll()
        liveAckSequences[TurnIdentity(sessionID: sessionID, turnID: turnID)] = ProjectedCursor()
        let record = DurableConversationTurnRecord(
            scope: scope,
            sessionID: sessionID,
            turnID: turnID,
            lastSequence: 0
        )
        do {
            try prepareDirectory(url.deletingLastPathComponent())
            try JSONEncoder().encode(record).write(to: url, options: .atomic)
            if failMainCheckpointSync { throw CocoaError(.fileWriteUnknown) }
            try synchronizeFileAndParent(url)
            return readRecord(at: url) == record
        } catch { return false }
    }

    /// A prior atomic publish might have failed before synchronization. Do
    /// not release the submission ledger until this exact identity is synced.
    func ensureDurable(sessionID: String, turnID: UInt64) -> Bool {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        guard let (record, recordURL) = selectedRecord(), owns(record),
              record.sessionID == sessionID, record.turnID == turnID else { return false }
        do {
            if failMainCheckpointSync { throw CocoaError(.fileWriteUnknown) }
            try synchronizeFileAndParent(recordURL)
            return readRecord(at: recordURL) == record
        } catch { return false }
    }

    func setFailMainCheckpointSyncForTesting(_ fail: Bool) {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        failMainCheckpointSync = fail
    }

    func load() -> DurableConversationTurnRecord? {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        guard
            let (record, _) = selectedRecord(),
            owns(record)
        else { return nil }
        var attachRecord = record
        // The UI projection is not durably persisted event-by-event, so a
        // fresh source/process must replay from the retained suffix again.
        attachRecord.lastSequence = liveAckSequences[TurnIdentity(sessionID: record.sessionID, turnID: record.turnID)]?.contiguous ?? 0
        return attachRecord
    }

    func projectedSequence(sessionID: String, turnID: UInt64) -> UInt64 {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        return liveAckSequences[TurnIdentity(sessionID: sessionID, turnID: turnID)]?.contiguous ?? 0
    }

    @discardableResult
    func updateSequence(sessionID: String, turnID: UInt64, sequence: UInt64) -> Bool {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        let identity = TurnIdentity(sessionID: sessionID, turnID: turnID)
        var cursor = liveAckSequences[identity] ?? ProjectedCursor()
        guard sequence > cursor.contiguous else { return true }
        // Out-of-order confirmations are bounded; never skip a missing entry
        // just to make room for a later one.
        guard sequence == cursor.contiguous + 1 || cursor.later.contains(sequence) || cursor.later.count < 512 else { return false }
        cursor.later.insert(sequence)
        while cursor.contiguous < UInt64.max, cursor.later.remove(cursor.contiguous + 1) != nil {
            cursor.contiguous += 1
        }
        liveAckSequences[identity] = cursor
        return true
    }

    func clear(turnID: UInt64? = nil, sessionID: String? = nil) {
        Self.fileLock.lock()
        defer { Self.fileLock.unlock() }
        guard let (record, recordURL) = selectedRecord(), owns(record) else { return }
        guard turnID == nil || record.turnID == turnID else { return }
        guard sessionID == nil || record.sessionID == sessionID else { return }
        liveAckSequences.removeValue(forKey: TurnIdentity(sessionID: record.sessionID, turnID: record.turnID))
        try? FileManager.default.removeItem(at: recordURL)
    }

    private func synchronizeFileAndParent(_ fileURL: URL) throws {
        let file = try FileHandle(forWritingTo: fileURL)
        defer { try? file.close() }
        try file.synchronize()
        try synchronizeDirectory(fileURL.deletingLastPathComponent())
    }

    private func prepareDirectory(_ directory: URL) throws {
        let manager = FileManager.default
        var missing: [URL] = []
        var ancestor = directory
        while !manager.fileExists(atPath: ancestor.path) {
            missing.append(ancestor)
            let parent = ancestor.deletingLastPathComponent()
            guard parent.path != ancestor.path else { throw CocoaError(.fileWriteUnknown) }
            ancestor = parent
        }
        guard !missing.isEmpty else { return }
        try manager.createDirectory(at: directory, withIntermediateDirectories: true)
        for created in missing { try synchronizeDirectory(created) }
        try synchronizeDirectory(ancestor)
    }

    private func synchronizeDirectory(_ directory: URL) throws {
        let descriptor = open(directory.path, O_RDONLY)
        guard descriptor >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        defer { close(descriptor) }
        guard fsync(descriptor) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
    }

    private func readRecord(at recordURL: URL) -> DurableConversationTurnRecord? {
        guard
            let data = try? Data(contentsOf: recordURL),
            let record = try? JSONDecoder().decode(DurableConversationTurnRecord.self, from: data)
        else { return nil }
        return record
    }

    private func owns(_ record: DurableConversationTurnRecord) -> Bool {
        record.scope == scope || (canMigrateLegacy && record.scope == legacyScope)
    }

    private func matchingLegacyRecord() -> DurableConversationTurnRecord? {
        guard canMigrateLegacy, let record = readRecord(at: legacyURL), record.scope == legacyScope else { return nil }
        return record
    }

    private func migrateLegacyIfNeeded() {
        guard !FileManager.default.fileExists(atPath: url.path), matchingLegacyRecord() != nil else { return }
        // Both URLs share a directory; FileManager refuses an existing target.
        // A failed migration leaves the old checkpoint available to load.
        try? FileManager.default.moveItem(at: legacyURL, to: url)
    }

    private func selectedRecord() -> (DurableConversationTurnRecord, URL)? {
        migrateLegacyIfNeeded()
        if FileManager.default.fileExists(atPath: url.path) {
            return readRecord(at: url).map { ($0, url) }
        }
        return matchingLegacyRecord().map { ($0, legacyURL) }
    }
}
#endif
