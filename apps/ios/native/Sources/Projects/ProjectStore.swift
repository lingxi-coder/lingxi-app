import Foundation
import Observation

@MainActor
@Observable
final class ProjectStore {
    private let repository: ProjectRepository
    private let synchronizer: ProjectWorkspaceSynchronizer
    private let bookmarkResolver: ProjectBookmarkResolving
    private let executionDomain: ProjectStoreExecutionDomain

    var projects: [ProjectSnapshot] = []
    var activeProjectId: String?
    var globalSessions: [ProjectSessionSummary] = []
    var conflicts: [ProjectSyncConflict] = []
    var loading = true
    var operation: ProjectOperation?
    var errorMessage: String?
    var onWillArchiveSession: ((String?, String) async throws -> Void)?
    var sessionScopeClaims: (() -> [String: ConversationScope]?)?
    private var catalogIsHealthy = true
    private var catalogErrorMessage: String?
    private var projectIndexPresent = false
    private var globalIndexPresent = false

    var activeProject: ProjectSnapshot? {
        projects.first(where: { $0.record.id == activeProjectId })
    }

    init(
        repository: ProjectRepository,
        synchronizer: ProjectWorkspaceSynchronizer,
        bookmarkResolver: ProjectBookmarkResolving = SecurityScopedProjectBookmarkResolver(),
        executionDomain: ProjectStoreExecutionDomain = ProjectStoreExecutionDomain()
    ) {
        self.repository = repository
        self.synchronizer = synchronizer
        self.bookmarkResolver = bookmarkResolver
        self.executionDomain = executionDomain
        publish(executionDomain.sync { repository.load() })
    }

    convenience init(managedRoot: String) {
        let paths = MobileUserStorage.directories(managedRoot: managedRoot)
        let repository = ProjectRepository(
            projectsRoot: URL(fileURLWithPath: paths.projects, isDirectory: true),
            userHome: URL(fileURLWithPath: paths.home, isDirectory: true)
        )
        let synchronizer = ProjectWorkspaceSynchronizer(repository: repository)
        self.init(repository: repository, synchronizer: synchronizer)
    }

    /// Keep simulated UI-test sessions and projects out of the user's index.
    static func forRuntime(
        managedRoot: String,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> ProjectStore {
        #if DEBUG
        if environment["LINGXI_UI_TESTING"] == "1" {
            let testRoot = URL(fileURLWithPath: managedRoot, isDirectory: true)
                .appendingPathComponent("ui-testing", isDirectory: true).path
            return ProjectStore(managedRoot: testRoot)
        }
        #endif
        return ProjectStore(managedRoot: managedRoot)
    }

    private func validatedSessionScopeClaims() throws -> [String: ConversationScope] {
        guard catalogIsHealthy else {
            throw ProjectRepositoryError.invalidSessionOwnership(catalogErrorMessage ?? ProjectRepositoryError.invalidProjectData.localizedDescription)
        }
        if projectIndexPresent, !FileManager.default.fileExists(atPath: repository.projectsRoot.appendingPathComponent(projectIndexFileName).path) {
            throw ProjectRepositoryError.invalidSessionOwnership("projects-index: \(ProjectRepositoryError.missingProjectIndex.localizedDescription)")
        }
        if globalIndexPresent, !FileManager.default.fileExists(atPath: repository.projectsRoot.appendingPathComponent(globalSessionIndexFileName).path) {
            throw ProjectRepositoryError.invalidSessionOwnership("global: \(ProjectRepositoryError.missingSessionIndex.localizedDescription)")
        }
        let claims: [String: ConversationScope]
        if let sessionScopeClaims {
            guard let stored = sessionScopeClaims() else {
                throw ProjectRepositoryError.invalidSessionOwnership("session-scopes: \(ProjectRepositoryError.invalidProjectData.localizedDescription)")
            }
            claims = stored
        } else {
            claims = [:]
        }
        if let message = Self.claimsError(claims, projects: projects, globalSessions: globalSessions,
                                          projectIndexPresent: projectIndexPresent, globalIndexPresent: globalIndexPresent) {
            throw ProjectRepositoryError.invalidSessionOwnership(message)
        }
        return claims
    }

    private static func claimsError(_ claims: [String: ConversationScope], projects: [ProjectSnapshot],
                                    globalSessions: [ProjectSessionSummary], projectIndexPresent: Bool,
                                    globalIndexPresent: Bool) -> String? {
        if let conflict = SessionCatalogOwnership.firstConflictingClaim(projects: projects, globalSessions: globalSessions, claims: claims) {
            return "\(conflict.sessionID) (\(conflict.indexed.workspaceKey), \(conflict.claimed.workspaceKey)): \(ProjectRepositoryError.conflictingSessionScope.localizedDescription)"
        }
        if !globalIndexPresent, claims.values.contains(.global) {
            return "global: \(ProjectRepositoryError.missingSessionIndex.localizedDescription)"
        }
        let projectClaims = Set(claims.values.compactMap(\.projectID))
        if !projectClaims.isEmpty, !projectIndexPresent {
            return "projects-index: \(ProjectRepositoryError.missingProjectIndex.localizedDescription)"
        }
        if let missing = projectClaims.first(where: { id in !projects.contains(where: { $0.id == id }) }) {
            return "project.\(missing): \(ProjectRepositoryError.invalidProjectData.localizedDescription)"
        }
        return nil
    }

    var sessionOwnershipError: String? {
        do { _ = try validatedSessionScopeClaims(); return nil }
        catch { return error.localizedDescription }
    }

    func allowsSession(_ sessionID: String, in scope: ConversationScope, mode: SessionMode? = nil) -> Bool {
        guard let claims = try? validatedSessionScopeClaims() else { return false }
        return SessionCatalogOwnership.allows(sessionID: sessionID, scope: scope, mode: mode,
            projects: projects, globalSessions: globalSessions, claimedScope: claims[canonicalSessionID(sessionID)])
    }

    /// Re-read every persisted index before starting or claiming a new owner.
    /// Existing admitted owners are handled by SessionHost and keep running.
    private func freshAdmissionState() -> (claims: [String: ConversationScope], state: ProjectRepositoryState)? {
        let claims: [String: ConversationScope]
        do { claims = try validatedSessionScopeClaims() }
        catch { errorMessage = error.localizedDescription; return nil }
        let state = repository.admissionSnapshot()
        if let error = state.errorMessage { errorMessage = error; return nil }
        if projectIndexPresent && !state.projectIndexPresent {
            errorMessage = "projects-index: \(ProjectRepositoryError.missingProjectIndex.localizedDescription)"
            return nil
        }
        if globalIndexPresent && !state.globalIndexPresent {
            errorMessage = "global: \(ProjectRepositoryError.missingSessionIndex.localizedDescription)"
            return nil
        }
        if let message = Self.claimsError(claims, projects: state.projects, globalSessions: state.globalSessions,
                                          projectIndexPresent: state.projectIndexPresent, globalIndexPresent: state.globalIndexPresent) {
            errorMessage = message
            return nil
        }
        return (claims, state)
    }

    func admitsUnclaimedOwner() -> Bool { freshAdmissionState() != nil }

    func admitsSessionIdentity(_ sessionID: String, in scope: ConversationScope, mode: SessionMode) -> Bool {
        guard let (claims, state) = freshAdmissionState() else { return false }
        guard SessionCatalogOwnership.allows(sessionID: sessionID, scope: scope, mode: mode,
            projects: state.projects, globalSessions: state.globalSessions,
            claimedScope: claims[canonicalSessionID(sessionID)]) else {
            errorMessage = ProjectRepositoryError.conflictingSessionScope.localizedDescription
            return false
        }
        return true
    }

    func isSessionIndexed(_ sessionID: String, in scope: ConversationScope) -> Bool {
        let id = canonicalSessionID(sessionID)
        guard !id.isEmpty else { return false }
        switch scope {
        case .global:
            return globalSessions.contains { canonicalSessionID($0.sessionId) == id }
        case .project(let projectID):
            return projects.first { $0.id == projectID }?.sessions.contains { canonicalSessionID($0.sessionId) == id } ?? false
        case .scheduled, .remote:
            return false
        }
    }

    private func requireConsistentOwners(projectId: String?) throws -> ConversationScope {
        _ = try validatedSessionScopeClaims()
        let scope: ConversationScope = projectId.map { .project($0) } ?? .global
        if let projectId, !projects.contains(where: { $0.id == projectId }) {
            throw ProjectRepositoryError.projectDoesNotExist(projectId)
        }
        return scope
    }

    func reload() {
        Task { await reloadAsync() }
    }

    func reloadAsync() async {
        await runOperation(.refreshSessions, message: String(localized: "project_loading_message")) {
            let repository = self.repository
            let executionDomain = self.executionDomain
            return try await executionDomain.async {
                repository.load()
            }
        } onSuccess: { state in
            self.publish(state)
        }
    }

    @discardableResult
    func createInternal(name: String, mode: SessionMode = .code) async throws -> ProjectSnapshot {
        try await runThrowingOperation(.create, message: String(localized: "project_creating_message")) {
            _ = try self.requireConsistentOwners(projectId: nil)
            let repository = self.repository
            let executionDomain = self.executionDomain
            return try await executionDomain.async {
                let created = try repository.createInternal(name: name, mode: mode)
                return StoreCreateResult(project: created, state: repository.load())
            }
        } onSuccess: { result in
            self.publish(result.state)
            return result.project
        }
    }

    /// Creates a dedicated Mobile Project whose active Code cwd is the
    /// immutable app source copy. The project ID remains the native workspace
    /// identity; the source folder is an authorized home directory.
    @discardableResult
    func createLocalAppAuthoringProject(name: String, sourceDirectory: String) async throws -> ProjectSnapshot {
        let project = try await createInternal(name: name)
        var record = project.record
        record.activeDirectory = sourceDirectory
        var seenDirectories = Set<String>()
        record.additionalDirectories = (record.additionalDirectories + [sourceDirectory]).filter {
            seenDirectories.insert($0).inserted
        }
        let repository = repository
        let executionDomain = executionDomain
        let updatedRecord = record
        let updated = try await executionDomain.async { try repository.updateProject(updatedRecord) }
        let state = try await executionDomain.async { repository.load() }
        publish(state, preservingConflicts: true)
        return updated
    }

    @discardableResult
    func importExternal(name: String, directoryURL: URL, mode: SessionMode = .code) async throws -> ProjectSnapshot {
        try await runThrowingOperation(.import, message: String(localized: "project_importing_message")) {
            _ = try self.requireConsistentOwners(projectId: nil)
            let repository = self.repository
            let synchronizer = self.synchronizer
            let bookmarkResolver = self.bookmarkResolver
            let executionDomain = self.executionDomain
            return try await executionDomain.async {
                try bookmarkResolver.withInitialAccess(to: directoryURL) { scopedURL in
                    let bookmark = try bookmarkResolver.makeBookmark(for: scopedURL)
                    _ = try requireCanonicalDirectory(scopedURL)
                    let created = try repository.createImported(name: name, sourceBookmark: bookmark, mode: mode)
                    do {
                        let syncResult = try synchronizer.importInitial(projectId: created.record.id)
                        return StoreSyncResult(
                            project: syncResult.project,
                            state: repository.load(),
                            conflicts: syncResult.conflicts
                        )
                    } catch {
                        _ = try? repository.deleteProject(projectId: created.record.id)
                        throw error
                    }
                }
            }
        } onSuccess: { result in
            self.publish(result.state)
            self.conflicts = result.conflicts
            return result.project
        }
    }

    func addMode(_ mode: SessionMode, projectID: String) async throws {
        _ = try requireConsistentOwners(projectId: projectID)
        guard let project = projects.first(where: { $0.id == projectID }), !project.record.visibleModes.contains(mode) else { return }
        var record = project.record
        record.modeMembership = record.visibleModes.union([mode])
        let updatedRecord = record
        let repository = repository
        let state = try await executionDomain.async {
            _ = try repository.updateProject(updatedRecord)
            return repository.load()
        }
        publish(state, preservingConflicts: true)
    }

    func persistActive(projectId: String?) async throws -> ProjectRepositoryState {
        _ = try requireConsistentOwners(projectId: projectId)
        let repository = repository
        let persisted = try await executionDomain.async {
            try repository.setActiveProject(projectId)
        }
        publish(persisted, preservingConflicts: true)
        return persisted
    }

    func persistActiveForSwitch(projectId: String?) async throws -> ProjectActiveSelectionRollback {
        let rollback = ProjectActiveSelectionRollback(previousProjectId: activeProjectId)
        _ = try await persistActive(projectId: projectId)
        return rollback
    }

    func rollbackActiveSwitch(_ rollback: ProjectActiveSelectionRollback) async throws -> ProjectRepositoryState {
        try await persistActive(projectId: rollback.previousProjectId)
    }

    func publishActive(_ persisted: ProjectRepositoryState) {
        publish(persisted, preservingConflicts: true)
    }

    func recordWorkspace(projectId: String, state: SessionWorkspaceState) async throws {
        let scope = try requireConsistentOwners(projectId: projectId)
        guard allowsSession(state.sessionId, in: scope) else { throw ProjectRepositoryError.conflictingSessionScope }
        let persisted = try await executionDomain.async { [repository] in
            try repository.recordWorkspace(projectId: projectId, state: state)
        }
        publish(persisted, preservingConflicts: true)
    }

    func syncEngineSessions(projectId: String?, rows: [ProjectSessionSummary]) async throws {
        let scope = try requireConsistentOwners(projectId: projectId)
        guard rows.allSatisfy({ allowsSession($0.sessionId, in: scope, mode: $0.mode) }) else {
            throw ProjectRepositoryError.conflictingSessionScope
        }
        let repository = repository
        let next = try await executionDomain.async {
            try repository.updateSessions(projectId: projectId, sessions: rows)
        }
        publish(next, preservingConflicts: true)
    }

    func recordStartedSession(
        projectId: String?,
        sessionId: String,
        title: String,
        mode: SessionMode = .code,
        initialMessageCount: Int = 0,
        workspaceDirectory: String? = nil
    ) async throws {
        let scope = try requireConsistentOwners(projectId: projectId)
        guard allowsSession(sessionId, in: scope, mode: mode) else { throw ProjectRepositoryError.conflictingSessionScope }
        let repository = repository
        let next = try await executionDomain.async {
            try repository.recordStartedSession(
                projectId: projectId,
                sessionId: sessionId,
                title: title,
                mode: mode,
                initialMessageCount: initialMessageCount,
                workspaceDirectory: workspaceDirectory
            )
        }
        publish(next, preservingConflicts: true)
    }

    func setSessionArchived(projectId: String?, sessionId: String, archived: Bool) async throws {
        let scope = try requireConsistentOwners(projectId: projectId)
        guard allowsSession(sessionId, in: scope) else { throw ProjectRepositoryError.conflictingSessionScope }
        if archived { try await onWillArchiveSession?(projectId, sessionId) }
        let repository = repository
        let next = try await executionDomain.async {
            try repository.setSessionArchived(projectId: projectId, sessionId: sessionId, archived: archived)
        }
        publish(next, preservingConflicts: true)
    }

    func reimport(projectId: String) {
        Task { await reimportAsync(projectId: projectId) }
    }

    func reimportAsync(projectId: String) async {
        await runOperation(.reimport, projectId: projectId, message: String(localized: "project_reimporting_message")) {
            _ = try self.requireConsistentOwners(projectId: projectId)
            let repository = self.repository
            let synchronizer = self.synchronizer
            let executionDomain = self.executionDomain
            return try await executionDomain.async {
                let result = try synchronizer.reimport(projectId: projectId)
                return StoreSyncResult(project: result.project, state: repository.load(), conflicts: result.conflicts)
            }
        } onSuccess: { result in
            self.publish(result.state)
            self.conflicts = result.conflicts
        }
    }

    func reauthorize(projectId: String, directoryURL: URL) {
        Task { await reauthorizeAsync(projectId: projectId, directoryURL: directoryURL) }
    }

    func reauthorizeAsync(projectId: String, directoryURL: URL) async {
        await runOperation(.reimport, projectId: projectId, message: String(localized: "project_reauthorizing_message")) {
            _ = try self.requireConsistentOwners(projectId: projectId)
            let repository = self.repository
            let synchronizer = self.synchronizer
            let bookmarkResolver = self.bookmarkResolver
            let executionDomain = self.executionDomain
            return try await executionDomain.async {
                let snapshot = try repository.project(projectId: projectId)
                let originalRecord = snapshot.record
                return try bookmarkResolver.withInitialAccess(to: directoryURL) { scopedURL in
                    let bookmark = try bookmarkResolver.makeBookmark(for: scopedURL)
                    var replacement = originalRecord
                    replacement.sourceBookmark = bookmark
                    replacement.syncState = .changesPending
                    _ = try repository.updateProject(replacement)
                    do {
                        let result = try synchronizer.reimport(projectId: projectId)
                        return StoreSyncResult(project: result.project, state: repository.load(), conflicts: result.conflicts)
                    } catch {
                        _ = try? repository.updateProject(originalRecord)
                        throw error
                    }
                }
            }
        } onSuccess: { result in
            self.publish(result.state)
            self.conflicts = result.conflicts
        }
    }

    func export(projectId: String) {
        Task { await exportAsync(projectId: projectId) }
    }

    func exportAsync(projectId: String) async {
        await runOperation(.export, projectId: projectId, message: String(localized: "project_exporting_message")) {
            _ = try self.requireConsistentOwners(projectId: projectId)
            let repository = self.repository
            let synchronizer = self.synchronizer
            let executionDomain = self.executionDomain
            return try await executionDomain.async {
                let result = try synchronizer.export(projectId: projectId)
                return StoreSyncResult(project: result.project, state: repository.load(), conflicts: result.conflicts)
            }
        } onSuccess: { result in
            self.publish(result.state)
            self.conflicts = result.conflicts
        }
    }

    func resolveConflicts(projectId: String, resolution: ProjectConflictResolution) {
        Task { await resolveConflictsAsync(projectId: projectId, resolution: resolution) }
    }

    func resolveConflictsAsync(projectId: String, resolution: ProjectConflictResolution) async {
        await runOperation(.resolveConflicts, projectId: projectId, message: String(localized: "project_resolving_conflicts_message")) {
            _ = try self.requireConsistentOwners(projectId: projectId)
            let repository = self.repository
            let synchronizer = self.synchronizer
            let executionDomain = self.executionDomain
            return try await executionDomain.async {
                let result = try synchronizer.resolve(projectId: projectId, resolution: resolution)
                return StoreSyncResult(project: result.project, state: repository.load(), conflicts: result.conflicts)
            }
        } onSuccess: { result in
            self.publish(result.state)
            self.conflicts = result.conflicts
        }
    }

    func clearError() {
        errorMessage = nil
    }

    private func runOperation<T>(
        _ kind: ProjectOperationKind,
        projectId: String? = nil,
        message: String,
        work: @escaping @MainActor () async throws -> T,
        onSuccess: @escaping @MainActor (T) -> Void
    ) async {
        operation = ProjectOperation(kind: kind, projectId: projectId, message: message)
        errorMessage = nil
        defer { operation = nil }
        do {
            let result = try await work()
            onSuccess(result)
        } catch {
            publish(executionDomain.sync { repository.load() }, preservingConflicts: true)
            errorMessage = error.localizedDescription
        }
    }

    private func runThrowingOperation<T, Output>(
        _ kind: ProjectOperationKind,
        projectId: String? = nil,
        message: String,
        work: @escaping @MainActor () async throws -> T,
        onSuccess: @escaping @MainActor (T) -> Output
    ) async throws -> Output {
        operation = ProjectOperation(kind: kind, projectId: projectId, message: message)
        errorMessage = nil
        defer { operation = nil }
        do {
            let result = try await work()
            return onSuccess(result)
        } catch {
            publish(executionDomain.sync { repository.load() }, preservingConflicts: true)
            errorMessage = error.localizedDescription
            throw error
        }
    }

    private func publish(_ state: ProjectRepositoryState, preservingConflicts: Bool = false) {
        catalogIsHealthy = state.errorMessage == nil
        catalogErrorMessage = state.errorMessage
        projectIndexPresent = state.projectIndexPresent
        globalIndexPresent = state.globalIndexPresent
        projects = state.projects
        activeProjectId = state.activeProjectId
        globalSessions = state.globalSessions
        loading = state.loading
        errorMessage = state.errorMessage ?? sessionOwnershipError
        if !preservingConflicts {
            conflicts = []
        }
    }
}

private struct StoreCreateResult: Sendable {
    let project: ProjectSnapshot
    let state: ProjectRepositoryState
}

private struct StoreSyncResult: Sendable {
    let project: ProjectSnapshot
    let state: ProjectRepositoryState
    let conflicts: [ProjectSyncConflict]
}

/// Serializes every access to the repository and synchronizer off the main
/// actor. Safety: `queue` is immutable and all closures touching those shared
/// objects execute on this single serial queue.
final class ProjectStoreExecutionDomain: @unchecked Sendable {
    private let queue: DispatchQueue

    init(label: String = "com.lingxi.ios.projects.execution") {
        self.queue = DispatchQueue(label: label, qos: .userInitiated)
    }

    func sync<T>(_ work: () throws -> T) rethrows -> T {
        try queue.sync(execute: work)
    }

    func async<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                do {
                    continuation.resume(returning: try work())
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }
}
