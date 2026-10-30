import FirebaseAuth
import FirebaseFirestore
import Foundation
import Observation

private struct TeamBotList: Decodable { let bots: [TeamBotCard] }
private struct TeamBotAck: Decodable {}
private struct TeamConversationList: Decodable { let conversations: [TeamMessageConversation]; let next: String? }
private struct TeamMessageAck: Decodable, Sendable { let lastReadSeq: Int?; let conversationId: String? }
private final class TeamMessageRedirectPolicy: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}

@MainActor @Observable
final class TeamMessagingModel {
    private(set) var cache: TeamMessageProjection?
    private(set) var offline = true
    private(set) var error = ""
    private(set) var revoked = false
    private(set) var bots: [TeamBotCard] = []
    private(set) var botStates: [String: TeamBotState] = [:]
    var selected = ""
    @ObservationIgnored private var lifecycle = 0
    @ObservationIgnored private var running = false
    @ObservationIgnored private var again = false
    @ObservationIgnored private var task: Task<Void, Never>?
    @ObservationIgnored private var poll: Task<Void, Never>?
    @ObservationIgnored private var listener: ListenerRegistration?
    @ObservationIgnored private var readListener: ListenerRegistration?
    @ObservationIgnored private var authListener: AuthStateDidChangeListenerHandle?
    private var uid = ""
    private var teamId = ""
    private var generation = 0
    private var active = false
    private var botStateWindow = 100
    private let apiRoot: URL
    private let session = URLSession(configuration: .ephemeral, delegate: TeamMessageRedirectPolicy(), delegateQueue: nil)

    init(apiRoot: URL = URL(string: "https://lingxios.net/v1/teams/messages")!) {
        self.apiRoot = apiRoot
    }

    func start(uid: String, team: TeamSummary) {
        stop(); active = true; lifecycle += 1
        self.uid = uid; teamId = team.id; generation = team.generation; revoked = false; offline = true
        cache = nil; selected = ""; bots = []; botStates = [:]; botStateWindow = 100
        let life = lifecycle
        authListener = Auth.auth().addStateDidChangeListener { [weak self] _, user in
            let current = user?.uid
            Task { @MainActor in guard let self, self.active, self.lifecycle == life else { return }; if current != uid { self.invalidate() } }
        }
        listener = Firestore.firestore().collection("teams").document(team.id).collection("conversations").whereField("memberUids", arrayContains: uid)
            .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                let wake = snapshot?.metadata.isFromCache == false || error != nil
                Task { @MainActor in guard let self, self.active, self.lifecycle == life else { return }; if wake { self.refresh() } }
            }
        poll = Task { [weak self] in
            do { try await self?.commit { _ in } } catch { self?.error = "STORAGE_FAILED"; return }
            while !Task.isCancelled { self?.refresh(); do { try await Task.sleep(for: .seconds(15)) } catch { return } }
        }
    }
    func stop() {
        active = false; lifecycle += 1; task?.cancel(); poll?.cancel(); task = nil; poll = nil
        listener?.remove(); readListener?.remove(); listener = nil; readListener = nil
        if let authListener { Auth.auth().removeStateDidChangeListener(authListener) }; authListener = nil
        running = false; again = false; bots = []; botStates = [:]
    }
    private func invalidate() {
        revoked = true; selected = ""; error = "MEMBERSHIP_REQUIRED"
        cache = nil
        let owner = uid, team = teamId, epoch = generation
        stop()
        Task { _ = try? await TeamMessageFiles.shared.update(uid: owner, teamId: team, generation: epoch) { $0.conversations = []; $0.threads = [:]; $0.outbox = []; $0.creation = nil; $0.botOutbox = nil } }
    }
    private func commit(_ reduce: @Sendable (inout TeamMessageProjection) throws -> Void) async throws {
        let life = lifecycle
        guard active else { throw CancellationError() }
        let value = try await TeamMessageFiles.shared.update(uid: uid, teamId: teamId, generation: generation, reduce)
        guard active, life == lifecycle else { throw CancellationError() }; cache = value
    }
    func select(_ id: String) {
        selected = id; botStates = [:]; botStateWindow = 100; readListener?.remove()
        let life = lifecycle
        readListener = Firestore.firestore().collection("teams").document(teamId).collection("conversations").document(id).collection("reads").document(uid)
            .addSnapshotListener { [weak self] snapshot, _ in
                let wake = snapshot?.metadata.isFromCache == false
                Task { @MainActor in guard let self, self.active, self.lifecycle == life else { return }; if wake { self.refresh() } }
            }
        refresh()
    }
    func showBotHistory(limit: Int) {
        botStateWindow = max(100, limit)
        refresh()
    }
    func refresh() {
        guard active, !revoked else { return }
        if running { again = true; return }
        running = true; let life = lifecycle
        task = Task { [weak self] in
            guard let self else { return }
            defer { if self.lifecycle == life { self.running = false; if self.again { self.again = false; self.refresh() } } }
            do {
                try await commit { _ in }
                let creationLease = Date().timeIntervalSince1970 * 1000 + 30000
                try await commit { value in if value.creation?.error == nil, (value.creation?.leaseUntil ?? 0) <= Date().timeIntervalSince1970 * 1000 { value.creation?.leaseUntil = creationLease; value.creation?.attempted = true } }
                if let creation = cache?.creation, creation.leaseUntil == creationLease {
                    do {
                        let body = try JSONSerialization.jsonObject(with: JSONEncoder().encode(creation)) as! [String: Any]
                        let _: TeamMessageAck = try await call(body.filter { $0.key != "error" && $0.key != "leaseUntil" && $0.key != "attempted" }, life: life)
                        try await commit { $0.creation = nil }
                        if selected.isEmpty { selected = creation.conversationId }
                    } catch {
                        let code = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
                        if ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED"].contains(code) { throw error }
                        let permanent = ["CONVERSATION_REJECTED", "PARTICIPANT_REQUIRED", "BOT_USE_REQUIRED", "INVALID_INPUT"].contains(code)
                        try await commit { value in value.creation?.leaseUntil = nil; if permanent { value.creation?.error = code } }
                        if !permanent { throw error }
                    }
                }
                var conversations: [TeamMessageConversation] = [], after: String?
                repeat {
                    var body: [String: Any] = ["action": "list"]
                    if let after { body["after"] = after }
                    let page: TeamConversationList = try await call(body, life: life)
                    conversations += page.conversations; after = page.next
                } while after != nil
                let cards: TeamBotList = try await call(["action": "bots"], life: life)
                bots = cards.bots
                let latestConversations = conversations
                try await commit { value in value.conversations = latestConversations.map { incoming in incoming.merging(value.conversations.first { $0.id == incoming.id }) } }
                try await commit { _ in }
                for pending in cache?.outbox ?? [] where pending.error == nil {
                    let intentID = pending.id
                    let lease = Date().timeIntervalSince1970 * 1000 + 30000
                    try await commit { value in
                        if let index = value.outbox.firstIndex(where: { $0.id == intentID }), value.outbox[index].error == nil, (value.outbox[index].leaseUntil ?? 0) <= Date().timeIntervalSince1970 * 1000 { value.outbox[index].leaseUntil = lease; value.outbox[index].attempted = true }
                    }
                    guard let intent = cache?.outbox.first(where: { $0.id == intentID && $0.leaseUntil == lease }) else { continue }
                    do {
                        let body = try JSONSerialization.jsonObject(with: JSONEncoder().encode(intent)) as! [String: Any]
                        let _: TeamMessageAck = try await call(body.filter { $0.key != "error" && $0.key != "leaseUntil" && $0.key != "attempted" }, life: life)
                        try await commit { $0.outbox.removeAll { $0.id == intent.id } }
                    } catch {
                        let code = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
                        let permanent = ["MESSAGE_ID_CONFLICT", "MESSAGE_RECALLED", "MESSAGE_AUTHOR_REQUIRED", "REPLY_REQUIRED", "BOT_USE_REQUIRED", "BOT_PARTICIPANT_REQUIRED", "INVALID_BOT_TARGET", "INVALID_INPUT"].contains(code)
                        try await commit { value in if let index = value.outbox.firstIndex(where: { $0.id == intent.id }) { value.outbox[index].leaseUntil = nil; if permanent { value.outbox[index].error = code } } }
                        if !permanent { throw error }
                    }
                }
                for pending in cache?.botOutbox ?? [] where pending.error == nil {
                    let intentID = pending.id, lease = Date().timeIntervalSince1970 * 1000 + 30000
                    try await commit { value in
                        if let index = value.botOutbox?.firstIndex(where: { $0.id == intentID }), value.botOutbox?[index].error == nil, (value.botOutbox?[index].leaseUntil ?? 0) <= Date().timeIntervalSince1970 * 1000 {
                            value.botOutbox?[index].leaseUntil = lease; value.botOutbox?[index].attempted = true
                        }
                    }
                    guard let intent = cache?.botOutbox?.first(where: { $0.id == intentID && $0.leaseUntil == lease }) else { continue }
                    do {
                        let _: TeamBotAck = try await call(intent.body, life: life)
                        try await commit { $0.acknowledgeBotIntent(intentID) }
                    } catch {
                        let code = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
                        let permanent = TeamBotIntent.isPermanentFailure(code)
                        try await commit { value in
                            if let index = value.botOutbox?.firstIndex(where: { $0.id == intentID }) {
                                value.botOutbox?[index].recordFailure(code)
                            }
                        }
                        if !permanent { throw error }
                    }
                }
                let id = selected
                if !id.isEmpty, conversations.contains(where: { $0.id == id }) {
                    var more: Bool
                    repeat {
                        let page: TeamMessagePage = try await call(["action": "sync", "conversationId": id, "after": cache?.threads[id]?.cursor ?? 0], life: life)
                        try await commit { try $0.apply(page) }; more = page.hasMore
                    } while more
                } else if !id.isEmpty { selected = "" }
                // Never authorize question controls from cached message metadata.
                // Selection may change while a page is in flight.
                if selected == id, !id.isEmpty {
                    var states: [String: TeamBotState] = [:]
                    let terminal = Set(["completed", "failed", "missed", "cancelled", "recovered", "revoked"])
                    let conversation = cache?.conversations.first { $0.id == id }
                    let triggers = Set((cache?.threads[id]?.messages ?? []).suffix(botStateWindow).flatMap { message in conversation.map { message.botTriggers(in: $0) } ?? [] })
                    for trigger in triggers {
                        // Terminal receipts never become executable again. Poll
                        // live requests within the visible history window only.
                        if let known = botStates[trigger], terminal.contains(known.state) { states[trigger] = known; continue }
                        do {
                            let state: TeamBotState = try await call(["action": "botState", "conversationId": id, "triggerId": trigger], life: life)
                            states[trigger] = state
                        } catch let failure as TeamMessageFailure where failure.code == "TRIGGER_REQUIRED" {
                            // SDK mention metadata alone has no execution receipt.
                            continue
                        }
                    }
                    if selected == id { botStates = states }
                }
                offline = false; error = ""
            } catch {
                guard active, life == lifecycle else { return }
                offline = true; bots = []; botStates = [:]; self.error = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
                if ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED"].contains(self.error) { invalidate() }
            }
        }
    }
    func create(kind: String, title: String, uids: [String], botKeys: [String] = [], id: String) async throws {
        guard active, !revoked else { return }
        guard TeamBotSelection.unavailable(Set(botKeys), cards: bots).isEmpty else { throw TeamMessageFailure(code: "BOT_USE_REQUIRED") }
        let intent = TeamMessageCreation(teamId: teamId, generation: generation, conversationId: id, kind: kind, title: title, participantUids: Array(Set([uid] + uids)).sorted(), botKeys: botKeys.isEmpty ? nil : botKeys.sorted())
        try await commit { value in guard value.creation == nil else { throw TeamMessageFailure(code: "CREATION_PENDING") }; value.creation = intent }
        selected = id; refresh()
    }
    func queue(operation: String, text: String, messageId: String? = nil, replyTo: String? = nil, targets: [String] = []) async throws {
        guard active, !revoked, !selected.isEmpty, operation == "recall" || (!text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && text.utf8.count <= 8192) else { throw TeamMessageFailure(code: "INVALID_INPUT") }
        if operation == "create" {
            guard let conversation = cache?.conversations.first(where: { $0.id == selected }), TeamBotSelection.canSend(in: conversation, selected: Set(targets), cards: bots) else { throw TeamMessageFailure(code: "BOT_USE_REQUIRED") }
        }
        let intent = TeamMessageIntent(teamId: teamId, generation: generation, conversationId: selected, messageId: messageId ?? UUID().uuidString, changeId: UUID().uuidString, operation: operation, text: text, replyTo: replyTo, targets: operation == "create" && !targets.isEmpty ? targets.sorted() : nil)
        try await commit { $0.outbox.append(intent) }; refresh()
    }
    func answer(_ message: TeamMessage, answers: [String: String]) async throws {
        guard active, !revoked, !offline, cache?.threads[selected]?.messages.contains(where: { $0.id == message.id }) == true, let trigger = message.triggerId, let question = message.question,
              botStates[trigger]?.canAnswer(message, uid: uid, generation: generation, now: Date().timeIntervalSince1970 * 1000) == true,
              TeamBotIntent.validAnswers(answers, for: question) else { throw TeamMessageFailure(code: "QUESTION_RECIPIENT_REQUIRED") }
        let intent = TeamBotIntent(action: "botAnswer", conversationId: selected, triggerId: trigger, questionId: question.id, answerId: UUID().uuidString, answers: answers, botKey: nil, text: nil)
        try await saveBotIntent(intent)
    }
    func recover(trigger: String, text: String) async throws {
        guard active, !revoked, !offline, let state = botStates[trigger], state.canRecover(uid: uid, now: Date().timeIntervalSince1970 * 1000),
              !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, text.utf8.count <= 8192 else { throw TeamMessageFailure(code: "RECOVERY_NOT_REQUIRED") }
        let intent = TeamBotIntent(action: "recoverBot", conversationId: selected, triggerId: trigger, questionId: nil, answerId: UUID().uuidString, answers: nil, botKey: state.botKey, text: text)
        try await saveBotIntent(intent)
    }
    private func saveBotIntent(_ intent: TeamBotIntent) async throws {
        try await commit { value in
            guard !(value.botOutbox ?? []).contains(where: { $0.conversationId == intent.conversationId && $0.triggerId == intent.triggerId && $0.action == intent.action && (intent.action != "botAnswer" || $0.questionId == intent.questionId) }) else { throw TeamMessageFailure(code: "MESSAGE_IN_FLIGHT") }
            if value.botOutbox == nil { value.botOutbox = [] }
            value.botOutbox?.append(intent)
        }
        refresh()
    }
    func discardBotIntent(_ id: String) async throws {
        try await commit { value in
            if (value.botOutbox ?? []).contains(where: { $0.id == id && !$0.canDiscard(now: Date().timeIntervalSince1970 * 1000) }) { throw TeamMessageFailure(code: "MESSAGE_IN_FLIGHT") }
            value.botOutbox?.removeAll { $0.id == id }
        }
        refresh()
    }
    func unbind(_ bot: TeamBotCard) async throws {
        guard bot.ownerUid == uid, bot.enabled, !offline else { throw TeamMessageFailure(code: "BOT_OWNER_REQUIRED") }
        let _: TeamBotAck = try await call(["action": "unbindBot", "botKey": bot.key, "expectedRevision": bot.revision], life: lifecycle)
        bots.removeAll { $0.key == bot.key }; refresh()
    }
    func canAnswer(_ message: TeamMessage, now: Double) -> Bool {
        guard !offline, let trigger = message.triggerId else { return false }
        return botStates[trigger]?.canAnswer(message, uid: uid, generation: generation, now: now) == true
            && cache?.hasPendingAnswer(for: message) != true
    }
    func canRecover(_ trigger: String, now: Double) -> Bool {
        !offline && botStates[trigger]?.canRecover(uid: uid, now: now) == true
            && !(cache?.botOutbox ?? []).contains { $0.triggerId == trigger && $0.action == "recoverBot" }
    }
    func discard(_ id: String) async throws { try await commit { value in
        if value.outbox.contains(where: { $0.id == id && (($0.attempted == true && $0.error == nil) || ($0.leaseUntil ?? 0) > Date().timeIntervalSince1970 * 1000) }) { throw TeamMessageFailure(code: "MESSAGE_IN_FLIGHT") }
        value.outbox.removeAll { $0.id == id }
    } }
    func discardCreation() async throws { try await commit { value in
        if (value.creation?.attempted == true && value.creation?.error == nil) || (value.creation?.leaseUntil ?? 0) > Date().timeIntervalSince1970 * 1000 { throw TeamMessageFailure(code: "MESSAGE_IN_FLIGHT") }; value.creation = nil
    } }
    func markRead() async throws {
        let id = selected, seq = cache?.threads[id]?.messages.map(\.seq).max() ?? 0
        let value: TeamMessageAck = try await call(["action": "read", "conversationId": id, "lastReadSeq": seq], life: lifecycle)
        try await commit { cache in if let index = cache.conversations.firstIndex(where: { $0.id == id }) { cache.conversations[index].lastReadSeq = max(cache.conversations[index].lastReadSeq, value.lastReadSeq ?? 0) } }
    }
    private func call<T: Decodable>(_ body: [String: Any], life: Int) async throws -> T {
        guard active, life == lifecycle, let user = Auth.auth().currentUser, user.uid == uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        let token = try await user.getIDToken()
        guard active, life == lifecycle, Auth.auth().currentUser?.uid == uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        var body = body; body["teamId"] = teamId; body["generation"] = generation
        var request = URLRequest(url: apiRoot)
        request.httpMethod = "POST"; request.timeoutInterval = 20; request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization"); request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await session.data(for: request)
        guard active, life == lifecycle, Auth.auth().currentUser?.uid == uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        guard let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) else {
            let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            let code = (object?["error"] as? [String: Any])?["code"] as? String ?? "SERVICE_UNAVAILABLE"
            if ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED"].contains(code) { invalidate() }
            throw TeamMessageFailure(code: code)
        }
        return try JSONDecoder().decode(T.self, from: data)
    }
}
