import Foundation

// Pure shell logic (spec §8.6). Foundation only: no SwiftUI, no UIKit, no app state, so the same file
// runs in the macOS XCTest harness and is checked against `mobile-team-bot-vectors.json`
// (`mobile.tabBarVisible`, `.tabForLink`, `.tabBadgeText`, `.botsBadgeCount`, `.settingsJump`).
// Android has the identical contract in `tabs/TabLogic.kt`.

/// Where an incoming link opens: one of the five tabs, `stay` (a shell-level sheet on whatever tab is
/// showing) or `none` (not a link this build routes: the existing "unavailable" toast).
enum LinkTarget: String, Equatable, Sendable {
    case chats, bots, teams, friends, settings, stay, none

    /// The tab to select, or `nil` when the link keeps the current tab (`stay`) or is unknown (`none`).
    var tab: AppTab? {
        switch self {
        case .chats: .chats
        case .bots: .bots
        case .teams: .teams
        case .friends: .friends
        case .settings: .settings
        case .stay, .none: nil
        }
    }
}

enum TabLogic {
    /// The one app-wide breakpoint (D3), in points. The drawer and the settings two-pane already used it.
    static let wideBreakpoint: Double = 840

    static func isWide(width: Double) -> Bool { width >= wideBreakpoint }

    /// D2. The bar is visible on a tab's root screen only: hidden on pushed screens, with the keyboard
    /// up, while the phone drawer of Chats is open or opening, and under full-screen overlays. Wide
    /// layouts keep it (the system top bar / sidebar) except under overlays.
    static func tabBarVisible(
        wide: Bool,
        depth: Int,
        keyboardVisible: Bool,
        drawerOpen: Bool,
        selectedTab: AppTab,
        overlayOpen: Bool
    ) -> Bool {
        if overlayOpen { return false }
        if wide { return true }
        return depth == 0 && !keyboardVisible && !(selectedTab == .chats && drawerOpen)
    }

    /// §2.6: the tab a link route opens, from its first segment.
    static func tabForLink(_ route: String) -> LinkTarget {
        guard let first = route.split(separator: "/", omittingEmptySubsequences: true).first else { return .none }
        switch first {
        case "session", "new", "project", "apps", "market", "runtime", "terminal", "cron": return .chats
        case "remote": return .bots
        case "team": return .teams
        case "friend": return .friends
        case "settings", "provider", "extensions": return .settings
        case "import": return .stay
        default: return .none
        }
    }

    /// §2.5: nothing at 0 (or below), the number up to 99, `99+` from 100.
    static func tabBadgeText(_ count: Int) -> String? {
        if count <= 0 { return nil }
        return count >= 100 ? "99+" : String(count)
    }

    /// §2.5: the chats waiting for the user on the CONNECTED computer, summed; negative counts are zero.
    static func botsBadgeCount(waitingThreads: [Int]) -> Int {
        waitingThreads.reduce(0) { $0 + max(0, $1) }
    }
}

/// §8.6 `SettingsJumpState`: the Settings tab's stack and the tab a jump came from. A jump that carries an
/// `origin` makes Back at the last page hand control back to that tab; deeper pages pop first; tapping a
/// tab forgets the origin. `Page` is the app's settings page type; `root` is its home page, which is an
/// empty stack.
struct SettingsJumpState<Page: Equatable>: Equatable {
    var stack: [Page]
    var origin: AppTab?
    let root: Page

    init(root: Page, stack: [Page] = [], origin: AppTab? = nil) {
        self.root = root
        self.stack = stack
        self.origin = origin
    }

    enum Event: Equatable {
        /// External links pass no origin; in-app shortcuts pass the tab they came from.
        case jump(Page, origin: AppTab?)
        case push(Page)
        case back
        case userSelectedTab
    }

    /// Pure transition. `leaveTo` is the tab the shell must select now (the origin of the jump) when Back
    /// left the Settings stack; with an empty stack and no origin Back changes nothing (the shell decides).
    func reduced(by event: Event) -> (state: SettingsJumpState, leaveTo: AppTab?) {
        var next = self
        switch event {
        case let .jump(page, origin):
            next.stack = page == root ? [] : [page]
            next.origin = origin
        case let .push(page):
            next.stack.append(page)
        case .back:
            if next.stack.count >= 2 {
                next.stack.removeLast()
            } else {
                next.stack = []
                if let origin = next.origin {
                    next.origin = nil
                    return (next, origin)
                }
            }
        case .userSelectedTab:
            next.origin = nil
        }
        return (next, nil)
    }
}
