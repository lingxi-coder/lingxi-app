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

    /// Resolves a host-owned cron manifest; generated runs remain outside the Project list.
    static func scheduledWorkspace(sessionID: String?, managedRoot: String = LXISHDefaultWorkspace.managedRootPath()) throws -> String {
        let paths = directories(managedRoot: managedRoot)
        guard let sessionID, !sessionID.isEmpty else { return paths.scheduledCwd }
        let id = sessionID.hasPrefix("sess:") ? String(sessionID.dropFirst(5)) : sessionID
        guard UUID(uuidString: id)?.uuidString.lowercased() == id else { throw ProjectRepositoryError.escapedWorkspace }
        let brandHome = URL(fileURLWithPath: paths.configHome, isDirectory: true)
        let manifest = brandHome.appendingPathComponent("automations/sessions/\(id).json")
        guard FileManager.default.fileExists(atPath: manifest.path) else { return paths.scheduledCwd }
        guard try manifest.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink != true else { throw ProjectRepositoryError.escapedWorkspace }
        struct Record: Decodable { let version: Int; let workspace: String }
        let record = try JSONDecoder().decode(Record.self, from: Data(contentsOf: manifest))
        let parts = record.workspace.split(separator: "/", omittingEmptySubsequences: false)
        guard record.version == 1, parts.count == 3, parts[0] == "workspaces",
              parts.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." && !$0.contains("\\") }) else { throw ProjectRepositoryError.escapedWorkspace }
        var workspace = brandHome
        for part in parts {
            workspace.appendPathComponent(String(part), isDirectory: true)
            let metadata = try workspace.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
            guard metadata.isDirectory == true, metadata.isSymbolicLink != true else { throw ProjectRepositoryError.escapedWorkspace }
        }
        let canonical = workspace.resolvingSymlinksInPath().standardizedFileURL
        let root = brandHome.resolvingSymlinksInPath().standardizedFileURL.appendingPathComponent("workspaces", isDirectory: true)
        guard canonical.path.hasPrefix(root.path + "/") else { throw ProjectRepositoryError.escapedWorkspace }
        return canonical.path
    }

    static var projectsURL: URL {
        URL(fileURLWithPath: directories().projects, isDirectory: true)
    }

    /// Selects exact, native Project-authorized guest paths for iOS Mobile
    /// Linux bind mounts. The Project record is the authority; this method
    /// never expands a grant to the whole home, `.lingxi`, or `.apps` tree.
    static func projectMountGrantPaths(
        activeDirectory: String,
        additionalDirectories: [String],
        managedRoot: String = LXISHDefaultWorkspace.managedRootPath()
    ) throws -> [String] {
        let paths = directories(managedRoot: managedRoot)
        let homeGuest = paths.guestHome
        let projectsGuest = paths.projectsGuestPath
        let configGuest = paths.configGuestPath
        let appsGuest = paths.appsGuestPath
        let homeHost = URL(fileURLWithPath: paths.home, isDirectory: true)
            .resolvingSymlinksInPath().standardizedFileURL
        let configHost = URL(fileURLWithPath: paths.configHome, isDirectory: true)
            .resolvingSymlinksInPath().standardizedFileURL
        let appsHost = URL(fileURLWithPath: paths.apps, isDirectory: true)
            .resolvingSymlinksInPath().standardizedFileURL
        let primaryProjectsHost = URL(fileURLWithPath: paths.projects, isDirectory: true)
            .resolvingSymlinksInPath().standardizedFileURL

        var candidates = additionalDirectories
        if !activeDirectory.isEmpty, !candidates.contains(activeDirectory) {
            candidates.append(activeDirectory)
        }
        guard Set(candidates).count == candidates.count else {
            throw ProjectRepositoryError.escapedWorkspace
        }
        var grants: [String] = []
        for guestPath in candidates.sorted() {
            guard isNormalizedGuestPath(guestPath),
                  guestPath.hasPrefix(homeGuest + "/") else {
                throw ProjectRepositoryError.escapedWorkspace
            }
            if guestPath == projectsGuest || guestPath.hasPrefix(projectsGuest + "/") {
                // The primary Mobile Linux workspace mount already covers it.
                continue
            }
            guard !projectsGuest.hasPrefix(guestPath + "/") else {
                throw ProjectRepositoryError.escapedWorkspace
            }

            let isLocalAppSource = guestPath.hasPrefix(appsGuest + "/")
            if guestPath == appsGuest || appsGuest.hasPrefix(guestPath + "/") {
                throw ProjectRepositoryError.escapedWorkspace
            }
            let overlapsConfigGuest = guestPath == configGuest
                || guestPath.hasPrefix(configGuest + "/")
                || configGuest.hasPrefix(guestPath + "/")
            if overlapsConfigGuest && !isLocalAppSource {
                throw ProjectRepositoryError.escapedWorkspace
            }

            let relative = String(guestPath.dropFirst(homeGuest.count + 1))
            let hostPath = try canonicalUserHomeDirectory(homeHost, relative: relative)
            guard hostPath != homeHost,
                  hostPath.path.hasPrefix(homeHost.path + "/") else {
                throw ProjectRepositoryError.escapedWorkspace
            }
            if overlaps(hostPath, primaryProjectsHost) {
                throw ProjectRepositoryError.escapedWorkspace
            }
            let overlapsConfigHost = overlaps(hostPath, configHost)
            if overlapsConfigHost && !(isLocalAppSource && hostPath.path.hasPrefix(appsHost.path + "/")) {
                throw ProjectRepositoryError.escapedWorkspace
            }
            if overlaps(hostPath, appsHost) && !isLocalAppSource {
                throw ProjectRepositoryError.escapedWorkspace
            }
            if grants.contains(where: { pathsOverlap($0, guestPath) }) {
                throw ProjectRepositoryError.escapedWorkspace
            }
            grants.append(guestPath)
        }
        return grants
    }

    static func projectMountGrantPaths(
        for project: ProjectSnapshot,
        managedRoot: String = LXISHDefaultWorkspace.managedRootPath()
    ) throws -> [String] {
        try projectMountGrantPaths(
            activeDirectory: project.record.activeDirectory,
            additionalDirectories: project.record.additionalDirectories,
            managedRoot: managedRoot
        )
    }

    private static func isNormalizedGuestPath(_ path: String) -> Bool {
        guard path.hasPrefix("/"), path.count > 1, !path.hasSuffix("/"), !path.contains("\\") else {
            return false
        }
        return path.split(separator: "/", omittingEmptySubsequences: false).dropFirst()
            .allSatisfy { !$0.isEmpty && $0 != "." && $0 != ".." }
    }

    private static func canonicalUserHomeDirectory(_ root: URL, relative: String) throws -> URL {
        var current = root
        for component in relative.split(separator: "/", omittingEmptySubsequences: false) {
            guard !component.isEmpty, component != ".", component != ".." else {
                throw ProjectRepositoryError.escapedWorkspace
            }
            current.appendPathComponent(String(component), isDirectory: true)
            var info = stat()
            let result = current.path.withCString { lstat($0, &info) }
            guard result == 0, (info.st_mode & S_IFMT) == S_IFDIR else {
                throw ProjectRepositoryError.escapedWorkspace
            }
        }
        let canonical = current.resolvingSymlinksInPath().standardizedFileURL
        guard canonical.path.hasPrefix(root.path + "/") else {
            throw ProjectRepositoryError.escapedWorkspace
        }
        return canonical
    }

    private static func overlaps(_ left: URL, _ right: URL) -> Bool {
        pathsOverlap(left.path, right.path)
    }

    private static func pathsOverlap(_ left: String, _ right: String) -> Bool {
        left == right || left.hasPrefix(right + "/") || right.hasPrefix(left + "/")
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
