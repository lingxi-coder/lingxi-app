import Foundation

struct TeamBotCard: Decodable, Identifiable, Sendable {
    let key: String
    let ownerUid: String
    let name: String
    let description: String
    let revision: Int
    let use: String
    let enabled: Bool
    let permitted: Bool
    let online: Bool
    var onlineUntil: Double? = nil
    var id: String { key }
    var selectable: Bool { enabled && permitted }
}
struct TeamBotOrigin: Codable, Sendable, Equatable { let uid: String; let generation: Int }
struct TeamBotQuestion: Codable, Equatable, Sendable { let id: String; let questions: [String] }
struct TeamBotState: Decodable, Sendable {
    let state: String
    let ownerUid: String
    let botKey: String
    let origin: TeamBotOrigin
    let leaseUntil: Double?
    let questionId: String?
    let answered: Bool

    func canAnswer(_ message: TeamMessage, uid: String, generation: Int, now: Double) -> Bool {
        message.author.kind == "bot" && message.kind == "question" && message.recalled != true
            && message.author.botKey == botKey && message.author.uid == ownerUid
            && message.origin == origin && origin == TeamBotOrigin(uid: uid, generation: generation)
            && message.question?.id == questionId && questionId != nil && !answered
            && state == "running" && (leaseUntil ?? 0) > now
    }
    func needsRecovery(now: Double) -> Bool {
        state == "recovery" || (["claimed", "running"].contains(state) && (leaseUntil ?? .infinity) <= now)
    }
    func canRecover(uid: String, now: Double) -> Bool {
        ownerUid == uid && needsRecovery(now: now)
    }
}

/// The exact answer or settlement is saved before transport. A lost response,
/// page exit or process restart must resend the same ID and body.
struct TeamBotIntent: Codable, Equatable, Identifiable, Sendable {
    let action: String
    let conversationId: String
    let triggerId: String
    let questionId: String?
    let answerId: String
    let answers: [String: String]?
    let botKey: String?
    let text: String?
    var attempted: Bool? = nil
    var leaseUntil: Double? = nil
    var error: String? = nil
    var id: String { answerId }
    var body: [String: Any] {
        var body: [String: Any] = ["action": action, "conversationId": conversationId, "triggerId": triggerId]
        if action == "botAnswer" {
            body["questionId"] = questionId; body["answerId"] = answerId; body["answers"] = answers
        } else {
            body["botKey"] = botKey; body["text"] = text
        }
        return body
    }
    static func isPermanentFailure(_ code: String) -> Bool {
        ["QUESTION_RECIPIENT_REQUIRED", "ANSWER_CONFLICT", "INVALID_ANSWER", "BOT_USE_REQUIRED", "BOT_PARTICIPANT_REQUIRED", "INVALID_BOT_TARGET", "BOT_OWNER_REQUIRED", "BOT_REVISION_CONFLICT", "BOT_RESULT_CONFLICT", "RECOVERY_NOT_REQUIRED", "TRIGGER_REQUIRED", "BOT_RUN_BLOCKED", "INVALID_INPUT"].contains(code)
    }
    mutating func recordFailure(_ code: String) {
        leaseUntil = nil
        if Self.isPermanentFailure(code) { error = code }
    }
    func canDiscard(now: Double) -> Bool {
        !(attempted == true && error == nil) && (leaseUntil ?? 0) <= now
    }
    static func validAnswers(_ answers: [String: String], for question: TeamBotQuestion) -> Bool {
        !question.questions.isEmpty && question.questions.count <= 4
            && Set(question.questions).count == question.questions.count
            && Set(answers.keys) == Set(question.questions)
            && answers.values.allSatisfy { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && $0.utf16.count <= 2000 }
    }
}

extension TeamMessage {
    /// Human text/mentions are presentation only; a missing trusted trigger is
    /// handled as absent state. The server alone authorizes execution.
    func botTriggers(in conversation: TeamMessageConversation) -> [String] {
        if author.kind == "bot" { return triggerId.map { [$0] } ?? [] }
        guard author.kind == "user" else { return [] }
        let keys = conversation.kind == "bot_dm" ? conversation.participants.filter { $0.hasPrefix("bot:") } : (mentions ?? [])
        return keys.filter { conversation.participants.contains($0) }.map { key in
            TeamBotTrigger.identity(messageId: id, botKey: key)
        }
    }
}

/// Draft validation uses the full current usable set. An omitted card is not
/// an available Bot: other owners' revoked cards are intentionally absent.
enum TeamBotSelection {
    static func unavailable(_ selected: Set<String>, cards: [TeamBotCard], participants: [String]? = nil) -> Set<String> {
        var usable = Set(cards.filter(\.selectable).map(\.key))
        if let participants { usable.formIntersection(participants) }
        return selected.subtracting(usable)
    }
    static func targets(in conversation: TeamMessageConversation, selected: Set<String>) -> Set<String> {
        conversation.kind == "bot_dm" ? Set(conversation.participants.filter { $0.hasPrefix("bot:") }) : selected
    }
    static func canSend(in conversation: TeamMessageConversation, selected: Set<String>, cards: [TeamBotCard]) -> Bool {
        let targets = targets(in: conversation, selected: selected)
        return (conversation.kind != "bot_dm" || targets.count == 1)
            && unavailable(targets, cards: cards, participants: conversation.participants).isEmpty
    }
}
