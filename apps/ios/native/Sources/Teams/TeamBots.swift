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
    /// The list this Bot shares with the people who may use it, when it shares one.
    var sharedList: TeamBotListInfo? = nil
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
/// The answer a waiting task is blocked on; present only while `state` is `waiting`. `messageId` is the Bot's question.
struct TeamTaskWait: Decodable, Equatable, Sendable {
    let waitId: String
    let kind: String
    let recipient: TeamMemberRef
    let expiresAt: Double
    let messageId: String
}
struct TeamTask: Decodable, Equatable, Identifiable, Sendable {
    let taskId: String
    let conversationId: String
    let sharedBotId: String
    let requester: TeamMemberRef
    let state: String
    let createdAt: Double
    /// Ends the current phase: the pick-up limit while queued, the running budget while running, the wait's expiry while waiting.
    let deadline: Double
    let replyId: String?
    let artifactId: String?
    var usage: TeamTaskUsage? = nil
    var cancelRequestedAt: Double? = nil
    var wait: TeamTaskWait? = nil
    var id: String { taskId }
    var terminal: Bool { !["queued", "running", "waiting"].contains(state) }
    /// A worker has it or is about to: its state changes within seconds. A waiting task changes only with its answer or its expiry.
    var busy: Bool { state == "queued" || state == "running" }
    /// Cancellation was asked while the Bot was working; the task ends at the worker's next check.
    var cancelling: Bool { state == "running" && cancelRequestedAt != nil }
}
/// The identity of one decision on a request. Made once and sent again unchanged on a retry.
struct TeamDecisionIds: Equatable, Sendable {
    var commandId = UUID().uuidString
    var messageId = UUID().uuidString
    var changeId = UUID().uuidString
}
/// Where a Bot's request to change its list stands for the viewer (Desktop `ApprovalCard`).
enum TeamApprovalState: Equatable, Sendable {
    /// The task waits for the viewer's decision.
    case decide
    /// The task waits for this member's decision.
    case waiting(uid: String)
    case approved(uid: String)
    case declined(uid: String)
    /// It ended without a decision: it expired, or its task was cancelled or lost its authorization.
    case closed
}
/// What the viewer may do with a task (Desktop `BotRun`).
enum TeamTaskActions {
    enum ExpiryUnit: Equatable, Sendable { case minutes, hours, days }
    /// The requester and the Bot's maintainer may stop a task that has not ended.
    static func canCancel(_ task: TeamTask, viewer: TeamMemberRef, maintainer: TeamMemberRef?) -> Bool {
        !task.terminal && !task.cancelling && (task.requester == viewer || maintainer == viewer)
    }
    /// The wait an answer from the viewer would resolve: the task waits for exactly this viewer and no answer to it is already on its way.
    static func answerableWait(_ task: TeamTask?, viewer: TeamMemberRef, pendingWaitIds: Set<String>) -> String? {
        // A proposed change is approved or declined on its own card, never answered in words.
        guard let task, task.state == "waiting", let wait = task.wait, wait.kind == "human_input", wait.recipient == viewer, !pendingWaitIds.contains(wait.waitId) else { return nil }
        return wait.waitId
    }
    /// Where the request `waitId` stands: open while its task waits on it, then decided by the reply that carries the decision, else closed.
    static func approvalState(waitId: String, task: TeamTask?, viewer: TeamMemberRef, messages: [TeamMessage]) -> TeamApprovalState {
        if let task, task.state == "waiting", let wait = task.wait, wait.waitId == waitId, wait.kind == "tool_approval" {
            return wait.recipient == viewer ? .decide : .waiting(uid: wait.recipient.uid)
        }
        guard let reply = messages.first(where: { $0.waitId == waitId && $0.author.kind == "user" && $0.decision?.approved != nil }) else { return .closed }
        return reply.decision?.approved == true ? .approved(uid: reply.author.uid) : .declined(uid: reply.author.uid)
    }
    /// How long until `at` (both in milliseconds), rounded up to whole minutes, hours (under 48) or days; never less than one minute.
    static func expiry(at: Double, now: Double) -> (value: Int, unit: ExpiryUnit) {
        let minutes = max(1, Int(((at - now) / 60_000).rounded(.up)))
        if minutes < 60 { return (minutes, .minutes) }
        let hours = Int((Double(minutes) / 60).rounded(.up))
        return hours < 48 ? (hours, .hours) : (Int((Double(hours) / 24).rounded(.up)), .days)
    }
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
    /// The largest shared document body the service stores, in UTF-8 bytes.
    static let documentBytes = 256 * 1024
    /// The longest text of one shared-list item, in UTF-8 bytes, and the most changes one write carries.
    static let listItemBytes = 500, listChanges = 20
    /// One change of a shared-list write exactly as the service accepts it.
    private static func listChange(_ change: [String: Any]) -> Bool {
        func text(_ value: Any?) -> Bool {
            guard let text = value as? String else { return false }
            return !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && text.utf8.count <= listItemBytes && !text.contains("\0")
        }
        func flag(_ value: Any?) -> Bool { (value as? NSNumber).map { CFGetTypeID($0) == CFBooleanGetTypeID() } ?? false }
        let keys = Set(change.keys)
        switch change["op"] as? String {
        case "add": return keys == ["op", "text"] && text(change["text"])
        case "update":
            return keys.isSubset(of: ["op", "itemId", "text", "done"]) && identifier(change["itemId"]) && (change["text"] != nil || change["done"] != nil)
                && (change["text"] == nil || text(change["text"])) && (change["done"] == nil || flag(change["done"]))
        case "remove": return keys == ["op", "itemId"] && identifier(change["itemId"])
        default: return false
        }
    }
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
        case "readArtifact":
            try shape(["artifactId"], ["revision"]); try id("artifactId")
            if payload["revision"] != nil { guard integer(payload["revision"], minimum: 1) != nil else { try fail() } }
        case "listArtifactRevisions": try shape(["artifactId"]); try id("artifactId")
        case "reviseArtifact":
            try shape(["artifactId", "expectedRevision", "operationId", "document"]); try id("artifactId"); try id("operationId")
            guard integer(payload["expectedRevision"], minimum: 1) != nil, let document = payload["document"] as? [String: Any],
                  Set(document.keys) == ["title", "body"], let title = document["title"] as? String, let body = document["body"] as? String,
                  case let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines), !trimmed.isEmpty, trimmed.utf16.count <= 120,
                  !body.isEmpty, body.utf8.count <= documentBytes else { try fail() }
        case "listOperations": try shape(["taskId"]); try id("taskId")
        case "readList": try shape(["sharedBotId"]); try id("sharedBotId")
        case "writeList":
            try shape(["sharedBotId", "operationId", "expectedDataRevision", "changes"]); try id("sharedBotId"); try id("operationId")
            guard integer(payload["expectedDataRevision"], minimum: 0) != nil, let changes = payload["changes"] as? [[String: Any]],
                  (1...listChanges).contains(changes.count), changes.allSatisfy(listChange) else { try fail() }
        case "resolveOperation":
            try shape(["taskId", "operationId", "resolution"]); try id("taskId")
            guard let operation = payload["operationId"] as? String, operation.range(of: "^[a-f0-9]{64}$", options: .regularExpression) != nil,
                  let resolution = payload["resolution"] as? String, ["succeeded", "failed"].contains(resolution) else { try fail() }
        case "cancelTask": try shape(["taskId"]); try id("taskId")
        case "answerWait":
            try shape(["conversationId", "waitId", "messageId", "changeId", "text"])
            for key in ["conversationId", "waitId", "messageId", "changeId"] { try id(key) }
            guard let text = payload["text"] as? String, !text.isEmpty, text.utf8.count <= 8192 else { try fail() }
        case "decideWait":
            try shape(["conversationId", "waitId", "messageId", "changeId", "decision"])
            for key in ["conversationId", "waitId", "messageId", "changeId"] { try id(key) }
            guard let decision = payload["decision"] as? String, ["approve", "deny"].contains(decision) else { try fail() }
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
