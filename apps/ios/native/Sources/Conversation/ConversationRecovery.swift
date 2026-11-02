import Combine
import Foundation
import OSLog
import SwiftUI

#if canImport(harness_runtimeFFI)
import AuthenticationServices
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
#endif

#if canImport(harness_runtimeFFI)
@MainActor
final class DurableConversationTurnClientStore {
    private var scope: String
    private let url: URL
    private var sessionID: String?
    private var liveAckSequences: [UInt64: UInt64] = [:]

    init(config: EngineConfig) {
        scope = config.projectCwd ?? LXISHGuestPaths.workspaceRoot
        sessionID = config.initialSessionId.map(canonicalSessionID).flatMap { $0.isEmpty ? nil : $0 }
        url = URL(fileURLWithPath: config.appSandboxRoot, isDirectory: true)
            .appendingPathComponent("durable-conversation-turns.json")
    }

    func bind(sessionID: String) {
        let owner = canonicalSessionID(sessionID)
        guard !owner.isEmpty else { return }
        if self.sessionID != owner { liveAckSequences.removeAll() }
        self.sessionID = owner
    }

    func begin(sessionID: String, turnID: UInt64, workspace: String? = nil) {
        let owner = canonicalSessionID(sessionID)
        guard !owner.isEmpty else { return }
        if let workspace, !workspace.isEmpty { scope = workspace }
        bind(sessionID: owner)
        liveAckSequences = [turnID: 0]
        var records = loadRecords()
        records.removeAll { canonicalSessionID($0.sessionID) == owner }
        records.append(DurableConversationTurnRecord(scope: scope, sessionID: owner, turnID: turnID, lastSequence: 0))
        write(records)
    }

    func load() -> DurableConversationTurnRecord? {
        guard let sessionID,
              var record = loadRecords().last(where: { canonicalSessionID($0.sessionID) == sessionID })
        else { return nil }
        // The catalog cwd may lag a workspace change. The immutable session
        // UUID identifies the owner; a confirmed resume supplies the actual
        // workspace before the source requests reattachment.
        // A fresh projection must replay from the beginning; only a warm
        // source can use its in-memory acknowledgement cursor.
        record.lastSequence = liveAckSequences[record.turnID] ?? 0
        return record
    }

    func updateSequence(turnID: UInt64, sequence: UInt64) {
        liveAckSequences[turnID] = max(liveAckSequences[turnID] ?? 0, sequence)
    }

    func clear(turnID: UInt64) {
        guard let sessionID else { return }
        var records = loadRecords()
        let removed = records.filter { canonicalSessionID($0.sessionID) == sessionID && $0.turnID == turnID }
        guard !removed.isEmpty else { return }
        records.removeAll { canonicalSessionID($0.sessionID) == sessionID && $0.turnID == turnID }
        for record in removed { liveAckSequences.removeValue(forKey: record.turnID) }
        write(records)
    }

    private func write(_ records: [DurableConversationTurnRecord]) {
        guard let data = try? JSONEncoder().encode(records) else { return }
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: url, options: .atomic)
    }

    private func loadRecords() -> [DurableConversationTurnRecord] {
        guard let data = try? Data(contentsOf: url) else { return [] }
        return (try? JSONDecoder().decode([DurableConversationTurnRecord].self, from: data)) ?? []
    }
}
#endif
