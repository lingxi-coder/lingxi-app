import Foundation

/// Resolve application resources before the existing Rust host adapter creates
/// an SDK configuration. Only this product layer reads Bundle.main or product
/// environment overrides; the SDK receives explicit paths through its config.
enum LXISHRuntimeResourceBootstrap {
    static let archiveEnvironmentKey = "LINGXI_IOS_ISH_ROOTFS_ZIP"
    static let defaultMountEnvironmentKey = "LINGXI_IOS_ISH_DEFAULT_MOUNT_DIR"
    static let rootfsPatchEnvironmentKey = "LINGXI_IOS_ISH_ROOTFS_PATCH_DIR"

    // Swift initializes a static stored property once, serializing concurrent
    // terminal/settings/engine entry points. Bundled resources are immutable
    // for the lifetime of this application process.
    private static let prepared: Void = {
        let managedRoot = LXISHDefaultWorkspace.managedRootPath()
        let environment = ProcessInfo.processInfo.environment
        // Resolve existing overrides and Bundle resources once. The Rust host
        // composer resolves the final managed-root fallback per configuration,
        // so a caller's custom managed root never inherits the default root.
        let resolved = resolvedLegacyResourceOverrides(managedRoot: "", environment: environment)
        for key in [archiveEnvironmentKey, defaultMountEnvironmentKey] {
            if let path = resolved[key] {
                setenv(key, path, 1)
            } else if environment[key] != nil {
                unsetenv(key)
            }
        }
        if environment[rootfsPatchEnvironmentKey] == nil,
           let patch = rootfsPatchURL(managedRoot: managedRoot) {
            setenv(rootfsPatchEnvironmentKey, patch.path, 0)
        }
    }()

    static func prepare() {
        _ = prepared
    }

    static func resolvedLegacyResourceOverrides(
        managedRoot: String,
        bundle: Bundle = .main,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fileManager: FileManager = .default
    ) -> [String: String] {
        var resolved: [String: String] = [:]
        if let archive = rootfsArchiveURL(managedRoot: managedRoot, bundle: bundle, environment: environment, fileManager: fileManager) {
            resolved[archiveEnvironmentKey] = archive.path
        }
        if let mount = defaultMountURL(managedRoot: managedRoot, bundle: bundle, environment: environment, fileManager: fileManager) {
            resolved[defaultMountEnvironmentKey] = mount.path
        }
        return resolved
    }

    private static func resourceOverrideURL(_ value: String?, isDirectory: Bool = false) -> URL? {
        guard let value, !value.isEmpty else { return nil }
        return URL(fileURLWithPath: value, isDirectory: isDirectory)
    }

    static func rootfsArchiveURL(
        managedRoot: String,
        bundle: Bundle = .main,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fileManager: FileManager = .default
    ) -> URL? {
        let managed = managedRoot.isEmpty ? nil : URL(fileURLWithPath: managedRoot, isDirectory: true)
        return firstExistingURL([
            resourceOverrideURL(environment[archiveEnvironmentKey]),
            bundle.url(forResource: "alpine-rootfs", withExtension: "zip"),
            bundle.url(forResource: "alpine-rootfs", withExtension: "zip", subdirectory: "LinuxRuntimeNative"),
            bundle.resourceURL?.appendingPathComponent("alpine-rootfs.zip"),
            managed?.appendingPathComponent("resources/alpine-rootfs.zip"),
        ], fileManager: fileManager)
    }

    static func defaultMountURL(
        managedRoot: String,
        bundle: Bundle = .main,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fileManager: FileManager = .default
    ) -> URL? {
        let managed = managedRoot.isEmpty ? nil : URL(fileURLWithPath: managedRoot, isDirectory: true)
        return firstExistingURL([
            resourceOverrideURL(environment[defaultMountEnvironmentKey], isDirectory: true),
            bundle.url(forResource: "default_mount", withExtension: nil),
            bundle.resourceURL?.appendingPathComponent("default_mount", isDirectory: true),
            managed?.appendingPathComponent("resources/default_mount", isDirectory: true),
        ], fileManager: fileManager)
    }

    static func rootfsPatchURL(
        managedRoot: String,
        bundle: Bundle = .main,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fileManager: FileManager = .default
    ) -> URL? {
        let managed = managedRoot.isEmpty ? nil : URL(fileURLWithPath: managedRoot, isDirectory: true)
        return firstExistingURL([
            environment[rootfsPatchEnvironmentKey].map { URL(fileURLWithPath: $0, isDirectory: true) },
            bundle.url(forResource: "RootfsPatch", withExtension: "bundle"),
            bundle.resourceURL?.appendingPathComponent("RootfsPatch.bundle", isDirectory: true),
            managed?.appendingPathComponent("resources/RootfsPatch.bundle", isDirectory: true),
        ], fileManager: fileManager)
    }

    private static func firstExistingURL(_ candidates: [URL?], fileManager: FileManager) -> URL? {
        candidates.compactMap { $0 }.first { fileManager.fileExists(atPath: $0.path) }
    }
}
