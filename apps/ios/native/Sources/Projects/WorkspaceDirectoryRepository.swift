import Foundation

struct WorkspaceDirectoryEntry: Identifiable, Equatable, Sendable {
    let path: String
    let name: String
    let isDirectory: Bool
    let isSymbolicLink: Bool
    let byteCount: Int64
    var modified: Date = .distantPast
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
        let keys: Set<URLResourceKey> = [.isDirectoryKey, .isSymbolicLinkKey, .fileSizeKey, .contentModificationDateKey]
        return try fm.contentsOfDirectory(at: directory, includingPropertiesForKeys: Array(keys),
                                          options: includingHidden ? [] : [.skipsHiddenFiles])
            .map { url in
                let value = try url.resourceValues(forKeys: keys)
                return WorkspaceDirectoryEntry(path: child(url.lastPathComponent, of: path),
                    name: url.lastPathComponent, isDirectory: value.isDirectory == true,
                    isSymbolicLink: value.isSymbolicLink == true, byteCount: Int64(value.fileSize ?? 0), modified: value.contentModificationDate ?? .distantPast)
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
        let source = try resolve(path, directoryOnly: false)
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
        let directory = try resolve(path, directoryOnly: false)
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

    func resolve(_ path: String, directoryOnly: Bool = true) throws -> URL {
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
        guard fm.fileExists(atPath: url.path, isDirectory: &directory), (!directoryOnly || directory.boolValue)
        else { throw WorkspaceDirectoryError.notDirectory }
        return url
    }

    private func requireWritable(_ path: String) throws {
        let settings = paths.guestHome + "/" + paths.settings
        guard path != settings, !path.hasPrefix(settings + "/") else { throw WorkspaceDirectoryError.protectedDirectory }
        let resolved = try resolve(path, directoryOnly: false)
        let settingsURL = paths.home.appendingPathComponent(paths.settings).resolvingSymlinksInPath()
        guard resolved.path != settingsURL.path, !resolved.path.hasPrefix(settingsURL.path + "/")
        else { throw WorkspaceDirectoryError.protectedDirectory }
    }

    private func requireMutable(_ path: String, protectedPaths: [String]) throws {
        try requireWritable(path)
        let protected = [paths.guestHome, paths.workspace, paths.projects] + protectedPaths
        guard !protected.contains(where: { $0 == path || $0.hasPrefix(path + "/") })
        else { throw WorkspaceDirectoryError.protectedDirectory }
        let target = try resolve(path, directoryOnly: false)
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

// File operations share the directory picker's account and protection rules.
enum WorkspaceFileConflict: Sendable { case keepBoth, skip, cancel }
enum WorkspaceFileSort: String, CaseIterable, Identifiable { case name, modified, size, type; var id: String { rawValue } }
struct WorkspaceFileProgress: Sendable { let completed: Int; let total: Int; let name: String }
struct WorkspaceFileFailure: Sendable { let path: String; let message: String }
struct WorkspaceFileSnapshot: Identifiable, Sendable {
    let id = UUID()
    let directory: URL
    let url: URL
    let originalPath: String
    func remove() { try? FileManager.default.removeItem(at: directory) }
}

extension WorkspaceDirectoryEntry {
    var fileExtension: String { (name as NSString).pathExtension.lowercased() }
    var symbol: String {
        if isSymbolicLink { return "link" }
        if isDirectory { return "folder.fill" }
        switch fileExtension {
        case "png", "jpg", "jpeg", "heic", "gif", "webp": return "photo"
        case "pdf": return "doc.richtext"
        case "mp4", "mov", "m4v", "webm": return "film"
        case "mp3", "wav", "m4a", "flac", "aac": return "waveform"
        case "zip", "gz", "tar", "7z": return "doc.zipper"
        case "swift", "kt", "rs", "js", "ts", "py", "json", "html", "css", "sh", "yaml", "yml": return "curlybraces"
        default: return "doc.text"
        }
    }
    static func sorted(_ values: [Self], by sort: WorkspaceFileSort) -> [Self] {
        values.sorted { a, b in
            if a.isDirectory != b.isDirectory { return a.isDirectory }
            switch sort {
            case .modified: if a.modified != b.modified { return a.modified > b.modified }
            case .size: if a.byteCount != b.byteCount { return a.byteCount > b.byteCount }
            case .type: if a.fileExtension != b.fileExtension { return a.fileExtension < b.fileExtension }
            case .name: break
            }
            return a.name.localizedStandardCompare(b.name) == .orderedAscending
        }
    }
}

extension WorkspaceDirectoryRepository {
    func files(at path: String, hidden: Bool, query: String, recursive: Bool, sort: WorkspaceFileSort = .name) throws -> [WorkspaceDirectoryEntry] {
        var results: [WorkspaceDirectoryEntry] = []
        var pending = [path]
        while let directory = pending.popLast() {
            try Task.checkCancellation()
            let children = try entries(at: directory, includingHidden: hidden)
            for entry in children {
                try Task.checkCancellation()
                if query.isEmpty || entry.name.localizedStandardContains(query) { results.append(entry) }
                if recursive && !query.isEmpty && entry.isDirectory && !entry.isSymbolicLink { pending.append(entry.path) }
            }
        }
        return WorkspaceDirectoryEntry.sorted(results, by: sort)
    }

    func snapshot(_ path: String) throws -> WorkspaceFileSnapshot {
        let source = try resolve(path, directoryOnly: false)
        guard try source.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile == true else {
            throw WorkspaceDirectoryError.notDirectory
        }
        let directory = fm.temporaryDirectory.appendingPathComponent("lingxi-file-" + UUID().uuidString, isDirectory: true)
        try fm.createDirectory(at: directory, withIntermediateDirectories: false)
        let target = directory.appendingPathComponent(source.lastPathComponent)
        do { try copyFile(source, to: target); return WorkspaceFileSnapshot(directory: directory, url: target, originalPath: path) }
        catch { try? fm.removeItem(at: directory); throw error }
    }

    func textPreview(_ snapshot: WorkspaceFileSnapshot) throws -> String {
        let handle = try FileHandle(forReadingFrom: snapshot.url)
        defer { try? handle.close() }
        let data = try handle.read(upToCount: 2 * 1024 * 1024 + 1) ?? Data()
        guard data.count <= 2 * 1024 * 1024 else { throw FileBrowserError.message(FilesStrings.text("large_text")) }
        guard !data.contains(0), let text = String(data: data, encoding: .utf8) else {
            throw FileBrowserError.message(FilesStrings.text("binary"))
        }
        return text
    }

    func transfer(_ sources: [String], to destination: String, move: Bool, conflict: WorkspaceFileConflict,
                  protectedPaths: [String], progress: @Sendable (WorkspaceFileProgress) async -> Void) async throws -> [WorkspaceFileFailure] {
        let parent = try resolve(destination)
        try requireWritable(destination)
        var failures: [WorkspaceFileFailure] = []
        // Parents cover their selected children; do not copy or delete children twice.
        let selected = sources.filter { child in !sources.contains { child != $0 && child.hasPrefix($0 + "/") } }
        for (index, path) in selected.enumerated() {
            try Task.checkCancellation()
            do {
                let source = try resolve(path, directoryOnly: false)
                if move { try requireMutable(path, protectedPaths: protectedPaths) }
                guard parent.path != source.path, !parent.path.hasPrefix(source.path + "/") else {
                    throw FileBrowserError.message(FilesStrings.text("self_move"))
                }
                guard !isLink(path) else { throw WorkspaceDirectoryError.protectedDirectory }
                let target = try availableTarget(parent, name: source.lastPathComponent, conflict: conflict)
                if let target {
                    if move {
                        try fm.moveItem(at: source, to: target)
                        do { try updateLocations(from: guestPath(for: source), to: guestPath(for: target)) }
                        catch { try? fm.moveItem(at: target, to: source); throw error }
                    } else { try stagedCopy(source, to: target) }
                }
            } catch is CancellationError { throw CancellationError() }
            catch { failures.append(.init(path: path, message: error.localizedDescription)) }
            await progress(.init(completed: index + 1, total: selected.count, name: (path as NSString).lastPathComponent))
        }
        return failures
    }

    func deleteFiles(_ paths: [String], protectedPaths: [String],
                     progress: @Sendable (WorkspaceFileProgress) async -> Void) async throws -> [WorkspaceFileFailure] {
        let selected = paths.filter { child in !paths.contains { child != $0 && child.hasPrefix($0 + "/") } }
        var failures: [WorkspaceFileFailure] = []
        for (index, path) in selected.enumerated() {
            try Task.checkCancellation()
            do { try deleteDirectory(path, protectedPaths: protectedPaths) }
            catch { failures.append(.init(path: path, message: error.localizedDescription)) }
            await progress(.init(completed: index + 1, total: selected.count, name: (path as NSString).lastPathComponent))
        }
        return failures
    }

    /// System URLs are only read inside their picker-granted scope. Reject links while importing.
    func importItems(_ urls: [URL], to destination: String, conflict: WorkspaceFileConflict,
                     progress: @Sendable (WorkspaceFileProgress) async -> Void) async throws -> [WorkspaceFileFailure] {
        try requireWritable(destination)
        let parent = try resolve(destination)
        var failures: [WorkspaceFileFailure] = []
        for (index, url) in urls.enumerated() {
            try Task.checkCancellation()
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            do {
                if let target = try availableTarget(parent, name: validName(url.lastPathComponent), conflict: conflict) {
                    try stagedCopy(url, to: target)
                }
            } catch is CancellationError { throw CancellationError() }
            catch { failures.append(.init(path: url.lastPathComponent, message: error.localizedDescription)) }
            await progress(.init(completed: index + 1, total: urls.count, name: url.lastPathComponent))
        }
        return failures
    }

    func exportItems(_ paths: [String], to destination: URL, conflict: WorkspaceFileConflict,
                     progress: @Sendable (WorkspaceFileProgress) async -> Void) async throws -> [WorkspaceFileFailure] {
        let scoped = destination.startAccessingSecurityScopedResource()
        defer { if scoped { destination.stopAccessingSecurityScopedResource() } }
        var failures: [WorkspaceFileFailure] = []
        for (index, path) in paths.enumerated() {
            try Task.checkCancellation()
            do {
                let source = try resolve(path, directoryOnly: false)
                if let target = try availableTarget(destination, name: source.lastPathComponent, conflict: conflict) {
                    try stagedCopy(source, to: target)
                }
            } catch is CancellationError { throw CancellationError() }
            catch { failures.append(.init(path: path, message: error.localizedDescription)) }
            await progress(.init(completed: index + 1, total: paths.count, name: (path as NSString).lastPathComponent))
        }
        return failures
    }

    private func availableTarget(_ parent: URL, name: String, conflict: WorkspaceFileConflict) throws -> URL? {
        var target = parent.appendingPathComponent(name)
        guard fm.fileExists(atPath: target.path) || (try? target.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink) == true else { return target }
        switch conflict {
        case .skip: return nil
        case .cancel: throw CancellationError()
        case .keepBoth:
            let ext = (name as NSString).pathExtension
            let stem = ext.isEmpty ? name : (name as NSString).deletingPathExtension
            var index = 2
            repeat {
                target = parent.appendingPathComponent(stem + " (\(index))" + (ext.isEmpty ? "" : "." + ext)); index += 1
            } while fm.fileExists(atPath: target.path) || (try? target.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink) == true
            return target
        }
    }

    private func isLink(_ path: String) -> Bool {
        let suffix = path.dropFirst(paths.guestHome.count).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        return (try? paths.home.appendingPathComponent(suffix).resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink) == true
    }
    private func updateLocations(from old: String, to next: String) throws {
        let rows = try locations().map { row in
            var row = row
            if row.path == old || row.path.hasPrefix(old + "/") { row.path = next + row.path.dropFirst(old.count) }
            return row
        }
        try writeLocations(rows)
    }
    private func stagedCopy(_ source: URL, to target: URL) throws {
        let sourcePath = source.resolvingSymlinksInPath().standardizedFileURL.path
        let targetPath = target.resolvingSymlinksInPath().standardizedFileURL.path
        guard targetPath != sourcePath, !targetPath.hasPrefix(sourcePath + "/") else { throw FileBrowserError.message(FilesStrings.text("self_move")) }
        let staging = target.deletingLastPathComponent().appendingPathComponent(".lingxi-copy-" + UUID().uuidString)
        defer { try? fm.removeItem(at: staging) }
        try copyTree(source, to: staging)
        try Task.checkCancellation()
        guard !fm.fileExists(atPath: target.path) else { throw WorkspaceDirectoryError.alreadyExists }
        try fm.moveItem(at: staging, to: target)
    }
    private func copyTree(_ source: URL, to target: URL) throws {
        try Task.checkCancellation()
        let values = try source.resourceValues(forKeys: [.isSymbolicLinkKey, .isDirectoryKey, .isRegularFileKey])
        guard values.isSymbolicLink != true else { throw WorkspaceDirectoryError.protectedDirectory }
        if values.isDirectory == true {
            try fm.createDirectory(at: target, withIntermediateDirectories: false)
            for child in try fm.contentsOfDirectory(at: source, includingPropertiesForKeys: nil) {
                try copyTree(child, to: target.appendingPathComponent(child.lastPathComponent))
            }
        } else {
            guard values.isRegularFile == true else { throw WorkspaceDirectoryError.notDirectory }
            try copyFile(source, to: target)
        }
    }
    private func copyFile(_ source: URL, to target: URL) throws {
        guard fm.createFile(atPath: target.path, contents: nil) else { throw WorkspaceDirectoryError.unavailable }
        let input = try FileHandle(forReadingFrom: source)
        defer { try? input.close() }
        let output = try FileHandle(forWritingTo: target)
        defer { try? output.close() }
        while let data = try input.read(upToCount: 256 * 1024), !data.isEmpty {
            try Task.checkCancellation()
            try output.write(contentsOf: data)
        }
    }
}
