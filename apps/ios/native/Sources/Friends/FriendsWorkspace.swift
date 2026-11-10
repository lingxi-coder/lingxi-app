import FirebaseAuth
import FirebaseFirestore
import Foundation
import Observation

/// The app-level Friends data layer (docs/FRIENDS-CLIENTS.md D2; the counterpart of `TeamsWorkspace`): started when
/// the app root appears, stopped on sign-out, restarted on an account change, never by a page closing, so the Friends
/// tab badge works without opening the tab. Created by the app root and injected with `.environment(_:)`. The
/// no-argument initialiser has no side effects; `start()` is idempotent.
///
/// - Live data is two owner-only Firestore listeners and nothing per friend: `users/{uid}/friends` (requests and
///   friends, the chat summary included) and `users/{uid}/blocks`. The list, the order, the previews, the unread
///   counts and the tab badge all come from the cards.
/// - Everything else is HTTP with the Firebase ID token: `GET /v1/friends/profile` and `POST /v1/friends`.
/// - A friend's chat is the Teams message layer over a `TeamsWorkspace` pinned to the pair team of the card (D9).
///   At most one is open at a time, and it is stopped as soon as the chat closes.
/// - On sign-out or an account change everything is cleared and every listener stops (D19); `authority` increases so
///   that sheets drop what they were about to confirm. Nothing is written to disk.
@MainActor @Observable
final class FriendsWorkspace {
    /// The open friend chat: whose it is, which pair team it runs on, and its Teams workspace.
    struct Chat {
        let uid: String
        let teamID: String
        let generation: Int
        let workspace: TeamsWorkspace
    }

    /// The three things a person can do with a request.
    enum RequestAction: String {
        case accept, decline, cancel
    }

    /// How long the card of a new friend may lag behind the command that made the friendship.
    static let cardGrace: Duration = .seconds(15)

    // MARK: Observable state

    /// The signed-in account, `nil` when signed out.
    private(set) var uid: String?
    /// The cards as the listener delivered them (names are the cards' own; `list` applies what D10 learned).
    private(set) var cards: [FriendCard] = []
    /// The first snapshot of the cards listener has arrived (D3). With metadata changes on, the SDK raises a saved copy only
    /// when it has documents or the device is offline, so an empty first snapshot is an answer too.
    private(set) var cardsLoaded = false
    /// The cards listener failed (`friends_load_error`, `common_retry`).
    private(set) var cardsFailed = false
    private(set) var blocks: [FriendBlock] = []
    private(set) var blocksLoaded = false
    private(set) var blocksFailed = false
    /// The caller's profile and friend link; in memory only, fetched on the first open and on every open of Add friend or
    /// Profile settings.
    private(set) var profile: FriendProfile?
    private(set) var profileLoading = false
    /// Code of the last failed profile fetch; empty after a good one.
    private(set) var profileError = ""
    /// People with an action in flight (their row is disabled).
    private(set) var busy: Set<String> = []
    /// Code of the last failed action on a row or on the chat menu.
    private(set) var actionError = ""
    /// A short confirmation shown once (`friends_now_friends`, `friends_chat_unavailable`).
    private(set) var notice: String?
    /// Increments whenever the account changes or the workspace stops; views drop open confirmations.
    private(set) var authority = 0
    /// The open friend chat, `nil` when none is open or it is not available.
    private(set) var chat: Chat?
    /// The friend whose chat the Friends tab has on its stack.
    private(set) var chatUID: String?
    /// The friend whose chat is on screen right now (visible, app in the foreground).
    private(set) var readingUID: String?
    /// People who just became friends through a command of this device, until their card arrives.
    private(set) var expecting: Set<String> = []
    /// What a friend's own profile said about their name (D10), for the rest of the session.
    private var overrides: [String: NameOverride] = [:]

    private struct NameOverride: Equatable {
        let name: String
        let handle: String?
    }

    // MARK: Bookkeeping

    @ObservationIgnored private let apiRoot: String
    @ObservationIgnored private let authOverride: Auth?
    @ObservationIgnored private let dbOverride: Firestore?
    @ObservationIgnored private let session = URLSession(configuration: .ephemeral, delegate: TeamMessageRedirectPolicy(), delegateQueue: nil)
    @ObservationIgnored private var started = false
    /// Bumped on start, stop and every account change; callbacks of an older one are dropped.
    @ObservationIgnored private var lifecycle = 0
    /// Bumped on start and stop only: the auth listener outlives account changes, which bump `lifecycle`.
    @ObservationIgnored private var authEpoch = 0
    @ObservationIgnored private var authListener: AuthStateDidChangeListenerHandle?
    @ObservationIgnored private var cardsListener: ListenerRegistration?
    @ObservationIgnored private var blocksListener: ListenerRegistration?
    /// The unsent text of each friendship's chat by pair team id, kept while the chat is closed (in memory, as the Teams drafts
    /// are). A re-friend gets a new pair team and so a fresh composer.
    @ObservationIgnored private var chatDrafts: [String: TeamComposerDraft] = [:]

    init(apiRoot: String = "https://lingxios.net/v1/friends", auth: Auth? = nil, db: Firestore? = nil) {
        self.apiRoot = apiRoot
        authOverride = auth
        dbOverride = db
    }

    private var auth: Auth { authOverride ?? Auth.auth() }
    private var db: Firestore { dbOverride ?? Firestore.firestore() }

    // MARK: Reading

    /// `true` until the first answer for a signed-in account.
    var loading: Bool { uid != nil && !cardsLoaded && !cardsFailed }
    /// The cards with the names D10 learned.
    var list: [FriendCard] {
        cards.map { card in overrides[card.uid].map { card.renamed($0.name, handle: $0.handle) } ?? card }
    }
    /// The friend with this uid, if they are one.
    func friend(_ uid: String) -> FriendCard? {
        guard let card = cards.first(where: { $0.uid == uid && $0.state == .friends }) else { return nil }
        return overrides[uid].map { card.renamed($0.name, handle: $0.handle) } ?? card
    }
    /// Blocked people, newest first.
    var blockedPeople: [FriendBlock] { FriendsLogic.sortedBlocks(blocks) }
    /// The Friends tab badge (D14): incoming requests plus the unread of every friend; the chat on screen counts as read.
    var badge: Int { uid == nil ? 0 : FriendsLogic.badgeCount(cards, reading: readingUID) }
    /// The unread count a row shows (D13): the card's, except for the chat on screen.
    func unread(of card: FriendCard) -> Int { card.uid == readingUID ? 0 : card.unread }
    func isBusy(_ uid: String) -> Bool { busy.contains(uid) }

    // MARK: Lifecycle

    func start() {
        guard !started else { return }
        started = true; lifecycle += 1; authEpoch += 1
        FirebaseAccount.configure()
        let current = authEpoch
        authListener = auth.addStateDidChangeListener { [weak self] _, user in
            let uid = user?.uid
            Task { @MainActor in
                guard let self, self.started, self.authEpoch == current else { return }
                self.accountChanged(uid)
            }
        }
        accountChanged(auth.currentUser?.uid)
    }

    func stop() {
        guard started else { return }
        started = false; lifecycle += 1; authEpoch += 1
        if let authListener { auth.removeStateDidChangeListener(authListener) }
        authListener = nil
        reset()
        uid = nil
    }

    /// D19: the listeners and the open chat stop, and cards, profile, name overrides and notices are forgotten.
    private func reset() {
        authority += 1
        cardsListener?.remove(); cardsListener = nil
        blocksListener?.remove(); blocksListener = nil
        chat?.workspace.stop()
        chat = nil; chatUID = nil; readingUID = nil; chatDrafts = [:]; expecting = []
        cards = []; cardsLoaded = false; cardsFailed = false
        blocks = []; blocksLoaded = false; blocksFailed = false
        profile = nil; profileLoading = false; profileError = ""
        overrides = [:]; busy = []; actionError = ""; notice = nil
    }

    private func accountChanged(_ value: String?) {
        guard started else { return }
        if value != uid {
            lifecycle += 1
            reset()
            uid = value
        }
        listen()
    }

    /// Opens the cards and blocks listeners that are not running. The Retry button calls it after a failure.
    private func listen() {
        guard started, let uid else { return }
        let life = lifecycle
        if cardsListener == nil {
            cardsListener = db.collection("users").document(uid).collection("friends")
                .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                    let parsed = snapshot.map { Self.cards(in: $0) }
                    let failed = error != nil
                    Task { @MainActor in self?.cardsChanged(life: life, cards: parsed, failed: failed) }
                }
        }
        if blocksListener == nil {
            blocksListener = db.collection("users").document(uid).collection("blocks")
                .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                    let parsed = snapshot.map { Self.blocks(in: $0) }
                    let failed = error != nil
                    Task { @MainActor in self?.blocksChanged(life: life, blocks: parsed, failed: failed) }
                }
        }
    }

    /// The Retry of `friends_load_error`: both listeners start again.
    func reload() {
        guard started, uid != nil else { return }
        cardsListener?.remove(); cardsListener = nil
        blocksListener?.remove(); blocksListener = nil
        cardsFailed = false; blocksFailed = false
        listen()
    }

    private nonisolated static func cards(in snapshot: QuerySnapshot) -> [FriendCard] {
        snapshot.documents.compactMap { FriendCard.parse(uid: $0.documentID, data: $0.data(), millis: TeamMessagingModel.milliseconds) }
    }

    private nonisolated static func blocks(in snapshot: QuerySnapshot) -> [FriendBlock] {
        snapshot.documents.map { FriendBlock.parse(uid: $0.documentID, data: $0.data(), millis: TeamMessagingModel.milliseconds) }
    }

    private func cardsChanged(life: Int, cards new: [FriendCard]?, failed: Bool) {
        guard started, life == lifecycle else { return }
        if failed { cardsFailed = true; return }
        guard let new else { return }
        cardsFailed = false
        cards = new
        cardsLoaded = true
        expecting.subtract(new.filter { $0.state == .friends }.map(\.uid))
        let names = Set(new.map(\.uid))
        overrides = overrides.filter { names.contains($0.key) }
        syncChat()
    }

    private func blocksChanged(life: Int, blocks new: [FriendBlock]?, failed: Bool) {
        guard started, life == lifecycle else { return }
        if failed { blocksFailed = true; return }
        guard let new else { return }
        blocksFailed = false
        blocks = new
        blocksLoaded = true
    }

    // MARK: Profile (D2, D11)

    /// `GET /v1/friends/profile`: creates the profile and the first friend link on first use. A failure is kept as
    /// `profileError`; it never blocks the list.
    func loadProfile() async {
        guard started, uid != nil, !profileLoading else { return }
        let life = lifecycle
        profileLoading = true; profileError = ""
        defer { if life == lifecycle { profileLoading = false } }
        do {
            let value: FriendProfile = try await call("/profile")
            guard life == lifecycle else { return }
            profile = value
            syncChat()
        } catch {
            if life == lifecycle { profileError = Self.code(error) }
        }
    }

    /// Sends one `profile` action with only the changed fields. `nil` when it worked, else the failure code.
    func saveProfile(_ changes: [String: Any]) async -> String? {
        guard started, uid != nil, !changes.isEmpty else { return nil }
        let life = lifecycle
        var body = changes
        body["action"] = "profile"
        do {
            let value: FriendProfile = try await call(body: body)
            if life == lifecycle { profile = value; syncChat() }
            return nil
        } catch {
            return Self.code(error)
        }
    }

    /// `rotateCode` (a new link; the old one stops working) or `revokeCode` (no link). `nil` when it worked, else the failure code.
    func changeFriendLink(rotate: Bool) async -> String? {
        guard started, uid != nil else { return Self.code(TeamMessageFailure(code: "INVALID_SESSION")) }
        let life = lifecycle
        do {
            let result: FriendCommandResult = try await call(body: ["action": rotate ? "rotateCode" : "revokeCode"])
            guard life == lifecycle else { return nil }
            let code = rotate ? result.code.flatMap { code in result.link.map { FriendProfile.Code(code: code, link: $0) } } : nil
            if rotate && code == nil { return "INVALID_RESPONSE" }
            profile = profile?.replacing(friendCode: code)
            return nil
        } catch {
            return Self.code(error)
        }
    }

    // MARK: Requests and the chat menu (D4, D10, D12)

    /// Accept, decline or cancel one request (D4). The row is disabled meanwhile; the listener removes it.
    func respond(_ action: RequestAction, to card: FriendCard) async {
        let done = await perform(["action": action.rawValue, "otherUid": card.uid], on: card.uid)
        if done, action == .accept { show(notice: String(localized: "friends_now_friends \(card.displayName)")) }
    }

    /// `remove`, `block` or `unblock` (D10, D12). `true` when it was applied.
    @discardableResult
    func end(_ action: String, with otherUID: String) async -> Bool {
        await perform(["action": action, "otherUid": otherUID], on: otherUID)
    }

    private func perform(_ body: [String: String], on key: String) async -> Bool {
        guard started, uid != nil, !busy.contains(key) else { return false }
        let life = lifecycle
        busy.insert(key); actionError = ""
        defer { if life == lifecycle { busy.remove(key) } }
        do {
            let _: FriendCommandResult = try await call(body: body)
            return life == lifecycle
        } catch {
            if life == lifecycle { actionError = Self.code(error) }
            return false
        }
    }

    func clearActionError() { actionError = "" }
    func show(notice text: String) { notice = text }
    func clearNotice() { notice = nil }

    // MARK: Lookup and confirmation (D6 to D8, D15)

    func lookup(_ proof: FriendProof) async throws -> FriendLookupResult {
        try await call(body: FriendsLogic.lookupBody(proof))
    }

    func command(_ body: [String: String]) async throws -> FriendCommandResult {
        try await call(body: body)
    }

    /// A command of this device made a friendship: its card arrives a moment later, and a chat that is asked for
    /// meanwhile waits for it instead of reporting that the chat is gone.
    func expectFriend(_ uid: String) {
        expecting.insert(uid)
        let life = lifecycle
        Task { [weak self] in
            try? await Task.sleep(for: Self.cardGrace)
            guard let self, self.lifecycle == life else { return }
            self.expecting.remove(uid)
        }
    }

    // MARK: A friend's chat (D9, D10)

    /// The chat on the Friends tab's stack: it opens as soon as the card says they are a friend, and `nil` (the screen
    /// is gone) stops its listeners and keeps its unsent text.
    func openChat(_ uid: String?) {
        guard chatUID != uid else { return }
        chatUID = uid
        if uid == nil { readingUID = nil }
        syncChat()
    }

    /// The chat screen of `uid` is on screen (or not): the list and the badge show it as read meanwhile.
    func reading(_ uid: String, visible: Bool) {
        if visible {
            readingUID = uid
        } else if readingUID == uid {
            readingUID = nil
        }
    }

    /// The app came back to the foreground: the open chat retries what is pending (its outbox, a lost listener).
    func foregrounded() {
        chat?.workspace.foregrounded()
    }

    /// The Teams workspace of the chat with `uid`, once it is running.
    func chat(for uid: String) -> Chat? {
        chat?.uid == uid ? chat : nil
    }

    /// D10 "View profile": reads `profiles/{uid}` once (ignoring a failure) and, if the name or username differ from
    /// the card, uses the profile's for this friend from now on. Cards are never written.
    func refreshProfile(of friendUID: String) async {
        guard started, uid != nil, let card = friend(friendUID) else { return }
        let life = lifecycle
        guard let snapshot = try? await db.collection("profiles").document(friendUID).getDocument(), snapshot.exists,
              let data = snapshot.data(), let name = data["displayName"] as? String, !JSText.isBlank(name), life == lifecycle else { return }
        let handle = (data["handle"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        guard name != card.displayName || handle != card.handle else { return }
        overrides[friendUID] = NameOverride(name: name, handle: handle)
        syncChat()
    }

    /// Opens, keeps or ends the chat that was asked for, from the card as it is now. A card that is gone, or a new
    /// friendship (another pair team), ends the old workspace; the new pair opens a new one.
    private func syncChat() {
        guard let chatUID else {
            retireChat()
            return
        }
        guard started, let card = friend(chatUID), let pair = FriendsLogic.pairTeam(for: card, me: profile.map { (uid: $0.uid, name: $0.displayName) }) else {
            retireChat()
            return
        }
        if let chat, chat.teamID == pair.team.id, chat.generation == pair.team.generation {
            chat.workspace.updatePair(pair)
            return
        }
        retireChat()
        let workspace = TeamsWorkspace(pair: pair, auth: authOverride, db: dbOverride)
        workspace.start()
        if let draft = chatDrafts[pair.team.id] { workspace.setDraft(draft, for: FriendsLogic.chatConversationID) }
        chat = Chat(uid: chatUID, teamID: pair.team.id, generation: pair.team.generation, workspace: workspace)
    }

    private func retireChat() {
        guard let chat else { return }
        let draft = chat.workspace.draft(for: FriendsLogic.chatConversationID)
        if draft.isEmpty { chatDrafts.removeValue(forKey: chat.teamID) } else { chatDrafts[chat.teamID] = draft }
        chat.workspace.stop()
        self.chat = nil
    }

    // MARK: HTTP

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

    /// The failure code of an error: a server verdict, or `SERVICE_UNAVAILABLE` for anything else (connectivity, a timeout).
    static func code(_ error: Error) -> String {
        (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
    }
}
