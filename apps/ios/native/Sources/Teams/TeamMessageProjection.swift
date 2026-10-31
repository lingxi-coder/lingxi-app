import Foundation
import CryptoKit

struct TeamMessageConversation: Decodable, Equatable, Identifiable, Sendable {
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
struct TeamMessage: Decodable, Equatable, Identifiable, Sendable {
    struct Author: Decodable, Equatable, Sendable { let kind: String; let uid: String; let botKey: String? }
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
    var mentions: [String]? = nil
    var triggerId: String? = nil
    var origin: TeamBotOrigin? = nil
    var question: TeamBotQuestion? = nil
}
struct TeamMessageIntent: Codable, Equatable, Identifiable, Sendable {
    var action = "mutate"
    let teamId: String
    let generation: Int
    let conversationId: String
    let messageId: String
    let changeId: String
    let operation: String
    let text: String
    let replyTo: String?
    var targets: [String]? = nil
    var error: String?
    var leaseUntil: Double?
    var attempted: Bool?
    var id: String { changeId }
}
struct TeamMessageCreation: Codable, Equatable, Sendable {
    var action = "create"
    let teamId: String
    let generation: Int
    let conversationId: String
    let kind: String
    let title: String
    let participantUids: [String]
    var botKeys: [String]? = nil
    var error: String?
    var leaseUntil: Double?
    var attempted: Bool?
}
struct TeamMessageProjection: Codable, Equatable, Sendable {
    struct Thread: Equatable, Sendable { var messages: [TeamMessage] = [] }
    let version: Int
    let uid: String
    let teamId: String
    let generation: Int
    private enum CodingKeys: String, CodingKey { case version, uid, teamId, generation, outbox, creation, botOutbox }
    var conversations: [TeamMessageConversation] = []
    var threads: [String: Thread] = [:]
    var outbox: [TeamMessageIntent] = []
    var creation: TeamMessageCreation?
    var botOutbox: [TeamBotIntent]? = nil
    mutating func acknowledgeBotIntent(_ id: String) { botOutbox?.removeAll { $0.id == id } }
    func hasPendingAnswer(for message: TeamMessage) -> Bool {
        (botOutbox ?? []).contains { $0.action == "botAnswer" && $0.triggerId == message.triggerId && $0.questionId == message.question?.id }
    }

}
struct TeamMessageFailure: Error { let code: String }

/// Only pending HTTP commands survive restart. Firestore owns the message cache.
actor TeamCommandFiles {
    static let shared = TeamCommandFiles()
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
    func load(uid: String, teamId: String, generation: Int) throws -> TeamMessageProjection {
        let file = try url(uid: uid, teamId: teamId, generation: generation)
        guard FileManager.default.fileExists(atPath: file.path) else { return TeamMessageProjection(version: 1, uid: uid, teamId: teamId, generation: generation) }
        let data = try Data(contentsOf: file)
        let value = try JSONDecoder().decode(TeamMessageProjection.self, from: data)
        if let old = try JSONSerialization.jsonObject(with: data) as? [String: Any], old["conversations"] != nil || old["threads"] != nil {
            try JSONEncoder().encode(value).write(to: file, options: [.atomic, .completeFileProtectionUnlessOpen])
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
        }
        guard value.version == 1, value.uid == uid, value.teamId == teamId, value.generation == generation else { throw TeamMessageFailure(code: "STORAGE_FAILED") }
        return value
    }
    func update(uid: String, teamId: String, generation: Int, _ reduce: @Sendable (inout TeamMessageProjection) throws -> Void) throws -> TeamMessageProjection {
        let old = try load(uid: uid, teamId: teamId, generation: generation)
        var value = old
        try reduce(&value)
        guard value != old else { return old }
        let file = try url(uid: uid, teamId: teamId, generation: generation)
        try JSONEncoder().encode(value).write(to: file, options: [.atomic, .completeFileProtectionUnlessOpen])
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
        return value
    }
}

enum TeamBotTrigger {
    static func identity(messageId: String, botKey: String) -> String {
        // Both fields use the backend's ASCII URL-safe identity grammar.
        let data = Data("[\"\(messageId)\",\"\(botKey)\"]".utf8)
        return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
}
