import Foundation

struct TeamMemberRef: Codable, Equatable, Sendable { let uid: String; let generation: Int }
struct TeamUsePolicy: Codable, Equatable, Sendable { let mode: String; var members: [TeamMemberRef]? }
struct TeamRequestQuotas: Codable, Equatable, Sendable { let team: Int; let member: Int; let bot: Int }
struct TeamBotCard: Decodable, Identifiable, Equatable, Sendable {
    let sharedBotId: String
    let name: String
    let description: String
    let maintainer: TeamMemberRef
    let revision: Int
    let state: String
    let permitted: Bool
    let ready: Bool
    let onlineUntil: Double
    let usePolicy: TeamUsePolicy
    let quotas: TeamRequestQuotas
    let instanceId: String?
    var id: String { sharedBotId }
    var selectable: Bool { state == "published" && permitted }
    var avatarId: String { sharedBotId }
}
/// What a Bot run cost, as the host reports it. `missing` carries no figures; `partial` and `complete` carry the
/// token counts, and the cost only when the host could price it (`costMicros` in millionths of `currency`).
struct TeamTaskUsage: Decodable, Equatable, Sendable {
    let status: String
    var inputTokens: Int? = nil
    var outputTokens: Int? = nil
    var costMicros: Double? = nil
    var currency: String? = nil
}
struct TeamTask: Decodable, Equatable, Identifiable, Sendable {
    let taskId: String
    let conversationId: String
    let sharedBotId: String
    let requester: TeamMemberRef
    let state: String
    let createdAt: Double
    let deadline: Double
    let replyId: String?
    let artifactId: String?
    var usage: TeamTaskUsage? = nil
    var id: String { taskId }
    var terminal: Bool { state != "queued" && state != "running" }
}
struct TeamArtifact: Decodable, Equatable, Identifiable, Sendable {
    struct Document: Decodable, Equatable, Sendable { let title: String; let body: String }
    let artifactId: String
    let revision: Int
    let digest: String
    let document: Document
    var id: String { artifactId }
}
/// IDs are raw SharedBot IDs in commands and explicit shared_bot principals in Chat participants.
enum TeamBotSelection {
    static func unavailable(_ selected: Set<String>, cards: [TeamBotCard], participants: [String]? = nil) -> Set<String> {
        var usable = Set(cards.filter(\.selectable).map(\.sharedBotId))
        if let participants { usable.formIntersection(participants.compactMap { $0.hasPrefix("shared_bot:") ? String($0.dropFirst(11)) : nil }) }
        return selected.subtracting(usable)
    }
    static func targets(in conversation: TeamMessageConversation, selected: Set<String>) -> Set<String> {
        conversation.kind == "bot_dm" ? Set(conversation.participants.compactMap { $0.hasPrefix("shared_bot:") ? String($0.dropFirst(11)) : nil }) : selected
    }
    static func canSend(in conversation: TeamMessageConversation, selected: Set<String>, cards: [TeamBotCard]) -> Bool {
        let targets = targets(in: conversation, selected: selected)
        return (conversation.kind != "bot_dm" || targets.count == 1) && unavailable(targets, cards: cards, participants: conversation.participants).isEmpty
    }
}

/// Member commands are validated independently of storage/execution versions.
/// Unknown fields and retired actions cannot enter the transport through a journal replay.
enum TeamExecutionContract {
    static func validate(_ value: [String: Any]) throws {
        func fail() throws -> Never { throw TeamMessageFailure(code: "INVALID_INPUT") }
        guard Set(value.keys) == ["schemaVersion", "commandId", "teamId", "generation", "action", "payload"],
              integer(value["schemaVersion"], minimum: 1) == 1,
              integer(value["generation"], minimum: 1) != nil,
              identifier(value["commandId"]), identifier(value["teamId"]),
              let action = value["action"] as? String, let payload = value["payload"] as? [String: Any] else { try fail() }
        func shape(_ required: Set<String>, _ optional: Set<String> = []) throws {
            guard required.isSubset(of: Set(payload.keys)), Set(payload.keys).isSubset(of: required.union(optional)) else { try fail() }
        }
        func id(_ key: String) throws { guard identifier(payload[key]) else { try fail() } }
        func ids(_ key: String, minimum: Int = 1, maximum: Int = 8) throws {
            guard let values = payload[key] as? [String], values.count >= minimum, values.count <= maximum,
                  Set(values).count == values.count, values.allSatisfy({ identifier($0) }) else { try fail() }
        }
        switch action {
        case "listSharedBots": try shape([])
        case "getTask": try shape(["taskId"]); try id("taskId")
        case "readArtifact": try shape(["artifactId"]); try id("artifactId")
        case "cancelTask": try shape(["taskId"]); try id("taskId")
        case "answerWait":
            try shape(["conversationId", "waitId", "messageId", "changeId", "text"])
            for key in ["conversationId", "waitId", "messageId", "changeId"] { try id(key) }
            guard let text = payload["text"] as? String, !text.isEmpty, text.utf8.count <= 8192 else { try fail() }
        case "listTasks":
            try shape([], ["conversationId", "after"])
            for key in payload.keys { try id(key) }
        case "pauseSharedBot":
            try shape(["sharedBotId", "expectedRevision"]); try id("sharedBotId")
            guard integer(payload["expectedRevision"], minimum: 1) != nil else { try fail() }
        case "setConversationPolicy":
            try shape(["conversationId", "expectedRevision", "targets"]); try id("conversationId"); try ids("targets", minimum: 0)
            guard integer(payload["expectedRevision"], minimum: 0) != nil else { try fail() }
        case "submit":
            try shape(["conversationId", "messageId", "changeId", "text", "targets"], ["replyTo"])
            for key in ["conversationId", "messageId", "changeId"] { try id(key) }; try ids("targets")
            if payload["replyTo"] != nil { try id("replyTo") }
            guard let text = payload["text"] as? String, !text.isEmpty, text.utf8.count <= 8192 else { try fail() }
        case "createConversation":
            try shape(["conversationId", "kind", "title", "participantUids", "sharedBotIds"]); try id("conversationId"); try ids("sharedBotIds")
            guard let kind = payload["kind"] as? String, ["bot_dm", "group", "channel"].contains(kind),
                  let title = payload["title"] as? String, !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, title.trimmingCharacters(in: .whitespacesAndNewlines).utf16.count <= 120,
                  let uids = payload["participantUids"] as? [String], !uids.isEmpty, uids.count <= 200, Set(uids).count == uids.count,
                  uids.allSatisfy({ $0.range(of: "^[A-Za-z0-9_-]{1,128}$", options: .regularExpression) != nil }) else { try fail() }
        default: try fail()
        }
    }
    private static func identifier(_ value: Any?) -> Bool {
        guard let value = value as? String else { return false }
        return value.range(of: "^[A-Za-z0-9_-]{8,64}$", options: .regularExpression) != nil
    }
    private static func integer(_ value: Any?, minimum: Double) -> Double? {
        guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else { return nil }
        let value = number.doubleValue
        return value.isFinite && value.rounded(.towardZero) == value && value >= minimum && value <= 9_007_199_254_740_991 ? value : nil
    }
}
