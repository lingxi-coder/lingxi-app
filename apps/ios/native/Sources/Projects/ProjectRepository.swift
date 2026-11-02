import Foundation

let projectIndexFileName = "index.json"
let projectManifestFileName = "project.json"
let projectSessionIndexFileName = "session-spaces.json"
let projectSyncBaselineFileName = "sync-baseline.json"
let projectWorkspaceDirectoryName = "workspace"
let globalSessionIndexFileName = "global-session-spaces.json"

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

    init(version: Int = 1, activeProjectId: String? = nil, projectIds: [String] = []) {
        self.version = version
        self.activeProjectId = activeProjectId
        self.projectIds = projectIds
    }

    private enum CodingKeys: String, CodingKey { case version, activeProjectId, projectIds }

    init(from decoder: any Decoder) throws {
        let fields = try decoder.container(keyedBy: CodingKeys.self)
        version = try fields.decode(Int.self, forKey: .version)
        activeProjectId = try fields.decodeIfPresent(String.self, forKey: .activeProjectId)
        projectIds = try fields.decode([String].self, forKey: .projectIds)
    }

    func encode(to encoder: any Encoder) throws {
        var fields = encoder.container(keyedBy: CodingKeys.self)
        try fields.encode(version, forKey: .version)
        try fields.encodeIfPresent(activeProjectId, forKey: .activeProjectId)
        try fields.encode(projectIds, forKey: .projectIds)
    }
}

private struct ProjectSessionIndex: Codable {
    var version: Int = 2
    var sessions: [ProjectSessionSummary]
}

private struct VersionedBaseline: Codable {
    var version: Int = 1
    var files: [ProjectSyncFile]
}

final class ProjectRepository: @unchecked Sendable {
    let projectsRoot: URL
    private let userHome: URL?
    private let now: @Sendable () -> Date
    private let newId: @Sendable () -> String
    private let writer: ProjectAtomicWriting
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder
    // Once this repository has seen an index, its disappearance cannot be a
    // fresh install. Access is serialized by ProjectStoreExecutionDomain.
    private var observedProjectIndex = false
    private var observedGlobalIndex = false

    init(
        projectsRoot: URL,
        now: @escaping @Sendable () -> Date = { Date() },
        newId: @escaping @Sendable () -> String = { UUID().uuidString.lowercased() },
        writer: ProjectAtomicWriting = DefaultProjectAtomicWriter(),
        userHome: URL? = nil
    ) {
        self.projectsRoot = projectsRoot
        self.userHome = userHome
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

    func load() -> ProjectRepositoryState {
        try? FileManager.default.createDirectory(at: projectsRoot, withIntermediateDirectories: true)
        let indexResult = Result { try readIndex() }
        if case .success = indexResult,
           FileManager.default.fileExists(atPath: projectsRoot.appendingPathComponent(projectIndexFileName).path) {
            observedProjectIndex = true
        }
        let index = try? indexResult.get()
        let fm = FileManager.default
        let directoriesResult = Result { try fm.contentsOfDirectory(
            at: projectsRoot,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        ) }
        var projectDirectories: [URL] = []
        var directoryErrors: [String] = []
        for url in (try? directoriesResult.get()) ?? [] where isLowercaseUUID(url.lastPathComponent) {
            do {
                guard try url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true else {
                    throw ProjectRepositoryError.invalidProjectData
                }
                projectDirectories.append(url)
            } catch {
                directoryErrors.append("project.\(url.lastPathComponent): \(error.localizedDescription)")
            }
        }

        let loaded = projectDirectories.map { url in
            (url, Result { try loadProject(id: url.lastPathComponent) })
        }
        let discovered = loaded.compactMap { try? $0.1.get() }
            .sorted { $0.record.updatedAt > $1.record.updatedAt }
        let corrupt = directoryErrors + loaded.compactMap { url, result -> String? in
            guard case let .failure(error) = result else { return nil }
            return "project.\(url.lastPathComponent): \(error.localizedDescription)"
        }
        let discoveredIds = discovered.map(\.record.id)
        let missingIndexedProjects = index?.projectIds.filter { !discoveredIds.contains($0) } ?? []
        let active = index?.activeProjectId.flatMap { discoveredIds.contains($0) ? $0 : nil }
        let globalURL = projectsRoot.appendingPathComponent(globalSessionIndexFileName)
        let globalResult = Result { try readSessions(at: globalURL, required: observedGlobalIndex) }
        if case .success = globalResult, fm.fileExists(atPath: globalURL.path) { observedGlobalIndex = true }
        let globalSessions = (try? globalResult.get()) ?? []
        let storageError: String? = {
            if case let .failure(error) = indexResult { return "projects-index: \(error.localizedDescription)" }
            if case let .failure(error) = directoriesResult { return "projects-directory: \(error.localizedDescription)" }
            if case let .failure(error) = globalResult { return "global: \(error.localizedDescription)" }
            if !corrupt.isEmpty {
                return "\(String(localized: "project_corrupt_projects_message \(corrupt.count)")) \(corrupt.joined(separator: "; "))"
            }
            if !missingIndexedProjects.isEmpty {
                return "\(missingIndexedProjects.map { "project.\($0)" }.joined(separator: ", ")): \(ProjectRepositoryError.invalidProjectData.localizedDescription)"
            }
            if let conflict = SessionCatalogOwnership.firstScopeConflict(projects: discovered, globalSessions: globalSessions) {
                return "\(conflict.sessionID) (\(conflict.first.workspaceKey), \(conflict.second.workspaceKey)): \(ProjectRepositoryError.conflictingSessionScope.localizedDescription)"
            }
            return nil
        }()
        return ProjectRepositoryState(
            projects: discovered,
            activeProjectId: active,
            globalSessions: globalSessions,
            projectIndexPresent: fm.fileExists(atPath: projectsRoot.appendingPathComponent(projectIndexFileName).path),
            globalIndexPresent: fm.fileExists(atPath: projectsRoot.appendingPathComponent(globalSessionIndexFileName).path),
            loading: false,
            errorMessage: storageError
        )
    }

    private func healthyState() throws -> ProjectRepositoryState {
        let state = load()
        guard state.errorMessage == nil else { throw ProjectRepositoryError.invalidProjectData }
        return state
    }

    /// Admission must inspect current files rather than a possibly stale UI
    /// snapshot. Use an independent reader so a long sync cannot block the
    /// main actor on the store's serialized execution queue.
    func admissionSnapshot() -> ProjectRepositoryState {
        ProjectRepository(projectsRoot: projectsRoot, userHome: userHome).load()
    }

    func createInternal(name: String, mode: SessionMode = .code) throws -> ProjectSnapshot {
        try createProject(name: name, storageKind: .internal, sourceBookmark: nil, mode: mode)
    }

    func createImported(name: String, sourceBookmark: ProjectExternalBookmark, mode: SessionMode = .code) throws -> ProjectSnapshot {
        try createProject(name: name, storageKind: .externalBookmarkMirror, sourceBookmark: sourceBookmark, mode: mode)
    }

    func setActiveProject(_ projectId: String?) throws -> ProjectRepositoryState {
        if let projectId {
            try validateProjectID(projectId)
            guard FileManager.default.fileExists(atPath: projectDirectory(projectId).appendingPathComponent(projectManifestFileName).path)
            else { throw ProjectRepositoryError.projectDoesNotExist(projectId) }
        }
        let current = try healthyState()
        try writeIndex(ProjectIndex(activeProjectId: projectId, projectIds: current.projects.map(\.record.id)))
        return load()
    }

    func updateProject(_ record: ProjectRecord) throws -> ProjectSnapshot {
        _ = try healthyState()
        try validateProjectID(record.id)
        guard FileManager.default.fileExists(atPath: projectDirectory(record.id).path)
        else { throw ProjectRepositoryError.projectDoesNotExist(record.id) }
        var normalized = record
        normalized.updatedAt = now()
        try writeProject(normalized)
        return try loadProject(id: record.id)
    }

    func updateSessions(projectId: String?, sessions: [ProjectSessionSummary]) throws -> ProjectRepositoryState {
        let state = try healthyState()
        let snapshot = try projectId.map { try loadProject(id: $0) }
        let existing = try snapshot?.sessions ?? readSessions(at: projectsRoot.appendingPathComponent(globalSessionIndexFileName), required: false)
        var incomingByID: [String: ProjectSessionSummary] = [:]
        for row in sessions {
            let identity = canonicalSessionID(row.sessionId)
            guard !identity.isEmpty else { throw ProjectRepositoryError.invalidSessionID }
            try checkSessionScope(identity, projectId: projectId, state: state)
            if let previous = incomingByID[identity], previous != row { throw ProjectRepositoryError.invalidSessionID }
            incomingByID[identity] = row
            if let prior = existing.first(where: { canonicalSessionID($0.sessionId) == identity }), prior.mode != row.mode {
                throw ProjectRepositoryError.conflictingSessionMode
            }
        }
        let incoming = sessions
            .filter { !$0.sessionId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            .map { row in
                var row = row
                row.sessionId = canonicalSessionID(row.sessionId)
                let cached = existing.first(where: { $0.sessionId == row.sessionId })
                if let cached, cached.pendingCatalogConfirmation, row.messageCount < cached.messageCount {
                    return cached
                }
                row.workspaceDirectory = row.workspaceDirectory ?? cached?.workspaceDirectory
                row.originalProjectDirectory = row.originalProjectDirectory ?? cached?.originalProjectDirectory
                row.transcriptPath = row.transcriptPath ?? cached?.transcriptPath
                row.isArchived = cached?.isArchived ?? false
                row.pendingCatalogConfirmation = false
                return row
            }
            .uniqued(on: \.sessionId)
            .sorted { $0.updatedAt > $1.updatedAt }
        let incomingIDs = Set(incoming.map(\.sessionId))
        // Keep unacknowledged starts and archive metadata across stale engine lists.
        let normalized = (incoming + existing.filter {
            !incomingIDs.contains($0.sessionId) && ($0.pendingCatalogConfirmation || $0.isArchived ||
                (snapshot != nil && $0.workspaceDirectory != nil && $0.workspaceDirectory != snapshot?.record.activeDirectory))
        }).sorted { $0.updatedAt > $1.updatedAt }
        if let projectId {
            try validateProjectID(projectId)
            try writeSessions(projectId: projectId, sessions: normalized)
            let snapshot = try loadProject(id: projectId)
            var record = snapshot.record
            record.modeMembership.formUnion(incoming.map(\.mode))
            try writeProject(record)
        } else {
            try writeSessionIndex(normalized, to: projectsRoot.appendingPathComponent(globalSessionIndexFileName))
        }
        return load()
    }

    func recordStartedSession(
        projectId: String?,
        sessionId: String,
        title: String,
        mode: SessionMode = .code,
        initialMessageCount: Int = 0,
        workspaceDirectory: String? = nil
    ) throws -> ProjectRepositoryState {
        let state = try healthyState()
        let canonicalID = canonicalSessionID(sessionId)
        guard !canonicalID.isEmpty else { throw ProjectRepositoryError.invalidSessionID }
        try checkSessionScope(canonicalID, projectId: projectId, state: state)
        let snapshot = try projectId.map { try loadProject(id: $0) }
        let sessions = try snapshot?.sessions ?? readSessions(at: projectsRoot.appendingPathComponent(globalSessionIndexFileName), required: false)
        let timestamp = now()
        let existing = sessions.first(where: { $0.sessionId == canonicalID })
        guard existing == nil || existing?.mode == mode else { throw ProjectRepositoryError.conflictingSessionMode }
        let started = ProjectSessionSummary(
            sessionId: canonicalID,
            title: title.isEmpty ? (existing?.title ?? String(localized: "chat_new_conversation")) : title,
            messageCount: max(existing?.messageCount ?? 0, initialMessageCount),
            relativeTime: String(localized: "project_relative_time_just_now"),
            updatedAt: timestamp,
            mode: existing?.mode ?? mode,
            isArchived: existing?.isArchived ?? false,
            pendingCatalogConfirmation: (existing?.pendingCatalogConfirmation ?? true) ||
                initialMessageCount > (existing?.messageCount ?? 0),
            workspaceDirectory: workspaceDirectory ?? existing?.workspaceDirectory ?? snapshot?.record.activeDirectory,
            originalProjectDirectory: existing?.originalProjectDirectory ?? snapshot?.record.activeDirectory,
            transcriptPath: existing?.transcriptPath
        )
        let updated = ([started] + sessions.filter { $0.sessionId != canonicalID })
        if let projectId {
            try writeSessions(projectId: projectId, sessions: updated)
            if let snapshot {
                try writeProject(snapshot.record.with(updatedAt: timestamp))
            }
        } else {
            try writeSessionIndex(updated, to: projectsRoot.appendingPathComponent(globalSessionIndexFileName))
        }
        return load()
    }

    private func checkSessionScope(_ sessionID: String, projectId: String?, state: ProjectRepositoryState) throws {
        let requested: ConversationScope = projectId.map { .project($0) } ?? .global
        guard SessionCatalogOwnership.allows(sessionID: sessionID, scope: requested, mode: nil,
                                              projects: state.projects, globalSessions: state.globalSessions,
                                              claimedScope: nil) else {
            throw ProjectRepositoryError.conflictingSessionScope
        }
    }

    func setSessionArchived(projectId: String?, sessionId: String, archived: Bool) throws -> ProjectRepositoryState {
        _ = try healthyState()
        let canonicalID = canonicalSessionID(sessionId)
        let sessions = try projectId.map { try loadProject(id: $0).sessions }
            ?? readSessions(at: projectsRoot.appendingPathComponent(globalSessionIndexFileName), required: false)
        guard sessions.contains(where: { $0.sessionId == canonicalID }) else {
            throw ProjectRepositoryError.sessionNotIndexed
        }
        let updated = sessions.map { row in
            var row = row
            if row.sessionId == canonicalID {
                row.pendingCatalogConfirmation = row.pendingCatalogConfirmation || (row.isArchived && !archived)
                row.isArchived = archived
            }
            return row
        }
        if let projectId {
            try writeSessions(projectId: projectId, sessions: updated)
        } else {
            try writeSessionIndex(updated, to: projectsRoot.appendingPathComponent(globalSessionIndexFileName))
        }
        return load()
    }

    func deleteProject(projectId: String) throws -> ProjectRepositoryState {
        let current = try healthyState()
        try validateProjectID(projectId)
        let directory = projectDirectory(projectId)
        guard FileManager.default.fileExists(atPath: directory.path) else {
            throw ProjectRepositoryError.projectDoesNotExist(projectId)
        }
        try FileManager.default.removeItem(at: directory)
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

    func recordWorkspace(projectId: String, state: SessionWorkspaceState) throws -> ProjectRepositoryState {
        _ = try healthyState()
        let snapshot = try loadProject(id: projectId)
        _ = try workspaceForDirectory(projectId: projectId, guestPath: state.cwd)
        for directory in state.additionalDirectories { _ = try workspaceForDirectory(projectId: projectId, guestPath: directory, requireExisting: false) }
        var record = snapshot.record
        record.activeDirectory = state.cwd
        record.additionalDirectories = (record.additionalDirectories + state.additionalDirectories + [state.cwd])
            .filter { $0 != record.directory }.uniqued()
        try writeProject(record)
        try writeSessions(projectId: projectId, sessions: snapshot.sessions.map { row in
            guard row.sessionId == state.sessionId else { return row }
            var row = row
            row.workspaceDirectory = state.cwd
            row.originalProjectDirectory = state.projectRoot
            row.transcriptPath = state.transcriptPath
            return row
        })
        return load()
    }

    private func workspaceForDirectory(projectId: String, guestPath: String, requireExisting: Bool = true) throws -> ProjectWorkspace {
        let projectsGuest = LXISHGuestPaths.workspace("projects")
        let homeGuest = LXISHGuestPaths.home
        guard (guestPath == homeGuest || guestPath.hasPrefix(homeGuest + "/")),
              !guestPath.split(separator: "/").contains(where: { $0 == ".." || $0 == "." })
        else { throw ProjectRepositoryError.escapedWorkspace }
        let root: URL
        let relative: String
        if guestPath == projectsGuest || guestPath.hasPrefix(projectsGuest + "/") {
            root = projectsRoot
            relative = String(guestPath.dropFirst(projectsGuest.count)).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        } else {
            guard let userHome else { throw ProjectRepositoryError.escapedWorkspace }
            root = userHome
            relative = String(guestPath.dropFirst(homeGuest.count)).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        }
        let canonicalRoot = try requireCanonicalDirectory(root)
        let requested = root.appendingPathComponent(relative, isDirectory: true)
        let directory = requireExisting ? try requireCanonicalDirectory(requested) : requested.resolvingSymlinksInPath().standardizedFileURL
        guard directory == canonicalRoot || directory.path.hasPrefix(canonicalRoot.path + "/") else { throw ProjectRepositoryError.escapedWorkspace }
        return ProjectWorkspace(projectId: projectId, hostURL: directory, guestPath: guestPath)
    }

    func project(projectId: String) throws -> ProjectSnapshot {
        try loadProject(id: projectId)
    }

    private func createProject(
        name: String,
        storageKind: ProjectStorageKind,
        sourceBookmark: ProjectExternalBookmark?,
        mode: SessionMode
    ) throws -> ProjectSnapshot {
        _ = try healthyState()
        let index = try readIndex()
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
            sourceBookmark: sourceBookmark,
            modeMembership: [mode]
        )
        do {
            try FileManager.default.createDirectory(at: workspaceURL, withIntermediateDirectories: true)
            try writeProject(record)
            try writeSessions(projectId: id, sessions: [])
            try writeBaseline(projectId: id, baseline: ProjectSyncBaseline())
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
        if (try? workspaceURL.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink) == true {
            throw ProjectRepositoryError.escapedWorkspace
        }
        let canonicalWorkspace = workspaceURL.resolvingSymlinksInPath().standardizedFileURL
        guard canonicalWorkspace.deletingLastPathComponent() == canonicalDir else {
            throw ProjectRepositoryError.escapedWorkspace
        }
        return ProjectSnapshot(
            record: record,
            workspace: try workspaceForDirectory(projectId: projectId, guestPath: record.activeDirectory, requireExisting: false),
            sourceWorkspace: ProjectWorkspace(projectId: projectId, hostURL: canonicalWorkspace),
            sessions: try readSessions(at: directory.appendingPathComponent(projectSessionIndexFileName), required: true)
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
        if FileManager.default.fileExists(atPath: url.path) {
            _ = try readSessions(at: url, required: true)
        }
        try write(ProjectSessionIndex(sessions: sessions), to: url)
        if url == projectsRoot.appendingPathComponent(globalSessionIndexFileName) { observedGlobalIndex = true }
    }

    private func readSessions(at url: URL, required: Bool) throws -> [ProjectSessionSummary] {
        guard FileManager.default.fileExists(atPath: url.path) else {
            if required { throw ProjectRepositoryError.missingSessionIndex }
            return []
        }
        let decoded = try decode(ProjectSessionIndex.self, from: url)
        guard decoded.version == 2 else { throw ProjectRepositoryError.unsupportedVersion }
        var identities = Set<String>()
        return try decoded.sessions.map {
            var row = $0
            row.sessionId = canonicalSessionID(row.sessionId)
            guard !row.sessionId.isEmpty, identities.insert(row.sessionId).inserted else {
                throw ProjectRepositoryError.invalidSessionID
            }
            return row
        }
    }

    private func readIndex() throws -> ProjectIndex {
        let url = projectsRoot.appendingPathComponent(projectIndexFileName)
        guard FileManager.default.fileExists(atPath: url.path) else {
            let children = try FileManager.default.contentsOfDirectory(atPath: projectsRoot.path)
            guard !observedProjectIndex, !children.contains(where: isLowercaseUUID) else { throw ProjectRepositoryError.missingProjectIndex }
            return ProjectIndex()
        }
        let decoded = try decode(ProjectIndex.self, from: url)
        guard decoded.version == 1 else { throw ProjectRepositoryError.unsupportedVersion }
        guard decoded.projectIds.allSatisfy(isLowercaseUUID),
              Set(decoded.projectIds).count == decoded.projectIds.count else { throw ProjectRepositoryError.invalidProjectData }
        if let active = decoded.activeProjectId,
           !isLowercaseUUID(active) || !decoded.projectIds.contains(active) { throw ProjectRepositoryError.invalidProjectData }
        return decoded
    }

    private func writeIndex(_ index: ProjectIndex) throws {
        try write(index, to: projectsRoot.appendingPathComponent(projectIndexFileName))
        observedProjectIndex = true
    }

    private func write<T: Codable>(_ value: T, to url: URL) throws {
        try writer.write(value, to: url, encoder: encoder, decoder: decoder)
    }

    private func decode<T: Decodable>(_ type: T.Type, from url: URL) throws -> T {
        let data = try Data(contentsOf: url)
        try ProjectJSONKeyGuard.validate(data)
        return try decoder.decode(type, from: data)
    }
}

/// Foundation's keyed decoder accepts duplicate JSON names. Reject them before
/// decoding so corrupt indexes cannot be interpreted as a different owner.
private struct ProjectJSONKeyGuard {
    private let bytes: [UInt8]
    private var offset = 0

    static func validate(_ data: Data) throws {
        _ = try JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed])
        var parser = Self(bytes: Array(data))
        try parser.value(depth: 0)
        parser.skipWhitespace()
        guard parser.offset == parser.bytes.count else { throw ProjectRepositoryError.invalidProjectData }
    }

    private mutating func value(depth: Int) throws {
        guard depth <= 128 else { throw ProjectRepositoryError.invalidProjectData }
        skipWhitespace()
        guard offset < bytes.count else { throw ProjectRepositoryError.invalidProjectData }
        switch bytes[offset] {
        case 123:
            offset += 1
            skipWhitespace()
            if take(125) { return }
            var keys = Set<String>()
            repeat {
                skipWhitespace()
                let key = try string(decode: true)
                guard keys.insert(key).inserted else { throw ProjectRepositoryError.invalidProjectData }
                skipWhitespace()
                guard take(58) else { throw ProjectRepositoryError.invalidProjectData }
                try value(depth: depth + 1)
                skipWhitespace()
                if take(125) { return }
                guard take(44) else { throw ProjectRepositoryError.invalidProjectData }
            } while true
        case 91:
            offset += 1
            skipWhitespace()
            if take(93) { return }
            repeat {
                try value(depth: depth + 1)
                skipWhitespace()
                if take(93) { return }
                guard take(44) else { throw ProjectRepositoryError.invalidProjectData }
            } while true
        case 34:
            _ = try string(decode: false)
        default:
            while offset < bytes.count, ![9, 10, 13, 32, 44, 93, 125].contains(bytes[offset]) { offset += 1 }
        }
    }

    private mutating func string(decode: Bool) throws -> String {
        let start = offset
        guard take(34) else { throw ProjectRepositoryError.invalidProjectData }
        while offset < bytes.count {
            let byte = bytes[offset]
            offset += 1
            if byte == 92 {
                guard offset < bytes.count else { throw ProjectRepositoryError.invalidProjectData }
                offset += 1
            } else if byte == 34 {
                guard decode else { return "" }
                let wrapped = Data([91] + bytes[start..<offset] + [93])
                guard let values = try JSONSerialization.jsonObject(with: wrapped) as? [String],
                      let value = values.first else { throw ProjectRepositoryError.invalidProjectData }
                return value
            }
        }
        throw ProjectRepositoryError.invalidProjectData
    }

    private mutating func skipWhitespace() {
        while offset < bytes.count, [9, 10, 13, 32].contains(bytes[offset]) { offset += 1 }
    }

    private mutating func take(_ byte: UInt8) -> Bool {
        guard offset < bytes.count, bytes[offset] == byte else { return false }
        offset += 1
        return true
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
    case invalidProjectData
    case missingProjectIndex
    case missingSessionIndex
    case conflictingSessionMode
    case conflictingSessionScope
    case invalidSessionOwnership(String)

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
        case .invalidProjectData: return "Project data is invalid; the original files were preserved."
        case .missingProjectIndex: return "The project index is missing; existing projects were preserved."
        case .missingSessionIndex: return "The project session index is missing; existing files were preserved."
        case .conflictingSessionMode: return "The session already belongs to a different mode."
        case .conflictingSessionScope: return "The session belongs to conflicting conversation spaces; the original files were preserved."
        case .invalidSessionOwnership(let message): return message
        }
    }
}

/// Catalog rows and persisted claims are equal evidence. A UUID with multiple
/// owners is ambiguous regardless of which file or claim was read first.
enum SessionCatalogOwnership {
    static func scopes(sessionID: String, projects: [ProjectSnapshot],
                       globalSessions: [ProjectSessionSummary]) -> Set<ConversationScope> {
        let id = canonicalSessionID(sessionID)
        var result = Set<ConversationScope>()
        if globalSessions.contains(where: { canonicalSessionID($0.sessionId) == id }) { result.insert(.global) }
        for project in projects where project.sessions.contains(where: { canonicalSessionID($0.sessionId) == id }) {
            result.insert(.project(project.id))
        }
        return result
    }

    static func allows(sessionID: String, scope: ConversationScope, mode: SessionMode?,
                       projects: [ProjectSnapshot], globalSessions: [ProjectSessionSummary],
                       claimedScope: ConversationScope?) -> Bool {
        let id = canonicalSessionID(sessionID)
        guard !id.isEmpty else { return false }
        var owners = scopes(sessionID: id, projects: projects, globalSessions: globalSessions)
        if let claimedScope { owners.insert(claimedScope) }
        guard owners.isEmpty || owners == [scope] else { return false }
        if let mode {
            let rows = globalSessions + projects.flatMap(\.sessions)
            guard rows.filter({ canonicalSessionID($0.sessionId) == id }).allSatisfy({ $0.mode == mode }) else { return false }
        }
        return true
    }

    static func firstScopeConflict(projects: [ProjectSnapshot], globalSessions: [ProjectSessionSummary])
        -> (sessionID: String, first: ConversationScope, second: ConversationScope)? {
        var owners: [String: ConversationScope] = [:]
        for row in globalSessions { owners[canonicalSessionID(row.sessionId)] = .global }
        for project in projects {
            for row in project.sessions {
                let id = canonicalSessionID(row.sessionId)
                let scope = ConversationScope.project(project.id)
                if let first = owners[id], first != scope { return (id, first, scope) }
                owners[id] = scope
            }
        }
        return nil
    }

    static func firstConflictingClaim(projects: [ProjectSnapshot], globalSessions: [ProjectSessionSummary],
                                      claims: [String: ConversationScope])
        -> (sessionID: String, indexed: ConversationScope, claimed: ConversationScope)? {
        for row in globalSessions {
            let id = canonicalSessionID(row.sessionId)
            if let claimed = claims[id], claimed != .global { return (id, .global, claimed) }
        }
        for project in projects {
            for row in project.sessions {
                let id = canonicalSessionID(row.sessionId)
                let scope = ConversationScope.project(project.id)
                if let claimed = claims[id], claimed != scope { return (id, scope, claimed) }
            }
        }
        return nil
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
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    let candidate = trimmed.hasPrefix("sess:") ? String(trimmed.dropFirst("sess:".count)) : trimmed
    if let uuid = UUID(uuidString: candidate) { return uuid.uuidString.lowercased() }
    return candidate
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
        syncState: ProjectSyncState? = nil
    ) -> ProjectRecord {
        var copy = self
        if let updatedAt { copy.updatedAt = updatedAt }
        if let syncState { copy.syncState = syncState }
        return copy
    }
}
