import MobileLinuxNativeSupport

/// Product path profile over the independent SDK's guest conventions.
enum LXISHGuestPaths {
    static let home = MobileLinuxNativeSupport.LXISHGuestPaths.home
    static let scratch = MobileLinuxNativeSupport.LXISHGuestPaths.scratch
    static let workspaceRoot = MobileLinuxNativeSupport.LXISHGuestPaths.workspaceRoot

    static func workspace(_ stableWorkspaceId: String) -> String {
        MobileLinuxNativeSupport.LXISHGuestPaths.workspace(stableWorkspaceId)
    }
}
