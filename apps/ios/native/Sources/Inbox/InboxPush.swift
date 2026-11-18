import Foundation

struct InboxNoticeTarget: Equatable, Sendable {
    let through: Int
    var messageId: String?
    var taskId: String?

    var exclusiveBefore: Int? {
        guard through > 0, through < 9_007_199_254_740_991 else { return nil }
        return through + 1
    }

    func matches(seq: Int, messageID: String, taskIDs: [String]) -> Bool {
        guard through >= seq, seq > 0 else { return false }
        if let messageId, messageId != messageID { return false }
        if let taskId { return taskIDs.contains(taskId) }
        return seq == through
    }
}

struct InboxAssistantPreviewBody: Decodable {
    struct Message: Decodable {
        struct Block: Decodable {
            let type: String
            let text: String?
        }

        let role: String
        let transcriptUUID: String?
        let blocks: [Block]

        private enum CodingKeys: String, CodingKey {
            case role, blocks
            case transcriptUUID = "transcript_uuid"
        }
    }

    let available: Bool
    let messages: [Message]

    static func text(from raw: String, verifying messageID: String?) -> [String] {
        let bytes = Data(raw.utf8)
        guard bytes.count <= 262_144,
              let body = try? JSONDecoder().decode(Self.self, from: bytes),
              body.available, body.messages.count <= 16,
              body.messages.allSatisfy({ $0.role == "assistant" }),
              messageID.map({ expected in body.messages.contains { $0.transcriptUUID == expected } }) ?? true
        else { return [] }
        return body.messages.flatMap { message in
            message.blocks.filter { $0.type == "text" }.compactMap(\.text).filter { !$0.isEmpty }
        }
    }
}

struct InboxNotificationPreview: Equatable, Identifiable, Sendable {
    let row: InboxRow
    let target: InboxNoticeTarget
    let id: String
}

struct InboxPush: Equatable, Sendable {
    let ref: InboxRef
    let recipient: String
    let through: Int
    let eventKind: String
    let eventID: String
    let expiresAt: Int64
    var messageId: String?
    var taskId: String?

    var target: InboxNoticeTarget { InboxNoticeTarget(through: through, messageId: messageId, taskId: taskId) }

    static var now: Int64 { Int64(Date().timeIntervalSince1970 * 1_000) }

    var isAction: Bool {
        ["approval", "input", "pending_input", "pending_approval", "waiting_for_input", "waiting_for_approval"].contains(eventKind)
    }

    static func parse(_ payload: [AnyHashable: Any], currentUID: String?, enabled: Bool,
                      now: Int64 = Self.now) -> Self? {
        guard enabled, let currentUID, payload["kind"] as? String == "inbox",
              payload["recipientUid"] as? String == currentUID,
              let key = payload["key"] as? String,
              let generation = integer(payload["generation"]),
              let ref = InboxRef(key: key, generation: generation),
              let rawRoute = payload["route"] as? String, rawRoute.hasPrefix("inbox/"),
              String(rawRoute.dropFirst(6)).removingPercentEncoding == key,
              let through = integer(payload["through"]), through >= 0,
              let eventKind = payload["eventKind"] as? String, InboxRef.safe(eventKind),
              let expiration = integer(payload["expiresAt"]), expiration > now else { return nil }
        let eventID = (payload["notificationId"] as? String).flatMap { $0.utf8.count <= 512 ? $0 : nil }
            ?? "\(key):\(generation):\(eventKind):\(through)"
        let messageID = payload["messageId"] as? String
        let taskID = payload["taskId"] as? String
        guard messageID.map(InboxRef.safe) ?? true, taskID.map(InboxRef.safe) ?? true else { return nil }
        return Self(ref: ref, recipient: currentUID, through: through, eventKind: eventKind,
                    eventID: eventID, expiresAt: Int64(expiration), messageId: messageID, taskId: taskID)
    }

    private static func integer(_ value: Any?) -> Int? {
        if let string = value as? String { return Int(string) }
        return value as? Int
    }
}
