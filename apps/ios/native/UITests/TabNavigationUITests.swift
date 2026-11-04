import UIKit
import XCTest

/// The four shell tabs as UI tests address them. The app sets the `tab.<name>` accessibility identifier on
/// each `Tab`; if the system tab bar does not surface it, the bar's buttons are found by position.
enum ShellTab: String, CaseIterable {
    case chats, bots, teams, settings

    var identifier: String { "tab.\(rawValue)" }
    var index: Int { Self.allCases.firstIndex(of: self) ?? 0 }
}

extension XCUIApplication {
    /// The system tab bar button for `tab`.
    func tabButton(_ tab: ShellTab) -> XCUIElement {
        let byIdentifier = tabBars.buttons[tab.identifier]
        return byIdentifier.exists ? byIdentifier : tabBars.buttons.element(boundBy: tab.index)
    }

    /// Whether the system tab bar is on screen and usable (it is hidden on pushed screens, with the
    /// keyboard, under the open drawer and under overlays).
    var tabBarIsShown: Bool {
        let bar = tabBars.firstMatch
        return bar.exists && bar.isHittable
    }

    /// Taps `tab` in the bar once it is on screen, and returns its button.
    @discardableResult
    func selectTab(_ tab: ShellTab, timeout: TimeInterval = 20, file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let button = tabButton(tab)
        XCTAssertTrue(button.waitForExistence(timeout: timeout), "tab bar button \(tab.identifier): \(debugDescription)", file: file, line: line)
        let hittable = XCTNSPredicateExpectation(predicate: NSPredicate(format: "hittable == true"), object: button)
        XCTAssertEqual(XCTWaiter.wait(for: [hittable], timeout: 10), .completed, "\(tab.identifier) is not hittable", file: file, line: line)
        button.tap()
        return button
    }

    /// Replaces the old `drawer.settings` tap: the gear left the drawer, Settings is a tab.
    func openSettingsTab(timeout: TimeInterval = 20, file: StaticString = #filePath, line: UInt = #line) {
        selectTab(.settings, timeout: timeout, file: file, line: line)
    }

    /// Replaces the old `drawer.remote-control` tap: Remote Control is the Bots tab.
    func openBotsTab(timeout: TimeInterval = 20, file: StaticString = #filePath, line: UInt = #line) {
        selectTab(.bots, timeout: timeout, file: file, line: line)
    }

    /// Waits until the bar's visibility is `shown` (it animates in and out).
    func waitForTabBar(shown: Bool, timeout: TimeInterval = 8) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            if tabBarIsShown == shown { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        } while Date() < deadline
        return tabBarIsShown == shown
    }
}

/// Shell behaviour of spec §2 / §13.2 (device or simulator; written against the system `TabView`).
/// The identifiers of the bar buttons and the re-tap behaviour are the two things SwiftUI does not
/// document: they are what these tests verify first on a device.
final class TabNavigationUITests: XCTestCase {
    private var app: XCUIApplication!
    private var isPhone: Bool { UIDevice.current.userInterfaceIdiom == .phone }

    override func setUpWithError() throws {
        continueAfterFailure = false
        app = XCUIApplication()
        app.launchEnvironment["LINGXI_UI_TESTING"] = "1"
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)"]
        app.launch()
        XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 20), app.debugDescription)
    }

    func testFourTabsArePresentAndChatsIsSelectedAtLaunch() {
        for tab in ShellTab.allCases {
            XCTAssertTrue(app.tabButton(tab).waitForExistence(timeout: 10), "\(tab.identifier): \(app.debugDescription)")
        }
        XCTAssertTrue(app.tabButton(.chats).isSelected, app.debugDescription)
        XCTAssertTrue(app.tabBarIsShown, "the bar is visible on the Chats root")
    }

    /// Chats is never disposed: a draft survives a visit to another tab.
    func testSwitchingTabsKeepsTheChatsDraft() {
        app.textViews["composer.input"].tap()
        app.textViews["composer.input"].typeText("keep me")
        dismissKeyboard()
        XCTAssertTrue(app.waitForTabBar(shown: true), "keyboard down: bar visible")

        app.selectTab(.teams)
        XCTAssertTrue(app.navigationBars.firstMatch.waitForExistence(timeout: 10), app.debugDescription)
        app.selectTab(.chats)
        XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(String(describing: app.textViews["composer.input"].value).contains("keep me"), app.debugDescription)
    }

    func testTheBarLeavesPushedScreensAndReturnsAtTheRoot() throws {
        try XCTSkipUnless(isPhone, "on wide layouts the bar stays on pushed screens (D2)")
        openDrawer()
        app.buttons["drawer.entry.projects"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["projects.page"].firstMatch.waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.waitForTabBar(shown: false), "pushed Chats page: bar hidden")

        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.waitForTabBar(shown: true), "back at the Chats root: bar visible")
    }

    func testTheBarLeavesWhileTheDrawerIsOpen() throws {
        try XCTSkipUnless(isPhone, "the drawer rule is the phone layout's")
        openDrawer()
        XCTAssertTrue(app.waitForTabBar(shown: false), "drawer open: bar hidden")
        app.buttons["drawer.entry.projects"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["projects.page"].firstMatch.waitForExistence(timeout: 10), app.debugDescription)
    }

    func testTheBarLeavesWhileTheKeyboardIsUp() throws {
        try XCTSkipUnless(isPhone, "wide layouts keep the bar with the keyboard")
        app.textViews["composer.input"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.waitForTabBar(shown: false), "keyboard up: bar hidden")
        dismissKeyboard()
        XCTAssertTrue(app.waitForTabBar(shown: true), "keyboard down: bar visible")
    }

    func testTheDrawerOffersNeitherRemoteTeamsNorSettings() {
        openDrawer()
        XCTAssertFalse(app.buttons["drawer.remote-control"].exists)
        XCTAssertFalse(app.buttons["drawer.entry.teams"].exists)
        XCTAssertFalse(app.buttons["drawer.settings"].exists)
        XCTAssertTrue(app.buttons["drawer.account"].exists, "the account row stays")
        XCTAssertTrue(app.buttons["drawer.terminal"].exists, "the terminal button stays")
    }

    func testTheSettingsTabDoneReturnsToChats() {
        app.openSettingsTab()
        XCTAssertTrue(app.descendants(matching: .any)["settings.root"].firstMatch.waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.tabButton(.settings).isSelected)

        let close = app.descendants(matching: .any)["settings.close"].firstMatch
        XCTAssertTrue(close.waitForExistence(timeout: 5), app.debugDescription)
        close.tap()
        XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.tabButton(.chats).isSelected)
    }

    func testTheAccountRowJumpsToSettingsAccountAndBackReturnsToChats() throws {
        try XCTSkipUnless(isPhone, "uses the compact drawer")
        openDrawer()
        app.buttons["drawer.account"].tap()
        XCTAssertTrue(app.waitForTabBar(shown: false), "Settings/Account is a pushed page: bar hidden")
        XCTAssertTrue(app.staticTexts["LingXi 账户"].waitForExistence(timeout: 10), app.debugDescription)

        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.textViews["composer.input"].waitForExistence(timeout: 10), "Back leaves to the tab the jump came from")
        XCTAssertTrue(app.tabButton(.chats).isSelected, app.debugDescription)
        let drawerRow = app.buttons["drawer.filter.chat"]
        XCTAssertFalse(drawerRow.exists && drawerRow.isHittable, "the drawer is closed")
    }

    func testASettingsSubPageHidesTheBarOnPhones() throws {
        try XCTSkipUnless(isPhone, "wide layouts keep the bar on pushed screens (D2)")
        app.openSettingsTab()
        XCTAssertTrue(app.waitForTabBar(shown: true), "Settings root: bar visible")
        let group = app.buttons["settings.group.models"]
        XCTAssertTrue(group.waitForExistence(timeout: 10), app.debugDescription)
        group.tap()
        XCTAssertTrue(app.buttons["provider.add"].waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.waitForTabBar(shown: false), "Settings sub-page: bar hidden")

        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.waitForTabBar(shown: true), "back at the Settings root: bar visible")
    }

    /// The bar is visible on pushed screens only on wide layouts, so only there can the active tab be re-tapped.
    func testReTappingSettingsPopsToItsRootOnWideLayouts() throws {
        try XCTSkipIf(isPhone, "phones hide the bar on pushed screens")
        app.openSettingsTab()
        let group = app.buttons["settings.group.models"]
        XCTAssertTrue(group.waitForExistence(timeout: 10), app.debugDescription)
        group.tap()
        XCTAssertTrue(app.buttons["provider.add"].waitForExistence(timeout: 10), app.debugDescription)

        app.selectTab(.settings)
        XCTAssertTrue(app.tabButton(.settings).isSelected)
        XCTAssertTrue(group.waitForExistence(timeout: 10), "re-tap pops the Settings stack to its root")
    }

    /// Both tabs stay in the bar and keep it on their roots, signed in or not (the screens explain themselves).
    func testBotsAndTeamsTabsAreReachable() {
        app.openBotsTab()
        XCTAssertTrue(app.navigationBars.firstMatch.waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.tabBarIsShown, "the Bots root keeps the bar")
        app.selectTab(.teams)
        XCTAssertTrue(app.navigationBars.firstMatch.waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.tabBarIsShown, "the Teams root keeps the bar")
    }

    // MARK: helpers

    private func openDrawer() {
        let chatsFilter = app.buttons["drawer.filter.chat"]
        if chatsFilter.exists && chatsFilter.isHittable { return }
        let trigger = app.buttons["drawer.open"]
        XCTAssertTrue(trigger.waitForExistence(timeout: 10), app.debugDescription)
        trigger.tap()
        XCTAssertTrue(chatsFilter.waitForExistence(timeout: 10), app.debugDescription)
    }

    /// Best effort across iPhone and iPad keyboards: the iPad's dismiss key, else a downward drag on the transcript.
    private func dismissKeyboard() {
        guard app.keyboards.firstMatch.exists else { return }
        let hide = app.keyboards.buttons.matching(
            NSPredicate(format: "label IN {'Hide keyboard', '隐藏键盘', '收起键盘'}")
        ).firstMatch
        if hide.exists { hide.tap(); return }
        app.scrollViews["conversation.message-list"].swipeDown()
    }
}
