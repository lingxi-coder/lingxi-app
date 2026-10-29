import Foundation
import CryptoKit

struct TeamMessageConversation: Codable, Identifiable, Sendable {
    let id: String
    let kind: String
    let title: String
    let participants: [String]
    var lastSeq: Int
    var lastChangeSeq: Int
    var lastReadSeq: Int
    func merging(_ old: TeamMessageConversation?) -> TeamMessageConversation {
        var value = self; value.lastReadSeq = max(old?.lastReadSeq ?? 0, lastReadSeq); value.lastSeq = max(old?.lastSeq ?? 0, lastSeq); value.lastChangeSeq = max(old?.lastChangeSeq ?? 0, lastChangeSeq); return value
    }
    var unread: Int { max(0, lastSeq - lastReadSeq) }
}
struct TeamMessage: Codable, Identifiable, Sendable {
    struct Author: Codable, Sendable { let kind: String; let uid: String; let botKey: String? }
    let id: String
    let seq: Int
    let author: Author
    let kind: String
    let text: String
    let replyTo: String?
    let createdAt: String?
    let editedAt: String?
    let recalled: Bool?
    let changeSeq: Int
    let changeId: String
}
struct TeamMessageChange: Decodable, Sendable { let id: String; let seq: Int; let messageId: String; let operation: String }
struct TeamMessagePage: Decodable, Sendable { let conversation: TeamMessageConversation; let changes: [TeamMessageChange]; let messages: [TeamMessage]; let hasMore: Bool }
struct TeamMessageIntent: Codable, Identifiable, Sendable {
    var action = "mutate"
    let teamId: String
    let generation: Int
    let conversationId: String
    let messageId: String
    let changeId: String
    let operation: String
    let text: String
    let replyTo: String?
    var error: String?
    var leaseUntil: Double?
    var attempted: Bool?
    var id: String { changeId }
}
struct TeamMessageCreation: Codable, Sendable {
    var action = "create"
    let teamId: String
    let generation: Int
    let conversationId: String
    let kind: String
    let title: String
    let participantUids: [String]
    var error: String?
    var leaseUntil: Double?
    var attempted: Bool?
}
struct TeamMessageProjection: Codable, Sendable {
    struct Thread: Codable, Sendable { var cursor = 0; var messages: [TeamMessage] = [] }
    let version: Int
    let uid: String
    let teamId: String
    let generation: Int
    var conversations: [TeamMessageConversation] = []
    var threads: [String: Thread] = [:]
    var outbox: [TeamMessageIntent] = []
    var creation: TeamMessageCreation?
    mutating func apply(_ page: TeamMessagePage) throws {
        let id = page.conversation.id
        var thread = threads[id] ?? Thread()
        var records = Dictionary(uniqueKeysWithValues: thread.messages.map { ($0.id, $0) })
        for change in page.changes where change.seq > thread.cursor {
            guard change.seq == thread.cursor + 1, let value = page.messages.first(where: { $0.id == change.messageId }), value.changeSeq >= change.seq else { throw TeamMessageFailure(code: "CHANGE_GAP") }
            if (records[value.id]?.changeSeq ?? -1) < value.changeSeq { records[value.id] = value }
            thread.cursor = change.seq
        }
        thread.messages = records.values.sorted { $0.seq < $1.seq }
        threads[id] = thread
        let conversation = page.conversation.merging(conversations.first { $0.id == id })
        conversations.removeAll { $0.id == id }; conversations.append(conversation)
    }
}
struct TeamMessageFailure: Error { let code: String }

/// One atomic file contains the text, cursor and pending identities. The file is
/// scoped to account + team + membership generation and excluded from backup.
actor TeamMessageFiles {
    static let shared = TeamMessageFiles()
    private let directory: URL?
    init(directory: URL? = nil) { self.directory = directory }
    func url(uid: String, teamId: String, generation: Int) throws -> URL {
        let directory = try self.directory ?? FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true).appendingPathComponent("TeamMessages", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        var values = URLResourceValues(); values.isExcludedFromBackup = true
        var folder = directory; try folder.setResourceValues(values)
        let key = try JSONEncoder().encode([uid, teamId, String(generation)])
        let name = SHA256.hash(data: key).map { String(format: "%02x", $0) }.joined()
        return directory.appendingPathComponent(name + ".json")
    }
    func update(uid: String, teamId: String, generation: Int, _ reduce: @Sendable (inout TeamMessageProjection) throws -> Void) throws -> TeamMessageProjection {
        let file = try url(uid: uid, teamId: teamId, generation: generation)
        var value = TeamMessageProjection(version: 1, uid: uid, teamId: teamId, generation: generation)
        if FileManager.default.fileExists(atPath: file.path) {
            value = try JSONDecoder().decode(TeamMessageProjection.self, from: Data(contentsOf: file))
            guard value.version == 1, value.uid == uid, value.teamId == teamId, value.generation == generation else { throw TeamMessageFailure(code: "STORAGE_FAILED") }
        }
        try reduce(&value)
        try JSONEncoder().encode(value).write(to: file, options: [.atomic, .completeFileProtectionUnlessOpen])
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
        return value
    }
}
