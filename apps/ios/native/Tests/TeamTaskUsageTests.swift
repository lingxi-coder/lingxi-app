import Foundation
import XCTest
@testable import LingxiCode

/// The usage line under a Team task and which messages are Markdown: the same rules as Desktop's `usageText`
/// and the conversation view's `author.kind === 'shared_bot'`. The line is docs/TEAMS-UI-POLISH.md U5: the total of
/// the tokens in the compact number of the language, a middle dot, then the cost with two to four decimals.
final class TeamTaskUsageTests: XCTestCase {
    private let words = TeamsBotText.UsageStrings.english

    private func line(_ json: String?, strings: TeamsBotText.UsageStrings? = nil) throws -> String {
        let usage = try json.map { try JSONDecoder().decode(TeamTaskUsage.self, from: Data($0.utf8)) }
        return TeamsBotText.usageLine(usage, strings: strings ?? words)
    }

    func testMissingOrAbsentUsageNeverInventsATokenCountOrACost() throws {
        XCTAssertEqual(try line(nil), "Usage unavailable")
        XCTAssertEqual(try line(#"{"status":"missing"}"#), "Usage unavailable")
        XCTAssertEqual(try line(#"{"status":"missing","inputTokens":5,"outputTokens":6,"costMicros":7,"currency":"USD"}"#), "Usage unavailable")
    }

    func testPartialUsageGetsAPrefixAndCostUnknownWithoutACost() throws {
        XCTAssertEqual(try line(#"{"status":"partial","inputTokens":3,"outputTokens":4}"#), "Partial usage · 7 tokens · cost unknown")
    }

    func testCompleteUsageHasNoPrefixAndShowsTheTotalOfTheTokens() throws {
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":123,"outputTokens":456,"costMicros":1322,"currency":"USD"}"#),
                       "579 tokens · $0.0013")
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":3,"outputTokens":4,"costMicros":0,"currency":"USD"}"#),
                       "7 tokens · $0.00")
        // A missing count is zero, as on Desktop.
        XCTAssertEqual(try line(#"{"status":"complete","outputTokens":4,"costMicros":2000000,"currency":"USD"}"#), "4 tokens · $2.00")
    }

    func testTokensUseTheCompactNumberOfTheLanguage() throws {
        let usage = #"{"status":"complete","inputTokens":1000,"outputTokens":234,"costMicros":1200,"currency":"USD"}"#
        XCTAssertEqual(try line(usage), "1.2K tokens · $0.0012")
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":2000000,"outputTokens":0,"costMicros":0,"currency":"USD"}"#), "2M tokens · $0.00")
        var chinese = words
        chinese.locale = Locale(identifier: "zh_Hans_CN")
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":12345,"outputTokens":0,"costMicros":0,"currency":"CNY"}"#, strings: chinese),
                       "1.2万 tokens · ¥0.00")
    }

    func testCostHasTwoToFourDecimalsInTheCurrencyOfTheTask() throws {
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":1,"outputTokens":1,"costMicros":1322,"currency":"USD"}"#), "2 tokens · $0.0013")
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":1,"outputTokens":1,"costMicros":120000,"currency":"USD"}"#), "2 tokens · $0.12")
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":1,"outputTokens":1,"costMicros":123456,"currency":"usd"}"#), "2 tokens · $0.1235", "The code is not case sensitive")
        XCTAssertTrue(try line(#"{"status":"complete","inputTokens":1,"outputTokens":1,"costMicros":2500000,"currency":"EUR"}"#).hasSuffix("€2.50"))
    }

    func testACostBelowTheSmallestAmountSaysSo() throws {
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":1,"outputTokens":1,"costMicros":50,"currency":"USD"}"#), "2 tokens · <$0.0001")
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":1,"outputTokens":1,"costMicros":100,"currency":"USD"}"#), "2 tokens · $0.0001")
    }

    func testCostWithoutCurrencySaysCurrencyUnknown() throws {
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":1,"outputTokens":2,"costMicros":2500000}"#),
                       "3 tokens · 2.50 currency unknown")
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":1,"outputTokens":2,"costMicros":2500000,"currency":""}"#),
                       "3 tokens · 2.50 currency unknown")
    }

    func testCompleteUsageWithoutACostSaysCostUnknown() throws {
        XCTAssertEqual(try line(#"{"status":"complete","inputTokens":1,"outputTokens":2}"#), "3 tokens · cost unknown")
    }

    /// The split stays available as the accessibility value (`teams_usage_tokens`).
    func testTheSplitOfTheTokensIsTheAccessibilityValue() throws {
        let decode = { (json: String) in try JSONDecoder().decode(TeamTaskUsage.self, from: Data(json.utf8)) }
        XCTAssertEqual(TeamsBotText.tokenSplit(try decode(#"{"status":"complete","inputTokens":123,"outputTokens":456}"#), strings: words), "123 / 456 tokens")
        XCTAssertEqual(TeamsBotText.tokenSplit(try decode(#"{"status":"partial","outputTokens":4}"#), strings: words), "0 / 4 tokens")
        XCTAssertNil(TeamsBotText.tokenSplit(try decode(#"{"status":"missing"}"#), strings: words))
        XCTAssertNil(TeamsBotText.tokenSplit(nil, strings: words))
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

    // MARK: Approving a list change (M2 Q05)

    private let approvalWait = #","epoch":2,"wait":{"waitId":"wait_1","kind":"tool_approval","recipient":{"uid":"asker","generation":3},"expiresAt":9000000,"messageId":"request"}"#
    private func message(_ id: String, _ author: String, _ extra: String = "") throws -> TeamMessage {
        let json = #"{"id":""# + id + #"","seq":1,"author":"# + author + #","kind":"text","text":"x","changeSeq":1,"changeId":"change_0001","taskId":"t","waitId":"wait_1""# + extra + "}"
        return try JSONDecoder().decode(TeamMessage.self, from: Data(json.utf8))
    }
    private let bot = #"{"kind":"shared_bot","sharedBotId":"sharedbot_01","displayName":"Helper"}"#
    private let person = #"{"kind":"user","uid":"asker"}"#

    func testAProposedChangeIsDecidedOnItsCardAndNeverAnsweredInWords() throws {
        let waiting = try task("waiting", approvalWait)
        XCTAssertNil(TeamTaskActions.answerableWait(waiting, viewer: asker, pendingWaitIds: []))
        XCTAssertEqual(TeamTaskActions.approvalState(waitId: "wait_1", task: waiting, viewer: asker, messages: []), .decide)
        XCTAssertEqual(TeamTaskActions.approvalState(waitId: "wait_1", task: waiting, viewer: maintainer, messages: []), .waiting(uid: "asker"))
        // The same person after rejoining is another member.
        XCTAssertEqual(TeamTaskActions.approvalState(waitId: "wait_1", task: waiting, viewer: TeamMemberRef(uid: "asker", generation: 4), messages: []), .waiting(uid: "asker"))
        // A question is not an approval, and another wait of the same task is not this request.
        XCTAssertEqual(TeamTaskActions.approvalState(waitId: "wait_1", task: try task("waiting", wait), viewer: asker, messages: []), .closed)
        XCTAssertEqual(TeamTaskActions.approvalState(waitId: "wait_0", task: waiting, viewer: asker, messages: []), .closed)
    }

    func testADecidedRequestSaysWhoDecidedAndAnUndecidedOneIsClosed() throws {
        let done = try task("completed")
        let approved = try message("reply", person, #","decision":"approve""#), declined = try message("reply", person, #","decision":"deny""#)
        XCTAssertEqual(TeamTaskActions.approvalState(waitId: "wait_1", task: done, viewer: asker, messages: [approved]), .approved(uid: "asker"))
        XCTAssertEqual(TeamTaskActions.approvalState(waitId: "wait_1", task: nil, viewer: other, messages: [declined]), .declined(uid: "asker"))
        XCTAssertEqual(TeamTaskActions.approvalState(waitId: "wait_1", task: try task("expired"), viewer: asker, messages: []), .closed)
        // Only a member's reply with one of the two decisions counts.
        for stray in [try message("reply", person), try message("reply", person, #","decision":"maybe""#), try message("reply", person, #","decision":true"#), try message("reply", bot, #","decision":"approve""#)] {
            XCTAssertEqual(TeamTaskActions.approvalState(waitId: "wait_1", task: done, viewer: asker, messages: [stray]), .closed)
        }
        XCTAssertEqual(TeamTaskActions.approvalState(waitId: "wait_2", task: done, viewer: asker, messages: [approved]), .closed)
    }

    func testARequestIsShownOnlyWhenEveryChangeCanBeSaidInWords() throws {
        let changes = #"[{"op":"add","text":"Bread"},{"op":"update","itemId":"item_0001","done":true,"item":"Milk"},{"op":"remove","itemId":"item_0002","item":null}]"#
        let request = try XCTUnwrap(try message("request", bot, #","approval":{"title":"Groceries","changes":"# + changes + "}").approvalRequest)
        XCTAssertEqual(request.title, "Groceries")
        XCTAssertEqual(request.changes, [.init(op: "add", text: "Bread"), .init(op: "update", done: true, item: "Milk"), .init(op: "remove")])
        // A request that cannot be read never fails the message it is on; it is just not shown as a request.
        for broken in [#""soon""#, #"{"title":"Groceries"}"#, #"{"title":"Groceries","changes":[]}"#, #"{"title":"Groceries","changes":[{"op":"clear"}]}"#,
                       #"{"title":"Groceries","changes":[{"op":"add"}]}"#, #"{"title":"Groceries","changes":[{"op":"update","itemId":"item_0001"}]}"#, #"{"title":"Groceries","changes":[{"op":"add","text":5}]}"#] {
            let read = try message("request", bot, #","approval":"# + broken)
            XCTAssertNil(read.approvalRequest, broken); XCTAssertEqual(read.text, "x")
        }
        // Only a Bot asks, and a recalled request asks nothing.
        XCTAssertNil(try message("request", person, #","approval":{"title":"Groceries","changes":"# + changes + "}").approvalRequest)
        XCTAssertNil(try message("request", bot, #","recalled":true,"approval":{"title":"Groceries","changes":"# + changes + "}").approvalRequest)
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
