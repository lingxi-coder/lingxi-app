import MobileLinuxNativeSupport

/// Product path profile over the independent SDK's guest conventions.
enum LXISHGuestPaths {
    private static var directories: MobileUserDirectories { MobileUserStorage.directories() }

    static var home: String { directories.guestHome }
    static let scratch = MobileLinuxNativeSupport.LXISHGuestPaths.scratch
    // The shared container is an anchor for managed resources, not a
    // conversation's working directory.
    static var workspaceRoot: String { directories.configGuestPath }

    static func conversationWorkspace(for mode: SessionMode) -> String {
        switch mode {
        case .chat: directories.chatGuestCwd
        case .code: directories.codeGuestCwd
        }
    }

    static func workspace(_ stableWorkspaceId: String) -> String {
        if stableWorkspaceId == "projects" { return directories.projectsGuestPath }
        if stableWorkspaceId == "scheduled" { return directories.scheduledGuestCwd }
        if stableWorkspaceId.isEmpty { return directories.configGuestPath }
        return directories.configGuestPath + "/" + stableWorkspaceId
    }
}
