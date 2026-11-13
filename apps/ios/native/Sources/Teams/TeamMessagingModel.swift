import FirebaseAuth
import FirebaseFirestore
import Foundation
import Observation

private struct TeamBotList: Decodable { let bots: [TeamBotCard] }
private struct TeamBotAck: Decodable {}
private struct TeamTaskList: Decodable, Sendable { let tasks: [TeamTask]; let next: String? }
private struct TeamTaskCancellation: Decodable, Sendable { let taskId: String; let state: String }
private struct TeamMessageAck: Decodable, Sendable { let lastReadSeq: Int?; let conversationId: String? }
final class TeamMessageRedirectPolicy: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}

@MainActor @Observable
final class TeamMessagingModel {
    static let conversationFields: Set<String> = ["kind", "title", "participants", "lastSeq", "lastChangeSeq", "lastMessage", "sharedBotId"]
    private static let messageFields: Set<String> = ["seq", "author", "kind", "text", "replyTo", "createdAt", "editedAt", "recalled", "changeSeq", "changeId", "mentions", "targets", "taskId", "taskIds", "artifactId", "waitId", "approval", "decision"]
    private(set) var cache: TeamMessageProjection?
    private(set) var offline = true
    private(set) var error = ""
    private(set) var revoked = false
    private(set) var bots: [TeamBotCard] = []
    /// The Bot list has been fetched at least once for this team.
    private(set) var botsLoaded = false
    /// The last fetch of the Bot list failed, so Bot availability is unknown.
    private(set) var botsUnverified = false
    private(set) var tasks: [TeamTask] = []
    private(set) var artifact: TeamArtifact?
    var selected = ""
    /// Called once when access to this team is lost (revoked membership, rejected session): the owner of the
    /// model re-reads the team list, which is the authority on what is left.
    @ObservationIgnored var onRevoked: (() -> Void)?
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
    /// The first directory snapshot of this run has arrived.
    private(set) var directoryReady = false
    /// Conversations whose first `reads/{uid}` snapshot has not arrived yet. Their `lastReadSeq` is not known, so
    /// nothing is unread, no "new messages" divider is placed and nothing is marked read until it is.
    private(set) var readsPending: Set<String> = []
    /// Bumped whenever the listeners are torn down or re-created, so a callback already queued by a removed
    /// listener can tell it is stale.
    @ObservationIgnored private var watchGeneration = 0
    private var botsDirty = true
    /// The listeners were stopped by a failure; a good refresh must not hide the error until `reconnect()`.
    @ObservationIgnored private var listenersFailed = false
    @ObservationIgnored private var authListener: AuthStateDidChangeListenerHandle?
    private var uid = ""
    private var teamId = ""
    private var generation = 0
    private var active = false
    private var artifactNeedsValidation = false
    private let auth: Auth
    private let db: Firestore
    private let apiRoot: URL
    private let session = URLSession(configuration: .ephemeral, delegate: TeamMessageRedirectPolicy(), delegateQueue: nil)

    init(apiRoot: URL = URL(string: "https://lingxios.net/v1/teams/messages")!, auth: Auth = Auth.auth(), db: Firestore = Firestore.firestore()) {
        self.apiRoot = apiRoot; self.auth = auth; self.db = db
    }

    func start(uid: String, team: TeamSummary) {
        stop(); active = true; lifecycle += 1
        self.uid = uid; teamId = team.id; generation = team.generation; revoked = false; offline = true; error = ""; listenersFailed = false
        cache = nil; selected = ""; directoryCached = true; messagesCached = true; botsDirty = true; bots = []; botsLoaded = false; botsUnverified = false; tasks = []; artifact = nil
        directoryReady = false; readsPending = []
        let life = lifecycle
        authListener = auth.addStateDidChangeListener { [weak self] _, user in
            let current = user?.uid
            Task { @MainActor in guard let self, self.active, self.lifecycle == life else { return }; if current != uid { self.invalidate() } }
        }
        cache = TeamMessageProjection(version: 1, uid: uid, teamId: team.id, generation: team.generation)
        watchMembers(life: life)
        task = Task { [weak self] in
            guard let self else { return }
            do { try await self.reload(); self.refresh() } catch { self.error = "STORAGE_FAILED" }
        }
    }
    /// The membership check: the viewer's `members` document must carry this team generation before the
    /// directory is read, and a different one afterwards revokes the team.
    private func watchMembers(life: Int) {
        watchGeneration += 1
        let watch = watchGeneration, uid = self.uid
        memberListener = db.collection("teams").document(teamId).collection("members").document(uid).addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
            Task { @MainActor in
                guard let self, self.active, self.lifecycle == life, self.watchGeneration == watch, self.auth.currentUser?.uid == self.uid else { return }
                if let error { self.listenerFailed(error); return }
                if (snapshot?.data()?["generation"] as? NSNumber)?.intValue == self.generation {
                    if !self.membershipReady { self.membershipReady = true; self.listenDirectory(life: life) }
                } else if self.membershipReady || snapshot?.metadata.isFromCache == false { self.invalidate() }
            }
        }
    }
    private func listenDirectory(life: Int) {
        let uid = self.uid, root = db.collection("teams").document(teamId), watch = watchGeneration
        listener = root.collection("conversations").whereField("memberUids", arrayContains: uid)
            .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                Task { @MainActor in
                    guard let self, self.active, self.lifecycle == life, self.watchGeneration == watch, self.auth.currentUser?.uid == self.uid else { return }
                    if let error { self.listenerFailed(error); return }
                    guard let snapshot else { return }
                    let previous = Dictionary(uniqueKeysWithValues: (self.cache?.conversations ?? []).map { ($0.id, $0) })
                    let conversations: [TeamMessageConversation]
                    do {
                        conversations = try snapshot.documents.map { doc in
                            let data = doc.data().filter { Self.conversationFields.contains($0.key) }
                            return try TeamsPointerParser.parseConversation(id: doc.documentID, data: data, lastReadSeq: previous[doc.documentID]?.lastReadSeq ?? 0, millis: Self.milliseconds)
                        }
                    } catch {
                        // A conversation that breaks the stored contract is never shown half-parsed: the team's
                        // listeners stop with INVALID_DATA, as on Desktop.
                        self.invalidData()
                        return
                    }
                    self.cache?.conversations = conversations
                    if self.messageListener == nil, !self.selected.isEmpty, conversations.contains(where: { $0.id == self.selected }) { self.select(self.selected) }
                    let ids = Set(conversations.map(\.id))
                    let retainedThreads = self.cache?.threads.filter { ids.contains($0.key) } ?? [:]
                    self.cache?.threads = retainedThreads
                    for id in Array(self.readListeners.keys) where !ids.contains(id) { self.readListeners.removeValue(forKey: id)?.remove() }
                    self.readsPending.formIntersection(ids)
                    for id in ids.sorted() where self.readListeners[id] == nil {
                        self.readsPending.insert(id)
                        self.readListeners[id] = root.collection("conversations").document(id).collection("reads").document(uid).addSnapshotListener(includeMetadataChanges: true) { [weak self] mark, error in
                            Task { @MainActor in
                                guard let self, self.active, self.lifecycle == life, self.watchGeneration == watch, self.auth.currentUser?.uid == self.uid else { return }
                                self.readsPending.remove(id)
                                if let error { self.listenerFailed(error); return }
                                let data = mark?.data()
                                let seq = (data?["generation"] as? NSNumber)?.intValue == self.generation ? (data?["lastReadSeq"] as? NSNumber)?.intValue ?? 0 : 0
                                // A read mark only ever moves forward (Desktop: `Math.max`): an older cached snapshot after an
                                // acknowledged `read` must not make messages unread again.
                                if let index = self.cache?.conversations.firstIndex(where: { $0.id == id }), let current = self.cache?.conversations[index].lastReadSeq, seq > current { self.cache?.conversations[index].lastReadSeq = seq }
                            }
                        }
                    }
                    self.directoryReady = true
                    self.directoryCached = snapshot.metadata.isFromCache; self.updateOffline()
                    if !snapshot.metadata.isFromCache {
                        if !self.selected.isEmpty, !ids.contains(self.selected), self.cache?.creation?.conversationId != self.selected { self.select("") }
                        self.refresh()
                    }
                }
            }
        signalListener = root.collection("signals").addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
            Task { @MainActor in
                guard let self, self.active, self.lifecycle == life, self.watchGeneration == watch, self.auth.currentUser?.uid == self.uid else { return }
                if let error { self.listenerFailed(error); return }
                if snapshot?.metadata.isFromCache == false { self.artifactNeedsValidation = true; self.botsDirty = true; self.refresh() }
            }
        }
    }
    /// Epoch milliseconds of a Firestore `Timestamp`; `nil` for anything else (the pointer parser's `millis`).
    nonisolated static func milliseconds(_ value: Any) -> Int64? {
        guard let stamp = value as? Timestamp else { return nil }
        return stamp.seconds * 1000 + Int64(stamp.nanoseconds / 1_000_000)
    }
    private func listenerFailed(_ error: Error) {
        let code = (error as NSError).code
        if code == FirestoreErrorCode.permissionDenied.rawValue || code == FirestoreErrorCode.unauthenticated.rawValue { invalidate() }
        else { stopListening("MESSAGE_LISTENER_FAILED") }
    }
    /// A stored conversation document broke the contract (`TeamsPointerParser`): stop and say so.
    private func invalidData() { stopListening(TeamsPointerParser.invalidDataCode) }
    /// Every listener of this team stops (as Desktop's watcher does); the saved view stays and
    /// `reconnect()` opens them again.
    private func stopListening(_ code: String) {
        removeListeners(); listenersFailed = true; offline = true; error = code
    }
    private func removeListeners() {
        watchGeneration += 1
        listener?.remove(); messageListener?.remove(); memberListener?.remove(); signalListener?.remove()
        listener = nil; messageListener = nil; memberListener = nil; signalListener = nil
        readListeners.values.forEach { $0.remove() }; readListeners = [:]; membershipReady = false
    }
    /// Opens the listeners again after they stopped (invalid data, a failed listener, a lost connection) and
    /// retries what is pending.
    func reconnect() {
        guard active, !revoked else { return }
        error = ""; listenersFailed = false; botsDirty = true
        removeListeners()
        directoryReady = false; readsPending = []
        directoryCached = true; messagesCached = true; updateOffline()
        watchMembers(life: lifecycle)
        refresh()
    }
    private func updateOffline() {
        offline = directoryCached || (!selected.isEmpty && messagesCached)
    }
    /// Message times reach the views as ISO-8601 with milliseconds (`TeamsStreamLogic.parseInstant` reads them back).
    private static let isoFormatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()
    private func decode<T: Decodable>(_ data: [String: Any]) throws -> T {
        var value = data
        for key in ["createdAt", "editedAt"] { if let time = value[key] as? Timestamp { value[key] = Self.isoFormatter.string(from: time.dateValue()) } }
        guard JSONSerialization.isValidJSONObject(value) else { throw TeamMessageFailure(code: "INVALID_MESSAGE_SNAPSHOT") }
        return try JSONDecoder().decode(T.self, from: JSONSerialization.data(withJSONObject: value))
    }
    func stop() {
        active = false; lifecycle += 1; task?.cancel(); retry?.cancel(); deadline?.cancel(); deadline = nil; task = nil; retry = nil
        removeListeners()
        if let authListener { auth.removeStateDidChangeListener(authListener) }; authListener = nil
        running = false; again = false; bots = []; tasks = []; artifact = nil
    }
    private func invalidate() {
        guard !revoked else { return }
        revoked = true; selected = ""; error = "MEMBERSHIP_REQUIRED"
        cache = nil
        let owner = uid, team = teamId, epoch = generation
        stop()
        onRevoked?()
        Task { _ = try? await TeamCommandFiles.shared.update(uid: owner, teamId: team, generation: epoch) { $0.conversations = []; $0.threads = [:]; $0.outbox = []; $0.creation = nil; $0.commands = nil } }
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
        selected = id; messagesCached = true; updateOffline(); cache?.threads = retainedThreads; tasks = []; artifact = nil; messageListener?.remove(); messageListener = nil
        guard !id.isEmpty, cache?.conversations.contains(where: { $0.id == id }) == true else { refresh(); return }
        let life = lifecycle, watch = watchGeneration
        var firstSnapshot = true
        messageListener = db.collection("teams").document(teamId).collection("conversations").document(id).collection("messages").order(by: "seq")
            .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                Task { @MainActor in
                    guard let self, self.active, self.lifecycle == life, self.watchGeneration == watch, self.selected == id, self.auth.currentUser?.uid == self.uid else { return }
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
                            self.messagesCached = snapshot.metadata.isFromCache; self.updateOffline()
                        if !snapshot.metadata.isFromCache { self.error = ""; self.refresh() }
                    } catch { self.error = "INVALID_MESSAGE_SNAPSHOT" }
                }
            }
        refresh()
    }
    func refreshTasks() { refresh() }
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
                        let _: TeamMessageAck = try await call(creation.body, life: life)
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
                        let _: TeamMessageAck = try await call(intent.body, life: life)
                        try await commit { $0.outbox.removeAll { $0.id == intent.id } }
                    } catch {
                        let code = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
                        let permanent = ["MESSAGE_ID_CONFLICT", "MESSAGE_RECALLED", "MESSAGE_AUTHOR_REQUIRED", "REPLY_REQUIRED", "REQUEST_QUOTA_EXCEEDED", "CONVERSATION_POLICY_REQUIRED", "INPUT_GRANT_REQUIRED", "COMMAND_ID_CONFLICT", "BOT_USE_REQUIRED", "BOT_PARTICIPANT_REQUIRED", "INVALID_BOT_TARGET", "INVALID_INPUT", "WAIT_REQUIRED", "WAIT_RECIPIENT_REQUIRED", "WAIT_CLOSED", "TASK_REVOKED"].contains(code)
                        try await commit { value in if let index = value.outbox.firstIndex(where: { $0.id == intent.id }) { value.outbox[index].leaseUntil = nil; if permanent { value.outbox[index].error = code } } }
                        if !permanent { throw error }
                    }
                }
                for pending in cache?.commands ?? [] where pending.error == nil {
                    let intentID = pending.id, lease = Date().timeIntervalSince1970 * 1000 + 30000
                    try await commit { value in
                        if let index = value.commands?.firstIndex(where: { $0.id == intentID }), (value.commands?[index].leaseUntil ?? 0) <= Date().timeIntervalSince1970 * 1000 {
                            value.commands?[index].leaseUntil = lease; value.commands?[index].attempted = true
                        }
                    }
                    guard let intent = cache?.commands?.first(where: { $0.id == intentID && $0.leaseUntil == lease }) else { continue }
                    do {
                        let _: TeamBotAck = try await execute(intent.action, payload: intent.payload, commandId: intent.commandId, life: life)
                        try await commit { $0.commands?.removeAll { $0.id == intentID } }
                        botsDirty = true
                    } catch {
                        let code = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
                        let permanent = ["INVALID_INPUT", "COMMAND_ID_CONFLICT", "BOT_REVISION_CONFLICT", "MAINTAINER_REQUIRED"].contains(code)
                        try await commit { value in if let index = value.commands?.firstIndex(where: { $0.id == intentID }) {
                            value.commands?[index].leaseUntil = nil; if permanent { value.commands?[index].error = code }
                        } }
                        if !permanent { throw error }
                    }
                }
                if botsDirty {
                    botsDirty = false
                    do { let cards: TeamBotList = try await execute("listSharedBots", payload: [:], life: life); bots = cards.bots; botsLoaded = true; botsUnverified = false }
                    catch { botsDirty = true; botsUnverified = true; throw error }
                }
                if artifactNeedsValidation {
                    artifactNeedsValidation = false
                    if let current = artifact {
                        do { let _: TeamArtifact = try await execute("readArtifact", payload: ["artifactId": current.artifactId], life: life) }
                        catch is CancellationError { throw CancellationError() }
                        catch { artifact = nil }
                    }
                }
                let id = selected
                try await observeTasks(conversationId: cache?.conversations.contains(where: { $0.id == id }) == true ? id : nil, life: life)
                if !listenersFailed { error = cache?.commands?.first(where: { $0.error != nil })?.error ?? "" }
                retryDelay = 1
            } catch {
                guard active, life == lifecycle else { return }
                // Without the card list Bot availability is unknown, not "unavailable".
                again = false; offline = true; bots = []; botsUnverified = true; tasks = []; artifact = nil; self.error = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
                if ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED"].contains(self.error) { invalidate() }
            }
        }
    }
    private func observeTasks(conversationId: String?, life: Int) async throws {
        let payload: [String: Any] = conversationId.map { ["conversationId": $0] } ?? [:]
        let selection = selected
        let result: TeamTaskList = try await execute("listTasks", payload: payload, life: life)
        if selected == selection { tasks = result.tasks }
    }
    func refreshTask(_ taskId: String) async throws {
        let selection = selected
        let result: TeamTask = try await execute("getTask", payload: ["taskId": taskId], life: lifecycle)
        guard selected == selection else { return }
        if let index = tasks.firstIndex(where: { $0.taskId == taskId }) { tasks[index] = result } else { tasks.append(result) }
    }
    /// Stops a task: at once while it is queued or waiting, at the worker's next check while the Bot is working. A task that has already ended stays as it is.
    func cancelTask(_ taskId: String) async throws {
        do { let _: TeamTaskCancellation = try await execute("cancelTask", payload: ["taskId": taskId], life: lifecycle) }
        catch let failure as TeamMessageFailure where failure.code == "TASK_FINISHED" {}
        try await refreshTask(taskId)
    }
    /// Approves or declines the list change a task waits on. `ids` are made once per decision by the caller: sending them again after a lost
    /// answer repeats the same command, so the reply is never posted twice.
    func decideWait(_ waitId: String, taskId: String, approve: Bool, ids: TeamDecisionIds) async throws {
        guard !selected.isEmpty else { throw TeamMessageFailure(code: "INVALID_INPUT") }
        let _: TeamBotAck = try await execute("decideWait", payload: ["conversationId": selected, "waitId": waitId, "messageId": ids.messageId, "changeId": ids.changeId,
                                                                                 "decision": approve ? "approve" : "deny"], commandId: ids.commandId, life: lifecycle)
        try await refreshTask(taskId)
    }
    /// What a task did outside the Chat. Only its requester and the Bot's maintainer may ask.
    func listOperations(_ taskId: String) async throws -> [TeamTaskOperation] {
        let result: TeamTaskOperations = try await execute("listOperations", payload: ["taskId": taskId], life: lifecycle)
        return result.operations
    }
    /// The maintainer states what happened to an action nobody could verify. The task is read again either way: a refusal often means it has moved.
    func resolveOperation(_ taskId: String, operationId: String, happened: Bool) async throws {
        do { let _: TeamOperationResolved = try await execute("resolveOperation", payload: ["taskId": taskId, "operationId": operationId, "resolution": happened ? "succeeded" : "failed"], life: lifecycle) }
        catch { try? await refreshTask(taskId); throw error }
        try await refreshTask(taskId)
    }
    func openArtifact(_ artifactId: String) async throws {
        let selection = selected
        let result: TeamArtifact = try await execute("readArtifact", payload: ["artifactId": artifactId], life: lifecycle)
        if selected == selection { artifact = result }
    }
    func closeArtifact() { artifact = nil }
    /// One revision of a shared document: the latest, or the one named.
    func readArtifact(_ artifactId: String, revision: Int? = nil) async throws -> TeamArtifact {
        var payload: [String: Any] = ["artifactId": artifactId]
        if let revision { payload["revision"] = revision }
        return try await execute("readArtifact", payload: payload, life: lifecycle)
    }
    func listArtifactRevisions(_ artifactId: String) async throws -> TeamArtifactRevisions {
        try await execute("listArtifactRevisions", payload: ["artifactId": artifactId], life: lifecycle)
    }
    /// Saves `draft` as the revision after `expectedRevision`, or saves nothing when someone saved first (`ARTIFACT_REVISION_CONFLICT`).
    func reviseArtifact(_ artifactId: String, expectedRevision: Int, operationId: String, draft: TeamDocumentDraft) async throws {
        let _: TeamArtifactRevised = try await execute("reviseArtifact", payload: ["artifactId": artifactId, "expectedRevision": expectedRevision,
                                                                                  "operationId": operationId, "document": draft.payload], life: lifecycle)
    }
    /// A Bot's shared list as it is now. It is read on demand: the list is not part of the message stream.
    func readList(_ sharedBotId: String) async throws -> TeamSharedList {
        try await execute("readList", payload: ["sharedBotId": sharedBotId], life: lifecycle)
    }
    /// Saves `changes` on top of `expectedDataRevision`, or saves nothing. Sending the same `operationId` again repeats the same
    /// request, so a save whose answer was lost is not applied twice.
    func writeList(_ sharedBotId: String, operationId: String, expectedDataRevision: Int, changes: [TeamSharedListChange]) async throws {
        let _: TeamSharedListWritten = try await execute("writeList", payload: ["sharedBotId": sharedBotId, "operationId": operationId,
                                                                             "expectedDataRevision": expectedDataRevision, "changes": changes.map(\.payload)], life: lifecycle)
    }
    func create(kind: String, title: String, uids: [String], sharedBotIds: [String] = [], id: String) async throws {
        guard active, !revoked else { return }
        guard TeamBotSelection.unavailable(Set(sharedBotIds), cards: bots).isEmpty else { throw TeamMessageFailure(code: "BOT_USE_REQUIRED") }
        let intent = TeamMessageCreation(teamId: teamId, generation: generation, conversationId: id, kind: kind, title: title, participantUids: Array(Set([uid] + uids)).sorted(), sharedBotIds: sharedBotIds.isEmpty ? nil : sharedBotIds.sorted())
        try await commit { value in guard value.creation == nil else { throw TeamMessageFailure(code: "CREATION_PENDING") }; value.creation = intent }
        select(id)
    }
    /// `waitId` makes a new message the answer to a Bot's question: it asks no Bot and starts no task.
    func queue(operation: String, text: String, messageId: String? = nil, replyTo: String? = nil, targets: [String] = [], waitId: String? = nil) async throws {
        guard active, !revoked, !selected.isEmpty, operation == "recall" || (!text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && text.utf8.count <= 8192) else { throw TeamMessageFailure(code: "INVALID_INPUT") }
        if let waitId {
            guard operation == "create" else { throw TeamMessageFailure(code: "INVALID_INPUT") }
            let intent = TeamMessageIntent(teamId: teamId, generation: generation, conversationId: selected, messageId: messageId ?? UUID().uuidString, changeId: UUID().uuidString, operation: operation, text: text, replyTo: replyTo, waitId: waitId)
            try await commit { $0.outbox.append(intent) }; refresh(); return
        }
        if operation == "create" {
            guard let conversation = cache?.conversations.first(where: { $0.id == selected }), TeamBotSelection.canSend(in: conversation, selected: Set(targets), cards: bots) else { throw TeamMessageFailure(code: "BOT_USE_REQUIRED") }
        }
        let conversation = cache?.conversations.first { $0.id == selected }
        let resolvedTargets = operation == "create" ? conversation.map { Array(TeamBotSelection.targets(in: $0, selected: Set(targets))).sorted() } : nil
        let intent = TeamMessageIntent(teamId: teamId, generation: generation, conversationId: selected, messageId: messageId ?? UUID().uuidString, changeId: UUID().uuidString, operation: operation, text: text, replyTo: replyTo, targets: resolvedTargets ?? nil)
        try await commit { $0.outbox.append(intent) }; refresh()
    }
    func pause(_ bot: TeamBotCard) async throws {
        guard active, !revoked, bot.maintainer == TeamMemberRef(uid: uid, generation: generation), bot.state == "published" else { throw TeamMessageFailure(code: "MAINTAINER_REQUIRED") }
        let intent = TeamExecutionIntent(commandId: UUID().uuidString, action: "pauseSharedBot", sharedBotId: bot.sharedBotId, expectedRevision: bot.revision)
        try await commit { if $0.commands == nil { $0.commands = [] }; $0.commands?.append(intent) }
        refresh()
    }
    func discardCommand(_ commandId: String) async throws {
        try await commit { value in
            guard let intent = value.commands?.first(where: { $0.id == commandId }), intent.error != nil,
                  (intent.leaseUntil ?? 0) <= Date().timeIntervalSince1970 * 1000 else { throw TeamMessageFailure(code: "MESSAGE_IN_FLIGHT") }
            value.commands?.removeAll { $0.id == commandId }
        }
        refresh()
    }
    func discard(_ id: String) async throws { try await commit { value in
        if value.outbox.contains(where: { $0.id == id && (($0.attempted == true && $0.error == nil) || ($0.leaseUntil ?? 0) > Date().timeIntervalSince1970 * 1000) }) { throw TeamMessageFailure(code: "MESSAGE_IN_FLIGHT") }
        value.outbox.removeAll { $0.id == id }
    } }
    func discardCreation() async throws { try await commit { value in
        if (value.creation?.attempted == true && value.creation?.error == nil) || (value.creation?.leaseUntil ?? 0) > Date().timeIntervalSince1970 * 1000 { throw TeamMessageFailure(code: "MESSAGE_IN_FLIGHT") }; value.creation = nil
    } }
    /// Marks the open conversation read up to its newest loaded message (the header action).
    func markRead() async throws {
        let id = selected
        try await markRead(conversationId: id, lastReadSeq: cache?.threads[id]?.messages.map(\.seq).max() ?? 0)
    }
    /// Sends the `read` command for any conversation of the directory. Nothing happens offline or for a
    /// watermark of 0. The listener of `reads/{uid}` and this acknowledgement both only ever raise `lastReadSeq`.
    func markRead(conversationId id: String, lastReadSeq seq: Int) async throws {
        guard active, !revoked, !offline, !id.isEmpty, seq > 0 else { return }
        let value: TeamMessageAck = try await call(["action": "read", "conversationId": id, "lastReadSeq": seq], life: lifecycle)
        if let index = cache?.conversations.firstIndex(where: { $0.id == id }) {
            let lastReadSeq = max(cache?.conversations[index].lastReadSeq ?? 0, value.lastReadSeq ?? 0)
            cache?.conversations[index].lastReadSeq = lastReadSeq
        }
    }
    /// Unread messages of one conversation; 0 until its read mark is known (a mark that has not arrived yet must
    /// not make the whole conversation look unread).
    func unread(of conversation: TeamMessageConversation) -> Int {
        directoryReady && !readsPending.contains(conversation.id) ? TeamsListLogic.unreadCount(conversation) : 0
    }
    /// Every listed conversation has delivered its read mark.
    var readsReady: Bool { directoryReady && readsPending.isEmpty }
    /// Sum of the unread messages of every conversation of this team.
    var unreadTotal: Int { (cache?.conversations ?? []).reduce(0) { $0 + unread(of: $1) } }
    /// The listeners deliver the saved copy (directory, or the open conversation) rather than the server's.
    var fromCache: Bool { directoryCached || (!selected.isEmpty && messagesCached) }
    private func scheduleDeadline() {
        deadline?.cancel(); deadline = nil
        let now = Date().timeIntervalSince1970 * 1000
        let expirations = bots.filter { $0.ready }.map(\.onlineUntil) + tasks.filter { !$0.terminal }.map(\.deadline)
        guard active, let next = expirations.filter({ $0 > now }).min() else { return }
        deadline = Task { [weak self] in do { try await Task.sleep(for: .seconds((next - now) / 1000)); self?.botsDirty = true; self?.refresh() } catch {} }
    }
    private func scheduleRetry() {
        retry?.cancel(); retry = nil
        guard active, let cache else { return }
        var leases = cache.outbox.filter { $0.error == nil }.map { $0.leaseUntil ?? 0 }
        leases.append(contentsOf: (cache.commands ?? []).filter { $0.error == nil }.map { $0.leaseUntil ?? 0 })
        if let creation = cache.creation, creation.error == nil { leases.append(creation.leaseUntil ?? 0) }
        if tasks.contains(where: \.busy) { leases.append(Date().timeIntervalSince1970 * 1000 + 3000) }
        guard let earliest = leases.min() else { return }
        let delay = max(retryDelay, (earliest - Date().timeIntervalSince1970 * 1000) / 1000)
        retryDelay = min(30, retryDelay * 2)
        retry = Task { [weak self] in do { try await Task.sleep(for: .seconds(delay)); self?.refresh() } catch {} }
    }
    private func execute<T: Decodable>(_ action: String, payload: [String: Any], commandId: String = UUID().uuidString, life: Int) async throws -> T {
        try await call(["schemaVersion": 1, "commandId": commandId, "action": action, "payload": payload], life: life)
    }
    private func call<T: Decodable>(_ body: [String: Any], life: Int) async throws -> T {
        guard active, life == lifecycle, let user = auth.currentUser, user.uid == uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        let token = try await user.getIDToken()
        guard active, life == lifecycle, auth.currentUser?.uid == uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        var body = body; body["teamId"] = teamId; body["generation"] = generation
        if body["schemaVersion"] != nil { try TeamExecutionContract.validate(body) }
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
