import Foundation
import XCTest
@testable import LingxiCode

/// The usage line under a Team task and which messages are Markdown: the same rules as Desktop's `usageText`
/// and the conversation view's `author.kind === 'shared_bot'`.
final class TeamTaskUsageTests: XCTestCase {
    private let words = TeamsBotText.UsageStrings.english

    private func line(_ json: String?) throws -> String {
        let usage = try json.map { try JSONDecoder().decode(TeamTaskUsage.self, from: Data($0.utf8)) }
        return TeamsBotText.usageLine(usage, strings: words)
    }

    func testMissingOrAbsentUsageNeverInventsACost() throws {
        XCTAssertEqual(try line(nil), "Missing usage · cost unknown")
        XCTAssertEqual(try line(#"{"status":"missing"}"#), "Missing usage · cost unknown")
    }

    func testPartialUsageWithoutCostSaysCostUnknown() throws {
        XCTAssertEqual(try line(#"{"status":"partial","inputTokens":3,"outputTokens":4}"#), "Partial usage · 3 / 4 tokens · cost unknown")
    }

    func testCompleteUsageShowsTokensAndCostInSixDecimals() throws {
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":123,"outputTokens":456,"costMicros":1322,"currency":"USD"}"#),
                       "Complete usage · 123 / 456 tokens · 0.001322 USD")
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":3,"outputTokens":4,"costMicros":0,"currency":"USD"}"#),
                       "Complete usage · 3 / 4 tokens · 0.000000 USD")
    }

    func testCostWithoutCurrencySaysCurrencyUnknown() throws {
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":1,"outputTokens":2,"costMicros":2500000}"#),
                       "Complete usage · 1 / 2 tokens · 2.500000 currency unknown")
    }

    func testTaskDecodesUsageWhenPresentAndTolleratesItsAbsence() throws {
        let base = #"{"taskId":"t","conversationId":"c","sharedBotId":"b","requester":{"uid":"u","generation":1},"state":"completed","createdAt":1,"deadline":2"#
        let with = try JSONDecoder().decode(TeamTask.self, from: Data((base + #","usage":{"status":"partial","inputTokens":5,"outputTokens":6}}"#).utf8))
        XCTAssertEqual(with.usage, TeamTaskUsage(status: "partial", inputTokens: 5, outputTokens: 6))
        let without = try JSONDecoder().decode(TeamTask.self, from: Data((base + "}").utf8))
        XCTAssertNil(without.usage)
    }

    // MARK: Waiting and cancelling (M2)

    private func task(_ state: String, _ extra: String = "") throws -> TeamTask {
        let json = #"{"taskId":"t","conversationId":"c","sharedBotId":"b","requester":{"uid":"asker","generation":3},"state":""# + state + #"","createdAt":1,"deadline":2"# + extra + "}"
        return try JSONDecoder().decode(TeamTask.self, from: Data(json.utf8))
    }
    private let wait = #","epoch":2,"wait":{"waitId":"wait_1","kind":"human_input","recipient":{"uid":"asker","generation":3},"expiresAt":9000000,"messageId":"question"}"#
    private let asker = TeamMemberRef(uid: "asker", generation: 3)
    private let maintainer = TeamMemberRef(uid: "keeper", generation: 3)
    private let other = TeamMemberRef(uid: "other", generation: 3)

    func testAWaitingTaskIsNeitherFinishedNorPolled() throws {
        let waiting = try task("waiting", wait)
        XCTAssertEqual(waiting.wait, TeamTaskWait(waitId: "wait_1", kind: "human_input", recipient: asker, expiresAt: 9_000_000, messageId: "question"))
        XCTAssertFalse(waiting.terminal); XCTAssertFalse(waiting.busy)
        for state in ["queued", "running"] { XCTAssertTrue(try task(state).busy, state); XCTAssertFalse(try task(state).terminal, state) }
        for state in ["completed", "failed", "expired", "interrupted", "authorization_required", "revoked", "cancelled"] {
            XCTAssertTrue(try task(state).terminal, state); XCTAssertFalse(try task(state).busy, state)
        }
    }

    func testOnlyTheRequesterAndTheMaintainerMayCancelAnUnfinishedTask() throws {
        for state in ["queued", "running", "waiting"] {
            let live = try task(state, state == "waiting" ? wait : "")
            XCTAssertTrue(TeamTaskActions.canCancel(live, viewer: asker, maintainer: maintainer), state)
            XCTAssertTrue(TeamTaskActions.canCancel(live, viewer: maintainer, maintainer: maintainer), state)
            XCTAssertFalse(TeamTaskActions.canCancel(live, viewer: other, maintainer: maintainer), state)
            XCTAssertFalse(TeamTaskActions.canCancel(live, viewer: other, maintainer: nil), state)
        }
        // The same person after rejoining is another member.
        XCTAssertFalse(TeamTaskActions.canCancel(try task("queued"), viewer: TeamMemberRef(uid: "asker", generation: 4), maintainer: nil))
        for state in ["completed", "failed", "cancelled", "expired"] { XCTAssertFalse(TeamTaskActions.canCancel(try task(state), viewer: asker, maintainer: maintainer), state) }
        let stopping = try task("running", #","cancelRequestedAt":5"#)
        XCTAssertTrue(stopping.cancelling)
        XCTAssertFalse(TeamTaskActions.canCancel(stopping, viewer: asker, maintainer: maintainer))
        XCTAssertFalse(try task("cancelled", #","cancelRequestedAt":5"#).cancelling)
    }

    func testOnlyTheMemberATaskWaitsForMayAnswerItAndOnlyOnce() throws {
        let waiting = try task("waiting", wait)
        XCTAssertEqual(TeamTaskActions.answerableWait(waiting, viewer: asker, pendingWaitIds: []), "wait_1")
        XCTAssertNil(TeamTaskActions.answerableWait(waiting, viewer: maintainer, pendingWaitIds: []))
        XCTAssertNil(TeamTaskActions.answerableWait(waiting, viewer: TeamMemberRef(uid: "asker", generation: 4), pendingWaitIds: []))
        XCTAssertNil(TeamTaskActions.answerableWait(waiting, viewer: asker, pendingWaitIds: ["wait_1"]))
        XCTAssertEqual(TeamTaskActions.answerableWait(waiting, viewer: asker, pendingWaitIds: ["wait_0"]), "wait_1")
        XCTAssertNil(TeamTaskActions.answerableWait(try task("running", wait), viewer: asker, pendingWaitIds: []))
        XCTAssertNil(TeamTaskActions.answerableWait(try task("waiting"), viewer: asker, pendingWaitIds: []))
        XCTAssertNil(TeamTaskActions.answerableWait(nil, viewer: asker, pendingWaitIds: []))
    }

    func testAWaitExpiryIsRoundedUpToMinutesHoursOrDays() {
        func expiry(_ ms: Double) -> String { let e = TeamTaskActions.expiry(at: 1_000_000 + ms, now: 1_000_000); return "\(e.value) \(e.unit)" }
        XCTAssertEqual(expiry(-5), "1 minutes")
        XCTAssertEqual(expiry(1), "1 minutes")
        XCTAssertEqual(expiry(60_000), "1 minutes")
        XCTAssertEqual(expiry(60_001), "2 minutes")
        XCTAssertEqual(expiry(59 * 60_000), "59 minutes")
        XCTAssertEqual(expiry(60 * 60_000), "1 hours")
        XCTAssertEqual(expiry(61 * 60_000), "2 hours")
        XCTAssertEqual(expiry(24 * 3_600_000), "24 hours")
        XCTAssertEqual(expiry(47 * 3_600_000), "47 hours")
        XCTAssertEqual(expiry(48 * 3_600_000), "2 days")
        XCTAssertEqual(expiry(49 * 3_600_000), "3 days")
    }

    func testAnAnswerIsSentAsAnswerWaitAndNeverAsksABot() throws {
        let waitId = "wait_0123456789abcdef0123456789abcdef01234567"
        let answer = TeamMessageIntent(teamId: "team_0001", generation: 7, conversationId: "chat_0001", messageId: "message_0002", changeId: "change_0002", operation: "create", text: "Blue", replyTo: "question", waitId: waitId)
        let body = answer.body
        XCTAssertEqual(body["action"] as? String, "answerWait")
        XCTAssertEqual(body["commandId"] as? String, "change_0002")
        XCTAssertEqual(body["generation"] as? Int, 7)
        let payload = try XCTUnwrap(body["payload"] as? [String: String])
        XCTAssertEqual(payload, ["conversationId": "chat_0001", "waitId": waitId, "messageId": "message_0002", "changeId": "change_0002", "text": "Blue"])
        XCTAssertNoThrow(try TeamExecutionContract.validate(body))
        // The saved outbox keeps the wait, so a restart still sends an answer and not an ordinary message.
        let saved = try JSONDecoder().decode(TeamMessageIntent.self, from: JSONEncoder().encode(answer))
        XCTAssertEqual(saved.waitId, waitId)
        XCTAssertEqual(saved.body["action"] as? String, "answerWait")
    }

    func testOnlySharedBotMessagesAreMarkdown() {
        XCTAssertTrue(TeamsBotText.rendersMarkdown(authorKind: "shared_bot", recalled: false))
        XCTAssertFalse(TeamsBotText.rendersMarkdown(authorKind: "user", recalled: false))
        XCTAssertFalse(TeamsBotText.rendersMarkdown(authorKind: "shared_bot", recalled: true))
    }

    func testABotsMarkdownReadsAsOnePlainLine() {
        let vectors: [(String, String)] = [
            ("# QA Follow-Up Check\n\n**Goal: close the open items.**\n\n- Scope: defects", "QA Follow-Up Check Goal: close the open items. Scope: defects"),
            ("**G", "G"),
            ("- [ ] Confirm the list\n- [x] Done", "Confirm the list Done"),
            ("| Item | Owner |\n|---|---|\n| Confirm scope | QA 2 |", "Item Owner Confirm scope QA 2"),
            ("See [the doc](https://example.com/a) and `code`", "See the doc and code"),
            ("```ts\nconst a = 1;\n```", "const a = 1;"),
            ("2 * 3 * 4 and snake_case_name", "2 * 3 * 4 and snake_case_name"),
            ("1. first\n2. second", "first second"),
            ("> quoted _text_", "quoted text"),
            ("![chart](https://x/y.png)", "chart"),
            ("---", ""),
            ("", ""),
        ]
        for (markdown, plain) in vectors { XCTAssertEqual(TeamsBotText.plainText(markdown), plain, markdown) }
    }

    func testPlainTextKeepsWhatIsNotMarkup() {
        XCTAssertEqual(TeamsBotText.plainText("[ ] not a list item"), "[ ] not a list item")
        XCTAssertEqual(TeamsBotText.plainText("a*b*c and a_b_c"), "a*b*c and a_b_c")
        XCTAssertEqual(TeamsBotText.plainText("*emphasis* and __more__"), "emphasis and more")
        XCTAssertEqual(TeamsBotText.plainText("a\r\n\r\nb"), "a b")
        XCTAssertEqual(TeamsBotText.plainText("| a | b |\n| :--- | ---: |"), "a b")
        XCTAssertEqual(TeamsBotText.plainText("run `ls | grep x`"), "run ls | grep x")
    }
}
