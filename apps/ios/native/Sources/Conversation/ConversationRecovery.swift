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
final class DurableConversationTurnClientStore {
    private let scope: String
    private let url: URL
    private var liveAckSequences: [UInt64: UInt64] = [:]

    init(config: EngineConfig) {
        scope = config.projectCwd ?? "__global__"
        url = URL(fileURLWithPath: config.appSandboxRoot, isDirectory: true)
            .appendingPathComponent("durable-conversation-turn.json")
    }

    func begin(sessionID: String, turnID: UInt64) {
        guard !sessionID.isEmpty else { return }
        liveAckSequences[turnID] = 0
        write(DurableConversationTurnRecord(
            scope: scope,
            sessionID: sessionID,
            turnID: turnID,
            lastSequence: 0
        ))
    }

    func load() -> DurableConversationTurnRecord? {
        guard
            let record = loadPersistedRecord(),
            record.scope == scope
        else { return nil }
        var attachRecord = record
        // The UI projection is not durably persisted event-by-event, so a
        // fresh source/process must replay from the retained suffix again.
        attachRecord.lastSequence = liveAckSequences[record.turnID] ?? 0
        return attachRecord
    }

    func updateSequence(turnID: UInt64, sequence: UInt64) {
        liveAckSequences[turnID] = max(liveAckSequences[turnID] ?? 0, sequence)
    }

    func clear(turnID: UInt64? = nil) {
        guard let record = loadPersistedRecord() else { return }
        guard turnID == nil || record.turnID == turnID else { return }
        liveAckSequences.removeValue(forKey: record.turnID)
        try? FileManager.default.removeItem(at: url)
    }

    private func write(_ record: DurableConversationTurnRecord) {
        guard let data = try? JSONEncoder().encode(record) else { return }
        try? FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try? data.write(to: url, options: .atomic)
    }

    private func loadPersistedRecord() -> DurableConversationTurnRecord? {
        guard
            let data = try? Data(contentsOf: url),
            let record = try? JSONDecoder().decode(DurableConversationTurnRecord.self, from: data)
        else { return nil }
        return record
    }
}
#endif
