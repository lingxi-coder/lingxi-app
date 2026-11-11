import Foundation
import Observation

/// One lookup and the confirmation that follows it (docs/FRIENDS-CLIENTS.md D6 to D8, D15, D18): the proof, the
/// person it resolved to, the note, and what came of the confirming tap. A screen or sheet owns one with `@State`;
/// dropping it drops the friend code, the result and the request id, none of which is ever persisted or logged.
@MainActor @Observable
final class FriendLookup {
    /// What a confirmation did.
    enum Outcome: Equatable {
        /// `request` was sent and is waiting for them (`friends_request_sent_to`).
        case requested
        /// `redeem`, `accept` or a mutual `request` made them friends: the chat opens (`friends_now_friends`).
        case friends
    }

    private(set) var proof: FriendProof?
    private(set) var result: FriendLookupResult?
    private(set) var looking = false
    private(set) var confirming = false
    /// Code of the last failed lookup or confirmation; empty when none.
    private(set) var error = ""
    private(set) var outcome: Outcome?
    /// The optional note of a request, at most 200 characters; a friend code carries none.
    var note = ""

    /// The confirmation in flight, or the last one whose outcome is unknown, with what it was made for (D18).
    private struct Attempt {
        let id: String
        let proof: FriendProof
        let note: String
    }
    @ObservationIgnored private var attempt: Attempt?
    /// Bumped by every lookup and every clear; the answer of an older one is dropped.
    @ObservationIgnored private var round = 0

    /// A friend code redeems; it has no note and no choice of action.
    var isCode: Bool {
        if case .code = proof { true } else { false }
    }

    /// `lookup` with one proof. The result replaces whatever was shown; a failure stays in `error`.
    func lookup(_ proof: FriendProof, in friends: FriendsWorkspace) async {
        clear()
        let current = round
        self.proof = proof
        looking = true
        defer { if current == round { looking = false } }
        do {
            let found = try await friends.lookup(proof)
            guard current == round else { return }
            result = found
        } catch {
            guard current == round else { return }
            self.error = FriendsWorkspace.code(error)
        }
    }

    /// The confirming tap (D7): `redeem` for a code, `accept` for someone who asked first, else `request`. Its request id
    /// is new for a new confirmation and kept only to retry the same one after an unknown outcome.
    func confirm(in friends: FriendsWorkspace) async {
        guard let proof, let result, !confirming, outcome == nil else { return }
        let current = round
        let text = isCode ? "" : JSText.trim(note)
        let id = attempt.flatMap { $0.proof == proof && $0.note == text ? $0.id : nil } ?? FriendsLogic.newRequestID()
        attempt = Attempt(id: id, proof: proof, note: text)
        let body = FriendsLogic.confirmBody(proof, lookup: result, requestId: id, note: text)
        confirming = true; error = ""
        defer { if current == round { confirming = false } }
        do {
            let answer = try await friends.command(body)
            guard current == round else { return }
            attempt = nil
            let friendsNow = body["action"] != "request" || answer.becameFriends
            if friendsNow { friends.expectFriend(result.uid) }
            outcome = friendsNow ? .friends : .requested
        } catch {
            guard current == round else { return }
            let code = FriendsWorkspace.code(error)
            self.error = code
            if FriendsLogic.isDefinitive(code) { attempt = nil }
        }
    }

    /// Editing the field, a new account or leaving the screen: everything is forgotten.
    func clear() {
        round += 1
        proof = nil; result = nil; looking = false; confirming = false
        error = ""; outcome = nil; note = ""; attempt = nil
    }
}
