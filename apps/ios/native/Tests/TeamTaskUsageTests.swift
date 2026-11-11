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
