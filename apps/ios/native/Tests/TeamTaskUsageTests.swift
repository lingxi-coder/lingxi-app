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
