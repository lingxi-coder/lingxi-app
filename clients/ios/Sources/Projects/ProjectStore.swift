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

    convenience init(appSandboxRoot: URL) {
        let repository = ProjectRepository(
            projectsRoot: ProjectRepository.appManagedProjectsRoot(appSandboxRoot: appSandboxRoot)
        )
        let synchronizer = ProjectWorkspaceSynchronizer(repository: repository)
        self.init(repository: repository, synchronizer: synchronizer)
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
    func createInternal(name: String) async throws -> ProjectSnapshot {
        try await runThrowingOperation(.create, message: String(localized: "project_creating_message")) {
            let repository = self.repository
            let executionDomain = self.executionDomain
            return try await executionDomain.async {
                let created = try repository.createInternal(name: name)
                return StoreCreateResult(project: created, state: repository.load())
            }
        } onSuccess: { result in
            self.publish(result.state)
            return result.project
        }
    }

    @discardableResult
    func importExternal(name: String, directoryURL: URL) async throws -> ProjectSnapshot {
        try await runThrowingOperation(.import, message: String(localized: "project_importing_message")) {
            let repository = self.repository
            let synchronizer = self.synchronizer
            let bookmarkResolver = self.bookmarkResolver
            let executionDomain = self.executionDomain
            return try await executionDomain.async {
                try bookmarkResolver.withInitialAccess(to: directoryURL) { scopedURL in
                    let bookmark = try bookmarkResolver.makeBookmark(for: scopedURL)
                    _ = try requireCanonicalDirectory(scopedURL)
                    let created = try repository.createImported(name: name, sourceBookmark: bookmark)
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

    func persistActive(projectId: String?) async throws -> ProjectRepositoryState {
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

    func syncEngineSessions(projectId: String?, rows: [ProjectSessionSummary]) async throws {
        let repository = repository
        let next = try await executionDomain.async {
            try repository.updateSessions(projectId: projectId, sessions: rows)
        }
        publish(next, preservingConflicts: true)
    }

    func recordStartedSession(projectId: String?, sessionId: String, title: String) async throws {
        let repository = repository
        let next = try await executionDomain.async {
            try repository.recordStartedSession(projectId: projectId, sessionId: sessionId, title: title)
        }
        publish(next, preservingConflicts: true)
    }

    func markActiveSession(projectId: String, sessionId: String) async throws {
        let repository = repository
        let next = try await executionDomain.async {
            try repository.markActiveSession(projectId: projectId, sessionId: sessionId)
        }
        publish(next, preservingConflicts: true)
    }

    func reimport(projectId: String) {
        Task { await reimportAsync(projectId: projectId) }
    }

    func reimportAsync(projectId: String) async {
        await runOperation(.reimport, projectId: projectId, message: String(localized: "project_reimporting_message")) {
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
        projects = state.projects
        activeProjectId = state.activeProjectId
        globalSessions = state.globalSessions
        loading = state.loading
        errorMessage = state.errorMessage
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
