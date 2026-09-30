import Foundation

// Product preferences and storage layout remain owned by the application.
enum LXISHDefaultWorkspace {
    static let defaultsKey = "lingxi.mobile-linux.workspace.default.id"

    /// `/root` is mounted by the runtime unconditionally (the persistent home
    /// layer) and is always writable, so it is a valid cwd even with no project.
    static let guestHome = LXISHGuestPaths.home

    static func stableID(defaults: UserDefaults = .standard) -> String {
        if let persisted = defaults.string(forKey: defaultsKey),
           UUID(uuidString: persisted) != nil
        {
            return persisted.lowercased()
        }
        let generated = UUID().uuidString.lowercased()
        defaults.set(generated, forKey: defaultsKey)
        return generated
    }

    /// One derivation of the Application Support base for both paths below.
    /// Two sibling copies of this guard were how the managed root and the
    /// workspace could ever disagree on their failure behavior.
    private static var supportDirectory: URL? {
        FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask).first
    }

    static func hostPath(id: String? = nil) -> String {
        let workspaceID = id ?? stableID()
        guard let support = supportDirectory else {
            return ""
        }
        let url = support
            .appendingPathComponent("workspaces", isDirectory: true)
            .appendingPathComponent(workspaceID, isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
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
