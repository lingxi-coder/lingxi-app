import XCTest

@testable import LingxiCode

/// The Friends rules of docs/FRIENDS-CLIENTS.md (D5 order and filter, D6, D7, D11, D13, D14, D18, and for F4 the `offers.*`
/// sections: D26 picker and entry, D27 expiry and order, D28 badge, D30 errors) against the shared vectors every client loads
/// (`packages/teams/fixtures/friends-vectors.json`). Android runs the same rows in `FriendsLogicTest`, Web and Desktop in
/// `friends.test.ts`.
final class FriendsLogicTests: XCTestCase {
    private func strings(_ value: Any?) -> [String] { value as? [String] ?? [] }

    // MARK: limits, request ids, handles, lengths

    func testLimitsAreTheVectorLimits() throws {
        let limits = try XCTUnwrap(FriendsVectors.section("limits") as? [String: Any])
        XCTAssertEqual(FriendsLogic.sectionRows, limits["sectionRows"] as? Int)
        XCTAssertEqual(FriendsLogic.displayNameMax, limits["displayNameMax"] as? Int)
        XCTAssertEqual(FriendsLogic.noteMax, limits["noteMax"] as? Int)
        XCTAssertEqual(FriendsLogic.codeLength, limits["codeLength"] as? Int)
        XCTAssertEqual(FriendsLogic.chatConversationID, limits["chatConversationId"] as? String)
    }

    func testRequestIDsFollowThePattern() throws {
        let section = try XCTUnwrap(FriendsVectors.section("requestId") as? [String: Any])
        XCTAssertFalse(strings(section["valid"]).isEmpty)
        for id in strings(section["valid"]) { XCTAssertTrue(FriendsLogic.isRequestID(id), id) }
        for id in strings(section["invalid"]) { XCTAssertFalse(FriendsLogic.isRequestID(id), id) }
        XCTAssertTrue(FriendsLogic.isRequestID(FriendsLogic.newRequestID()), "a minted id is fine")
        XCTAssertNotEqual(FriendsLogic.newRequestID(), FriendsLogic.newRequestID())
    }

    func testHandlesFollowThePattern() throws {
        let section = try XCTUnwrap(FriendsVectors.section("handle") as? [String: Any])
        XCTAssertFalse(strings(section["valid"]).isEmpty)
        for handle in strings(section["valid"]) { XCTAssertTrue(FriendsLogic.isHandle(handle), handle) }
        for handle in strings(section["invalid"]) { XCTAssertFalse(FriendsLogic.isHandle(handle), handle) }
    }

    func testLengthsCountUTF16Units() throws {
        let rows = try FriendsVectors.rows("lengthRule")
        XCTAssertEqual(rows.count, 9)
        for row in rows {
            let field = try XCTUnwrap(row["field"] as? String)
            let text = String(repeating: try XCTUnwrap(row["unit"] as? String), count: try XCTUnwrap(row["count"] as? Int))
            let valid = field == "displayName" ? FriendsLogic.isDisplayName(text) : FriendsLogic.isNote(text)
            XCTAssertEqual(valid, row["valid"] as? Bool, "\(field) \(row["count"] ?? "?")")
        }
    }

    func testClippedKeepsWholeCharactersWithinTheLimit() {
        XCTAssertEqual(FriendsLogic.clipped("abc", to: 5), "abc")
        XCTAssertEqual(FriendsLogic.clipped("abcdef", to: 4), "abcd")
        XCTAssertEqual(FriendsLogic.clipped(String(repeating: "😀", count: 101), to: FriendsLogic.noteMax), String(repeating: "😀", count: 100))
        XCTAssertEqual(FriendsLogic.clipped("a😀", to: 2), "a", "an emoji is never cut in half")
    }

    // MARK: D5 order

    private func cards(_ rows: [[String: Any]], state: String? = nil) throws -> [FriendCard] {
        try rows.map { row in
            var row = row
            if let state { row["state"] = state }
            return try FriendsVectors.card(row)
        }
    }

    func testFriendsAreOrderedByActivityThenNameThenUID() throws {
        let vector = try XCTUnwrap(FriendsVectors.section("sort", "friends") as? [String: Any])
        let input = try cards(try XCTUnwrap(vector["input"] as? [[String: Any]]), state: "friends")
        XCTAssertEqual(input.count, 18)
        for locale in FriendsVectors.locales {
            let ordered = FriendsLogic.sections(input, locale: locale).friends.map(\.uid)
            XCTAssertEqual(ordered, strings(vector["expect"]), locale.identifier)
        }
    }

    func testTheWholeCollectionIsIncomingThenOutgoingThenFriends() throws {
        let vector = try XCTUnwrap(FriendsVectors.section("sort", "cards") as? [String: Any])
        let input = try cards(try XCTUnwrap(vector["input"] as? [[String: Any]]))
        for locale in FriendsVectors.locales {
            XCTAssertEqual(FriendsLogic.sorted(input, locale: locale).map(\.uid), strings(vector["expect"]), locale.identifier)
        }
        let sections = FriendsLogic.sections(input, locale: Locale(identifier: "en"))
        XCTAssertEqual(sections.received.map(\.uid), ["i2", "i3", "i1"])
        XCTAssertEqual(sections.sent.map(\.uid), ["o2", "o1"])
        XCTAssertEqual(sections.friends.map(\.uid), ["f1", "f4", "f2", "f3"])
    }

    func testRequestsAreIncomingFirstThenNewestThenUID() throws {
        let vector = try XCTUnwrap(FriendsVectors.section("sort", "requests") as? [String: Any])
        let input = try cards(try XCTUnwrap(vector["input"] as? [[String: Any]]))
        XCTAssertEqual(input.sorted { FriendsLogic.compareRequests($0, $1) < 0 }.map(\.uid), strings(vector["expect"]))
    }

    func testNamesTieAtPrimaryStrengthSoTheUIDDecides() {
        let en = Locale(identifier: "en")
        XCTAssertEqual(FriendsLogic.compareNames("Sam", "sam", locale: en), 0)
        XCTAssertEqual(FriendsLogic.compareNames("Alice", "alice", locale: en), 0)
        XCTAssertEqual(FriendsLogic.compareNames("Émile", "emile", locale: en), 0, "accents are ignored too")
        XCTAssertLessThan(FriendsLogic.compareNames("adam", "Al", locale: en), 0)
        XCTAssertGreaterThan(FriendsLogic.compareNames("Zed", "bob", locale: en), 0)
        XCTAssertLessThan(FriendsLogic.compareText("n10", "n9"), 0, "uids compare as text, not numerically")
        XCTAssertEqual(FriendsLogic.compareText("a", "a"), 0)
    }

    func testAMessageWithoutATimeCountsAsNone() throws {
        var row: [String: Any] = ["uid": "a", "state": "friends", "displayName": "Zed"]
        let none = try FriendsVectors.card(row)
        row["uid"] = "b"; row["displayName"] = "Amy"; row["lastAtMs"] = 5
        let some = try FriendsVectors.card(row)
        let unstamped = FriendCard(
            uid: "c", state: .friends, displayName: "Bea", handle: nil, note: nil, sinceMs: 0, teamId: "f_c", generation: 1, lastSeq: 1, lastReadSeq: 0,
            lastMessage: TeamLastMessage(id: "m", seq: 1, atMs: nil, authorKind: "user", authorUid: "x", authorSharedBotId: nil, preview: "hi", recalled: false))
        XCTAssertNil(unstamped.lastAtMs)
        XCTAssertEqual(FriendsLogic.sorted([none, unstamped, some], locale: Locale(identifier: "en")).map(\.uid), ["b", "c", "a"])
    }

    func testBlocksAreNewestFirst() {
        let blocks = [
            FriendBlock(uid: "b", atMs: 10, displayName: "", handle: nil), FriendBlock(uid: "a", atMs: 10, displayName: "", handle: nil),
            FriendBlock(uid: "c", atMs: 20, displayName: "", handle: nil),
        ]
        XCTAssertEqual(FriendsLogic.sortedBlocks(blocks).map(\.uid), ["c", "a", "b"])
    }

    // MARK: D5 filter

    func testFilterFollowsTheVectors() throws {
        let section = try XCTUnwrap(FriendsVectors.section("filter") as? [String: Any])
        let friends = try cards(try XCTUnwrap(section["friends"] as? [[String: Any]]), state: "friends")
        let cases = try XCTUnwrap(section["cases"] as? [[String: Any]])
        XCTAssertEqual(cases.count, 12)
        for testCase in cases {
            let query = try XCTUnwrap(testCase["query"] as? String)
            XCTAssertEqual(friends.filter { FriendsLogic.matches($0, query: query) }.map(\.uid), strings(testCase["expect"]), "'\(query)': \(testCase["note"] ?? "")")
        }
    }

    func testFoldRemovesCaseAccentsAndCompatibilityForms() {
        XCTAssertEqual(FriendsLogic.fold("ÉMILE"), "emile")
        XCTAssertEqual(FriendsLogic.fold("Zoë"), "zoe")
        XCTAssertEqual(FriendsLogic.fold("ＡＢＣ"), "abc", "NFKC first")
        XCTAssertEqual(FriendsLogic.fold("李雷"), "李雷")
    }

    // MARK: D13, D14

    func testUnreadIsTheGapBetweenTheCounters() throws {
        let rows = try FriendsVectors.rows("unread")
        XCTAssertEqual(rows.count, 8)
        for row in rows {
            var data: [String: Any] = ["state": "friends", "teamId": "f_1", "generation": 1, "displayName": "A"]
            data.merge(try XCTUnwrap(row["card"] as? [String: Any])) { _, new in new }
            let card = try XCTUnwrap(FriendCard.parse(uid: "u", data: data, millis: { _ in nil }))
            XCTAssertEqual(card.unread, row["expect"] as? Int, row["note"] as? String ?? "")
        }
    }

    func testBadgeCountsRequestsAndUnread() throws {
        let rows = try FriendsVectors.rows("badge")
        XCTAssertEqual(rows.count, 10)
        for row in rows {
            let note = row["note"] as? String ?? ""
            var raw = try XCTUnwrap(row["cards"] as? [[String: Any]])
            for index in raw.indices { raw[index]["uid"] = "u\(index)" }
            let count = FriendsLogic.badgeCount(try cards(raw))
            XCTAssertEqual(count, row["expectCount"] as? Int, note)
            XCTAssertEqual(TabLogic.tabBadgeText(count), row["expectText"] as? String, note)
        }
    }

    func testTheOpenChatCountsAsRead() throws {
        let cards = try self.cards([
            ["uid": "a", "state": "friends", "lastSeq": 5, "lastReadSeq": 2], ["uid": "b", "state": "friends", "lastSeq": 4],
            ["uid": "c", "state": "incoming"], ["uid": "d", "state": "outgoing"],
        ])
        XCTAssertEqual(FriendsLogic.badgeCount(cards), 8)
        XCTAssertEqual(FriendsLogic.badgeCount(cards, reading: "a"), 5, "3 unread of a are shown as read")
        XCTAssertEqual(FriendsLogic.badgeCount(cards, reading: "nobody"), 8)
        XCTAssertEqual(FriendsLogic.badgeCount(cards, reading: "c"), 8, "a request is not a chat")
    }

    // MARK: D6, D7

    func testEachRelationHasOneAction() throws {
        let map = try XCTUnwrap(FriendsVectors.section("relationAction") as? [String: String])
        XCTAssertEqual(map.count, 4)
        for (relation, action) in map {
            XCTAssertEqual(FriendsLogic.action(for: try XCTUnwrap(FriendRelation(rawValue: relation))).rawValue, action, relation)
        }
    }

    func testTheSearchFieldIsClassifiedByTheVectors() throws {
        let rows = try FriendsVectors.rows("query")
        XCTAssertGreaterThanOrEqual(rows.count, 30)
        let found = FriendLookupResult(uid: "u2", displayName: "N", handle: nil, relation: .none)
        for row in rows {
            let input = try XCTUnwrap(row["input"] as? String)
            let expect = try XCTUnwrap(row["expect"] as? [String: Any])
            let proof = FriendsLogic.classify(input)
            switch expect["kind"] as? String {
            case "handle": XCTAssertEqual(proof, .handle(try XCTUnwrap(expect["handle"] as? String)), input)
            case "email": XCTAssertEqual(proof, .email(try XCTUnwrap(expect["email"] as? String)), input)
            case "code": XCTAssertEqual(proof, .code(try XCTUnwrap(expect["code"] as? String)), input)
            default: XCTAssertNil(proof, "\(input): \(row["note"] ?? "")")
            }
            guard let proof else { continue }
            XCTAssertEqual(FriendsLogic.lookupBody(proof), row["lookup"] as? [String: String], input)
            var confirm = FriendsLogic.confirmBody(proof, lookup: found, requestId: "request-0001", note: "")
            XCTAssertEqual(confirm.removeValue(forKey: "requestId"), "request-0001", "redeem and request carry the id minted at the tap")
            XCTAssertEqual(confirm, row["confirm"] as? [String: String], input)
        }
    }

    func testAnIncomingRequestIsAcceptedNotRequestedAgain() {
        let incoming = FriendLookupResult(uid: "u9", displayName: "Ann", handle: "ann", relation: .incoming)
        let accept = ["action": "accept", "otherUid": "u9"]
        XCTAssertEqual(FriendsLogic.confirmBody(.handle("ann"), lookup: incoming, requestId: "request-0001", note: "hi"), accept)
        XCTAssertEqual(FriendsLogic.confirmBody(.email("a@b.co"), lookup: incoming, requestId: "request-0001", note: ""), accept)
        XCTAssertEqual(FriendsLogic.confirmBody(.team(teamId: "t1", uid: "u9"), lookup: incoming, requestId: "request-0001", note: ""), accept)
        let code = String(repeating: "a", count: 32)
        XCTAssertEqual(
            FriendsLogic.confirmBody(.code(code), lookup: incoming, requestId: "request-0001", note: "ignored"),
            ["action": "redeem", "code": code, "requestId": "request-0001"], "a code always redeems and takes no note")
    }

    func testATeamMemberIsRequestedThroughTheTeamProofWithATrimmedNote() {
        let none = FriendLookupResult(uid: "u9", displayName: "Ann", handle: nil, relation: .none)
        let proof = FriendProof.team(teamId: "t1", uid: "u9")
        XCTAssertEqual(FriendsLogic.lookupBody(proof), ["action": "lookup", "teamId": "t1", "uid": "u9"])
        XCTAssertEqual(
            FriendsLogic.confirmBody(proof, lookup: none, requestId: "request-0001", note: "  hello  "),
            ["action": "request", "via": "team", "teamId": "t1", "uid": "u9", "requestId": "request-0001", "note": "hello"])
        XCTAssertNil(FriendsLogic.confirmBody(proof, lookup: none, requestId: "request-0001", note: " \n ")["note"], "a blank note is not sent")
    }

    func testAFriendCodeIsOnlyRecognisedInTheThreeLinkForms() {
        let code = "aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-"
        for link in ["lingxi://friend/add?code=\(code)", "https://lingxios.net/open/friend/add?code=\(code)", "https://lingxios.net/friends/add?code=\(code)", "HTTPS://LINGXIOS.NET/friends/add?code=\(code)"] {
            XCTAssertEqual(FriendsLogic.classify(link), .code(code), link)
        }
        for link in [
            "https://lingxios.net:8443/open/friend/add?code=\(code)", "https://user@lingxios.net/open/friend/add?code=\(code)",
            "https://lingxios.net/open/friend/add?code=%61B0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-", "https://lingxios.net/open/friend/add?x=1&code=\(code)",
            "https://lingxios.net/open/friend/add/?code=\(code)", "lingxi://friend/add",
        ] {
            // Whatever else it is (a link with userinfo reads as an email), it is never a friend code.
            if case .code = FriendsLogic.classify(link) { XCTFail("\(link) is not a friend link") }
        }
    }

    // MARK: D11

    func testOnlyTheChangedProfileFieldsAreSent() throws {
        let profile = try JSONDecoder().decode(FriendProfile.self, from: Data(
            #"{"uid":"u","displayName":"Ann","handle":"Ann_1","findByHandle":true,"findByEmail":true,"friendCode":null}"#.utf8))
        func changes(name: String = "Ann", handle: String = "Ann_1", byHandle: Bool = true, byEmail: Bool = true) -> [String: Any] {
            FriendsLogic.profileChanges(profile, name: name, handle: handle, findByHandle: byHandle, findByEmail: byEmail)
        }
        XCTAssertTrue(changes().isEmpty, "Save stays off")
        XCTAssertEqual(changes(name: "  Ann Lee ")["displayName"] as? String, "Ann Lee")
        XCTAssertTrue(changes(name: " Ann ").isEmpty, "trimmed before comparing")
        XCTAssertTrue(changes(handle: "").isEmpty, "an empty username means leave as is")
        XCTAssertEqual(changes(handle: "ann_1")["handle"] as? String, "ann_1", "a change of case is a change")
        let both = changes(byHandle: false, byEmail: false)
        XCTAssertEqual(Set(both.keys), ["findByHandle", "findByEmail"])
        XCTAssertEqual(both["findByHandle"] as? Bool, false)
    }

    // MARK: D18

    func testErrorCodesMapToKeysAndKnownOutcomes() throws {
        let rows = try FriendsVectors.rows("errors")
        XCTAssertEqual(rows.count, 21)
        for row in rows {
            let code = try XCTUnwrap(row["code"] as? String)
            XCTAssertEqual(FriendsLogic.errorKey(code), row["key"] as? String, "'\(code)'")
            XCTAssertEqual(FriendsLogic.isDefinitive(code), row["definitive"] as? Bool, "'\(code)'")
            XCTAssertEqual(FriendsLogic.chatEnded(code), row["key"] as? String == "friends_chat_unavailable", "'\(code)' ends the chat only by that key")
        }
    }

    func testAFriendLinkThatDoesNotResolveHasItsOwnWords() {
        XCTAssertEqual(FriendsLogic.linkErrorKey("NOT_FOUND"), "friends_link_invalid")
        XCTAssertEqual(FriendsLogic.linkErrorKey("RATE_LIMITED"), "friends_error_rate_limited")
        XCTAssertEqual(FriendsLogic.linkErrorKey("SERVICE_UNAVAILABLE"), "friends_error_network")
    }

    // MARK: D9

    func testAFriendCardBecomesItsPairTeam() throws {
        let card = FriendCard(
            uid: "u2", state: .friends, displayName: "Ann", handle: "ann", note: nil, sinceMs: 1, teamId: "f_abc", generation: 1, lastSeq: 0, lastReadSeq: 0,
            lastMessage: nil)
        let pair = try XCTUnwrap(FriendsLogic.pairTeam(for: card, me: (uid: "u1", name: "Me")))
        XCTAssertEqual(pair.team, TeamSummary(id: "f_abc", name: "Ann", memberCount: 2, role: "member", generation: 1))
        XCTAssertEqual(pair.members.map(\.uid), ["u2", "u1"])
        XCTAssertEqual(pair.members.map(\.displayName), ["Ann", "Me"])
        XCTAssertEqual(pair.friendUID, "u2")
        XCTAssertEqual(pair.handle, "ann")
        XCTAssertEqual(FriendsLogic.pairTeam(for: card, me: nil)?.members.map(\.uid), ["u2"], "the viewer joins once their profile name is known")
        let request = FriendCard(
            uid: "u3", state: .incoming, displayName: "Bo", handle: nil, note: nil, sinceMs: 1, teamId: nil, generation: nil, lastSeq: 0, lastReadSeq: 0, lastMessage: nil)
        XCTAssertNil(FriendsLogic.pairTeam(for: request, me: nil), "only a friend has a pair team")
    }

    // MARK: F4, team invitations (vectors `offers.*`)

    /// The one clock of every `offers` row: the rows never age.
    private func offersNowMs() throws -> Int64 {
        try ms(FriendsVectors.section("offers", "nowMs"))
    }

    private func ms(_ value: Any?) throws -> Int64 {
        try XCTUnwrap((value as? NSNumber)?.int64Value, "epoch milliseconds, not \(String(describing: value))")
    }

    private func offer(_ team: String, _ expiresAtMs: Int64) -> TeamOffer {
        TeamOffer(teamId: team, teamName: "Team \(team)", fromUid: "u9", fromName: "Zoe", expiresAtMs: expiresAtMs)
    }

    func testOffersReadTheClockOfTheVectors() throws {
        XCTAssertEqual(try offersNowMs(), 1_762_000_000_000)
        let limits = try XCTUnwrap(FriendsVectors.section("limits") as? [String: Any])
        XCTAssertEqual(limits["offerTtlDays"] as? Int, 7)
        XCTAssertEqual(limits["offerLimit"] as? Int, 20)
        XCTAssertEqual(FriendsLogic.sectionRows, 20, "the Team invitations section is cut at the same 20 rows as the requests")
    }

    func testAnOfferExpiryIsRoundedUpToWholeHoursThenDays() throws {
        let now = try offersNowMs()
        let rows = try FriendsVectors.rows("offers", "expiry")
        XCTAssertEqual(rows.count, 18)
        for row in rows {
            let note = row["note"] as? String ?? ""
            let expect = try XCTUnwrap(row["expect"] as? [String: Any])
            let wanted: OfferExpiry
            if expect["state"] as? String == "expired" {
                wanted = .expired
            } else {
                wanted = .live(unit: try XCTUnwrap(OfferExpiry.Unit(rawValue: try XCTUnwrap(expect["unit"] as? String))), value: try XCTUnwrap(expect["value"] as? Int))
            }
            XCTAssertEqual(FriendsLogic.offerExpiry(expiresAtMs: try ms(row["expiresAtMs"]), nowMs: now), wanted, note)
        }
    }

    func testOffersAreLiveFirstThenNewestThenTeamID() throws {
        let now = try offersNowMs()
        let vector = try XCTUnwrap(FriendsVectors.section("offers", "sort") as? [String: Any])
        let rows = try XCTUnwrap(vector["input"] as? [[String: Any]])
        XCTAssertEqual(rows.count, 8)
        let offers = try rows.map { offer(try XCTUnwrap($0["teamId"] as? String), try ms($0["expiresAtMs"])) }
        XCTAssertEqual(FriendsLogic.sortedOffers(offers, nowMs: now).map(\.teamId), strings(vector["expect"]))
        XCTAssertEqual(FriendsLogic.sortedOffers(offers.reversed(), nowMs: now).map(\.teamId), strings(vector["expect"]), "the input order decides nothing")
        // A live offer always expires after an expired one, so the clock never reorders the rows: it only changes what they say.
        XCTAssertEqual(FriendsLogic.sortedOffers(offers, nowMs: now + 400_000_000).map(\.teamId), strings(vector["expect"]))
    }

    func testAnOfferThatExpiresExactlyNowIsNotLive() {
        let now: Int64 = 1_000
        XCTAssertTrue(FriendsLogic.isLive(offer("a", now + 1), nowMs: now))
        XCTAssertFalse(FriendsLogic.isLive(offer("a", now), nowMs: now), "the comparison is strict, as on the server")
        XCTAssertEqual(FriendsLogic.earliestLiveExpiry([offer("a", now), offer("b", now + 9), offer("c", now + 5)], nowMs: now), now + 5, "the timer fires at the first live expiry")
        XCTAssertNil(FriendsLogic.earliestLiveExpiry([offer("a", now), offer("b", now - 1)], nowMs: now), "nothing live: no timer")
        XCTAssertNil(FriendsLogic.earliestLiveExpiry([], nowMs: now))
    }

    func testTheBadgeAlsoCountsLiveOffers() throws {
        let now = try offersNowMs()
        let rows = try FriendsVectors.rows("offers", "badge")
        XCTAssertEqual(rows.count, 10)
        for row in rows {
            let note = row["note"] as? String ?? ""
            var raw = try XCTUnwrap(row["cards"] as? [[String: Any]])
            for index in raw.indices { raw[index]["uid"] = "u\(index)" }
            var offers: [TeamOffer] = []
            for (index, stamp) in try XCTUnwrap(row["offers"] as? [Any]).enumerated() { offers.append(offer("team_\(index)", try ms(stamp))) }
            let count = FriendsLogic.badgeCount(try cards(raw), offers: offers, nowMs: now)
            XCTAssertEqual(count, row["expectCount"] as? Int, note)
            XCTAssertEqual(TabLogic.tabBadgeText(count), row["expectText"] as? String, note)
        }
    }

    func testTheOpenChatStillCountsAsReadWithOffers() throws {
        let cards = try self.cards([["uid": "a", "state": "friends", "lastSeq": 5, "lastReadSeq": 2], ["uid": "c", "state": "incoming"]])
        let live = [offer("t1", 2_000), offer("t2", 1_500), offer("t3", 500)]
        XCTAssertEqual(FriendsLogic.badgeCount(cards, offers: live, nowMs: 1_000), 3 + 1 + 2)
        XCTAssertEqual(FriendsLogic.badgeCount(cards, offers: live, nowMs: 1_000, reading: "a"), 1 + 2, "3 unread of a are shown as read")
        XCTAssertEqual(FriendsLogic.badgeCount(cards, offers: live, nowMs: 1_600), 3 + 1 + 1, "an offer stops counting when its time comes")
        XCTAssertEqual(FriendsLogic.badgeCount(cards, offers: [], nowMs: 1_000), FriendsLogic.badgeCount(cards))
    }

    func testTheInvitePickerListsFriendsWhoAreNotInTheTeam() throws {
        let section = try XCTUnwrap(FriendsVectors.section("offers", "picker") as? [String: Any])
        let shared = try cards(try XCTUnwrap(section["cards"] as? [[String: Any]]))
        let cases = try XCTUnwrap(section["cases"] as? [[String: Any]])
        XCTAssertEqual(cases.count, 13)
        for testCase in cases {
            let input = try (testCase["cards"] as? [[String: Any]]).map { try cards($0) } ?? shared
            let members = Set(strings(testCase["members"]))
            let query = try XCTUnwrap(testCase["query"] as? String)
            for locale in FriendsVectors.locales {
                let listed = FriendsLogic.invitePicker(input, memberUIDs: members, query: query, locale: locale).map(\.uid)
                XCTAssertEqual(listed, strings(testCase["expect"]), "'\(query)' in \(locale.identifier): \(testCase["note"] ?? "")")
            }
        }
    }

    func testOnlyAnOwnerOrAnAdminSeesInviteFriends() throws {
        let rows = try FriendsVectors.rows("offers", "entry")
        XCTAssertEqual(rows.count, 5)
        for row in rows {
            let role = try XCTUnwrap(row["role"] as? String)
            XCTAssertEqual(FriendsLogic.canInviteFriends(role: role), row["expect"] as? Bool, "\(role) with \(row["memberCount"] ?? "?") members")
        }
        XCTAssertFalse(FriendsLogic.canInviteFriends(role: "guest"), "a role it does not know is no manager")
    }

    func testOfferErrorsAreChosenPerAction() throws {
        let rows = try FriendsVectors.rows("offers", "errors")
        XCTAssertEqual(rows.count, 33)
        for row in rows {
            let name = try XCTUnwrap(row["action"] as? String)
            let action = try XCTUnwrap(OfferAction(rawValue: name), name)
            let code = try XCTUnwrap(row["code"] as? String)
            XCTAssertEqual(FriendsLogic.offerErrorKey(action, code), row["key"] as? String, "\(name) '\(code)'")
            XCTAssertEqual(FriendsLogic.isDefinitive(code), row["definitive"] as? Bool, "\(name) '\(code)'")
        }
    }

    func testOfferErrorsNeverEndTheChat() {
        // `errorKey` reads these two as "the chat ended"; for an offer they mean something else (D30).
        XCTAssertEqual(FriendsLogic.errorKey("MEMBERSHIP_REQUIRED"), "friends_chat_unavailable")
        XCTAssertEqual(FriendsLogic.offerErrorKey(.offerTeam, "MEMBERSHIP_REQUIRED"), "friends_error_offer_team_gone")
        XCTAssertEqual(FriendsLogic.offerErrorKey(.acceptOffer, "MEMBERSHIP_REQUIRED"), "friends_error_generic")
        XCTAssertEqual(FriendsLogic.offerErrorKey(.acceptOffer, "STALE_MEMBERSHIP"), "friends_error_offer_unavailable")
        XCTAssertEqual(FriendsLogic.offerErrorKey(.offerTeam, "STALE_MEMBERSHIP"), "friends_error_generic")
        for action in [OfferAction.offerTeam, .acceptOffer, .declineOffer] {
            XCTAssertEqual(FriendsLogic.offerErrorKey(action, "INVALID_JSON"), "friends_error_invalid_input", "the common row also reads a malformed body")
            XCTAssertNotEqual(FriendsLogic.offerErrorKey(action, "CONVERSATION_REQUIRED"), "friends_chat_unavailable")
        }
        XCTAssertEqual(FriendsLogic.offerErrorKey(.acceptOffer, "RATE_LIMITED"), "friends_error_generic", "only offerTeam is rate limited")
        XCTAssertEqual(FriendsLogic.offerErrorKey(.declineOffer, "RATE_LIMITED"), "friends_error_generic")
    }

    func testOfferActionsCarryOnlyTheirFields() {
        XCTAssertEqual(
            FriendsLogic.offerTeamBody(teamID: "t1", friendUID: "u2", requestID: "request-0001"),
            ["action": "offerTeam", "teamId": "t1", "friendUid": "u2", "requestId": "request-0001"])
        XCTAssertEqual(FriendsLogic.acceptOfferBody(offerID: "t1", requestID: "request-0001"), ["action": "acceptOffer", "offerId": "t1", "requestId": "request-0001"])
        XCTAssertEqual(FriendsLogic.declineOfferBody(offerID: "t1"), ["action": "declineOffer", "offerId": "t1"], "no request id: it is safe to repeat")
    }

    func testAnExpiryReadsAsTheSystemsRelativeTimeInTheFuture() {
        let en = Locale(identifier: "en")
        XCTAssertEqual(FriendsLogic.relativeExpiry(unit: .day, value: 3, locale: en), "in 3 days")
        XCTAssertEqual(FriendsLogic.relativeExpiry(unit: .day, value: 7, locale: en), "in 7 days")
        XCTAssertEqual(FriendsLogic.relativeExpiry(unit: .hour, value: 5, locale: en), "in 5 hours")
        XCTAssertEqual(FriendsLogic.relativeExpiry(unit: .hour, value: 1, locale: en), "in 1 hour", "numeric always: never a word")
        XCTAssertEqual(FriendsLogic.relativeExpiry(unit: .hour, value: 24, locale: en), "in 24 hours")
        XCTAssertFalse(FriendsLogic.relativeExpiry(unit: .day, value: 3, locale: Locale(identifier: "zh-Hans")).isEmpty)
    }
}
