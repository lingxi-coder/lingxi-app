import Foundation
import XCTest
@testable import LingxiCode

/// The replies of a Bot that wait for its maintainer (Bot v3 B6b): the replies as the service writes them, what the maintainer may send, the commands, whose replies a person
/// decides on and where they are shown, the list and the decisions on it, the time, and the words in every language. Foundation-only, so it runs without the app: the screens
/// are `TeamHeldViews.swift`.
final class TeamHeldRepliesTests: XCTestCase {
    private let viewer = TeamMemberRef(uid: "alice", generation: 1)
    private let minute = 60_000.0
    private let start = 1_800_000_000_000.0

    /// One held reply as the service writes it; `n` is its number, newer replies have the larger one.
    private func wire(_ n: Int = 1, unasked: Bool = false, extra: [String: Any] = [:]) -> [String: Any] {
        var value: [String: Any] = ["taskId": String(format: "task-%04d", n), "conversationId": "chat_0001", "messageId": "message-\(n)", "requester": ["uid": "bob", "generation": 1],
                                    "unasked": unasked, "heldAt": start + Double(n) * 1000, "expiresAt": start + Double(n) * 1000 + 10 * minute, "text": "Dinner at 7 works for me.",
                                    "conversationTitle": "Planning"]
        value.merge(extra) { _, new in new }
        return value
    }
    private func decode<T: Decodable>(_ type: T.Type, _ value: Any) throws -> T { try JSONDecoder().decode(T.self, from: JSONSerialization.data(withJSONObject: value)) }
    private func reply(_ n: Int = 1, unasked: Bool = false, bot: String = "sharedbot_01", extra: [String: Any] = [:]) throws -> TeamHeldReply {
        var value = try decode(TeamHeldReply.self, wire(n, unasked: unasked, extra: extra))
        value.sharedBotId = bot
        return value
    }
    private func card(_ id: String, maintainer: String = "alice", generation: Int = 1, daily: Int? = nil) throws -> TeamBotCard {
        var value: [String: Any] = ["sharedBotId": id, "name": id, "description": "", "maintainer": ["uid": maintainer, "generation": generation], "revision": 1, "state": "published",
                                    "permitted": true, "ready": true, "onlineUntil": 0, "usePolicy": ["mode": "all"], "quotas": ["team": 1, "member": 1, "bot": 1]]
        if let daily { value["autoReply"] = ["daily": daily] }
        return try decode(TeamBotCard.self, value)
    }
    private func chat(_ participants: [String], kind: String = "group") -> TeamMessageConversation {
        TeamMessageConversation(id: "chat_0001", kind: kind, title: "Trip", participants: participants, lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
    }
    private func row(_ n: Int, unasked: Bool = false, bot: String = "sharedbot_01", chat: String = "chat_0001", title: String? = "Planning") throws -> TeamHeldRow {
        var extra: [String: Any] = ["conversationId": chat]
        extra["conversationTitle"] = title ?? NSNull()
        return TeamHeldRow(reply: try reply(n, unasked: unasked, bot: bot, extra: extra))
    }

    // MARK: The replies as the service writes them

    func testAReplyIsReadAsTheServiceWroteIt() throws {
        let full = try decode(TeamHeldReply.self, wire(7, unasked: true, extra: ["document": ["title": "Dinner plan", "body": "1. Book a table"]]))
        XCTAssertEqual(full, TeamHeldReply(taskId: "task-0007", conversationId: "chat_0001", messageId: "message-7", requester: TeamMemberRef(uid: "bob", generation: 1), unasked: true,
                                           heldAt: start + 7000, expiresAt: start + 7000 + 10 * minute, text: "Dinner at 7 works for me.",
                                           document: TeamHeldDocument(title: "Dinner plan", body: "1. Book a table"), conversationTitle: "Planning"))
        XCTAssertEqual(full.sharedBotId, "", "the service does not say whose reply it is: the screen that asked tags it")
        var bare = wire(8); bare.removeValue(forKey: "conversationTitle")
        let plain = try decode(TeamHeldReply.self, bare)
        XCTAssertNil(plain.document); XCTAssertNil(plain.conversationTitle); XCTAssertFalse(plain.unasked)
        XCTAssertNil(try decode(TeamHeldReply.self, wire(9, extra: ["conversationTitle": ""])).conversationTitle, "an empty title is no title")
        XCTAssertNil(try decode(TeamHeldReply.self, wire(9, extra: ["conversationTitle": 7])).conversationTitle, "a title that is not text is no title; the reply is still one")
        XCTAssertEqual(try decode(TeamHeldReply.self, wire(10, extra: ["document": ["title": "T", "body": ""]])).document?.body, "", "a document may have no body")
    }

    func testAReplyThisVersionCannotReadIsNotOneAndAReplyIsNeverHalfShown() throws {
        let unreadable: [[String: Any]] = [
            wire(1, extra: ["taskId": ""]), wire(1, extra: ["conversationId": ""]), wire(1, extra: ["messageId": ""]), wire(1, extra: ["text": ""]), wire(1, extra: ["text": 5]),
            wire(1, extra: ["unasked": "no"]), wire(1, extra: ["unasked": NSNull()]), wire(1, extra: ["heldAt": "1"]), wire(1, extra: ["expiresAt": 9e15]), wire(1, extra: ["heldAt": NSNull()]),
            wire(1, extra: ["requester": ["uid": "", "generation": 1]]), wire(1, extra: ["requester": ["uid": "bob", "generation": -1]]), wire(1, extra: ["requester": "bob"]),
            // The maintainer would be letting go of something they did not see.
            wire(1, extra: ["document": NSNull()]), wire(1, extra: ["document": "text"]), wire(1, extra: ["document": ["title": "", "body": "x"]]), wire(1, extra: ["document": ["title": "T"]]),
            wire(1, extra: ["document": ["title": "T", "body": 5]]), ["nonsense": true],
        ]
        for value in unreadable { XCTAssertThrowsError(try decode(TeamHeldReply.self, value), "\(value)") }
        var missing = wire(1); missing.removeValue(forKey: "heldAt")
        XCTAssertThrowsError(try decode(TeamHeldReply.self, missing))
    }

    func testTheAnswerKeepsTheRepliesItCanReadAndAnAnswerThatIsNoListIsNotAnEmptyOne() throws {
        let answer = try decode(TeamHeldAnswer.self, ["replies": [wire(3), ["nonsense": true], 7, NSNull(), wire(1, extra: ["text": ""]), wire(2, unasked: true)]])
        XCTAssertEqual(answer.replies.map(\.taskId), ["task-0003", "task-0002"], "the replies around one that cannot be read are still shown")
        XCTAssertEqual(try decode(TeamHeldAnswer.self, ["replies": [Any]()]).replies, [])
        let nonsense: [Any] = [["nope": true] as [String: Any], ["replies": "x"] as [String: Any], ["replies": NSNull()] as [String: Any], [String: Any]()]
        for value in nonsense { XCTAssertThrowsError(try decode(TeamHeldAnswer.self, value), "\(value)") }
    }

    func testADecisionIsReadAsTheServiceCameToItAndAnAnswerThatSaysNothingOfTheTaskIsNone() throws {
        XCTAssertEqual(try decode(TeamHeldDecision.self, ["taskId": "task-0001", "state": "completed", "replyId": "reply-0001", "artifactId": NSNull()]),
                       TeamHeldDecision(taskId: "task-0001", state: "completed", replyId: "reply-0001"))
        XCTAssertEqual(try decode(TeamHeldDecision.self, ["taskId": "task-0001", "state": "declined", "declineReason": "approval_rejected"]).declineReason, "approval_rejected")
        XCTAssertEqual(try decode(TeamHeldDecision.self, ["taskId": "task-0001", "state": "declined", "declineReason": "revoked"]).declineReason, "revoked")
        XCTAssertNil(try decode(TeamHeldDecision.self, ["taskId": "task-0001", "state": "declined", "declineReason": "a_new_reason"]).declineReason)
        XCTAssertEqual(try decode(TeamHeldDecision.self, ["taskId": "task-0001", "state": "completed", "artifactId": "artifact-1", "replyId": ""]).artifactId, "artifact-1")
        let nonsense: [Any] = [[String: Any](), ["taskId": "task-0001"] as [String: Any], ["state": "completed"] as [String: Any], ["taskId": "", "state": "completed"] as [String: Any],
                               ["taskId": "task-0001", "state": ""] as [String: Any]]
        for value in nonsense { XCTAssertThrowsError(try decode(TeamHeldDecision.self, value), "\(value)") }
        XCTAssertThrowsError(try JSONDecoder().decode(TeamHeldDecision.self, from: Data("null".utf8)), "an answer that is nothing")
    }

    // MARK: What may be sent

    func testTheTextTheMaintainerSendsIsTrimmedAtLeastACharacterAndEightKilobytes() {
        XCTAssertEqual(TeamHeldRules.edit(draft: "Dinner at 7.", edited: "  Dinner at 8.  "), .send("Dinner at 8."), "trimmed")
        XCTAssertEqual(TeamHeldRules.edit(draft: "Dinner at 7.", edited: "Dinner at 7."), .send(nil), "nothing changed: the reply goes as the Bot made it, and is not recorded as changed")
        XCTAssertEqual(TeamHeldRules.edit(draft: "Dinner at 7.\n", edited: "\n  Dinner at 7.  "), .send(nil), "only spaces and lines at the ends changed")
        for edited in ["", "   ", "\n\t "] { XCTAssertEqual(TeamHeldRules.edit(draft: "Dinner", edited: edited), .empty, "\(edited.debugDescription)") }
        XCTAssertEqual(TeamHeldRules.edit(draft: "x", edited: String(repeating: "€", count: 2731)), .tooLong, "2731 euro signs are 8193 bytes")
        XCTAssertEqual(TeamHeldRules.edit(draft: "x", edited: String(repeating: "€", count: 2730)), .send(String(repeating: "€", count: 2730)), "2730 are 8190")
        XCTAssertEqual(TeamHeldRules.edit(draft: "x", edited: "  " + String(repeating: "x", count: 8192) + "  "), .send(String(repeating: "x", count: 8192)), "the spaces around are not counted")
        XCTAssertEqual(TeamHeldRules.edit(draft: "x", edited: String(repeating: "x", count: 8193)), .tooLong)
        XCTAssertEqual(TeamExecutionContract.heldReplyBytes, 8192)
    }

    func testTheCommandsAreTheMemberCommandsTheServiceTakes() throws {
        func command(_ action: String, _ payload: [String: Any]) -> [String: Any] {
            ["schemaVersion": 1, "commandId": "command_0001", "teamId": "team_0001", "generation": 1, "action": action, "payload": payload]
        }
        let bot = "sharedbot_01", task = "task-0001", chatId = "chat_0001"
        XCTAssertEqual(TeamHeldRules.listPayload(sharedBotId: bot) as NSDictionary, ["sharedBotId": bot] as NSDictionary, "without a Chat, every Chat")
        XCTAssertEqual(TeamHeldRules.listPayload(sharedBotId: bot, conversationId: chatId) as NSDictionary, ["sharedBotId": bot, "conversationId": chatId] as NSDictionary)
        XCTAssertEqual(TeamHeldRules.approvePayload(taskId: task) as NSDictionary, ["taskId": task] as NSDictionary, "as the Bot made it: no text")
        XCTAssertEqual(TeamHeldRules.approvePayload(taskId: task, text: "Dinner at 8.") as NSDictionary, ["taskId": task, "text": "Dinner at 8."] as NSDictionary)
        XCTAssertEqual(TeamHeldRules.rejectPayload(taskId: task) as NSDictionary, ["taskId": task] as NSDictionary)
        for payload in [TeamHeldRules.listPayload(sharedBotId: bot), TeamHeldRules.listPayload(sharedBotId: bot, conversationId: chatId)] { XCTAssertNoThrow(try TeamExecutionContract.validate(command("listHeldReplies", payload))) }
        for payload in [TeamHeldRules.approvePayload(taskId: task), TeamHeldRules.approvePayload(taskId: task, text: "Dinner at 8."), TeamHeldRules.approvePayload(taskId: task, text: String(repeating: "€", count: 2730))] {
            XCTAssertNoThrow(try TeamExecutionContract.validate(command("approveReply", payload)))
        }
        XCTAssertNoThrow(try TeamExecutionContract.validate(command("rejectReply", TeamHeldRules.rejectPayload(taskId: task))))
        // What the service would refuse is refused before it is sent.
        let tooLong = TeamHeldRules.approvePayload(taskId: task, text: String(repeating: "€", count: 2731))
        let refused: [(String, [String: Any])] = [
            ("listHeldReplies", [:]), ("listHeldReplies", ["conversationId": chatId]), ("listHeldReplies", ["sharedBotId": "x"]), ("listHeldReplies", ["sharedBotId": bot, "conversationId": "x"]),
            ("listHeldReplies", ["sharedBotId": bot, "limit": 5]), ("approveReply", [:]), ("approveReply", ["taskId": "x"]), ("approveReply", ["taskId": task, "text": ""]),
            ("approveReply", ["taskId": task, "text": "  \n "]), ("approveReply", tooLong), ("approveReply", ["taskId": task, "text": 5]), ("approveReply", ["taskId": task, "commandId": "command_0002"]),
            ("rejectReply", [:]), ("rejectReply", ["taskId": "x"]), ("rejectReply", ["taskId": task, "text": "no"]),
            ("holdReply", ["taskId": task, "text": "Dinner"]),
        ]
        for (action, payload) in refused { XCTAssertThrowsError(try TeamExecutionContract.validate(command(action, payload)), "\(action) \(payload)") }
    }

    // MARK: Whose replies, and where they are shown

    func testTheBotsOfAChatWhoseRepliesAPersonDecidesOnAreTheOnesTheyMaintainThatAreInIt() throws {
        let cards = [try card("sharedbot_mine"), try card("sharedbot_auto", daily: 5), try card("sharedbot_theirs", maintainer: "owner"), try card("sharedbot_older", generation: 0), try card("sharedbot_elsewhere")]
        let participants = ["user:bob", "shared_bot:sharedbot_mine", "shared_bot:sharedbot_auto", "shared_bot:sharedbot_theirs", "shared_bot:sharedbot_older"]
        func ids(_ conversation: TeamMessageConversation, _ who: TeamMemberRef? = nil) -> [String] { TeamHeldRules.bots(conversation, cards: cards, viewer: who ?? viewer).map(\.sharedBotId) }
        XCTAssertEqual(ids(chat(participants)), ["sharedbot_mine", "sharedbot_auto"], "a Bot that holds replies need not answer on its own; one of someone else, of an older membership or outside the Chat is not theirs here")
        XCTAssertEqual(ids(chat(participants), TeamMemberRef(uid: "bob", generation: 1)), [], "a person who maintains none has none")
        XCTAssertEqual(ids(chat(["user:bob"])), [], "a Chat with no Bot in it has none")
        XCTAssertEqual(ids(chat(["user:bob", "shared_bot:sharedbot_mine"], kind: "bot_dm")), ["sharedbot_mine"], "taking part in the Chat is not needed")
    }

    func testAReplySitsWithItsTaskOneNobodyAskedForIsARowOfItsOwnOldestFirstAndAnAskedOneWaitsForItsTask() throws {
        let rows = [try row(5, unasked: true), try row(4), try row(3), try row(2, unasked: true), try row(1, unasked: true)]
        let placed = TeamHeldRules.place(rows, listed: ["task-0004", "task-0001"])
        XCTAssertEqual(Set(placed.byTask.keys), ["task-0004", "task-0001"])
        XCTAssertEqual(placed.orphans.map(\.reply.taskId), ["task-0002", "task-0005"], "the stream reads oldest first; a reply whose task is not listed yet is not a row of its own unless nobody asked for it")
        let none = TeamHeldRules.place([], listed: [])
        XCTAssertTrue(none.byTask.isEmpty); XCTAssertTrue(none.orphans.isEmpty)
    }

    func testTheCardOfABotCountsTheRepliesThatStillWaitAndTheListGroupsThemByChat() throws {
        var ended = try row(4); ended.ended = "REPLY_APPROVED"
        let list = TeamHeldList(status: .ready, rows: [try row(6, chat: "chat_0002", title: "Trip"), try row(5), ended, try row(2, chat: "chat_0003", title: nil), try row(1)], error: nil, now: start + 3500 + 10 * minute)
        XCTAssertEqual(TeamHeldRules.waiting(list), 2, "the ones whose time is up and the one that ended do not wait: only the two newest are still within their ten minutes")
        XCTAssertEqual(TeamHeldRules.waiting(TeamHeldList(status: .ready, rows: list.rows, error: nil, now: start)), 4, "the ended one does not wait however much time is left")
        XCTAssertEqual(TeamHeldRules.waiting(TeamHeldList()), 0)
        let chats = TeamHeldRules.byChat(list.rows)
        XCTAssertEqual(chats.map(\.id), ["chat_0002", "chat_0001", "chat_0003"], "the Chat of the newest reply first")
        XCTAssertEqual(chats.map(\.title), ["Trip", "Planning", nil], "a Chat the service gave no title for has none: the screen names it")
        XCTAssertEqual(chats[1].rows.map(\.reply.taskId), ["task-0005", "task-0004", "task-0001"], "newest first within a Chat")
        XCTAssertEqual(TeamHeldRules.byChat([]), [])
        XCTAssertEqual(try row(1).over(at: start + 1000 + 10 * minute), "REPLY_EXPIRED", "the end of its time is the end of it, even before the service is asked again")
        XCTAssertNil(try row(1).over(at: start + 1000 + 10 * minute - 1))
        XCTAssertEqual(ended.over(at: start), "REPLY_APPROVED", "what the service said stands")
    }

    func testTheRepliesOfSeveralBotsAreTaggedWithTheirBotNewestFirstAndABotThatIsNotTheirsAddsNothing() async throws {
        let replies: [String: [TeamHeldReply]] = ["bot-a": [try reply(1), try reply(3)], "bot-b": [try reply(2)]]
        let asked = Collected<String>()
        let read = try await TeamHeldRules.read(["bot-a", "bot-b", "bot-c"]) { id in
            asked.add(id)
            if id == "bot-c" { throw TeamMessageFailure(code: "MAINTAINER_REQUIRED") }
            return replies[id] ?? []
        }
        XCTAssertEqual(read.map(\.taskId), ["task-0003", "task-0002", "task-0001"]); XCTAssertEqual(read.map(\.sharedBotId), ["bot-a", "bot-b", "bot-a"])
        XCTAssertEqual(Set(asked.all), ["bot-a", "bot-b", "bot-c"])
        for (bots, code) in [(["bot-c"], "MAINTAINER_REQUIRED"), ([], "MAINTAINER_REQUIRED"), (["bot-a", "bot-d"], "SERVICE_UNAVAILABLE")] as [([String], String)] {
            do {
                _ = try await TeamHeldRules.read(bots) { id in
                    if id == "bot-d" { throw TeamMessageFailure(code: "SERVICE_UNAVAILABLE") }
                    if id == "bot-c" { throw TeamMessageFailure(code: "MAINTAINER_REQUIRED") }
                    return []
                }
                XCTFail("\(bots) should not read")
            } catch { XCTAssertEqual(TeamHeldRules.code(error), code, "\(bots): every Bot refusing is the person not being a maintainer, any other failure is the failure") }
        }
        let first = Collected<String>()
        do {
            _ = try await TeamHeldRules.read(["bot-x", "bot-y"]) { id in first.add(id); throw TeamMessageFailure(code: id == "bot-x" ? "INVALID_DATA" : "SERVICE_UNAVAILABLE") }
            XCTFail("should not read")
        } catch { XCTAssertEqual(TeamHeldRules.code(error), "INVALID_DATA", "of the Bots that failed, the first one's failure is told") }
        XCTAssertEqual(TeamHeldRules.code(URLError(.notConnectedToInternet)), "SERVICE_UNAVAILABLE", "an error that is not the service's is the service being unavailable")
    }

    // MARK: The list

    /// Lets the other tasks of the main actor run until `done`, and fails the test instead of waiting for ever.
    @MainActor private func spin(_ done: () -> Bool, file: StaticString = #filePath, line: UInt = #line) async {
        for _ in 0..<20_000 { if done() { return }; await Task.yield() }
        XCTFail("what the test waited for did not happen", file: file, line: line)
    }

    /// What a screen's board reads and decides, scripted by the test.
    private final class Service: @unchecked Sendable {
        private let lock = NSLock()
        private var _replies: [TeamHeldReply]
        private var _reads = 0, _approvals: [String] = [], _rejections: [String] = []
        var failure: String?
        var approveOutcome: (@Sendable (String) -> Result<TeamHeldDecision, TeamMessageFailure>)?
        var rejectOutcome: (@Sendable (String) -> Result<TeamHeldDecision, TeamMessageFailure>)?
        init(_ replies: [TeamHeldReply]) { _replies = replies }
        var replies: [TeamHeldReply] { get { lock.withLock { _replies } } set { lock.withLock { _replies = newValue } } }
        var reads: Int { lock.withLock { _reads } }
        /// `taskId|text|commandId` of every approval sent, `taskId|commandId` of every rejection.
        var approvals: [String] { lock.withLock { _approvals } }
        var rejections: [String] { lock.withLock { _rejections } }
        func read() throws -> [TeamHeldReply] {
            try lock.withLock { _reads += 1; if let failure { throw TeamMessageFailure(code: failure) }; return _replies }
        }
        func approve(_ taskId: String, _ text: String?, _ commandId: String) throws -> TeamHeldDecision {
            lock.withLock { _approvals.append("\(taskId)|\(text ?? "-")|\(commandId)") }
            let outcome = approveOutcome?(taskId) ?? .success(TeamHeldDecision(taskId: taskId, state: "completed", replyId: "reply-0001"))
            return try lock.withLock { if case .success = outcome { _replies.removeAll { $0.taskId == taskId } }; return try outcome.get() }
        }
        func reject(_ taskId: String, _ commandId: String) throws -> TeamHeldDecision {
            lock.withLock { _rejections.append("\(taskId)|\(commandId)") }
            let outcome = rejectOutcome?(taskId) ?? .success(TeamHeldDecision(taskId: taskId, state: "declined", declineReason: "approval_rejected"))
            return try lock.withLock { if case .success = outcome { _replies.removeAll { $0.taskId == taskId } }; return try outcome.get() }
        }
    }
    /// A board over the service; `now` is the time it keeps. `lists` is every list it published.
    @MainActor private func board(_ service: Service, now: TestClock = TestClock(), sleep: @escaping @Sendable (Double) async throws -> Void = { _ in throw CancellationError() }) -> (board: TeamHeldBoard, lists: Collected<TeamHeldList>) {
        let lists = Collected<TeamHeldList>(), ids = Counter()
        let board = TeamHeldBoard(read: { try service.read() },
                                  decide: TeamHeldDecide(approve: { try service.approve($0, $1, $2) }, reject: { try service.reject($0, $1) }),
                                  changed: { lists.add($0) }, clock: { now.value }, newId: { "command-\(ids.next() + 1)" }, sleep: sleep)
        return (board, lists)
    }

    @MainActor func testTheListIsReadWhenAskedNewestFirstAndAReadThatFailsKeepsWhatWasRead() async throws {
        let service = Service([try reply(1), try reply(5)])
        let (list, lists) = board(service)
        XCTAssertEqual(list.list.status, .loading); XCTAssertTrue(list.list.rows.isEmpty)
        await list.load()
        XCTAssertEqual(list.list.status, .ready); XCTAssertEqual(list.list.rows.map(\.reply.taskId), ["task-0005", "task-0001"]); XCTAssertEqual(lists.all.count, 1)
        XCTAssertTrue(list.list.rows.allSatisfy { $0.busy == nil && $0.error == nil && $0.ended == nil })
        service.failure = "SERVICE_UNAVAILABLE"
        await list.load()
        XCTAssertEqual(list.list.status, .failed); XCTAssertEqual(list.list.error, "SERVICE_UNAVAILABLE"); XCTAssertEqual(list.list.rows.count, 2, "what was read stays")
        service.failure = nil
        await list.load()
        XCTAssertEqual(list.list.status, .ready); XCTAssertNil(list.list.error)
        service.failure = "MAINTAINER_REQUIRED"
        await list.load()
        XCTAssertEqual(list.list.status, .denied); XCTAssertTrue(list.list.rows.isEmpty, "the service says none of it is theirs: nothing of it stays")
    }

    @MainActor func testReadsDoNotPileUpOneIsMadeAtATimeAndRequestsThatComeMeanwhileAreAnsweredByOneMore() async throws {
        let service = Service([try reply(1)]), gates = [AsyncGate(), AsyncGate()], arrivals = Counter()
        let pending = TeamHeldBoard(read: { let mine = arrivals.next(); if mine < 2 { await gates[mine].wait() }; return try service.read() },
                                    decide: TeamHeldDecide(approve: { try service.approve($0, $1, $2) }, reject: { try service.reject($0, $1) }), changed: { _ in })
        let first = Task { @MainActor in await pending.load() }
        await spin { arrivals.count >= 1 }
        // The requests that come while the read is on its way.
        let second = Task { @MainActor in await pending.load() }, third = Task { @MainActor in await pending.load() }
        for _ in 0..<10 { await Task.yield() }
        XCTAssertEqual(arrivals.count, 1, "one read is on its way")
        gates[0].open()
        await spin { arrivals.count >= 2 }
        gates[1].open()
        await first.value; await second.value; await third.value
        XCTAssertEqual(arrivals.count, 2, "and one more followed it, for all the requests that came meanwhile")
        XCTAssertEqual(pending.list.status, .ready)
        // A read that answers after the board was closed changes nothing, and a closed board reads no more.
        let stop = AsyncGate(), seen = Collected<TeamHeldList>(), reads = Counter()
        let one = try reply(1)
        let closing = TeamHeldBoard(read: { _ = reads.next(); await stop.wait(); return [one] }, decide: TeamHeldDecide(approve: { try service.approve($0, $1, $2) }, reject: { try service.reject($0, $1) }), changed: { seen.add($0) })
        let late = Task { @MainActor in await closing.load() }
        await spin { reads.count >= 1 }
        closing.close(); stop.open(); await late.value
        XCTAssertEqual(seen.all.count, 0, "nothing was shown after it was closed"); XCTAssertEqual(closing.list.status, .loading)
        await closing.load()
        XCTAssertEqual(reads.count, 1)
    }

    @MainActor func testApprovingSendsTheDecisionOnceARetryOfTheSameDecisionIsTheSameCommandAndWhatTheServiceTookLeavesTheList() async throws {
        let service = Service([try reply(1), try reply(2)])
        let (list, _) = board(service)
        await list.load()
        service.approveOutcome = { _ in .failure(TeamMessageFailure(code: "SERVICE_UNAVAILABLE")) }
        var taken = await list.approve("task-0001")
        XCTAssertFalse(taken)
        XCTAssertEqual(list.list.row("task-0001")?.error, "SERVICE_UNAVAILABLE"); XCTAssertNil(list.list.row("task-0001")?.busy, "it can be tried again")
        taken = await list.approve("task-0001"); XCTAssertFalse(taken)
        taken = await list.approve("task-0001", text: "Dinner at 8."); XCTAssertFalse(taken)
        taken = await list.approve("task-0001", text: "Dinner at 8."); XCTAssertFalse(taken)
        service.approveOutcome = nil
        taken = await list.approve("task-0001", text: "Dinner at 8."); XCTAssertTrue(taken)
        XCTAssertEqual(service.approvals, ["task-0001|-|command-1", "task-0001|-|command-1", "task-0001|Dinner at 8.|command-2", "task-0001|Dinner at 8.|command-2", "task-0001|Dinner at 8.|command-2"],
                       "a retry of the same decision is the same command, a different decision is a new one")
        XCTAssertEqual(list.list.rows.map(\.reply.taskId), ["task-0002"], "taken: it is gone")
        taken = await list.approve("task-0001"); XCTAssertFalse(taken, "a reply that is not on the list is not decided")
        XCTAssertEqual(service.approvals.count, 5)
        // A decision that is on its way is not sent again, whatever is pressed.
        let slow = AsyncGate(), arrivals = Counter()
        let blocked = TeamHeldBoard(read: { try service.read() }, decide: TeamHeldDecide(approve: { id, _, _ in _ = arrivals.next(); await slow.wait(); return TeamHeldDecision(taskId: id, state: "completed") }, reject: { try service.reject($0, $1) }), changed: { _ in })
        await blocked.load()
        let first = Task { @MainActor in await blocked.approve("task-0002") }
        await spin { arrivals.count >= 1 }
        XCTAssertEqual(blocked.list.row("task-0002")?.busy, .approve)
        let again = await blocked.approve("task-0002"), other = await blocked.reject("task-0002")
        XCTAssertFalse(again); XCTAssertFalse(other); XCTAssertEqual(arrivals.count, 1); XCTAssertTrue(service.rejections.isEmpty)
        slow.open()
        let done = await first.value
        XCTAssertTrue(done)
    }

    @MainActor func testARejectIsOneCommandTooAndDropsTheReply() async throws {
        let service = Service([try reply(1)])
        let (list, _) = board(service)
        await list.load()
        service.rejectOutcome = { _ in .failure(TeamMessageFailure(code: "SERVICE_UNAVAILABLE")) }
        var taken = await list.reject("task-0001"); XCTAssertFalse(taken)
        taken = await list.reject("task-0001"); XCTAssertFalse(taken)
        service.rejectOutcome = nil
        taken = await list.reject("task-0001"); XCTAssertTrue(taken)
        XCTAssertEqual(service.rejections, ["task-0001|command-1", "task-0001|command-1", "task-0001|command-1"])
        XCTAssertTrue(list.list.rows.isEmpty); XCTAssertTrue(service.approvals.isEmpty)
    }

    @MainActor func testAReplyTheServiceSaysIsNoLongerThereStaysOnScreenSayingSoUntilItIsClosedAndAnyOtherFailureLeavesItAsItWas() async throws {
        for code in ["REPLY_EXPIRED", "REPLY_SUPERSEDED", "REPLY_APPROVED", "REPLY_REJECTED", "REPLY_NOT_HELD", "TASK_REQUIRED", "MAINTAINER_REQUIRED"] {
            let service = Service([try reply(1)])
            let (list, _) = board(service)
            await list.load()
            service.approveOutcome = { _ in .failure(TeamMessageFailure(code: code)) }; service.replies = []
            var taken = await list.approve("task-0001")
            XCTAssertFalse(taken, code)
            for _ in 0..<5 { await Task.yield() }
            let row = try XCTUnwrap(list.list.row("task-0001"), code)
            XCTAssertEqual(row.ended, code, "it says what became of it, and the list that no longer has it does not take it away"); XCTAssertNil(row.busy); XCTAssertNil(row.error)
            taken = await list.reject("task-0001")
            XCTAssertFalse(taken, "nothing more is sent for it"); XCTAssertEqual(service.approvals.count, 1); XCTAssertTrue(service.rejections.isEmpty)
            list.dismiss("task-0001")
            XCTAssertTrue(list.list.rows.isEmpty, "\(code): read, and gone")
        }
        let service = Service([try reply(1)])
        let (list, _) = board(service)
        await list.load()
        list.dismiss("task-0001")
        XCTAssertEqual(list.list.rows.count, 1, "a reply that is still to be decided is not dismissed")
        service.approveOutcome = { _ in .failure(TeamMessageFailure(code: "INVALID_INPUT")) }
        _ = await list.approve("task-0001")
        XCTAssertEqual(list.list.row("task-0001")?.error, "INVALID_INPUT"); XCTAssertNil(list.list.row("task-0001")?.ended)
    }

    @MainActor func testAnApprovalTheServiceTookThatPostedNothingSaysSo() async throws {
        let service = Service([try reply(1)])
        let (list, _) = board(service)
        await list.load()
        service.approveOutcome = { id in .success(TeamHeldDecision(taskId: id, state: "declined", declineReason: "revoked")) }
        let taken = await list.approve("task-0001")
        XCTAssertTrue(taken, "the decision was taken")
        for _ in 0..<5 { await Task.yield() }
        XCTAssertEqual(list.list.row("task-0001")?.ended, TeamHeldRules.notPosted)
        list.dismiss("task-0001")
        XCTAssertTrue(list.list.rows.isEmpty)
    }

    @MainActor func testAReplyThatIsNoLongerListedGoesQuietlyUnlessItIsBeingDecidedOrItsTimeRanOutWhileItWasShown() async throws {
        let clock = TestClock(start + 1000)
        let service = Service([try reply(1, extra: ["expiresAt": start + 1000 + 5 * minute]), try reply(2, extra: ["expiresAt": start + 1000 + minute]), try reply(3, extra: ["expiresAt": start + 1000 + 8 * minute])])
        let slow = AsyncGate(), arrivals = Counter()
        let lists = Collected<TeamHeldList>()
        let list = TeamHeldBoard(read: { try service.read() }, decide: TeamHeldDecide(approve: { id, _, _ in _ = arrivals.next(); await slow.wait(); return TeamHeldDecision(taskId: id, state: "completed") }, reject: { try service.reject($0, $1) }),
                                 changed: { lists.add($0) }, clock: { clock.value })
        await list.load()
        let deciding = Task { @MainActor in await list.approve("task-0003") }
        await spin { arrivals.count >= 1 }
        clock.value += 2 * minute; service.replies = []
        await list.load()
        XCTAssertNil(list.list.row("task-0001"), "dropped for a newer message, or decided on another screen: it simply goes")
        XCTAssertEqual(list.list.row("task-0002")?.ended, "REPLY_EXPIRED", "its time ran out while it was shown: it says so")
        XCTAssertEqual(list.list.row("task-0003")?.busy, .approve, "a decision that is on its way keeps its reply on screen")
        slow.open(); _ = await deciding.value
        for _ in 0..<5 { await Task.yield() }
        XCTAssertEqual(list.list.rows.map(\.reply.taskId), ["task-0002"])
    }

    // MARK: The time

    @MainActor func testTheTimeMovesOnEvery30SecondsAndTheListIsReadAgainWhenTheFirstReplyRunsOut() async throws {
        let clock = TestClock(start)
        let service = Service([try reply(1, extra: ["expiresAt": start + 100_000]), try reply(2, extra: ["expiresAt": start + 400_000])])
        let waits = Collected<Double>()
        // Sleeping is the clock moving on; the sixth time the screen goes away.
        let (list, _) = board(service, now: clock, sleep: { seconds in
            waits.add(seconds)
            if waits.all.count > 5 { throw CancellationError() }
            clock.value += seconds * 1000
        })
        await list.load()
        XCTAssertEqual(TeamHeldRules.tick, 30); XCTAssertEqual(list.list.now, start)
        service.replies = service.replies.filter { $0.taskId != "task-0001" }
        await list.keepTime()
        XCTAssertEqual(waits.all, [30, 30, 30, 10.5, 30, 30], "every 30 s, and once sooner: the first reply runs out 10 s after the third tick, and is read 0.5 s later")
        XCTAssertEqual(service.reads, 2, "a tick reads nothing; the end of a reply is read for once")
        XCTAssertEqual(list.list.row("task-0001")?.ended, "REPLY_EXPIRED"); XCTAssertNil(list.list.row("task-0002")?.ended)
        XCTAssertEqual(list.list.now, start + 130_500, "the time is the time of the last wake")
        // A closed board keeps no time.
        list.close()
        let after = waits.all.count
        await list.keepTime()
        XCTAssertEqual(waits.all.count, after)
    }

    @MainActor func testTheTimeIsKeptEvenWhenNothingWaitsAndWhenTheServiceStillListsAReplyThatRanOutHere() async throws {
        let clock = TestClock(start)
        let service = Service([try reply(1, extra: ["expiresAt": start + 5000])])
        let waits = Collected<Double>()
        let (list, _) = board(service, now: clock, sleep: { seconds in
            waits.add(seconds)
            if waits.all.count > 3 { throw CancellationError() }
            clock.value += seconds * 1000
        })
        await list.load()
        await list.keepTime()
        // The service's clock is behind this one: it still lists the reply. The card is over by this clock, and the list is not read in a loop for it.
        XCTAssertEqual(waits.all, [5.5, 30, 30, 30]); XCTAssertEqual(service.reads, 2)
        XCTAssertEqual(list.list.row("task-0001")?.over(at: list.list.now), "REPLY_EXPIRED")
        XCTAssertEqual(TeamHeldRules.waiting(list.list), 0)
        let empty = Service([])
        let waited = Collected<Double>()
        let (quiet, _) = board(empty, now: clock, sleep: { seconds in waited.add(seconds); if waited.all.count > 1 { throw CancellationError() } })
        await quiet.keepTime()
        XCTAssertEqual(waited.all, [30, 30]); XCTAssertEqual(empty.reads, 0, "with nothing to wait for, the time is kept and nothing is read")
    }

    // MARK: The words

    func testEveryWayAReplyCanBeRefusedHasWordsAndTheRestIsWhatEveryMessageCommandSays() throws {
        let table: [(codes: [String], key: String)] = [
            (["REPLY_APPROVED", "REPLY_REJECTED", "REPLY_NOT_HELD"], "teams_error_reply_decided"), (["REPLY_EXPIRED"], "teams_error_reply_expired"), (["REPLY_SUPERSEDED"], "teams_error_reply_superseded"),
            (["TASK_REQUIRED"], "teams_error_reply_gone"), (["MAINTAINER_REQUIRED"], "teams_error_held_maintainer"), ([TeamHeldRules.notPosted], "teams_held_revoked"),
        ]
        for (codes, key) in table { for code in codes { XCTAssertEqual(TeamHeldRules.errorKey(code), key, code) } }
        for code in TeamHeldRules.ended { XCTAssertNotEqual(TeamHeldRules.errorKey(code), "teams_error", "a reply that is gone says why: \(code)") }
        for code in ["SERVICE_UNAVAILABLE", "INVALID_INPUT", "MEMBERSHIP_REQUIRED", "INVALID_DATA", "SOMETHING_ELSE"] { XCTAssertEqual(TeamHeldRules.errorKey(code), TeamsErrors.key(code), code) }
        XCTAssertNotEqual(TeamHeldRules.errorKey("MAINTAINER_REQUIRED"), TeamsErrors.key("MAINTAINER_REQUIRED"), "the words of a reply, not those of an operation")
        XCTAssertEqual(TeamHeldRules.ended, ["REPLY_APPROVED", "REPLY_REJECTED", "REPLY_NOT_HELD", "REPLY_EXPIRED", "REPLY_SUPERSEDED", "TASK_REQUIRED", "MAINTAINER_REQUIRED"])
    }

    func testTheWordsOfThisFeatureAreInEveryLanguageEachDifferentFromTheOthersWithABotNeverCalledAnAssistant() throws {
        let catalogue = try Catalogue.load()
        let languages = ["en", "zh-Hans", "zh-Hant", "ja", "ko"]
        for key in Set(TeamHeldRules.allKeys) {
            let values = languages.map { catalogue.value(key, $0) }
            for (language, value) in zip(languages, values) { XCTAssertFalse((value ?? "").isEmpty, "\(key) has no text in \(language)") }
            if key.hasPrefix("teams_held_") || key.hasPrefix("teams_error_reply_") || key == "teams_error_held_maintainer" {
                XCTAssertEqual(Set(values.compactMap { $0 }).count, 5, "\(key) differs in every language")
            }
        }
        for key in ["teams_held_expires %@", "teams_held_document %@"] { for language in languages { XCTAssertTrue((catalogue.value(key, language) ?? "").contains("%@"), "\(key) in \(language) says where its figure goes") } }
        let said = TeamHeldRules.allKeys.flatMap { key in languages.compactMap { catalogue.value(key, $0) } }.joined(separator: " ")
        XCTAssertNil(said.range(of: "assistant|助手|アシスタント|어시스턴트", options: [.regularExpression, .caseInsensitive]), "a Bot is never called an assistant")
        XCTAssertEqual(catalogue.value("teams_held_waiting", "zh-Hant"), "待核准的回覆"); XCTAssertEqual(catalogue.value("teams_held_waiting", "en"), "Replies waiting")
        XCTAssertEqual(catalogue.value("teams_held_expires %@", "en"), "Dropped %@ if not approved"); XCTAssertEqual(catalogue.value("teams_held_document %@", "en"), "Includes the document “%@”")
        XCTAssertEqual(catalogue.value("teams_approval_approve", "zh-Hant"), "核准", "the word of a decision a person is already used to")
        XCTAssertTrue(catalogue.value("teams_error_held_maintainer", "ja")?.contains("メンテナー") == true); XCTAssertTrue(catalogue.value("teams_error_held_maintainer", "ko")?.contains("메인테이너") == true)
    }
}

/// The time the board under test keeps, which the test moves.
private final class TestClock: @unchecked Sendable {
    private let lock = NSLock()
    private var _value: Double
    init(_ value: Double = 1_800_000_000_000) { _value = value }
    var value: Double { get { lock.withLock { _value } } set { lock.withLock { _value = newValue } } }
}
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var value = 0
    /// The number of the call, from 0.
    func next() -> Int { lock.withLock { defer { value += 1 }; return value } }
    var count: Int { lock.withLock { value } }
}
/// What a read or a change callback saw, in order.
private final class Collected<Element>: @unchecked Sendable {
    private var values: [Element] = []
    private let lock = NSLock()
    func add(_ value: Element) { lock.withLock { values.append(value) } }
    var all: [Element] { lock.withLock { values } }
}
/// Holds a read until the test lets it answer.
private final class AsyncGate: @unchecked Sendable {
    private var opened = false
    private var waiters: [CheckedContinuation<Void, Never>] = []
    private let lock = NSLock()
    func wait() async {
        await withCheckedContinuation { continuation in
            lock.lock()
            if opened { lock.unlock(); continuation.resume() } else { waiters.append(continuation); lock.unlock() }
        }
    }
    func open() {
        lock.lock(); opened = true; let pending = waiters; waiters = []; lock.unlock()
        for continuation in pending { continuation.resume() }
    }
}
