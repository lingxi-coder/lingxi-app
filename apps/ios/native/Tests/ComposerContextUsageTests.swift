import XCTest
@testable import LingxiCode

final class ComposerContextUsageTests: XCTestCase {
    func testIncludesCachesButExcludesOutput() throws {
        let usage = ConversationUsageSnapshot(inputTokens: 100, outputTokens: 9_000, cacheReadTokens: 200, cacheCreationTokens: 50)
        let snapshot = try XCTUnwrap(ComposerContextUsage(usage: usage, capacity: 1_000))
        XCTAssertEqual(snapshot.used, 350)
        XCTAssertEqual(snapshot.percent, 35)
    }

    func testMissingOrInvalidDataHidesIndicator() {
        let zero = ConversationUsageSnapshot(inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheCreationTokens: 0)
        XCTAssertNil(ComposerContextUsage(usage: nil, capacity: 1_000))
        XCTAssertNil(ComposerContextUsage(usage: zero, capacity: nil))
        XCTAssertNil(ComposerContextUsage(usage: zero, capacity: 0))
        XCTAssertEqual(ComposerContextUsage(usage: zero, capacity: 1_000)?.percent, 0)
        let overflow = ConversationUsageSnapshot(inputTokens: .max, outputTokens: 0, cacheReadTokens: 1, cacheCreationTokens: 0)
        XCTAssertNil(ComposerContextUsage(usage: overflow, capacity: 1_000))
    }

    func testPercentRoundsAndClampsWithoutTruncatingTokenCount() {
        let usage = ConversationUsageSnapshot(inputTokens: 1_005, outputTokens: 0, cacheReadTokens: 0, cacheCreationTokens: 0)
        XCTAssertEqual(ComposerContextUsage(usage: usage, capacity: 1_000)?.percent, 100)
        XCTAssertEqual(ComposerContextUsage(usage: usage, capacity: 1_000)?.used, 1_005)
        XCTAssertEqual(ComposerContextUsage(usage: usage, capacity: 10_000)?.percent, 10)
    }
}
