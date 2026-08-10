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
    case localApps(appID: String?)
    case sessionDetails(sessionID: String)
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
        case .localApps(let appID):
            return "local-apps:\(appID ?? "library")"
        case .sessionDetails(let sessionID):
            return "session-details:\(sessionID)"
        }
    }
}

@Observable
@MainActor
final class AppNavigationModel {
    var path: [AppRoute] = []
    var presentedRoute: AppRoute?
    /// Sidebar visibility in a regular-width (iPad) split layout. Ignored while
    /// the split view is collapsed.
    var columnVisibility: NavigationSplitViewVisibility = .all
    /// Which column the collapsed (iPhone) split view shows. This is the only
    /// sidebar control that applies in compact width — `columnVisibility` is
    /// ignored there — so navigation resets it, never `columnVisibility`.
    /// Leaving the regular-width sidebar alone is deliberate: an iPad user who
    /// picks a session keeps the two columns they asked for.
    var compactColumn: NavigationSplitViewColumn = .detail
    var settingsOpen = false
    var settingsPath: [SettingsPage] = []

    /// Reveals the sidebar from code (deep links, app actions). The system's own
    /// back button and back-swipe drive the same state without going through here.
    func showSidebar() {
        columnVisibility = .all
        compactColumn = .sidebar
    }

    func closeSidebar() { compactColumn = .detail }

    func showSettings(_ page: SettingsPage = .main) {
        closeSidebar()
        settingsPath = page == .main ? [] : [page]
        settingsOpen = true
    }

    func closeSettings() {
        settingsOpen = false
        settingsPath = []
    }

    func pushSettings(_ page: SettingsPage) {
        settingsPath.append(page)
    }

    func popSettings() {
        guard !settingsPath.isEmpty else { return }
        settingsPath.removeLast()
    }

    func resetSettings() {
        settingsPath.removeAll()
    }

    func replaceSettingsTail(removing count: Int, with pages: [SettingsPage]) {
        let retainedCount = max(0, settingsPath.count - count)
        settingsPath = Array(settingsPath.prefix(retainedCount)) + pages
    }

    func openTerminal(
        sessionID: String = "interactive",
        initialCommand: String? = nil,
        projectID: String? = nil,
        requestedCwd: TerminalRouteCwd? = nil
    ) {
        closeSidebar()
        settingsOpen = false
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

    func openCron(scopeID: String? = nil, taskID: String? = nil) {
        closeSidebar()
        settingsOpen = false
        presentedRoute = .cron(scopeID: scopeID, taskID: taskID)
    }

    func openCronRun(_ runID: String) {
        closeSidebar()
        settingsOpen = false
        presentedRoute = .cronRun(runID: runID)
    }

    func openLocalApps(appID: String? = nil) {
        closeSidebar()
        settingsOpen = false
        presentedRoute = .localApps(appID: appID)
    }

    func openSessionDetails(sessionID: String) {
        closeSidebar()
        settingsOpen = false
        path.append(.sessionDetails(sessionID: sessionID))
    }

    func closePresentedRoute() { presentedRoute = nil }
}
