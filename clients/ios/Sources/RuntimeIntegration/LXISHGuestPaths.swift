import MobileLinuxNativeSupport

/// Product path profile over the independent SDK's guest conventions.
enum LXISHGuestPaths {
    static let home = MobileLinuxNativeSupport.LXISHGuestPaths.home
    static let scratch = MobileLinuxNativeSupport.LXISHGuestPaths.scratch
    static let workspaceRoot = MobileLinuxNativeSupport.LXISHGuestPaths.workspaceRoot
    static let localAppBuildRoot = "/var/lingxi/local-app-build"
    static let localAppBuildProjectDirectory = "project"

    static func workspace(_ stableWorkspaceId: String) -> String {
        MobileLinuxNativeSupport.LXISHGuestPaths.workspace(stableWorkspaceId)
    }

    static func localAppBuildProject(appId: String, channel: String) -> String {
        "\(localAppBuildRoot)/\(appId)/\(channel)/\(localAppBuildProjectDirectory)"
    }
}
