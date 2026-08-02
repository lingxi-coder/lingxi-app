import Observation

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
        }
    }
}

@Observable
@MainActor
final class AppNavigationModel {
    var path: [AppRoute] = []
    var presentedRoute: AppRoute?
    var drawerOpen = false
    var settingsOpen = false
    var settingsPath: [SettingsPage] = []

    func showDrawer() { drawerOpen = true }
    func closeDrawer() { drawerOpen = false }

    func showSettings(_ page: SettingsPage = .main) {
        drawerOpen = false
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
        drawerOpen = false
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
        drawerOpen = false
        settingsOpen = false
        presentedRoute = .cron(scopeID: scopeID, taskID: taskID)
    }

    func openCronRun(_ runID: String) {
        drawerOpen = false
        settingsOpen = false
        presentedRoute = .cronRun(runID: runID)
    }

    func closePresentedRoute() { presentedRoute = nil }
}
