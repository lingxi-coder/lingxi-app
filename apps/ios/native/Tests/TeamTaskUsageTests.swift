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
}
