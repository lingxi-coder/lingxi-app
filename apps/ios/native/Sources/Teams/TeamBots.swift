import Foundation

struct TeamMemberRef: Codable, Equatable, Sendable { let uid: String; let generation: Int }
struct TeamUsePolicy: Codable, Equatable, Sendable { let mode: String; var members: [TeamMemberRef]? }
struct TeamRequestQuotas: Codable, Equatable, Sendable { let team: Int; let member: Int; let bot: Int }
struct TeamBotAutoReply: Decodable, Equatable, Sendable { let daily: Int }
/// Only member-safe state crosses this boundary; detailed host/model failures stay on Desktop.
enum TeamBotAvailability: String, Decodable, Equatable, Sendable {
    case ready, paused, offline, maintenance, quota_exhausted, unverified
    init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: value) ?? .unverified
    }
}
struct TeamBotCapabilities: Decodable, Equatable, Sendable {
    let durableWait: Bool
    let writeOperations: Bool
}
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
    /// Present when the maintainer published the Bot to judge for itself whether to answer: `daily` caps those answers per UTC day.
    var autoReply: TeamBotAutoReply? = nil
    /// What the maintainer published of their computer for the Bot to use in the Team; `nil` when nothing. A Chat can narrow it (`TeamMessageConversation.botScopes`).
    var scope: TeamBotScope? = nil
    var examples: [String]? = nil
    var capabilities: TeamBotCapabilities? = nil
    var availability: TeamBotAvailability? = nil
    /// What a published Bot takes from other Bots and whom it may ask (Bot v3 B8): `acceptDelegation` is `none`, `team_bots` or `listed` (then `acceptFrom` lists the Bots), and `delegateTo` lists the Bots it may
    /// ask. All three are absent for a Bot published before; a code this version does not know takes nobody. The member only reads them: they are chosen where the Bot is published.
    var acceptDelegation: String? = nil
    var acceptFrom: [String]? = nil
    var delegateTo: [String]? = nil
    var dailyRemaining: Int? = nil
    /// UTC epoch milliseconds, already projected for this member by the service.
    var quotaResetsAt: Double? = nil
    /// Independent freshness boundary for the projected status, including maintenance.
    var statusExpiresAt: Double? = nil
    /// A known refusal survives stale/failed validation; only a fresh ready projection can release it.
    private var unavailableUntilRevalidated: TeamBotAvailability? = nil
    private enum CodingKeys: String, CodingKey {
        case sharedBotId, name, description, maintainer, revision, state, permitted, ready, onlineUntil, usePolicy, quotas, instanceId
        case sharedList, examples, capabilities, availability, dailyRemaining, quotaResetsAt, statusExpiresAt, autoReply, scope, acceptDelegation, acceptFrom, delegateTo
    }
    var newRequestBlock: TeamBotAvailability? {
        [.paused, .offline, .maintenance, .quota_exhausted].contains(memberAvailability) ? memberAvailability : unavailableUntilRevalidated
    }
    func awaitingStatusValidation() -> Self {
        var result = self
        result.unavailableUntilRevalidated = newRequestBlock
        result.availability = .unverified
        result.dailyRemaining = nil
        return result
    }
    func preservingAdmission(from previous: Self?, now: Double) -> Self {
        var result = expiringStatus(at: now)
        let freshReady = memberAvailability == .ready && (memberStatusExpiry.map { $0 > now } ?? false)
        if !freshReady { result.unavailableUntilRevalidated = result.newRequestBlock ?? previous?.newRequestBlock }
        return result
    }
    var memberStatusExpiry: Double? {
        let value = statusExpiresAt ?? (ready ? onlineUntil : nil)
        return value.flatMap { $0.isFinite && $0 > 0 ? $0 : nil }
    }
    func expiringStatus(at now: Double) -> Self {
        guard let expiry = memberStatusExpiry, expiry <= now else { return self }
        return awaitingStatusValidation()
    }
    var memberAvailability: TeamBotAvailability { availability ?? .unverified }
    var memberExamples: [String] {
        var seen = Set<String>()
        return Array((examples ?? []).map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty && seen.insert($0).inserted }.prefix(3))
    }
    var memberRemaining: Int? { dailyRemaining.flatMap { $0 >= 0 ? $0 : nil } }
    var memberQuotaReset: Double? { quotaResetsAt.flatMap { $0.isFinite && $0 > 0 ? $0 : nil } }
    var id: String { sharedBotId }
    var selectable: Bool { state == "published" && permitted }
    var avatarId: String { sharedBotId }
}
/// Member directory visibility is determined by the server response; use permission only gates new use.
enum TeamBotMemberRules {
    static func directory(_ cards: [TeamBotCard]) -> [TeamBotCard] {
        cards.filter { $0.state == "published" || $0.state == "paused" }
    }
    /// Only new Bot requests observe this gate. A never-verified projection defers to the server; a known refusal waits for revalidation.
    /// `waitsForHost`: in a group or channel a Bot whose Desktop is off is no refusal: the message is sent and its task waits for the Desktop (rule 6). A Bot DM says no at once.
    static func blockedAvailability(targets: Set<String>, cards: [TeamBotCard], operation: String = "create", answeringWait: Bool = false, waitsForHost: Bool = false) -> TeamBotAvailability? {
        guard operation == "create", !answeringWait else { return nil }
        return cards.lazy.filter { targets.contains($0.sharedBotId) }.compactMap(\.newRequestBlock).first { !(waitsForHost && $0 == .offline) }
    }
    static func validatedCards(_ incoming: [TeamBotCard], previous: [TeamBotCard], now: Double) -> [TeamBotCard] {
        incoming.map { card in card.preservingAdmission(from: previous.first { $0.sharedBotId == card.sharedBotId }, now: now) }
    }
    /// Preserve only known refusals on read failure; this grants no use permission to cards that used to be unavailable.
    static func failedValidation(_ cards: [TeamBotCard]) -> [TeamBotCard] {
        cards.filter { $0.newRequestBlock != nil }.map { $0.awaitingStatusValidation() }
    }
    static func nextStatusRefresh(_ cards: [TeamBotCard], now: Double) -> Double? {
        cards.compactMap(\.memberStatusExpiry).filter { $0 > now }.min()
    }
}
/// What a member reads about a Bot in the list, picker, details and conversation header. The words are catalog keys, the `teams_bot_*` keys
/// Desktop and Web read too (`packages/teams/BotMemberDetails.tsx`), so every client says the same thing in every language. These rules
/// choose the keys and the figures; `TeamsStrings.swift` puts them into the UI language.
enum TeamBotMemberText {
    /// One "label: value" item of the member text. The label is a catalog key; the value is another key (Supported, Not supported, Unknown)
    /// or a figure that needs no translation (a count, a UTC time).
    struct Item: Equatable, Sendable {
        enum Value: Equatable, Sendable { case key(String), figure(String) }
        let label: String
        let value: Value
    }

    /// How a Bot's availability reads. A Bot whose sharing stopped reads "Sharing stopped" (`teams_bot_disabled`, docs/TEAMS-UI-POLISH.md U1).
    static func availabilityKey(_ value: TeamBotAvailability) -> String {
        switch value {
        case .ready: "teams_bot_avail_ready"
        case .paused: "teams_bot_disabled"
        case .offline: "teams_bot_avail_offline"
        case .maintenance: "teams_bot_avail_maintenance"
        case .quota_exhausted: "teams_bot_avail_quota"
        case .unverified: "teams_bot_avail_unverified"
        }
    }
    /// Wait and resume, and write operations, each Supported or Not supported. A Bot that has not reported them is Unknown for both.
    static func capabilities(_ value: TeamBotCapabilities?) -> [Item] {
        func supported(_ on: Bool?) -> Item.Value { .key(on.map { $0 ? "teams_bot_supported" : "teams_bot_not_supported" } ?? "chat_status_unknown") }
        return [Item(label: "teams_bot_wait_resume", value: supported(value?.durableWait)),
                Item(label: "teams_bot_write_ops", value: supported(value?.writeOperations))]
    }
    /// What a published Bot takes from other Bots and whom it may ask (Bot v3 B8): "Accepts delegation from: Planner, Travel · May delegate to: Travel". Nothing for a Bot that is not published, and no
    /// "May delegate to" for a Bot that may ask none. The limit its maintainer set is not shown: a member never reads the limits that were configured.
    static func delegation(_ bot: TeamBotCard, cards: [TeamBotCard]) -> [Item] {
        guard bot.state == "published" else { return [] }
        let accepts: Item.Value
        switch TeamDelegations.accepts(bot, cards: cards) {
        case .nobody: accepts = .key("teams_bot_accepts_none")
        case .teamBots: accepts = .key("teams_bot_accepts_team")
        case .listed(let names): accepts = .figure(names.joined(separator: ", "))
        }
        let asks = TeamDelegations.delegatesTo(bot, cards: cards)
        return [Item(label: "teams_bot_accepts_delegation", value: accepts)] + (asks.isEmpty ? [] : [Item(label: "teams_bot_delegates_to", value: .figure(asks.joined(separator: ", ")))])
    }
    /// The member's own requests left today, or Unknown when the service has not said.
    static func remaining(_ value: Int?) -> Item {
        Item(label: "teams_bot_remaining_today", value: value.map { .figure(String($0)) } ?? .key("chat_status_unknown"))
    }
    /// When the daily quota resets, in UTC; `nil` when the service reports no usable time.
    static func reset(_ value: Double?) -> Item? {
        guard let value, value.isFinite, value > 0 else { return nil }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        formatter.dateFormat = "yyyy-MM-dd HH:mm 'UTC'"
        return Item(label: "teams_bot_resets", value: .figure(formatter.string(from: Date(timeIntervalSince1970: value / 1000))))
    }
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
    /// Only for `kind` `delegation` (Bot v3 B8): the task waits for another Bot's reply, which nobody types, so the `recipient` (the person who asked) has nothing to answer.
    var delegation: TeamTaskWaitDelegation? = nil
}
/// The Bot a task that waits for another Bot's reply waits for, and the task it asked.
struct TeamTaskWaitDelegation: Decodable, Equatable, Sendable {
    let sharedBotId: String
    let taskId: String
}
/// One Bot of a chain of delegations: its shared id and the name it had when it was asked. A chain made from a handoff may carry an id as the name.
struct TeamDelegationLink: Decodable, Equatable, Sendable {
    let sharedBotId: String
    let name: String
}
/// A task another Bot asked for (Bot v3 B8): `chain` runs from the Bot the person asked to the Bot that runs the task, and `parentBotId` is the Bot that asked for it.
struct TeamTaskDelegation: Decodable, Equatable, Sendable {
    let parentBotId: String
    let chain: [TeamDelegationLink]
}
struct TeamTask: Decodable, Equatable, Identifiable, Sendable {
    let taskId: String
    let conversationId: String
    let sharedBotId: String
    let requester: TeamMemberRef
    let state: String
    let createdAt: Double
    /// Ends the current phase: the pick-up limit while queued, the running budget while running, the wait's expiry while waiting, the maintainer's time to decide while the reply awaits approval.
    let deadline: Double
    let replyId: String?
    let artifactId: String?
    var usage: TeamTaskUsage? = nil
    var cancelRequestedAt: Double? = nil
    var wait: TeamTaskWait? = nil
    /// Present only while queued: the message was sent while the Bot's Desktop was off, so the task waits up to a day for it instead of five minutes.
    var hostWait: Bool? = nil
    /// Another Bot asked for this task (Bot v3 B8).
    var delegation: TeamTaskDelegation? = nil
    var id: String { taskId }
    var terminal: Bool { !["queued", "running", "waiting", "awaiting_approval"].contains(state) }
    /// A worker has it or is about to: its state changes within seconds. A waiting task changes only with its answer or its expiry, and one whose reply awaits approval with the maintainer's decision or the end of their time.
    var busy: Bool { state == "queued" || state == "running" }
    /// Cancellation was asked while the Bot was working; the task ends at the worker's next check.
    var cancelling: Bool { state == "running" && cancelRequestedAt != nil }
    /// Queued for a Desktop that was off when the message was sent (rule 6): the card says the Bot answers when it is back online. Once the day is over the task reads Expired.
    var waitsForHost: Bool { state == "queued" && hostWait == true }
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
    /// The requester and the Bot's maintainer may stop a task that has not ended; so may the maintainer of the Bot that asked for it, when another Bot did (`delegator`, the service's `canCancel`).
    static func canCancel(_ task: TeamTask, viewer: TeamMemberRef, maintainer: TeamMemberRef?, delegator: TeamMemberRef?) -> Bool {
        !task.terminal && !task.cancelling && (task.requester == viewer || maintainer == viewer || delegator == viewer)
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
/// What a task and a Bot's card say of Bots asking Bots (Bot v3 B8), without a screen and without words of their own: the run card, the Bot's card and the auto-reply log put them into the language of the person.
/// `packages/teams/delegation.ts` says the same on Web and Desktop, and Android's `TeamDelegations` too.
enum TeamDelegations {
    /// What a Bot is called: by its card as the Team lists it now, else by the name it had when it was asked. An id is never a name.
    static func name(of sharedBotId: String, cards: [TeamBotCard], asked: String? = nil) -> String? {
        if let named = cards.first(where: { $0.sharedBotId == sharedBotId })?.name, !named.isEmpty { return named }
        guard let asked, !asked.isEmpty, asked != sharedBotId else { return nil }
        return asked
    }
    /// "A → B": the Bots from the one the person asked to the one that runs the task. Nothing for a task nobody delegated. `unnamed` stands for a Bot with no name to give.
    static func chain(_ task: TeamTask, cards: [TeamBotCard], unnamed: String) -> String? {
        guard let chain = task.delegation?.chain, chain.count > 1 else { return nil }
        return chain.map { name(of: $0.sharedBotId, cards: cards, asked: $0.name) ?? unnamed }.joined(separator: " → ")
    }
    /// The Bot a task that waits for another Bot's reply waits for: by its card, else by the name it had when it was asked, which the task it asked carries in its chain when that task is among
    /// `known`. Nothing for a task that waits for something else, or a Bot with no name to give.
    static func waitingFor(_ task: TeamTask, cards: [TeamBotCard], known: [TeamTask] = []) -> String? {
        guard task.state == "waiting", let asked = task.wait?.delegation else { return nil }
        let link = known.first { $0.taskId == asked.taskId }?.delegation?.chain.first { $0.sharedBotId == asked.sharedBotId }
        return name(of: asked.sharedBotId, cards: cards, asked: link?.name)
    }
    /// The maintainer of the Bot that asked for a task another Bot asked for, when its card is known: one more person who may stop the task.
    static func delegator(_ task: TeamTask, cards: [TeamBotCard]) -> TeamMemberRef? {
        task.delegation.flatMap { asked in cards.first { $0.sharedBotId == asked.parentBotId }?.maintainer }
    }
    /// The Bot that passed a message on, for a row of the auto-reply log, by the name it has in the chain; nothing for a message nobody delegated. A Bot with no name to give is `unnamed`.
    static func delegatedBy(_ delegation: TeamTaskDelegation?, unnamed: String) -> String? {
        guard let delegation else { return nil }
        let name = delegation.chain.first { $0.sharedBotId == delegation.parentBotId }?.name
        return name.flatMap { $0.isEmpty || $0 == delegation.parentBotId ? nil : $0 } ?? unnamed
    }
    /// What a published Bot takes from other Bots: nobody, any Bot of the Team, or the Bots it lists that are still in the Team (a list of Bots that are all gone takes nobody).
    enum Accepts: Equatable, Sendable { case nobody, teamBots, listed([String]) }
    static func accepts(_ bot: TeamBotCard, cards: [TeamBotCard]) -> Accepts {
        if bot.acceptDelegation == "team_bots" { return .teamBots }
        let listed = bot.acceptDelegation == "listed" ? names(of: bot.acceptFrom, cards: cards) : []
        return listed.isEmpty ? .nobody : .listed(listed)
    }
    /// The Bots a published Bot may hand part of a request to, by name; the ones no longer in the Team are left out.
    static func delegatesTo(_ bot: TeamBotCard, cards: [TeamBotCard]) -> [String] { names(of: bot.delegateTo, cards: cards) }
    private static func names(of ids: [String]?, cards: [TeamBotCard]) -> [String] {
        (ids ?? []).compactMap { id in cards.first { $0.sharedBotId == id }?.name }
    }
}
struct TeamArtifact: Decodable, Equatable, Identifiable, Sendable {
    struct Document: Decodable, Equatable, Sendable { let title: String; let body: String }
    let artifactId: String
    let revision: Int
    let digest: String
    let document: Document
    /// The latest revision of this document; `revision` is older when an entry of the history is shown.
    let currentRevision: Int
    var id: String { artifactId }
    var old: Bool { revision < currentRevision }
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
    /// The most rows one page of a Bot's auto-reply log has.
    static let triageLimit = 50
    /// The most Bots of a Chat that may answer one message nobody asked them about, and what a message that mentions only people does to them (`TeamConversationAutoReply`).
    static let maxRepliesLimit = 5
    static let humanMentionModes: Set<String> = ["quiet", "judge"]
    /// The longest reply a maintainer may approve with their own text, trimmed, in UTF-8 bytes.
    static let heldReplyBytes = 8192
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
        case "listHistory":
            try shape(["sharedBotId"], ["beforeRevision"]); try id("sharedBotId")
            if payload["beforeRevision"] != nil { guard integer(payload["beforeRevision"], minimum: 1) != nil else { try fail() } }
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
        case "listTriage":
            // The maintainer's read of a Bot's auto-reply log: newest first, optionally of one Chat, from a cursor, at most one page of rows.
            try shape(["sharedBotId"], ["conversationId", "cursor", "limit"]); try id("sharedBotId")
            for key in ["conversationId", "cursor"] where payload[key] != nil { try id(key) }
            if payload["limit"] != nil { guard let limit = integer(payload["limit"], minimum: 1), limit <= Double(triageLimit) else { try fail() } }
        case "listHeldReplies":
            // The maintainer's read of the replies of a Bot that wait for them, optionally of one Chat. Holding a reply is the Bot's host's command, which no member sends.
            try shape(["sharedBotId"], ["conversationId"]); try id("sharedBotId")
            if payload["conversationId"] != nil { try id("conversationId") }
        case "approveReply":
            // The task, and the reply as the maintainer changed it to: trimmed, with something in it, and no longer than a Bot's own.
            try shape(["taskId"], ["text"]); try id("taskId")
            if payload["text"] != nil {
                guard let text = payload["text"] as? String, case let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines), !trimmed.isEmpty, trimmed.utf8.count <= heldReplyBytes else { try fail() }
            }
        case "rejectReply": try shape(["taskId"]); try id("taskId")
        case "pauseSharedBot":
            try shape(["sharedBotId", "expectedRevision"]); try id("sharedBotId")
            guard integer(payload["expectedRevision"], minimum: 1) != nil else { try fail() }
        case "setConversationPolicy", "setConversationAutoReply":
            // The two settings of the Chat's auto-reply are left out by a caller that does not change them; present, each is whole and known.
            let autoReply = action == "setConversationAutoReply"
            try shape(["conversationId", "expectedRevision", "targets"], autoReply ? ["maxReplies", "humanMentions"] : []); try id("conversationId"); try ids("targets", minimum: 0)
            guard integer(payload["expectedRevision"], minimum: 0) != nil else { try fail() }
            if payload["maxReplies"] != nil { guard let most = integer(payload["maxReplies"], minimum: 1), most <= Double(maxRepliesLimit) else { try fail() } }
            if payload["humanMentions"] != nil { guard let mode = payload["humanMentions"] as? String, humanMentionModes.contains(mode) else { try fail() } }
        case "setConversationBotScope":
            // The scope is the whole narrowing or `null`; a key that is left out is not a lifted narrowing.
            try shape(["conversationId", "sharedBotId", "expectedRevision", "scope"]); try id("conversationId"); try id("sharedBotId")
            guard integer(payload["expectedRevision"], minimum: 0) != nil, TeamBotScopeRules.validNarrowing(payload["scope"]) else { try fail() }
        case "submit":
            // A message that asks Bots, carries files, or both.
            try shape(["conversationId", "messageId", "changeId", "text", "targets"], ["replyTo", "attachments"])
            for key in ["conversationId", "messageId", "changeId"] { try id(key) }
            let files = payload["attachments"] != nil
            try ids("targets", minimum: files ? 0 : 1)
            if files { try ids("attachments", maximum: TeamAttachments.messageFiles) }
            if payload["replyTo"] != nil { try id("replyTo") }
            guard let text = payload["text"] as? String, files || !text.isEmpty, text.utf8.count <= 8192 else { try fail() }
        case "createUpload":
            try shape(["conversationId", "uploadId", "name", "mime", "byteLength", "sha256"]); try id("conversationId"); try id("uploadId")
            guard let name = payload["name"] as? String, name == TeamAttachments.name(name),
                  let mime = payload["mime"] as? String, TeamAttachments.types.contains(mime),
                  let length = integer(payload["byteLength"], minimum: 1), length <= Double(TeamAttachments.fileBytes),
                  let digest = payload["sha256"] as? String, digest.range(of: "^[a-f0-9]{64}$", options: .regularExpression) != nil else { try fail() }
        case "putUploadPart":
            try shape(["uploadId", "index", "data"]); try id("uploadId")
            guard let index = integer(payload["index"], minimum: 0), index <= Double((TeamAttachments.fileBytes - 1) / TeamAttachments.partBytes),
                  let data = payload["data"] as? String, data.utf8.count >= 4, data.utf8.count <= 4 * ((TeamAttachments.partBytes + 2) / 3),
                  data.range(of: "^[A-Za-z0-9+/]+={0,2}$", options: .regularExpression) != nil else { try fail() }
        case "finalizeUpload", "cancelUpload": try shape(["uploadId"]); try id("uploadId")
        case "readAttachmentPart":
            try shape(["attachmentId", "index"]); try id("attachmentId")
            guard let index = integer(payload["index"], minimum: 0), index <= Double((TeamAttachments.fileBytes - 1) / TeamAttachments.partBytes) else { try fail() }
        case "revokeAttachment": try shape(["attachmentId", "changeId"]); try id("attachmentId"); try id("changeId")
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

/// Which Bots of a Chat judge for themselves whether to answer its messages. Any person in a group, channel or Bot DM may change it.
enum TeamAutoReply {
    static let kinds: Set<String> = ["group", "channel", "bot_dm"]
    /// The Bots that can be switched on (published to answer on their own), and those already on, which can always be switched off.
    /// `kept`: Bots of the Chat already on that this person cannot see. The setting is the whole list, so a change here leaves them on.
    static func bots(_ conversation: TeamMessageConversation, cards: [TeamBotCard], uid: String) -> (bots: [TeamBotCard], on: [String], kept: [String]) {
        guard kinds.contains(conversation.kind), conversation.participants.contains("user:" + uid) else { return ([], [], []) }
        let on = conversation.autoReply?.targets ?? []
        let bots = cards.filter { card in conversation.participants.contains("shared_bot:" + card.sharedBotId) && (on.contains(card.sharedBotId) || available(card)) }
        let shown = { (id: String) -> Bool in bots.contains { $0.sharedBotId == id } }
        return (bots, on.filter(shown), on.filter { !shown($0) && conversation.participants.contains("shared_bot:" + $0) })
    }
    static func available(_ card: TeamBotCard) -> Bool { card.state == "published" && card.autoReply != nil }
    /// The setting after one Bot is switched, in the Chat's Bot order, with the Bots this person cannot see left as they are.
    static func targets(_ bots: [TeamBotCard], on: [String], kept: [String] = [], switching id: String, to enabled: Bool) -> [String] {
        bots.map(\.sharedBotId).filter { $0 == id ? enabled : on.contains($0) } + kept
    }
    /// The setting as it stands, for a change of one of the other two settings.
    static func targets(_ bots: [TeamBotCard], on: [String], kept: [String] = []) -> [String] { bots.map(\.sharedBotId).filter(on.contains) + kept }
    /// How many Bots of a group or channel may answer one message nobody asked them about, and whether a message that mentions only people is judged too. A Bot DM has the one Bot,
    /// which is always the one asked, so neither means anything there. Until someone sets them it is one Bot, and a message to people alone is theirs.
    static func hasRounds(_ conversation: TeamMessageConversation) -> Bool { conversation.kind == "group" || conversation.kind == "channel" }
    static func maxReplies(_ conversation: TeamMessageConversation) -> Int { min(max(conversation.autoReply?.maxReplies ?? 1, 1), TeamExecutionContract.maxRepliesLimit) }
    static func judgesHumanMentions(_ conversation: TeamMessageConversation) -> Bool { conversation.autoReply?.humanMentions == "judge" }
}
