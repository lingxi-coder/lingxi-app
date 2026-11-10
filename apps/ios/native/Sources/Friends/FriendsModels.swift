import Foundation

// The value types of Friends (docs/FRIENDS-CLIENTS.md appendix A): the cards of `users/{uid}/friends`, the
// entries of `users/{uid}/blocks` and the results of `GET /v1/friends/profile` and `POST /v1/friends`.
// Foundation only, so the pure rules (`FriendsLogic`) and their tests run in the macOS XCTest harness. A
// Firestore `Timestamp` is recognised by the caller's `millis` closure, as in `TeamsPointerParser`.

/// The state of one card: a friend, or a request in either direction.
enum FriendState: String, Equatable, Sendable {
    case friends, incoming, outgoing
}

/// How a person relates to the viewer, as `lookup` reports it.
enum FriendRelation: String, Decodable, Equatable, Sendable {
    case none, friends, incoming, outgoing
}

/// One document of `users/{uid}/friends`: the whole collection is the list (requests and friends).
struct FriendCard: Equatable, Identifiable, Sendable {
    let uid: String
    let state: FriendState
    /// The name the person had when the relation was made (D10 refreshes it only when a chat or profile opens).
    let displayName: String
    let handle: String?
    /// The sender's note of a request.
    let note: String?
    /// When the relation began, epoch milliseconds.
    let sinceMs: Int64
    /// The pair team of a friend's chat (`friends` only).
    let teamId: String?
    let generation: Int?
    /// The chat summary (D13), `friends` only; a missing counter is 0.
    let lastSeq: Int
    let lastReadSeq: Int
    /// The Teams pointer to the newest message; `nil` = no message yet.
    let lastMessage: TeamLastMessage?

    var id: String { uid }
    /// `max(0, lastSeq - lastReadSeq)`; `lastChangeSeq` is the backend's ordering token and never counts.
    var unread: Int { max(0, lastSeq - lastReadSeq) }
    /// The time of the newest message in epoch milliseconds, `nil` when there is none or it is unreadable.
    var lastAtMs: Int64? { lastMessage?.atMs }

    /// A document of the friends collection, or `nil` when it is not a card this client can show: one bad card
    /// never fails the list. A pointer that is not the whole `lastMessage` counts as no message.
    static func parse(uid: String, data: [String: Any], millis: (Any) -> Int64?) -> FriendCard? {
        guard !uid.isEmpty, let state = (data["state"] as? String).flatMap(FriendState.init(rawValue:)) else { return nil }
        var teamId: String?, generation: Int?, lastSeq = 0, lastReadSeq = 0
        var lastMessage: TeamLastMessage?
        if state == .friends {
            guard let id = nonEmpty(data["teamId"]), let epoch = TeamsPointerParser.safeInteger(data["generation"]) else { return nil }
            teamId = id; generation = epoch
            lastSeq = max(0, TeamsPointerParser.safeInteger(data["lastSeq"]) ?? 0)
            lastReadSeq = max(0, TeamsPointerParser.safeInteger(data["lastReadSeq"]) ?? 0)
            if let pointer = data["lastMessage"], !(pointer is NSNull) {
                lastMessage = try? TeamsPointerParser.parseLastMessage(present: true, value: pointer, millis: millis)
            }
        }
        return FriendCard(
            uid: uid, state: state, displayName: data["displayName"] as? String ?? "", handle: nonEmpty(data["handle"]), note: nonEmpty(data["note"]),
            sinceMs: data["since"].flatMap(millis) ?? 0, teamId: teamId, generation: generation, lastSeq: lastSeq, lastReadSeq: lastReadSeq,
            lastMessage: lastMessage)
    }

    func renamed(_ name: String, handle: String?) -> FriendCard {
        FriendCard(
            uid: uid, state: state, displayName: name, handle: handle, note: note, sinceMs: sinceMs, teamId: teamId, generation: generation,
            lastSeq: lastSeq, lastReadSeq: lastReadSeq, lastMessage: lastMessage)
    }
}

/// One document of `users/{uid}/blocks`.
struct FriendBlock: Equatable, Identifiable, Sendable {
    let uid: String
    let atMs: Int64
    /// The person's name when the block was made; empty when the entry has none (D12 shows `Member`).
    let displayName: String
    let handle: String?

    var id: String { uid }

    static func parse(uid: String, data: [String: Any], millis: (Any) -> Int64?) -> FriendBlock {
        FriendBlock(uid: uid, atMs: data["at"].flatMap(millis) ?? 0, displayName: JSText.trim(data["displayName"] as? String ?? ""), handle: nonEmpty(data["handle"]))
    }
}

/// `GET /v1/friends/profile` and the `profile` action.
struct FriendProfile: Decodable, Equatable, Sendable {
    struct Code: Decodable, Equatable, Sendable {
        let code: String
        let link: String
    }

    let uid: String
    let displayName: String
    let handle: String?
    let findByHandle: Bool
    let findByEmail: Bool
    /// The one active friend link, `nil` when it is turned off.
    let friendCode: Code?

    /// The same profile with another friend link (`rotateCode` returned one, `revokeCode` removed it).
    func replacing(friendCode: Code?) -> FriendProfile {
        FriendProfile(uid: uid, displayName: displayName, handle: handle, findByHandle: findByHandle, findByEmail: findByEmail, friendCode: friendCode)
    }
}

/// The result of `lookup`: who a proof resolves to and how they relate to the viewer.
struct FriendLookupResult: Decodable, Equatable, Sendable {
    let uid: String
    let displayName: String
    let handle: String?
    let relation: FriendRelation
}

/// What a command answers: `request` (`uid`, `state`, `teamId` once it was mutual), `accept` (`state`, `teamId`), `redeem`
/// (`state`, `uid`, `teamId`), `rotateCode` (`code`, `link`); `decline`, `cancel`, `remove`, `block`, `unblock` and
/// `revokeCode` answer `{ok: true}`, which reads as all `nil`.
struct FriendCommandResult: Decodable, Equatable, Sendable {
    let uid: String?
    let state: String?
    let teamId: String?
    let code: String?
    let link: String?

    var becameFriends: Bool { state == FriendState.friends.rawValue }
}

private func nonEmpty(_ value: Any?) -> String? {
    guard let text = value as? String, !text.isEmpty else { return nil }
    return text
}
