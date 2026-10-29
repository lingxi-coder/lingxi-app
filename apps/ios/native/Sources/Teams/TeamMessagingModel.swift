import FirebaseAuth
import FirebaseFirestore
import Foundation
import Observation

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
    private let session = URLSession(configuration: .ephemeral, delegate: TeamMessageRedirectPolicy(), delegateQueue: nil)

    func start(uid: String, team: TeamSummary) {
        stop(); active = true; lifecycle += 1
        self.uid = uid; teamId = team.id; generation = team.generation; revoked = false; offline = true
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
        running = false; again = false
    }
    private func invalidate() {
        revoked = true; selected = ""; error = "MEMBERSHIP_REQUIRED"
        cache = nil
        let owner = uid, team = teamId, epoch = generation
        stop()
        Task { _ = try? await TeamMessageFiles.shared.update(uid: owner, teamId: team, generation: epoch) { $0.conversations = []; $0.threads = [:]; $0.outbox = []; $0.creation = nil } }
    }
    private func commit(_ reduce: @Sendable (inout TeamMessageProjection) throws -> Void) async throws {
        let life = lifecycle
        guard active else { throw CancellationError() }
        let value = try await TeamMessageFiles.shared.update(uid: uid, teamId: teamId, generation: generation, reduce)
        guard active, life == lifecycle else { throw CancellationError() }; cache = value
    }
    func select(_ id: String) {
        selected = id; readListener?.remove()
        let life = lifecycle
        readListener = Firestore.firestore().collection("teams").document(teamId).collection("conversations").document(id).collection("reads").document(uid)
            .addSnapshotListener { [weak self] snapshot, _ in
                let wake = snapshot?.metadata.isFromCache == false
                Task { @MainActor in guard let self, self.active, self.lifecycle == life else { return }; if wake { self.refresh() } }
            }
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
                        let permanent = ["CONVERSATION_REJECTED", "PARTICIPANT_REQUIRED", "INVALID_INPUT"].contains(code)
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
                        let permanent = ["MESSAGE_ID_CONFLICT", "MESSAGE_RECALLED", "MESSAGE_AUTHOR_REQUIRED", "REPLY_REQUIRED", "INVALID_INPUT"].contains(code)
                        try await commit { value in if let index = value.outbox.firstIndex(where: { $0.id == intent.id }) { value.outbox[index].leaseUntil = nil; if permanent { value.outbox[index].error = code } } }
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
                offline = false; error = ""
            } catch {
                guard active, life == lifecycle else { return }
                offline = true; self.error = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
                if ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED"].contains(self.error) { invalidate() }
            }
        }
    }
    func create(kind: String, title: String, uids: [String], id: String) async throws {
        guard active, !revoked else { return }
        let intent = TeamMessageCreation(teamId: teamId, generation: generation, conversationId: id, kind: kind, title: title, participantUids: [uid] + uids)
        try await commit { value in guard value.creation == nil else { throw TeamMessageFailure(code: "CREATION_PENDING") }; value.creation = intent }
        selected = id; refresh()
    }
    func queue(operation: String, text: String, messageId: String? = nil, replyTo: String? = nil) async throws {
        guard active, !revoked, !selected.isEmpty, operation == "recall" || (!text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && text.utf8.count <= 8192) else { throw TeamMessageFailure(code: "INVALID_INPUT") }
        let intent = TeamMessageIntent(teamId: teamId, generation: generation, conversationId: selected, messageId: messageId ?? UUID().uuidString, changeId: UUID().uuidString, operation: operation, text: text, replyTo: replyTo)
        try await commit { $0.outbox.append(intent) }; refresh()
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
        var request = URLRequest(url: URL(string: "https://lingxios.net/v1/teams/messages")!)
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
