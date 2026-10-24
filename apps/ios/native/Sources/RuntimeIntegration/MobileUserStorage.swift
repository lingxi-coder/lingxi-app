import Foundation

/// Native backing locations of the Linux runtime account. Stable project and
/// session identities use guest paths; host paths are only filesystem adapters.
enum MobileUserStorage {
    private static let preparationLock = NSLock()
    static func directories(managedRoot: String = LXISHDefaultWorkspace.managedRootPath()) -> MobileUserDirectories {
        // The shared resolver is pure and can fail only for a malformed host
        // root. Filesystem availability is reported by repository/engine I/O.
        // iOS can spell the same container through /var and /private/var.
        // Normalize the managed root before deriving owned child paths, so
        // repository containment checks do not reject the system's alias.
        guard (try? mobileUserDirectories(managedRoot: managedRoot)) != nil else {
            preconditionFailure("Application Support must provide an absolute runtime storage root")
        }
        let canonicalRoot = URL(fileURLWithPath: managedRoot, isDirectory: true)
            .resolvingSymlinksInPath().standardizedFileURL.path
        guard let paths = try? mobileUserDirectories(managedRoot: canonicalRoot) else {
            preconditionFailure("Application Support must provide an absolute runtime storage root")
        }
        return paths
    }

    static var projectsURL: URL {
        URL(fileURLWithPath: directories().projects, isDirectory: true)
    }

    /// Recover only the exact self-link produced by a redundant native bind.
    /// Preserve that link and its target text before recreating the directory;
    /// this cannot recover files that a previous runtime already deleted.
    static func prepareProjectsDirectory(managedRoot: String = LXISHDefaultWorkspace.managedRootPath(),
                                         fileManager: FileManager = .default) throws -> URL {
        preparationLock.lock()
        defer { preparationLock.unlock() }
        let paths = directories(managedRoot: managedRoot)
        let projects = URL(fileURLWithPath: paths.projects, isDirectory: true).standardizedFileURL
        func spellingWithoutFollowingLeaf(_ url: URL) -> String {
            let normalized = url.standardizedFileURL
            return normalized.deletingLastPathComponent().resolvingSymlinksInPath()
                .appendingPathComponent(normalized.lastPathComponent).path
        }
        if (try? fileManager.attributesOfItem(atPath: projects.path)[.type] as? FileAttributeType) == .typeSymbolicLink {
            let target = try fileManager.destinationOfSymbolicLink(atPath: projects.path)
            let destination = target.hasPrefix("/") ? URL(fileURLWithPath: target)
                : projects.deletingLastPathComponent().appendingPathComponent(target)
            if spellingWithoutFollowingLeaf(destination) == spellingWithoutFollowingLeaf(projects) {
                let archive = URL(fileURLWithPath: paths.configHome, isDirectory: true)
                    .appendingPathComponent("recovery/workspace-self-link-" + UUID().uuidString, isDirectory: true)
                try fileManager.createDirectory(at: archive, withIntermediateDirectories: true)
                let receipt: [String: String] = ["originalPath": projects.path, "linkTarget": target,
                    "status": "corruptLinkPreserved; previously deleted files are not recovered"]
                try JSONSerialization.data(withJSONObject: receipt, options: [.sortedKeys])
                    .write(to: archive.appendingPathComponent("receipt.json"), options: .atomic)
                try fileManager.moveItem(at: projects, to: archive.appendingPathComponent("projects"))
            }
        }
        try fileManager.createDirectory(at: projects, withIntermediateDirectories: true)
        return projects
    }
}
