import FirebaseAuth
import FirebaseFirestore
import Foundation
import Observation

private struct TeamBotList: Decodable { let bots: [TeamBotCard] }
private struct TeamBotAck: Decodable {}
private struct TeamBotStates: Decodable, Sendable { let states: [String: TeamBotState?] }
private struct TeamMessageAck: Decodable, Sendable { let lastReadSeq: Int?; let conversationId: String? }
private final class TeamMessageRedirectPolicy: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}

@MainActor @Observable
final class TeamMessagingModel {
    private static let conversationFields: Set<String> = ["kind", "title", "participants", "lastSeq", "lastChangeSeq"]
    private static let messageFields: Set<String> = ["seq", "author", "kind", "text", "replyTo", "createdAt", "editedAt", "recalled", "changeSeq", "changeId", "mentions", "triggerId", "origin", "question"]
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
    @ObservationIgnored private var retry: Task<Void, Never>?
    private var retryDelay = 1.0
    @ObservationIgnored private var deadline: Task<Void, Never>?
    @ObservationIgnored private var listener: ListenerRegistration?
    @ObservationIgnored private var messageListener: ListenerRegistration?
    @ObservationIgnored private var memberListener: ListenerRegistration?
    @ObservationIgnored private var signalListener: ListenerRegistration?
    @ObservationIgnored private var readListeners: [String: ListenerRegistration] = [:]
    @ObservationIgnored private var directoryCached = true
    @ObservationIgnored private var messagesCached = true
    private var membershipReady = false
    private var botsDirty = true
    @ObservationIgnored private var authListener: AuthStateDidChangeListenerHandle?
    private var uid = ""
    private var teamId = ""
    private var generation = 0
    private var active = false
    private var botStateWindow = 100
    private var terminalStates: [String: TeamBotState] = [:]
    private var missingStates: [String: String] = [:]
    private let auth: Auth
    private let db: Firestore
    private let apiRoot: URL
    private let session = URLSession(configuration: .ephemeral, delegate: TeamMessageRedirectPolicy(), delegateQueue: nil)

    init(apiRoot: URL = URL(string: "https://lingxios.net/v1/teams/messages")!, auth: Auth = Auth.auth(), db: Firestore = Firestore.firestore()) {
        self.apiRoot = apiRoot; self.auth = auth; self.db = db
    }

    func start(uid: String, team: TeamSummary) {
        stop(); active = true; lifecycle += 1
        self.uid = uid; teamId = team.id; generation = team.generation; revoked = false; offline = true
        cache = nil; selected = ""; directoryCached = true; messagesCached = true; botsDirty = true; bots = []; botStates = [:]; botStateWindow = 100
        let life = lifecycle
        authListener = auth.addStateDidChangeListener { [weak self] _, user in
            let current = user?.uid
            Task { @MainActor in guard let self, self.active, self.lifecycle == life else { return }; if current != uid { self.invalidate() } }
        }
        cache = TeamMessageProjection(version: 1, uid: uid, teamId: team.id, generation: team.generation)
        let root = db.collection("teams").document(team.id)
        memberListener = root.collection("members").document(uid).addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
            Task { @MainActor in
                guard let self, self.active, self.lifecycle == life, self.auth.currentUser?.uid == self.uid else { return }
                if let error { self.listenerFailed(error); return }
                if (snapshot?.data()?["generation"] as? NSNumber)?.intValue == self.generation {
                    if !self.membershipReady { self.membershipReady = true; self.listenDirectory(life: life) }
                } else if self.membershipReady || snapshot?.metadata.isFromCache == false { self.invalidate() }
            }
        }
        task = Task { [weak self] in
            guard let self else { return }
            do { try await self.reload(); self.refresh() } catch { self.error = "STORAGE_FAILED" }
        }
    }
    private func listenDirectory(life: Int) {
        let uid = self.uid, root = db.collection("teams").document(teamId)
        listener = root.collection("conversations").whereField("memberUids", arrayContains: uid)
            .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                Task { @MainActor in
                    guard let self, self.active, self.lifecycle == life, self.auth.currentUser?.uid == self.uid else { return }
                    if let error { self.listenerFailed(error); return }
                    guard let snapshot else { return }
                    do {
                        let previous = Dictionary(uniqueKeysWithValues: (self.cache?.conversations ?? []).map { ($0.id, $0) })
                        let conversations: [TeamMessageConversation] = try snapshot.documents.map { doc in
                            var data = doc.data().filter { Self.conversationFields.contains($0.key) }
                            data["id"] = doc.documentID; data["lastReadSeq"] = previous[doc.documentID]?.lastReadSeq ?? 0
                            return try self.decode(data)
                        }
                        self.cache?.conversations = conversations
                        if self.messageListener == nil, !self.selected.isEmpty, conversations.contains(where: { $0.id == self.selected }) { self.select(self.selected) }
                        let ids = Set(conversations.map(\.id))
                        let retainedThreads = self.cache?.threads.filter { ids.contains($0.key) } ?? [:]
                        self.cache?.threads = retainedThreads
                        for id in Array(self.readListeners.keys) where !ids.contains(id) { self.readListeners.removeValue(forKey: id)?.remove() }
                        for id in ids where self.readListeners[id] == nil {
                            self.readListeners[id] = root.collection("conversations").document(id).collection("reads").document(uid).addSnapshotListener(includeMetadataChanges: true) { [weak self] mark, error in
                                Task { @MainActor in
                                    guard let self, self.active, self.lifecycle == life, self.auth.currentUser?.uid == self.uid else { return }
                                    if let error { self.listenerFailed(error); return }
                                    let data = mark?.data()
                                    let seq = (data?["generation"] as? NSNumber)?.intValue == self.generation ? (data?["lastReadSeq"] as? NSNumber)?.intValue ?? 0 : 0
                                    if let index = self.cache?.conversations.firstIndex(where: { $0.id == id }), self.cache?.conversations[index].lastReadSeq != seq { self.cache?.conversations[index].lastReadSeq = seq }
                                }
                            }
                        }
                        self.missingStates = [:]
                        self.directoryCached = snapshot.metadata.isFromCache; self.updateOffline()
                        if !snapshot.metadata.isFromCache {
                            if !self.selected.isEmpty, !ids.contains(self.selected), self.cache?.creation?.conversationId != self.selected { self.select("") }
                            self.refresh()
                        }
                    } catch { self.error = "INVALID_MESSAGE_SNAPSHOT" }
                }
            }
        signalListener = root.collection("signals").addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
            Task { @MainActor in
                guard let self, self.active, self.lifecycle == life, self.auth.currentUser?.uid == self.uid else { return }
                if let error { self.listenerFailed(error); return }
                if snapshot?.metadata.isFromCache == false { self.botsDirty = true; self.missingStates = [:]; self.refresh() }
            }
        }
    }
    private func listenerFailed(_ error: Error) {
        let code = (error as NSError).code
        if code == FirestoreErrorCode.permissionDenied.rawValue || code == FirestoreErrorCode.unauthenticated.rawValue { invalidate() }
        else { offline = true; self.error = "MESSAGE_LISTENER_FAILED" }
    }
    private func updateOffline() {
        offline = directoryCached || (!selected.isEmpty && messagesCached)
    }
    private func decode<T: Decodable>(_ data: [String: Any]) throws -> T {
        var value = data
        for key in ["createdAt", "editedAt"] { if let time = value[key] as? Timestamp { value[key] = ISO8601DateFormatter().string(from: time.dateValue()) } }
        guard JSONSerialization.isValidJSONObject(value) else { throw TeamMessageFailure(code: "INVALID_MESSAGE_SNAPSHOT") }
        return try JSONDecoder().decode(T.self, from: JSONSerialization.data(withJSONObject: value))
    }
    func stop() {
        active = false; lifecycle += 1; task?.cancel(); retry?.cancel(); deadline?.cancel(); deadline = nil; task = nil; retry = nil
        listener?.remove(); messageListener?.remove(); memberListener?.remove(); signalListener?.remove()
        listener = nil; messageListener = nil; memberListener = nil; signalListener = nil
        readListeners.values.forEach { $0.remove() }; readListeners = [:]; membershipReady = false
        if let authListener { auth.removeStateDidChangeListener(authListener) }; authListener = nil
        running = false; again = false; bots = []; botStates = [:]; terminalStates = [:]; missingStates = [:]
    }
    private func invalidate() {
        revoked = true; selected = ""; error = "MEMBERSHIP_REQUIRED"
        cache = nil
        let owner = uid, team = teamId, epoch = generation
        stop()
        Task { _ = try? await TeamCommandFiles.shared.update(uid: owner, teamId: team, generation: epoch) { $0.conversations = []; $0.threads = [:]; $0.outbox = []; $0.creation = nil; $0.botOutbox = nil } }
    }
    private func commit(_ reduce: @Sendable (inout TeamMessageProjection) throws -> Void) async throws {
        let life = lifecycle
        guard active else { throw CancellationError() }
        var value = try await TeamCommandFiles.shared.update(uid: uid, teamId: teamId, generation: generation, reduce)
        guard active, life == lifecycle, auth.currentUser?.uid == uid else { throw CancellationError() }
        value.conversations = cache?.conversations ?? []; value.threads = cache?.threads ?? [:]; cache = value
    }
    private func reload() async throws {
        let life = lifecycle
        guard active else { throw CancellationError() }
        var value = try await TeamCommandFiles.shared.load(uid: uid, teamId: teamId, generation: generation)
        guard active, life == lifecycle, auth.currentUser?.uid == uid else { throw CancellationError() }
        value.conversations = cache?.conversations ?? []; value.threads = cache?.threads ?? [:]; cache = value
    }
    func select(_ id: String) {
        let retainedThreads = cache?.threads.filter { $0.key == id } ?? [:]
        selected = id; messagesCached = true; updateOffline(); cache?.threads = retainedThreads; botStates = [:]; botStateWindow = 100; messageListener?.remove(); messageListener = nil
        guard !id.isEmpty, cache?.conversations.contains(where: { $0.id == id }) == true else { refresh(); return }
        let life = lifecycle
        var firstSnapshot = true
        messageListener = db.collection("teams").document(teamId).collection("conversations").document(id).collection("messages").order(by: "seq")
            .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                Task { @MainActor in
                    guard let self, self.active, self.lifecycle == life, self.selected == id, self.auth.currentUser?.uid == self.uid else { return }
                    if let error { self.listenerFailed(error); return }
                    guard let snapshot else { return }
                    do {
                        let reset = firstSnapshot; firstSnapshot = false
                        if reset || !snapshot.documentChanges.isEmpty {
                            var messages: [String: TeamMessage] = reset ? [:] : Dictionary(uniqueKeysWithValues: (self.cache?.threads[id]?.messages ?? []).map { ($0.id, $0) })
                            for change in snapshot.documentChanges {
                                let key = change.document.documentID
                                if change.type == .removed { messages.removeValue(forKey: key) }
                                else {
                                    var data = change.document.data().filter { Self.messageFields.contains($0.key) }; data["id"] = key
                                    let message: TeamMessage = try self.decode(data); messages[key] = message
                                }
                            }
                            self.cache?.threads[id] = TeamMessageProjection.Thread(messages: messages.values.sorted { $0.seq < $1.seq })
                        }
                        self.missingStates = [:]
                        self.messagesCached = snapshot.metadata.isFromCache; self.updateOffline()
                        if !snapshot.metadata.isFromCache { self.error = ""; self.refresh() }
                    } catch { self.error = "INVALID_MESSAGE_SNAPSHOT" }
                }
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
            defer { if self.lifecycle == life { self.running = false; self.scheduleRetry(); self.scheduleDeadline(); if self.again { self.again = false; self.refresh() } } }
            do {
                try await reload()
                let creationLease = Date().timeIntervalSince1970 * 1000 + 30000
                if cache?.creation != nil { try await commit { value in if value.creation?.error == nil, (value.creation?.leaseUntil ?? 0) <= Date().timeIntervalSince1970 * 1000 { value.creation?.leaseUntil = creationLease; value.creation?.attempted = true } } }
                if let creation = cache?.creation, creation.leaseUntil == creationLease {
                    do {
                        let body = try JSONSerialization.jsonObject(with: JSONEncoder().encode(creation)) as! [String: Any]
                        let _: TeamMessageAck = try await call(body.filter { $0.key != "error" && $0.key != "leaseUntil" && $0.key != "attempted" }, life: life)
                        try await commit { $0.creation = nil }
                        if selected.isEmpty { select(creation.conversationId) }
                    } catch {
                        let code = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
                        if ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED"].contains(code) { throw error }
                        let permanent = ["CONVERSATION_REJECTED", "PARTICIPANT_REQUIRED", "BOT_USE_REQUIRED", "INVALID_INPUT"].contains(code)
                        try await commit { value in value.creation?.leaseUntil = nil; if permanent { value.creation?.error = code } }
                        if !permanent { throw error }
                    }
                }
                try await reload()
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
                if botsDirty {
                    botsDirty = false
                    do { let cards: TeamBotList = try await call(["action": "bots"], life: life); bots = cards.bots }
                    catch { botsDirty = true; throw error }
                }
                let id = selected
                if !id.isEmpty { try await observeStates(conversationId: id, life: life) }
                error = ""; retryDelay = 1
            } catch {
                guard active, life == lifecycle else { return }
                again = false; offline = true; bots = []; botStates = [:]; self.error = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
                if ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED"].contains(self.error) { invalidate() }
            }
        }
    }
    private func observeStates(conversationId id: String, life: Int) async throws {
        guard let conversation = cache?.conversations.first(where: { $0.id == id }) else { return }
        var fingerprints: [String: String] = [:]
        for message in (cache?.threads[id]?.messages ?? []).suffix(botStateWindow) {
            for trigger in message.botTriggers(in: conversation) { fingerprints[trigger] = "\(message.id)/\(message.changeSeq)/\(conversation.participants.joined(separator: ","))" }
        }
        var states: [String: TeamBotState] = [:], missing: [String] = []
        for trigger in fingerprints.keys {
            let key = "\(id)/\(trigger)"
            if let known = terminalStates[key] { states[trigger] = known }
            else if missingStates[key] != fingerprints[trigger] { missing.append(trigger) }
        }
        let batches = stride(from: 0, to: missing.count, by: 30).map { Array(missing[$0..<min($0 + 30, missing.count)]) }
        let result = try await withThrowingTaskGroup(of: TeamBotStates.self) { group in
            var index = 0
            func enqueue() {
                let batch = batches[index]; index += 1
                group.addTask { @MainActor [self] in try await call(["action": "botStates", "conversationId": id, "triggerIds": batch], life: life) }
            }
            while index < min(4, batches.count) { enqueue() }
            var result: [TeamBotStates] = []
            while let value = try await group.next() { result.append(value); if index < batches.count { enqueue() } }
            return result
        }
        guard active, life == lifecycle, auth.currentUser?.uid == uid else { throw CancellationError() }
        let terminal: Set<String> = ["completed", "failed", "missed", "cancelled", "recovered", "revoked"]
        for batch in result {
            for (trigger, state) in batch.states {
                let key = "\(id)/\(trigger)"
                if let state {
                    states[trigger] = state
                    if terminal.contains(state.state) { terminalStates[key] = state }
                } else { missingStates[key] = fingerprints[trigger] }
            }
        }
        while terminalStates.count > 512, let key = terminalStates.keys.first { terminalStates.removeValue(forKey: key) }
        while missingStates.count > 512, let key = missingStates.keys.first { missingStates.removeValue(forKey: key) }
        if selected == id { botStates = states }
    }
    func create(kind: String, title: String, uids: [String], botKeys: [String] = [], id: String) async throws {
        guard active, !revoked else { return }
        guard TeamBotSelection.unavailable(Set(botKeys), cards: bots).isEmpty else { throw TeamMessageFailure(code: "BOT_USE_REQUIRED") }
        let intent = TeamMessageCreation(teamId: teamId, generation: generation, conversationId: id, kind: kind, title: title, participantUids: Array(Set([uid] + uids)).sorted(), botKeys: botKeys.isEmpty ? nil : botKeys.sorted())
        try await commit { value in guard value.creation == nil else { throw TeamMessageFailure(code: "CREATION_PENDING") }; value.creation = intent }
        select(id)
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
        bots.removeAll { $0.key == bot.key }; botsDirty = true; refresh()
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
        if let index = cache?.conversations.firstIndex(where: { $0.id == id }) {
            let lastReadSeq = max(cache?.conversations[index].lastReadSeq ?? 0, value.lastReadSeq ?? 0)
            cache?.conversations[index].lastReadSeq = lastReadSeq
        }
    }
    private func scheduleDeadline() {
        deadline?.cancel(); deadline = nil
        let now = Date().timeIntervalSince1970 * 1000
        let expirations = bots.filter { $0.online }.compactMap(\.onlineUntil) + botStates.values.filter { ["claimed", "running"].contains($0.state) }.compactMap(\.leaseUntil)
        guard active, let next = expirations.filter({ $0 > now }).min() else { return }
        deadline = Task { [weak self] in do { try await Task.sleep(for: .seconds((next - now) / 1000)); self?.botsDirty = true; self?.refresh() } catch {} }
    }
    private func scheduleRetry() {
        retry?.cancel(); retry = nil
        guard active, let cache else { return }
        var leases = cache.outbox.filter { $0.error == nil }.map { $0.leaseUntil ?? 0 }
        leases.append(contentsOf: (cache.botOutbox ?? []).filter { $0.error == nil }.map { $0.leaseUntil ?? 0 })
        if let creation = cache.creation, creation.error == nil { leases.append(creation.leaseUntil ?? 0) }
        guard let earliest = leases.min() else { return }
        let delay = max(retryDelay, (earliest - Date().timeIntervalSince1970 * 1000) / 1000)
        retryDelay = min(30, retryDelay * 2)
        retry = Task { [weak self] in do { try await Task.sleep(for: .seconds(delay)); self?.refresh() } catch {} }
    }
    private func call<T: Decodable>(_ body: [String: Any], life: Int) async throws -> T {
        guard active, life == lifecycle, let user = auth.currentUser, user.uid == uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        let token = try await user.getIDToken()
        guard active, life == lifecycle, auth.currentUser?.uid == uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        var body = body; body["teamId"] = teamId; body["generation"] = generation
        var request = URLRequest(url: apiRoot)
        request.httpMethod = "POST"; request.timeoutInterval = 20; request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization"); request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await session.data(for: request)
        guard active, life == lifecycle, auth.currentUser?.uid == uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        guard let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) else {
            let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            let code = (object?["error"] as? [String: Any])?["code"] as? String ?? "SERVICE_UNAVAILABLE"
            if ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED"].contains(code) { invalidate() }
            throw TeamMessageFailure(code: code)
        }
        return try JSONDecoder().decode(T.self, from: data)
    }
}
