import Combine
import Foundation
import OSLog
import SwiftUI

#if canImport(harness_runtimeFFI)
import AuthenticationServices
import UIKit
import harness_runtimeFFI

@MainActor
final class DurableConversationTurnClientStore {
    private var scope: String
    private let url: URL
    private let ledger: DurableConversationTurnLedger
    private var sessionID: String?
    private var liveAckSequences: [UInt64: UInt64] = [:]

    init(config: EngineConfig, ledger: DurableConversationTurnLedger = .shared) {
        scope = config.projectCwd ?? LXISHGuestPaths.workspaceRoot
        sessionID = config.initialSessionId.map(canonicalSessionID).flatMap { $0.isEmpty ? nil : $0 }
        url = URL(fileURLWithPath: config.appSandboxRoot, isDirectory: true)
            .appendingPathComponent("durable-conversation-turns.json")
        self.ledger = ledger
    }

    func bind(sessionID: String) {
        let owner = canonicalSessionID(sessionID)
        guard !owner.isEmpty else { return }
        if self.sessionID != owner { liveAckSequences.removeAll() }
        self.sessionID = owner
    }

    func begin(sessionID: String, turnID: UInt64, workspace: String? = nil) async throws {
        let owner = canonicalSessionID(sessionID)
        let selectedScope = workspace.flatMap { $0.isEmpty ? nil : $0 } ?? scope
        let previousOwner = self.sessionID
        try await ledger.begin(url: url, sessionID: owner, turnID: turnID, scope: selectedScope)
        // A session transition can bind a different owner while the actor is
        // writing. Leave that binding intact; the caller will retire this
        // unsubmitted checkpoint using the explicit captured owner.
        if self.sessionID == previousOwner || self.sessionID == owner {
            scope = selectedScope
            bind(sessionID: owner)
            liveAckSequences = [turnID: 0]
        }
    }

    func load() async throws -> DurableConversationTurnRecord? {
        guard let sessionID else { return nil }
        guard var record = try await ledger.load(url: url, sessionID: sessionID) else { return nil }
        guard self.sessionID == sessionID else { throw CancellationError() }
        // Only a warm projection owns its in-memory acknowledgement cursor.
        record.lastSequence = liveAckSequences[record.turnID] ?? 0
        return record
    }

    func updateSequence(turnID: UInt64, sequence: UInt64) {
        liveAckSequences[turnID] = max(liveAckSequences[turnID] ?? 0, sequence)
    }

    func clear(turnID: UInt64, sessionID: String? = nil) async throws {
        guard let owner = sessionID.map(canonicalSessionID) ?? self.sessionID else { throw DurableConversationRecoveryError.invalidStore }
        try await ledger.clear(url: url, sessionID: owner, turnID: turnID)
        if self.sessionID == owner { liveAckSequences.removeValue(forKey: turnID) }
    }
}
#endif
