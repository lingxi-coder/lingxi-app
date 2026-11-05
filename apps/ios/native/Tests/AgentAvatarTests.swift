import XCTest

@testable import LingxiCode

/// Spec §7.2 / §8.5: the avatar index is Desktop's `agentAvatarIndex`, which normalises the id to
/// `agent:<id>` before hashing its UTF-16 units. The vectors are shared with Android and Desktop.
final class AgentAvatarTests: XCTestCase {
    /// `packages/bridge-client/fixtures/agent-avatar-identities.json`: 34 `{id, index}` entries.
    func testDesktopIdentityFixture() throws {
        let entries = try XCTUnwrap(MobileVectors.json(MobileVectors.avatarIdentitiesFile) as? [[String: Any]])
        XCTAssertEqual(entries.count, 34)
        for entry in entries {
            let id = try XCTUnwrap(entry["id"] as? String)
            XCTAssertEqual(agentAvatarIndex(id), entry["index"] as? Int, "id '\(id)'")
        }
    }

    /// `mobile-team-bot-vectors.json` › `avatars.indexPrefixed`, including the `a` × 128 row.
    func testPrefixedIndexVectors() throws {
        let rows = try MobileVectors.rows("avatars", "indexPrefixed")
        XCTAssertEqual(rows.count, 8)
        for row in rows {
            var id = try XCTUnwrap(row["agentId"] as? String)
            if let repeated = row["repeat"] as? [String: Any],
               let char = repeated["char"] as? String, let count = repeated["count"] as? Int {
                id = String(repeating: char, count: count)
            }
            XCTAssertEqual(agentAvatarIndex(id), row["expect"] as? Int, "id '\(id.prefix(24))' (\(id.utf16.count) units)")
        }
    }

    func testTheNamedExamplesOfTheSpec() {
        let vectors: [(String, Int)] = [
            ("", 14), ("main", 1), ("agent-review-42", 26), ("搜索-agent", 2), ("🧠-review", 26),
            (String(repeating: "a", count: 128), 12),
            ("c0ffee00-1234-5678-9abc-def012345678", 14),
            ("agent:c0ffee00-1234-5678-9abc-def012345678", 14),
        ]
        for (id, expected) in vectors { XCTAssertEqual(agentAvatarIndex(id), expected, id) }
    }

    func testABareIdAndItsPrefixedFormShareOneFace() {
        for id in ["main", "c0ffee00-1234-5678-9abc-def012345678", "搜索-agent", ""] {
            XCTAssertEqual(agentAvatarIndex(id), agentAvatarIndex("agent:" + id), id)
        }
        // Only a leading "agent:" is normalised; the prefix inside an id is just text.
        XCTAssertNotEqual(agentAvatarIndex("x-agent:main"), agentAvatarIndex("main"))
    }

    /// Desktop's `startsWith` compares code units: `"agent:" + U+0301` already has the prefix, even though
    /// Swift's `hasPrefix` would see ":" + a combining mark as one different character.
    func testThePrefixTestComparesCodeUnitsLikeDesktop() {
        let id = "agent:\u{0301}x"
        XCTAssertFalse(id.hasPrefix("agent:"))
        XCTAssertEqual(agentAvatarIndex(id), hashIndex("agent:\u{0301}x"))
        XCTAssertNotEqual(hashIndex("agent:" + id), agentAvatarIndex(id), "must not be prefixed a second time")
    }

    func testIdentitiesStillCoverAllTwentyEightArtPairs() {
        XCTAssertEqual(Set((0..<300).map { agentAvatarIndex("agent-\($0)") }).count, 28)
    }

    /// Desktop's loop over a ready-made key, for the cases that must not be normalised again.
    private func hashIndex(_ key: String) -> Int {
        var hash: Int64 = 0
        for unit in key.utf16 { hash = (hash * 31 + Int64(unit)) % 2147483647 }
        return Int(hash % 28)
    }
}
