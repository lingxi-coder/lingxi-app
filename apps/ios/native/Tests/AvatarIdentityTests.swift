import XCTest

@testable import LingxiCode

/// Spec §7.3 / §8.5: the colour of a person, their initials and the id a Bot's avatar is drawn from. The
/// vectors are the ones Desktop produced (`mobile-team-bot-vectors.json` › `avatars`), shared with Android.
/// The sub-agent index (`agentAvatarIndex`) is covered by `AgentAvatarTests`.
final class AvatarIdentityTests: XCTestCase {
    func testTheTwelveColoursAreTheSharedList() throws {
        let shared = try XCTUnwrap(MobileVectors.section("avatars", "colorList") as? [String])
        XCTAssertEqual(AvatarIdentity.avatarColors, shared)
        XCTAssertEqual(shared.count, 12)
    }

    func testFnvColourVectors() throws {
        let rows = try MobileVectors.rows("avatars", "color")
        XCTAssertEqual(rows.count, 6)
        for row in rows {
            let id = try XCTUnwrap(row["id"] as? String)
            XCTAssertEqual(Int(AvatarIdentity.fnv1a(id)), row["fnv1a"] as? Int, "fnv1a '\(id)'")
            XCTAssertEqual(AvatarIdentity.avatarColorIndex(id), row["index"] as? Int, "index '\(id)'")
            XCTAssertEqual(AvatarIdentity.avatarColorHex(id), row["color"] as? String, "colour '\(id)'")
        }
    }

    func testComponentsAreTheHexColourAsFractions() {
        // "u1" is index 3, #8a5a1f.
        let c = AvatarIdentity.avatarColorComponents("u1")
        XCTAssertEqual(c.red, 0x8a / 255.0, accuracy: 1e-9)
        XCTAssertEqual(c.green, 0x5a / 255.0, accuracy: 1e-9)
        XCTAssertEqual(c.blue, 0x1f / 255.0, accuracy: 1e-9)
    }

    func testInitialsVectors() throws {
        let rows = try MobileVectors.rows("avatars", "initials")
        XCTAssertEqual(rows.count, 10)
        for row in rows {
            let name = try XCTUnwrap(row["name"] as? String)
            XCTAssertEqual(AvatarIdentity.initials(name), row["expect"] as? String, "name '\(name)'")
        }
    }

    func testInitialsSplitOnJavaScriptWhitespaceAndTakeCodePoints() {
        // A no-break space (U+00A0) and an ideographic space (U+3000) separate words, as JavaScript's \s does.
        XCTAssertEqual(AvatarIdentity.initials("ann\u{00A0}lee"), "AL")
        XCTAssertEqual(AvatarIdentity.initials("ann\u{3000}lee"), "AL")
        // Three words still give two letters; a combining mark stays out (first code point only).
        XCTAssertEqual(AvatarIdentity.initials("a\u{0301}nn lee x"), "AL")
        // CJK is detected on the first code point of the first word only.
        XCTAssertEqual(AvatarIdentity.initials("张 伟"), "张")
        XCTAssertEqual(AvatarIdentity.initials("Zhang 伟"), "Z伟")
        XCTAssertEqual(AvatarIdentity.initials(""), "?")
        XCTAssertEqual(AvatarIdentity.initials("\u{FEFF}"), "?", "the BOM is whitespace in JavaScript")
    }

    func testTeamInitialIsTheFirstCodePointOfTheName() {
        XCTAssertEqual(AvatarIdentity.teamInitial("Acme Design"), "A")
        XCTAssertEqual(AvatarIdentity.teamInitial("  acme"), "A")
        XCTAssertEqual(AvatarIdentity.teamInitial("设计组"), "设")
        XCTAssertEqual(AvatarIdentity.teamInitial("🧠 lab"), "🧠")
        XCTAssertEqual(AvatarIdentity.teamInitial("   "), "?")
        XCTAssertEqual(AvatarIdentity.teamInitial(""), "?")
    }

    func testTeamBotAvatarIdVectors() throws {
        let rows = try XCTUnwrap(MobileVectors.section("avatars", "teamBotAvatarId") as? [[Any]])
        XCTAssertEqual(rows.count, 3)
        for row in rows {
            let id = try XCTUnwrap(row[0] as? String)
            XCTAssertEqual(AvatarIdentity.teamBotAvatarId(id), row[1] as? String, id)
        }
    }

    func testOnlyAKeyIsCutAtItsLastColon() {
        XCTAssertEqual(AvatarIdentity.teamBotAvatarId("bot:"), "")
        XCTAssertEqual(AvatarIdentity.teamBotAvatarId("a:b"), "a:b", "not a Bot key: used as it is")
        XCTAssertEqual(AvatarIdentity.teamBotAvatarId("botx:a:b"), "botx:a:b")
        XCTAssertEqual(AvatarIdentity.teamBotAvatarId("bot:o:h:搜索"), "搜索")
    }

    /// The card knows the bare id; before it loads the key stands in.
    func testABotCardDrawsFromItsBotIdElseFromItsKey() throws {
        let key = "bot:owner:host:helper"
        func card(botId: String) -> TeamBotCard {
            TeamBotCard(key: key, ownerUid: "owner", hostId: "host", botId: botId, name: "Helper", description: "", revision: 1,
                        use: "all", enabled: true, permitted: true, online: true)
        }
        XCTAssertEqual(card(botId: "helper").avatarId, "helper")
        XCTAssertEqual(card(botId: "").avatarId, "helper")
        // The same face as Desktop and the Bots tab: the sub-agent hash of the bare id.
        XCTAssertEqual(agentAvatarIndex(card(botId: "helper").avatarId), agentAvatarIndex(AvatarIdentity.teamBotAvatarId(key)))
    }
}
