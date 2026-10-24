import Foundation
import Darwin

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

    /// An app update can move the data container while preserving an absolute
    /// link left by the old native bind. Match only this runtime's managed
    /// projects location, never an arbitrary project or an external directory.
    static func isRelocatedManagedWorkspaceLink(_ target: String, projects: String,
                                                applicationHome: String = NSHomeDirectory()) -> Bool {
        func components(_ path: String) -> [String]? {
            guard path.hasPrefix("/") else { return nil }
            var parts = path.split(separator: "/").map(String.init)
            guard !parts.contains("."), !parts.contains("..") else { return nil }
            if parts.first == "private" { parts.removeFirst() }
            return parts
        }
        guard let home = components(applicationHome), let current = components(projects),
              let previous = components(target), home.count == 6,
              Array(home.prefix(5)) == ["var", "mobile", "Containers", "Data", "Application"],
              UUID(uuidString: home[5]) != nil else { return false }
        let managedProjects = ["Library", "Application Support", "mobile-linux", "ios-ish",
                               "persistent", "lingxi", "lingxi", "projects"]
        return current == home + managedProjects && previous.count == current.count
            && Array(previous.prefix(5)) == Array(home.prefix(5))
            && UUID(uuidString: previous[5]) != nil && previous[5] != home[5]
            && Array(previous.dropFirst(6)) == managedProjects
    }

    /// Recover the redundant native self-link, including its stale spelling
    /// after iOS relocates this app's data container during installation.
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
            let sameLocation = spellingWithoutFollowingLeaf(destination) == spellingWithoutFollowingLeaf(projects)
            var targetInfo = stat()
            let targetResult = destination.path.withCString { fstatat(AT_FDCWD, $0, &targetInfo, 0) }
            let targetError = errno
            let staleContainerLink = !sameLocation
                && isRelocatedManagedWorkspaceLink(target, projects: projects.path)
                && targetResult == -1 && (targetError == ENOENT || targetError == ELOOP)
            if sameLocation || staleContainerLink {
                let archive = URL(fileURLWithPath: paths.configHome, isDirectory: true)
                    .appendingPathComponent("recovery/workspace-self-link-" + UUID().uuidString, isDirectory: true)
                try fileManager.createDirectory(at: archive, withIntermediateDirectories: true)
                let receipt: [String: String] = ["originalPath": projects.path, "linkTarget": target,
                    "reason": staleContainerLink ? "relocatedManagedWorkspaceLink" : "workspaceSelfLink",
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
