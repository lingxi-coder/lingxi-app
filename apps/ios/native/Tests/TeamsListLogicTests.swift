import XCTest

@testable import LingxiCode

/// Spec §8.1: unread, ordering, grouping, labels, previews and search of the Teams list, against the shared
/// vectors (`mobile-team-bot-vectors.json`, produced by running Desktop's `workspace.ts`). A failing row here
/// and not on Android is a parity bug in this platform, never a reason to edit the file.
final class TeamsListLogicTests: XCTestCase {
    private let uid = "u1"

    // MARK: adapters (vector shape -> app types)

    private func members() throws -> [TeamMember] {
        try MobileVectors.rows("members").map {
            TeamMember(uid: try XCTUnwrap($0["uid"] as? String), displayName: try XCTUnwrap($0["displayName"] as? String), role: "member", generation: 1)
        }
    }

    private func bots() throws -> [TeamBotCard] {
        try MobileVectors.rows("bots").map {
            try JSONDecoder().decode(TeamBotCard.self, from: JSONSerialization.data(withJSONObject: $0))
        }
    }

    static func instant(_ iso: String) -> Int64 {
        let formatter = ISO8601DateFormatter()
        return Int64((formatter.date(from: iso)!.timeIntervalSince1970 * 1000).rounded())
    }

    /// `at`: absent = no pointer; null = a pointer whose server time is not stamped yet; string = its time.
    /// The pointer of the list tests is {id: "m"+id, seq: lastSeq, author: user u2, preview: "hi"}.
    private func conversation(_ row: [String: Any]) throws -> TeamMessageConversation {
        let id = try XCTUnwrap(row["id"] as? String)
        var pointer: TeamLastMessage?
        if row.keys.contains("at") {
            let atMs = (row["at"] as? String).map(Self.instant)
            pointer = TeamLastMessage(id: "m" + id, seq: row["lastSeq"] as? Int ?? 0, atMs: atMs, authorKind: "user", authorUid: "u2", authorSharedBotId: nil, preview: "hi", recalled: false)
        }
        return TeamMessageConversation(
            id: id, kind: try XCTUnwrap(row["kind"] as? String), title: row["title"] as? String ?? "",
            participants: row["participants"] as? [String] ?? [], lastSeq: row["lastSeq"] as? Int ?? 0, lastChangeSeq: 0,
            lastReadSeq: row["lastReadSeq"] as? Int ?? 0, sharedBotId: row["sharedBotId"] as? String, lastMessage: pointer)
    }

    private func fixture() throws -> [String: TeamMessageConversation] {
        var result: [String: TeamMessageConversation] = [:]
        for row in try MobileVectors.rows("conversations") {
            let value = try conversation(row)
            result[value.id] = value
        }
        return result
    }

    private func strings(zh: Bool) -> TeamsListStrings { zh ? .zh : .en }

    // MARK: vectors

    func testTheTwoStringSetsAreTheVectorSets() throws {
        for (name, value) in [("en", TeamsListStrings.en), ("zh", TeamsListStrings.zh)] {
            let row = try XCTUnwrap(MobileVectors.section("strings", name) as? [String: String])
            XCTAssertEqual(row["channel"], value.channel, name)
            XCTAssertEqual(row["directMessage"], value.directMessage, name)
            XCTAssertEqual(row["group"], value.group, name)
            XCTAssertEqual(row["member"], value.member, name)
            XCTAssertEqual(row["you"], value.you, name)
            XCTAssertEqual(row["bot"], value.bot, name)
            XCTAssertEqual(row["noMessages"], value.noMessages, name)
            XCTAssertEqual(row["recalled"], value.recalled, name)
            XCTAssertEqual(row["nonText"], value.nonText, name)
            XCTAssertEqual(row["listSeparator"], value.listSeparator, name)
            XCTAssertEqual(row["authorSeparator"], value.authorSeparator, name)
        }
        XCTAssertTrue(TeamsListStrings.isChinese(languageTag: "zh-Hans"))
        XCTAssertTrue(TeamsListStrings.isChinese(languageTag: "ZH-tw"))
        XCTAssertFalse(TeamsListStrings.isChinese(languageTag: "en-US"))
        XCTAssertFalse(TeamsListStrings.isChinese(languageTag: "ja"))
    }

    func testUnreadCount() throws {
        let rows = try MobileVectors.rows("unreadCount")
        XCTAssertEqual(rows.count, 3)
        for row in rows {
            let value = TeamMessageConversation(id: "x", kind: "channel", title: "", participants: [], lastSeq: try XCTUnwrap(row["lastSeq"] as? Int),
                                                lastChangeSeq: 0, lastReadSeq: try XCTUnwrap(row["lastReadSeq"] as? Int))
            XCTAssertEqual(TeamsListLogic.unreadCount(value), row["expect"] as? Int)
            XCTAssertEqual(value.unread, row["expect"] as? Int)
        }
    }

    func testSort() throws {
        let all = try fixture()
        let sort = try XCTUnwrap(MobileVectors.section("sort") as? [String: Any])
        let input = try XCTUnwrap(sort["input"] as? [String]).map { try XCTUnwrap(all[$0]) }
        let expected = try XCTUnwrap(sort["expect"] as? [String])
        XCTAssertEqual(input.sorted { TeamsListLogic.compareConversations($0, $1) < 0 }.map(\.id), expected)
        // Whatever order the rows arrive in, the result is the same.
        XCTAssertEqual(input.reversed().sorted { TeamsListLogic.compareConversations($0, $1) < 0 }.map(\.id), expected)
    }

    func testGroup() throws {
        let all = try fixture()
        let group = try XCTUnwrap(MobileVectors.section("group") as? [String: Any])
        let input = try XCTUnwrap(group["input"] as? [String]).map { try XCTUnwrap(all[$0]) }
        let expect = try XCTUnwrap(group["expect"] as? [String: [String]])
        let result = TeamsListLogic.groupConversations(input, uid: uid)
        XCTAssertEqual(result.channels.map(\.id), expect["channels"])
        XCTAssertEqual(result.direct.map(\.id), expect["direct"])
        XCTAssertEqual(result.botDirect.map(\.id), expect["botDirect"])
    }

    func testLabels() throws {
        let all = try fixture(), members = try members(), bots = try bots()
        let rows = try MobileVectors.rows("labels")
        XCTAssertEqual(rows.count, 12)
        for row in rows {
            let spec = try XCTUnwrap(row["conversation"] as? [String: Any])
            let value = spec["kind"] == nil ? try XCTUnwrap(all[try XCTUnwrap(spec["id"] as? String)]) : try conversation(spec)
            let zh = row["zh"] as? Bool ?? false
            XCTAssertEqual(TeamsListLogic.conversationLabel(value, members: members, uid: uid, bots: bots, strings: strings(zh: zh)), row["expect"] as? String, row["name"] as? String ?? "")
        }
    }

    func testDefaultConversationTitle() throws {
        let members = try members()
        let rows = try MobileVectors.rows("defaultConversationTitle")
        XCTAssertEqual(rows.count, 2)
        for row in rows {
            let title = TeamsListLogic.defaultConversationTitle(try XCTUnwrap(row["uids"] as? [String]), members: members, uid: uid, limit: try XCTUnwrap(row["limit"] as? Int))
            XCTAssertEqual(title, row["expect"] as? String)
        }
    }

    func testParticipants() throws {
        let all = try fixture(), members = try members(), bots = try bots()
        let spec = try XCTUnwrap(MobileVectors.section("participants") as? [String: Any])
        let value = try XCTUnwrap(all[try XCTUnwrap(spec["conversation"] as? String)])
        let expect = try XCTUnwrap(spec["expect"] as? [[String: Any]])
        let result = TeamsListLogic.conversationParticipants(value, members: members, bots: bots, uid: uid, strings: .en)
        XCTAssertEqual(result.map(\.kind.rawValue), expect.map { $0["kind"] as? String ?? "" })
        XCTAssertEqual(result.map(\.id), expect.map { $0["id"] as? String ?? "" })
        XCTAssertEqual(result.map(\.name), expect.map { $0["name"] as? String ?? "" })
        XCTAssertEqual(result.map(\.isSelf), expect.map { $0["self"] as? Bool ?? true })
    }

    func testFinds() throws {
        let all = try fixture()
        let finds = try XCTUnwrap(MobileVectors.section("finds") as? [String: Any])
        let botDm = try XCTUnwrap(finds["botDm"] as? String), dm = try XCTUnwrap(finds["dm"] as? String)
        XCTAssertEqual(TeamsListLogic.findBotDm([try XCTUnwrap(all["bd2"]), try XCTUnwrap(all["bd1"])], uid: uid, sharedBotId: "sharedbot_helper")?.id, botDm)
        XCTAssertEqual(TeamsListLogic.findDm([try XCTUnwrap(all["dm1"]), try XCTUnwrap(all["grp"])], uid: uid, peerUid: "u2")?.id, dm)
        XCTAssertNil(TeamsListLogic.findDm([try XCTUnwrap(all["dm1"])], uid: uid, peerUid: "u3"))
        XCTAssertTrue(finds.keys.contains("dmMissing"), "the vector states the missing case")
    }

    func testPreviews() throws {
        let spec = try XCTUnwrap(MobileVectors.section("previews") as? [String: Any])
        let names = try XCTUnwrap(spec["authorNames"] as? [String: String])
        let cases = try XCTUnwrap(spec["cases"] as? [[String: Any]])
        XCTAssertEqual(cases.count, 11)
        for row in cases {
            var pointer: TeamLastMessage?
            if let last = row["lastMessage"] as? [String: Any] {
                let author = try XCTUnwrap(last["author"] as? [String: Any])
                pointer = TeamLastMessage(
                    id: try XCTUnwrap(last["id"] as? String), seq: try XCTUnwrap(last["seq"] as? Int),
                    atMs: (last["at"] as? String).map(Self.instant), authorKind: try XCTUnwrap(author["kind"] as? String),
                    authorUid: author["uid"] as? String ?? "", authorSharedBotId: author["sharedBotId"] as? String,
                    preview: try XCTUnwrap(last["preview"] as? String), recalled: last["recalled"] as? Bool ?? false)
            }
            let value = TeamMessageConversation(id: "x", kind: try XCTUnwrap(row["kind"] as? String), title: "", participants: ["user:u1", "user:u2"],
                                                lastSeq: 1, lastChangeSeq: 1, lastReadSeq: 0, lastMessage: pointer)
            let text = TeamsListLogic.conversationPreview(value, uid: uid, strings: strings(zh: row["zh"] as? Bool ?? false)) { last in
                last.authorKind == "shared_bot" ? (names["bot"] ?? "") : (names[last.authorUid] ?? last.authorUid)
            }
            XCTAssertEqual(text, row["expect"] as? String, row["name"] as? String ?? "")
        }
    }

    func testSearch() throws {
        let rows = try MobileVectors.rows("search")
        XCTAssertEqual(rows.count, 7)
        for row in rows {
            XCTAssertEqual(TeamsListLogic.conversationMatches(label: try XCTUnwrap(row["label"] as? String), query: try XCTUnwrap(row["query"] as? String)), row["expect"] as? Bool, "\(row)")
        }
    }

    // MARK: beyond the vectors

    func testTiesFallToLastSeqThenNumericTitleThenId() {
        func channel(_ id: String, _ title: String, seq: Int = 1, at: Int64? = nil) -> TeamMessageConversation {
            TeamMessageConversation(id: id, kind: "channel", title: title, participants: [], lastSeq: seq, lastChangeSeq: 0, lastReadSeq: 0,
                                    lastMessage: at.map { TeamLastMessage(id: "m", seq: seq, atMs: $0, authorKind: "user", authorUid: "u2", authorSharedBotId: nil, preview: "", recalled: false) })
        }
        XCTAssertLessThan(TeamsListLogic.compareConversations(channel("a", "x", at: 2), channel("b", "x", at: 1)), 0, "newer first")
        XCTAssertGreaterThan(TeamsListLogic.compareConversations(channel("a", "x"), channel("b", "x", at: 1)), 0, "a row with a time comes first")
        XCTAssertLessThan(TeamsListLogic.compareConversations(channel("a", "x", seq: 5), channel("b", "x", seq: 4)), 0)
        XCTAssertLessThan(TeamsListLogic.compareConversations(channel("a", "room 2"), channel("b", "room 10")), 0)
        XCTAssertLessThan(TeamsListLogic.compareConversations(channel("a", "Alpha"), channel("b", "beta")), 0, "case-insensitive")
        XCTAssertLessThan(TeamsListLogic.compareConversations(channel("a", "x"), channel("b", "x")), 0)
        XCTAssertEqual(TeamsListLogic.compareConversations(channel("a", "x"), channel("a", "x")), 0)
    }

    func testAnEmptyParticipantListIsTheViewersOwnForTheBotDmOrder() {
        func botDm(_ id: String, _ participants: [String], at: Int64) -> TeamMessageConversation {
            TeamMessageConversation(id: id, kind: "bot_dm", title: "", participants: participants, lastSeq: 1, lastChangeSeq: 0, lastReadSeq: 0,
                                    lastMessage: TeamLastMessage(id: "m", seq: 1, atMs: at, authorKind: "user", authorUid: "u2", authorSharedBotId: nil, preview: "", recalled: false))
        }
        let result = TeamsListLogic.groupConversations([botDm("other", ["user:u9", "sharedbot_k"], at: 9), botDm("mine", ["user:u1", "sharedbot_k"], at: 1), botDm("unlisted", [], at: 5)], uid: uid)
        XCTAssertEqual(result.botDirect.map(\.id), ["unlisted", "mine", "other"])
    }

    func testBotLabelsFallBackAndTheEffectiveBotKeyIsTheFirstBotParticipant() throws {
        let bots = try bots(), members = try members()
        // No sharedBotId field: the first "bot:" participant is the Bot (Desktop `sharedBotIdOf`).
        let dm = TeamMessageConversation(id: "x", kind: "bot_dm", title: "Stored", participants: ["user:u1", "shared_bot:sharedbot_helper"], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
        XCTAssertEqual(dm.effectiveSharedBotId, "sharedbot_helper")
        XCTAssertEqual(TeamsListLogic.conversationLabel(dm, members: members, uid: uid, bots: bots, strings: .en), "Helper")
        // No card and no title: the word "Bot".
        let bare = TeamMessageConversation(id: "x", kind: "bot_dm", title: " ", participants: ["user:u1", "shared_bot:sharedbot_zz"], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
        XCTAssertEqual(TeamsListLogic.conversationLabel(bare, members: members, uid: uid, bots: bots, strings: .en), "Bot")
        // The owner reading two people's DM names all of them, with the Chinese separator in Chinese.
        let reading = TeamMessageConversation(id: "x", kind: "bot_dm", title: "", participants: ["user:u2", "user:u3", "shared_bot:sharedbot_helper"], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
        XCTAssertEqual(TeamsListLogic.conversationLabel(reading, members: members, uid: uid, bots: bots, strings: .zh), "Helper · Ann、Bob")
    }

    func testAGeneratedTitleIsRecognisedWhateverTheOrderAndSeparator() throws {
        let members = try members()
        func label(_ title: String, strings: TeamsListStrings = .en) -> String {
            let group = TeamMessageConversation(id: "x", kind: "group", title: title, participants: ["user:u1", "user:u2", "user:u3"], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
            return TeamsListLogic.conversationLabel(group, members: members, uid: uid, bots: [], strings: strings)
        }
        XCTAssertEqual(label("Bob, Ann, Me"), "Ann, Bob", "any order")
        XCTAssertEqual(label("Me、Ann、Bob", strings: .zh), "Ann、Bob", "the Chinese enumeration comma")
        XCTAssertEqual(label("Me, Ann"), "Me, Ann", "a subset of the names is a chosen title")
        XCTAssertEqual(label("Me, Ann, Bob, Bob"), "Me, Ann, Bob, Bob", "more parts than people")
    }

    func testParticipantsBeforeTheCardLoadsUseTheKeyAndTheWordBot() throws {
        let dm = TeamMessageConversation(id: "x", kind: "bot_dm", title: "", participants: ["user:u1", "shared_bot:sharedbot_ghost"], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
        let result = TeamsListLogic.conversationParticipants(dm, members: try members(), bots: [], uid: uid, strings: .en)
        XCTAssertEqual(result.map(\.id), ["sharedbot_ghost", "u1"])
        XCTAssertEqual(result.map(\.name), ["Bot", "Me"])
        XCTAssertEqual(result.map(\.isSelf), [false, true])
    }

    func testThePreviewNamesTheAuthorOnlyInGroupsAndChannelsWithACallback() {
        let pointer = TeamLastMessage(id: "m", seq: 1, atMs: 1, authorKind: "user", authorUid: "u2", authorSharedBotId: nil, preview: "hello", recalled: false)
        func preview(kind: String, author: ((TeamLastMessage) -> String)?) -> String {
            let value = TeamMessageConversation(id: "x", kind: kind, title: "", participants: [], lastSeq: 1, lastChangeSeq: 0, lastReadSeq: 0, lastMessage: pointer)
            return TeamsListLogic.conversationPreview(value, uid: uid, strings: .en, author: author)
        }
        XCTAssertEqual(preview(kind: "group", author: nil), "hello")
        XCTAssertEqual(preview(kind: "bot_dm", author: { _ in "Ann" }), "hello")
        XCTAssertEqual(preview(kind: "channel", author: { _ in "Ann" }), "Ann: hello")
    }

    /// A shared Bot's last message writes Markdown, so its preview is plain text; a person's stays exactly as stored.
    func testOnlyABotsPreviewIsPlainText() {
        func last(_ kind: String, _ preview: String, recalled: Bool = false) -> TeamLastMessage {
            TeamLastMessage(id: "m", seq: 1, atMs: 1, authorKind: kind, authorUid: kind == "user" ? "u2" : "", authorSharedBotId: kind == "shared_bot" ? "b" : nil, preview: preview, recalled: recalled)
        }
        func preview(_ pointer: TeamLastMessage, author: ((TeamLastMessage) -> String)? = nil) -> String {
            TeamsListLogic.pointerPreview(pointer, uid: uid, strings: .en, author: author)
        }
        let markdown = "# QA Follow-Up Check\n\n**Goal: close the open items.**\n\n- Scope: defects"
        XCTAssertEqual(preview(last("shared_bot", markdown)), "QA Follow-Up Check Goal: close the open items. Scope: defects")
        XCTAssertEqual(preview(last("shared_bot", "# QA Follow-Up Check"), author: { _ in "Fixture Bot" }), "Fixture Bot: QA Follow-Up Check")
        XCTAssertEqual(preview(last("user", "2 * 3 * 4 # keep")), "2 * 3 * 4 # keep")
        XCTAssertEqual(preview(last("user", markdown)), markdown)
        XCTAssertEqual(preview(last("shared_bot", "---")), TeamsListStrings.en.nonText)
        XCTAssertEqual(preview(last("shared_bot", "")), TeamsListStrings.en.nonText)
        XCTAssertEqual(preview(last("shared_bot", "**x**", recalled: true)), TeamsListStrings.en.recalled)
        let group = TeamMessageConversation(id: "g", kind: "group", title: "", participants: [], lastSeq: 1, lastChangeSeq: 0, lastReadSeq: 0, lastMessage: last("shared_bot", "**G"))
        XCTAssertEqual(TeamsListLogic.conversationPreview(group, uid: uid, strings: .en) { _ in "Bot" }, "Bot: G")
    }
}
