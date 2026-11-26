import Foundation
import XCTest
@testable import LingxiCode

/// The auto-reply log of a Bot (Bot v3 B5b), for the person who maintains it: the rows as the service writes them, where a judged message ended up, whose log a
/// person reads in a Chat, the paged list, the search for why a Bot stayed silent, the words in every language, and the command that reads it. Foundation-only,
/// so it runs without the app: the screens are `TeamTriageLogViews.swift`.
final class TeamTriageLogTests: XCTestCase {
    private let viewer = TeamMemberRef(uid: "alice", generation: 1)

    /// One row of the log as the service writes it; `n` is its number, newest verdicts have the larger one.
    private func row(_ n: Int, state: String = "declined", title: String? = "Planning", triage: [String: Any] = [:], extra: [String: Any] = [:]) -> [String: Any] {
        var verdict: [String: Any] = ["action": "decline", "source": "decision", "reason": "below_threshold", "probability": 0.12, "at": 1_700_000_000_000.0 + Double(n) * 1000]
        verdict.merge(triage) { _, new in new }
        var value: [String: Any] = ["taskId": String(format: "task-%04d", n), "conversationId": "chat_0001", "messageId": "message-\(n)", "createdAt": 1_700_000_000_000.0 + Double(n),
                                    "state": state, "triage": verdict]
        if let title { value["conversationTitle"] = title }
        value.merge(extra) { _, new in new }
        return value
    }
    private func decode<T: Decodable>(_ type: T.Type, _ value: Any) throws -> T { try JSONDecoder().decode(T.self, from: JSONSerialization.data(withJSONObject: value)) }
    private func item(_ n: Int, state: String = "declined", triage: [String: Any] = [:]) throws -> TeamTriageItem { try decode(TeamTriageItem.self, row(n, state: state, triage: triage)) }
    private func page(_ numbers: [Int], next: String? = nil) throws -> TeamTriagePage { TeamTriagePage(items: try numbers.map { try item($0) }, nextCursor: next) }

    private func card(_ id: String, maintainer: String = "alice", generation: Int = 1, daily: Int? = 5) throws -> TeamBotCard {
        var value: [String: Any] = ["sharedBotId": id, "name": id, "description": "", "maintainer": ["uid": maintainer, "generation": generation], "revision": 1, "state": "published",
                                    "permitted": true, "ready": true, "onlineUntil": 0, "usePolicy": ["mode": "all"], "quotas": ["team": 1, "member": 1, "bot": 1]]
        if let daily { value["autoReply"] = ["daily": daily] }
        return try decode(TeamBotCard.self, value)
    }
    private func chat(_ kind: String = "group", _ participants: [String], on: [String]? = nil) -> TeamMessageConversation {
        TeamMessageConversation(id: "chat_0001", kind: kind, title: "Trip", participants: participants, lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0,
                                autoReply: on.map { TeamConversationAutoReply(revision: 1, targets: $0) })
    }
    private func task(_ id: String, bot: String = "sharedbot_01", state: String = "declined") throws -> TeamTask {
        try decode(TeamTask.self, ["taskId": id, "conversationId": "chat_0001", "sharedBotId": bot, "requester": ["uid": "alice", "generation": 1], "state": state, "createdAt": 1.0, "deadline": 2.0])
    }

    // MARK: Where a message ended up

    func testEveryStateOfATaskIsOneOfFiveOutcomes() {
        let expected: [String: TeamTriageOutcome] = ["completed": .replied, "declined": .silent, "cancelled": .silent, "queued": .waiting, "running": .waiting, "waiting": .waiting, "awaiting_approval": .approval,
                                                     "failed": .failed, "expired": .failed, "interrupted": .failed, "authorization_required": .failed, "revoked": .failed, "needs_reconciliation": .failed]
        XCTAssertEqual(TeamTriageRules.outcomes, expected)
        for state in expected.keys { XCTAssertNotEqual(TeamsBotText.runState(state).labelKey, "chat_status_unknown", "the card of every state this log lists is one the app knows: \(state)") }
        XCTAssertEqual(TeamTriageRules.percent(0.124), 12); XCTAssertEqual(TeamTriageRules.percent(0.125), 13); XCTAssertEqual(TeamTriageRules.percent(1), 100); XCTAssertEqual(TeamTriageRules.percent(0), 0)
    }

    // MARK: The rows as the service writes them

    func testARowIsReadAsTheServiceWroteIt() throws {
        let full = try item(7)
        XCTAssertEqual(full, TeamTriageItem(taskId: "task-0007", conversationId: "chat_0001", messageId: "message-7", createdAt: 1_700_000_000_007,
                                            triage: TeamTriageVerdict(action: "decline", source: "decision", at: 1_700_000_007_000, reason: "below_threshold", probability: 0.12),
                                            state: "declined", conversationTitle: "Planning"))
        let bare = try decode(TeamTriageItem.self, row(8, title: nil, triage: ["action": "run", "source": "rule", "reason": NSNull(), "probability": NSNull()]))
        XCTAssertEqual(bare.triage.action, "run"); XCTAssertNil(bare.triage.reason); XCTAssertNil(bare.triage.probability); XCTAssertNil(bare.conversationTitle)
        XCTAssertNil(try decode(TeamTriageItem.self, row(9, extra: ["conversationTitle": ""])).conversationTitle, "an empty title is no title")
        // What this version does not know is left out of the row; what it needs is the row.
        XCTAssertNil(try item(10, triage: ["reason": "a_new_reason"]).triage.reason)
        for probability in [-0.1, 1.2] { XCTAssertNil(try item(11, triage: ["probability": probability]).triage.probability, "\(probability)") }
        XCTAssertNil(try item(12, triage: ["probability": "0.5"]).triage.probability)
        XCTAssertEqual(try item(13, triage: ["reason": "quiet_hours", "source": "quiet", "probability": 0]).triage.probability, 0, "zero is a probability")
        let unreadable: [[String: Any]] = [
            row(1, extra: ["taskId": ""]), row(1, extra: ["conversationId": ""]), row(1, extra: ["messageId": ""]), row(1, extra: ["createdAt": "1"]), row(1, extra: ["createdAt": 9e15]),
            row(1, state: "a_new_state"), row(1, extra: ["state": NSNull()]), row(1, triage: ["action": "a_new_action"]), row(1, triage: ["source": "elsewhere"]), row(1, triage: ["at": 1e16]),
            row(1, extra: ["triage": NSNull()]), ["nonsense": true],
        ]
        for value in unreadable { XCTAssertThrowsError(try decode(TeamTriageItem.self, value), "\(value)") }
    }

    func testAPageKeepsTheRowsItCanReadAndGoesOnAfterOneItCannot() throws {
        let value: [String: Any] = ["items": [row(3), ["nonsense": true], 7, NSNull(), row(1, state: "a_new_state"), row(2)], "nextCursor": "task-0002"]
        let page = try decode(TeamTriagePage.self, value)
        XCTAssertEqual(page.items.map(\.taskId), ["task-0003", "task-0002"]); XCTAssertEqual(page.nextCursor, "task-0002")
        XCTAssertNil(try decode(TeamTriagePage.self, ["items": [Any]()]).nextCursor, "the end of the log")
        XCTAssertNil(try decode(TeamTriagePage.self, ["items": [Any](), "nextCursor": ""]).nextCursor)
        XCTAssertNil(try decode(TeamTriagePage.self, ["items": [Any](), "nextCursor": NSNull()]).nextCursor)
        for nonsense: Any in [["nope": true], ["items": "x"], ["items": NSNull()]] { XCTAssertThrowsError(try decode(TeamTriagePage.self, nonsense), "an answer that is no page is not an empty log: \(nonsense)") }
    }

    // MARK: A reply held for the maintainer

    func testAHeldReplySaysHowItCameOutAndACodeThisVersionDoesNotKnowIsLeftOut() throws {
        for approval in ["held", "approved", "rejected", "timeout", "superseded"] {
            XCTAssertEqual(try decode(TeamTriageItem.self, row(3, state: "awaiting_approval", extra: ["approval": approval])).approval, approval, approval)
        }
        XCTAssertNil(try item(4).approval, "a reply that never was held says nothing")
        XCTAssertEqual(try item(4, state: "awaiting_approval").state, "awaiting_approval", "a row whose reply waits is a row this version reads")
        for approval: Any in ["a_new_outcome", "", 7, NSNull()] { XCTAssertNil(try decode(TeamTriageItem.self, row(5, extra: ["approval": approval])).approval, "\(approval)") }
        // The words: a reply that still waits says so as its outcome, so `held` has none of its own; the others say how it was decided.
        XCTAssertEqual(TeamTriageRules.outcomeKey(.approval), "teams_task_waiting_approval")
        XCTAssertEqual(["approved", "rejected", "timeout", "superseded"].map { TeamTriageRules.approvalKey($0) },
                       ["teams_approval_reply_approved", "teams_approval_reply_declined", "teams_triage_approval_timeout", "teams_triage_approval_superseded"])
        for none in [nil, "held", "a_new_outcome"] as [String?] { XCTAssertNil(TeamTriageRules.approvalKey(none), "\(none ?? "nil")") }
    }

    // MARK: The words

    func testEveryVerdictReasonAndOutcomeHasWordsInEveryLanguage() throws {
        let catalogue = try Catalogue.load()
        let languages = ["en", "zh-Hans", "zh-Hant", "ja", "ko"]
        var keys = TeamTriageRules.allKeys
        for action in TeamTriageRules.actions.sorted() { keys.append(try XCTUnwrap(TeamTriageRules.actionKey(action), action)) }
        for reason in TeamTriageRules.reasons.sorted() { keys.append(try XCTUnwrap(TeamTriageRules.reasonKey(reason), reason)) }
        for outcome in [TeamTriageOutcome.replied, .silent, .failed, .waiting, .approval] { keys.append(TeamTriageRules.outcomeKey(outcome)) }
        for approval in TeamTriageRules.approvals.sorted() { if let key = TeamTriageRules.approvalKey(approval) { keys.append(key) } }
        for key in Set(keys) {
            let values = languages.map { catalogue.value(key, $0) }
            for (language, value) in zip(languages, values) { XCTAssertFalse((value ?? "").isEmpty, "\(key) has no text in \(language)") }
            // The words the log adds are not the Simplified Chinese they were written from, in any other language.
            if key.hasPrefix("teams_triage_") { for (language, value) in zip(languages, values) where language != "zh-Hans" { XCTAssertNotEqual(value, values[1], "\(key) in \(language)") } }
        }
        XCTAssertNil(TeamTriageRules.reasonKey(nil)); XCTAssertNil(TeamTriageRules.reasonKey("a_new_reason")); XCTAssertNil(TeamTriageRules.actionKey("a_new_action"))
        XCTAssertEqual(catalogue.value("teams_triage_title", "zh-Hant"), "自主回覆紀錄"); XCTAssertEqual(catalogue.value("teams_triage_title", "en"), "Auto-reply log")
        XCTAssertEqual(catalogue.value("teams_triage_probability %lld", "en"), "%lld%% likely")
        let intros = languages.compactMap { catalogue.value("teams_triage_intro", $0) }.joined(separator: " ")
        XCTAssertNil(intros.range(of: "assistant|助手|アシスタント|어시스턴트", options: [.regularExpression, .caseInsensitive]), "a Bot is never called an assistant")
    }

    // MARK: Whose log

    func testTheBotsOfAChatWhoseLogAPersonReadsAreTheOnesTheyMaintainThatAnswerOnTheirOwn() throws {
        let cards = [try card("sharedbot_on"), try card("sharedbot_off", daily: nil), try card("sharedbot_set", daily: nil), try card("sharedbot_theirs", maintainer: "owner"),
                     try card("sharedbot_older", generation: 0), try card("sharedbot_elsewhere")]
        let participants = ["user:bob", "shared_bot:sharedbot_on", "shared_bot:sharedbot_off", "shared_bot:sharedbot_set", "shared_bot:sharedbot_theirs", "shared_bot:sharedbot_older"]
        func ids(_ conversation: TeamMessageConversation, _ who: TeamMemberRef? = nil) -> [String] { TeamTriageRules.bots(conversation, cards: cards, viewer: who ?? viewer).map(\.sharedBotId) }
        XCTAssertEqual(ids(chat("group", participants)), ["sharedbot_on"], "a Bot that never answers on its own has no log, a Bot of someone else is not theirs, and one not in the Chat is not there")
        XCTAssertEqual(ids(chat("group", participants, on: ["sharedbot_set"])), ["sharedbot_on", "sharedbot_set"], "one switched on in this Chat has")
        XCTAssertEqual(ids(chat("group", participants), TeamMemberRef(uid: "bob", generation: 1)), [], "a person who maintains none has none")
        XCTAssertEqual(ids(chat("group", ["user:alice", "shared_bot:sharedbot_on"])), ["sharedbot_on"])
        XCTAssertEqual(ids(chat("group", ["user:bob", "shared_bot:sharedbot_on"])), ["sharedbot_on"], "taking part in the Chat is not needed")
        XCTAssertFalse(TeamTriageRules.maintains(cards[4], viewer: viewer), "by the membership they hold now"); XCTAssertTrue(TeamTriageRules.maintains(cards[0], viewer: viewer))
    }

    func testOnlyTheMaintainerOfABotDmIsToldWhyItStayedSilent() throws {
        let mine = try card("sharedbot_01"), theirs = try card("sharedbot_01", maintainer: "owner")
        let dm = chat("bot_dm", ["user:alice", "shared_bot:sharedbot_01"])
        XCTAssertEqual(TeamTriageRules.reasonBot(dm, cards: [mine], viewer: viewer)?.sharedBotId, "sharedbot_01")
        XCTAssertNil(TeamTriageRules.reasonBot(dm, cards: [theirs], viewer: viewer), "someone who only talks to the Bot is not told")
        XCTAssertNil(TeamTriageRules.reasonBot(chat("group", ["user:alice", "shared_bot:sharedbot_01"]), cards: [mine], viewer: viewer), "there is no row in a group, and the Chat's log is one button away")
        XCTAssertNil(TeamTriageRules.reasonBot(dm, cards: [], viewer: viewer))
        let tasks = [try task("task-0001"), try task("task-0002", state: "completed"), try task("task-0003", bot: "sharedbot_02"), try task("task-0004")]
        XCTAssertEqual(TeamTriageRules.silentTasks(tasks, of: "sharedbot_01"), ["task-0001", "task-0004"], "only the tasks of this Bot that did not reply")
        XCTAssertEqual(TeamTriageRules.silentTasks([], of: "sharedbot_01"), [])
        let parts = TeamTriageRules.reasonParts(try item(1))
        XCTAssertEqual(parts.reasonKey, "teams_triage_reason_below_threshold"); XCTAssertEqual(parts.percent, 12)
        let quiet = TeamTriageRules.reasonParts(try item(2, triage: ["source": "quiet", "reason": "quiet_hours", "probability": NSNull()]))
        XCTAssertEqual(quiet.reasonKey, "teams_triage_reason_quiet_hours"); XCTAssertNil(quiet.percent)
        let none = TeamTriageRules.reasonParts(nil)
        XCTAssertNil(none.reasonKey); XCTAssertNil(none.percent)
    }

    // MARK: The command

    func testTheLogIsAskedForWithTheMemberCommand() throws {
        func command(_ payload: [String: Any]) -> [String: Any] { ["schemaVersion": 1, "commandId": "command_0001", "teamId": "team_0001", "generation": 1, "action": "listTriage", "payload": payload] }
        let bot = "sharedbot_01"
        for payload in [["sharedBotId": bot], ["sharedBotId": bot, "conversationId": "chat_0001", "limit": 20], ["sharedBotId": bot, "conversationId": "chat_0001", "cursor": "task_0001", "limit": 50], ["sharedBotId": bot, "limit": 1]] as [[String: Any]] {
            XCTAssertNoThrow(try TeamExecutionContract.validate(command(payload)), "\(payload)")
        }
        let refused: [[String: Any]] = [[:], ["conversationId": "chat_0001"], ["sharedBotId": "x"], ["sharedBotId": bot, "conversationId": "x"], ["sharedBotId": bot, "cursor": "x"],
                                        ["sharedBotId": bot, "limit": 0], ["sharedBotId": bot, "limit": 51], ["sharedBotId": bot, "limit": 1.5], ["sharedBotId": bot, "limit": "5"], ["sharedBotId": bot, "limit": true],
                                        ["sharedBotId": bot, "teamId": "team_0001"], ["sharedBotId": bot, "deploymentId": "deployment_01"], ["sharedBotId": bot, "includeHidden": true]]
        for payload in refused { XCTAssertThrowsError(try TeamExecutionContract.validate(command(payload)), "\(payload)") }
        var host = command(["taskId": "task_0001", "action": "decline", "source": "decision"]); host["action"] = "recordTriage"
        XCTAssertThrowsError(try TeamExecutionContract.validate(host), "the host's write of a verdict is not a member command")
        XCTAssertEqual(TeamExecutionContract.triageLimit, 50); XCTAssertLessThanOrEqual(TeamTriageRules.page, TeamExecutionContract.triageLimit)
    }

    // MARK: The list

    @MainActor func testTheListReadsAPageAtATimeUntilTheLogEnds() async throws {
        let all = try (1...45).reversed().map { try item($0) }
        let statuses = Collected<TeamTriageList.Status>(), cursors = Collected<String?>()
        let reader = TeamTriageReader(read: { cursor, limit in
            cursors.add(cursor)
            let from = cursor.flatMap { id in all.firstIndex { $0.taskId == id } }.map { $0 + 1 } ?? 0
            let rows = Array(all[from..<min(all.count, from + limit)])
            return TeamTriagePage(items: rows, nextCursor: from + limit < all.count ? rows.last?.taskId : nil)
        }, changed: { statuses.add($0.status) })
        XCTAssertEqual(reader.list.status, .loading)
        await reader.load()
        XCTAssertEqual(statuses.all, [.loading, .ready]); XCTAssertEqual(reader.list.items.count, 20); XCTAssertEqual(reader.list.items.first?.taskId, "task-0045"); XCTAssertEqual(reader.list.nextCursor, "task-0026")
        await reader.more()
        XCTAssertEqual(reader.list.items.count, 40); XCTAssertEqual(reader.list.items.last?.taskId, "task-0006"); XCTAssertEqual(reader.list.nextCursor, "task-0006")
        await reader.more()
        XCTAssertEqual(reader.list.items.count, 45); XCTAssertNil(reader.list.nextCursor, "the end of the log")
        let before = cursors.all.count
        await reader.more()
        XCTAssertEqual(cursors.all.count, before, "nothing is asked after the end")
        XCTAssertEqual(cursors.all, [nil, "task-0026", "task-0006"])
        XCTAssertEqual(Set(reader.list.items.map(\.taskId)).count, 45)
        await reader.load()
        XCTAssertEqual(reader.list.items.count, 20, "refreshing starts over from the newest")
    }

    @MainActor func testAPageThatRepeatsRowsOfTheOneBeforeAddsNothingTwice() async throws {
        let pages = Feed<TeamTriagePage>([try page([3, 2], next: "task-0002"), try page([2, 1])])
        let reader = TeamTriageReader(read: { _, _ in pages.next() }, changed: { _ in })
        await reader.load(); await reader.more()
        XCTAssertEqual(reader.list.items.map(\.taskId), ["task-0003", "task-0002", "task-0001"])
    }

    @MainActor func testAReadThatFailsKeepsWhatWasReadAndWhereItStoppedAndOnlyNotTheMaintainerEndsTheLog() async throws {
        let outcomes = Feed<Result<TeamTriagePage, Error>>([.success(try page([3, 2], next: "task-0002")), .failure(TeamMessageFailure(code: "SERVICE_UNAVAILABLE")), .success(try page([1]))])
        let cursors = Collected<String?>()
        let reader = TeamTriageReader(read: { cursor, _ in cursors.add(cursor); return try outcomes.next().get() }, changed: { _ in })
        await reader.load(); await reader.more()
        XCTAssertEqual(reader.list.status, .failed); XCTAssertEqual(reader.list.error, "SERVICE_UNAVAILABLE"); XCTAssertEqual(reader.list.items.count, 2, "the rows read before stay"); XCTAssertEqual(reader.list.nextCursor, "task-0002")
        await reader.more()
        XCTAssertEqual(reader.list.status, .ready); XCTAssertEqual(reader.list.items.count, 3); XCTAssertEqual(cursors.all, [nil, "task-0002", "task-0002"], "reading again goes on from where it stopped")
        // A first page that fails has nothing and starts again.
        let first = TeamTriageReader(read: { _, _ in throw URLError(.notConnectedToInternet) }, changed: { _ in })
        await first.load()
        XCTAssertEqual(first.list.status, .failed); XCTAssertTrue(first.list.items.isEmpty); XCTAssertEqual(first.list.error, "SERVICE_UNAVAILABLE", "an error that is not the service's is the service being unavailable")
        await first.more()
        XCTAssertEqual(first.list.status, .failed, "there is no page to go on from")
        // Not the maintainer: no log, no rows, nothing more to read.
        let reads = Collected<Int>()
        let denied = TeamTriageReader(read: { _, _ in reads.add(1); throw TeamMessageFailure(code: "MAINTAINER_REQUIRED") }, changed: { _ in })
        await denied.load(); await denied.more()
        XCTAssertEqual(denied.list.status, .denied); XCTAssertTrue(denied.list.items.isEmpty); XCTAssertEqual(reads.all.count, 1)
        XCTAssertFalse(denied.list.busy)
    }

    @MainActor func testAReadThatWasOvertakenOrAnswersAfterTheListWasClosedChangesNothing() async throws {
        let gates = (0..<3).map { _ in AsyncGate() }, arrivals = Counter()
        let pages = [try page([1]), try page([9]), try page([5])]
        let seen = Collected<TeamTriageList.Status>()
        let reader = TeamTriageReader(read: { _, _ in
            let mine = await arrivals.next()
            await gates[mine].wait()
            return pages[mine]
        }, changed: { seen.add($0.status) })
        let stale = Task { @MainActor in await reader.load() }
        while await arrivals.count < 1 { await Task.yield() }
        let current = Task { @MainActor in await reader.load() }
        while await arrivals.count < 2 { await Task.yield() }
        gates[1].open(); await current.value
        gates[0].open(); await stale.value
        XCTAssertEqual(reader.list.items.map(\.taskId), ["task-0009"], "the older answer did not replace the newer one")
        let late = Task { @MainActor in await reader.load() }
        while await arrivals.count < 3 { await Task.yield() }
        reader.close()
        let shown = seen.all.count
        gates[2].open(); await late.value
        XCTAssertEqual(seen.all.count, shown, "nothing was shown after it was closed")
        XCTAssertEqual(reader.list.items.map(\.taskId), [], "what the closed read would have brought is not there; the list is the one the newest read left (loading)")
    }

    // MARK: Why a Bot stayed silent

    func testTheReasonOfASilenceIsLookedForFromTheNewestVerdictDownOnlyAsFarAsItMust() async throws {
        let all = try (1...260).reversed().map { try item($0) }
        let box = Box()
        let read: TeamTriageRead = { cursor, limit in
            await box.add(limit)
            let from = cursor.flatMap { id in all.firstIndex { $0.taskId == id } }.map { $0 + 1 } ?? 0
            let rows = Array(all[from..<min(all.count, from + limit)])
            return TeamTriagePage(items: rows, nextCursor: from + limit < all.count ? rows.last?.taskId : nil)
        }
        let near = await TeamTriageSearch.find(["task-0260", "task-0220"], read: read)
        XCTAssertEqual(Set(near.keys), ["task-0260", "task-0220"]); let nearReads = await box.limits
        XCTAssertEqual(nearReads, [50], "both are on the first page: one read of 50")
        await box.reset()
        let far = await TeamTriageSearch.find(["task-0260", "task-0150"], read: read)
        XCTAssertEqual(Set(far.keys), ["task-0260", "task-0150"]); let farReads = await box.limits.count
        XCTAssertEqual(farReads, 3)
        await box.reset()
        let lost = await TeamTriageSearch.find(["task-0150", "task-9999"], read: read)
        XCTAssertEqual(Set(lost.keys), ["task-0150"], "a task with no row is not in the answer"); let lostReads = await box.limits.count
        XCTAssertEqual(lostReads, TeamTriageRules.findPages, "and the search is not endless"); XCTAssertEqual(TeamTriageRules.findPages, 4)
        await box.reset()
        let capped = await TeamTriageSearch.find(["task-0001"], pages: 2, read: read); let cappedReads = await box.limits.count
        XCTAssertTrue(capped.isEmpty); XCTAssertEqual(cappedReads, 2, "the cap of pages is a parameter")
        await box.reset()
        let nothing = await TeamTriageSearch.find([], read: read); let none = await box.limits.count
        XCTAssertTrue(nothing.isEmpty); XCTAssertEqual(none, 0, "nothing wanted, nothing read")
        // A cursor that does not move ends the search; a read that fails ends it with what it has.
        let stuck = Box()
        let still = await TeamTriageSearch.find(["x"], read: { _, _ in await stuck.add(1); return TeamTriagePage(items: [], nextCursor: "task-0001") })
        let stuckReads = await stuck.limits.count
        XCTAssertTrue(still.isEmpty); XCTAssertEqual(stuckReads, 2)
        let two = try item(2)
        let partial = await TeamTriageSearch.find(["task-0002", "task-0001"], read: { cursor, _ in
            if cursor != nil { throw TeamMessageFailure(code: "SERVICE_UNAVAILABLE") }
            return TeamTriagePage(items: [two], nextCursor: "task-0002")
        })
        XCTAssertEqual(Set(partial.keys), ["task-0002"])
    }
}

private actor Box {
    private(set) var limits: [Int] = []
    func add(_ limit: Int) { limits.append(limit) }
    func reset() { limits = [] }
}
private actor Counter {
    private(set) var count = 0
    func next() -> Int { defer { count += 1 }; return count }
}
/// What a read or a change callback saw, in order. The reads run off the main actor, so it locks.
private final class Collected<Element>: @unchecked Sendable {
    private var values: [Element] = []
    private let lock = NSLock()
    func add(_ value: Element) { lock.lock(); values.append(value); lock.unlock() }
    var all: [Element] { lock.lock(); defer { lock.unlock() }; return values }
}
/// Hands out what a test scripted, one at a time.
private final class Feed<Element>: @unchecked Sendable {
    private var values: [Element]
    private let lock = NSLock()
    init(_ values: [Element]) { self.values = values }
    func next() -> Element { lock.lock(); defer { lock.unlock() }; return values.removeFirst() }
}
/// Holds a read until the test lets it answer, so two reads can be made to answer in the other order.
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
