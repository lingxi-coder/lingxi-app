import XCTest

@testable import LingxiCode

/// The shell's navigation model (spec §2, §5): tab selection, the Settings jump API with its origin, Back out
/// of the last page, re-tapping a tab, and the depths the tab-bar rule reads. Pure model tests, so they also run
/// in the macOS runner: `macos-pkg/run.sh <ios-native> TabNavigationModelTests.swift`.
@MainActor
final class TabNavigationModelTests: XCTestCase {

    func testLeavingChatsRejectsLateAppPresentationEvenAfterReturning() {
        for tab in [AppTab.bots, .teams, .settings] {
            let navigation = AppNavigationModel()
            navigation.openApps()
            let revision = navigation.presentationRevision
            XCTAssertTrue(navigation.mayPresentImportedApp(capturedRevision: revision))
            navigation.tabs.select(tab)
            XCTAssertFalse(navigation.mayPresentImportedApp(capturedRevision: revision))
            navigation.tabs.select(.chats)
            XCTAssertFalse(navigation.mayPresentImportedApp(capturedRevision: revision))
            XCTAssertTrue(navigation.mayPresentImportedApp(capturedRevision: navigation.presentationRevision))
        }
    }

    func testTheModelStartsOnTheChatsTabWithNothingPushed() {
        let navigation = AppNavigationModel()

        XCTAssertEqual(navigation.selectedTab, .chats)
        XCTAssertFalse(navigation.settingsOpen)
        for tab in AppTab.allCases { XCTAssertEqual(navigation.depth(of: tab), 0, "\(tab)") }
        XCTAssertFalse(navigation.drawerOpenOrOpening)
    }

    func testShowingSettingsSelectsTheTabAndKeepsTheOldCallSitesWorking() {
        let navigation = AppNavigationModel()
        navigation.showSidebar()

        navigation.showSettings()
        XCTAssertEqual(navigation.selectedTab, .settings)
        XCTAssertTrue(navigation.settingsOpen)
        XCTAssertEqual(navigation.settingsPath, [])
        XCTAssertFalse(navigation.drawerOpenOrOpening, "opening settings closes the drawer")
        XCTAssertNil(navigation.settingsOrigin)

        navigation.showSettings(.voice)
        XCTAssertEqual(navigation.settingsPath, [.voice])
        XCTAssertEqual(navigation.depth(of: .settings), 1)
    }

    func testAJumpReplacesTheStackAndRemembersWhereItCameFrom() {
        let navigation = AppNavigationModel()
        navigation.showSettings(.account, origin: .chats)
        navigation.pushSettings(.voice)
        XCTAssertEqual(navigation.settingsPath, [.account, .voice])
        XCTAssertEqual(navigation.settingsOrigin, .chats)

        navigation.showSettings(.permissionMode, origin: .teams)
        XCTAssertEqual(navigation.settingsPath, [.permissionMode])
        XCTAssertEqual(navigation.settingsOrigin, .teams)

        navigation.showSettings(.main, origin: .bots)
        XCTAssertEqual(navigation.settingsPath, [], ".main is the empty stack")
        XCTAssertEqual(navigation.settingsOrigin, .bots)
    }

    func testClosingSettingsReturnsToTheOriginElseChats() {
        let navigation = AppNavigationModel()
        navigation.showSettings(.account, origin: .teams)
        navigation.closeSettings()
        XCTAssertEqual(navigation.selectedTab, .teams)
        XCTAssertEqual(navigation.settingsPath, [])
        XCTAssertNil(navigation.settingsOrigin)

        navigation.showSettings(.account)
        navigation.closeSettings()
        XCTAssertEqual(navigation.selectedTab, .chats, "an external link has no origin")

        navigation.showSettings(.account, origin: .bots)
        navigation.leaveSettings()
        XCTAssertEqual(navigation.selectedTab, .chats, "actions that belong to Chats ignore the origin")
        XCTAssertNil(navigation.settingsOrigin)
    }

    /// The `NavigationStack` writes the path itself when the user goes Back; the Settings tab reports it.
    func testGoingBackOutOfTheLastPageLeavesToTheOrigin() {
        let navigation = AppNavigationModel()
        navigation.showSettings(.providerList(.init(.llm)), origin: .chats)
        navigation.pushSettings(.mcpList)
        XCTAssertEqual(navigation.settingsPath.count, 2)

        // User Back: the binding shortens the path, then the tab reports old -> new.
        var old = navigation.settingsPath
        navigation.settingsPath.removeLast()
        navigation.settingsPathDidChange(from: old, to: navigation.settingsPath)
        XCTAssertEqual(navigation.selectedTab, .settings, "deeper pages pop first")
        XCTAssertEqual(navigation.settingsOrigin, .chats)

        old = navigation.settingsPath
        navigation.settingsPath.removeLast()
        navigation.settingsPathDidChange(from: old, to: navigation.settingsPath)
        XCTAssertEqual(navigation.selectedTab, .chats, "Back at depth one leaves to where the jump came from")
        XCTAssertNil(navigation.settingsOrigin)
        XCTAssertEqual(navigation.settingsPath, [])
    }

    func testManualSettingsNavigationNeverLeavesTheTab() {
        let navigation = AppNavigationModel()
        navigation.tabs.select(.settings)
        navigation.pushSettings(.appearance)

        let old = navigation.settingsPath
        navigation.settingsPath.removeLast()
        navigation.settingsPathDidChange(from: old, to: navigation.settingsPath)

        XCTAssertEqual(navigation.selectedTab, .settings)
        XCTAssertEqual(navigation.settingsPath, [])
    }

    func testTheModelsOwnWritesAreNotMistakenForTheUsersBack() {
        let navigation = AppNavigationModel()
        navigation.showSettings(.account, origin: .chats)
        let old = navigation.settingsPath

        // "Save, then pop to the root": page code calls the model; the tab then reports old -> new.
        navigation.resetSettings()
        navigation.settingsPathDidChange(from: old, to: navigation.settingsPath)

        XCTAssertEqual(navigation.selectedTab, .settings)
        XCTAssertEqual(navigation.settingsOrigin, .chats, "only the user's own Back leaves")
    }

    func testTappingATabForgetsTheOrigin() {
        let navigation = AppNavigationModel()
        navigation.showSettings(.account, origin: .chats)

        navigation.userSelectedTab(.teams)
        XCTAssertEqual(navigation.selectedTab, .teams)
        XCTAssertNil(navigation.settingsOrigin)
        XCTAssertEqual(navigation.settingsPath, [.account], "the Settings tab keeps its stack")

        navigation.userSelectedTab(.settings)
        navigation.closeSettings()
        XCTAssertEqual(navigation.selectedTab, .chats)
    }

    func testReTappingTheSelectedTabUnwindsItsStack() {
        let navigation = AppNavigationModel()
        navigation.openProjects()
        navigation.openProject("project-a")
        XCTAssertEqual(navigation.depth(of: .chats), 2)
        navigation.userSelectedTab(.chats)
        XCTAssertEqual(navigation.path, [])

        navigation.showSettings(.account, origin: .chats)
        navigation.pushSettings(.voice)
        navigation.userSelectedTab(.settings)
        XCTAssertEqual(navigation.settingsPath, [])
        XCTAssertEqual(navigation.selectedTab, .settings, "a re-tap stays on the tab")
        XCTAssertNil(navigation.settingsOrigin)

        let scrolls = navigation.tabs.scrollToTopRequests[.settings] ?? 0
        navigation.userSelectedTab(.settings)
        XCTAssertEqual(navigation.tabs.scrollToTopRequests[.settings], scrolls + 1, "at the root a re-tap scrolls to top")
    }

    func testReTapOnBotsAndTeamsPopsOnlyWhenTheTabReportedDepth() {
        let navigation = AppNavigationModel()
        navigation.tabs.select(.bots)

        navigation.userSelectedTab(.bots)
        XCTAssertNil(navigation.tabs.popToRootRequests[.bots])
        XCTAssertEqual(navigation.tabs.scrollToTopRequests[.bots], 1)

        navigation.tabs.reportDepth(.bots, 2)
        XCTAssertEqual(navigation.depth(of: .bots), 2)
        navigation.userSelectedTab(.bots)
        XCTAssertEqual(navigation.tabs.popToRootRequests[.bots], 1)
        XCTAssertEqual(navigation.tabs.scrollToTopRequests[.bots], 1)
    }

    func testPagesAndOverlaysOfChatsSelectTheChatsTab() {
        let navigation = AppNavigationModel()
        let opens: [(String, (AppNavigationModel) -> Void)] = [
            ("terminal", { $0.openTerminal() }),
            ("cron", { $0.openCron() }),
            ("cron run", { $0.openCronRun("run-1") }),
            ("projects", { $0.openProjects() }),
            ("apps", { $0.openApps() }),
            ("session details", { $0.openSessionDetails(sessionID: "s1") }),
            ("market app", { $0.openMarketApp("app-1") }),
        ]
        for (name, open) in opens {
            navigation.tabs.select(.teams)
            navigation.path.removeAll()
            open(navigation)
            XCTAssertEqual(navigation.selectedTab, .chats, name)
            XCTAssertEqual(navigation.depth(of: .chats), 1, name)
        }

        navigation.tabs.select(.bots)
        navigation.runApp("app-1")
        XCTAssertEqual(navigation.selectedTab, .chats)
        XCTAssertEqual(navigation.runningAppID, "app-1")
    }

    func testTheTerminalRouteIsTheOnlyOverlayRoute() {
        let navigation = AppNavigationModel()
        navigation.openProjects()
        XCTAssertFalse(navigation.path.last?.isTerminal ?? true)
        navigation.openTerminal()
        XCTAssertTrue(navigation.path.last?.isTerminal ?? false)
    }

    func testTheDrawerIsOpenWhileRevealedOrSelected() {
        let navigation = AppNavigationModel()
        navigation.drawerRevealed = true
        XCTAssertTrue(navigation.drawerOpenOrOpening, "dragging counts")
        navigation.drawerRevealed = false
        navigation.showSidebar()
        XCTAssertTrue(navigation.drawerOpenOrOpening)
        navigation.closeSidebar()
        XCTAssertFalse(navigation.drawerOpenOrOpening)
    }

    func testTheTabCoordinatorKeepsDepthsPerTab() {
        let tabs = TabCoordinator()
        tabs.reportDepth(.teams, 3)
        tabs.reportDepth(.bots, -4)
        XCTAssertEqual(tabs.depth[.teams], 3)
        XCTAssertEqual(tabs.depth[.bots], 0, "a depth is never negative")
        tabs.select(.bots)
        XCTAssertEqual(tabs.selected, .bots)
    }
}
