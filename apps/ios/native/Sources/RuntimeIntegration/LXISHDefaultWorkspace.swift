import Foundation

// Product preferences and storage layout remain owned by the application.
enum LXISHDefaultWorkspace {
    static let guestHome = LXISHGuestPaths.workspaceRoot

    // Mount identity belongs to the runtime, independently of selected projects.
    static func stableID() -> String { "projects" }

    /// One derivation of the Application Support base for both paths below.
    /// Two sibling copies of this guard were how the managed root and the
    /// workspace could ever disagree on their failure behavior.
    private static var supportDirectory: URL? {
        FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask).first
    }

    static func hostPath() -> String {
        let url = MobileUserStorage.projectsURL
        do { return try MobileUserStorage.prepareProjectsDirectory().path }
        catch { NSLog("Linux workspace preparation failed: %@", error.localizedDescription) }
        return url.path
    }

    /// Where the Alpine rootfs is installed. One machine, so one path.
    ///
    /// The Linux runtime page computed `<AppSupport>/mobile-linux/ios-ish`
    /// while the terminal fell back to `<AppSandboxRoot>/mobile-linux/ios-ish`
    /// — and `appSandboxRoot` appends `LingxiCode` first, so the two were
    /// different directories. The only thing that reconciled them was
    /// `LinuxRuntimeState.managedRoot`, which is `Equatable` and not persisted,
    /// so it was nil on every cold launch: open the terminal before visiting
    /// Settings and it probed a root with no rootfs, or installed a second full
    /// Alpine beside the first, and Settings' repair/reset then operated on the
    /// copy the terminal never booted.
    static func managedRootPath() -> String {
        guard let support = supportDirectory else {
            return ""
        }
        return support
            .appendingPathComponent("mobile-linux/ios-ish", isDirectory: true)
            .path
    }
}
