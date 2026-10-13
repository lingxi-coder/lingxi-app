import XCTest
@testable import LingxiCode

final class SettingsGroupsTests: XCTestCase {
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
            "linux-runtime", "dream", "typescript-lsp", "privacy",
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

    func testStableAccessibilityIdentifiersSurvive() {
        let byID = Dictionary(uniqueKeysWithValues: DesktopSettingsEntry.all.map { ($0.id, $0) })
        XCTAssertEqual(byID["provider-credentials"]?.accessibilityID, "settings.provider.llm")
        XCTAssertEqual(byID["app-integration"]?.accessibilityID, "settings.appIntegration")
        XCTAssertEqual(byID["custom-providers"]?.accessibilityID, "settings.page.custom-providers")
    }
}
