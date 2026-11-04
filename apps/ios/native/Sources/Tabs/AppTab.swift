import Observation
import SwiftUI

/// The four tabs of the app shell (spec D1): Chats is the chat-first screen with its drawer, unchanged
/// inside; Bots, Teams and Settings own one `NavigationStack` each.
enum AppTab: String, CaseIterable, Hashable, Sendable {
    case chats, bots, teams, settings
}

extension EnvironmentValues {
    /// `true` at >= 840 pt (`TabLogic.wideBreakpoint`), measured once by `TabShell` on the window (D3).
    /// Panes inside the tabs (Bots / Teams / Settings two-pane) read this instead of measuring themselves.
    /// The shell's own tab bar is the system `TabView`; on iPad it is `.sidebarAdaptable`, on iPhone the
    /// standard bottom bar, so this value never switches the bar's form.
    @Entry var isWideLayout = false
}

/// A request to open a computer, or one of its chats, in the Bots tab (deep link, push notification).
struct BotsRequest: Equatable {
    var hostID: String
    var sessionID: String?
}

/// Tab state shared by the shell and the tab roots: which tab is selected, how deep each tab's stack is,
/// and the one-shot requests deep links leave for a tab. Created by `AppNavigationModel` (the app root
/// owns both) and injected with `.environment(_:)` by `TabShell`.
@MainActor
@Observable
final class TabCoordinator {
    var selected: AppTab = .chats
    /// Pushed screens above each tab's root, as the tab reports them (`reportDepth`). The Chats and
    /// Settings stacks live in `AppNavigationModel`, which answers for them (`depth(of:)`).
    var depth: [AppTab: Int] = [:]
    /// Set by deep links and push routes; `BotsTab` consumes it and sets it back to nil.
    var botsRequest: BotsRequest?
    /// The invite code of a `team/join` link; `TeamsTab` consumes it and sets it back to nil. It only
    /// fills the join form: a link never joins by itself.
    var teamsJoinCode: String?
    /// Counters a tab root watches: the user tapped the already-selected tab while that tab was
    /// pushed (pop to root) or already at its root (scroll to top).
    private(set) var popToRootRequests: [AppTab: Int] = [:]
    private(set) var scrollToTopRequests: [AppTab: Int] = [:]

    /// Programmatic selection (deep links, jumps, closers). Taps go through
    /// `AppNavigationModel.userSelectedTab(_:)`, which also forgets the Settings origin.
    func select(_ tab: AppTab) {
        guard selected != tab else { return }
        selected = tab
    }

    func reportDepth(_ tab: AppTab, _ depth: Int) {
        let clamped = max(0, depth)
        guard self.depth[tab] != clamped else { return }
        self.depth[tab] = clamped
    }

    /// Re-tap of the active tab: unwind its stack, or, when it is at its root, ask it to scroll to top.
    /// Chats and Settings are answered by `AppNavigationModel`, which owns those stacks.
    func reselect(_ tab: AppTab) {
        if (depth[tab] ?? 0) > 0 {
            popToRootRequests[tab, default: 0] += 1
        } else {
            requestScrollToTop(tab)
        }
    }

    func requestScrollToTop(_ tab: AppTab) {
        scrollToTopRequests[tab, default: 0] += 1
    }
}
