import Foundation

struct WorkspaceDirectoryEntry: Identifiable, Equatable, Sendable {
    let path: String
    let name: String
    let isDirectory: Bool
    let isSymbolicLink: Bool
    let byteCount: Int64
    var id: String { path }
}

struct WorkspaceLocation: Identifiable, Codable, Equatable, Sendable {
    let id: UUID
    var name: String
    var path: String
}

struct WorkspaceDirectoryPaths: Sendable {
    let home: URL
    let guestHome: String
    let workspace: String
    let projects: String
    let settings: String
    let savedLocations: URL

    init(home: URL, guestHome: String, workspace: String, projects: String, settings: String) {
        self.home = home.standardizedFileURL
        self.guestHome = guestHome
        self.workspace = workspace
        self.projects = projects
        self.settings = settings
        self.savedLocations = home.appendingPathComponent(settings + "/workspace-locations.json")
    }

    static func current() -> Self {
        let paths = MobileUserStorage.directories()
        return Self(home: URL(fileURLWithPath: paths.home, isDirectory: true),
                    guestHome: paths.guestHome, workspace: paths.defaultGuestCwd,
                    projects: paths.projectsGuestPath,
                    settings: URL(fileURLWithPath: paths.configHome).lastPathComponent)
    }

    func display(_ path: String) -> String {
        if path == guestHome { return "~" }
        return path.hasPrefix(guestHome + "/") ? "~" + path.dropFirst(guestHome.count) : path
    }
}

enum WorkspaceDirectoryError: LocalizedError {
    case outsideHome, invalidName, protectedDirectory, notDirectory, alreadyExists, sessionChanged, unavailable, busy

    var errorDescription: String? {
        switch self {
        case .outsideHome: WorkspaceBrowserStrings.text("outside_home")
        case .invalidName: WorkspaceBrowserStrings.text("invalid_name")
        case .protectedDirectory: WorkspaceBrowserStrings.text("in_use")
        case .notDirectory: WorkspaceBrowserStrings.text("not_directory")
        case .alreadyExists: WorkspaceBrowserStrings.text("already_exists")
        case .sessionChanged: String(localized: "workspace_session_changed")
        case .unavailable: WorkspaceBrowserStrings.text("unavailable")
        case .busy: WorkspaceBrowserStrings.text("busy")
        }
    }
}

/// The SDK mounts this persistent home into Linux. Native browsing operates on
/// the same files without starting a shell or interpreting user-entered names.
actor WorkspaceDirectoryRepository {
    let paths: WorkspaceDirectoryPaths
    private let fm = FileManager.default

    init(paths: WorkspaceDirectoryPaths = .current()) { self.paths = paths }

    func entries(at path: String, includingHidden: Bool) throws -> [WorkspaceDirectoryEntry] {
        let directory = try resolve(path)
        let keys: Set<URLResourceKey> = [.isDirectoryKey, .isSymbolicLinkKey, .fileSizeKey]
        return try fm.contentsOfDirectory(at: directory, includingPropertiesForKeys: Array(keys),
                                          options: includingHidden ? [] : [.skipsHiddenFiles])
            .map { url in
                let value = try url.resourceValues(forKeys: keys)
                return WorkspaceDirectoryEntry(path: child(url.lastPathComponent, of: path),
                    name: url.lastPathComponent, isDirectory: value.isDirectory == true,
                    isSymbolicLink: value.isSymbolicLink == true, byteCount: Int64(value.fileSize ?? 0))
            }
            .sorted {
                if $0.isDirectory != $1.isDirectory { return $0.isDirectory }
                return $0.name.localizedStandardCompare($1.name) == .orderedAscending
            }
    }

    func validateDirectory(_ path: String) throws { _ = try resolve(path) }

    func canonicalDirectory(_ path: String) throws -> String { guestPath(for: try resolve(path)) }

    func locations() throws -> [WorkspaceLocation] {
        guard fm.fileExists(atPath: paths.savedLocations.path) else { return [] }
        _ = try resolve(paths.guestHome + "/" + paths.settings)
        try rejectLinkedLocationFile()
        return try JSONDecoder().decode([WorkspaceLocation].self, from: Data(contentsOf: paths.savedLocations))
    }

    @discardableResult
    func saveLocation(path: String, name: String, id: UUID? = nil) throws -> [WorkspaceLocation] {
        let path = try canonicalDirectory(path)
        let label = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !label.isEmpty else { throw WorkspaceDirectoryError.invalidName }
        var rows = try locations()
        if let id {
            guard !rows.contains(where: { $0.path == path && $0.id != id }) else {
                throw WorkspaceDirectoryError.alreadyExists
            }
            guard let index = rows.firstIndex(where: { $0.id == id }) else {
                throw WorkspaceDirectoryError.notDirectory
            }
            rows[index].name = label
            rows[index].path = path
        } else if rows.contains(where: { $0.path == path }) {
            return rows
        } else {
            rows.append(WorkspaceLocation(id: UUID(), name: label, path: path))
        }
        try writeLocations(rows)
        return rows
    }

    @discardableResult
    func removeLocation(id: UUID) throws -> [WorkspaceLocation] {
        let rows = try locations().filter { $0.id != id }
        try writeLocations(rows)
        return rows
    }

    func createDirectory(in parent: String, name: String) throws -> String {
        try requireWritable(parent)
        let name = try validName(name)
        let parentURL = try resolve(parent)
        let destination = parentURL.appendingPathComponent(name, isDirectory: true)
        guard !fm.fileExists(atPath: destination.path) else { throw WorkspaceDirectoryError.alreadyExists }
        try fm.createDirectory(at: destination, withIntermediateDirectories: false)
        return guestPath(for: destination)
    }

    func renameDirectory(_ path: String, name: String, protectedPaths: [String]) throws -> String {
        try requireMutable(path, protectedPaths: protectedPaths)
        let source = try resolve(path)
        let path = guestPath(for: source)
        let name = try validName(name)
        let destination = source.deletingLastPathComponent().appendingPathComponent(name, isDirectory: true)
        guard !fm.fileExists(atPath: destination.path) else { throw WorkspaceDirectoryError.alreadyExists }
        let nextPath = child(name, of: (path as NSString).deletingLastPathComponent)
        let rows = try locations().map { row in
            var row = row
            if row.path == path || row.path.hasPrefix(path + "/") {
                row.path = nextPath + row.path.dropFirst(path.count)
                if row.name == source.lastPathComponent { row.name = name }
            }
            return row
        }
        try fm.moveItem(at: source, to: destination)
        do { try writeLocations(rows) }
        catch {
            try? fm.moveItem(at: destination, to: source)
            throw error
        }
        return nextPath
    }

    func deleteDirectory(_ path: String, protectedPaths: [String]) throws {
        try requireMutable(path, protectedPaths: protectedPaths)
        let directory = try resolve(path)
        let path = guestPath(for: directory)
        let previous = try locations()
        try writeLocations(previous.filter { $0.path != path && !$0.path.hasPrefix(path + "/") })
        do { try fm.removeItem(at: directory) }
        catch {
            try? writeLocations(previous)
            throw error
        }
    }

    private func guestPath(for url: URL) -> String {
        let root = paths.home.resolvingSymlinksInPath().standardizedFileURL.path
        return paths.guestHome + url.path.dropFirst(root.count)
    }

    private func child(_ name: String, of parent: String) -> String { parent + "/" + name }

    private func validName(_ value: String) throws -> String {
        let name = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name != ".", name != "..", !name.contains("/"),
              !name.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
        else { throw WorkspaceDirectoryError.invalidName }
        return name
    }

    private func resolve(_ path: String) throws -> URL {
        guard path == paths.guestHome || path.hasPrefix(paths.guestHome + "/"),
              !path.contains("\0"), !path.split(separator: "/").contains(where: { $0 == "." || $0 == ".." })
        else { throw WorkspaceDirectoryError.outsideHome }
        guard (try? paths.home.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink) != true
        else { throw WorkspaceDirectoryError.outsideHome }
        let root = paths.home.resolvingSymlinksInPath().standardizedFileURL
        let suffix = String(path.dropFirst(paths.guestHome.count)).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let url = root.appendingPathComponent(suffix, isDirectory: true).resolvingSymlinksInPath().standardizedFileURL
        guard url.path == root.path || url.path.hasPrefix(root.path + "/") else { throw WorkspaceDirectoryError.outsideHome }
        var directory: ObjCBool = false
        guard fm.fileExists(atPath: url.path, isDirectory: &directory), directory.boolValue
        else { throw WorkspaceDirectoryError.notDirectory }
        return url
    }

    private func requireWritable(_ path: String) throws {
        let settings = paths.guestHome + "/" + paths.settings
        guard path != settings, !path.hasPrefix(settings + "/") else { throw WorkspaceDirectoryError.protectedDirectory }
        let resolved = try resolve(path)
        let settingsURL = paths.home.appendingPathComponent(paths.settings).resolvingSymlinksInPath()
        guard resolved.path != settingsURL.path, !resolved.path.hasPrefix(settingsURL.path + "/")
        else { throw WorkspaceDirectoryError.protectedDirectory }
    }

    private func requireMutable(_ path: String, protectedPaths: [String]) throws {
        try requireWritable(path)
        let protected = [paths.guestHome, paths.workspace, paths.projects] + protectedPaths
        guard !protected.contains(where: { $0 == path || $0.hasPrefix(path + "/") })
        else { throw WorkspaceDirectoryError.protectedDirectory }
        let target = try resolve(path)
        for protectedPath in protected {
            if let protectedURL = try? resolve(protectedPath),
               protectedURL.path == target.path || protectedURL.path.hasPrefix(target.path + "/") {
                throw WorkspaceDirectoryError.protectedDirectory
            }
        }
        // A link is a directory shortcut, not a directory to rename recursively.
        let suffix = String(path.dropFirst(paths.guestHome.count)).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let original = paths.home.appendingPathComponent(suffix)
        guard (try original.resourceValues(forKeys: [.isSymbolicLinkKey])).isSymbolicLink != true
        else { throw WorkspaceDirectoryError.protectedDirectory }
    }

    private func rejectLinkedLocationFile() throws {
        if let type = (try? fm.attributesOfItem(atPath: paths.savedLocations.path))?[.type] as? FileAttributeType,
           type == .typeSymbolicLink {
            throw WorkspaceDirectoryError.outsideHome
        }
    }

    private func writeLocations(_ rows: [WorkspaceLocation]) throws {
        let settings = paths.guestHome + "/" + paths.settings
        let settingsURL = paths.home.appendingPathComponent(paths.settings, isDirectory: true)
        if !fm.fileExists(atPath: settingsURL.path) {
            try fm.createDirectory(at: settingsURL, withIntermediateDirectories: false)
        }
        _ = try resolve(settings)
        try rejectLinkedLocationFile()
        try JSONEncoder().encode(rows).write(to: paths.savedLocations, options: .atomic)
    }
}
