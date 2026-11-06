import XCTest

@testable import LingxiCode

/// Spec §8.2: the rows of a message stream (date separators, the "new messages" divider, author grouping),
/// the rendered window and the time classes of the list, against the shared vectors (`stream`,
/// `visibleStart`, `listTimeKinds`, `dayLabelKinds`) produced by running Desktop's `stream.ts`.
final class TeamsStreamLogicTests: XCTestCase {
    private let utc = TimeZone(identifier: "UTC")!

    private func meta<T>(_ key: String, as _: T.Type = T.self) throws -> T {
        let document = try MobileVectors.document()
        return try XCTUnwrap((try XCTUnwrap(document["_meta"] as? [String: Any]))[key] as? T)
    }

    private func messages(_ rows: [[String: Any]]) throws -> [StreamMessage] {
        try rows.map {
            StreamMessage(id: try XCTUnwrap($0["id"] as? String), seq: try XCTUnwrap($0["seq"] as? Int),
                          authorKind: try XCTUnwrap($0["authorKind"] as? String), authorUid: try XCTUnwrap($0["authorUid"] as? String),
                          authorSharedBotId: $0["authorSharedBotId"] as? String, createdAtMs: ($0["createdAtMs"] as? NSNumber)?.int64Value,
                          recalled: $0["recalled"] as? Bool ?? false)
        }
    }

    /// "date:YYYY-MM-DD:today|yesterday|other", "unread", "message:<id>:head|grouped" (the vectors' encoding).
    private func encode(_ rows: [StreamRow]) -> [String] {
        rows.map {
            switch $0 {
            case let .date(key, _, kind): key + ":" + (kind == .today ? "today" : kind == .yesterday ? "yesterday" : "other")
            case .unread: "unread"
            case let .message(message, grouped): "message:\(message.id):" + (grouped ? "grouped" : "head")
            }
        }
    }

    private func rows(_ list: [StreamMessage], lastReadSeq: Int, uid: String = "u1") throws -> [String] {
        encode(TeamsStreamLogic.buildStreamRows(list, lastReadSeq: lastReadSeq, uid: uid, nowMs: try meta("nowMs"), zone: utc))
    }

    func testTheMainStreamAndItsAllReadVariant() throws {
        let stream = try XCTUnwrap(MobileVectors.section("stream") as? [String: Any])
        let list = try messages(try XCTUnwrap(stream["messages"] as? [[String: Any]]))
        XCTAssertEqual(list.count, 10)
        XCTAssertEqual(try rows(list, lastReadSeq: try XCTUnwrap(stream["lastReadSeq"] as? Int)), stream["expectRows"] as? [String])
        XCTAssertEqual(try rows(list, lastReadSeq: 10), stream["expectRowsAllRead"] as? [String])
        XCTAssertEqual(TeamsStreamLogic.firstUnreadMessage(list, lastReadSeq: 4, uid: "u1")?.id, stream["firstUnreadId"] as? String)
        XCTAssertEqual(TeamsStreamLogic.groupWindowMs, (stream["groupWindowMs"] as? NSNumber)?.int64Value)
    }

    func testOnlyOwnMessagesNeverAnchorTheDivider() throws {
        let own = try XCTUnwrap((try XCTUnwrap(MobileVectors.section("stream") as? [String: Any]))["ownOnly"] as? [String: Any])
        let list = try messages(try XCTUnwrap(own["messages"] as? [[String: Any]]))
        XCTAssertEqual(try rows(list, lastReadSeq: try XCTUnwrap(own["lastReadSeq"] as? Int)), own["expectRows"] as? [String])
        XCTAssertNil(TeamsStreamLogic.firstUnreadMessage(list, lastReadSeq: 0, uid: "u1"))
    }

    func testMoreStreamCases() throws {
        let stream = try XCTUnwrap(MobileVectors.section("stream") as? [String: Any])
        let cases = try XCTUnwrap(stream["more"] as? [[String: Any]])
        XCTAssertGreaterThanOrEqual(cases.count, 6)
        for row in cases {
            let list = try messages(try XCTUnwrap(row["messages"] as? [[String: Any]]))
            XCTAssertEqual(try rows(list, lastReadSeq: try XCTUnwrap(row["lastReadSeq"] as? Int), uid: row["uid"] as? String ?? "u1"), row["expectRows"] as? [String], row["name"] as? String ?? "")
        }
    }

    func testAnEmptyConversationHasNoRows() throws {
        XCTAssertEqual(try rows([], lastReadSeq: 0), [])
    }

    // MARK: window

    /// "n1..n150 (150, alternating)" = n1..n150, odd seq by u2 and even by u1; "c1..c600 (600, all u2)" = c1..c600 by u2.
    private func windowMessages(_ description: String) throws -> [StreamMessage] {
        let pattern = try NSRegularExpression(pattern: #"^([a-z])1\.\.\1(\d+) \((\d+), (alternating|all u2)\)$"#)
        let range = NSRange(description.startIndex..., in: description)
        let match = try XCTUnwrap(pattern.firstMatch(in: description, range: range), description)
        func group(_ index: Int) -> String { String(description[Range(match.range(at: index), in: description)!]) }
        let prefix = group(1), count = try XCTUnwrap(Int(group(2))), alternating = group(4) == "alternating"
        XCTAssertEqual(Int(group(3)), count, description)
        return (1...count).map { seq in
            StreamMessage(id: "\(prefix)\(seq)", seq: seq, authorKind: "user", authorUid: alternating && seq % 2 == 0 ? "u1" : "u2", createdAtMs: nil)
        }
    }

    func testVisibleStart() throws {
        let section = try XCTUnwrap(MobileVectors.section("visibleStart") as? [String: Any])
        let constants = try XCTUnwrap(section["constants"] as? [String: Int])
        XCTAssertEqual(TeamsStreamLogic.windowSize, constants["WINDOW_SIZE"])
        XCTAssertEqual(TeamsStreamLogic.windowMax, constants["WINDOW_MAX"])
        XCTAssertEqual(TeamsStreamLogic.unreadContext, constants["UNREAD_CONTEXT"])
        let cases = try XCTUnwrap(section["cases"] as? [[String: Any]])
        XCTAssertEqual(cases.count, 6)
        for row in cases {
            let list = try windowMessages(try XCTUnwrap(row["messages"] as? String))
            let start = TeamsStreamLogic.visibleStart(list, startId: row["startId"] as? String, lastReadSeq: try XCTUnwrap(row["lastReadSeq"] as? Int), uid: "u1")
            XCTAssertEqual(start, row["expect"] as? Int, "\(row)")
        }
    }

    func testAskingForEarlierMessagesExtendsPastTheCapAndNeverBelowZero() throws {
        let list = try windowMessages("c1..c600 (600, all u2)")
        XCTAssertEqual(TeamsStreamLogic.visibleStart(list, startId: "c10", lastReadSeq: 600, uid: "u1"), 9, "the reader's request has no cap")
        XCTAssertEqual(TeamsStreamLogic.visibleStart(list, startId: "c600", lastReadSeq: 600, uid: "u1"), 500, "later than the window: unchanged")
        XCTAssertEqual(TeamsStreamLogic.visibleStart(list, startId: "gone", lastReadSeq: 600, uid: "u1"), 500, "an unknown id is ignored")
        XCTAssertEqual(TeamsStreamLogic.visibleStart([], startId: nil, lastReadSeq: 0, uid: "u1"), 0)
        XCTAssertEqual(TeamsStreamLogic.visibleStart(list, startId: nil, lastReadSeq: 0, uid: "u1", size: 10, max: 50), 550, "max bounds the widening")
    }

    // MARK: time classes

    func testListTimeKinds() throws {
        let nowMs: Int64 = try meta("nowMs")
        let rows = try MobileVectors.rows("listTimeKinds")
        XCTAssertEqual(rows.count, 9)
        for row in rows {
            let at = TeamsStreamLogic.parseInstant(row["at"] as? String)
            XCTAssertEqual(name(TeamsStreamLogic.listTimeKind(atMs: at, nowMs: nowMs, zone: utc)), row["expect"] as? String, "\(row)")
        }
    }

    func testDayLabelKinds() throws {
        let nowMs: Int64 = try meta("nowMs")
        let rows = try MobileVectors.rows("dayLabelKinds")
        XCTAssertEqual(rows.count, 4)
        for row in rows {
            let at = try XCTUnwrap(TeamsStreamLogic.parseInstant(try XCTUnwrap(row["at"] as? String)))
            XCTAssertEqual(name(TeamsStreamLogic.dayLabelKind(atMs: at, nowMs: nowMs, zone: utc)), row["expect"] as? String, "\(row)")
        }
    }

    private func name(_ kind: ListTimeKind) -> String {
        switch kind {
        case .none: "none"
        case .clock: "clock"
        case .yesterday: "yesterday"
        case .weekday: "weekday"
        case .dateThisYear: "dateThisYear"
        case .dateOtherYear: "dateOtherYear"
        }
    }

    private func name(_ kind: DayKind) -> String {
        switch kind {
        case .today: "today"
        case .yesterday: "yesterday"
        case .dateThisYear: "dateThisYear"
        case .dateOtherYear: "dateOtherYear"
        }
    }

    func testDayBoundariesFollowTheViewersZoneNotUTC() throws {
        let shanghai = try XCTUnwrap(TimeZone(identifier: "Asia/Shanghai"))
        let losAngeles = try XCTUnwrap(TimeZone(identifier: "America/Los_Angeles"))
        // 2026-11-01T17:00Z is already 11-02 01:00 in Shanghai (today there), but 11-01 10:00 in Los Angeles (yesterday).
        let at = try XCTUnwrap(TeamsStreamLogic.parseInstant("2026-11-01T17:00:00Z"))
        let now = try XCTUnwrap(TeamsStreamLogic.parseInstant("2026-11-02T10:00:00Z"))
        XCTAssertEqual(TeamsStreamLogic.dayLabelKind(atMs: at, nowMs: now, zone: shanghai), .today)
        XCTAssertEqual(TeamsStreamLogic.dayLabelKind(atMs: at, nowMs: now, zone: losAngeles), .yesterday)
        XCTAssertEqual(TeamsStreamLogic.listTimeKind(atMs: at, nowMs: now, zone: shanghai), .clock)
        // The date row key is the viewer's local day.
        let message = StreamMessage(id: "a", seq: 1, authorKind: "user", authorUid: "u2", createdAtMs: at)
        let inShanghai = TeamsStreamLogic.buildStreamRows([message], lastReadSeq: 1, uid: "u1", nowMs: now, zone: shanghai)
        XCTAssertEqual(inShanghai.first?.key, "date:2026-11-02")
        let inLosAngeles = TeamsStreamLogic.buildStreamRows([message], lastReadSeq: 1, uid: "u1", nowMs: now, zone: losAngeles)
        XCTAssertEqual(inLosAngeles.first?.key, "date:2026-11-01")
    }

    func testTheYearRolloverIsAYesterdayAndAFutureTimeIsADate() throws {
        let now = try XCTUnwrap(TeamsStreamLogic.parseInstant("2026-01-01T09:00:00Z"))
        let lastNight = try XCTUnwrap(TeamsStreamLogic.parseInstant("2025-12-31T23:30:00Z"))
        XCTAssertEqual(TeamsStreamLogic.listTimeKind(atMs: lastNight, nowMs: now, zone: utc), .yesterday)
        XCTAssertEqual(TeamsStreamLogic.dayLabelKind(atMs: lastNight, nowMs: now, zone: utc), .yesterday)
        let earlier = try XCTUnwrap(TeamsStreamLogic.parseInstant("2025-12-20T12:00:00Z"))
        XCTAssertEqual(TeamsStreamLogic.listTimeKind(atMs: earlier, nowMs: now, zone: utc), .dateOtherYear)
        let future = try XCTUnwrap(TeamsStreamLogic.parseInstant("2026-03-05T12:00:00Z"))
        XCTAssertEqual(TeamsStreamLogic.listTimeKind(atMs: future, nowMs: now, zone: utc), .dateThisYear)
    }

    // MARK: ISO-8601 instants

    func testParseInstant() {
        XCTAssertEqual(TeamsStreamLogic.parseInstant("2026-11-02T10:00:00Z"), 1_793_613_600_000)
        XCTAssertEqual(TeamsStreamLogic.parseInstant("2026-11-02T10:00:00.5Z"), 1_793_613_600_500)
        XCTAssertEqual(TeamsStreamLogic.parseInstant("2026-11-02T10:00:00.123456Z"), 1_793_613_600_123)
        XCTAssertEqual(TeamsStreamLogic.parseInstant("2026-11-02T18:00:00+08:00"), 1_793_613_600_000)
        XCTAssertEqual(TeamsStreamLogic.parseInstant("2026-11-02T05:30:00-04:30"), 1_793_613_600_000)
        XCTAssertEqual(TeamsStreamLogic.parseInstant("1970-01-01T00:00:00Z"), 0)
        XCTAssertEqual(TeamsStreamLogic.parseInstant("1969-12-31T23:59:59.999Z"), -1)
        XCTAssertEqual(TeamsStreamLogic.parseInstant("2024-02-29T00:00:00Z"), 1_709_164_800_000, "a leap day")
        for bad in ["nope", "", "2026-11-02", "2026-11-02T10:00:00", "2026-13-02T10:00:00Z", "2026-02-30T10:00:00Z", "2025-02-29T10:00:00Z",
                    "2026-11-02T25:00:00Z", "2026-11-02T10:00:00.Z", "2026-11-02T10:00:00Zjunk", "2026-11-02 10:00:00Z"] {
            XCTAssertNil(TeamsStreamLogic.parseInstant(bad), bad)
        }
        XCTAssertNil(TeamsStreamLogic.parseInstant(nil))
    }

    func testAMessageTimeIsItsCreatedAtString() throws {
        let json = #"{"id":"m","seq":1,"author":{"kind":"user","uid":"u2"},"kind":"text","text":"hi","createdAt":"2026-11-02T10:00:00.250Z","changeSeq":1,"changeId":"c"}"#
        let message = try JSONDecoder().decode(TeamMessage.self, from: Data(json.utf8))
        XCTAssertEqual(message.createdAtMs, 1_793_613_600_250)
        let stream = StreamMessage(message)
        XCTAssertEqual(stream, StreamMessage(id: "m", seq: 1, authorKind: "user", authorUid: "u2", authorSharedBotId: nil, createdAtMs: 1_793_613_600_250, recalled: false))
        let unstamped = try JSONDecoder().decode(TeamMessage.self, from: Data(json.replacingOccurrences(of: #""2026-11-02T10:00:00.250Z""#, with: "null").utf8))
        XCTAssertNil(unstamped.createdAtMs)
    }

    func testABotMessageCarriesItsKeyIntoTheGroupingIdentity() throws {
        let json = #"{"id":"b","seq":2,"author":{"kind":"shared_bot","uid":"o","sharedBotId":"sharedbot_x"},"kind":"text","text":"","recalled":true,"changeSeq":1,"changeId":"c"}"#
        let message = try JSONDecoder().decode(TeamMessage.self, from: Data(json.utf8))
        let stream = StreamMessage(message)
        XCTAssertEqual(stream.authorSharedBotId, "sharedbot_x")
        XCTAssertTrue(stream.recalled)
        XCTAssertNil(stream.createdAtMs)
    }
}
