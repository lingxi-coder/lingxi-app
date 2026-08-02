import Foundation

let projectIndexFileName = "index.json"
let projectManifestFileName = "project.json"
let projectSessionIndexFileName = "session-index.json"
let projectSyncBaselineFileName = "sync-baseline.json"
let projectWorkspaceDirectoryName = "workspace"
let globalSessionIndexFileName = "global-session-index.json"

protocol ProjectAtomicWriting: Sendable {
    func write<T: Codable>(_ value: T, to url: URL, encoder: JSONEncoder, decoder: JSONDecoder) throws
    func writeData(_ data: Data, to url: URL, validate: (Data) throws -> Void) throws
}

struct DefaultProjectAtomicWriter: ProjectAtomicWriting {
    func write<T: Codable>(_ value: T, to url: URL, encoder: JSONEncoder, decoder: JSONDecoder) throws {
        let data = try encoder.encode(value)
        try writeData(data, to: url) { try _ = decoder.decode(T.self, from: $0) }
    }

    func writeData(_ data: Data, to url: URL, validate: (Data) throws -> Void) throws {
        let fm = FileManager.default
        try fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let tempURL = url.deletingLastPathComponent()
            .appendingPathComponent(".\(url.lastPathComponent).\(UUID().uuidString).tmp")
        do {
            try data.write(to: tempURL, options: .atomic)
            let staged = try Data(contentsOf: tempURL)
            try validate(staged)
            if fm.fileExists(atPath: url.path) {
                _ = try fm.replaceItemAt(url, withItemAt: tempURL)
            } else {
                try fm.moveItem(at: tempURL, to: url)
            }
        } catch {
            try? fm.removeItem(at: tempURL)
            throw error
        }
    }
}

private struct ProjectIndex: Codable, Equatable {
    var version: Int = 1
    var activeProjectId: String? = nil
    var projectIds: [String] = []
}

private struct ProjectSessionIndex: Codable {
    var version: Int = 1
    var sessions: [ProjectSessionSummary]
}

private struct VersionedBaseline: Codable {
    var version: Int = 1
    var files: [ProjectSyncFile]
}

final class ProjectRepository: @unchecked Sendable {
    let projectsRoot: URL
    private let now: @Sendable () -> Date
    private let newId: @Sendable () -> String
    private let writer: ProjectAtomicWriting
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder

    init(
        projectsRoot: URL,
        now: @escaping @Sendable () -> Date = { Date() },
        newId: @escaping @Sendable () -> String = { UUID().uuidString.lowercased() },
        writer: ProjectAtomicWriting = DefaultProjectAtomicWriter()
    ) {
        self.projectsRoot = projectsRoot
        self.now = now
        self.newId = newId
        self.writer = writer
        self.encoder = JSONEncoder()
        self.decoder = JSONDecoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        decoder.dateDecodingStrategy = .iso8601
        try? FileManager.default.createDirectory(at: projectsRoot, withIntermediateDirectories: true)
    }

    static func appManagedProjectsRoot(appSandboxRoot: URL) -> URL {
        appSandboxRoot.appendingPathComponent("Projects", isDirectory: true)
    }

    func load() -> ProjectRepositoryState {
        try? FileManager.default.createDirectory(at: projectsRoot, withIntermediateDirectories: true)
        let index = readIndex()
        let fm = FileManager.default
        let directories = (try? fm.contentsOfDirectory(
            at: projectsRoot,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        )) ?? []
        let projectDirectories = directories.filter {
            isLowercaseUUID($0.lastPathComponent) &&
            ((try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) ?? false) == true
        }

        let loaded = projectDirectories.map { url in
            (url, Result { try loadProject(id: url.lastPathComponent) })
        }
        let discovered = loaded.compactMap { try? $0.1.get() }
            .sorted { $0.record.updatedAt > $1.record.updatedAt }
        let corruptCount = loaded.count { if case .failure = $0.1 { return true } else { return false } }
        let discoveredIds = discovered.map(\.record.id)
        let active = index.activeProjectId.flatMap { discoveredIds.contains($0) ? $0 : nil }
        if index.projectIds != discoveredIds || active != index.activeProjectId {
            try? writeIndex(ProjectIndex(activeProjectId: active, projectIds: discoveredIds))
        }
        return ProjectRepositoryState(
            projects: discovered,
            activeProjectId: active,
            globalSessions: readSessions(at: projectsRoot.appendingPathComponent(globalSessionIndexFileName)),
            loading: false,
            errorMessage: corruptCount > 0 ? "\(corruptCount) 个项目数据损坏或路径异常，已隔离；其他项目仍可使用。" : nil
        )
    }

    func createInternal(name: String) throws -> ProjectSnapshot {
        try createProject(name: name, storageKind: .internal, sourceBookmark: nil)
    }

    func createImported(name: String, sourceBookmark: ProjectExternalBookmark) throws -> ProjectSnapshot {
        try createProject(name: name, storageKind: .externalBookmarkMirror, sourceBookmark: sourceBookmark)
    }

    func setActiveProject(_ projectId: String?) throws -> ProjectRepositoryState {
        if let projectId {
            try validateProjectID(projectId)
            guard FileManager.default.fileExists(atPath: projectDirectory(projectId).appendingPathComponent(projectManifestFileName).path)
            else { throw ProjectRepositoryError.projectDoesNotExist(projectId) }
        }
        let current = load()
        try writeIndex(ProjectIndex(activeProjectId: projectId, projectIds: current.projects.map(\.record.id)))
        return load()
    }

    func updateProject(_ record: ProjectRecord) throws -> ProjectSnapshot {
        try validateProjectID(record.id)
        guard FileManager.default.fileExists(atPath: projectDirectory(record.id).path)
        else { throw ProjectRepositoryError.projectDoesNotExist(record.id) }
        var normalized = record
        normalized.updatedAt = now()
        try writeProject(normalized)
        return try loadProject(id: record.id)
    }

    func updateSessions(projectId: String?, sessions: [ProjectSessionSummary]) throws -> ProjectRepositoryState {
        let normalized = sessions
            .filter { !$0.sessionId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            .map { row in
                var row = row
                row.sessionId = canonicalSessionID(row.sessionId)
                return row
            }
            .uniqued(on: \.sessionId)
            .sorted { $0.updatedAt > $1.updatedAt }
        if let projectId {
            try validateProjectID(projectId)
            try writeSessions(projectId: projectId, sessions: normalized)
            let snapshot = try loadProject(id: projectId)
            let lastActive = snapshot.record.lastActiveSessionId.flatMap { id in
                normalized.contains(where: { $0.sessionId == id }) ? id : nil
            }
            try writeProject(snapshot.record.with(lastActiveSessionId: lastActive))
        } else {
            try writeSessionIndex(normalized, to: projectsRoot.appendingPathComponent(globalSessionIndexFileName))
        }
        return load()
    }

    func recordStartedSession(projectId: String?, sessionId: String, title: String) throws -> ProjectRepositoryState {
        let canonicalID = canonicalSessionID(sessionId)
        guard !canonicalID.isEmpty else { throw ProjectRepositoryError.invalidSessionID }
        let snapshot = try projectId.map { try loadProject(id: $0) }
        let sessions = snapshot?.sessions ?? load().globalSessions
        let timestamp = now()
        let existing = sessions.first(where: { $0.sessionId == canonicalID })
        let started = ProjectSessionSummary(
            sessionId: canonicalID,
            title: title.isEmpty ? (existing?.title ?? "新对话") : title,
            messageCount: existing?.messageCount ?? 0,
            relativeTime: "刚刚",
            updatedAt: timestamp
        )
        let updated = ([started] + sessions.filter { $0.sessionId != canonicalID })
        if let projectId {
            try writeSessions(projectId: projectId, sessions: updated)
            if let snapshot {
                try writeProject(snapshot.record.with(updatedAt: timestamp, lastActiveSessionId: canonicalID))
            }
        } else {
            try writeSessionIndex(updated, to: projectsRoot.appendingPathComponent(globalSessionIndexFileName))
        }
        return load()
    }

    func markActiveSession(projectId: String, sessionId: String) throws -> ProjectRepositoryState {
        try validateProjectID(projectId)
        let snapshot = try loadProject(id: projectId)
        guard snapshot.sessions.contains(where: { $0.sessionId == sessionId }) else {
            throw ProjectRepositoryError.sessionNotIndexed
        }
        try writeProject(snapshot.record.with(lastActiveSessionId: sessionId))
        return load()
    }

    func deleteProject(projectId: String) throws -> ProjectRepositoryState {
        try validateProjectID(projectId)
        let directory = projectDirectory(projectId)
        guard FileManager.default.fileExists(atPath: directory.path) else {
            throw ProjectRepositoryError.projectDoesNotExist(projectId)
        }
        try FileManager.default.removeItem(at: directory)
        let current = load()
        let nextIDs = current.projects.map(\.record.id).filter { $0 != projectId }
        let nextActive = current.activeProjectId == projectId ? nil : current.activeProjectId
        try writeIndex(ProjectIndex(activeProjectId: nextActive, projectIds: nextIDs))
        return load()
    }

    func readBaseline(projectId: String) throws -> ProjectSyncBaseline {
        try validateProjectID(projectId)
        let url = projectDirectory(projectId).appendingPathComponent(projectSyncBaselineFileName)
        guard FileManager.default.fileExists(atPath: url.path) else { return ProjectSyncBaseline() }
        let stored = try decode(VersionedBaseline.self, from: url)
        guard stored.version == 1 else { throw ProjectRepositoryError.unsupportedVersion }
        let files = try stored.files.map { file in
            guard isSafeRelativePath(file.relativePath) else {
                throw ProjectRepositoryError.unsafeBaselinePath(file.relativePath)
            }
            return file
        }
        return ProjectSyncBaseline(files: Dictionary(uniqueKeysWithValues: files.map { ($0.relativePath, $0) }))
    }

    func writeBaseline(projectId: String, baseline: ProjectSyncBaseline) throws {
        try validateProjectID(projectId)
        let files = try baseline.files.values.sorted { $0.relativePath < $1.relativePath }.map { file in
            guard isSafeRelativePath(file.relativePath) else {
                throw ProjectRepositoryError.unsafeBaselinePath(file.relativePath)
            }
            return file
        }
        try write(
            VersionedBaseline(files: files),
            to: projectDirectory(projectId).appendingPathComponent(projectSyncBaselineFileName)
        )
    }

    func workspace(projectId: String) throws -> ProjectWorkspace {
        try loadProject(id: projectId).workspace
    }

    func project(projectId: String) throws -> ProjectSnapshot {
        try loadProject(id: projectId)
    }

    private func createProject(
        name: String,
        storageKind: ProjectStorageKind,
        sourceBookmark: ProjectExternalBookmark?
    ) throws -> ProjectSnapshot {
        let id = newId()
        try validateProjectID(id)
        let cleanName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanName.isEmpty else { throw ProjectRepositoryError.blankProjectName }
        guard cleanName.count <= 120 else { throw ProjectRepositoryError.projectNameTooLong }
        let directory = projectDirectory(id)
        guard !FileManager.default.fileExists(atPath: directory.path) else {
            throw ProjectRepositoryError.projectAlreadyExists(id)
        }
        let workspaceURL = directory.appendingPathComponent(projectWorkspaceDirectoryName, isDirectory: true)
        let timestamp = now()
        let record = ProjectRecord(
            id: id,
            name: cleanName,
            storageKind: storageKind,
            createdAt: timestamp,
            updatedAt: timestamp,
            sourceBookmark: sourceBookmark
        )
        do {
            try FileManager.default.createDirectory(at: workspaceURL, withIntermediateDirectories: true)
            try writeProject(record)
            try writeSessions(projectId: id, sessions: [])
            try writeBaseline(projectId: id, baseline: ProjectSyncBaseline())
            let index = readIndex()
            try writeIndex(ProjectIndex(
                activeProjectId: index.activeProjectId,
                projectIds: (index.projectIds + [id]).uniqued()
            ))
        } catch {
            try? FileManager.default.removeItem(at: directory)
            throw error
        }
        return try loadProject(id: id)
    }

    private func loadProject(id projectId: String) throws -> ProjectSnapshot {
        try validateProjectID(projectId)
        let directory = projectDirectory(projectId)
        let canonicalRoot = try requireCanonicalDirectory(projectsRoot)
        let canonicalDir = try requireCanonicalDirectory(directory)
        guard canonicalDir.deletingLastPathComponent() == canonicalRoot else {
            throw ProjectRepositoryError.escapedProjectsRoot
        }
        let record = try decode(ProjectRecord.self, from: directory.appendingPathComponent(projectManifestFileName))
        guard record.id == projectId else { throw ProjectRepositoryError.manifestMismatch }
        let workspaceURL = directory.appendingPathComponent(projectWorkspaceDirectoryName, isDirectory: true)
        if !FileManager.default.fileExists(atPath: workspaceURL.path) {
            try FileManager.default.createDirectory(at: workspaceURL, withIntermediateDirectories: true)
        }
        let canonicalWorkspace = try requireCanonicalDirectory(workspaceURL)
        guard canonicalWorkspace.deletingLastPathComponent() == canonicalDir else {
            throw ProjectRepositoryError.escapedWorkspace
        }
        return ProjectSnapshot(
            record: record,
            workspace: ProjectWorkspace(projectId: projectId, hostURL: canonicalWorkspace),
            sessions: readSessions(at: directory.appendingPathComponent(projectSessionIndexFileName))
        )
    }

    private func projectDirectory(_ projectId: String) -> URL {
        projectsRoot.appendingPathComponent(projectId, isDirectory: true)
    }

    private func validateProjectID(_ projectId: String) throws {
        guard isLowercaseUUID(projectId) else { throw ProjectRepositoryError.invalidProjectID }
    }

    private func writeProject(_ record: ProjectRecord) throws {
        try write(record, to: projectDirectory(record.id).appendingPathComponent(projectManifestFileName))
    }

    private func writeSessions(projectId: String, sessions: [ProjectSessionSummary]) throws {
        try writeSessionIndex(sessions, to: projectDirectory(projectId).appendingPathComponent(projectSessionIndexFileName))
    }

    private func writeSessionIndex(_ sessions: [ProjectSessionSummary], to url: URL) throws {
        try write(ProjectSessionIndex(sessions: sessions), to: url)
    }

    private func readSessions(at url: URL) -> [ProjectSessionSummary] {
        guard FileManager.default.fileExists(atPath: url.path),
              let decoded = try? decode(ProjectSessionIndex.self, from: url),
              decoded.version == 1
        else { return [] }
        return decoded.sessions.map {
            var row = $0
            row.sessionId = canonicalSessionID(row.sessionId)
            return row
        }
    }

    private func readIndex() -> ProjectIndex {
        let url = projectsRoot.appendingPathComponent(projectIndexFileName)
        guard FileManager.default.fileExists(atPath: url.path) else { return ProjectIndex() }
        do {
            let decoded = try decode(ProjectIndex.self, from: url)
            guard decoded.version == 1 else { throw ProjectRepositoryError.unsupportedVersion }
            return ProjectIndex(
                activeProjectId: decoded.activeProjectId,
                projectIds: decoded.projectIds.filter(isLowercaseUUID).uniqued()
            )
        } catch {
            let recovered = ProjectIndex()
            try? writeIndex(recovered)
            return recovered
        }
    }

    private func writeIndex(_ index: ProjectIndex) throws {
        try write(index, to: projectsRoot.appendingPathComponent(projectIndexFileName))
    }

    private func write<T: Codable>(_ value: T, to url: URL) throws {
        try writer.write(value, to: url, encoder: encoder, decoder: decoder)
    }

    private func decode<T: Decodable>(_ type: T.Type, from url: URL) throws -> T {
        let data = try Data(contentsOf: url)
        return try decoder.decode(type, from: data)
    }
}

enum ProjectRepositoryError: LocalizedError {
    case invalidProjectID
    case invalidSessionID
    case blankProjectName
    case projectNameTooLong
    case projectAlreadyExists(String)
    case projectDoesNotExist(String)
    case sessionNotIndexed
    case manifestMismatch
    case escapedProjectsRoot
    case escapedWorkspace
    case unsupportedVersion
    case unsafeBaselinePath(String)

    var errorDescription: String? {
        switch self {
        case .invalidProjectID: return "project id must be a lowercase UUID"
        case .invalidSessionID: return "session id cannot be blank"
        case .blankProjectName: return "project name cannot be blank"
        case .projectNameTooLong: return "project name is too long"
        case .projectAlreadyExists(let id): return "project already exists: \(id)"
        case .projectDoesNotExist(let id): return "project does not exist: \(id)"
        case .sessionNotIndexed: return "session is not indexed by project"
        case .manifestMismatch: return "project manifest id does not match directory"
        case .escapedProjectsRoot: return "project directory escaped the managed projects root"
        case .escapedWorkspace: return "project workspace escaped its managed project directory"
        case .unsupportedVersion: return "unsupported project data version"
        case .unsafeBaselinePath(let path): return "unsafe baseline path: \(path)"
        }
    }
}

func isLowercaseUUID(_ value: String) -> Bool {
    value.count == 36 &&
    value == value.lowercased() &&
    UUID(uuidString: value)?.uuidString.lowercased() == value
}

func isSafeRelativePath(_ value: String) -> Bool {
    if value.isEmpty || value.hasPrefix("/") || value.hasPrefix("\\") { return false }
    let parts = value.replacingOccurrences(of: "\\", with: "/").split(separator: "/")
    return !parts.isEmpty && parts.allSatisfy { !$0.isEmpty && $0 != "." && $0 != ".." }
}

func canonicalSessionID(_ value: String) -> String {
    if value.hasPrefix("sess:") {
        return String(value.dropFirst("sess:".count))
    }
    return value
}

func requireCanonicalDirectory(_ url: URL) throws -> URL {
    let standardized = url.standardizedFileURL
    let values = try standardized.resourceValues(forKeys: [.isSymbolicLinkKey, .isDirectoryKey])
    guard values.isSymbolicLink != true else {
        throw ProjectRepositoryError.escapedWorkspace
    }
    let canonical = standardized.resolvingSymlinksInPath()
    let canonicalValues = try canonical.resourceValues(forKeys: [.isDirectoryKey])
    guard canonicalValues.isDirectory == true else {
        throw CocoaError(.fileReadUnknown)
    }
    guard canonical.path == standardized.path else {
        throw ProjectRepositoryError.escapedWorkspace
    }
    return canonical
}

func isSafeProjectWorkspacePath(root: URL, candidate: URL) -> Bool {
    let normalizedRoot = root.standardizedFileURL
    let normalizedCandidate = candidate.standardizedFileURL
    guard normalizedCandidate.path != normalizedRoot.path,
          normalizedCandidate.path.hasPrefix(normalizedRoot.path + "/")
    else { return false }

    var current = normalizedCandidate
    while current.path != normalizedRoot.path {
        let isSymlink = (try? current.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink) ?? false
        if isSymlink { return false }
        guard let parent = current.deletingLastPathComponentIfPossible() else { return false }
        current = parent
    }
    let rootIsSymlink = (try? normalizedRoot.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink) ?? false
    return !rootIsSymlink
}

private extension URL {
    func deletingLastPathComponentIfPossible() -> URL? {
        let next = deletingLastPathComponent()
        return next.path == path ? nil : next
    }
}

private extension Array {
    func uniqued<HashableValue: Hashable>(on keyPath: KeyPath<Element, HashableValue>) -> [Element] {
        var seen = Set<HashableValue>()
        return filter { seen.insert($0[keyPath: keyPath]).inserted }
    }
}

private extension Array where Element: Hashable {
    func uniqued() -> [Element] {
        var seen = Set<Element>()
        return filter { seen.insert($0).inserted }
    }
}

extension ProjectRecord {
    func with(
        updatedAt: Date? = nil,
        lastActiveSessionId: String? = nil,
        syncState: ProjectSyncState? = nil
    ) -> ProjectRecord {
        var copy = self
        if let updatedAt { copy.updatedAt = updatedAt }
        copy.lastActiveSessionId = lastActiveSessionId
        if let syncState { copy.syncState = syncState }
        return copy
    }
}
