import MobileLinuxNativeSupport

/// Product path profile over the independent SDK's guest conventions.
enum LXISHGuestPaths {
    private static var directories: MobileUserDirectories { MobileUserStorage.directories() }

    static var home: String { directories.guestHome }
    static let scratch = MobileLinuxNativeSupport.LXISHGuestPaths.scratch
    static var workspaceRoot: String { directories.defaultGuestCwd }

    static func workspace(_ stableWorkspaceId: String) -> String {
        if stableWorkspaceId == "projects" { return directories.projectsGuestPath }
        if stableWorkspaceId.isEmpty { return directories.defaultGuestCwd }
        return directories.defaultGuestCwd + "/" + stableWorkspaceId
    }
}
