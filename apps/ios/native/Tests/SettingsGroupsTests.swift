import XCTest
@testable import LingxiCode

final class SettingsGroupsTests: XCTestCase {
    func testRuntimeCapabilityDoesNotEnableTerminalBeforeRootfsIsReady() {
        var state = LinuxRuntimeState()
        state.available = true
        state.terminalSupported = true
        for status in [LinuxRuntimeRootfsState.missing, .corrupt, .installing, .unsupported] {
            state.rootfsState = status
            XCTAssertFalse(state.availableForExecution)
            XCTAssertFalse(state.canOpenTerminal)
        }
        state.rootfsState = .ready
        XCTAssertTrue(state.canOpenTerminal)
        state.available = false
        XCTAssertFalse(state.canOpenTerminal)
    }

    private var phoneEntries: [DesktopSettingsEntry] {
        DesktopSettingsEntry.all.filter(\.availableOnMobile)
    }

    func testEveryGroupListsAtLeastOnePage() {
        for group in SettingsGroup.allCases {
            XCTAssertFalse(
                phoneEntries.filter { $0.mobileGroup == group }.isEmpty,
                "\(group.rawValue) has no pages"
            )
        }
    }

    func testOnlyTheAccountPageLivesOutsideTheGroups() {
        XCTAssertEqual(phoneEntries.filter { $0.mobileGroup == nil }.map(\.id), ["account"])
    }

    func testAPageIsListedOnce() {
        let pages = phoneEntries.map(\.page)
        XCTAssertEqual(Set(pages).count, pages.count)
        let ids = DesktopSettingsEntry.all.map(\.id)
        XCTAssertEqual(Set(ids).count, ids.count)
    }

    func testUnavailablePagesAreNotOffered() {
        XCTAssertFalse(phoneEntries.contains { $0.id == "fusion" })
        // The old General hub is replaced by the groups.
        XCTAssertFalse(phoneEntries.contains { $0.id == "general" })
    }

    func testPagesFromTheGeneralHubAreStillReachable() {
        let ids = Set(phoneEntries.map(\.id))
        for id in [
            "language", "notifications", "input", "app-integration", "web-search", "web-fetch",
            "linux-runtime", "dream", "typescript-lsp", "privacy", "browser",
        ] {
            XCTAssertTrue(ids.contains(id), "\(id) is no longer reachable")
        }
    }

    func testSearchSpansGroupsAndMatchesKeys() {
        XCTAssertTrue(DesktopSettingsEntry.search("routing").contains { $0.id == "custom-providers" })
        XCTAssertTrue(DesktopSettingsEntry.search("siri").contains { $0.id == "app-integration" })
        XCTAssertTrue(DesktopSettingsEntry.search("dark").contains { $0.id == "appearance" })
        XCTAssertEqual(DesktopSettingsEntry.search("").count, phoneEntries.count)
    }

    func testBrowserSettingsAreDiscoverableAcrossBrowserNames() throws {
        let entry = try XCTUnwrap(phoneEntries.first { $0.id == "browser" })
        XCTAssertEqual(entry.page, .browser)
        XCTAssertEqual(entry.mobileGroup, .phone)
        for query in ["browser", "Safari", "Chrome", "Chromium", "Firefox", "浏览器", "extension"] {
            XCTAssertTrue(DesktopSettingsEntry.search(query).contains { $0.page == .browser }, query)
        }
    }

    func testStableAccessibilityIdentifiersSurvive() {
        let byID = Dictionary(uniqueKeysWithValues: DesktopSettingsEntry.all.map { ($0.id, $0) })
        XCTAssertEqual(byID["provider-credentials"]?.accessibilityID, "settings.provider.llm")
        XCTAssertEqual(byID["app-integration"]?.accessibilityID, "settings.appIntegration")
        XCTAssertEqual(byID["custom-providers"]?.accessibilityID, "settings.page.custom-providers")
    }
}
