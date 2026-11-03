import Foundation
import Darwin
import CryptoKit

let projectIndexFileName = "index.json"
let projectManifestFileName = "project.json"
let projectSessionIndexFileName = "session-spaces.json"
let projectSyncBaselineFileName = "sync-baseline.json"
let projectWorkspaceDirectoryName = "workspace"
let globalSessionIndexFileName = "global-session-spaces.json"
private let localAppIntentPrefix = ".local-app-intent-"
private let localAppSelectionFileName = ".local-app-selection.json"
private let localAppCreatePrefix = ".local-app-create-"
private let localAppImportsDirectoryName = "local-app-imports"

protocol ProjectAtomicWriting: Sendable {
    func write<T: Codable>(_ value: T, to url: URL, encoder: JSONEncoder, decoder: JSONDecoder) throws
    func writeData(_ data: Data, to url: URL, validate: (Data) throws -> Void) throws
    func writeData(_ data: Data, to url: URL, expected: OwnerCatalogExpectation, validate: (Data) throws -> Void,
                   commit: (_ replace: () throws -> Void) throws -> Void) throws
}

struct DefaultProjectAtomicWriter: ProjectAtomicWriting {
    private let onGuardedStaged: (@Sendable (URL) -> Void)?
    private let onGuardedVerified: (@Sendable () -> Void)?

    init(onGuardedStaged: (@Sendable (URL) -> Void)? = nil, onGuardedVerified: (@Sendable () -> Void)? = nil) {
        self.onGuardedStaged = onGuardedStaged
        self.onGuardedVerified = onGuardedVerified
    }

    func write<T: Codable>(_ value: T, to url: URL, encoder: JSONEncoder, decoder: JSONDecoder) throws {
        let data = try encoder.encode(value)
        try writeData(data, to: url) { try _ = decoder.decode(T.self, from: $0) }
    }

    func writeData(_ data: Data, to url: URL, validate: (Data) throws -> Void) throws {
        try stageData(data, to: url, expected: ownerCatalogExpectation(url), validate: validate) { try $0() }
    }

    func writeData(_ data: Data, to url: URL, expected: OwnerCatalogExpectation, validate: (Data) throws -> Void,
                   commit: (_ replace: () throws -> Void) throws -> Void) throws {
        try stageData(data, to: url, expected: expected, validate: validate, commit: commit)
    }

    private func stageData(_ data: Data, to url: URL, expected: OwnerCatalogExpectation,
                           validate: (Data) throws -> Void,
                           commit: (_ replace: () throws -> Void) throws -> Void) throws {
        let fm = FileManager.default
        try fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        guard try ownerCatalogExpectation(url) == expected else { throw ProjectRepositoryError.invalidProjectData }
        let tempURL = url.deletingLastPathComponent()
            .appendingPathComponent(".\(url.lastPathComponent).\(UUID().uuidString).tmp")
        do {
            try data.write(to: tempURL, options: .atomic)
            let staged = try ownerCatalogSnapshot(tempURL)
            guard staged.bytes == data else { throw ProjectRepositoryError.invalidProjectData }
            try validate(staged.bytes)
            onGuardedStaged?(tempURL)
            guard try ownerCatalogExpectation(url) == expected,
                  try ownerCatalogSnapshot(tempURL) == staged else { throw ProjectRepositoryError.invalidProjectData }
            onGuardedVerified?()
            // Both paths are in the same directory. POSIX rename atomically
            // replaces an existing file without Foundation's LaunchServices
            // XPC, which can block while another task owns the history lock.
            try commit {
                // Recheck both names at the publication boundary. POSIX has no
                // inode-CAS here, so a non-cooperating process can still race
                // the final check and rename; we do not claim otherwise.
                guard try ownerCatalogExpectationMatchesVersion(url, expected),
                      try ownerCatalogVersion(tempURL) == staged.version else {
                    throw ProjectRepositoryError.invalidProjectData
                }
                let result: Int32 = tempURL.withUnsafeFileSystemRepresentation { source in
                    url.withUnsafeFileSystemRepresentation { destination in
                        guard let source, let destination else { return Int32(-1) }
                        return Darwin.rename(source, destination)
                    }
                }
                guard result == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            }
            // A conditional commit may intentionally leave the staged file
            // unused when its owner is no longer selected.
            try? fm.removeItem(at: tempURL)
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

    /// Persist before calling createAuthoring. The request is a recovery hint,
    /// never authority to mount a source or admit a conversation turn.
    func storeLocalAppSourceCreateIntent(_ intent: LocalAppSourceCreateIntent) throws {
        try validateLocalAppSourceCreateIntent(intent)
        let url = projectsRoot.appendingPathComponent(localAppCreatePrefix + intent.requestId + ".json")
        let data = try encoder.encode(intent)
        guard data.count <= 65_536 else { throw ProjectRepositoryError.invalidProjectData }
        if try ownerCatalogFileExists(url) {
            let existing = try boundedAuthoringIntentSnapshot(url)
            guard try decodeLocalAppSourceCreateIntent(existing.bytes) == intent else {
                throw ProjectRepositoryError.invalidProjectData
            }
            try synchronizeOwnerCatalog(url, expectedBytes: existing.bytes)
            return
        }
        try writer.writeData(data, to: url, expected: .absent, validate: { staged in
            guard try self.decodeLocalAppSourceCreateIntent(staged) == intent else {
                throw ProjectRepositoryError.invalidProjectData
            }
        }, commit: { try $0() })
        try synchronizeOwnerCatalog(url, expectedBytes: data)
    }

    func pendingLocalAppSourceCreations() throws -> [LocalAppSourceCreateIntent] {
        let names = try FileManager.default.contentsOfDirectory(atPath: projectsRoot.path)
            .filter { $0.hasPrefix(localAppCreatePrefix) }.sorted()
        guard names.count <= 4096 else { throw ProjectRepositoryError.invalidProjectData }
        return try names.compactMap { name in
            guard name.hasSuffix(".json") else { throw ProjectRepositoryError.invalidProjectData }
            let identifier = String(name.dropFirst(localAppCreatePrefix.count).dropLast(5))
            guard isLowercaseUUID(identifier) else { throw ProjectRepositoryError.invalidProjectData }
            let intent = try decodeLocalAppSourceCreateIntent(
                boundedAuthoringIntentSnapshot(projectsRoot.appendingPathComponent(name)).bytes)
            guard intent.requestId == identifier else { throw ProjectRepositoryError.invalidProjectData }
            return intent.completed ? nil : intent
        }
    }

    /// Completion hides the recovery offer only after the native owner receipt
    /// has been checked. It does not provide authorization itself.
    func completeLocalAppSourceCreateIntent(requestID: String, owner: LocalAppAuthoringIntent) throws {
        guard let current = try localAppSourceCreateIntent(requestID: requestID),
              owner.intentId == requestID, owner.projectId == current.projectId,
              owner.sourceRequestId == requestID,
              (current.operation == .marketRemix ? (owner.appId?.isEmpty == false) : owner.appId == current.appId),
              owner.authoringSessionId != nil, owner.transcriptPath != nil else {
            throw ProjectRepositoryError.invalidProjectData
        }
        if current.operation == .bind {
            guard current.sourceId == owner.sourceId, current.sourceDirectory == owner.sourceDirectory else {
                throw ProjectRepositoryError.invalidProjectData
            }
        }
        let matching = try localAppAuthoringIntents(projectId: owner.projectId).filter { $0.intentId == requestID }
        guard matching.count == 1, matching[0] == owner else { throw ProjectRepositoryError.invalidProjectData }
        if current.completed { return }
        let url = projectsRoot.appendingPathComponent(localAppCreatePrefix + requestID + ".json")
        let snapshot = try boundedAuthoringIntentSnapshot(url)
        guard try decodeLocalAppSourceCreateIntent(snapshot.bytes) == current else { throw ProjectRepositoryError.invalidProjectData }
        var updated = current
        updated.completed = true
        let data = try encoder.encode(updated)
        try writer.writeData(data, to: url, expected: .present(snapshot), validate: { staged in
            guard try self.decodeLocalAppSourceCreateIntent(staged) == updated else {
                throw ProjectRepositoryError.invalidProjectData
            }
        }, commit: { try $0() })
        try synchronizeOwnerCatalog(url, expectedBytes: data)
    }

    func localAppSourceCreateIntent(requestID: String) throws -> LocalAppSourceCreateIntent? {
        guard isLowercaseUUID(requestID) else { throw ProjectRepositoryError.invalidProjectData }
        let url = projectsRoot.appendingPathComponent(localAppCreatePrefix + requestID + ".json")
        guard try ownerCatalogFileExists(url) else { return nil }
        let intent = try decodeLocalAppSourceCreateIntent(boundedAuthoringIntentSnapshot(url).bytes)
        guard intent.requestId == requestID else { throw ProjectRepositoryError.invalidProjectData }
        return intent
    }

    private func decodeLocalAppSourceCreateIntent(_ data: Data) throws -> LocalAppSourceCreateIntent {
        try UniqueJSONKeyGuard.validate(data)
        guard let fields = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw ProjectRepositoryError.invalidProjectData
        }
        let required: Set<String> = ["version", "requestId", "projectId", "mode", "name", "prompt", "operation", "completed"]
        let allowed = required.union(["appId", "revision", "sourceId", "sourceDirectory", "remixInputDigest"])
        guard required.isSubset(of: Set(fields.keys)), Set(fields.keys).isSubset(of: allowed) else {
            throw ProjectRepositoryError.invalidProjectData
        }
        let intent = try decoder.decode(LocalAppSourceCreateIntent.self, from: data)
        try validateLocalAppSourceCreateIntent(intent)
        return intent
    }

    private func validateLocalAppSourceCreateIntent(_ intent: LocalAppSourceCreateIntent) throws {
        let name = intent.name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard intent.version == 1, intent.mode == .code, isLowercaseUUID(intent.requestId), isLowercaseUUID(intent.projectId),
              name == intent.name, !name.isEmpty, name.count <= 120,
              intent.prompt.utf8.count <= 48_000 else { throw ProjectRepositoryError.invalidProjectData }
        switch intent.operation {
        case .create:
            guard name.unicodeScalars.count <= 80,
                  intent.appId == nil, intent.revision == nil, intent.sourceId == nil, intent.sourceDirectory == nil,
                  intent.remixInputDigest == nil else {
                throw ProjectRepositoryError.invalidProjectData
            }
        case .restore:
            guard let appID = intent.appId, !appID.isEmpty, appID.utf8.count <= 256,
                  let revision = intent.revision, revision > 0, intent.sourceId == nil, intent.sourceDirectory == nil,
                  intent.remixInputDigest == nil else {
                throw ProjectRepositoryError.invalidProjectData
            }
        case .bind:
            let prefix = MobileUserStorage.directories().appsGuestPath + "/"
            guard let appID = intent.appId, !appID.isEmpty, appID.utf8.count <= 256,
                  let sourceID = intent.sourceId, isLowercaseUUID(sourceID),
                  let directory = intent.sourceDirectory, directory.hasPrefix(prefix),
                  !directory.dropFirst(prefix.count).isEmpty,
                  !directory.dropFirst(prefix.count).contains("/"),
                  !directory.dropFirst(prefix.count).contains("\\"),
                  directory.dropFirst(prefix.count) != ".", directory.dropFirst(prefix.count) != "..",
                  intent.revision == nil, intent.remixInputDigest == nil else { throw ProjectRepositoryError.invalidProjectData }
        case .marketRemix:
            guard intent.appId == nil, intent.revision == nil, intent.sourceId == nil, intent.sourceDirectory == nil,
                  let digest = intent.remixInputDigest, Self.isSHA256(digest) else {
                throw ProjectRepositoryError.invalidProjectData
            }
        }
    }

    private static func isSHA256(_ value: String) -> Bool {
        value.utf8.count == 64 && value.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }

    private static func digest(_ bytes: Data) -> String {
        SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
    }

    private static func encodeRemixInput(_ input: LocalAppMarketRemixInput) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return try encoder.encode(input)
    }

    static func localAppRemixInputDigest(_ input: LocalAppMarketRemixInput) throws -> String {
        digest(try encodeRemixInput(input))
    }

    private func validateRemixInput(_ input: LocalAppMarketRemixInput, intent: LocalAppSourceCreateIntent) throws {
        let encoder = JSONEncoder()
        guard try decoder.decode(UnifiedAppManifest.self, from: encoder.encode(input.signedManifest)) == input.manifest,
              try decoder.decode([LocalAppMarketRemixInput.File].self, from: encoder.encode(input.signedFiles)) == input.files,
              try decoder.decode([LocalAppMarketRemixInput.File].self, from: encoder.encode(input.signedDependencies)) == input.dependencies else {
            throw ProjectRepositoryError.invalidProjectData
        }
        guard intent.operation == .marketRemix, input.requestId == intent.requestId,
              (input.archivePath as NSString).isAbsolutePath,
              URL(fileURLWithPath: input.archivePath).standardizedFileURL.path == input.archivePath,
              URL(fileURLWithPath: input.archivePath).lastPathComponent == "source.zip",
              URL(fileURLWithPath: input.archivePath).deletingLastPathComponent().lastPathComponent == "market-" + intent.requestId,
              URL(fileURLWithPath: input.archivePath).deletingLastPathComponent().deletingLastPathComponent().lastPathComponent == localAppImportsDirectoryName,
              input.archiveBytes > 0, input.archiveBytes <= 25 * 1024 * 1024,
              Self.isSHA256(input.archiveSha256), input.manifest.remix?.allowed == true,
              input.manifest.name.trimmingCharacters(in: .whitespacesAndNewlines) == intent.name,
              !input.marketOrigin.appId.isEmpty, !input.marketOrigin.releaseId.isEmpty,
              input.marketOrigin.remixedFrom == input.marketOrigin.appId,
              input.files.count > 0, input.files.count <= 2_000,
              Set(input.files.map(\.path)).count == input.files.count,
              input.files.contains(where: { $0.path == "lingxi-app.json" }),
              input.files.allSatisfy({ !$0.path.isEmpty && $0.path.utf16.count <= 512 &&
                  $0.sizeBytes >= 0 && $0.sizeBytes <= 25 * 1024 * 1024 && Self.isSHA256($0.sha256) }),
              input.dependencies.allSatisfy({ input.files.contains($0) }) else {
            throw ProjectRepositoryError.invalidProjectData
        }
    }

    /// Publish the exact release description before the discoverable request;
    /// an interrupted write can leave an inert orphan but never a false proof.
    func storeLocalAppRemixRequest(_ intent: LocalAppSourceCreateIntent, input: LocalAppMarketRemixInput,
                                   configHome: URL) throws {
        try validateLocalAppSourceCreateIntent(intent)
        try validateRemixInput(input, intent: intent)
        let bytes = try Self.encodeRemixInput(input)
        guard bytes.count <= 8 * 1024 * 1024, Self.digest(bytes) == intent.remixInputDigest else {
            throw ProjectRepositoryError.invalidProjectData
        }
        let directory = try remixStagingDirectory(configHome, requestID: intent.requestId)
        guard input.archivePath == directory.appendingPathComponent("source.zip").path else {
            throw ProjectRepositoryError.invalidProjectData
        }
        let url = directory.appendingPathComponent("input.json")
        if try ownerCatalogFileExists(url) {
            let existing = try boundedOwnerCatalogSnapshot(url, maximumSize: 8 * 1024 * 1024)
            guard existing.bytes == bytes else { throw ProjectRepositoryError.invalidProjectData }
        } else {
            try writer.writeData(bytes, to: url, expected: .absent, validate: { staged in
                guard staged == bytes else { throw ProjectRepositoryError.invalidProjectData }
            }, commit: { try $0() })
        }
        try synchronizeOwnerCatalog(url, expectedBytes: bytes)
        try storeLocalAppSourceCreateIntent(intent)
    }

    /// Loading a completed SDK request must not require its ZIP still to be
    /// present: the native handler checks the durable request receipt first.
    func localAppRemixInput(_ intent: LocalAppSourceCreateIntent, configHome: URL) throws -> (LocalAppMarketRemixInput, URL) {
        try validateLocalAppSourceCreateIntent(intent)
        guard try localAppSourceCreateIntent(requestID: intent.requestId) == intent else {
            throw ProjectRepositoryError.invalidProjectData
        }
        let directory = try remixStagingDirectory(configHome, requestID: intent.requestId)
        let metadata = try boundedOwnerCatalogSnapshot(directory.appendingPathComponent("input.json"),
                                                       maximumSize: 8 * 1024 * 1024).bytes
        guard Self.digest(metadata) == intent.remixInputDigest else { throw ProjectRepositoryError.invalidProjectData }
        try UniqueJSONKeyGuard.validate(metadata)
        let input = try decoder.decode(LocalAppMarketRemixInput.self, from: metadata)
        try validateRemixInput(input, intent: intent)
        return (input, URL(fileURLWithPath: input.archivePath))
    }

    func storeLocalAppRemixArchive(_ bytes: Data, intent: LocalAppSourceCreateIntent, configHome: URL) throws -> URL {
        let (input, url) = try localAppRemixInput(intent, configHome: configHome)
        let currentDirectory = try remixStagingDirectory(configHome, requestID: intent.requestId)
        guard url.path == currentDirectory.appendingPathComponent("source.zip").path,
              bytes.count == input.archiveBytes, Self.digest(bytes) == input.archiveSha256 else {
            throw ProjectRepositoryError.invalidProjectData
        }
        if try ownerCatalogFileExists(url) {
            let existing = try boundedOwnerCatalogSnapshot(url, maximumSize: 25 * 1024 * 1024)
            guard existing.bytes == bytes else { throw ProjectRepositoryError.invalidProjectData }
        } else {
            try writer.writeData(bytes, to: url, expected: .absent, validate: { staged in
                guard staged == bytes else { throw ProjectRepositoryError.invalidProjectData }
            }, commit: { try $0() })
        }
        try synchronizeOwnerCatalog(url, expectedBytes: bytes)
        return url
    }

    private func remixStagingDirectory(_ configHome: URL, requestID: String) throws -> URL {
        guard isLowercaseUUID(requestID) else { throw ProjectRepositoryError.invalidProjectData }
        let root = configHome.standardizedFileURL
        let descriptor = root.path.withCString { open($0, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC) }
        guard descriptor >= 0 else { throw ProjectRepositoryError.invalidProjectData }
        defer { close(descriptor) }
        func openChild(_ parent: Int32, name: String) throws -> Int32 {
            let made = name.withCString { mkdirat(parent, $0, 0o700) }
            guard made == 0 || errno == EEXIST else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            if made == 0, fsync(parent) != 0 { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            let child = name.withCString { openat(parent, $0, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC) }
            guard child >= 0 else { throw ProjectRepositoryError.invalidProjectData }
            var information = stat()
            guard fstat(child, &information) == 0, information.st_mode & 0o077 == 0 else {
                close(child)
                throw ProjectRepositoryError.invalidProjectData
            }
            return child
        }
        let imports = try openChild(descriptor, name: localAppImportsDirectoryName)
        defer { close(imports) }
        let requestName = "market-" + requestID
        let request = try openChild(imports, name: requestName)
        defer { close(request) }
        return root.appendingPathComponent(localAppImportsDirectoryName, isDirectory: true)
            .appendingPathComponent(requestName, isDirectory: true)
    }

    /// Publish a complete recoverable project in one directory rename. A
    /// crash before the rename leaves the durable create request available;
    /// a crash afterwards exposes both the project and its selected intent.
    func createLocalAppAuthoringProject(name: String, sourceDirectory: String, sourceID: String, sourceRequestID: String,
                                        appID: String?, projectID: String, intentID: String)
        throws -> (ProjectSnapshot, LocalAppAuthoringIntent) {
        try validateProjectID(projectID)
        guard isLowercaseUUID(intentID), isLowercaseUUID(sourceID), isLowercaseUUID(sourceRequestID) else {
            throw ProjectRepositoryError.invalidProjectData
        }
        let cleanName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanName.isEmpty, cleanName.count <= 120 else { throw ProjectRepositoryError.blankProjectName }
        let intent = LocalAppAuthoringIntent(intentId: intentID, sourceId: sourceID, sourceRequestId: sourceRequestID,
            sourceDirectory: sourceDirectory, projectId: projectID, appId: appID)
        let directory = projectDirectory(projectID)
        if try ownerCatalogFileExists(directory) {
            let project = try loadProject(id: projectID)
            let matching = try localAppAuthoringIntents(projectId: projectID).filter { $0.intentId == intentID }
            guard matching.count == 1, matching[0].sourceId == sourceID, matching[0].sourceRequestId == sourceRequestID,
                  matching[0].sourceDirectory == sourceDirectory, matching[0].appId == appID else {
                throw ProjectRepositoryError.invalidProjectData
            }
            return (project, matching[0])
        }

        let indexURL = projectsRoot.appendingPathComponent(projectIndexFileName)
        var indexExpected = try ownerCatalogExpectation(indexURL)
        _ = try healthyState()
        let index = try readIndex()
        // Ensure an existing, valid index before exposing a directory. The
        // repository deliberately treats a missing index beside a UUID as
        // corruption, including after a crash during project creation.
        if case .absent = indexExpected {
            try writeIndex(index, expected: .absent)
            let data = try ownerCatalogSnapshot(indexURL).bytes
            try synchronizeOwnerCatalog(indexURL, expectedBytes: data)
            indexExpected = try ownerCatalogExpectation(indexURL)
        }
        let staging = projectsRoot.appendingPathComponent(".local-app-staging-" + UUID().uuidString.lowercased(), isDirectory: true)
        let workspaceURL = staging.appendingPathComponent(projectWorkspaceDirectoryName, isDirectory: true)
        var published = false
        do {
            try FileManager.default.createDirectory(at: workspaceURL, withIntermediateDirectories: true)
            let timestamp = now()
            let record = ProjectRecord(id: projectID, name: cleanName, storageKind: .internal,
                createdAt: timestamp, updatedAt: timestamp, activeDirectory: sourceDirectory,
                additionalDirectories: [sourceDirectory], modeMembership: [.code])
            let project = ProjectSnapshot(record: record,
                workspace: try workspaceForDirectory(projectId: projectID, guestPath: sourceDirectory, requireExisting: false),
                sourceWorkspace: ProjectWorkspace(projectId: projectID, hostURL: workspaceURL), sessions: [])
            try validateAuthoringIntent(intent, project: project)
            let selection = LocalAppAuthoringSelection(projectId: projectID, intentId: intentID)
            try writeDurableStaged(record, to: staging.appendingPathComponent(projectManifestFileName))
            try writeDurableStaged(ProjectSessionIndex(sessions: []), to: staging.appendingPathComponent(projectSessionIndexFileName))
            try writeDurableStaged(VersionedBaseline(files: []), to: staging.appendingPathComponent(projectSyncBaselineFileName))
            try writeDurableStaged(intent, to: staging.appendingPathComponent(localAppIntentPrefix + intentID + ".json"))
            try writeDurableStaged(selection, to: staging.appendingPathComponent(localAppSelectionFileName))
            let result: Int32 = staging.withUnsafeFileSystemRepresentation { source in
                directory.withUnsafeFileSystemRepresentation { destination in
                    guard let source, let destination else { return -1 }
                    return Darwin.renamex_np(source, destination, UInt32(RENAME_EXCL))
                }
            }
            guard result == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            published = true
            let descriptor = projectsRoot.path.withCString { open($0, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC) }
            guard descriptor >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            defer { close(descriptor) }
            guard fsync(descriptor) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            try writeIndex(ProjectIndex(activeProjectId: index.activeProjectId,
                projectIds: (index.projectIds + [projectID]).uniqued()), expected: indexExpected)
            let indexData = try ownerCatalogSnapshot(indexURL).bytes
            try synchronizeOwnerCatalog(indexURL, expectedBytes: indexData)
            return (try loadProject(id: projectID), intent)
        } catch {
            if !published { try? FileManager.default.removeItem(at: staging) }
            throw error
        }
    }

    private func writeDurableStaged<T: Codable>(_ value: T, to url: URL) throws {
        let data = try encoder.encode(value)
        guard data.count <= 65_536 else { throw ProjectRepositoryError.invalidProjectData }
        try writer.writeData(data, to: url, expected: .absent, validate: { staged in
            _ = try self.decoder.decode(T.self, from: staged)
        }, commit: { try $0() })
        try synchronizeOwnerCatalog(url, expectedBytes: data)
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
        let expected = try ownerCatalogExpectation(projectsRoot.appendingPathComponent(projectIndexFileName))
        let current = try healthyState()
        try writeIndex(ProjectIndex(activeProjectId: projectId, projectIds: current.projects.map(\.record.id)), expected: expected)
        return load()
    }

    func updateProject(_ record: ProjectRecord) throws -> ProjectSnapshot {
        try validateProjectID(record.id)
        let expected = try ownerCatalogExpectation(projectDirectory(record.id).appendingPathComponent(projectManifestFileName))
        _ = try healthyState()
        try validateProjectID(record.id)
        guard FileManager.default.fileExists(atPath: projectDirectory(record.id).path)
        else { throw ProjectRepositoryError.projectDoesNotExist(record.id) }
        var normalized = record
        normalized.updatedAt = now()
        try writeProject(normalized, expected: expected)
        return try loadProject(id: record.id)
    }

    /// Intent is recovery evidence only. The native Local App service must
    /// still authenticate sourceId and the exact session before granting use.
    func localAppAuthoringIntents(projectId: String) throws -> [LocalAppAuthoringIntent] {
        let project = try loadProject(id: projectId)
        let directory = projectDirectory(projectId)
        let names = try FileManager.default.contentsOfDirectory(atPath: directory.path)
            .filter { $0.hasPrefix(localAppIntentPrefix) }.sorted()
        guard names.count <= 4096 else { throw ProjectRepositoryError.invalidProjectData }
        return try names.map { name in
            guard name.hasSuffix(".json") else { throw ProjectRepositoryError.invalidProjectData }
            let identifier = String(name.dropFirst(localAppIntentPrefix.count).dropLast(5))
            guard isLowercaseUUID(identifier) else { throw ProjectRepositoryError.invalidProjectData }
            let snapshot = try boundedAuthoringIntentSnapshot(directory.appendingPathComponent(name))
            let intent = try decodeAuthoringIntent(snapshot.bytes)
            try validateAuthoringIntent(intent, project: project)
            guard intent.intentId == identifier else { throw ProjectRepositoryError.invalidProjectData }
            return intent
        }
    }

    func storeLocalAppAuthoringIntent(_ intent: LocalAppAuthoringIntent,
                                      replacing previous: LocalAppAuthoringIntent?) throws {
        let project = try loadProject(id: intent.projectId)
        try validateAuthoringIntent(intent, project: project)
        if let previous {
            guard previous.intentId == intent.intentId, previous.sourceId == intent.sourceId,
                  previous.sourceRequestId == intent.sourceRequestId,
                  previous.sourceDirectory == intent.sourceDirectory, previous.projectId == intent.projectId,
                  previous.mode == intent.mode, previous.appId == intent.appId,
                  previous.authoringSessionId == nil || previous.authoringSessionId == intent.authoringSessionId,
                  previous.transcriptPath == nil || previous.transcriptPath == intent.transcriptPath else {
                throw ProjectRepositoryError.invalidProjectData
            }
        }
        let url = projectDirectory(intent.projectId)
            .appendingPathComponent(localAppIntentPrefix + intent.intentId + ".json")
        let expected: OwnerCatalogExpectation
        if try ownerCatalogFileExists(url) {
            let snapshot = try boundedAuthoringIntentSnapshot(url)
            let current = try decodeAuthoringIntent(snapshot.bytes)
            try validateAuthoringIntent(current, project: project)
            if current == intent {
                try synchronizeOwnerCatalog(url, expectedBytes: snapshot.bytes)
                return
            }
            guard current == previous else { throw ProjectRepositoryError.invalidProjectData }
            expected = .present(snapshot)
        } else {
            guard previous == nil else { throw ProjectRepositoryError.invalidProjectData }
            expected = .absent
        }
        let data = try encoder.encode(intent)
        guard data.count <= 65_536 else { throw ProjectRepositoryError.invalidProjectData }
        try writer.writeData(data, to: url, expected: expected, validate: { staged in
            guard try self.decodeAuthoringIntent(staged) == intent else { throw ProjectRepositoryError.invalidProjectData }
        }, commit: { try $0() })
        try synchronizeOwnerCatalog(url, expectedBytes: data)
    }

    func selectedLocalAppAuthoringIntent(projectId: String) throws -> LocalAppAuthoringIntent? {
        let intents = try localAppAuthoringIntents(projectId: projectId)
        let url = projectDirectory(projectId).appendingPathComponent(localAppSelectionFileName)
        guard try ownerCatalogFileExists(url) else { return nil }
        let snapshot = try boundedAuthoringIntentSnapshot(url)
        let selection = try decodeAuthoringSelection(snapshot.bytes, projectId: projectId)
        let matches = intents.filter { $0.intentId == selection.intentId }
        guard matches.count == 1 else { throw ProjectRepositoryError.invalidProjectData }
        return matches[0]
    }

    func selectLocalAppAuthoringIntent(projectId: String, intentId: String,
                                       authority: WorkspaceWriteAuthority, revision: UInt64,
                                       projectAuthority: WorkspaceWriteAuthority, projectRevision: UInt64) throws {
        let intents = try localAppAuthoringIntents(projectId: projectId)
        guard intents.filter({ $0.intentId == intentId }).count == 1 else {
            throw ProjectRepositoryError.invalidProjectData
        }
        let url = projectDirectory(projectId).appendingPathComponent(localAppSelectionFileName)
        let expected: OwnerCatalogExpectation
        if try ownerCatalogFileExists(url) {
            let snapshot = try boundedAuthoringIntentSnapshot(url)
            _ = try decodeAuthoringSelection(snapshot.bytes, projectId: projectId)
            expected = .present(snapshot)
        } else {
            expected = .absent
        }
        let selection = LocalAppAuthoringSelection(projectId: projectId, intentId: intentId)
        let data = try encoder.encode(selection)
        try writer.writeData(data, to: url, expected: expected, validate: { staged in
            guard try self.decodeAuthoringSelection(staged, projectId: projectId) == selection else {
                throw ProjectRepositoryError.invalidProjectData
            }
        }, commit: { replace in
            try authority.commit(revision: revision) { _ in
                try projectAuthority.commit(revision: projectRevision) { _ in try replace() }
            }
        })
        try synchronizeOwnerCatalog(url, expectedBytes: data)
    }

    private func decodeAuthoringSelection(_ data: Data, projectId: String) throws -> LocalAppAuthoringSelection {
        try UniqueJSONKeyGuard.validate(data)
        guard let fields = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              Set(fields.keys) == Set(["version", "projectId", "intentId"]) else {
            throw ProjectRepositoryError.invalidProjectData
        }
        let selection = try decoder.decode(LocalAppAuthoringSelection.self, from: data)
        guard selection.version == 1, selection.projectId == projectId,
              isLowercaseUUID(selection.projectId), isLowercaseUUID(selection.intentId) else {
            throw ProjectRepositoryError.invalidProjectData
        }
        return selection
    }

    private func decodeAuthoringIntent(_ data: Data) throws -> LocalAppAuthoringIntent {
        try UniqueJSONKeyGuard.validate(data)
        guard let fields = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw ProjectRepositoryError.invalidProjectData
        }
        let required: Set<String> = ["version", "intentId", "sourceId", "sourceRequestId", "sourceDirectory", "projectId", "mode"]
        let allowed = required.union(["authoringSessionId", "transcriptPath", "appId"])
        guard required.isSubset(of: Set(fields.keys)), Set(fields.keys).isSubset(of: allowed) else {
            throw ProjectRepositoryError.invalidProjectData
        }
        return try decoder.decode(LocalAppAuthoringIntent.self, from: data)
    }

    private func validateAuthoringIntent(_ intent: LocalAppAuthoringIntent, project: ProjectSnapshot) throws {
        let root = MobileUserStorage.directories().appsGuestPath + "/"
        let tail = intent.sourceDirectory.hasPrefix(root) ? String(intent.sourceDirectory.dropFirst(root.count)) : ""
        guard intent.version == 1, intent.mode == .code,
              (intent.authoringSessionId == nil && intent.transcriptPath == nil)
                || (intent.authoringSessionId != nil && intent.transcriptPath.map({ !$0.isEmpty && ($0 as NSString).isAbsolutePath }) == true),
              isLowercaseUUID(intent.intentId), isLowercaseUUID(intent.sourceId), isLowercaseUUID(intent.sourceRequestId),
              isLowercaseUUID(intent.projectId), intent.projectId == project.record.id,
              intent.authoringSessionId.map(isLowercaseUUID) ?? true,
              !tail.isEmpty, tail != ".", tail != "..", !tail.contains("/"), !tail.contains("\\"),
              intent.appId.map({ !$0.isEmpty && $0.utf8.count <= 256 }) ?? true,
              project.record.activeDirectory == intent.sourceDirectory
                || project.record.additionalDirectories.contains(intent.sourceDirectory) else {
            throw ProjectRepositoryError.invalidProjectData
        }
    }

    func updateSessions(projectId: String?, sessions: [ProjectSessionSummary]) throws -> ProjectRepositoryState {
        if let projectId { try validateProjectID(projectId) }
        let sessionURL = projectId.map { projectDirectory($0).appendingPathComponent(projectSessionIndexFileName) }
            ?? projectsRoot.appendingPathComponent(globalSessionIndexFileName)
        let expected = try ownerCatalogExpectation(sessionURL)
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
            if let prior = existing.first(where: { canonicalSessionID($0.sessionId) == identity }) {
                guard prior.mode == row.mode else { throw ProjectRepositoryError.conflictingSessionMode }
                if let oldPath = prior.transcriptPath, !oldPath.isEmpty,
                   let newPath = row.transcriptPath, !newPath.isEmpty, oldPath != newPath {
                    throw ProjectRepositoryError.invalidSessionOwnership(
                        "\(identity): the session catalog conflicts with its saved transcript path."
                    )
                }
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
                if row.transcriptPath?.isEmpty != false { row.transcriptPath = cached?.transcriptPath }
                row.isArchived = cached?.isArchived ?? false
                row.pendingCatalogConfirmation = false
                return row
            }
            .uniqued(on: \.sessionId)
            .sorted { $0.updatedAt > $1.updatedAt }
        let incomingIDs = Set(incoming.map(\.sessionId))
        // Catalogs can cover only one workspace. An omitted confirmed source
        // remains an owner even when a later listing does not include it.
        let normalized = (incoming + existing.filter {
            !incomingIDs.contains($0.sessionId) && ($0.transcriptPath?.isEmpty == false || $0.pendingCatalogConfirmation || $0.isArchived ||
                (snapshot != nil && $0.workspaceDirectory != nil && $0.workspaceDirectory != snapshot?.record.activeDirectory))
        }).sorted { $0.updatedAt > $1.updatedAt }
        if let projectId {
            try validateProjectID(projectId)
            try writeSessions(projectId: projectId, sessions: normalized, expected: expected)
            let manifestExpected = try ownerCatalogExpectation(projectDirectory(projectId).appendingPathComponent(projectManifestFileName))
            let snapshot = try loadProject(id: projectId)
            var record = snapshot.record
            record.modeMembership.formUnion(incoming.map(\.mode))
            try writeProject(record, expected: manifestExpected)
        } else {
            try writeSessionIndex(normalized, to: sessionURL, expected: expected)
        }
        return load()
    }

    func recordStartedSession(
        projectId: String?,
        sessionId: String,
        title: String,
        mode: SessionMode = .code,
        initialMessageCount: Int = 0,
        workspaceDirectory: String? = nil,
        authoritativeTranscriptPath: String
    ) throws -> ProjectRepositoryState {
        if let projectId { try validateProjectID(projectId) }
        let sessionURL = projectId.map { projectDirectory($0).appendingPathComponent(projectSessionIndexFileName) }
            ?? projectsRoot.appendingPathComponent(globalSessionIndexFileName)
        let expected = try ownerCatalogExpectation(sessionURL)
        let manifestExpected = try projectId.map { try ownerCatalogExpectation(projectDirectory($0).appendingPathComponent(projectManifestFileName)) }
        let state = try healthyState()
        let canonicalID = canonicalSessionID(sessionId)
        guard !canonicalID.isEmpty else { throw ProjectRepositoryError.invalidSessionID }
        try checkSessionScope(canonicalID, projectId: projectId, state: state)
        let snapshot = try projectId.map { try loadProject(id: $0) }
        let sessions = try snapshot?.sessions ?? readSessions(at: projectsRoot.appendingPathComponent(globalSessionIndexFileName), required: false)
        let timestamp = now()
        let existing = sessions.first(where: { $0.sessionId == canonicalID })
        guard existing == nil || existing?.mode == mode else { throw ProjectRepositoryError.conflictingSessionMode }
        guard !authoritativeTranscriptPath.isEmpty, (authoritativeTranscriptPath as NSString).isAbsolutePath else {
            throw ProjectRepositoryError.missingSessionTranscriptPath
        }
        if let old = existing?.transcriptPath, !old.isEmpty, old != authoritativeTranscriptPath {
            throw ProjectRepositoryError.invalidSessionOwnership("\(canonicalID): the started session conflicts with its saved transcript path.")
        }
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
            transcriptPath: existing?.transcriptPath?.isEmpty == false ? existing?.transcriptPath : authoritativeTranscriptPath
        )
        let updated = ([started] + sessions.filter { $0.sessionId != canonicalID })
        if let projectId {
            try writeSessions(projectId: projectId, sessions: updated, expected: expected)
            if let snapshot {
                try writeProject(snapshot.record.with(updatedAt: timestamp), expected: manifestExpected!)
            }
        } else {
            try writeSessionIndex(updated, to: sessionURL, expected: expected)
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
        if let projectId { try validateProjectID(projectId) }
        let sessionURL = projectId.map { projectDirectory($0).appendingPathComponent(projectSessionIndexFileName) }
            ?? projectsRoot.appendingPathComponent(globalSessionIndexFileName)
        let expected = try ownerCatalogExpectation(sessionURL)
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
            try writeSessions(projectId: projectId, sessions: updated, expected: expected)
        } else {
            try writeSessionIndex(updated, to: sessionURL, expected: expected)
        }
        return load()
    }

    func deleteProject(projectId: String) throws -> ProjectRepositoryState {
        let expected = try ownerCatalogExpectation(projectsRoot.appendingPathComponent(projectIndexFileName))
        let current = try healthyState()
        try validateProjectID(projectId)
        let directory = projectDirectory(projectId)
        guard FileManager.default.fileExists(atPath: directory.path) else {
            throw ProjectRepositoryError.projectDoesNotExist(projectId)
        }
        try FileManager.default.removeItem(at: directory)
        let nextIDs = current.projects.map(\.record.id).filter { $0 != projectId }
        let nextActive = current.activeProjectId == projectId ? nil : current.activeProjectId
        try writeIndex(ProjectIndex(activeProjectId: nextActive, projectIds: nextIDs), expected: expected)
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

    /// A verified engine workspace may update the owner's cwd, but `/cd`
    /// preserves its original transcript. Recheck the exact source at the
    /// serialized write boundary, including when the owner is off screen.
    func recordAuthenticatedWorkspace(projectId: String?, state: SessionWorkspaceState,
                                      mode: SessionMode, authority: WorkspaceWriteAuthority,
                                      revision: UInt64) throws -> ProjectRepositoryState {
        let catalog = try healthyState()
        let identity = canonicalSessionID(state.sessionId)
        guard !identity.isEmpty, !state.transcriptPath.isEmpty,
              (state.transcriptPath as NSString).isAbsolutePath else {
            throw ProjectRepositoryError.missingSessionTranscriptPath
        }
        try checkSessionScope(identity, projectId: projectId, state: catalog)
        if let projectId { try validateProjectID(projectId) }
        let indexURL = projectId.map { projectDirectory($0).appendingPathComponent(projectSessionIndexFileName) }
            ?? projectsRoot.appendingPathComponent(globalSessionIndexFileName)
        let indexBefore = try ownerCatalogFileExists(indexURL) ? ownerCatalogSnapshot(indexURL) : nil
        let manifestURL = projectId.map { projectDirectory($0).appendingPathComponent(projectManifestFileName) }
        let manifestBefore = try manifestURL.map { try ownerCatalogSnapshot($0) }
        let snapshot = try projectId.map { try loadProject(id: $0) }
        var rows = try snapshot?.sessions ?? readSessions(at: projectsRoot.appendingPathComponent(globalSessionIndexFileName), required: false)
        if let indexBefore {
            guard try ownerCatalogSnapshot(indexURL) == indexBefore else { throw ProjectRepositoryError.invalidProjectData }
        } else if try ownerCatalogFileExists(indexURL) {
            throw ProjectRepositoryError.invalidProjectData
        }
        if let manifestURL, let manifestBefore {
            guard try ownerCatalogSnapshot(manifestURL) == manifestBefore else { throw ProjectRepositoryError.invalidProjectData }
        }
        if let index = rows.firstIndex(where: { canonicalSessionID($0.sessionId) == identity }) {
            guard rows[index].mode == mode else { throw ProjectRepositoryError.conflictingSessionMode }
            if let existing = rows[index].transcriptPath, !existing.isEmpty, existing != state.transcriptPath {
                throw ProjectRepositoryError.invalidSessionOwnership("\(identity): the workspace source conflicts with its saved transcript path.")
            }
        }
        if let projectId {
            _ = try workspaceForDirectory(projectId: projectId, guestPath: state.cwd)
            for directory in state.additionalDirectories {
                _ = try workspaceForDirectory(projectId: projectId, guestPath: directory, requireExisting: false)
            }
        }
        if let index = rows.firstIndex(where: { canonicalSessionID($0.sessionId) == identity }) {
            rows[index].workspaceDirectory = state.cwd
            rows[index].originalProjectDirectory = state.projectRoot
            rows[index].transcriptPath = state.transcriptPath
            guard let indexBefore else { throw ProjectRepositoryError.missingSessionIndex }
            try writeWorkspaceRecord(ProjectSessionIndex(sessions: rows), to: indexURL, expected: indexBefore,
                                     authority: authority, revision: revision, selectedOnly: false)
            if projectId == nil { observedGlobalIndex = true }
        }
        if let snapshot, let manifestURL, let manifestBefore {
            var record = snapshot.record
            record.activeDirectory = state.cwd
            record.additionalDirectories = (record.additionalDirectories + state.additionalDirectories + [state.cwd])
                .filter { $0 != record.directory }.uniqued()
            try writeWorkspaceRecord(record, to: manifestURL, expected: manifestBefore,
                                     authority: authority, revision: revision, selectedOnly: true)
        }
        return load()
    }

    private func writeWorkspaceRecord<T: Codable>(_ value: T, to url: URL, expected: OwnerCatalogSnapshot,
                                                  authority: WorkspaceWriteAuthority, revision: UInt64,
                                                  selectedOnly: Bool) throws {
        let data = try encoder.encode(value)
        try writer.writeData(data, to: url, expected: .present(expected), validate: { _ = try self.decoder.decode(T.self, from: $0) }) { replace in
            try authority.commit(revision: revision) { selected in
                if !selectedOnly || selected { try replace() }
            }
        }
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
        let indexExpected = try ownerCatalogExpectation(projectsRoot.appendingPathComponent(projectIndexFileName))
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
            try writeProject(record, expected: .absent)
            try writeSessions(projectId: id, sessions: [], expected: .absent)
            try writeBaseline(projectId: id, baseline: ProjectSyncBaseline())
            try writeIndex(ProjectIndex(
                activeProjectId: index.activeProjectId,
                projectIds: (index.projectIds + [id]).uniqued()
            ), expected: indexExpected)
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

    private func writeProject(_ record: ProjectRecord, expected: OwnerCatalogExpectation) throws {
        try write(record, to: projectDirectory(record.id).appendingPathComponent(projectManifestFileName), expected: expected)
    }

    private func writeSessions(projectId: String, sessions: [ProjectSessionSummary], expected: OwnerCatalogExpectation) throws {
        try writeSessionIndex(sessions, to: projectDirectory(projectId).appendingPathComponent(projectSessionIndexFileName), expected: expected)
    }

    private func writeSessionIndex(_ sessions: [ProjectSessionSummary], to url: URL, expected: OwnerCatalogExpectation) throws {
        if FileManager.default.fileExists(atPath: url.path) {
            _ = try readSessions(at: url, required: true)
        }
        try write(ProjectSessionIndex(sessions: sessions), to: url, expected: expected)
        if url == projectsRoot.appendingPathComponent(globalSessionIndexFileName) { observedGlobalIndex = true }
    }

    private func readSessions(at url: URL, required: Bool) throws -> [ProjectSessionSummary] {
        guard try ownerCatalogFileExists(url) else {
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
        guard try ownerCatalogFileExists(url) else {
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

    private func writeIndex(_ index: ProjectIndex, expected: OwnerCatalogExpectation) throws {
        try write(index, to: projectsRoot.appendingPathComponent(projectIndexFileName), expected: expected)
        observedProjectIndex = true
    }

    private func write<T: Codable>(_ value: T, to url: URL, expected: OwnerCatalogExpectation? = nil) throws {
        let expected = try expected ?? ownerCatalogExpectation(url)
        let data = try encoder.encode(value)
        try writer.writeData(data, to: url, expected: expected, validate: { _ = try self.decoder.decode(T.self, from: $0) }, commit: { try $0() })
    }

    private func decode<T: Decodable>(_ type: T.Type, from url: URL) throws -> T {
        guard try ownerCatalogFileExists(url) else { throw ProjectRepositoryError.invalidProjectData }
        let data = try Data(contentsOf: url)
        try UniqueJSONKeyGuard.validate(data)
        return try decoder.decode(type, from: data)
    }
}

struct OwnerCatalogVersion: Equatable, Sendable {
    let device: dev_t
    let inode: ino_t
    let size: off_t
    let modifiedSeconds: Int
    let modifiedNanoseconds: Int
    let changedSeconds: Int
    let changedNanoseconds: Int

    init(_ information: stat) throws {
        guard (information.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG) else {
            throw ProjectRepositoryError.invalidProjectData
        }
        device = information.st_dev
        inode = information.st_ino
        size = information.st_size
        modifiedSeconds = information.st_mtimespec.tv_sec
        modifiedNanoseconds = information.st_mtimespec.tv_nsec
        changedSeconds = information.st_ctimespec.tv_sec
        changedNanoseconds = information.st_ctimespec.tv_nsec
    }
}

struct OwnerCatalogSnapshot: Equatable, Sendable {
    let version: OwnerCatalogVersion
    let bytes: Data
}

enum OwnerCatalogExpectation: Equatable, Sendable {
    case absent
    case present(OwnerCatalogSnapshot)

    var bytes: Data? {
        if case let .present(snapshot) = self { return snapshot.bytes }
        return nil
    }
}

func ownerCatalogExpectation(_ url: URL) throws -> OwnerCatalogExpectation {
    guard try ownerCatalogFileExists(url) else { return .absent }
    return .present(try ownerCatalogSnapshot(url))
}

/// Flush a newly published catalog and its directory before acknowledging a
/// reservation. Detect a visible replacement while doing so.
func synchronizeOwnerCatalog(_ url: URL, expectedBytes: Data) throws {
    let expected = try ownerCatalogExpectation(url)
    guard case let .present(snapshot) = expected, snapshot.bytes == expectedBytes else { throw ProjectRepositoryError.invalidProjectData }
    let descriptor = url.path.withCString { open($0, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK) }
    guard descriptor >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    defer { close(descriptor) }
    var information = stat()
    guard fstat(descriptor, &information) == 0,
          try OwnerCatalogVersion(information) == snapshot.version else { throw ProjectRepositoryError.invalidProjectData }
    guard fsync(descriptor) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    guard fstat(descriptor, &information) == 0,
          try OwnerCatalogVersion(information) == snapshot.version else { throw ProjectRepositoryError.invalidProjectData }
    let directory = url.deletingLastPathComponent().path.withCString { open($0, O_RDONLY | O_CLOEXEC) }
    guard directory >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    defer { close(directory) }
    guard fsync(directory) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    guard try ownerCatalogExpectation(url) == expected else { throw ProjectRepositoryError.invalidProjectData }
}

private func ownerCatalogExpectationMatchesVersion(_ url: URL, _ expected: OwnerCatalogExpectation) throws -> Bool {
    switch expected {
    case .absent: return try !ownerCatalogFileExists(url)
    case let .present(snapshot):
        guard try ownerCatalogFileExists(url) else { return false }
        return try ownerCatalogVersion(url) == snapshot.version
    }
}

private func ownerCatalogVersion(_ url: URL) throws -> OwnerCatalogVersion {
    var information = stat()
    guard url.path.withCString({ lstat($0, &information) }) == 0 else {
        throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
    }
    return try OwnerCatalogVersion(information)
}

/// Read the same regular file that remains at this directory entry. Capturing
/// both its identity and bytes lets a staged update reject a changed source.
private func ownerCatalogSnapshot(_ url: URL) throws -> OwnerCatalogSnapshot {
    let descriptor = url.path.withCString { Darwin.open($0, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK) }
    guard descriptor >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    let handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
    defer { try? handle.close() }
    var information = stat()
    guard fstat(descriptor, &information) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    let before = try OwnerCatalogVersion(information)
    let bytes = try handle.readToEnd() ?? Data()
    guard fstat(descriptor, &information) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    let after = try OwnerCatalogVersion(information)
    guard before == after, try ownerCatalogVersion(url) == after else { throw ProjectRepositoryError.invalidProjectData }
    return OwnerCatalogSnapshot(version: after, bytes: bytes)
}

private func boundedAuthoringIntentSnapshot(_ url: URL) throws -> OwnerCatalogSnapshot {
    try boundedOwnerCatalogSnapshot(url, maximumSize: 65_536)
}

private func boundedOwnerCatalogSnapshot(_ url: URL, maximumSize: Int) throws -> OwnerCatalogSnapshot {
    let descriptor = url.path.withCString { Darwin.open($0, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK) }
    guard descriptor >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    let handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
    defer { try? handle.close() }
    var information = stat()
    guard fstat(descriptor, &information) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    let before = try OwnerCatalogVersion(information)
    guard before.size >= 0, before.size <= off_t(maximumSize) else { throw ProjectRepositoryError.invalidProjectData }
    let length = Int(before.size)
    var bytes = Data()
    while bytes.count < length {
        guard let chunk = try handle.read(upToCount: length - bytes.count), !chunk.isEmpty else {
            throw ProjectRepositoryError.invalidProjectData
        }
        bytes.append(chunk)
    }
    guard fstat(descriptor, &information) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    let after = try OwnerCatalogVersion(information)
    guard before == after, try ownerCatalogVersion(url) == after else { throw ProjectRepositoryError.invalidProjectData }
    return OwnerCatalogSnapshot(version: after, bytes: bytes)
}

/// Inspect the directory entry itself, including dangling symlinks. URL
/// resource values can be cached and fileExists follows symlinks.
func ownerCatalogFileExists(_ url: URL) throws -> Bool {
    var information = stat()
    let result = url.path.withCString { lstat($0, &information) }
    if result != 0 {
        if errno == ENOENT { return false }
        throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
    }
    guard (information.st_mode & mode_t(S_IFMT)) != mode_t(S_IFLNK) else {
        throw ProjectRepositoryError.invalidProjectData
    }
    return true
}

/// Foundation's keyed decoder accepts duplicate JSON names. Reject them before
/// decoding so corrupt indexes cannot be interpreted as a different owner.
struct UniqueJSONKeyGuard {
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
    case missingSessionTranscriptPath
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
        case .missingSessionTranscriptPath: return "The selected session's transcript path is unavailable. The session was not opened."
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
