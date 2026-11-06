import XCTest

@testable import LingxiCode

/// Spec §8.3: the send limit, the `@` picker, snippets and the composer lock, against the shared vectors
/// (`compose` in `mobile-team-bot-vectors.json`, produced by running Desktop's `compose.ts`; the lock cases
/// mirror `Composer.tsx` by hand). Indices are UTF-16 code-unit offsets, snippets count code points, the send
/// limit counts UTF-8 bytes.
final class TeamsComposeLogicTests: XCTestCase {
    /// The vectors write `<unit>×<N>` for N repetitions of the code point before the ×, optionally followed by
    /// more text (`x×90…`).
    private func expand(_ text: String) -> String {
        guard let times = text.firstIndex(of: "×"), times > text.startIndex else { return text }
        let unit = text[text.index(before: times)]
        let head = String(text[..<text.index(before: times)])
        let afterTimes = text[text.index(after: times)...]
        let digits = afterTimes.prefix { $0.isNumber }
        guard let count = Int(digits) else { return text }
        return head + String(repeating: unit, count: count) + afterTimes.dropFirst(digits.count)
    }

    private func vectors<T>(_ key: String, as _: T.Type = T.self) throws -> T {
        try XCTUnwrap(MobileVectors.section("compose", key) as? T)
    }

    private func cards() throws -> [TeamBotCard] {
        try vectors("botCards", as: [[String: Any]].self).map {
            try JSONDecoder().decode(TeamBotCard.self, from: JSONSerialization.data(withJSONObject: $0))
        }
    }

    func testTheConstants() throws {
        XCTAssertEqual(TeamsComposeLogic.messageBytes, try vectors("messageBytes", as: Int.self))
        XCTAssertEqual(TeamsComposeLogic.maxTargets, try vectors("maxTargets", as: Int.self))
    }

    func testCanSendText() throws {
        let rows = try vectors("canSendText", as: [[String: Any]].self)
        XCTAssertEqual(rows.count, 6)
        for row in rows {
            let text = expand(try XCTUnwrap(row["text"] as? String))
            XCTAssertEqual(TeamsComposeLogic.canSendText(text), row["expect"] as? Bool, "\(String(text.prefix(12))) (\(text.utf8.count) bytes)")
        }
    }

    func testByteLength() throws {
        let rows = try vectors("byteLength", as: [[Any]].self)
        XCTAssertEqual(rows.count, 3)
        for row in rows {
            XCTAssertEqual(TeamsComposeLogic.byteLength(try XCTUnwrap(row[0] as? String)), row[1] as? Int)
        }
    }

    func testTheLimitMeasuresTheUntrimmedText() {
        let padded = String(repeating: " ", count: 8190) + "ab" + "  "
        XCTAssertFalse(TeamsComposeLogic.canSendText(padded), "8194 bytes with the spaces")
        XCTAssertTrue(TeamsComposeLogic.canSendText(String(repeating: " ", count: 8190) + "ab"))
        XCTAssertFalse(TeamsComposeLogic.canSendText("\u{00A0}\u{3000}\n\t"), "JavaScript whitespace is blank")
    }

    func testMentionAt() throws {
        let rows = try vectors("mentionAt", as: [[String: Any]].self)
        XCTAssertEqual(rows.count, 8)
        for row in rows {
            let text = try XCTUnwrap(row["text"] as? String)
            let result = TeamsComposeLogic.mentionAt(text, caret: try XCTUnwrap(row["caret"] as? Int))
            if let expected = row["expect"] as? [String: Any] {
                XCTAssertEqual(result, MentionAt(start: try XCTUnwrap(expected["start"] as? Int), query: try XCTUnwrap(expected["query"] as? String)), "'\(text)'")
            } else {
                XCTAssertNil(result, "'\(text)'")
            }
        }
    }

    func testMentionAtEdges() {
        // The query is capped at 40 characters; 41 is no mention (the @ must directly precede the query).
        XCTAssertEqual(TeamsComposeLogic.mentionAt("@" + String(repeating: "a", count: 40), caret: 41)?.query.count, 40)
        XCTAssertNil(TeamsComposeLogic.mentionAt("@" + String(repeating: "a", count: 41), caret: 42))
        // A second @ ends the run: "a@b" has no mention opener before the @.
        XCTAssertNil(TeamsComposeLogic.mentionAt("@a@b", caret: 4))
        XCTAssertEqual(TeamsComposeLogic.mentionAt("x @a @b", caret: 7), MentionAt(start: 5, query: "b"))
        // Only the text before the caret counts, and a caret past the end is the end.
        XCTAssertEqual(TeamsComposeLogic.mentionAt("hi @he there", caret: 6), MentionAt(start: 3, query: "he"))
        XCTAssertEqual(TeamsComposeLogic.mentionAt("hi @he", caret: 99), MentionAt(start: 3, query: "he"))
        XCTAssertNil(TeamsComposeLogic.mentionAt("hi @he", caret: 0))
        // Offsets are UTF-16 code units: the brain emoji is two, but one scalar in the 40 limit.
        XCTAssertEqual(TeamsComposeLogic.mentionAt("🧠 @x", caret: 5), MentionAt(start: 3, query: "x"))
        XCTAssertEqual(TeamsComposeLogic.mentionAt("@🧠", caret: 3), MentionAt(start: 0, query: "🧠"))
        XCTAssertEqual(TeamsComposeLogic.mentionAt("a\n@b", caret: 4), MentionAt(start: 2, query: "b"), "a newline opens a mention")
        XCTAssertEqual(TeamsComposeLogic.mentionAt("a\u{3000}@b", caret: 4), MentionAt(start: 2, query: "b"), "so does an ideographic space")
    }

    func testWithoutMention() throws {
        let rows = try vectors("withoutMention", as: [[String: Any]].self)
        XCTAssertEqual(rows.count, 4)
        for row in rows {
            let text = try XCTUnwrap(row["text"] as? String)
            XCTAssertEqual(TeamsComposeLogic.withoutMention(text, start: try XCTUnwrap(row["start"] as? Int), caret: try XCTUnwrap(row["caret"] as? Int)), row["expect"] as? String, "'\(text)'")
        }
        XCTAssertEqual(TeamsComposeLogic.withoutMention("🧠 @x ok", start: 3, caret: 5), "🧠 ok", "UTF-16 offsets")
    }

    func testSnippet() throws {
        let rows = try vectors("snippet", as: [[String: Any]].self)
        XCTAssertEqual(rows.count, 4)
        for row in rows {
            let text = expand(try XCTUnwrap(row["text"] as? String))
            XCTAssertEqual(TeamsComposeLogic.snippet(text, length: try XCTUnwrap(row["length"] as? Int)), expand(try XCTUnwrap(row["expect"] as? String)), String(text.prefix(12)))
        }
        XCTAssertEqual(TeamsComposeLogic.snippet("short"), "short")
        XCTAssertEqual(TeamsComposeLogic.snippet(String(repeating: "a", count: 90)), String(repeating: "a", count: 90), "exactly the length: no ellipsis")
    }

    func testMentionCandidates() throws {
        let cards = try cards()
        let rows = try vectors("mentionCandidates", as: [[String: Any]].self)
        XCTAssertEqual(rows.count, 5)
        for row in rows {
            let conversation = TeamMessageConversation(id: "x", kind: try XCTUnwrap(row["kind"] as? String), title: "", participants: try XCTUnwrap(row["participants"] as? [String]),
                                                       lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
            let chosen = try XCTUnwrap(row["chosen"] as? [String])
            let result = TeamsComposeLogic.mentionCandidates(bots: cards, conversation: conversation, chosenKeys: chosen, query: try XCTUnwrap(row["query"] as? String))
            XCTAssertEqual(result.map(\.sharedBotId), row["expect"] as? [String], "\(row)")
        }
    }

    func testComposerLock() throws {
        let cards = try cards()
        let rows = try vectors("composerLock", as: [[String: Any]].self)
        XCTAssertEqual(rows.count, 9)
        let names: [String: ComposerLock] = ["none": .none, "ownerReading": .ownerReading, "botUnverified": .botUnverified, "botChecking": .botChecking, "botUnavailable": .botUnavailable]
        for row in rows {
            let known = try XCTUnwrap(row["bots"] as? [String])
            let result = TeamsComposeLogic.composerLock(
                uid: try XCTUnwrap(row["uid"] as? String), kind: try XCTUnwrap(row["kind"] as? String), participants: try XCTUnwrap(row["participants"] as? [String]),
                bots: cards.filter { known.contains($0.sharedBotId) }, botsLoaded: try XCTUnwrap(row["botsLoaded"] as? Bool), botsUnverified: try XCTUnwrap(row["botsUnverified"] as? Bool))
            XCTAssertEqual(result, names[try XCTUnwrap(row["expect"] as? String)], "\(row)")
        }
    }

    func testSearchIgnoresCaseAndSurroundingSpaceInTheBotName() throws {
        let cards = try cards()
        let group = TeamMessageConversation(id: "x", kind: "group", title: "", participants: ["user:u1", "shared_bot:sharedbot_a", "shared_bot:sharedbot_d"], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
        XCTAssertEqual(TeamsComposeLogic.mentionCandidates(bots: cards, conversation: group, chosenKeys: [String](), query: "  DEL ").map(\.sharedBotId), ["sharedbot_d"])
        XCTAssertEqual(TeamsComposeLogic.mentionCandidates(bots: cards, conversation: group, chosenKeys: Set(["sharedbot_a", "sharedbot_d"]), query: "").map(\.sharedBotId), [])
    }
}
