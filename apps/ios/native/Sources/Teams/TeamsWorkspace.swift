import FirebaseAuth
import FirebaseFirestore
import Foundation
import Observation

private struct TeamsList: Decodable { let teams: [TeamSummary] }
private struct TeamCommandResult: Decodable { let teamId: String?; let code: String? }

/// Unsent composer state of one conversation. Kept in memory by the workspace only, as on Desktop: it
/// survives switching conversations and tabs, not a restart (spec §6.3, §11).
struct TeamComposerDraft: Equatable, Sendable {
    var text = ""
    var replyToID: String?
    /// Bot keys chosen with `@`, in the order they were chosen.
    var targets: [String] = []
    var isEmpty: Bool { text.isEmpty && replyToID == nil && targets.isEmpty }
}

/// The app-level Teams data layer (spec §6.6, D10; the port of Desktop `packages/teams/workspace.ts`):
/// started when the app root appears, stopped on sign-out or an account change, never by a page closing, so
/// the Teams tab badge works without opening the tab. Created by the app root and injected with
/// `.environment(_:)`. The no-argument initialiser has no side effects; `start()` is idempotent.
///
/// - The team list and the selected team's members and invites come from HTTP (`GET /v1/teams`,
///   `/v1/teams/{id}`) and are authoritative; SDK listeners (`users/{uid}/teams`, `teams/{id}/members`) only
///   invalidate what is shown, and a response is applied only if no newer request or invalidation
///   superseded it (the `revision` / `authority` / `lifecycle` guards).
/// - The selected team has one full `TeamMessagingModel` (directory, reads, signals, the open thread, command
///   journals, Bot cards). Switching teams makes a fresh one for the new team and remembers the old team's
///   unread count.
/// - Every other team (at most `maxWatchedTeams`) has a directory-only `TeamsUnreadWatcher`; they feed the
///   switcher and the tab badge.
@MainActor @Observable
final class TeamsWorkspace {
    /// Teams other than the selected one that get a directory-only unread listener.
    static let maxWatchedTeams = 20
    /// Delay between a change notification and the refetch it causes.
    static let refreshDelay: Duration = .milliseconds(80)

    // MARK: Observable state

    /// The signed-in account, `nil` when signed out.
    private(set) var uid: String?
    private(set) var teams: [TeamSummary] = []
    private(set) var selectedTeamID = ""
    /// Members and invites of the selected team. Hidden (nil) from an invalidation until refetched.
    private(set) var detail: TeamDetail?
    /// The team list has been fetched (again) since the last invalidation.
    private(set) var listed = false
    /// A team command (create, join, invite, role, remove, leave, delete) is in flight.
    private(set) var busy = false
    /// A messaging action (create conversation, send, recall, mark read) is in flight.
    private(set) var messageBusy = false
    /// Code of the last failed team command or refetch; translate with `TeamsErrors.key`. Empty when none.
    private(set) var error = ""
    /// Code of the last failed refetch of the team list or detail; empty after a successful one.
    private(set) var loadError = ""
    /// Code of the last failed messaging action.
    private(set) var actionError = ""
    /// The invitation link created by the last successful invite command.
    private(set) var invitationLink = ""
    /// Increments whenever the visible account / membership view is invalidated; views drop open confirmations.
    private(set) var authority = 0
    /// The selected team's messaging model.
    private(set) var messaging: TeamMessagingModel?
    /// The team `messaging` belongs to. Survives an invalidation, so the open team stays on screen.
    private(set) var messagingTeam: TeamSummary?
    /// Collapsed list sections of the selected team.
    private(set) var collapsed: Set<TeamsSection> = []
    /// The platform notice (`teams_disclosure`) is shown once per run, under the first conversation that loads.
    private(set) var platformNoticeShown = false
    private var refreshing = 0
    private var unreadOthers: [String: Int] = [:]
    private var directories: [String: [TeamMember]] = [:]

    // MARK: Bookkeeping

    @ObservationIgnored private let preferences: TeamsPreferences
    @ObservationIgnored private let apiRoot: String
    @ObservationIgnored private let authOverride: Auth?
    @ObservationIgnored private let dbOverride: Firestore?
    @ObservationIgnored private let session = URLSession(configuration: .ephemeral, delegate: TeamMessageRedirectPolicy(), delegateQueue: nil)
    @ObservationIgnored private var started = false
    /// Bumped on start, stop and every account change; callbacks of an older one are dropped.
    @ObservationIgnored private var lifecycle = 0
    /// Bumped per refetch and per invalidation; only the newest response is applied.
    @ObservationIgnored private var revision = 0
    @ObservationIgnored private var authListener: AuthStateDidChangeListenerHandle?
    @ObservationIgnored private var indexListener: ListenerRegistration?
    @ObservationIgnored private var memberListener: (teamID: String, registration: ListenerRegistration)?
    @ObservationIgnored private var refreshTask: Task<Void, Never>?
    @ObservationIgnored private var watchers: [String: TeamsUnreadWatcher] = [:]
    @ObservationIgnored private var conversationByTeam: [String: String] = [:]
    @ObservationIgnored private var drafts: [String: TeamComposerDraft] = [:]
    /// The team to open first when it is in the list (the persisted selection).
    @ObservationIgnored private var initialTeamID = ""

    init(preferences: TeamsPreferences = TeamsPreferences(), apiRoot: String = "https://lingxios.net/v1/teams", auth: Auth? = nil, db: Firestore? = nil) {
        self.preferences = preferences
        self.apiRoot = apiRoot
        authOverride = auth
        dbOverride = db
    }

    private var auth: Auth { authOverride ?? Auth.auth() }
    private var db: Firestore { dbOverride ?? Firestore.firestore() }
    private var messagingRoot: URL { URL(string: apiRoot + "/messages")! }

    // MARK: Reading

    /// `true` until the first list has arrived for a signed-in account that has nothing on screen yet.
    var loading: Bool { uid != nil && !listed && messagingTeam == nil }
    var detailLoading: Bool { !selectedTeamID.isEmpty && detail == nil && refreshing > 0 }
    /// The selected team. Survives an invalidation while its conversations stay open.
    var team: TeamSummary? {
        if let value = teams.first(where: { $0.id == selectedTeamID }) { return value }
        if !selectedTeamID.isEmpty, messagingTeam?.id == selectedTeamID { return messagingTeam }
        return nil
    }
    /// People of the selected team, for display names; kept across an invalidation, unlike `detail`.
    var directory: [TeamMember] { detail?.members ?? directories[selectedTeamID] ?? [] }
    /// Unread messages per team: the selected team from its open directory (once its read marks are known),
    /// every other team from its directory-only watcher.
    var unreadByTeam: [String: Int] {
        var values = unreadOthers
        if let model = messaging, let team = messagingTeam, model.readsReady { values[team.id] = model.unreadTotal }
        return values
    }
    /// The Teams tab badge.
    var totalUnread: Int { unreadByTeam.values.reduce(0, +) }

    // MARK: Lifecycle

    func start() {
        guard !started else { return }
        started = true; lifecycle += 1
        FirebaseAccount.configure()
        let life = lifecycle
        authListener = auth.addStateDidChangeListener { [weak self] _, user in
            let uid = user?.uid
            Task { @MainActor in
                guard let self, self.started, self.lifecycle == life else { return }
                self.accountChanged(uid)
            }
        }
        accountChanged(auth.currentUser?.uid)
    }

    func stop() {
        guard started else { return }
        started = false; lifecycle += 1
        if let authListener { auth.removeStateDidChangeListener(authListener) }
        authListener = nil
        reset()
        uid = nil
    }

    private func reset() {
        revision += 1; authority += 1
        refreshTask?.cancel(); refreshTask = nil
        indexListener?.remove(); indexListener = nil
        memberListener?.registration.remove(); memberListener = nil
        closeMessaging()
        for watcher in watchers.values { watcher.stop() }
        watchers = [:]; unreadOthers = [:]; directories = [:]; conversationByTeam = [:]; drafts = [:]
        teams = []; selectedTeamID = ""; detail = nil; listed = false; invitationLink = ""
        busy = false; messageBusy = false; error = ""; loadError = ""; actionError = ""; collapsed = []
        initialTeamID = ""
    }

    private func accountChanged(_ value: String?) {
        guard started else { return }
        if value != uid {
            lifecycle += 1
            reset()
            uid = value
            if let value { initialTeamID = preferences.selectedTeam(uid: value) ?? "" }
        }
        guard let uid else { return }
        if indexListener == nil {
            let life = lifecycle
            indexListener = db.collection("users").document(uid).collection("teams")
                .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                    let invalidated = snapshot?.metadata.isFromCache != false || error != nil
                    Task { @MainActor in self?.changed(lifecycle: life, invalidated: invalidated) }
                }
        }
        scheduleRefresh()
    }

    /// A listener noticed a change in membership: hide what depended on it (when the snapshot is not
    /// server-confirmed) and refetch.
    private func changed(lifecycle life: Int, invalidated: Bool) {
        guard started, life == lifecycle else { return }
        if invalidated {
            revision += 1; authority += 1
            // Open conversations keep their own access checks; the unread counts stay until the list says otherwise.
            teams = []; detail = nil; invitationLink = ""; listed = false
            for watcher in watchers.values { watcher.stop() }
            watchers = [:]
        }
        scheduleRefresh()
    }

    // MARK: Team list and selection

    func scheduleRefresh() {
        refreshTask?.cancel()
        refreshTask = Task { [weak self] in
            do { try await Task.sleep(for: Self.refreshDelay) } catch { return }
            await self?.refresh()
        }
    }

    /// Refetch the team list and the selected team's detail. The newest request wins.
    func refresh() async {
        guard started, uid != nil else { return }
        revision += 1
        let request = revision, life = lifecycle, teamID = selectedTeamID
        refreshing += 1
        defer { refreshing -= 1 }
        let listFetch = Task { await self.attempt { () async throws -> TeamsList in try await self.call() } }
        let detailFetch = Task {
            await self.attempt { () async throws -> TeamDetail? in
                if teamID.isEmpty { return nil }
                return try await self.call("/" + Self.pathComponent(teamID)) as TeamDetail
            }
        }
        let listResult = await listFetch.value, detailResult = await detailFetch.value
        guard request == revision, life == lifecycle, started, !Task.isCancelled else { return }
        switch listResult {
        case let .failure(failure):
            self.listed = true
            error = Self.code(failure); loadError = error
            // Only a verdict hides what is shown; a connectivity failure keeps the last list and the open conversations.
            if Self.isVerdict(error) { detail = nil; teams = []; invitationLink = "" }
        case let .success(list):
            teams = list.teams; self.listed = true
            switch detailResult {
            case let .failure(failure):
                error = Self.code(failure); loadError = error
            case let .success(value):
                detail = value; error = ""; loadError = ""
                if let value { directories[value.team.id] = value.members }
            }
            // A selection follows the list: a team that is gone (left, removed, deleted) is dropped, one that merely failed to load is kept.
            if syncSelection() { scheduleRefresh() }
        }
    }

    func selectTeam(_ id: String) {
        guard started, id != selectedTeamID, let summary = teams.first(where: { $0.id == id }) else { return }
        remember(selection: id)
        detail = nil; invitationLink = ""; error = ""
        openMessaging(summary)
        watchSelectedMembers()
        reconcileWatchers()
        scheduleRefresh()
    }

    /// Run a team command: one at a time; ignored after an invalidation; refetches afterwards. `true` when applied.
    @discardableResult
    func execute(_ input: [String: Any]) async -> Bool {
        guard started, !busy, uid != nil else { return false }
        let life = lifecycle, authorized = authority
        let action = input["action"] as? String ?? ""
        busy = true; error = ""
        defer { if life == lifecycle { busy = false } }
        do {
            let result: TeamCommandResult = try await call(body: input)
            guard life == lifecycle, started else { return false }
            if action == "leave" || action == "delete" { dropSelection(); closeMessaging() }
            guard authorized == authority else { await refresh(); return false }
            if let code = result.code { invitationLink = TeamInvitation.link(code) }
            if let teamID = result.teamId, action == "create" || action == "join" {
                remember(selection: teamID)
                detail = nil
                watchSelectedMembers()
            }
            await refresh()
            return true
        } catch {
            if life == lifecycle { self.error = Self.code(error) }
            return false
        }
    }

    func clearError() { error = "" }
    func clearActionError() { actionError = "" }
    func markPlatformNoticeShown() { platformNoticeShown = true }

    private func dropSelection() {
        selectedTeamID = ""; detail = nil; invitationLink = ""; collapsed = []
        memberListener?.registration.remove(); memberListener = nil
    }

    private func remember(selection id: String) {
        selectedTeamID = id
        guard let uid else { return }
        preferences.setSelectedTeam(id, uid: uid)
        collapsed = preferences.collapsed(uid: uid, teamID: id)
    }

    /// Makes the messaging model follow the selected team. Returns whether a new team was chosen and still needs its detail.
    private func syncSelection() -> Bool {
        var chose = false
        if !selectedTeamID.isEmpty, !teams.contains(where: { $0.id == selectedTeamID }) {
            directories.removeValue(forKey: selectedTeamID)
            dropSelection()
        }
        if selectedTeamID.isEmpty, !teams.isEmpty {
            remember(selection: teams.first(where: { $0.id == initialTeamID })?.id ?? teams[0].id)
            chose = true
        }
        initialTeamID = ""
        if let summary = teams.first(where: { $0.id == selectedTeamID }) {
            if messagingTeam?.id != summary.id || messagingTeam?.generation != summary.generation {
                openMessaging(summary)
            } else {
                messagingTeam = summary
            }
        } else {
            closeMessaging()
        }
        watchSelectedMembers()
        reconcileWatchers()
        return chose
    }

    /// The members listener of the selected team: a membership change refetches the detail.
    private func watchSelectedMembers() {
        let teamID = selectedTeamID
        if memberListener?.teamID == teamID { return }
        memberListener?.registration.remove(); memberListener = nil
        guard started, !teamID.isEmpty else { return }
        let life = lifecycle
        let registration = db.collection("teams").document(teamID).collection("members")
            .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                let invalidated = snapshot?.metadata.isFromCache != false || error != nil
                Task { @MainActor in
                    guard let self, self.started, self.lifecycle == life else { return }
                    if error != nil, self.memberListener?.teamID == teamID {
                        self.memberListener?.registration.remove(); self.memberListener = nil
                    }
                    self.changed(lifecycle: life, invalidated: invalidated)
                }
            }
        memberListener = (teamID, registration)
    }

    // MARK: Messaging of the selected team

    private func openMessaging(_ team: TeamSummary) {
        guard let uid else { return }
        closeMessaging()
        let model = TeamMessagingModel(apiRoot: messagingRoot, auth: auth, db: db)
        model.onRevoked = { [weak self, weak model] in
            guard let self, let model, self.messaging === model else { return }
            self.scheduleRefresh()
        }
        messaging = model; messagingTeam = team; actionError = ""
        model.start(uid: uid, team: team)
    }

    private func closeMessaging() {
        if let model = messaging, let team = messagingTeam {
            if model.readsReady { unreadOthers[team.id] = model.unreadTotal }
            if !model.selected.isEmpty { conversationByTeam[team.id] = model.selected }
            model.onRevoked = nil
            model.stop()
        }
        messaging = nil; messagingTeam = nil
    }

    /// Opens a conversation's thread (its messages listener); an empty id closes it.
    func openConversation(_ id: String) {
        guard let model = messaging else { return }
        if model.selected != id { model.select(id) }
        if let team = messagingTeam, !id.isEmpty { conversationByTeam[team.id] = id }
    }

    /// The conversation that was open when the team was last on screen (the wide layout restores it).
    func rememberedConversation(for teamID: String) -> String? {
        conversationByTeam[teamID]
    }

    /// The banner's Reconnect: ask again, restart what stopped, and let the SDK reconnect.
    func reconnect() {
        guard started else { return }
        scheduleRefresh()
        messaging?.reconnect()
        for watcher in watchers.values { watcher.stop() }
        watchers = [:]
        reconcileWatchers()
    }

    /// The app came back to the foreground: retry what is pending and refetch after a failure.
    func foregrounded() {
        guard started else { return }
        messaging?.refresh()
        if !error.isEmpty || !loadError.isEmpty { scheduleRefresh() }
    }

    // MARK: Unread of the other teams

    private func reconcileWatchers() {
        guard started, let uid else { return }
        let wanted = Array(teams.filter { $0.id != selectedTeamID }.prefix(Self.maxWatchedTeams))
        let wantedByID = Dictionary(wanted.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        for (id, watcher) in Array(watchers) where wantedByID[id]?.generation != watcher.generation {
            watcher.stop(); watchers.removeValue(forKey: id)
        }
        for team in wanted where watchers[team.id] == nil { watch(team, uid: uid) }
        for id in Array(unreadOthers.keys) where id != selectedTeamID && wantedByID[id] == nil { unreadOthers.removeValue(forKey: id) }
    }

    private func watch(_ team: TeamSummary, uid: String) {
        let watcher = TeamsUnreadWatcher(
            uid: uid, team: team, auth: auth, db: db,
            onUnread: { [weak self] id, count in
                guard let self, self.started, self.watchers[id]?.generation == team.generation else { return }
                if self.unreadOthers[id] != count { self.unreadOthers[id] = count }
            },
            onStopped: { [weak self] id in
                // Revoked or failed: stop counting; the next list refresh restarts it if the team is still there.
                guard let self else { return }
                self.watchers.removeValue(forKey: id)
                self.unreadOthers.removeValue(forKey: id)
            })
        watchers[team.id] = watcher
        watcher.start()
    }

    // MARK: List sections and drafts

    func toggleSection(_ section: TeamsSection) {
        guard let uid, !selectedTeamID.isEmpty else { return }
        if collapsed.contains(section) { collapsed.remove(section) } else { collapsed.insert(section) }
        preferences.setCollapsed(collapsed, uid: uid, teamID: selectedTeamID)
    }

    func draft(for conversationID: String) -> TeamComposerDraft {
        drafts[draftKey(conversationID)] ?? TeamComposerDraft()
    }

    func setDraft(_ draft: TeamComposerDraft, for conversationID: String) {
        if draft.isEmpty { drafts.removeValue(forKey: draftKey(conversationID)) } else { drafts[draftKey(conversationID)] = draft }
    }

    private func draftKey(_ conversationID: String) -> String { "\(uid ?? ""):\(selectedTeamID):\(team?.generation ?? 0):\(conversationID)" }

    // MARK: Messaging actions

    /// Runs a messaging action with the screen's lock: one at a time, a failure is kept as `actionError`.
    /// A second call while one runs does nothing and returns `false`.
    @discardableResult
    func run(_ action: @MainActor () async throws -> Void) async -> Bool {
        guard started, !messageBusy else { return false }
        let life = lifecycle
        messageBusy = true; actionError = ""
        defer { if life == lifecycle { messageBusy = false } }
        do {
            try await action()
            return true
        } catch {
            if life == lifecycle { actionError = Self.code(error, fallback: "UNKNOWN") }
            return false
        }
    }

    /// Creates a dm, group or channel with people and Bots and returns its id; the model selects it. Every Bot
    /// must be usable now (`BOT_USE_REQUIRED`).
    func createConversation(kind: String, title: String, parties: [String], botParties: [String] = []) async -> String? {
        let id = UUID().uuidString
        let done = await run {
            guard let model = self.messaging else { throw TeamMessageFailure(code: "INVALID_SESSION") }
            if botParties.contains(where: { key in !model.bots.contains { $0.sharedBotId == key && $0.selectable } }) { throw TeamMessageFailure(code: "BOT_USE_REQUIRED") }
            try await model.create(kind: kind, title: JSText.trim(title), uids: parties, sharedBotIds: botParties, id: id)
        }
        return done ? id : nil
    }

    /// Opens the viewer's Bot DM with a Bot, creating it when `fresh` or when there is none. Returns the
    /// conversation to show. Throws nothing: a Bot that is not usable fails with `BOT_USE_REQUIRED` in `actionError`.
    func openBotDM(_ bot: TeamBotCard, fresh: Bool = false) async -> String? {
        var opened: String?
        let done = await run {
            guard let model = self.messaging, let uid = self.uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
            guard bot.selectable else { throw TeamMessageFailure(code: "BOT_USE_REQUIRED") }
            if !fresh, let existing = TeamsListLogic.findBotDm(model.cache?.conversations ?? [], uid: uid, sharedBotId: bot.sharedBotId) {
                opened = existing.id
                return
            }
            let id = UUID().uuidString
            try await model.create(kind: "bot_dm", title: bot.name, uids: [], sharedBotIds: [bot.sharedBotId], id: id)
            opened = id
        }
        return done ? opened : nil
    }

    /// Opens the viewer's DM with a person, creating it when there is none. Returns the conversation to show.
    func openDM(peerUID: String) async -> String? {
        var opened: String?
        let done = await run {
            guard let model = self.messaging, let uid = self.uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
            if let existing = TeamsListLogic.findDm(model.cache?.conversations ?? [], uid: uid, peerUid: peerUID) {
                opened = existing.id
                return
            }
            let id = UUID().uuidString
            try await model.create(kind: "dm", title: TeamsListLogic.defaultConversationTitle([peerUID], members: self.directory, uid: uid), uids: [peerUID], id: id)
            opened = id
        }
        return done ? opened : nil
    }

    /// Marks a conversation read (the header action and the row swipe). The open conversation is read up to
    /// its newest loaded message, any other up to its `lastSeq`. Nothing happens offline.
    @discardableResult
    func markRead(_ conversationID: String) async -> Bool {
        await run {
            guard let model = self.messaging, !conversationID.isEmpty else { return }
            if conversationID == model.selected { try await model.markRead(); return }
            guard let row = model.cache?.conversations.first(where: { $0.id == conversationID }), !model.offline, model.unread(of: row) > 0 else { return }
            try await model.markRead(conversationId: conversationID, lastReadSeq: row.lastSeq)
        }
    }

    /// The automatic mark-read of an open conversation (decision D12). It takes no lock and says nothing when it fails.
    func autoMarkRead(conversationID: String, lastReadSeq seq: Int) async {
        try? await messaging?.markRead(conversationId: conversationID, lastReadSeq: seq)
    }

    // MARK: HTTP

    private func attempt<T>(_ operation: @MainActor () async throws -> T) async -> Result<T, Error> {
        do { return .success(try await operation()) } catch { return .failure(error) }
    }

    private func call<T: Decodable>(_ path: String = "", body: [String: Any]? = nil) async throws -> T {
        guard let uid, let user = auth.currentUser, user.uid == uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        let token = try await user.getIDToken()
        guard auth.currentUser?.uid == uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        guard let url = URL(string: apiRoot + path) else { throw TeamMessageFailure(code: "INVALID_INPUT") }
        var request = URLRequest(url: url)
        request.timeoutInterval = 20
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.httpMethod = "POST"
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        let (data, response) = try await session.data(for: request)
        guard auth.currentUser?.uid == uid else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        guard let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) else {
            let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            throw TeamMessageFailure(code: (object?["error"] as? [String: Any])?["code"] as? String ?? "SERVICE_UNAVAILABLE")
        }
        return try JSONDecoder().decode(T.self, from: data)
    }

    private static func pathComponent(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? value
    }

    /// The failure code of an error: a server verdict, or `fallback` for anything else (connectivity).
    private static func code(_ error: Error, fallback: String = "SERVICE_UNAVAILABLE") -> String {
        (error as? TeamMessageFailure)?.code ?? fallback
    }

    /// A server verdict (an explicit error code) rather than a connectivity failure.
    private static func isVerdict(_ code: String) -> Bool {
        code != "SERVICE_UNAVAILABLE" && code.range(of: "^[A-Z][A-Z_]{2,59}$", options: .regularExpression) != nil
    }
}
