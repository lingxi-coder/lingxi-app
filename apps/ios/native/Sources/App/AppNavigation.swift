import Observation
import SwiftUI

enum TerminalRouteCwd: Hashable {
    case guestPath(String)
    case workspaceRelative(String)

    init?(_ rawValue: String?) {
        guard
            let rawValue = rawValue?.trimmingCharacters(in: .whitespacesAndNewlines),
            !rawValue.isEmpty
        else {
            return nil
        }
        if rawValue.hasPrefix("/") {
            self = .guestPath(rawValue)
        } else {
            self = .workspaceRelative(rawValue)
        }
    }

    var displayValue: String {
        switch self {
        case let .guestPath(path), let .workspaceRelative(path):
            return path
        }
    }
}

enum AppRoute: Hashable {
    case terminal(
        sessionID: String,
        initialCommand: String?,
        projectID: String?,
        requestedCwd: TerminalRouteCwd?
    )
    case cron(scopeID: String?, taskID: String?)
    case cronRun(runID: String)
    /// The new-task form, opened with a prompt already filled in.
    case cronNew(prompt: String)
    case sessionDetails(sessionID: String)
    case projects
    case project(id: String)
    /// 应用: the installed apps and the market; one app's page.
    case apps
    case localApp(id: String)
    case marketApp(id: String)
}

extension AppRoute {
    /// The terminal is a full-screen overlay of the Chats tab (§2.2, D16): it hides the tab bar even on
    /// wide layouts, where other pushed pages keep it.
    var isTerminal: Bool {
        if case .terminal = self { return true }
        return false
    }
}

extension AppRoute: Identifiable {
    var id: String {
        switch self {
        case .terminal(let sessionID, _, let projectID, let requestedCwd):
            return "terminal:\(projectID ?? "global"):\(sessionID):\(requestedCwd?.displayValue ?? "default")"
        case .cron(let scopeID, let taskID):
            return "cron:\(scopeID ?? "global"):\(taskID ?? "list")"
        case .cronRun(let runID):
            return "cron-run:\(runID)"
        case .cronNew:
            return "cron-new"
        case .sessionDetails(let sessionID):
            return "session-details:\(sessionID)"
        case .projects:
            return "projects"
        case .apps:
            return "apps"
        case .localApp(let id):
            return "local-app:\(id)"
        case .marketApp(let id):
            return "market-app:\(id)"
        case .project(let id):
            return "project:\(id)"
        }
    }
}

@Observable
@MainActor
final class AppNavigationModel {
    private var pagePresentationRevision: UInt64 = 0
    var presentationRevision: UInt64 { pagePresentationRevision &+ tabs.selectionRevision }
    var path: [AppRoute] = [] {
        didSet { if path != oldValue { pagePresentationRevision &+= 1 } }
    }
    /// The installed app running full screen, if any.
    var runningAppID: String? {
        didSet { if runningAppID != oldValue { pagePresentationRevision &+= 1 } }
    }
    /// The sidebar's recents filter chip. A list filter only — it never decides
    /// which mode the live conversation is in.
    var drawerFilter: RecentsFilter = .all
    /// Sidebar visibility in a regular-width (iPad) split layout. Ignored while
    /// the split view is collapsed.
    var columnVisibility: NavigationSplitViewVisibility = .all
    /// Compact sidebar state shared with the iPhone SwiftUI drawer. Keep it
    /// independent from iPad column visibility so choosing a session on iPad
    /// preserves the user's split layout.
    var compactColumn: NavigationSplitViewColumn = .detail
    /// The shell's tab state. The model owns it so every navigation entry point below can select a tab;
    /// `TabShell` reads it and injects it with `.environment(_:)`.
    let tabs: TabCoordinator
    /// The Settings tab's stack. `SettingsHost` binds its `NavigationStack` to this array.
    var settingsPath: [SettingsPage] = []
    /// The tab an in-app jump to Settings came from (§2.6): Back out of the stack, or "Done", returns
    /// there. External links carry none. Tapping any tab forgets it.
    private(set) var settingsOrigin: AppTab?
    /// The last stack this model wrote itself, so `settingsPathDidChange` can tell the user's own
    /// Back (swipe / chevron, written by the `NavigationStack` binding) from our programmatic writes.
    @ObservationIgnored private var expectedSettingsPath: [SettingsPage] = []
    /// True while the phone drawer is being dragged open (or is open); reported by the drawer so the
    /// tab bar can leave before the drawer lands (§2.2).
    var drawerRevealed = false

    init(tabs: TabCoordinator? = nil) {
        self.tabs = tabs ?? TabCoordinator()
    }

    var selectedTab: AppTab {
        get { tabs.selected }
        set { tabs.select(newValue) }
    }

    /// Settings is a tab now, not a presentation: "open" means it is the selected tab.
    var settingsOpen: Bool { tabs.selected == .settings }

    /// Pushed screens above `tab`'s root (§2.2). Chats and Settings are answered from the stacks this
    /// model owns, so the answer is current in the same update as the push itself; Bots and Teams report.
    func depth(of tab: AppTab) -> Int {
        switch tab {
        case .chats: path.count
        case .settings: settingsPath.count
        case .bots, .teams: tabs.depth[tab] ?? 0
        }
    }

    /// The drawer of the Chats tab is open, or being dragged open (phone layout).
    var drawerOpenOrOpening: Bool { drawerRevealed || compactColumn == .sidebar }

    /// Reveals the sidebar from the toolbar, deep links or app actions.
    /// The SwiftUI drawer writes interactive gesture results to this same state.
    func showSidebar() {
        columnVisibility = .all
        compactColumn = .sidebar
    }

    func closeSidebar() { compactColumn = .detail }

    /// Focus the main surface on every size class. `closeSidebar()` only
    /// changes compact navigation, which is correct for ordinary conversation
    /// selection but not for launching an app: on iPad the running app should
    /// receive the full canvas too.
    func focusDetail() {
        columnVisibility = .detailOnly
        compactColumn = .detail
    }

    // MARK: Settings tab (the Jump API, §5)

    private var settingsJump: SettingsJumpState<SettingsPage> {
        SettingsJumpState(root: .main, stack: settingsPath, origin: settingsOrigin)
    }

    private func adopt(_ state: SettingsJumpState<SettingsPage>) {
        settingsOrigin = state.origin
        if state.stack != settingsPath { writeSettingsPath(state.stack) }
    }

    private func writeSettingsPath(_ path: [SettingsPage]) {
        expectedSettingsPath = path
        settingsPath = path
    }

    /// The single entry for deep links and in-app shortcuts: select the Settings tab and replace its
    /// stack with `[page]` (`.main` is the empty stack). In-app shortcuts pass the tab they came from;
    /// external links pass none.
    func showSettings(_ page: SettingsPage = .main, origin: AppTab? = nil) {
        closeSidebar()
        adopt(settingsJump.reduced(by: .jump(page, origin: origin)).state)
        tabs.select(.settings)
    }

    /// "Close settings" (Done): the tab the user jumped from, else Chats; the stack is dropped.
    func closeSettings() {
        leaveSettings(to: settingsOrigin ?? .chats)
    }

    /// Leave Settings for `tab` and drop its stack and origin. For actions that belong to Chats
    /// (switching project, opening the terminal) whatever the origin was.
    func leaveSettings(to tab: AppTab = .chats) {
        settingsOrigin = nil
        writeSettingsPath([])
        tabs.select(tab)
    }

    func pushSettings(_ page: SettingsPage) {
        adopt(settingsJump.reduced(by: .push(page)).state)
    }

    func settingsBack() {
        let result = settingsJump.reduced(by: .back)
        adopt(result.state)
        if let tab = result.leaveTo { leaveSettings(to: tab) }
    }

    func popSettings() {
        guard !settingsPath.isEmpty else { return }
        writeSettingsPath(Array(settingsPath.dropLast()))
    }

    func resetSettings() {
        writeSettingsPath([])
    }

    func replaceSettingsTail(removing count: Int, with pages: [SettingsPage]) {
        let retainedCount = max(0, settingsPath.count - count)
        writeSettingsPath(Array(settingsPath.prefix(retainedCount)) + pages)
    }

    /// Fed with every change of `settingsPath` by the Settings tab. Writes this model made itself are
    /// recognised and ignored; what is left is the user going Back through the `NavigationStack`: each
    /// level popped is a `back` event, and Back out of the last page leaves to the jump's origin.
    func settingsPathDidChange(from old: [SettingsPage], to new: [SettingsPage]) {
        guard new != expectedSettingsPath else { return }
        expectedSettingsPath = new
        guard new.count < old.count else { return }
        var state = SettingsJumpState(root: SettingsPage.main, stack: old, origin: settingsOrigin)
        var leaveTo: AppTab?
        while state.stack.count > new.count {
            let step = state.reduced(by: .back)
            state = step.state
            leaveTo = step.leaveTo ?? leaveTo
            if state.stack.isEmpty { break }
        }
        settingsOrigin = state.origin
        if let leaveTo { leaveSettings(to: leaveTo) }
    }

    // MARK: Tab taps

    /// The user tapped `tab` in the bar. A new tab is selected and the Settings origin is forgotten
    /// (`userSelectedTab`); the already-selected one unwinds its stack, or scrolls to top at its root.
    func userSelectedTab(_ tab: AppTab) {
        adopt(settingsJump.reduced(by: .userSelectedTab).state)
        if tab == tabs.selected {
            reselect(tab)
        } else {
            tabs.select(tab)
        }
    }

    private func reselect(_ tab: AppTab) {
        switch tab {
        case .chats:
            if !path.isEmpty { path.removeAll() }
        case .settings:
            if settingsPath.isEmpty { tabs.requestScrollToTop(.settings) } else { resetSettings() }
        case .bots, .teams:
            tabs.reselect(tab)
        }
    }

    func openTerminal(
        sessionID: String = "interactive",
        initialCommand: String? = nil,
        projectID: String? = nil,
        requestedCwd: TerminalRouteCwd? = nil
    ) {
        closeSidebar()
        tabs.select(.chats)
        path.append(
            .terminal(
                sessionID: sessionID,
                initialCommand: initialCommand,
                projectID: projectID,
                requestedCwd: requestedCwd
            )
        )
    }

    func openTerminal(
        shellRequest: ConversationShellLaunchRequest,
        projectID: String? = nil,
        sessionID: String = "interactive"
    ) {
        openTerminal(
            sessionID: sessionID,
            initialCommand: shellRequest.command,
            projectID: projectID,
            requestedCwd: shellRequest.cwd
        )
    }

    /// Scheduled tasks are a page of their own, pushed like the terminal — not
    /// a sheet over the chat and not a settings sub-page.
    func openCron(scopeID: String? = nil, taskID: String? = nil) {
        pushPage(.cron(scopeID: scopeID, taskID: taskID))
    }

    func openCronRun(_ runID: String) {
        pushPage(.cronRun(runID: runID))
    }

    func openCronCreate(prompt: String) {
        pushPage(.cronNew(prompt: prompt))
    }

    func openProjects() {
        pushPage(.projects)
    }

    func openProject(_ projectID: String) {
        pushPage(.project(id: projectID))
    }

    func openApps() {
        pushPage(.apps)
    }

    func openLocalApp(_ appID: String) { pushPage(.localApp(id: appID)) }

    func mayPresent(capturedRevision: UInt64, on route: AppRoute) -> Bool {
        presentationRevision == capturedRevision && path.last == route && selectedTab == .chats && runningAppID == nil
    }

    func mayPresentImportedApp(capturedRevision: UInt64) -> Bool {
        mayPresent(capturedRevision: capturedRevision, on: .apps)
    }

    func openMarketApp(_ appID: String) {
        pushPage(.marketApp(id: appID))
    }

    /// Runs an installed app full screen over everything else.
    func runApp(_ appID: String) {
        closeSidebar()
        tabs.select(.chats)
        runningAppID = appID
    }

    /// Replace a page of the same family at the top instead of stacking
    /// duplicates (a second notification for the same task), and go back to
    /// the projects list when it is already under the top (re-opening it from
    /// a project page).
    private func pushPage(_ route: AppRoute) {
        closeSidebar()
        tabs.select(.chats)
        if let last = path.last, last.isSamePage(as: route) {
            path[path.count - 1] = route
        } else if route == .projects, let index = path.lastIndex(of: .projects) {
            path.removeSubrange((index + 1)...)
        } else {
            path.append(route)
        }
    }

    func openSessionDetails(sessionID: String) {
        closeSidebar()
        tabs.select(.chats)
        path.append(.sessionDetails(sessionID: sessionID))
    }

}

private extension AppRoute {
    /// Pages that replace one another when pushed twice in a row.
    func isSamePage(as other: AppRoute) -> Bool {
        switch (self, other) {
        case (.cron, .cron), (.cron, .cronRun), (.cronRun, .cron), (.cronRun, .cronRun),
             (.cronNew, .cronNew), (.cronNew, .cron), (.cron, .cronNew),
             (.projects, .projects), (.project, .project),
             (.apps, .apps), (.localApp, .localApp), (.marketApp, .marketApp):
            true
        default:
            false
        }
    }
}
