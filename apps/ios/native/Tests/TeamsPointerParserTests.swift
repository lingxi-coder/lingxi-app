import XCTest

@testable import LingxiCode

/// Spec §8.4: the strict reader of a conversation's `lastMessage` pointer (and of the whole directory
/// document), against the 27 shared vectors (`lastMessage` in `mobile-team-bot-vectors.json`, produced by
/// running Desktop's `firestoreLastMessage`). Absent and explicit `null` are different: absent is invalid.
final class TeamsPointerParserTests: XCTestCase {
    /// Stands in for the Firestore `Timestamp`, which the parser only sees through the `millis` closure.
    private struct Stamp { let ms: Int64 }

    private func stamps(_ value: Any) -> Any {
        if let map = value as? [String: Any] {
            if map.count == 1, let iso = map["$ts"] as? String {
                let formatter = ISO8601DateFormatter()
                return Stamp(ms: Int64((formatter.date(from: iso)!.timeIntervalSince1970 * 1000).rounded()))
            }
            return map.mapValues(stamps)
        }
        if let list = value as? [Any] { return list.map(stamps) }
        return value
    }

    private let millis: (Any) -> Int64? = { ($0 as? Stamp)?.ms }

    private func parse(_ input: Any?) throws -> TeamLastMessage? {
        if let map = input as? [String: Any], map["$absent"] as? Bool == true {
            return try TeamsPointerParser.parseLastMessage(present: false, value: nil, millis: millis)
        }
        return try TeamsPointerParser.parseLastMessage(present: true, value: input.map(stamps), millis: millis)
    }

    func testTheSharedVectors() throws {
        let rows = try MobileVectors.rows("lastMessage")
        XCTAssertEqual(rows.count, 27)
        for row in rows {
            let name = row["name"] as? String ?? ""
            let expect = try XCTUnwrap(row["expect"] as? [String: Any], name)
            if expect.keys.contains("error") {
                XCTAssertThrowsError(try parse(row["input"]), name) { error in
                    XCTAssertEqual((error as? TeamMessageFailure)?.code, expect["error"] as? String, name)
                }
                continue
            }
            let result = try parse(row["input"])
            guard let ok = expect["ok"] as? [String: Any] else {
                XCTAssertNil(result, name)
                continue
            }
            XCTAssertEqual(result, TeamLastMessage(
                id: try XCTUnwrap(ok["id"] as? String), seq: try XCTUnwrap(ok["seq"] as? Int), atMs: (ok["atMs"] as? NSNumber)?.int64Value,
                authorKind: try XCTUnwrap(ok["authorKind"] as? String), authorUid: try XCTUnwrap(ok["authorUid"] as? String),
                authorBotKey: ok["authorBotKey"] as? String, preview: try XCTUnwrap(ok["preview"] as? String),
                recalled: try XCTUnwrap(ok["recalled"] as? Bool)), name)
        }
    }

    // MARK: Firestore-shaped values (Swift-native numbers and booleans, not parsed from JSON)

    private func pointer(_ changes: [String: Any?] = [:]) -> [String: Any] {
        var base: [String: Any] = ["id": "m1", "seq": 3, "at": Stamp(ms: 1_793_613_600_000), "author": ["kind": "user", "uid": "u2"], "preview": "hi"]
        for (key, value) in changes { base[key] = value ?? NSNull() }
        return base
    }

    func testFirestoreDoublesAndIntegersAreBothIntegers() throws {
        XCTAssertEqual(try TeamsPointerParser.parseLastMessage(present: true, value: pointer(["seq": 3.0]), millis: millis)?.seq, 3)
        XCTAssertEqual(try TeamsPointerParser.parseLastMessage(present: true, value: pointer(["seq": Int64(3)]), millis: millis)?.seq, 3)
        XCTAssertEqual(try TeamsPointerParser.parseLastMessage(present: true, value: pointer(["seq": NSNumber(value: 9_007_199_254_740_991)]), millis: millis)?.seq, 9_007_199_254_740_991)
        for bad: Any in [1.5, Double.nan, Double.infinity, true, "3", NSNumber(value: 9_007_199_254_740_992), NSNumber(value: UInt64.max), 9_007_199_254_740_992.0, NSNull()] {
            XCTAssertThrowsError(try TeamsPointerParser.parseLastMessage(present: true, value: pointer(["seq": bad]), millis: millis), "seq \(bad)")
        }
    }

    func testRecalledIsExactlyTrueOrAbsent() throws {
        XCTAssertTrue(try XCTUnwrap(TeamsPointerParser.parseLastMessage(present: true, value: pointer(["recalled": true]), millis: millis)).recalled)
        for bad: Any in [false, 1, 0, "true", NSNull()] {
            XCTAssertThrowsError(try TeamsPointerParser.parseLastMessage(present: true, value: pointer(["recalled": bad]), millis: millis), "recalled \(bad)")
        }
    }

    func testTheTimeIsRecognisedOnlyByTheCallersClosure() throws {
        // A string, a number and a map are not timestamps, whatever they look like.
        for bad: Any in ["2026-11-02T10:00:00Z", 1_793_613_600_000, ["seconds": 1]] {
            XCTAssertThrowsError(try TeamsPointerParser.parseLastMessage(present: true, value: pointer(["at": bad]), millis: millis))
        }
        XCTAssertNil(try XCTUnwrap(TeamsPointerParser.parseLastMessage(present: true, value: pointer(["at": nil]), millis: millis)).atMs)
    }

    func testPresentWithoutAValueIsNoMessage() throws {
        XCTAssertNil(try TeamsPointerParser.parseLastMessage(present: true, value: nil, millis: millis))
        XCTAssertNil(try TeamsPointerParser.parseLastMessage(present: true, value: NSNull(), millis: millis))
        XCTAssertThrowsError(try TeamsPointerParser.parseLastMessage(present: false, value: NSNull(), millis: millis))
    }

    // MARK: the directory document

    private func document(_ changes: [String: Any?] = [:]) -> [String: Any] {
        var base: [String: Any] = ["kind": "bot_dm", "title": "Helper", "participants": ["user:u1", "bot:t1:h:helper"], "lastSeq": 5, "lastChangeSeq": 7,
                                   "botKey": "bot:t1:h:helper", "lastMessage": pointer()]
        for (key, value) in changes { if let value { base[key] = value } else { base.removeValue(forKey: key) } }
        return base
    }

    func testAWholeDocumentBecomesTheAppConversation() throws {
        let value = try TeamsPointerParser.parseConversation(id: "c1", data: document(), lastReadSeq: 2, millis: millis)
        XCTAssertEqual(value, TeamMessageConversation(
            id: "c1", kind: "bot_dm", title: "Helper", participants: ["user:u1", "bot:t1:h:helper"], lastSeq: 5, lastChangeSeq: 7, lastReadSeq: 2,
            botKey: "bot:t1:h:helper", lastMessage: TeamLastMessage(id: "m1", seq: 3, atMs: 1_793_613_600_000, authorKind: "user", authorUid: "u2", authorBotKey: nil, preview: "hi", recalled: false)))
        XCTAssertEqual(value.unread, 3)
    }

    func testANullPointerIsAConversationWithoutMessages() throws {
        let value = try TeamsPointerParser.parseConversation(id: "c1", data: document(["lastMessage": NSNull(), "botKey": nil, "lastSeq": 0]), lastReadSeq: 0, millis: millis)
        XCTAssertNil(value.lastMessage)
        XCTAssertNil(value.botKey)
        XCTAssertEqual(value.effectiveBotKey, "bot:t1:h:helper", "falls back to the first bot: participant")
    }

    func testAnEmptyBotKeyIsNoBotKey() throws {
        XCTAssertNil(try TeamsPointerParser.parseConversation(id: "c", data: document(["botKey": ""]), lastReadSeq: 0, millis: millis).botKey)
        XCTAssertNil(try TeamsPointerParser.parseConversation(id: "c", data: document(["botKey": NSNull()]), lastReadSeq: 0, millis: millis).botKey)
    }

    func testAnyBrokenFieldIsInvalidData() {
        let broken: [[String: Any?]] = [
            ["lastMessage": nil],                      // absent: the field is always present
            ["lastMessage": "x"],
            ["kind": nil], ["kind": 4],
            ["title": nil], ["title": 5],
            ["participants": nil], ["participants": ["user:u1", 4]], ["participants": "user:u1"],
            ["lastSeq": nil], ["lastSeq": 1.5], ["lastSeq": "5"],
            ["lastChangeSeq": nil], ["lastChangeSeq": true],
            ["botKey": 7],
        ]
        for changes in broken {
            XCTAssertThrowsError(try TeamsPointerParser.parseConversation(id: "c", data: document(changes), lastReadSeq: 0, millis: millis), "\(changes)") { error in
                XCTAssertEqual((error as? TeamMessageFailure)?.code, "INVALID_DATA", "\(changes)")
            }
        }
    }
}
