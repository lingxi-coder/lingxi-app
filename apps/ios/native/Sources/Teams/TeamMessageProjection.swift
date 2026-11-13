import Foundation
import CryptoKit

/// The newest message of a conversation as the backend stamps it onto the conversation document
/// (`lastMessage`, spec §8.4). `nil` on the conversation means "no message yet".
struct TeamLastMessage: Equatable, Sendable {
    let id: String
    let seq: Int
    /// Epoch milliseconds; `nil` while the server has not stamped a write made on this device.
    let atMs: Int64?
    /// `user` or `shared_bot`.
    let authorKind: String
    let authorUid: String
    /// Set for Bot authors.
    let authorSharedBotId: String?
    /// May be empty (a non-text message).
    let preview: String
    let recalled: Bool
}

struct TeamMessageConversation: Equatable, Identifiable, Sendable {
    let id: String
    let kind: String
    let title: String
    let participants: [String]
    var lastSeq: Int
    var lastChangeSeq: Int
    var lastReadSeq: Int
    /// The `sharedBotId` field of a Bot DM document.
    var sharedBotId: String? = nil
    /// The pointer to the newest message; `nil` = no message yet. Parsed strictly (`TeamsPointerParser`).
    var lastMessage: TeamLastMessage? = nil
    /// Who can read it, as written when it was made: its people and the maintainers its Bots had then. `nil` when the source does not say.
    var readers: [String]? = nil
    func merging(_ old: TeamMessageConversation?) -> TeamMessageConversation {
        var value = self; value.lastReadSeq = max(old?.lastReadSeq ?? 0, lastReadSeq); value.lastSeq = max(old?.lastSeq ?? 0, lastSeq); value.lastChangeSeq = max(old?.lastChangeSeq ?? 0, lastChangeSeq); return value
    }
    var unread: Int { max(0, lastSeq - lastReadSeq) }
    /// The raw SharedBot ID stored on a DM or carried by its explicit `shared_bot:` participant.
    var effectiveSharedBotId: String? { sharedBotId ?? participants.first { $0.hasPrefix("shared_bot:") }.map { String($0.dropFirst(11)) } }
}
/// A Bot's request to change its list: the list and exactly the changes the approval is for.
struct TeamListApproval: Decodable, Equatable, Sendable {
    struct Change: Decodable, Equatable, Sendable {
        let op: String
        var text: String? = nil
        var done: Bool? = nil
        /// The item as it read when the Bot asked: `nil` for an added item, and for one that was already gone.
        var item: String? = nil
    }
    let title: String
    let changes: [Change]
    init(title: String, changes: [Change]) { self.title = title; self.changes = changes }
    private enum CodingKeys: String, CodingKey { case title, changes }
    /// Never fails: one unreadable request must not hide the conversation it is in. It then has no changes and is not shown as a request.
    init(from decoder: Decoder) throws {
        let c = try? decoder.container(keyedBy: CodingKeys.self)
        title = (try? c?.decode(String.self, forKey: .title)) ?? ""
        changes = (try? c?.decode([Change].self, forKey: .changes)) ?? []
    }
    /// Every change is one the list knows, so each can be said in words.
    var readable: Bool {
        !changes.isEmpty && changes.allSatisfy { ["add", "update", "remove"].contains($0.op) && ($0.op != "add" || $0.text != nil) && ($0.op != "update" || $0.text != nil || $0.done != nil) }
    }
}
/// The requester's reply to such a request. Anything but the two decisions reads as no decision.
struct TeamWaitDecision: Decodable, Equatable, Sendable {
    let approved: Bool?
    init(approved: Bool?) { self.approved = approved }
    init(from decoder: Decoder) throws {
        let raw = try? decoder.singleValueContainer().decode(String.self)
        approved = raw == "approve" ? true : raw == "deny" ? false : nil
    }
}
struct TeamMessage: Decodable, Equatable, Identifiable, Sendable {
    struct Author: Decodable, Equatable, Sendable {
        let kind: String; let uid: String; let sharedBotId: String?; let displayName: String?
        private enum CodingKeys: String, CodingKey { case kind, uid, sharedBotId, displayName }
        init(kind: String, uid: String = "", sharedBotId: String? = nil, displayName: String? = nil) {
            self.kind = kind; self.uid = uid; self.sharedBotId = sharedBotId; self.displayName = displayName
        }
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self); kind = try c.decode(String.self, forKey: .kind)
            displayName = try c.decodeIfPresent(String.self, forKey: .displayName)
            switch kind {
            case "user": uid = try c.decode(String.self, forKey: .uid); sharedBotId = nil
            case "shared_bot": uid = ""; sharedBotId = try c.decode(String.self, forKey: .sharedBotId)
            default: throw TeamMessageFailure(code: "INVALID_DATA")
            }
        }
    }
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
    var taskIds: [String]? = nil
    var taskId: String? = nil
    var artifactId: String? = nil
    /// On a Bot's question and on the answer to it: the wait they belong to.
    var waitId: String? = nil
    /// On a Bot's request to approve a list change.
    var approval: TeamListApproval? = nil
    /// On the requester's reply to such a request.
    var decision: TeamWaitDecision? = nil
    /// Files the message carries, in the order they were added.
    var attachments: [TeamMessageAttachment]? = nil
    /// On a Bot's reply: the files of this conversation its model was given.
    var readFiles: [TeamReadFile]? = nil
    /// The files to show: none on a recalled message.
    var files: [TeamMessageAttachment] { recalled == true ? [] : attachments ?? [] }
    /// The request to show under this message, if it is one a Bot made and it can be read.
    var approvalRequest: TeamListApproval? {
        guard author.kind != "user", recalled != true, waitId != nil, let approval, approval.readable else { return nil }
        return approval
    }

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
    /// Set when the message answers a Bot's question: it is sent as `answerWait` and continues that task instead of starting one.
    /// `replyTo` then names the question only for the pending bubble; the server links the answer itself.
    var waitId: String? = nil
    /// Checked uploads of this sender for this conversation that the new message carries. It may then have no text and ask no Bot.
    var attachments: [String]? = nil
    /// Their names, for the pending bubble only.
    var attachmentNames: [String]? = nil
    var error: String?
    var leaseUntil: Double?
    var attempted: Bool?
    var id: String { changeId }
    var body: [String: Any] {
        let targets = targets ?? [], attachments = attachments ?? []
        var payload: [String: Any] = ["conversationId": conversationId, "messageId": messageId, "changeId": changeId, "text": text]
        if let waitId {
            payload["waitId"] = waitId
            return ["schemaVersion": 1, "commandId": changeId, "teamId": teamId, "generation": generation, "action": "answerWait", "payload": payload]
        }
        if let replyTo { payload["replyTo"] = replyTo }
        if operation == "create", !targets.isEmpty || !attachments.isEmpty {
            payload["targets"] = targets
            if !attachments.isEmpty { payload["attachments"] = attachments }
            return ["schemaVersion": 1, "commandId": changeId, "teamId": teamId, "generation": generation, "action": "submit", "payload": payload]
        }
        payload["action"] = "mutate"; payload["operation"] = operation
        return payload
    }
}
struct TeamMessageCreation: Codable, Equatable, Sendable {
    var action = "create"
    let teamId: String
    let generation: Int
    let conversationId: String
    let kind: String
    let title: String
    let participantUids: [String]
    var sharedBotIds: [String]? = nil
    var error: String?
    var leaseUntil: Double?
    var attempted: Bool?
    var body: [String: Any] {
        var payload: [String: Any] = ["conversationId": conversationId, "kind": kind, "title": title, "participantUids": participantUids]
        if let sharedBotIds, !sharedBotIds.isEmpty {
            payload["sharedBotIds"] = sharedBotIds
            return ["schemaVersion": 1, "commandId": conversationId, "teamId": teamId, "generation": generation, "action": "createConversation", "payload": payload]
        }
        payload["action"] = "create"
        return payload
    }
}
struct TeamExecutionIntent: Codable, Equatable, Identifiable, Sendable {
    let commandId: String
    let action: String
    let sharedBotId: String
    let expectedRevision: Int
    var error: String?
    var leaseUntil: Double?
    var attempted: Bool?
    var id: String { commandId }
    var payload: [String: Any] { ["sharedBotId": sharedBotId, "expectedRevision": expectedRevision] }
}
struct TeamMessageProjection: Codable, Equatable, Sendable {
    struct Thread: Equatable, Sendable { var messages: [TeamMessage] = [] }
    let version: Int
    let uid: String
    let teamId: String
    let generation: Int
    private enum CodingKeys: String, CodingKey { case version, uid, teamId, generation, outbox, creation, commands }
    var conversations: [TeamMessageConversation] = []
    var threads: [String: Thread] = [:]
    var outbox: [TeamMessageIntent] = []
    var creation: TeamMessageCreation?
    var commands: [TeamExecutionIntent]? = nil

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
        guard var raw = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw TeamMessageFailure(code: "STORAGE_FAILED") }
        var cleaned = raw.removeValue(forKey: "botOutbox") != nil
        if let creation = raw["creation"] as? [String: Any], (creation["botKeys"] as? [String])?.isEmpty == false { raw.removeValue(forKey: "creation"); cleaned = true }
        if let outbox = raw["outbox"] as? [[String: Any]] {
            let retained = outbox.filter { !(($0["targets"] as? [String])?.contains(where: { $0.hasPrefix("bot:") }) ?? false) }
            if retained.count != outbox.count { raw["outbox"] = retained; cleaned = true }
        }
        let value = try JSONDecoder().decode(TeamMessageProjection.self, from: JSONSerialization.data(withJSONObject: raw))
        guard value.version == 1, value.uid == uid, value.teamId == teamId, value.generation == generation else { throw TeamMessageFailure(code: "STORAGE_FAILED") }
        if cleaned { try JSONSerialization.data(withJSONObject: raw).write(to: file, options: [.atomic, .completeFileProtectionUnlessOpen]) }
        return value
    }
    func update(uid: String, teamId: String, generation: Int, _ reduce: @Sendable (inout TeamMessageProjection) throws -> Void) throws -> TeamMessageProjection {
        let old = try load(uid: uid, teamId: teamId, generation: generation)
        var value = old
        try reduce(&value)
        guard value != old else { return old }
        let file = try url(uid: uid, teamId: teamId, generation: generation)
        var raw: [String: Any] = [:]
        if let data = try? Data(contentsOf: file), let existing = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] { raw = existing }
        let encoded = try JSONSerialization.jsonObject(with: JSONEncoder().encode(value)) as! [String: Any]
        for key in ["version", "uid", "teamId", "generation", "outbox", "creation", "commands"] { raw[key] = encoded[key] }
        try JSONSerialization.data(withJSONObject: raw).write(to: file, options: [.atomic, .completeFileProtectionUnlessOpen])
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
        return value
    }
}
