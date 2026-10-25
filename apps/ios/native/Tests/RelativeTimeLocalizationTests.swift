import Foundation
import XCTest

@testable import LingxiCode

final class RelativeTimeLocalizationTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_772_352_000)

    func testRelativeTimeUsesRequestedLanguageAndSingularUnits() {
        let timestamp = ISO8601DateFormatter().string(from: now.addingTimeInterval(-3_600))
        XCTAssertEqual(RelativeTime.format(timestamp, now: now, locale: Locale(identifier: "en")), "1 hour ago")
        XCTAssertEqual(RelativeTime.format(timestamp, now: now, locale: Locale(identifier: "ja")), "1 時間前")
        XCTAssertEqual(RelativeTime.format(timestamp, now: now, locale: Locale(identifier: "ko")), "1시간 전")
    }

    func testOlderDatesUseTheRequestedLanguage() {
        let timestamp = "2026-02-14T12:00:00Z"
        XCTAssertTrue(RelativeTime.format(timestamp, now: now, locale: Locale(identifier: "en")).contains("Feb"))
        XCTAssertTrue(RelativeTime.format(timestamp, now: now, locale: Locale(identifier: "ja")).contains("2月"))
    }

    func testInvalidTimestampPreservesFallback() {
        XCTAssertEqual(RelativeTime.format("invalid", now: now), "invalid")
        XCTAssertEqual(RelativeTime.format("", now: now), "—")
    }
}
