import Foundation
import Observation

struct SessionOwnerEvidence: Equatable, Sendable {
    let scope: ConversationScope
    let mode: SessionMode
    let transcriptPath: String
}

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
    var pendingLocalAppCreations: [LocalAppSourceCreateIntent] = []
    var pendingLocalAppError: String?
    private(set) var localAppIntentRevision: UInt64 = 0
    var onWillArchiveSession: ((String?, String) async throws -> Void)?
    var sessionScopeClaims: (() -> [String: ConversationScope]?)?
    var additionalSessionEvidence: (() throws -> [String: SessionOwnerEvidence])?
    var beginOwnerScopeMutation: (() async throws -> UUID)?
    var finishOwnerScopeMutation: ((UUID) -> Void)?
    var refreshOwnerScopes: (() async throws -> Void)?
    var beginCatalogUpdate: (([String]) async throws -> UUID)?
    var finishCatalogUpdate: ((UUID) -> Void)?
    var validateCatalogRows: (([ProjectSessionSummary], ConversationScope) async throws -> Void)?
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
        let synchronizer = ProjectWorkspaceSynchronizer(repository: repository, configDirName: paths.configDirName)
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

    private func validatedSessionOwnership() throws -> (claims: [String: ConversationScope], evidence: [String: SessionOwnerEvidence]) {
        guard catalogIsHealthy else {
            throw ProjectRepositoryError.invalidSessionOwnership(catalogErrorMessage ?? ProjectRepositoryError.invalidProjectData.localizedDescription)
        }
        if projectIndexPresent, !FileManager.default.fileExists(atPath: repository.projectsRoot.appendingPathComponent(projectIndexFileName).path) {
            throw ProjectRepositoryError.invalidSessionOwnership("projects-index: \(ProjectRepositoryError.missingProjectIndex.localizedDescription)")
        }
        if globalIndexPresent, !FileManager.default.fileExists(atPath: repository.projectsRoot.appendingPathComponent(globalSessionIndexFileName).path) {
            throw ProjectRepositoryError.invalidSessionOwnership("global: \(ProjectRepositoryError.missingSessionIndex.localizedDescription)")
        }
        var claims: [String: ConversationScope]
        if let sessionScopeClaims {
            guard let stored = sessionScopeClaims() else {
                throw ProjectRepositoryError.invalidSessionOwnership("session-scopes: \(ProjectRepositoryError.invalidProjectData.localizedDescription)")
            }
            claims = stored
        } else {
            claims = [:]
        }
        let evidence = try additionalSessionEvidence?() ?? [:]
        for (id, item) in evidence {
                let scope = item.scope
                if let existing = claims[id], existing != scope {
                    throw ProjectRepositoryError.invalidSessionOwnership(
                        "\(id) (\(existing.workspaceKey), \(scope.workspaceKey)): \(ProjectRepositoryError.conflictingSessionScope.localizedDescription)")
                }
                claims[id] = scope
        }
        if let message = Self.claimsError(claims, projects: projects, globalSessions: globalSessions,
                                          projectIndexPresent: projectIndexPresent, globalIndexPresent: globalIndexPresent) {
            throw ProjectRepositoryError.invalidSessionOwnership(message)
        }
        if let message = Self.evidenceError(evidence, projects: projects, globalSessions: globalSessions) {
            throw ProjectRepositoryError.invalidSessionOwnership(message)
        }
        return (claims, evidence)
    }

    private static func evidenceError(_ evidence: [String: SessionOwnerEvidence], projects: [ProjectSnapshot],
                                      globalSessions: [ProjectSessionSummary]) -> String? {
        for row in globalSessions + projects.flatMap(\.sessions) {
            guard let owner = evidence[canonicalSessionID(row.sessionId)] else { continue }
            if owner.mode != row.mode || owner.transcriptPath != row.transcriptPath {
                return "\(row.sessionId) (\(owner.scope.workspaceKey)): \(ProjectRepositoryError.conflictingSessionMode.localizedDescription)"
            }
        }
        return nil
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
        do { _ = try validatedSessionOwnership(); return nil }
        catch { return error.localizedDescription }
    }

    func allowsSession(_ sessionID: String, in scope: ConversationScope, mode: SessionMode? = nil) -> Bool {
        guard let (claims, evidence) = try? validatedSessionOwnership() else { return false }
        if let mode, let owner = evidence[canonicalSessionID(sessionID)], owner.mode != mode { return false }
        return SessionCatalogOwnership.allows(sessionID: sessionID, scope: scope, mode: mode,
            projects: projects, globalSessions: globalSessions, claimedScope: claims[canonicalSessionID(sessionID)])
    }

    /// Re-read every persisted index before starting or claiming a new owner.
    /// Existing admitted owners are handled by SessionHost and keep running.
    private func freshAdmissionState() -> (claims: [String: ConversationScope], evidence: [String: SessionOwnerEvidence], state: ProjectRepositoryState)? {
        let claims: [String: ConversationScope]
        let evidence: [String: SessionOwnerEvidence]
        do { (claims, evidence) = try validatedSessionOwnership() }
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
        if let message = Self.evidenceError(evidence, projects: state.projects, globalSessions: state.globalSessions) {
            errorMessage = message
            return nil
        }
        return (claims, evidence, state)
    }

    func admitsUnclaimedOwner() -> Bool { freshAdmissionState() != nil }

    func admitsNewSessionIdentity(_ sessionID: String) -> Bool {
        guard let (claims, evidence, state) = freshAdmissionState() else { return false }
        let id = canonicalSessionID(sessionID)
        guard UUID(uuidString: id) != nil, claims[id] == nil, evidence[id] == nil else { return false }
        return !(state.globalSessions + state.projects.flatMap(\.sessions))
            .contains { canonicalSessionID($0.sessionId) == id }
    }

    func projectsForOwnerAdmission() throws -> [ProjectSnapshot] {
        guard let (_, _, state) = freshAdmissionState() else {
            throw ProjectRepositoryError.invalidSessionOwnership(errorMessage ?? ProjectRepositoryError.invalidProjectData.localizedDescription)
        }
        return state.projects
    }

    func admitsSessionIdentity(_ sessionID: String, in scope: ConversationScope, mode: SessionMode) -> Bool {
        guard let (claims, evidence, state) = freshAdmissionState() else { return false }
        if let owner = evidence[canonicalSessionID(sessionID)], owner.mode != mode {
            errorMessage = ProjectRepositoryError.conflictingSessionMode.localizedDescription
            return false
        }
        guard SessionCatalogOwnership.allows(sessionID: sessionID, scope: scope, mode: mode,
            projects: state.projects, globalSessions: state.globalSessions,
            claimedScope: claims[canonicalSessionID(sessionID)]) else {
            errorMessage = ProjectRepositoryError.conflictingSessionScope.localizedDescription
            return false
        }
        return true
    }

    func admitsSessionEvidence(_ sessionID: String, owner: SessionOwnerEvidence) -> Bool {
        guard let (claims, evidence, state) = freshAdmissionState() else { return false }
        let id = canonicalSessionID(sessionID)
        guard !id.isEmpty,
              evidence[id].map({ $0 == owner }) ?? true,
              SessionCatalogOwnership.allows(sessionID: id, scope: owner.scope, mode: owner.mode,
                projects: state.projects, globalSessions: state.globalSessions, claimedScope: claims[id]) else {
            errorMessage = ProjectRepositoryError.conflictingSessionScope.localizedDescription
            return false
        }
        for row in state.globalSessions + state.projects.flatMap(\.sessions) where canonicalSessionID(row.sessionId) == id {
            guard row.mode == owner.mode, row.transcriptPath == owner.transcriptPath else {
                errorMessage = ProjectRepositoryError.invalidSessionOwnership("\(id): the session transcript or mode conflicts with its catalog").localizedDescription
                return false
            }
        }
        return true
    }

    /// Validate a reserved scheduler UUID without reading project catalogs on
    /// the main actor. The caller holds that UUID's admission reservation
    /// across this scan and the ensuing receipt publication.
    func validateReservedSessionEvidence(_ sessionID: String, owner: SessionOwnerEvidence,
                                         evidence: [String: SessionOwnerEvidence]) async throws -> Bool {
        let repository = repository
        let state = await Task.detached { repository.admissionSnapshot() }.value
        guard catalogIsHealthy else {
            throw ProjectRepositoryError.invalidSessionOwnership(catalogErrorMessage ?? ProjectRepositoryError.invalidProjectData.localizedDescription)
        }
        if let error = state.errorMessage { throw ProjectRepositoryError.invalidSessionOwnership(error) }
        guard !projectIndexPresent || state.projectIndexPresent else { throw ProjectRepositoryError.missingProjectIndex }
        guard !globalIndexPresent || state.globalIndexPresent else { throw ProjectRepositoryError.missingSessionIndex }
        var claims: [String: ConversationScope]
        if let sessionScopeClaims {
            guard let stored = sessionScopeClaims() else { throw ProjectRepositoryError.invalidProjectData }
            claims = stored
        } else { claims = [:] }
        for (id, item) in evidence {
            if let existing = claims[id], existing != item.scope { throw ProjectRepositoryError.conflictingSessionScope }
            claims[id] = item.scope
        }
        if let message = Self.claimsError(claims, projects: state.projects, globalSessions: state.globalSessions,
                                          projectIndexPresent: state.projectIndexPresent, globalIndexPresent: state.globalIndexPresent) {
            throw ProjectRepositoryError.invalidSessionOwnership(message)
        }
        if let message = Self.evidenceError(evidence, projects: state.projects, globalSessions: state.globalSessions) {
            throw ProjectRepositoryError.invalidSessionOwnership(message)
        }
        let id = canonicalSessionID(sessionID)
        guard !id.isEmpty, evidence[id].map({ $0 == owner }) ?? true,
              SessionCatalogOwnership.allows(sessionID: id, scope: owner.scope, mode: owner.mode,
                projects: state.projects, globalSessions: state.globalSessions, claimedScope: claims[id]) else {
            throw ProjectRepositoryError.conflictingSessionScope
        }
        let rows = state.globalSessions + state.projects.flatMap(\.sessions)
        for row in rows where canonicalSessionID(row.sessionId) == id {
            guard row.mode == owner.mode, row.transcriptPath == owner.transcriptPath else {
                throw ProjectRepositoryError.invalidSessionOwnership("\(id): the session transcript or mode conflicts with its catalog")
            }
        }
        return evidence[id] == owner || rows.contains {
            canonicalSessionID($0.sessionId) == id && $0.mode == owner.mode && $0.transcriptPath == owner.transcriptPath
        }
    }

    /// Resolve from a fresh, validated catalog, not from the active cwd or a
    /// UUID search. The native engine validates the file itself at admission.
    func transcriptPathForSession(_ sessionID: String, in scope: ConversationScope, mode: SessionMode) throws -> String {
        guard let (claims, evidence, state) = freshAdmissionState() else {
            throw ProjectRepositoryError.invalidSessionOwnership(errorMessage ?? ProjectRepositoryError.invalidProjectData.localizedDescription)
        }
        guard SessionCatalogOwnership.allows(sessionID: sessionID, scope: scope, mode: mode,
            projects: state.projects, globalSessions: state.globalSessions,
            claimedScope: claims[canonicalSessionID(sessionID)]) else {
            throw ProjectRepositoryError.conflictingSessionScope
        }
        let rows: [ProjectSessionSummary]
        switch scope {
        case .global:
            rows = state.globalSessions
        case .project(let projectID):
            rows = state.projects.first(where: { $0.id == projectID })?.sessions ?? []
        case .scheduled, .remote:
            throw ProjectRepositoryError.sessionNotIndexed
        }
        let matches = rows.filter { canonicalSessionID($0.sessionId) == canonicalSessionID(sessionID) }
        // A scheduled run can create a project-owned source before its first
        // ordinary session-list refresh. Its durable reservation is already
        // exact owner evidence and may open only that same path and mode.
        if matches.isEmpty, let owner = evidence[canonicalSessionID(sessionID)],
           owner.scope == scope, owner.mode == mode, !owner.transcriptPath.isEmpty,
           (owner.transcriptPath as NSString).isAbsolutePath {
            return owner.transcriptPath
        }
        guard matches.count == 1, let row = matches.first else { throw ProjectRepositoryError.sessionNotIndexed }
        guard row.mode == mode else { throw ProjectRepositoryError.conflictingSessionMode }
        guard let path = row.transcriptPath, !path.isEmpty, (path as NSString).isAbsolutePath else {
            throw ProjectRepositoryError.missingSessionTranscriptPath
        }
        return path
    }

    /// The picker and save operation consult the same fresh catalog. A result
    /// without a unique fixed-mode owner and exact transcript is never offered.
    func cronSessionChoices(mode: SessionMode, generated: [CronGeneratedSession]) -> [CronSessionChoice] {
        guard let (claims, evidence, state) = freshAdmissionState() else { return [] }
        let generatedByID = Dictionary(grouping: generated, by: { canonicalSessionID($0.id) })
        func choice(_ row: ProjectSessionSummary, scope: ConversationScope, scopeID: String, title: String) -> CronSessionChoice? {
            guard !row.isArchived, row.mode == mode,
                  let path = row.transcriptPath, !path.isEmpty, (path as NSString).isAbsolutePath,
                  (generatedByID[canonicalSessionID(row.sessionId)] ?? []).allSatisfy({
                      $0.sessionScopeKey == scope.workspaceKey && $0.sessionMode == mode && $0.sessionTranscriptPath == path
                  }),
                  SessionCatalogOwnership.allows(sessionID: row.sessionId, scope: scope, mode: mode,
                    projects: state.projects, globalSessions: state.globalSessions,
                    claimedScope: claims[canonicalSessionID(row.sessionId)]) else { return nil }
            return CronSessionChoice(id: row.sessionId, title: title, scopeID: scopeID,
                                     owner: scope, mode: mode, transcriptPath: path)
        }
        var choices = state.globalSessions.compactMap {
            choice($0, scope: .global, scopeID: globalCronScopeID, title: $0.title)
        }
        for project in state.projects {
            choices += project.sessions.compactMap {
                choice($0, scope: .project(project.id), scopeID: project.id,
                       title: "\(project.record.name) · \($0.title)")
            }
        }
        choices += generated.compactMap { row in
            guard let key = row.sessionScopeKey, let scope = ConversationScope(workspaceKey: key),
                  scope.projectID == row.projectID, row.sessionMode == mode, let path = row.sessionTranscriptPath,
                  !path.isEmpty, (path as NSString).isAbsolutePath,
                  generatedByID[canonicalSessionID(row.id)]?.count == 1,
                  evidence[canonicalSessionID(row.id)] == SessionOwnerEvidence(scope: scope, mode: mode, transcriptPath: path),
                  !(state.globalSessions + state.projects.flatMap(\.sessions)).contains(where: {
                      canonicalSessionID($0.sessionId) == canonicalSessionID(row.id)
                  }),
                  SessionCatalogOwnership.allows(sessionID: row.id, scope: scope, mode: mode,
                    projects: state.projects, globalSessions: state.globalSessions,
                    claimedScope: claims[canonicalSessionID(row.id)]) else { return nil }
            let project = scope.projectID.flatMap { id in state.projects.first(where: { $0.id == id }) }
            if scope.projectID != nil && project == nil { return nil }
            let title = project.map { "\($0.record.name) · \(row.title)" } ?? row.title
            return CronSessionChoice(id: row.id, title: title, scopeID: project?.id ?? globalCronScopeID,
                                     owner: scope, mode: mode, transcriptPath: path)
        }
        let grouped = Dictionary(grouping: choices, by: { canonicalSessionID($0.id) })
        return choices.filter { grouped[canonicalSessionID($0.id)]?.count == 1 }
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
        _ = try validatedSessionOwnership()
        let scope: ConversationScope = projectId.map { .project($0) } ?? .global
        if let projectId, !projects.contains(where: { $0.id == projectId }) {
            throw ProjectRepositoryError.projectDoesNotExist(projectId)
        }
        return scope
    }

    /// Only snapshots from a currently valid owner catalog can grant the file
    /// browser access to a project's source workspace.
    func fileWorkspaceProjects() -> [ProjectSnapshot] {
        guard !loading, (try? validatedSessionOwnership()) != nil else { return [] }
        return projects
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
        try await withOwnerScopeMutation {
            try await self.runThrowingOperation(.create, message: String(localized: "project_creating_message")) {
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
    }

    func refreshPendingLocalAppCreations() async {
        do {
            let repository = repository
            pendingLocalAppCreations = try await executionDomain.async { try repository.pendingLocalAppSourceCreations() }
            pendingLocalAppError = nil
        } catch {
            pendingLocalAppCreations = []
            pendingLocalAppError = error.localizedDescription
        }
    }

    func storeLocalAppSourceCreateIntent(_ intent: LocalAppSourceCreateIntent) async throws {
        _ = try requireConsistentOwners(projectId: nil)
        let repository = repository
        pendingLocalAppCreations = try await executionDomain.async {
            try repository.storeLocalAppSourceCreateIntent(intent)
            return try repository.pendingLocalAppSourceCreations()
        }
        pendingLocalAppError = nil
    }

    func localAppSourceCreateIntent(requestID: String) async throws -> LocalAppSourceCreateIntent? {
        let repository = repository
        return try await executionDomain.async { try repository.localAppSourceCreateIntent(requestID: requestID) }
    }

    func storeLocalAppRemixRequest(_ intent: LocalAppSourceCreateIntent, input: LocalAppMarketRemixInput,
                                   configHome: URL) async throws {
        _ = try requireConsistentOwners(projectId: nil)
        let repository = repository
        pendingLocalAppCreations = try await executionDomain.async {
            try repository.storeLocalAppRemixRequest(intent, input: input, configHome: configHome)
            return try repository.pendingLocalAppSourceCreations()
        }
        pendingLocalAppError = nil
    }

    func localAppRemixInput(_ intent: LocalAppSourceCreateIntent, configHome: URL)
        async throws -> (LocalAppMarketRemixInput, URL) {
        let repository = repository
        return try await executionDomain.async { try repository.localAppRemixInput(intent, configHome: configHome) }
    }

    func storeLocalAppRemixArchive(_ bytes: Data, intent: LocalAppSourceCreateIntent, configHome: URL) async throws -> URL {
        let repository = repository
        return try await executionDomain.async {
            try repository.storeLocalAppRemixArchive(bytes, intent: intent, configHome: configHome)
        }
    }

    func completeLocalAppSourceCreateIntent(requestID: String, owner: LocalAppAuthoringIntent) async throws {
        let repository = repository
        try await executionDomain.async { try repository.completeLocalAppSourceCreateIntent(requestID: requestID, owner: owner) }
        await refreshPendingLocalAppCreations()
    }

    /// Publish a project together with its source intent and selection before
    /// it can appear in the project list or be used to prepare an engine.
    @discardableResult
    func createLocalAppAuthoringProject(name: String, sourceDirectory: String, sourceID: String, sourceRequestID: String,
                                        appID: String?, projectID: String, intentID: String)
        async throws -> (ProjectSnapshot, LocalAppAuthoringIntent) {
        let result = try await withOwnerScopeMutation {
            try await self.runThrowingOperation(.create, message: String(localized: "project_creating_message")) {
                _ = try self.requireConsistentOwners(projectId: nil)
                let repository = self.repository
                return try await self.executionDomain.async {
                    let (project, intent) = try repository.createLocalAppAuthoringProject(name: name,
                        sourceDirectory: sourceDirectory, sourceID: sourceID, sourceRequestID: sourceRequestID, appID: appID,
                        projectID: projectID, intentID: intentID)
                    return StoreAuthoringCreateResult(project: project, intent: intent, state: repository.load())
                }
            } onSuccess: { result in
                self.publish(result.state, preservingConflicts: true)
                return (result.project, result.intent)
            }
        }
        await refreshPendingLocalAppCreations()
        localAppIntentRevision &+= 1
        return result
    }

    func localAppAuthoringIntents(projectID: String) async throws -> [LocalAppAuthoringIntent] {
        let repository = repository
        return try await executionDomain.async { try repository.localAppAuthoringIntents(projectId: projectID) }
    }

    func storeLocalAppAuthoringIntent(_ intent: LocalAppAuthoringIntent,
                                      replacing previous: LocalAppAuthoringIntent? = nil) async throws {
        let repository = repository
        try await executionDomain.async { try repository.storeLocalAppAuthoringIntent(intent, replacing: previous) }
        localAppIntentRevision &+= 1
    }

    func selectedLocalAppAuthoringIntent(projectID: String) async throws -> LocalAppAuthoringIntent? {
        let repository = repository
        return try await executionDomain.async { try repository.selectedLocalAppAuthoringIntent(projectId: projectID) }
    }

    func selectLocalAppAuthoringIntent(projectID: String, intentID: String,
                                       authority: WorkspaceWriteAuthority, revision: UInt64,
                                       projectAuthority: WorkspaceWriteAuthority, projectRevision: UInt64) async throws {
        let repository = repository
        try await executionDomain.async {
            try repository.selectLocalAppAuthoringIntent(projectId: projectID, intentId: intentID,
                authority: authority, revision: revision, projectAuthority: projectAuthority, projectRevision: projectRevision)
        }
        localAppIntentRevision &+= 1
    }

    @discardableResult
    func importExternal(name: String, directoryURL: URL, mode: SessionMode = .code) async throws -> ProjectSnapshot {
        try await withOwnerScopeMutation {
            try await self.runThrowingOperation(.import, message: String(localized: "project_importing_message")) {
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

    /// Called after querying the retained native owner. The UUID lease fences
    /// other claims while an async queued commit checks its source revision.
    func recordAuthenticatedWorkspace(_ state: SessionWorkspaceState, in scope: ConversationScope,
                                      mode: SessionMode, authority: WorkspaceWriteAuthority,
                                      revision: UInt64) async throws {
        try Task.checkCancellation()
        let lease = try await beginCatalogUpdate?([state.sessionId])
        defer { if let lease { finishCatalogUpdate?(lease) } }
        guard scope == .global || scope.projectID != nil,
              let (claims, evidence, catalog) = freshAdmissionState() else {
            throw ProjectRepositoryError.invalidSessionOwnership(errorMessage ?? ProjectRepositoryError.invalidProjectData.localizedDescription)
        }
        let id = canonicalSessionID(state.sessionId)
        guard SessionCatalogOwnership.allows(sessionID: id, scope: scope, mode: mode,
            projects: catalog.projects, globalSessions: catalog.globalSessions, claimedScope: claims[id]) else {
            throw ProjectRepositoryError.conflictingSessionScope
        }
        if let owner = evidence[id] {
            guard owner == SessionOwnerEvidence(scope: scope, mode: mode, transcriptPath: state.transcriptPath) else {
                throw ProjectRepositoryError.invalidSessionOwnership("\(id): the workspace source conflicts with its durable owner.")
            }
        }
        _ = try await executionDomain.async { [repository] in
            try repository.recordAuthenticatedWorkspace(projectId: scope.projectID, state: state, mode: mode,
                                                         authority: authority, revision: revision)
        }
        // Another queued operation can finish before this continuation gets
        // the main actor. Publish a fresh snapshot, not an older completion.
        publish(repository.admissionSnapshot(), preservingConflicts: true)
    }

    func syncEngineSessions(projectId: String?, rows: [ProjectSessionSummary]) async throws {
        let lease = try await beginCatalogUpdate?(rows.map(\.sessionId))
        defer { if let lease { finishCatalogUpdate?(lease) } }
        let scope = try requireConsistentOwners(projectId: projectId)
        guard rows.allSatisfy({ allowsSession($0.sessionId, in: scope, mode: $0.mode) }) else {
            throw ProjectRepositoryError.conflictingSessionScope
        }
        let evidence = try validatedSessionOwnership().evidence
        let checkedRows = try rows.map { original -> ProjectSessionSummary in
            var row = original
            guard let owner = evidence[canonicalSessionID(row.sessionId)] else { return row }
            guard owner.scope == scope, owner.mode == row.mode,
                  row.transcriptPath.map({ $0.isEmpty || $0 == owner.transcriptPath }) ?? true else {
                throw ProjectRepositoryError.invalidSessionOwnership("\(row.sessionId): the session catalog conflicts with its durable owner.")
            }
            row.transcriptPath = owner.transcriptPath
            return row
        }
        try await validateCatalogRows?(checkedRows, scope)
        let repository = repository
        let next = try await executionDomain.async {
            try repository.updateSessions(projectId: projectId, sessions: checkedRows)
        }
        publish(next, preservingConflicts: true)
    }

    func recordStartedSession(
        projectId: String?,
        sessionId: String,
        title: String,
        mode: SessionMode = .code,
        initialMessageCount: Int = 0,
        workspaceDirectory: String? = nil,
        transcriptPath: String
    ) async throws {
        let scope = try requireConsistentOwners(projectId: projectId)
        guard allowsSession(sessionId, in: scope, mode: mode) else { throw ProjectRepositoryError.conflictingSessionScope }
        let owner = try validatedSessionOwnership().evidence[canonicalSessionID(sessionId)]
        guard !transcriptPath.isEmpty, (transcriptPath as NSString).isAbsolutePath else {
            throw ProjectRepositoryError.missingSessionTranscriptPath
        }
        guard owner.map({ $0.scope == scope && $0.mode == mode && $0.transcriptPath == transcriptPath }) ?? true else {
            throw ProjectRepositoryError.invalidSessionOwnership("\(sessionId): the started session conflicts with its durable owner.")
        }
        let repository = repository
        let next = try await executionDomain.async {
            try repository.recordStartedSession(
                projectId: projectId,
                sessionId: sessionId,
                title: title,
                mode: mode,
                initialMessageCount: initialMessageCount,
                workspaceDirectory: workspaceDirectory,
                authoritativeTranscriptPath: transcriptPath
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

    private func withOwnerScopeMutation<T>(_ work: () async throws -> T) async throws -> T {
        guard let beginOwnerScopeMutation, let finishOwnerScopeMutation else { return try await work() }
        let token = try await beginOwnerScopeMutation()
        defer { finishOwnerScopeMutation(token) }
        let result: Result<T, Error>
        do { result = .success(try await work()) }
        catch { result = .failure(error) }
        // Re-scan even after failure: a partial mutation may have left files
        // behind. A failed scan keeps subsequent owner admission closed.
        try await refreshOwnerScopes?()
        return try result.get()
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

private struct StoreAuthoringCreateResult: Sendable {
    let project: ProjectSnapshot
    let intent: LocalAppAuthoringIntent
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
