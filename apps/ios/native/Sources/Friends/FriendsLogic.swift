import Foundation

// The Friends rules of docs/FRIENDS-CLIENTS.md (D5 order and filter, D6 relation actions, D7 the one search
// field, D11 profile form, D13 unread, D14 badge, D18 errors and request ids). Pure: Foundation only, no UI, no
// Firebase, no clock; the one language-dependent input, the collation locale, is a parameter. Runs in the
// macOS XCTest harness against `packages/teams/fixtures/friends-vectors.json`. Web, Desktop and Android have the
// same contract (`packages/teams/friends.ts`, `friends/FriendsLogic.kt`).

/// What a person typed or opened to name someone: the proof a lookup and the confirming request carry (D7).
/// A bare uid is never a proof.
enum FriendProof: Equatable, Sendable {
    case handle(String)
    case email(String)
    case code(String)
    /// A member of a team the viewer is in (D15).
    case team(teamId: String, uid: String)
}

/// The one action a result card offers for each relation (D6): `friends_add`, `friends_request_accept`, the disabled
/// `friends_request_sent`, `friends_relation_friends` with `friends_open_chat`.
enum FriendAction: String, Equatable, Sendable {
    case add, accept, sent, friends
}

/// The cards of the list in their sections (D3, D4).
struct FriendsListSections: Equatable, Sendable {
    var received: [FriendCard]
    var sent: [FriendCard]
    var friends: [FriendCard]
}

enum FriendsLogic {
    // MARK: Limits (vectors `limits`)

    /// Rows a request section shows before `friends_show_all(n)` (D4).
    static let sectionRows = 20
    /// UTF-16 units, as the server counts them (an emoji is 2).
    static let displayNameMax = 80
    static let noteMax = 200
    static let codeLength = 32
    /// The one conversation of a pair team.
    static let chatConversationID = "friend_chat"
    /// The name the backend gives a profile that has none; the Friends root asks for a better one (D3).
    static let placeholderName = "Member"

    // MARK: Order and filter (D5)

    /// `fold`: NFKC, lowercase (full Unicode mapping, no locale), NFD, accents (U+0300...U+036F) removed, NFC.
    static func fold(_ text: String) -> String {
        let lowered = text.precomposedStringWithCompatibilityMapping.lowercased()
        var kept = String.UnicodeScalarView()
        for scalar in lowered.decomposedStringWithCanonicalMapping.unicodeScalars where !(0x300...0x36F).contains(scalar.value) { kept.append(scalar) }
        return String(kept).precomposedStringWithCanonicalMapping
    }

    /// Names by the locale's collation at primary strength: letters compared ignoring case and accents, so "Sam"
    /// and "sam" tie and the uid decides. Not `localizedStandardCompare`, which orders names that differ only in case.
    /// `locale` is the language the app shows its strings in, not the device region. Negative = `a` first.
    static func compareNames(_ a: String, _ b: String, locale: Locale) -> Int {
        switch a.compare(b, options: [.caseInsensitive, .diacriticInsensitive], range: a.startIndex..<a.endIndex, locale: locale) {
        case .orderedAscending: -1
        case .orderedDescending: 1
        case .orderedSame: 0
        }
    }

    /// Requests: incoming before outgoing, then the newest `since` first, then the uid.
    static func compareRequests(_ a: FriendCard, _ b: FriendCard) -> Int {
        if a.state != b.state { return a.state == .incoming ? -1 : 1 }
        if a.sinceMs != b.sinceMs { return a.sinceMs > b.sinceMs ? -1 : 1 }
        return compareText(a.uid, b.uid)
    }

    /// Friends by last activity and then by name: those with a message first, the newer `lastMessage.at` first
    /// (whole epoch milliseconds), then the collated name, then the uid.
    static func compareFriends(_ a: FriendCard, _ b: FriendCard, locale: Locale) -> Int {
        let x = a.lastAtMs, y = b.lastAtMs
        if let x, let y, x != y { return x > y ? -1 : 1 }
        if (x == nil) != (y == nil) { return x == nil ? 1 : -1 }
        let byName = compareNames(a.displayName, b.displayName, locale: locale)
        return byName != 0 ? byName : compareText(a.uid, b.uid)
    }

    /// The whole collection: incoming requests, outgoing requests, then friends (their `since` decides nothing).
    static func compareCards(_ a: FriendCard, _ b: FriendCard, locale: Locale) -> Int {
        if a.state == .friends && b.state == .friends { return compareFriends(a, b, locale: locale) }
        if a.state == .friends || b.state == .friends { return a.state == .friends ? 1 : -1 }
        return compareRequests(a, b)
    }

    /// The whole collection in D5 order.
    static func sorted(_ cards: [FriendCard], locale: Locale) -> [FriendCard] {
        cards.sorted { compareCards($0, $1, locale: locale) < 0 }
    }

    /// The cards in D5 order, split into the sections of the list.
    static func sections(_ cards: [FriendCard], locale: Locale) -> FriendsListSections {
        let ordered = sorted(cards, locale: locale)
        return FriendsListSections(
            received: ordered.filter { $0.state == .incoming }, sent: ordered.filter { $0.state == .outgoing }, friends: ordered.filter { $0.state == .friends })
    }

    /// The filter field: the trimmed, folded query (a leading `@` dropped) must be inside the folded name or handle; empty matches everyone.
    static func matches(_ card: FriendCard, query: String) -> Bool {
        var needle = fold(JSText.trim(query))
        if needle.hasPrefix("@") { needle = JSText.trim(String(needle.dropFirst())) }
        if needle.isEmpty { return true }
        return fold(card.displayName).contains(needle) || (card.handle.map { fold($0).contains(needle) } ?? false)
    }

    /// Blocked people, newest first.
    static func sortedBlocks(_ blocks: [FriendBlock]) -> [FriendBlock] {
        blocks.sorted { $0.atMs != $1.atMs ? $0.atMs > $1.atMs : compareText($0.uid, $1.uid) < 0 }
    }

    /// Compared as text by code point, not numerically: "n10" comes before "n9".
    static func compareText(_ a: String, _ b: String) -> Int {
        if a.unicodeScalars.elementsEqual(b.unicodeScalars) { return 0 }
        return a.unicodeScalars.lexicographicallyPrecedes(b.unicodeScalars) ? -1 : 1
    }

    // MARK: Unread and badge (D13, D14)

    /// Incoming requests plus the unread of every friend; outgoing requests add nothing. The friend whose chat is
    /// open and visible (`reading`) counts as read: a display choice that also hides the lag between a message
    /// and the read mark reaching the card.
    static func badgeCount(_ cards: [FriendCard], reading: String? = nil) -> Int {
        cards.reduce(0) { total, card in
            switch card.state {
            case .incoming: total + 1
            case .outgoing: total
            case .friends: total + (card.uid == reading ? 0 : card.unread)
            }
        }
    }

    // MARK: Result card (D6)

    static func action(for relation: FriendRelation) -> FriendAction {
        switch relation {
        case .none: .add
        case .incoming: .accept
        case .outgoing: .sent
        case .friends: .friends
        }
    }

    // MARK: One field, three kinds (D7)

    /// Classifies the trimmed text, first match wins: a friend code (32 URL-safe characters, or a friend link), an email
    /// (an `@` after position 0), a handle (an optional leading `@`). `nil` = invalid.
    static func classify(_ input: String) -> FriendProof? {
        let text = JSText.trim(input)
        if text.isEmpty { return nil }
        if isCode(text) { return .code(text) }
        if let code = codeInLink(text) { return .code(code) }
        let scalars = Array(text.unicodeScalars)
        if scalars.dropFirst().contains("@") {
            let email = text.lowercased()
            return isEmail(email) ? .email(email) : nil
        }
        let name = String(String.UnicodeScalarView(scalars.first == "@" ? scalars.dropFirst() : scalars[...]))
        return isHandle(name) ? .handle(name) : nil
    }

    /// The body of the first call, `lookup`, with that one proof.
    static func lookupBody(_ proof: FriendProof) -> [String: String] {
        switch proof {
        case let .handle(handle): ["action": "lookup", "handle": handle]
        case let .email(email): ["action": "lookup", "email": email]
        case let .code(code): ["action": "lookup", "code": code]
        case let .team(teamId, uid): ["action": "lookup", "teamId": teamId, "uid": uid]
        }
    }

    /// The body of the call the confirming tap sends. A code always redeems (the link is the owner's standing consent); a
    /// handle, email or team lookup whose relation is `incoming` accepts; otherwise a `request` carries the same proof,
    /// the `requestId` minted at that tap and the note (200 characters at most, none for a code).
    static func confirmBody(_ proof: FriendProof, lookup: FriendLookupResult, requestId: String, note: String) -> [String: String] {
        if case let .code(code) = proof { return ["action": "redeem", "code": code, "requestId": requestId] }
        if lookup.relation == .incoming { return ["action": "accept", "otherUid": lookup.uid] }
        var body = ["action": "request", "requestId": requestId]
        switch proof {
        case let .handle(handle): body["via"] = "handle"; body["handle"] = handle
        case let .email(email): body["via"] = "email"; body["email"] = email
        case let .team(teamId, uid): body["via"] = "team"; body["teamId"] = teamId; body["uid"] = uid
        case .code: break
        }
        let text = JSText.trim(note)
        if !text.isEmpty { body["note"] = text }
        return body
    }

    // MARK: Profile form (D11)

    /// `^[A-Za-z0-9_]{3,24}$`.
    static func isHandle(_ text: String) -> Bool {
        let scalars = text.unicodeScalars
        return (3...24).contains(scalars.count) && scalars.allSatisfy { $0 == "_" || isASCIILetterOrDigit($0) }
    }

    /// Trimmed, 1...80 UTF-16 units.
    static func isDisplayName(_ text: String) -> Bool {
        let name = JSText.trim(text)
        return !name.isEmpty && name.utf16.count <= displayNameMax
    }

    /// Trimmed, at most 200 UTF-16 units; empty is fine (no note).
    static func isNote(_ text: String) -> Bool {
        JSText.trim(text).utf16.count <= noteMax
    }

    /// `text` cut from the end, whole characters at a time, until it fits `limit` UTF-16 units.
    static func clipped(_ text: String, to limit: Int) -> String {
        var result = text
        while result.utf16.count > limit { result.removeLast() }
        return result
    }

    /// The fields of the `profile` action that differ from the profile: only those are sent. An empty username means
    /// "leave as is" (once set it can be changed but not removed). Empty = nothing changed, so Save stays off.
    static func profileChanges(_ profile: FriendProfile, name: String, handle: String, findByHandle: Bool, findByEmail: Bool) -> [String: Any] {
        var changes: [String: Any] = [:]
        let newName = JSText.trim(name), newHandle = JSText.trim(handle)
        if newName != profile.displayName { changes["displayName"] = newName }
        if !newHandle.isEmpty, newHandle != profile.handle { changes["handle"] = newHandle }
        if findByHandle != profile.findByHandle { changes["findByHandle"] = findByHandle }
        if findByEmail != profile.findByEmail { changes["findByEmail"] = findByEmail }
        return changes
    }

    // MARK: Errors and request ids (D18)

    /// The string key an `error.code` shows under the control that failed.
    static func errorKey(_ code: String) -> String {
        switch code {
        case "NOT_FOUND": "friends_error_not_found"
        case "RATE_LIMITED": "friends_error_rate_limited"
        case "FRIEND_LIMIT": "friends_error_friend_limit"
        case "REQUEST_LIMIT": "friends_error_request_limit"
        case "HANDLE_TAKEN": "friends_error_handle_taken"
        case "REQUEST_UNAVAILABLE": "friends_error_request_unavailable"
        case "STALE_REQUEST": "friends_error_stale_request"
        case "SIGN_IN_REQUIRED", "INVALID_SESSION": "friends_error_session"
        case "SERVICE_UNAVAILABLE": "friends_error_network"
        case "INVALID_INPUT", "INVALID_JSON": "friends_error_invalid_input"
        case "MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "CONVERSATION_REQUIRED": "friends_chat_unavailable"
        default: "friends_error_generic"
        }
    }

    /// The key under the confirmation of a friend link (D8): a link that no longer resolves (revoked, rotated, blocked, the
    /// viewer's own) says so in its own words.
    static func linkErrorKey(_ code: String) -> String {
        code == "NOT_FOUND" ? "friends_link_invalid" : errorKey(code)
    }

    /// Whether the call's outcome is known. After an unknown outcome (the network failed, the request timed out, the
    /// service was unavailable) the same confirmation may be retried with the same `requestId`; after any other result
    /// the id is dropped.
    static func isDefinitive(_ code: String) -> Bool {
        code != "SERVICE_UNAVAILABLE"
    }

    /// The id of one confirmation (a UUID); never stored.
    static func newRequestID() -> String { UUID().uuidString }

    /// `^[A-Za-z0-9_-]{8,64}$`.
    static func isRequestID(_ text: String) -> Bool {
        let scalars = text.unicodeScalars
        return (8...64).contains(scalars.count) && scalars.allSatisfy { $0 == "_" || $0 == "-" || isASCIILetterOrDigit($0) }
    }

    // MARK: The pair team (D9)

    /// The pair-pinned workspace's team for a friend: a synthesized summary named after the friend and the two people
    /// (the viewer only when their profile name is known). `nil` unless the card is a friend with its team.
    static func pairTeam(for card: FriendCard, me: (uid: String, name: String)?) -> PairTeam? {
        guard card.state == .friends, let teamId = card.teamId, let generation = card.generation else { return nil }
        var members = [TeamMember(uid: card.uid, displayName: card.displayName, role: "member", generation: generation)]
        if let me { members.append(TeamMember(uid: me.uid, displayName: me.name, role: "member", generation: generation)) }
        return PairTeam(
            team: TeamSummary(id: teamId, name: card.displayName, memberCount: 2, role: "member", generation: generation),
            members: members, friendUID: card.uid, handle: card.handle)
    }

    // MARK: helpers

    private static func isASCIILetterOrDigit(_ scalar: Unicode.Scalar) -> Bool {
        (0x30...0x39).contains(scalar.value) || (0x41...0x5A).contains(scalar.value) || (0x61...0x7A).contains(scalar.value)
    }

    /// 32 characters of `[A-Za-z0-9_-]`.
    private static func isCode(_ text: String) -> Bool {
        let scalars = text.unicodeScalars
        return scalars.count == codeLength && scalars.allSatisfy { $0 == "_" || $0 == "-" || isASCIILetterOrDigit($0) }
    }

    /// The code of `lingxi://friend/add?code=`, `https://lingxios.net/open/friend/add?code=` or
    /// `https://lingxios.net/friends/add?code=`, with exactly that one parameter and no userinfo, port or fragment.
    private static func codeInLink(_ text: String) -> String? {
        guard let parts = URLComponents(string: text), parts.user == nil, parts.password == nil, parts.port == nil, parts.fragment == nil,
              let scheme = parts.scheme?.lowercased() else { return nil }
        let host = parts.host?.lowercased() ?? ""
        switch scheme {
        case "lingxi": guard host == "friend", parts.path == "/add" else { return nil }
        case "https": guard host == "lingxios.net", parts.path == "/open/friend/add" || parts.path == "/friends/add" else { return nil }
        default: return nil
        }
        guard let query = parts.percentEncodedQuery, query.hasPrefix("code=") else { return nil }
        let code = String(query.dropFirst("code=".count))
        return isCode(code) ? code : nil
    }

    /// `^[^\s@]+@[^\s@]+\.[^\s@]+$`, at most 254 UTF-16 units, with JavaScript's `\s`.
    private static func isEmail(_ text: String) -> Bool {
        let scalars = Array(text.unicodeScalars)
        guard text.utf16.count <= 254, !scalars.contains(where: AvatarIdentity.isSpace),
              let at = scalars.firstIndex(of: "@"), at > 0, scalars.lastIndex(of: "@") == at else { return false }
        let domain = Array(scalars[(at + 1)...])
        return domain.count >= 3 && (1..<(domain.count - 1)).contains { domain[$0] == "." }
    }
}
