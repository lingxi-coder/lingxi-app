import XCTest

@testable import LingxiCode

/// Spec §8.6 shell logic against the shared vectors (`mobile.tabBarVisible`, `.tabForLink`,
/// `.tabBadgeText`, `.botsBadgeCount`, `.settingsJump`). Android runs the same rows in `TabLogicTest`.
final class TabLogicTests: XCTestCase {
    func testTabBarVisibleMatchesTheSharedRows() throws {
        let rows = try MobileVectors.rows("mobile", "tabBarVisible")
        XCTAssertEqual(rows.count, 8)
        for row in rows {
            let tab = try XCTUnwrap(AppTab(rawValue: row["selectedTab"] as? String ?? ""))
            let visible = TabLogic.tabBarVisible(
                wide: try XCTUnwrap(row["wide"] as? Bool),
                depth: try XCTUnwrap(row["depth"] as? Int),
                keyboardVisible: try XCTUnwrap(row["keyboardVisible"] as? Bool),
                drawerOpen: try XCTUnwrap(row["drawerOpen"] as? Bool),
                selectedTab: tab,
                overlayOpen: try XCTUnwrap(row["overlayOpen"] as? Bool)
            )
            XCTAssertEqual(visible, row["expect"] as? Bool, "\(row)")
        }
    }

    func testTheDrawerRuleOnlyAppliesToTheChatsTab() {
        for tab in AppTab.allCases {
            let visible = TabLogic.tabBarVisible(
                wide: false, depth: 0, keyboardVisible: false, drawerOpen: true, selectedTab: tab, overlayOpen: false
            )
            XCTAssertEqual(visible, tab != .chats, "\(tab)")
        }
    }

    func testAnOverlayHidesTheBarEvenOnWideLayouts() {
        XCTAssertFalse(TabLogic.tabBarVisible(
            wide: true, depth: 0, keyboardVisible: false, drawerOpen: false, selectedTab: .chats, overlayOpen: true
        ))
    }

    func testTabForLinkMatchesTheSharedRows() throws {
        let rows = try MobileVectors.rows("mobile", "tabForLink")
        XCTAssertEqual(rows.count, 21)
        for row in rows {
            let route = try XCTUnwrap(row["route"] as? String)
            XCTAssertEqual(TabLogic.tabForLink(route).rawValue, row["expect"] as? String, "route '\(route)'")
        }
    }

    func testLinkTargetsMapToTabs() {
        XCTAssertEqual(LinkTarget.chats.tab, .chats)
        XCTAssertEqual(LinkTarget.bots.tab, .bots)
        XCTAssertEqual(LinkTarget.teams.tab, .teams)
        XCTAssertEqual(LinkTarget.settings.tab, .settings)
        XCTAssertNil(LinkTarget.stay.tab)
        XCTAssertNil(LinkTarget.none.tab)
    }

    /// Every route the canonical parser accepts has a destination; a 'remote' link needs three parts.
    func testTabForLinkCoversTheCanonicalParserGrammar() throws {
        let urls: [(String, LinkTarget)] = [
            ("lingxi://session/abc", .chats),
            ("lingxi://new", .chats),
            ("lingxi://project/p1", .chats),
            ("lingxi://settings/account", .settings),
            ("lingxi://provider/p1", .settings),
            ("lingxi://extensions/mcp/x", .settings),
            ("lingxi://terminal/interactive", .chats),
            ("lingxi://apps/a1", .chats),
            ("lingxi://market/m1", .chats),
            ("lingxi://cron/job/j1", .chats),
            ("lingxi://cron/run/r1", .chats),
            ("lingxi://runtime/agent/r1", .chats),
            ("lingxi://remote/host1/session1", .bots),
            ("lingxi://team/inbox", .teams),
            ("lingxi://import/provider#payload=%7B%7D", .stay),
        ]
        for (raw, expected) in urls {
            let link = try XCTUnwrap(CanonicalDeepLink.parse(raw), raw)
            XCTAssertEqual(TabLogic.tabForLink(link.route), expected, raw)
        }
    }

    func testTabBadgeTextMatchesTheSharedRows() throws {
        let rows = try MobileVectors.rows("mobile", "tabBadgeText")
        XCTAssertEqual(rows.count, 7)
        for row in rows {
            let count = try XCTUnwrap(row["n"] as? Int)
            XCTAssertEqual(TabLogic.tabBadgeText(count), row["expect"] as? String, "n = \(count)")
        }
    }

    func testBotsBadgeCountSumsTheWaitingThreads() throws {
        let vector = try XCTUnwrap(try MobileVectors.section("mobile", "botsBadgeCount") as? [String: Any])
        let bots = try XCTUnwrap(vector["input"] as? [[String: Any]])
        let waiting = bots.map { $0["waitingThreads"] as? Int ?? 0 }
        XCTAssertEqual(TabLogic.botsBadgeCount(waitingThreads: waiting), vector["expect"] as? Int)
        XCTAssertEqual(TabLogic.botsBadgeCount(waitingThreads: [4, -2, 1]), 5, "negative counts are zero")
        XCTAssertEqual(TabLogic.botsBadgeCount(waitingThreads: []), 0)
    }

    func testWideBreakpointIsEightHundredFortyPoints() {
        XCTAssertFalse(TabLogic.isWide(width: 839.5))
        XCTAssertTrue(TabLogic.isWide(width: 840))
        XCTAssertTrue(TabLogic.isWide(width: 1366))
        XCTAssertFalse(TabLogic.isWide(width: 0))
    }

    func testSettingsJumpTracesMatchTheSharedRows() throws {
        let cases = try MobileVectors.rows("mobile", "settingsJump")
        XCTAssertEqual(cases.count, 8)
        for testCase in cases {
            let name = testCase["name"] as? String ?? "?"
            let trace = try XCTUnwrap(testCase["trace"] as? [[String: Any]], name)
            var state = SettingsJumpState<String>(root: "main")
            for (index, step) in trace.enumerated() {
                let event = try Self.event(try XCTUnwrap(step["event"] as? [String: Any], name))
                let result = state.reduced(by: event)
                state = result.state
                let context = "\(name) #\(index + 1)"
                XCTAssertEqual(state.stack, step["stack"] as? [String], "\(context): stack")
                XCTAssertEqual(state.origin?.rawValue, step["origin"] as? String, "\(context): origin")
                XCTAssertEqual(result.leaveTo?.rawValue, step["leaveTo"] as? String, "\(context): leaveTo")
            }
        }
    }

    func testReducedDoesNotMutateItsReceiver() {
        let state = SettingsJumpState<String>(root: "main", stack: ["account"], origin: .chats)
        let result = state.reduced(by: .back)
        XCTAssertEqual(state.stack, ["account"])
        XCTAssertEqual(state.origin, .chats)
        XCTAssertEqual(result.leaveTo, .chats)
        XCTAssertEqual(result.state.stack, [])
        XCTAssertNil(result.state.origin)
    }

    private static func event(_ raw: [String: Any]) throws -> SettingsJumpState<String>.Event {
        let origin = (raw["origin"] as? String).flatMap(AppTab.init(rawValue:))
        switch raw["type"] as? String {
        case "jump": return .jump(try XCTUnwrap(raw["page"] as? String), origin: origin)
        case "push": return .push(try XCTUnwrap(raw["page"] as? String))
        case "back": return .back
        case "userSelectedTab": return .userSelectedTab
        default: throw MobileVectors.VectorError.missing("settingsJump event \(raw)")
        }
    }
}
