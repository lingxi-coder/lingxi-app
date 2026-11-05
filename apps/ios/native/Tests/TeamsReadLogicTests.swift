import XCTest

@testable import LingxiCode

/// Spec §6.3 (D12) and §8.6: when the app marks a conversation read by itself, against the shared vectors
/// (`mobile.autoMarkRead` in `mobile-team-bot-vectors.json`).
final class TeamsReadLogicTests: XCTestCase {
    func testTheDefaultThresholdIsOneSecond() {
        XCTAssertEqual(TeamsReadLogic.autoMarkReadMs, 1000)
    }

    func testAutoMarkRead() throws {
        let rows = try MobileVectors.rows("mobile", "autoMarkRead")
        XCTAssertEqual(rows.count, 10)
        for row in rows {
            let input = try XCTUnwrap(row["input"] as? [String: Any])
            let result = TeamsReadLogic.autoMarkReadSeq(
                online: try XCTUnwrap(input["online"] as? Bool), fromCache: try XCTUnwrap(input["fromCache"] as? Bool), foreground: try XCTUnwrap(input["foreground"] as? Bool),
                atBottomMs: (input["atBottomMs"] as? NSNumber)?.int64Value, newestRenderedSeq: try XCTUnwrap(input["newestRenderedSeq"] as? Int),
                lastReadSeq: try XCTUnwrap(input["lastReadSeq"] as? Int), thresholdMs: (input["thresholdMs"] as? NSNumber)?.int64Value)
            XCTAssertEqual(result, row["expect"] as? Int, row["name"] as? String ?? "")
        }
    }

    func testTheDefaultThresholdAppliesWhenNoneIsGiven() {
        XCTAssertEqual(TeamsReadLogic.autoMarkReadSeq(online: true, fromCache: false, foreground: true, atBottomMs: 1000, newestRenderedSeq: 4, lastReadSeq: 3), 4)
        XCTAssertNil(TeamsReadLogic.autoMarkReadSeq(online: true, fromCache: false, foreground: true, atBottomMs: 999, newestRenderedSeq: 4, lastReadSeq: 3))
    }
}
