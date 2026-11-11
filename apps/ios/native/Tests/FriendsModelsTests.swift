import XCTest

@testable import LingxiCode

/// How a Firestore card, a block entry, a team offer and the HTTP results become the app's types (docs/FRIENDS-CLIENTS.md
/// D2, D12, D13, D27, appendix A). The `Timestamp` of a real document is stood in for by a number of epoch milliseconds.
final class FriendsModelsTests: XCTestCase {
    private let millis: (Any) -> Int64? = { ($0 as? NSNumber)?.int64Value }

    private func pointer(seq: Any, at: Any = 1_762_000_009_123, preview: String = "hi", author: String = "u2") -> [String: Any] {
        ["id": "m\(seq)", "seq": seq, "at": at, "author": ["kind": "user", "uid": author], "preview": preview]
    }

    private func friend(_ extra: [String: Any] = [:]) -> [String: Any] {
        var data: [String: Any] = ["state": "friends", "displayName": "Ann", "handle": "ann", "teamId": "f_abc", "generation": 1, "since": 1_762_000_000_000]
        data.merge(extra) { _, new in new }
        return data
    }

    // MARK: cards

    func testAFriendCardCarriesTheChatSummary() throws {
        let card = try XCTUnwrap(FriendCard.parse(
            uid: "u2", data: friend(["lastSeq": 7, "lastReadSeq": 4, "lastChangeSeq": 9, "lastMessage": pointer(seq: 7)]), millis: millis))
        XCTAssertEqual(card.state, .friends)
        XCTAssertEqual(card.displayName, "Ann")
        XCTAssertEqual(card.handle, "ann")
        XCTAssertEqual(card.teamId, "f_abc")
        XCTAssertEqual(card.generation, 1)
        XCTAssertEqual(card.sinceMs, 1_762_000_000_000, "whole epoch milliseconds, beyond 32 bits")
        XCTAssertEqual(card.unread, 3)
        XCTAssertEqual(card.lastAtMs, 1_762_000_009_123)
        XCTAssertEqual(card.lastMessage?.preview, "hi")
        XCTAssertEqual(card.lastMessage?.authorUid, "u2")
    }

    func testANewFriendshipHasNoSummary() throws {
        let card = try XCTUnwrap(FriendCard.parse(uid: "u2", data: friend(), millis: millis))
        XCTAssertEqual(card.unread, 0)
        XCTAssertNil(card.lastMessage)
        XCTAssertNil(card.lastAtMs)
        let null = try XCTUnwrap(FriendCard.parse(uid: "u2", data: friend(["lastMessage": NSNull()]), millis: millis))
        XCTAssertNil(null.lastMessage, "an explicit null is no message")
    }

    func testAPointerThatIsNotWholeCountsAsNoMessageAndNeverDropsTheCard() throws {
        var broken = pointer(seq: 3)
        broken.removeValue(forKey: "preview")
        for bad: Any in [broken, "text", 5, pointer(seq: 3, at: "yesterday"), pointer(seq: 1.5)] {
            let card = try XCTUnwrap(FriendCard.parse(uid: "u2", data: friend(["lastSeq": 3, "lastMessage": bad]), millis: millis), "\(bad)")
            XCTAssertNil(card.lastMessage, "\(bad)")
            XCTAssertEqual(card.unread, 3, "the counters still count")
        }
        let unstamped = try XCTUnwrap(FriendCard.parse(uid: "u2", data: friend(["lastMessage": pointer(seq: 1, at: NSNull())]), millis: millis))
        XCTAssertNotNil(unstamped.lastMessage)
        XCTAssertNil(unstamped.lastAtMs, "a pointer without a readable time counts as none for the order")
    }

    func testARecalledNewestMessageKeepsItsFlag() throws {
        var recalled = pointer(seq: 2, preview: "")
        recalled["recalled"] = true
        let card = try XCTUnwrap(FriendCard.parse(uid: "u2", data: friend(["lastSeq": 2, "lastMessage": recalled]), millis: millis))
        XCTAssertEqual(card.lastMessage?.recalled, true)
        XCTAssertEqual(card.lastMessage?.preview, "")
    }

    func testRequestsKeepTheirNoteAndNeedNoTeam() throws {
        let incoming = try XCTUnwrap(FriendCard.parse(
            uid: "u3", data: ["state": "incoming", "displayName": "Bo", "note": "from the book club", "since": 5], millis: millis))
        XCTAssertEqual(incoming.state, .incoming)
        XCTAssertEqual(incoming.note, "from the book club")
        XCTAssertNil(incoming.teamId)
        XCTAssertEqual(incoming.unread, 0)
        let outgoing = try XCTUnwrap(FriendCard.parse(uid: "u4", data: ["state": "outgoing", "displayName": "Cy", "handle": ""], millis: millis))
        XCTAssertEqual(outgoing.state, .outgoing)
        XCTAssertNil(outgoing.handle, "an empty handle is none")
        XCTAssertEqual(outgoing.sinceMs, 0)
    }

    func testOneBadCardIsDroppedAndTheRestSurvive() {
        XCTAssertNil(FriendCard.parse(uid: "u", data: ["state": "blocked", "displayName": "X"], millis: millis))
        XCTAssertNil(FriendCard.parse(uid: "u", data: ["displayName": "X"], millis: millis))
        XCTAssertNil(FriendCard.parse(uid: "", data: friend(), millis: millis))
        var withoutTeam = friend()
        withoutTeam.removeValue(forKey: "teamId")
        XCTAssertNil(FriendCard.parse(uid: "u", data: withoutTeam, millis: millis), "a friend without its pair team cannot be opened")
        XCTAssertNil(FriendCard.parse(uid: "u", data: friend(["generation": "1"]), millis: millis))
    }

    func testCountersThatAreNotWholeNumbersCountAsZero() throws {
        let card = try XCTUnwrap(FriendCard.parse(uid: "u", data: friend(["lastSeq": "9", "lastReadSeq": -4]), millis: millis))
        XCTAssertEqual(card.lastSeq, 0)
        XCTAssertEqual(card.lastReadSeq, 0)
    }

    func testRenamingKeepsEverythingElse() throws {
        let card = try XCTUnwrap(FriendCard.parse(uid: "u2", data: friend(["lastSeq": 2, "lastMessage": pointer(seq: 2)]), millis: millis))
        let renamed = card.renamed("Ann Lee", handle: nil)
        XCTAssertEqual(renamed.displayName, "Ann Lee")
        XCTAssertNil(renamed.handle)
        XCTAssertEqual(renamed.teamId, card.teamId)
        XCTAssertEqual(renamed.lastMessage, card.lastMessage)
        XCTAssertEqual(renamed.unread, card.unread)
    }

    // MARK: blocks

    func testABlockEntryKeepsThePersonsName() {
        let entry = FriendBlock.parse(uid: "u5", data: ["at": 42, "displayName": "  Dee ", "handle": "dee"], millis: millis)
        XCTAssertEqual(entry, FriendBlock(uid: "u5", atMs: 42, displayName: "Dee", handle: "dee"))
        let bare = FriendBlock.parse(uid: "u6", data: [:], millis: millis)
        XCTAssertEqual(bare.displayName, "", "the screen shows Member")
        XCTAssertNil(bare.handle)
        XCTAssertEqual(FriendBlock.parse(uid: "u7", data: ["displayName": "   "], millis: millis).displayName, "")
    }

    // MARK: team offers

    private func offerData(_ extra: [String: Any] = [:]) -> [String: Any] {
        var data: [String: Any] = ["teamId": "t1", "teamName": "Book club", "fromUid": "u2", "fromName": "Ann", "role": "member", "expiresAt": 1_762_604_800_000]
        data.merge(extra) { _, new in new }
        return data
    }

    func testAnOfferCarriesTheTeamTheSenderAndTheExpiry() throws {
        let offer = try XCTUnwrap(TeamOffer.parse(id: "t1", data: offerData(), millis: millis))
        XCTAssertEqual(offer, TeamOffer(teamId: "t1", teamName: "Book club", fromUid: "u2", fromName: "Ann", expiresAtMs: 1_762_604_800_000), "whole epoch milliseconds, beyond 32 bits")
        XCTAssertEqual(offer.id, "t1", "the team id is the row's identity and the offerId of its answers")
        let named = try XCTUnwrap(TeamOffer.parse(id: "t1", data: offerData(["teamName": "", "fromName": ""]), millis: millis))
        XCTAssertEqual(named.teamName, "", "a name is any string: the document is still an offer")
    }

    func testADocumentThatIsNotAnOfferIsSkippedAndNeverFailsTheList() {
        for key in ["teamId", "teamName", "fromUid", "fromName", "expiresAt"] {
            var broken = offerData()
            broken.removeValue(forKey: key)
            XCTAssertNil(TeamOffer.parse(id: "t1", data: broken, millis: millis), "no \(key)")
        }
        for key in ["teamId", "teamName", "fromUid", "fromName"] {
            XCTAssertNil(TeamOffer.parse(id: "t1", data: offerData([key: 5]), millis: millis), "\(key) must be a string")
        }
        XCTAssertNil(TeamOffer.parse(id: "t1", data: offerData(["expiresAt": "soon"]), millis: millis), "an expiry that is not a timestamp")
        XCTAssertNil(TeamOffer.parse(id: "t1", data: offerData(["expiresAt": NSNull()]), millis: millis))
        XCTAssertNil(TeamOffer.parse(id: "", data: offerData(), millis: millis), "a document without an id")
        let list = [("t1", offerData()), ("t2", offerData(["teamName": 1])), ("t3", offerData(["teamId": "t3"]))].compactMap { TeamOffer.parse(id: $0.0, data: $0.1, millis: millis) }
        XCTAssertEqual(list.map(\.teamId), ["t1", "t3"], "one bad document is dropped and the rest survive")
    }

    // MARK: HTTP results

    func testTheProfileDecodesWithAndWithoutAFriendLink() throws {
        let decoder = JSONDecoder()
        let on = try decoder.decode(FriendProfile.self, from: Data(
            #"{"uid":"u1","displayName":"Me","handle":"me_1","findByHandle":true,"findByEmail":false,"friendCode":{"code":"abc","link":"https://lingxios.net/open/friend/add?code=abc"}}"#.utf8))
        XCTAssertEqual(on.handle, "me_1")
        XCTAssertFalse(on.findByEmail)
        XCTAssertEqual(on.friendCode?.link, "https://lingxios.net/open/friend/add?code=abc")
        let off = try decoder.decode(FriendProfile.self, from: Data(#"{"uid":"u1","displayName":"Member","findByHandle":true,"findByEmail":true,"friendCode":null}"#.utf8))
        XCTAssertNil(off.friendCode)
        XCTAssertNil(off.handle)
        XCTAssertEqual(off.displayName, FriendsLogic.placeholderName)
    }

    func testALookupNamesTheRelation() throws {
        let decoder = JSONDecoder()
        for relation in ["none", "friends", "incoming", "outgoing"] {
            let result = try decoder.decode(FriendLookupResult.self, from: Data(#"{"uid":"u2","displayName":"Ann","relation":"\#(relation)"}"#.utf8))
            XCTAssertEqual(result.relation.rawValue, relation)
            XCTAssertNil(result.handle)
        }
        XCTAssertThrowsError(try decoder.decode(FriendLookupResult.self, from: Data(#"{"uid":"u2","displayName":"Ann","relation":"blocked"}"#.utf8)))
    }

    func testCommandResultsReadEveryShape() throws {
        let decoder = JSONDecoder()
        func result(_ json: String) throws -> FriendCommandResult { try decoder.decode(FriendCommandResult.self, from: Data(json.utf8)) }
        let ok = try result(#"{"ok":true}"#)
        XCTAssertNil(ok.uid); XCTAssertFalse(ok.becameFriends)
        let sent = try result(#"{"uid":"u2","state":"outgoing"}"#)
        XCTAssertEqual(sent.uid, "u2"); XCTAssertFalse(sent.becameFriends)
        let mutual = try result(#"{"uid":"u2","state":"friends","teamId":"f_abc"}"#)
        XCTAssertTrue(mutual.becameFriends); XCTAssertEqual(mutual.teamId, "f_abc")
        let accepted = try result(#"{"state":"friends","teamId":"f_abc"}"#)
        XCTAssertTrue(accepted.becameFriends); XCTAssertNil(accepted.uid)
        let rotated = try result(#"{"code":"abc","link":"https://lingxios.net/open/friend/add?code=abc"}"#)
        XCTAssertEqual(rotated.code, "abc"); XCTAssertEqual(rotated.link, "https://lingxios.net/open/friend/add?code=abc")
    }

    // MARK: the preview a row prints

    /// A Friends row prints the Teams text: the pointer alone gives the same line as the conversation it came from.
    func testAFriendRowPreviewIsTheTeamsPreview() throws {
        let strings = TeamsListStrings.en
        XCTAssertEqual(TeamsListLogic.pointerPreview(nil, uid: "u1", strings: strings), strings.noMessages)
        let cases: [(preview: String, author: String, recalled: Bool, expect: String)] = [
            ("hello", "u2", false, "hello"), ("hello", "u1", false, "You: hello"), ("", "u2", false, strings.nonText),
            ("", "u1", true, "You: " + strings.recalled), ("", "u2", true, strings.recalled),
        ]
        for item in cases {
            let last = TeamLastMessage(
                id: "m", seq: 1, atMs: 1, authorKind: "user", authorUid: item.author, authorSharedBotId: nil, preview: item.preview, recalled: item.recalled)
            XCTAssertEqual(TeamsListLogic.pointerPreview(last, uid: "u1", strings: strings), item.expect)
            let dm = TeamMessageConversation(id: "friend_chat", kind: "dm", title: "", participants: ["user:u1", "user:u2"], lastSeq: 1, lastChangeSeq: 1, lastReadSeq: 0, lastMessage: last)
            XCTAssertEqual(TeamsListLogic.conversationPreview(dm, uid: "u1", strings: strings), item.expect)
        }
    }
}
