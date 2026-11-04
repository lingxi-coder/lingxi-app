import SwiftUI
import UIKit

/// The app shell (spec §2): Chats / Bots / Teams / Settings on the system `TabView`.
///
/// - iPhone (portrait and landscape): the standard bottom tab bar. iPad: `.sidebarAdaptable`, i.e. the
///   system tab bar at the top that the user can turn into a sidebar. There is no custom rail.
/// - Every tab keeps its own state (the system retains each tab's view). Chats is the first tab and is
///   composed first; it owns the live engine, so a restored selection only applies after it appeared.
/// - The bar follows `TabLogic.tabBarVisible` (D2): visible on a tab's root only; hidden on pushed
///   screens, with the keyboard up, while the phone drawer of Chats is open or opening, and under the
///   full-screen overlays (onboarding, the running app, the terminal route). Wide layouts keep it.
///   Pushed destinations may additionally apply `hidesTabBarWhenPushed(in:overlay:)` so the system
///   animates the hiding together with the push; both come from the same function and agree.
/// - `isWideLayout` (>= 840 pt) is measured once, on the window, and injected for the panes inside tabs.
/// - Re-tapping the selected tab goes through the selection binding's setter
///   (`AppNavigationModel.userSelectedTab`); SwiftUI has no reselect callback. Verify on a device that
///   the setter runs for the already-selected tab.
struct TabShell<ChatsContent: View, SettingsContent: View>: View {
    @Environment(AppState.self) private var app
    @Environment(BotsBadge.self) private var botsBadge
    @Environment(TeamsWorkspace.self) private var teamsWorkspace
    let navigation: AppNavigationModel
    @ViewBuilder let chats: () -> ChatsContent
    @ViewBuilder let settings: () -> SettingsContent

    @State private var windowWidth: Double = 0
    @State private var keyboardVisible = false
    /// The selected tab survives process death with the scene (default Chats).
    @SceneStorage("shell.selectedTab") private var storedTab = AppTab.chats.rawValue

    private var wide: Bool { TabLogic.isWide(width: windowWidth) }

    /// Full-screen overlays above the shell (§2.1): the onboarding wizard, a running app and the terminal
    /// (a pushed route of Chats on iOS). Image previews and the like are modal covers and hide the bar
    /// by themselves.
    private var overlayOpen: Bool {
        if !app.setupDone || navigation.runningAppID != nil { return true }
        return navigation.path.last?.isTerminal == true
    }

    var body: some View {
        styledTabView
            .environment(\.isWideLayout, wide)
            .environment(navigation)
            .environment(navigation.tabs)
            .background {
                // The window, safe areas included: the breakpoint is about the window, not the tab content.
                GeometryReader { proxy in
                    Color.clear.onChange(of: proxy.size.width, initial: true) { _, width in
                        windowWidth = Double(width)
                    }
                }
                .ignoresSafeArea()
            }
            // Hide with the keyboard once it has landed (it covers the bar anyway), and bring the bar back
            // as it leaves: hiding earlier would shrink the safe area under the rising composer.
            .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardDidShowNotification)) { _ in
                keyboardVisible = true
            }
            .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)) { _ in
                keyboardVisible = false
            }
            .onAppear(perform: restoreSelectedTab)
            .onChange(of: navigation.tabs.selected) { _, tab in storedTab = tab.rawValue }
    }

    @ViewBuilder
    private var styledTabView: some View {
        if UIDevice.current.userInterfaceIdiom == .pad {
            tabView.tabViewStyle(.sidebarAdaptable)
        } else {
            tabView
        }
    }

    private var selection: Binding<AppTab> {
        Binding(
            get: { navigation.tabs.selected },
            set: { navigation.userSelectedTab($0) }
        )
    }

    private var tabView: some View {
        let signedIn = FirebaseAccount.shared.user != nil
        let botsWaiting = signedIn ? botsBadge.count : 0
        let teamsUnread = signedIn ? teamsWorkspace.totalUnread : 0
        let botsBadgeText = TabLogic.tabBadgeText(botsWaiting)
        let teamsBadgeText = TabLogic.tabBadgeText(teamsUnread)
        // What VoiceOver reads after the tab's name, with the real number (the badge text caps at 99+).
        let botsValue: LocalizedStringKey = botsBadgeText == nil ? "" : "remote_bot_waiting \(botsWaiting)"
        let teamsValue: LocalizedStringKey = teamsBadgeText == nil ? "" : "tabs_badge_unread \(teamsUnread)"
        return TabView(selection: selection) {
            Tab("tabs_chats", systemImage: "bubble.left.and.bubble.right", value: AppTab.chats) {
                chats()
                    .toolbar(barVisibility(for: .chats), for: .tabBar)
            }
            .accessibilityIdentifier("tab.chats")

            Tab("remote_bots", systemImage: "cpu", value: AppTab.bots) {
                BotsTab()
                    .toolbar(barVisibility(for: .bots), for: .tabBar)
            }
            .badge(botsBadgeText.map { Text($0) })
            .accessibilityValue(botsValue)
            .accessibilityIdentifier("tab.bots")

            Tab("teams_title", systemImage: "person.2", value: AppTab.teams) {
                TeamsTab()
                    .toolbar(barVisibility(for: .teams), for: .tabBar)
            }
            .badge(teamsBadgeText.map { Text($0) })
            .accessibilityValue(teamsValue)
            .accessibilityIdentifier("tab.teams")

            Tab("settings_title_main", systemImage: "gearshape", value: AppTab.settings) {
                settings()
                    .onChange(of: navigation.settingsPath) { old, new in
                        navigation.settingsPathDidChange(from: old, to: new)
                    }
                    .toolbar(barVisibility(for: .settings), for: .tabBar)
            }
            .accessibilityIdentifier("tab.settings")
        }
    }

    private func barVisibility(for tab: AppTab) -> Visibility {
        TabLogic.tabBarVisible(
            wide: wide,
            depth: navigation.depth(of: tab),
            keyboardVisible: keyboardVisible,
            drawerOpen: navigation.drawerOpenOrOpening,
            selectedTab: tab,
            overlayOpen: overlayOpen
        ) ? .automatic : .hidden
    }

    /// Applies the stored selection after the first frame, so Chats has been composed first. UI tests
    /// always start on Chats.
    private func restoreSelectedTab() {
        guard ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] != "1",
              let restored = AppTab(rawValue: storedTab),
              restored != navigation.tabs.selected else { return }
        navigation.tabs.select(restored)
    }
}

extension View {
    /// For a screen pushed onto a tab's `NavigationStack`: hides the system tab bar with the push (the
    /// `hidesBottomBarWhenPushed` mechanism), by the same rule as the shell. On wide layouts the bar stays,
    /// unless the screen is a full-screen overlay (`overlay`, e.g. the terminal).
    func hidesTabBarWhenPushed(in tab: AppTab, overlay: Bool = false) -> some View {
        modifier(PushedScreenTabBar(tab: tab, overlay: overlay))
    }
}

private struct PushedScreenTabBar: ViewModifier {
    @Environment(\.isWideLayout) private var wide
    let tab: AppTab
    let overlay: Bool

    func body(content: Content) -> some View {
        content.toolbar(
            TabLogic.tabBarVisible(
                wide: wide, depth: 1, keyboardVisible: false, drawerOpen: false,
                selectedTab: tab, overlayOpen: overlay
            ) ? .automatic : .hidden,
            for: .tabBar
        )
    }
}
