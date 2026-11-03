import SwiftUI
import UIKit

/// Holds UIKit's finite background-execution assertion while a user-started
/// conversation turn is active. Expiration only releases the assertion: it must
/// never manufacture a user cancellation or corrupt the engine's turn state.
@MainActor
final class ConversationBackgroundExecutionController {
    typealias BackgroundTaskExpirationHandler = @MainActor @Sendable () -> Void
    typealias TaskIdentifier = UIBackgroundTaskIdentifier
    typealias BeginTask = (@escaping BackgroundTaskExpirationHandler) -> TaskIdentifier
    typealias EndTask = (TaskIdentifier) -> Void
    typealias ShouldUseFiniteAssertion = () -> Bool

    private let beginTask: BeginTask
    private let endTask: EndTask
    private let shouldUseFiniteAssertion: ShouldUseFiniteAssertion
    private var taskIdentifier: TaskIdentifier?
    private var generation = 0
    private var expirationObserver: BackgroundTaskExpirationHandler?
    private var turnActive = false
    private var activeTurnID: UInt64?
    private var continuedProcessingLeaseAttached = false

    init(
        beginTask: @escaping BeginTask,
        endTask: @escaping EndTask,
        shouldUseFiniteAssertion: @escaping ShouldUseFiniteAssertion = {
            ConversationBackgroundLeasePolicy.usesFiniteAssertion()
        }
    ) {
        self.beginTask = beginTask
        self.endTask = endTask
        self.shouldUseFiniteAssertion = shouldUseFiniteAssertion
    }

    static func live() -> ConversationBackgroundExecutionController {
        let application = UIApplication.shared
        return ConversationBackgroundExecutionController(
            beginTask: { expirationHandler in
                application.beginBackgroundTask(
                    withName: "ConversationTurn",
                    expirationHandler: expirationHandler
                )
            },
            endTask: { identifier in
                application.endBackgroundTask(identifier)
            }
        )
    }

    func setTurnActive(_ active: Bool, turnID: UInt64? = nil) {
        turnActive = active
        activeTurnID = active ? turnID : nil
        if !active {
            // A subsequent turn must not inherit attachment state from a prior
            // continued-processing request. Its new request will re-establish
            // the state through the lease-change callback.
            continuedProcessingLeaseAttached = false
        }
        guard active,
              !continuedProcessingLeaseAttached,
              shouldUseFiniteAssertion()
        else {
            endCurrentTask()
            return
        }
        guard taskIdentifier == nil else { return }
        generation &+= 1
        let currentGeneration = generation
        let identifier = beginTask { [weak self] in
            self?.expire(generation: currentGeneration)
        }
        // UIKit can refuse a finite assertion and return `.invalid`. Never
        // latch that sentinel: keeping the slot empty lets a later state or
        // foreground transition retry the acquisition.
        guard identifier != .invalid else { return }
        // Be defensive about a test adapter (or future UIKit behaviour)
        // invoking expiration synchronously during acquisition.
        guard generation == currentGeneration else {
            endTask(identifier)
            return
        }
        taskIdentifier = identifier
    }

    /// Supply the actual continued-processing lease state. API availability or
    /// request submission is not enough: only an attached task can replace the
    /// finite UIKit assertion.
    func setContinuedProcessingLeaseAttached(_ attached: Bool, turnID: UInt64? = nil) {
        guard !attached || !turnActive || turnID == nil || activeTurnID == nil || turnID == activeTurnID else {
            return
        }
        continuedProcessingLeaseAttached = attached
        if attached {
            endCurrentTask()
        } else if turnActive {
            setTurnActive(true, turnID: activeTurnID)
        }
    }

    func setExpirationObserver(_ observer: BackgroundTaskExpirationHandler?) {
        expirationObserver = observer
    }

    private func expire(generation expiredGeneration: Int) {
        guard expiredGeneration == generation else { return }
        endCurrentTask()
        expirationObserver?()
    }

    private func endCurrentTask() {
        generation &+= 1
        guard let taskIdentifier else { return }
        self.taskIdentifier = nil
        endTask(taskIdentifier)
    }
}

/// App composition root. Every workspace-sensitive dependency is created from
/// the active project and replaced as one unit when the project or Provider
/// launch configuration changes.
@MainActor
struct RootView: View {
    @Environment(AppState.self) private var app
    @Environment(LocalizationManager.self) private var localization
    @Environment(\.theme) private var theme
    @Environment(\.scenePhase) private var scenePhase

    @State private var settingsStore: SettingsStore
    @State private var navigation: AppNavigationModel
    @State private var projectStore: ProjectStore
    @State private var cronRepository: CronRepository
    @State private var providerRepository: ProviderRepository
    @State private var clientEventCenter: ClientEventCenter
    @State private var source: any ConversationSource
    @State private var sessionHost: SessionHost
    @State private var sourceGeneration = UUID()
    @State private var workspaceWidth: CGFloat = 0
    @State private var sessionInspectorOpen = false
    @State private var sessionDetailsSheetOpen = false
    @State private var workspacePickerOpen = false
    private struct FilesBrowserRequest: Identifiable {
        let id = UUID()
        let source: any ConversationSource
        let sessionID: String
        let path: String
    }
    @State private var filesBrowserRequest: FilesBrowserRequest?
    @State private var liveWorkspaceState: SessionWorkspaceState?
    @State private var workspacePickerSource: (any ConversationSource)?
    @State private var workspacePickerSessionID = ""
    @State private var openTerminalAfterSessionDetails = false
    @State private var selectedPlanDocument: PlanDocument?
    @State private var providerCatalogBootstrapped = false
    /// The workspace the conversation currently runs in: global, the managed
    /// scheduled workspace, or a managed project.
    @State private var activeScope: ConversationScope
    @State private var activeMode: SessionMode
    @State private var activeSession: String
    /// Last session confirmed by SessionStarted/SessionResumed. Drawer taps may
    /// update `activeSession` optimistically, but only this value is persisted.
    @State private var confirmedSession: String
    @State private var pendingSessionRestoreID: String?
    @State private var draftOwnerSessionID: String
    @State private var draft: String
    @State private var hasUnsavedConversationMedia = false
    @State private var hasUnsavedTerminalInput = false
    @State private var voiceInteraction: VoiceInteractionController
    @State private var projectSwitching = false
    @State private var userNavigation = ConversationNavigationIntents<UserConversationNavigation>()
    @State private var localAppSelectionAuthority = WorkspaceWriteAuthority()
    @State private var localAppRegistrationOrder = LocalAppAuthoringRegistrationOrder.shared
    @State private var retiredAuthoringEntriesAwaitingRelease = Set<UUID>()
    @State private var processingSourceIntents = Set<String>()
    private struct LocalAppNavigationContext {
        let generation: UInt64
        let presentationRevision: UInt64
        let selectionRevision: UInt64
        let sequence: UInt64
        let providerSnapshot: ProviderLaunchSnapshot
        let registrationTicket: LocalAppAuthoringRegistrationOrder.Ticket
        let originatingSource: any ConversationSource
        let originatingEntry: SessionHost.Entry?
    }
    private struct PendingAuthoringAttempt {
        let request: LocalAppDevelopmentRequest
        let project: ProjectSnapshot
        var intent: LocalAppAuthoringIntent
        var setCurrentOnFirstAttempt = true
        var registrationTicket: LocalAppAuthoringRegistrationOrder.Ticket?
        var inFlight = false
        var registrationAttempted = false
        var registrationConfirmed = false
        var failureID: UUID?
    }
    @State private var pendingAuthoringAttempts: [UUID: PendingAuthoringAttempt] = [:]
    @State private var allowBlankAuthoringRetryFor: UUID?
    /// `LingxiAppActionStore.drain()` REMOVES what it hands back, so an action
    /// refused by `ConversationSessionMutationPolicy` used to be destroyed by
    /// the very act of reading it. A conversation notification is posted
    /// exactly for `waitingForUser` / `pausedRecoverable` turns, which is
    /// precisely when `hasUnresolvedTurnRecovery` is true and the gate refuses
    /// — so the marquee "tap the notification to reopen the conversation" path
    /// dropped its own action every time. Park refused actions here and retry
    /// them the moment the gate opens.
    @State private var deferredAppActions: [LingxiAppAction] = []
    @State private var externalEngineReady = false
    @State private var externalActionApproval: LingxiAppAction?
    @State private var approvedExternalActions: [LingxiAppAction] = []
    @State private var remoteDestination: RemoteControlDestination?
    @State private var externalImport: CanonicalDeepLink?
    @State private var externalLinkError: String?
    @State private var externalAsyncRouteID = UUID()
    @State private var lastExternalURLAction: (action: LingxiAppAction, receivedAt: Date)?
    @State private var pendingLinkedSessionID: String?
    @State private var pendingMessageLink: (sessionID: String, uuid: UUID)?

    /// A stuck gate plus a notification storm must not grow this without
    /// bound; only the newest few taps are worth replaying.
    private static let maxDeferredAppActions = 8
    @State private var workspacePinnedAt: [String: Date]
    @State private var sessionPinnedAt: [String: Date]
    @State private var cronResultsSeenAt: Date?
    @State private var pendingSessionFork: PendingSessionFork?
    @State private var pendingHandoffDrafts: [SessionHandoffDraft] = []
    @State private var handoffNavigationRevision = UUID()
    @State private var pendingConversationSelectionRestore: PendingConversationSelectionRestore?

    private let appSandboxRoot: String
    @State private var scopedPreferences: ProjectScopedPreferences
    @State private var cronOwnerGate: CronOwnerAdmissionGate
    #if DEBUG
    @State private var uiTestGateLease: UUID?
    @State private var uiTestGateProbe = "ready"
    #endif
    #if canImport(harness_runtimeFFI)
    private let mediaHostComposition: IOSNativeMediaHostComposition?
    private let mediaHostInitializationError: String?
    #endif

    #if canImport(harness_runtimeFFI)
    init(
        voiceCapability: VoiceCapabilityModel,
        mediaHostComposition: IOSNativeMediaHostComposition?,
        mediaHostInitializationError: String?
    ) {
        self.init(
            voiceCapability: voiceCapability,
            mediaHostObject: mediaHostComposition,
            mediaHostInitializationError: mediaHostInitializationError
        )
    }
    #else
    init(voiceCapability: VoiceCapabilityModel) {
        self.init(voiceCapability: voiceCapability, mediaHostObject: nil, mediaHostInitializationError: nil)
    }
    #endif

    private init(
        voiceCapability: VoiceCapabilityModel,
        mediaHostObject: AnyObject?,
        mediaHostInitializationError: String?
    ) {
        #if canImport(harness_runtimeFFI)
        let mediaHostComposition = mediaHostObject as? IOSNativeMediaHostComposition
        self.mediaHostComposition = mediaHostComposition
        self.mediaHostInitializationError = mediaHostInitializationError
        #endif
        let root = ConversationSourceFactory.appSandboxRoot()
        let admissionGate = CronOwnerAdmissionGate()
        _cronOwnerGate = State(initialValue: admissionGate)
        let projects = ProjectStore.forRuntime(managedRoot: LXISHDefaultWorkspace.managedRootPath())
        let settings = SettingsStore()
        let providers = ProviderRepository.shared
        settings.llmProviders = providers.settingsProviders()
        settings.searchProviders = []
        settings.fetchProviders = []

        let snapshot = providers.makeLaunchSnapshot()
        let preferences = ProjectScopedPreferences.forRuntime()
        #if canImport(harness_runtimeFFI)
        let cronStorageRoot = ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] == "1" ? root + "/ui-test-cron" : root
        #else
        let cronStorageRoot = root
        #endif
        let scheduledOwners = CronSessionOwnershipIndex(appSandboxRoot: cronStorageRoot, preferences: preferences)
        projects.sessionScopeClaims = { preferences.sessionScopeClaims }
        projects.additionalSessionEvidence = { try scheduledOwners.claims() }
        if let error = projects.sessionOwnershipError {
            projects.errorMessage = error
        }
        let currentMode = preferences.currentMode()
        let rememberedScope = preferences.lastScope(mode: currentMode) ?? .global
        let initialScope: ConversationScope
        if let projectID = rememberedScope.projectID,
           !projects.projects.contains(where: { $0.id == projectID && $0.record.visibleModes.contains(currentMode) }) {
            initialScope = .global
        } else {
            initialScope = rememberedScope
        }
        projects.activeProjectId = initialScope.projectID
        let activeProject = initialScope.projectID.flatMap { id in projects.projects.first { $0.id == id } }
        var storedSessionID = ConversationModeRestorePolicy.sessionID(
            mode: currentMode,
            scoped: preferences.storedActiveSessionID(scope: initialScope, mode: currentMode),
            sessions: initialScope == .scheduled ? [] : (activeProject?.sessions ?? projects.globalSessions)
        )
        #if DEBUG
            // File UI fixtures own an isolated session; never restore a persisted user session over it.
            if ProcessInfo.processInfo.environment["LINGXI_UI_TEST_FILES"] != nil {
                storedSessionID = ""
            }
        #endif
        if !storedSessionID.isEmpty, !projects.admitsSessionIdentity(storedSessionID, in: initialScope, mode: currentMode) {
            storedSessionID = ""
        }
        let initialMode = currentMode
        let initialProjectMountGrants: [String]
        let initialNeedsAuthoringAuthorization = activeProject.map {
            !MobileUserStorage.localAppSourceDirectories(for: $0).isEmpty
        } ?? false
        if let activeProject, !initialNeedsAuthoringAuthorization {
            initialProjectMountGrants = (try? MobileUserStorage.projectMountGrantPaths(for: activeProject)) ?? []
        } else {
            initialProjectMountGrants = []
        }
        settings.mcpServers = MCPConfigurationRepository.shared.loadServers(
            projectCwd: activeProject?.workspace.hostURL.path
        )
        let runtime = TerminalRuntimeDescriptor.make(
            appSandboxRoot: root,
            project: activeProject,
            linuxRuntime: settings.linuxRuntime
        ).config
        let initialRows = initialScope == .scheduled ? [] : (activeProject?.sessions ?? projects.globalSessions)
        var initialWorkspaceError: Error?
        let initialScheduledCwd: String?
        if initialScope == .scheduled {
            do { initialScheduledCwd = try MobileUserStorage.scheduledWorkspace(sessionID: storedSessionID) }
            catch {
                initialScheduledCwd = nil
                initialWorkspaceError = error
                projects.errorMessage = error.localizedDescription
            }
        } else {
            initialScheduledCwd = nil
        }
        let initialCwd = initialRows.first { canonicalSessionID($0.sessionId) == canonicalSessionID(storedSessionID) }?.workspaceDirectory
            ?? initialScheduledCwd
            ?? activeProject?.record.activeDirectory ?? LXISHGuestPaths.conversationWorkspace(for: initialMode)
        var initialTranscriptPath: String?
        if !storedSessionID.isEmpty, initialWorkspaceError == nil {
            do {
                initialTranscriptPath = try initialScope == .scheduled
                    ? CronRepository.scheduledTranscriptPath(appSandboxRoot: cronStorageRoot, sessionID: storedSessionID, mode: initialMode)
                    : projects.transcriptPathForSession(storedSessionID, in: initialScope, mode: initialMode)
            } catch {
                projects.errorMessage = error.localizedDescription
            }
        }
        var launchOptions = ConversationSourceFactory.LaunchOptions(
            projectCwd: initialCwd,
            additionalWorkspaceDirectories: activeProject?.record.additionalDirectories ?? [],
            sessionMode: initialMode,
            providerConfigured: !snapshot.enabledProfileIDs.isEmpty,
            providerProfilesJson: snapshot.providerProfilesJSON,
            providerRoutingJson: snapshot.routingJSON,
            defaultModelID: snapshot.defaultModelID,
            visionDelegationEnabled: snapshot.visionDelegationEnabled,
            mobileLinux: runtime,
            projectMountGrants: initialProjectMountGrants,
            initialSessionId: storedSessionID.isEmpty ? nil : storedSessionID,
            initialSessionTranscriptPath: initialTranscriptPath
        )
        #if canImport(harness_runtimeFFI)
        launchOptions.mediaHostComposition = mediaHostComposition
        #endif
        var conversation = ConversationSourceFactory.make(options: launchOptions)
        #if canImport(harness_runtimeFFI)
        if initialNeedsAuthoringAuthorization, let engine = conversation as? EngineConversationSource {
            engine.requireAuthoringMountAuthorization()
        }
        #endif
        if let initialWorkspaceError {
            conversation.model.error = ConversationError(kind: .host, message: initialWorkspaceError.localizedDescription)
        }
        #if canImport(harness_runtimeFFI)
        if let mediaHostInitializationError {
            conversation.model.error = ConversationError(kind: .host, message: mediaHostInitializationError)
        }
        #endif

        #if canImport(harness_runtimeFFI)
            let scopes = ProjectCronScopeProvider {
                (projects.projects, projects.activeProjectId)
            }
            let cronStores = FfiCronStoreProvider()
            let validateReservation: @MainActor @Sendable (CronOwnerReservation, CronScope, String) async throws -> CronGeneratedSession = { reservation, scope, title in
                guard let ownerScope = ConversationScope(workspaceKey: reservation.sessionScopeKey),
                      ownerScope.projectID == scope.projectID,
                      !reservation.sessionTranscriptPath.isEmpty else {
                    throw ProjectRepositoryError.conflictingSessionScope
                }
                let owner = SessionOwnerEvidence(scope: ownerScope, mode: reservation.sessionMode,
                                                 transcriptPath: reservation.sessionTranscriptPath)
                let evidence = try await scheduledOwners.claimsOffMain()
                let previouslyKnown = try await projects.validateReservedSessionEvidence(reservation.sessionId, owner: owner, evidence: evidence)
                guard admissionGate.allowsReservation(sessionID: reservation.sessionId, owner: owner,
                                                      previouslyKnown: previouslyKnown, checkPreparing: false) else {
                    throw ProjectRepositoryError.conflictingSessionScope
                }
                return CronGeneratedSession(id: reservation.sessionId, projectID: scope.projectID, title: title,
                    updatedAtMs: UInt64(Date().timeIntervalSince1970 * 1000),
                    sessionTranscriptPath: owner.transcriptPath, sessionMode: owner.mode, sessionScopeKey: owner.scope.workspaceKey)
            }
            let executor = FfiCronExecutor(
                appSandboxRoot: root,
                mediaHostComposition: mediaHostComposition,
                launchSnapshot: { providers.makeLaunchSnapshot() },
                terminalConfig: { scope in
                    let project = scope.projectID.flatMap { id in
                        projects.projects.first(where: { $0.record.id == id })
                    }
                    return TerminalRuntimeDescriptor.make(
                        appSandboxRoot: root,
                        project: project,
                        linuxRuntime: settings.linuxRuntime
                    ).config
                },
                admissionGate: admissionGate,
                prepareOwner: { candidate, scope, task in
                    let id = canonicalSessionID(candidate.sessionId)
                    guard UUID(uuidString: id) != nil else { return false }
                    let config = task.configuration
                    if config.runMode == .selectedSession {
                        guard canonicalSessionID(config.targetSessionId ?? "") == id,
                              let key = config.targetSessionScopeKey, let ownerScope = ConversationScope(workspaceKey: key),
                              ownerScope.projectID == scope.projectID, let mode = config.targetSessionMode,
                              let path = config.targetSessionTranscriptPath, !path.isEmpty else { return false }
                        let owner = SessionOwnerEvidence(scope: ownerScope, mode: mode, transcriptPath: path)
                        guard projects.admitsSessionEvidence(id, owner: owner) else { return false }
                        let indexedPath = ownerScope == .scheduled ? nil : try? projects.transcriptPathForSession(id, in: ownerScope, mode: mode)
                        let known = (try? scheduledOwners.claims()[id]) == owner || indexedPath == path
                        return admissionGate.allowsReservation(sessionID: id, owner: owner, previouslyKnown: known)
                    }
                    guard let key = config.sessionScopeKey, let ownerScope = ConversationScope(workspaceKey: key),
                          ownerScope.projectID == scope.projectID, let mode = config.sessionMode else { return false }
                    if config.runMode == .taskSession, let owned = config.ownedSessionId, !owned.isEmpty {
                        guard canonicalSessionID(owned) == id, let owner = try? scheduledOwners.claims()[id],
                              owner.transcriptPath == candidate.sessionTranscriptPath,
                              owner.scope == ownerScope, owner.mode == mode,
                              projects.admitsSessionEvidence(id, owner: owner) else { return false }
                        return admissionGate.allowsReservation(sessionID: id, owner: owner, previouslyKnown: true)
                    }
                    return projects.admitsNewSessionIdentity(id) && admissionGate.allowsUnclaimedReservation(sessionID: id)
                },
                confirmOwner: { reservation, scope, task in
                    guard cronReservationMatchesTask(reservation, task: task) else {
                        throw ProjectRepositoryError.conflictingSessionScope
                    }
                    let row = try await validateReservation(reservation, scope, String(task.prompt.prefix(200)))
                    try await scheduledOwners.mergeReceipts([row])
                    _ = try await validateReservation(reservation, scope, String(task.prompt.prefix(200)))
                },
                reconcileUnconfirmed: { scope in
                    let store = try await cronStores.storeForAdmission(for: scope, appSandboxRoot: root)
                    let reservations = try await store.listOwnerReservations()
                    var rows: [CronGeneratedSession] = []
                    for reservation in reservations {
                        rows.append(try await validateReservation(reservation, scope, reservation.sessionId))
                    }
                    try await scheduledOwners.mergeReceipts(rows)
                    for reservation in reservations { _ = try await validateReservation(reservation, scope, reservation.sessionId) }
                },
                blockAdmission: { scheduledOwners.blockNewClaims($0) }
            )
            let cron: CronRepository
            if ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] == "1" {
                cron = CronRepository(appSandboxRoot: cronStorageRoot)
            } else {
                cron = CronRepository(
                    appSandboxRoot: root,
                    scopeProvider: scopes,
                    storeProvider: cronStores,
                    executor: executor,
                    notifier: UserNotificationCronNotifier(),
                    scheduler: BestEffortBackgroundCronScheduler()
                )
            }
        #else
            let cron = CronRepository(
                appSandboxRoot: root,
                notifier: UserNotificationCronNotifier(),
                scheduler: BestEffortBackgroundCronScheduler()
            )
        #endif

        cron.startupOwnerIndex = scheduledOwners
        cron.startupOwnerCatalogPreflight = { [weak projects] in
            guard let projects, projects.admitsUnclaimedOwner() else {
                throw ProjectRepositoryError.invalidSessionOwnership(projects?.errorMessage ?? "Session owner catalog unavailable")
            }
        }
        cron.startupReceiptValidator = { [weak projects, weak admissionGate] receipt in
            guard let projects, let key = receipt.sessionScopeKey, let scope = ConversationScope(workspaceKey: key),
                  let mode = receipt.sessionMode, let path = receipt.sessionTranscriptPath else {
                throw ProjectRepositoryError.invalidSessionOwnership(projects?.errorMessage ?? "scheduled: invalid session receipt")
            }
            let owner = SessionOwnerEvidence(scope: scope, mode: mode, transcriptPath: path)
            guard projects.admitsSessionEvidence(receipt.id, owner: owner) else {
                throw ProjectRepositoryError.invalidSessionOwnership(projects.errorMessage ?? "scheduled: invalid session receipt")
            }
            let indexedPath = scope == .scheduled ? nil : try? projects.transcriptPathForSession(receipt.id, in: scope, mode: mode)
            let known = (try? scheduledOwners.claims()[canonicalSessionID(receipt.id)]) == owner || indexedPath == path
            guard admissionGate?.allowsReservation(sessionID: receipt.id, owner: owner, previouslyKnown: known) == true else {
                throw ProjectRepositoryError.invalidSessionOwnership("scheduled: \(receipt.id) conflicts with a live or preparing owner")
            }
        }
        #if canImport(harness_runtimeFFI)
        if ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] != "1" {
            cron.startupOwnerScopes = { [weak projects] in
                guard let projects else { throw ProjectRepositoryError.invalidProjectData }
                return ProjectCronScopeProvider.scopes(appSandboxRoot: root, projects: try projects.projectsForOwnerAdmission())
            }
        }
        #endif

        admissionGate.ownerCatalogIsCurrent = { [weak cron] in cron?.ownerAdmissionIsCurrent ?? false }
        projects.beginOwnerScopeMutation = { [weak admissionGate] in
            guard let admissionGate else { throw ProjectRepositoryError.invalidProjectData }
            return try await admissionGate.beginScopeMutation()
        }
        projects.finishOwnerScopeMutation = { [weak admissionGate] token in admissionGate?.endScopeMutation(token) }
        projects.beginCatalogUpdate = { [weak admissionGate] ids in
            guard let admissionGate else { throw ProjectRepositoryError.invalidProjectData }
            return try await admissionGate.beginCatalogUpdate(sessionIDs: ids)
        }
        projects.finishCatalogUpdate = { [weak admissionGate] token in admissionGate?.finish(token) }
        projects.refreshOwnerScopes = { [weak cron] in
            guard let cron else { throw ProjectRepositoryError.invalidProjectData }
            cron.invalidateOwnerAdmission()
            try await cron.prepareOwnerAdmission()
        }

        projects.onWillArchiveSession = { projectID, sessionID in
            try await cron.pauseTasksForArchivedSession(projectID: projectID, sessionID: sessionID)
        }
        cron.defaultModel = snapshot.defaultModelID
        if preferences.cronResultsSeenAt() == nil {
            preferences.setCronResultsSeenAt(Date())
        }
        _settingsStore = State(initialValue: settings)
        _navigation = State(initialValue: AppNavigationModel())
        _projectStore = State(initialValue: projects)
        _cronRepository = State(initialValue: cron)
        let eventCenter = ClientEventCenter()
        #if canImport(harness_runtimeFFI)
            eventCenter.subscribe { event in providers.handle(event: event) }
        #endif
        _providerRepository = State(initialValue: providers)
        _clientEventCenter = State(initialValue: eventCenter)
        let voiceReadinessOverride: (@MainActor () -> VoiceConfigurationReadiness)?
        #if DEBUG
            if ProcessInfo.processInfo.environment["LINGXI_UI_TEST_VOICE_CONFIGURATION_REQUIRED"] == "1" {
                voiceReadinessOverride = {
                    VoiceConfigurationReadiness(
                        speechConfigured: false,
                        ttsConfigured: false,
                        speechReady: false,
                        ttsReady: false,
                        issues: [
                            VoiceConfigurationIssue(
                                component: .speech,
                                kind: .unconfigured,
                                message: String(localized: "voice_setup_required_default")
                            ),
                        ]
                    )
                }
            } else {
                voiceReadinessOverride = nil
            }
        #else
            voiceReadinessOverride = nil
        #endif
        let sessionHost = SessionHost {
            VoiceInteractionController(
                voiceCapture: VoiceCapture(),
                capability: voiceCapability,
                speechPlayer: IOSAudioService.shared.speechPlayer,
                bargeInRecognizer: IOSAudioService.shared.bargeInRecognizer,
                readinessOverride: voiceReadinessOverride
            )
        }
        projects.validateCatalogRows = { [weak sessionHost] rows, scope in
            #if DEBUG
            if ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] == "1" { return }
            #endif
            guard let sessionHost else { throw ProjectRepositoryError.invalidProjectData }
            for row in rows {
                guard await sessionHost.validatesCatalogSource(sessionID: row.sessionId, scope: scope,
                    mode: row.mode, transcriptPath: row.transcriptPath) else {
                    throw ProjectRepositoryError.invalidSessionOwnership(
                        "\(row.sessionId): the session catalog conflicts with its live owner."
                    )
                }
            }
        }
        admissionGate.sessionHost = sessionHost
        sessionHost.beginSourcePreparation = { [weak admissionGate] source in admissionGate?.beginSourcePreparation(source) ?? false }
        sessionHost.finishSourcePreparation = { [weak admissionGate] source in admissionGate?.endSourcePreparation(source) }
        sessionHost.admitsSessionIdentity = { [weak projects, weak admissionGate, weak cron] id, scope, mode in
            (admissionGate?.allowsNewOwner(sessionID: id) ?? false) &&
                (cron?.ownerAdmissionIsCurrent ?? false) &&
                (projects?.admitsSessionIdentity(id, in: scope, mode: mode) ?? false)
        }
        sessionHost.admitsUnclaimedOwner = { [weak projects] in
            projects?.admitsUnclaimedOwner() ?? false
        }
        sessionHost.startupAdmissionReady = { [weak cron] in
            guard let cron else { return false }
            guard cron.ownerAdmissionIsCurrent else {
                Task { @MainActor [weak cron] in try? await cron?.prepareOwnerAdmission() }
                return false
            }
            return true
        }
        sessionHost.onWorkspaceChanged = { [weak projects, weak sessionHost] entry in
            guard entry.scope == .global || entry.scope.projectID != nil else { return }
            let source = entry.source
            let sessionID = entry.sessionID
            let mode = entry.mode
            Task { @MainActor [weak projects, weak sessionHost, weak entry, weak source] in
                guard let projects, let sessionHost, let entry, let source,
                      sessionHost.entry(for: source) === entry,
                      let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: entry),
                      sessionHost.entry(for: source) === entry,
                      canonicalSessionID(entry.sessionID) == canonicalSessionID(sessionID),
                      canonicalSessionID(source.model.activeSessionId) == canonicalSessionID(sessionID),
                      !source.model.sessionTransitionPending else { return }
                do {
                    let authority = entry.workspaceWriteAuthority
                    try await projects.recordAuthenticatedWorkspace(snapshot.state, in: entry.scope, mode: mode,
                                                                     authority: authority, revision: snapshot.revision)
                } catch is WorkspaceWriteAuthority.Failure {
                    // Re-read the retained native owner after a late revision;
                    // never publish the stale workspace snapshot.
                    sessionHost.refreshWorkspace(entry)
                } catch is CancellationError {
                    return
                } catch {
                    source.model.error = ConversationError(kind: .host, message: error.localizedDescription)
                }
            }
        }
        let initialRetention = sessionHost.retainInitial(
            source: conversation, scope: initialScope, mode: initialMode,
            sessionID: storedSessionID, workspace: initialCwd,
            draft: preferences.draft(scope: initialScope, mode: initialMode, ownerSessionID: storedSessionID)
        ) {
            // The catalog may become unreadable after the first restore check.
            // Keep the saved draft and files intact; the fresh source's first
            // UUID must still pass the normal admission gate.
            storedSessionID = ""
            let fallbackCwd = activeProject?.record.activeDirectory
                ?? (initialScope == .scheduled ? MobileUserStorage.directories().scheduledGuestCwd
                    : LXISHGuestPaths.conversationWorkspace(for: initialMode))
            launchOptions.initialSessionId = nil
            launchOptions.initialSessionTranscriptPath = nil
            launchOptions.projectCwd = fallbackCwd
            conversation = ConversationSourceFactory.make(options: launchOptions)
            #if canImport(harness_runtimeFFI)
            if initialNeedsAuthoringAuthorization, let engine = conversation as? EngineConversationSource {
                engine.requireAuthoringMountAuthorization()
            }
            if let mediaHostInitializationError {
                conversation.model.error = ConversationError(kind: .host, message: mediaHostInitializationError)
            }
            #endif
            return (conversation, fallbackCwd,
                    preferences.draft(scope: initialScope, mode: initialMode, ownerSessionID: ""))
        }
        if let error = initialRetention.rejection, projects.errorMessage == nil {
            projects.errorMessage = String(localized: "project_switch_failed_message \(error.localizedDescription)")
        }
        let initialEntry = initialRetention.entry
        sessionHost.select(initialEntry)
        _source = State(initialValue: conversation)
        initialEntry.draftSourceCodeUnits = preferences.draftSourceCodeUnits(
            scope: initialScope, mode: initialMode, ownerSessionID: storedSessionID)
        initialEntry.handoffReference = preferences.handoffReference(sessionID: storedSessionID, scope: initialScope, mode: initialMode)
        sessionHost.onActivity = { [weak projects, weak sessionHost] entry in
            guard let projects, entry.scope != .scheduled,
                  !entry.source.model.authoringBindingPending,
                  !entry.source.model.sessionTransitionPending, !entry.sessionID.isEmpty,
                  projects.allowsSession(entry.sessionID, in: entry.scope), !entry.isRecordingSession else { return }
            let first = entry.source.model.messages.first(where: { $0.role == .user })
            let indexedRows = entry.scope.projectID.flatMap { projectID in
                projects.projects.first(where: { $0.id == projectID })?.sessions
            } ?? projects.globalSessions
            let indexed = indexedRows.first(where: { canonicalSessionID($0.sessionId) == canonicalSessionID(entry.sessionID) })
            if indexed != nil && (first == nil || (indexed?.messageCount ?? 0) > 0 || entry.recordedFirstMessageID == first?.id) {
                entry.recordedFirstMessageID = first?.id
                return
            }
            entry.isRecordingSession = true
            let ownerID = entry.sessionID
            Task { @MainActor in
                defer { entry.isRecordingSession = false }
                do {
                    guard let sessionHost, sessionHost.entry(for: entry.source) === entry else { return }
                    let startingRevision = entry.workspaceWriteAuthority.currentRevision()
                    guard let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: entry) else {
                        if entry.workspaceWriteAuthority.currentRevision() != startingRevision {
                            Task { @MainActor in sessionHost.sync(entry) }
                        }
                        return
                    }
                    guard
                          sessionHost.entry(for: entry.source) === entry,
                          canonicalSessionID(entry.sessionID) == canonicalSessionID(ownerID),
                          canonicalSessionID(entry.source.model.activeSessionId) == canonicalSessionID(ownerID),
                          !entry.source.model.sessionTransitionPending else { return }
                    guard entry.workspaceWriteAuthority.currentRevision() == snapshot.revision else {
                        Task { @MainActor in sessionHost.sync(entry) }
                        return
                    }
                    let workspace = snapshot.state
                    try await projects.recordStartedSession(projectId: entry.scope.projectID, sessionId: ownerID,
                        title: first.map { String($0.text.prefix(120)) } ?? String(localized: "chat_new_conversation"),
                        mode: entry.mode, initialMessageCount: first == nil ? 0 : 1,
                        workspaceDirectory: workspace.cwd, transcriptPath: workspace.transcriptPath)
                    guard preferences.claimSessionScope(sessionID: ownerID, scope: entry.scope) else {
                        throw ProjectRepositoryError.conflictingSessionScope
                    }
                    entry.recordedFirstMessageID = first?.id
                    preferences.setDraft(entry.draft, scope: entry.scope, mode: entry.mode, ownerSessionID: ownerID)
                    preferences.setDraftSourceCodeUnits(entry.draftSourceCodeUnits, scope: entry.scope,
                                                        mode: entry.mode, ownerSessionID: ownerID)
                    if let reference = entry.handoffReference {
                        preferences.setHandoffReference(reference, sessionID: ownerID, scope: entry.scope, mode: entry.mode)
                    }
                    if sessionHost.selectedEntryID == entry.id,
                       canonicalSessionID(entry.sessionID) == canonicalSessionID(ownerID) {
                        preferences.setActiveSessionID(ownerID, scope: entry.scope, mode: entry.mode)
                    }
                    sessionHost.refreshWorkspace(entry)
                } catch {
                    entry.source.model.error = ConversationError(kind: .host,
                        message: String(localized: "session_index_save_new_failed \(error.localizedDescription)"))
                }
            }
        }
        _sessionHost = State(initialValue: sessionHost)
        _activeScope = State(initialValue: initialScope)
        _activeMode = State(initialValue: initialMode)
        _workspacePinnedAt = State(initialValue: preferences.workspacePinnedAt())
        _sessionPinnedAt = State(initialValue: preferences.sessionPinnedAt())
        _cronResultsSeenAt = State(initialValue: preferences.cronResultsSeenAt())
        _pendingSessionFork = State(initialValue: nil)
        _activeSession = State(initialValue: storedSessionID)
        _confirmedSession = State(initialValue: storedSessionID)
        _draftOwnerSessionID = State(initialValue: storedSessionID)
        _pendingSessionRestoreID = State(initialValue: storedSessionID.isEmpty ? nil : storedSessionID)
        let initialDraft = ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] == "1"
            ? ""
            : preferences.draft(scope: initialScope, mode: initialMode, ownerSessionID: storedSessionID)
        _draft = State(initialValue: initialDraft)
        _voiceInteraction = State(initialValue: initialEntry.voice)
        appSandboxRoot = root
        _scopedPreferences = State(initialValue: preferences)
    }

    private var session: SessionRef {
        let cached = (projectStore.activeProject?.sessions ?? projectStore.globalSessions)
            .first(where: { $0.sessionId == activeSession })
        return source.model.sessionReference(for: activeSession, cachedTitle: cached?.title)
    }

    var body: some View {
        // Split across three declarations on purpose. As one chained
        // expression this body exceeds the Swift type-checker's budget and
        // the build fails outright with "unable to type-check this
        // expression in reasonable time".
        withPresentations(withLifecycleObservers(rootStack))
            .onAppear { source.setSettingsActive(true) }

    }

    private var rootStack: some View {
        @Bindable var navigation = navigation
        return GeometryReader { geometry in
          ZStack {
            AdaptiveConversationSidebar(
                navigation: navigation,
                gesturesEnabled: app.setupDone && navigation.path.isEmpty
            ) {
                sidebar
                    .id(localization.language)
            } detail: {
                NavigationStack(path: $navigation.path) {
                    HStack(spacing: 0) {
                        detailSurface.frame(minWidth: 0, maxWidth: .infinity)
                            .environment(\.openPlanDocument, { document, previous in
                                guard workspaceWidth >= 840 else { return false }
                                if let previous {
                                    if selectedPlanDocument == previous { selectedPlanDocument = document }
                                } else {
                                    selectedPlanDocument = document
                                    sessionInspectorOpen = true
                                }
                                return true
                            })
                            .onChange(of: session.id) { _, _ in selectedPlanDocument = nil }
                        if workspaceWidth >= 840 && sessionInspectorOpen {
                            Divider()
                            VStack(spacing: 0) {
                                HStack {
                                    Text(selectedPlanDocument == nil ? "session_details_title" : "chat_plan_document_title").font(.headline)
                                    Spacer()
                                    Button("common_close", systemImage: "xmark") { sessionInspectorOpen = false }
                                        .labelStyle(.iconOnly)
                                        .accessibilityIdentifier("session-inspector-close")
                                }.padding()
                                if let document = selectedPlanDocument {
                                    PlanDocumentDetail(document: document)
                                } else {
                                SessionDetailsView(session: session, source: source,
                                    workspacePath: currentWorkspaceGuestPath,
                                    onOpenTerminal: openCurrentWorkspaceTerminal)
                                }
                            }
                            .frame(width: min(360, workspaceWidth * 0.35))
                            .background(theme.windowBg)
                        }
                    }
                        .toolbar {
                            if (UIDevice.current.userInterfaceIdiom == .phone || geometry.size.width < 840) && navigation.path.isEmpty {
                                ToolbarItem(placement: .topBarLeading) {
                                    Button(action: navigation.showSidebar) {
                                        Label("drawer_open_sidebar_a11y", systemImage: UIDevice.current.userInterfaceIdiom == .phone
                                              ? "line.3.horizontal" : "sidebar.left")
                                            .labelStyle(.iconOnly)
                                    }
                                    .tint(theme.text)
                                    .accessibilityIdentifier("drawer.open")
                                }
                            }
                        }
                        .navigationDestination(for: AppRoute.self, destination: destination)
                        .id(localization.language)
                }
            }

            .environment(\.horizontalSizeClass, UIDevice.current.userInterfaceIdiom == .pad && geometry.size.width >= 840 ? .regular : .compact)

            // Onboarding owns the whole window, sidebar and navigation bars
            // included, so it sits outside the split view rather than in a column.
            if !app.setupDone {
                SetupWizardView()
                    .zIndex(100)
                    .transition(.opacity)
            }

            #if DEBUG
            if ProcessInfo.processInfo.environment["LINGXI_UI_TEST_GATE_PROBE"] == "1" {
                HStack {
                    Button("Hold") {
                        Task { @MainActor in
                            guard uiTestGateLease == nil,
                                  let lease = try? await cronOwnerGate.beginScopeMutation() else { return }
                            uiTestGateLease = lease
                            uiTestGateProbe = probeHostPreparation() ? "admitted" : "blocked"
                        }
                    }.accessibilityIdentifier("test.gate.hold")
                    Button("Release") {
                        guard let lease = uiTestGateLease else { return }
                        cronOwnerGate.endScopeMutation(lease)
                        uiTestGateLease = nil
                        uiTestGateProbe = probeHostPreparation() ? "admitted" : "blocked"
                    }.accessibilityIdentifier("test.gate.release")
                    Text(uiTestGateProbe).accessibilityIdentifier("test.gate.probe")
                }
                .padding(8)
                .background(.regularMaterial)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                .padding(.top, 90)
                .zIndex(200)
            }
            #endif

            #if canImport(harness_runtimeFFI)
                // Keep one root-mounted native sheet presenter so permission
                // requests work over both the chat and any active SwiftUI sheet.
                if remoteDestination == nil {
                    EnginePermissionPromptHost(
                        model: source.model,
                        onApprove: { source.approvePermission($0, $1) },
                        onDeny: { source.denyPermission($0) }
                    )
                }
            #endif
          }
          .onAppear { workspaceWidth = geometry.size.width }
          .onChange(of: geometry.size.width) { _, width in
              workspaceWidth = width
              if width < 840 && sessionInspectorOpen {
                  sessionInspectorOpen = false
                  navigation.openSessionDetails(sessionID: session.id)
              }
          }
        }
    }

    #if DEBUG
    private func probeHostPreparation() -> Bool {
        let probe = MockConversationSource()
        let admitted = sessionHost.beginSourcePreparation?(probe) ?? false
        if admitted { sessionHost.finishSourcePreparation?(probe) }
        return admitted
    }
    #endif

    private func withLifecycleObservers(_ content: some View) -> some View {
        let backgroundLifecycle = content
            .environment(\.locale, localization.effectiveLocale())
            .onChange(of: scenePhase, handleScenePhase)
            .onChange(of: settingsStore.notifs) { _, config in
                sessionHost.setPreferences(config)
            }
            .onReceive(NotificationCenter.default.publisher(for: .lingxiConversationContinuedProcessingLeaseChanged)) { notification in
                let attached = notification.userInfo?["attached"] as? Bool ?? false
                let identifier = notification.userInfo?[ConversationContinuedProcessingExpiration.identifierKey] as? String
                let turnID = (notification.userInfo?[ConversationContinuedProcessingExpiration.turnIDKey] as? String).flatMap(UInt64.init)
                sessionHost.continuedProcessingLease(attached: attached, identifier: identifier, turnID: turnID)
            }
            .onReceive(NotificationCenter.default.publisher(for: .lingxiConversationContinuedProcessingExpired)) { notification in
                guard let expiration = ConversationContinuedProcessingExpiration(userInfo: notification.userInfo) else { return }
                sessionHost.continuedProcessingExpired(identifier: expiration.identifier, turnID: expiration.turnID)
            }
        let stateLifecycle = backgroundLifecycle
            .onChange(of: draft) { _, value in
                // A queued callback from the outgoing owner must not replace
                // the draft just restored for a different conversation.
                guard value == draft else { return }
                sessionHost.entry(for: source)?.draft = value
                scopedPreferences.setDraft(value, scope: activeScope, mode: activeMode, ownerSessionID: draftOwnerSessionID)
                if !draftOwnerSessionID.isEmpty && !isRestorableSession(draftOwnerSessionID, scope: activeScope, mode: activeMode) {
                    scopedPreferences.setDraft(value, scope: activeScope, mode: activeMode, ownerSessionID: "")
                }
            }
            .onChange(of: providerRepository.syncRevision) { _, _ in
                settingsStore.llmProviders = providerRepository.settingsProviders()
            }
        let presentationLifecycle = stateLifecycle
            .onChange(of: navigation.path) { _, _ in markHandoffNavigationChanged() }
            .onChange(of: navigation.settingsOpen) { _, _ in markHandoffNavigationChanged() }
            .onChange(of: navigation.runningAppID) { _, _ in markHandoffNavigationChanged() }
            .onChange(of: remoteDestination?.id) { _, _ in markHandoffNavigationChanged() }
            .onChange(of: externalEngineReady) { _, ready in if ready { retryDeferredAppActions() } }
            .onChange(of: app.setupDone) { _, ready in if ready { retryDeferredAppActions() } }
            .onChange(of: source.model.sessionTransitionPending) { _, pending in
                if !pending { retryDeferredAppActions(); resolvePendingMessageLink() }
            }
            .onChange(of: source.model.items) { _, _ in resolvePendingMessageLink() }
            .onChange(of: source.model.engineSessionsLoaded) { _, loaded in if loaded { retryDeferredAppActions() } }
            .onChange(of: source.model.engineSessions) { _, _ in retryDeferredAppActions() }

        let notificationLifecycle = presentationLifecycle
            .onReceive(NotificationCenter.default.publisher(for: .lingxiCronNotificationOpened)) { note in
                cronRepository.handleNotificationUserInfo(note.userInfo ?? [:])
                if note.userInfo?["lingxi.route"] as? String == "cron.task" {
                    navigation.openCron(scopeID: note.userInfo?["lingxi.cron.scope_id"] as? String,
                                        taskID: note.userInfo?["lingxi.cron.task_id"] as? String)
                } else if let runID = note.userInfo?["lingxi.cron.run_id"] as? String {
                    navigation.openCronRun(runID)
                }
            }
            .onReceive(NotificationCenter.default.publisher(for: .lingxiAppActionPending)) { _ in
                Task { await consumePendingAppActions() }
            }
            // The three inputs of `ConversationSessionMutationPolicy`. A
            // conversation notification is only ever posted for a turn that
            // makes at least one of them true, so without these the drained
            // action would stay parked forever.
            .onChange(of: source.model.hasUnresolvedTurnRecovery) { _, _ in
                retryDeferredAppActions()
                applyPendingConversationSelectionRestoreIfPossible()
            }
            .onChange(of: source.model.hasInactiveDurableRecovery) { _, _ in
                retryDeferredAppActions()
                applyPendingConversationSelectionRestoreIfPossible()
            }
            .onChange(of: source.model.isCancelling) { _, _ in
                retryDeferredAppActions()
                applyPendingConversationSelectionRestoreIfPossible()
            }
            .onChange(of: projectSwitching) { _, switching in
                guard !switching else { return }
                applyPendingConversationSelectionRestoreIfPossible()
                retryDeferredAppActions()
                resolvePendingMessageLink()
            }
            .onOpenURL(perform: handleIncomingURL)
            .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
                if let url = activity.webpageURL { handleIncomingURL(url) }
            }
        let bootstrapLifecycle = notificationLifecycle
            .task(id: sourceGeneration) {
                externalEngineReady = false
                let generation = sourceGeneration
                let current = source
                let bootstrapDraftRevision = sessionHost.entry(for: current)?.draftRevision
                do {
                    try await cronRepository.prepareOwnerAdmission()
                    let restoreID = sessionHost.entry(for: current)?.sessionID ?? ""
                    try await cronOwnerGate.waitUntilAvailable(sessionID: restoreID)
                    guard generation == sourceGeneration else { return }
                    #if canImport(harness_runtimeFFI)
                    if let engine = current as? EngineConversationSource, engine.needsAuthoringMountAuthorization {
                        guard let entry = sessionHost.entry(for: current),
                              let projectID = entry.scope.projectID,
                              let project = projectStore.projects.first(where: { $0.record.id == projectID }) else {
                            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The app authoring project is unavailable.")
                        }
                        if entry.sessionID.isEmpty {
                            let sources = MobileUserStorage.localAppSourceDirectories(for: project)
                            guard entry.mode == .code,
                                  let intent = try await projectStore.selectedLocalAppAuthoringIntent(projectID: projectID),
                                  sources == [intent.sourceDirectory] else {
                                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app authoring intent is missing or ambiguous.")
                            }
                            let initialRequest = try await projectStore.localAppSourceCreateIntent(requestID: intent.intentId)
                            guard initialRequest == nil || initialRequest?.projectId == projectID else {
                                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app creation request belongs to another project.")
                            }
                            guard generation == sourceGeneration, sessionHost.entry(for: current) === entry,
                                  projectStore.projects.first(where: { $0.record.id == projectID })?.record == project.record else {
                                throw CancellationError()
                            }
                            if let sessionID = intent.authoringSessionId, let transcriptPath = intent.transcriptPath {
                                guard sessionHost.claimIdentity(sessionID, for: current, scope: entry.scope, mode: .code) else {
                                    throw SessionHost.OwnershipError.conflictingOwner
                                }
                                if let bootstrapDraftRevision, entry.draftRevision == bootstrapDraftRevision,
                                   scopedPreferences.hasDraft(scope: entry.scope, mode: .code, ownerSessionID: sessionID) {
                                    let saved = scopedPreferences.draft(scope: entry.scope, mode: .code, ownerSessionID: sessionID)
                                    entry.draft = saved
                                    entry.draftSourceCodeUnits = scopedPreferences.draftSourceCodeUnits(
                                        scope: entry.scope, mode: .code, ownerSessionID: sessionID)
                                    entry.handoffReference = scopedPreferences.handoffReference(
                                        sessionID: sessionID, scope: entry.scope, mode: .code)
                                    if source === entry.source, sessionHost.selectedEntryID == entry.id {
                                        draft = saved
                                        draftOwnerSessionID = sessionID
                                    }
                                }
                                try engine.expectRecoveredAuthoringSession(sessionID: sessionID, transcriptPath: transcriptPath)
                                try await reconcileAuthoringIndexIfNeeded(entry: entry, project: project, intent: intent)
                                guard generation == sourceGeneration, sessionHost.entry(for: current) === entry else {
                                    throw CancellationError()
                                }
                                try validateBootstrapRestore(current)
                                guard let currentProject = projectStore.projects.first(where: { $0.record.id == projectID }) else {
                                    throw LocalAppServiceFailure("AUTHORING_CHANGED", "The app authoring project is unavailable.")
                                }
                                let resolved = try await resolveProjectMountGrants(currentProject, mode: .code, sessionID: sessionID)
                                guard generation == sourceGeneration, sessionHost.entry(for: current) === entry,
                                      let owner = resolved.owner else { throw CancellationError() }
                                try engine.authorizeAuthoringMount(grants: resolved.grants, owner: owner, intent: resolved.missingIntent)
                            } else {
                                guard allowBlankAuthoringRetryFor == entry.id else {
                                    throw LocalAppServiceFailure("AUTHORING_PENDING", "Retry to restore the app source before starting its session.")
                                }
                                _ = try await LocalApps.library.prepareAuthoringSource(sourceID: intent.sourceId,
                                                                                       sourceDirectory: intent.sourceDirectory, mode: intent.mode)
                                let grants = try MobileUserStorage.projectMountGrantPaths(for: project,
                                    authorizedLocalAppSource: intent.sourceDirectory)
                                guard generation == sourceGeneration, sessionHost.entry(for: current) === entry,
                                      projectStore.projects.first(where: { $0.record.id == projectID })?.record == project.record else {
                                    throw CancellationError()
                                }
                                try engine.authorizeProvisionalAuthoringMount(grants: grants,
                                    owner: LocalAppProvisionalAuthoringOwner(sourceDirectory: intent.sourceDirectory,
                                        projectID: intent.projectId, appID: intent.appId))
                                let request = LocalAppDevelopmentRequest(appId: intent.appId, name: project.record.name,
                                    sourceDirectory: intent.sourceDirectory, sourceID: intent.sourceId,
                                    sourceRequestID: intent.sourceRequestId, authoring: nil, prompt: initialRequest?.prompt ?? "")
                                pendingAuthoringAttempts[entry.id] = PendingAuthoringAttempt(
                                    request: request, project: project, intent: intent, setCurrentOnFirstAttempt: false)
                                allowBlankAuthoringRetryFor = nil
                            }
                            if let initialRequest, !initialRequest.completed, let bootstrapDraftRevision,
                               entry.draftRevision == bootstrapDraftRevision,
                               !scopedPreferences.hasDraft(scope: entry.scope, mode: .code, ownerSessionID: entry.sessionID) {
                                seedLocalAppDraft(LocalAppDevelopmentRequest(appId: intent.appId, name: project.record.name,
                                    sourceDirectory: intent.sourceDirectory, sourceID: intent.sourceId,
                                    sourceRequestID: intent.sourceRequestId, authoring: nil, prompt: initialRequest.prompt), entry: entry)
                            }
                        } else {
                            if !projectStore.isSessionIndexed(entry.sessionID, in: entry.scope) {
                                let intent = try await savedAuthoringIntent(project: project, sessionID: entry.sessionID)
                                try await reconcileAuthoringIndexIfNeeded(entry: entry, project: project, intent: intent)
                            }
                            try validateBootstrapRestore(current)
                            guard let currentProject = projectStore.projects.first(where: { $0.record.id == projectID }) else {
                                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The app authoring project is unavailable.")
                            }
                            let resolved = try await resolveProjectMountGrants(currentProject, mode: entry.mode, sessionID: entry.sessionID)
                            guard generation == sourceGeneration, sessionHost.entry(for: current) === entry,
                                  let owner = resolved.owner else { throw CancellationError() }
                            try engine.authorizeAuthoringMount(grants: resolved.grants, owner: owner, intent: resolved.missingIntent)
                        }
                    }
                    #endif
                    try validateBootstrapRestore(current)
                    wireCurrentSource(preserveCatalog: providerCatalogBootstrapped)
                    if !providerCatalogBootstrapped {
                        providerCatalogBootstrapped = true
                        #if DEBUG
                        if ProcessInfo.processInfo.environment["LINGXI_UI_TEST_FILES"] == nil {
                            _ = await providerRepository.refreshCatalog()
                        }
                        #else
                        _ = await providerRepository.refreshCatalog()
                        #endif
                    }
                    while true {
                        try await cronRepository.prepareOwnerAdmission()
                        try await cronOwnerGate.waitUntilAvailable(sessionID: restoreID)
                        guard generation == sourceGeneration else { return }
                        do { try await current.prepare() }
                        catch {
                            sessionHost.endPreparingClaim(of: current)
                            if !Task.isCancelled && (!cronRepository.ownerAdmissionIsCurrent || cronOwnerGate.scopeMutationPending ||
                                !cronOwnerGate.allowsNewOwner(sessionID: restoreID)) { continue }
                            throw error
                        }
                        sessionHost.endPreparingClaim(of: current)
                        break
                    }
                    guard generation == sourceGeneration else { return }
                    #if canImport(harness_runtimeFFI)
                    if let engine = current as? EngineConversationSource,
                       engine.pendingRecoveredAuthoringOwner != nil,
                       let entry = sessionHost.entry(for: current) {
                        try await confirmRecoveredAuthoringOwner(engine, entry: entry)
                        guard generation == sourceGeneration else { return }
                    }
                    #endif
                    current.setSettingsActive(true)
                    if let entry = sessionHost.entry(for: current), pendingAuthoringAttempts[entry.id] != nil {
                        retryAuthoringBinding(for: entry)
                    }
                    current.listSessions()
                    externalEngineReady = true
                    retryDeferredAppActions()
                    await providerRepository.refreshAccounts()
                    #if canImport(harness_runtimeFFI)
                        // 只登记灵犀的插件市场，不安装任何插件；用户在设置 → 插件里自己装。
                        // 记过一次就不再加回来，用户删掉的市场保持删掉。
                        guard generation == sourceGeneration else { return }
                        await LingxiMarketplaceSeeder.shared.ensure(events: clientEventCenter) { command in
                            try await current.submitEngineCommand(command)
                        }
                    #endif
                } catch {
                    guard generation == sourceGeneration else { return }
                    #if canImport(harness_runtimeFFI)
                    if let engine = current as? EngineConversationSource,
                       engine.needsAuthoringMountAuthorization || engine.pendingRecoveredAuthoringOwner != nil {
                        current.model.error = ConversationError(kind: .host, message: error.localizedDescription)
                        current.model.authoringBindingRetryAvailable = true
                    } else if !cronRepository.ownerAdmissionIsCurrent || projectStore.sessionOwnershipError != nil {
                        current.model.error = ConversationError(kind: .host, message: error.localizedDescription)
                    } else {
                        current.warmUp()
                    }
                    #else
                    if !cronRepository.ownerAdmissionIsCurrent || projectStore.sessionOwnershipError != nil {
                        current.model.error = ConversationError(kind: .host, message: error.localizedDescription)
                    } else {
                        current.warmUp()
                    }
                    #endif
                }
            }
        let launchLifecycle = bootstrapLifecycle
            .task {
                do {
                    try await cronRepository.prepareOwnerAdmission()
                    await cronRepository.handleLaunch()
                } catch { projectStore.errorMessage = error.localizedDescription }
            }
        return launchLifecycle
            .task { await LocalApps.initializeOnce() }
            .task {
                await consumePendingAppActions()
                syncConversationBackgroundSurfaces()
            }
    }

    private func withPresentations(_ content: some View) -> some View {
        content
            .fullScreenCover(item: $remoteDestination) { destination in
                #if canImport(harness_runtimeFFI)
                RemoteControlView(
                    destination: destination,
                    openAccount: { navigation.showSettings(.account) },
                    mediaHostComposition: mediaHostComposition,
                    mediaHostInitializationError: mediaHostInitializationError
                )
                    .id(destination.id)
                #endif
            }
            .sheet(item: $externalImport, onDismiss: retryDeferredAppActions) { target in
                DeepLinkImportPreview(target: target)
            }
            .alert(String(localized: "deep_link_open_title"), isPresented: Binding(
                get: { externalActionApproval != nil },
                set: { if !$0 { externalActionApproval = nil } }
            ), presenting: externalActionApproval) { action in
                Button(String(localized: "onboarding_cta_continue")) {
                    externalActionApproval = nil
                    if !approvedExternalActions.contains(action) { approvedExternalActions.append(action) }
                    cronRepository.discardDraft()
                    parkRefusedAppAction(action, applied: applyAppAction(action, approvedDraftChange: true))
                    retryDeferredAppActions()
                }
                Button(String(localized: "deep_link_keep_editing"), role: .cancel) { externalActionApproval = nil; retryDeferredAppActions() }
            } message: { _ in
                Text(String(localized: "deep_link_draft_warning"))
            }
            .alert(String(localized: "deep_link_unable_open"), isPresented: Binding(
                get: { externalLinkError != nil }, set: { if !$0 { externalLinkError = nil } }
            )) { Button(String(localized: "common_ok"), role: .cancel) { externalLinkError = nil } }
            message: { Text(externalLinkError ?? "") }

            .fullScreenCover(
                item: Binding(
                    get: { navigation.runningAppID.map(RunningApp.init) },
                    set: { navigation.runningAppID = $0?.id }
                )
            ) { running in
                Group {
                    AppRuntimeView(
                        library: LocalApps.library,
                        info: LocalApps.info,
                        appId: running.id,
                        onClose: { navigation.runningAppID = nil },
                        onShowDetails: { appID in
                            navigation.runningAppID = nil
                            navigation.openLocalApp(appID)
                        }
                    )
                    .environment(\.theme, theme)
                }
            }
            .fullScreenCover(
                isPresented: Binding(
                    get: { navigation.settingsOpen },
                    set: { if !$0 { navigation.closeSettings() } }
                ),
                onDismiss: handleSettingsDismissed
            ) {
                SettingsHost(
                    store: settingsStore,
                    convo: source.model,
                    projectCwd: projectStore.activeProject?.workspace.hostURL.path,
                    projectStore: projectStore,
                    // 设置页里的「切换项目」必须真的把引擎换过去：项目层/本地层写到
                    // 哪个目录由引擎进程的 cwd 决定，只更新「当前项目」这个记号是不够
                    // 的。关掉设置页再走同一条 `switchScope`，与抽屉里换项目走的是完全
                    // 相同的一条路径，而不是另起一套只在设置页生效的切换逻辑。
                    onSwitchProject: { projectID in
                        navigation.closeSettings()
                        requestScopeNavigation(to: .project(projectID))
                    },
                    onReconnectAfterSecretChange: {
                        #if canImport(harness_runtimeFFI)
                            let hasPendingPermission = !source.model.pendingPermissions.isEmpty
                        #else
                            let hasPendingPermission = false
                        #endif
                        guard !hasPendingPermission, source.model.pendingQuestions.isEmpty,
                              !source.model.streaming, !source.model.sessionTransitionPending,
                              !source.model.isCancelling,
                              !source.model.backgroundTasks.contains(where: { !$0.status.isTerminal }),
                              !source.model.agentSummaries.contains(where: { ["running", "working"].contains($0.status.lowercased()) })
                        else { throw NSError(domain: "LingXi.Settings", code: 1, userInfo: [NSLocalizedDescriptionKey: String(localized: "voice_session_busy_retry")]) }
                        try await rebuildSource(snapshot: providerRepository.makeLaunchSnapshot())
                    },
                    onRefreshMcp: { source.refreshMcpServers() },
                    onRefreshSkills: {
                        #if canImport(harness_runtimeFFI)
                            Task {
                                try? await source.submitEngineCommand(
                                    .refreshListings(which: [.slashCommands])
                                )
                            }
                        #endif
                    },
                    openTerminal: {
                        navigation.closeSettings()
                        openCurrentWorkspaceTerminal()
                    },
                    onPermissionModeChanged: { mode in
                        #if canImport(harness_runtimeFFI)
                            try await source.submitEngineCommand(.setPermissionMode(mode: mode))
                        #endif
                    },
                    onTypescriptLspModeChanged: { mode in
                        #if canImport(harness_runtimeFFI)
                            try await source.submitEngineCommand(.setTypescriptLspMode(mode: mode))
                        #endif
                    },
                    onClose: { navigation.closeSettings() },
                    navigation: navigation
                )
                .presentationDetents([.large])
                .presentationDragIndicator(.visible)
            }
    }

    private var sidebar: some View {
        @Bindable var navigation = navigation
        let account = FirebaseAccount.shared.user.map {
            DrawerAccountSummary(
                displayName: $0.displayName.flatMap { $0.trimmingCharacters(in: .whitespaces).isEmpty ? nil : $0 }
                    ?? $0.email
                    ?? String(localized: "settings_title_account"),
                email: $0.email
            )
        }
        return Drawer(
            projectStore: projectStore,
            activeScope: activeScope,
            activeMode: activeMode,
            activeSession: $activeSession,
            source: source,
            cronState: cronRepository.state,
            filter: $navigation.drawerFilter,
            sessionPinnedAt: sessionPinnedAt,
            statusForSession: { sessionHost.status(sessionID: $0) },
            newCronResults: newCronResultCount,
            account: account,
            openSettings: { navigation.showSettings() },
            openAccount: { navigation.showSettings(.account) },
            openRemoteControl: {
                navigation.closeSidebar()
                remoteDestination = RemoteControlDestination()
            },
            openTerminal: openCurrentWorkspaceTerminal,
            openCron: { scopeID, taskID in navigation.openCron(scopeID: scopeID, taskID: taskID) },
            openProjects: { navigation.openProjects() },
            openProject: { navigation.openProject($0) },
            closeSidebar: { navigation.closeSidebar() },
            onOpenSession: openRecentSession,
            onToggleSessionPinned: toggleSessionPinned,
            onContinueInMode: requestContinueInMode,
            appLibrary: LocalApps.library,
            appInfo: LocalApps.info,
            openApps: { navigation.openApps() },
            openTeams: { navigation.openTeams() },
            openApp: { navigation.runApp($0) }
        )
        .id(sourceGeneration)
    }

    private var detailSurface: some View {
        let owner = sessionHost.entry(for: source)
        return ZStack {
            theme.windowBg.ignoresSafeArea()
            ChatView(
                session: session,
                draft: Binding(
                    get: { owner?.draft ?? draft },
                    set: { value in
                        owner?.draft = value
                        if let owner {
                            scopedPreferences.setDraft(value, scope: owner.scope, mode: owner.mode,
                                                       ownerSessionID: owner.sessionID)
                            if !owner.sessionID.isEmpty && !isRestorableSession(owner.sessionID, scope: owner.scope, mode: owner.mode) {
                                scopedPreferences.setDraft(value, scope: owner.scope, mode: owner.mode, ownerSessionID: "")
                            }
                        }
                        if owner == nil || owner?.source === source { draft = value }
                    }
                ),
                voiceInteraction: voiceInteraction,
                source: source,
                draftRevision: { owner?.draftRevision },
                draftSourceCodeUnits: Binding(
                    get: { owner?.draftSourceCodeUnits },
                    set: { units in
                        guard let owner else { return }
                        owner.draftSourceCodeUnits = units
                        scopedPreferences.setDraft(owner.draft, scope: owner.scope, mode: owner.mode,
                                                   ownerSessionID: owner.sessionID)
                        scopedPreferences.setDraftSourceCodeUnits(units, scope: owner.scope, mode: owner.mode,
                                                                  ownerSessionID: owner.sessionID)
                        if !owner.sessionID.isEmpty && !isRestorableSession(owner.sessionID, scope: owner.scope, mode: owner.mode) {
                            scopedPreferences.setDraft(owner.draft, scope: owner.scope, mode: owner.mode, ownerSessionID: "")
                            scopedPreferences.setDraftSourceCodeUnits(units, scope: owner.scope, mode: owner.mode, ownerSessionID: "")
                        }
                    }
                ),
                attachmentBinding: Binding(
                    get: { owner?.attachment },
                    set: { owner?.attachment = $0 }
                ),
                projectName: projectStore.activeProject?.record.name,
                sessionMode: activeMode,
                workspaceName: currentWorkspaceName,
                workspaceKey: activeScope.workspaceKey,
                voiceEventsManagedByHost: true,
                onDraftMediaChanged: { value in
                    guard let owner, owner.source === source else { return }
                    hasUnsavedConversationMedia = value
                },
                isPinned: !activeSession.isEmpty && sessionPinnedAt[activeSession] != nil,
                canArchive: !activeSession.isEmpty && activeScope != .scheduled,
                onSelectNewChatMode: selectSessionMode,
                onSelectSessionMode: selectSessionMode,
                onPickWorkspace: {
                    workspacePickerSource = source
                    workspacePickerSessionID = source.model.activeSessionId
                    workspacePickerOpen = true
                },
                onContinueInOtherMode: continueCurrentSessionInOtherMode,
                onTogglePinned: { toggleSessionPinned(activeSession) },
                onMakeScheduled: scheduleCurrentSession,
                onOpenTerminal: openCurrentWorkspaceTerminal,
                onOpenFiles: { filesBrowserRequest = .init(source: source, sessionID: source.model.activeSessionId, path: currentWorkspaceGuestPath) },
                onArchive: archiveCurrentSession,
                // A scheduled run's workspace never indexes its sessions, so a
                // chat started there would be missing from the drawer.
                onNewChat: {
                    startNewChat(in: .global, mode: activeMode)
                },
                onOpenVoiceSettings: { navigation.showSettings(.voice) },
                onOpenSessionDetails: {
                    if workspaceWidth >= 840 {
                        selectedPlanDocument = nil
                        sessionInspectorOpen = true
                    } else {
                        sessionDetailsSheetOpen = true
                    }
                },
                onOpenShellTask: { request in
                    navigation.openTerminal(
                        shellRequest: request,
                        projectID: projectStore.activeProjectId,
                        sessionID: request.taskId
                    )
                },
                onRetryAuthoringBinding: { if let owner { retryAuthoringBinding(for: owner) } }
            )
            .environment(\.openLocalApp, { id in
                navigation.runApp(id)
                Task { await LocalApps.library.refresh() }
            })
            .id(sourceGeneration)
            ConversationProjectBridge(
                model: source.model,
                projectStore: projectStore,
                scope: activeScope,
                pendingRestoreID: pendingSessionRestoreID,
                onSessionChanged: { [expectedSource = source, expectedGeneration = sourceGeneration] sessionID in
                    guard source === expectedSource, sourceGeneration == expectedGeneration else { return false }
                    return adoptEngineSession(sessionID)
                },
                onSessionTransitionFailed: { [expectedSource = source, expectedGeneration = sourceGeneration] sessionID in
                    guard source === expectedSource, sourceGeneration == expectedGeneration else { return false }
                    return rollbackFailedSession(sessionID)
                },
                onModeChanged: { [expectedSource = source, expectedGeneration = sourceGeneration] mode in
                    guard source === expectedSource, sourceGeneration == expectedGeneration else { return }
                    adoptSessionMode(mode)
                },
                onRefreshSessions: { [expectedSource = source, expectedGeneration = sourceGeneration] in
                    guard source === expectedSource, sourceGeneration == expectedGeneration else { return }
                    expectedSource.listSessions()
                },
                onWorkspaceChanged: { [expectedSource = source, expectedGeneration = sourceGeneration] workspace in
                    guard source === expectedSource, sourceGeneration == expectedGeneration else { return }
                    liveWorkspaceState = workspace
                }
            )
            .id(sourceGeneration)

            if pendingSessionFork != nil {
                VStack {
                    HStack(spacing: 12) {
                        ProgressView().accessibilityLabel(String(localized: "chat_working"))
                        Button("common_cancel", action: cancelPendingHandoff)
                    }
                    .padding(12)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
                    Spacer()
                }
                .padding(.top, 60)
                .accessibilityIdentifier("conversation.handoff-progress")
                .zIndex(91)
            }

            if let handoff = pendingHandoffDrafts.first {
                VStack {
                    HStack(spacing: 12) {
                        Text("conversation_handoff_ready")
                        Button("common_open") { activatePendingHandoffDraft(handoff) }
                            .disabled(projectSwitching)
                        Button("common_close") { pendingHandoffDrafts.removeAll { $0.id == handoff.id } }
                    }
                    .padding(12)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
                    Spacer()
                }
                .padding(.top, pendingSessionFork == nil ? 60 : 120)
                .accessibilityIdentifier("conversation.handoff-ready")
                .zIndex(91)
            }

            if projectSwitching {
                ProgressView(String(localized: "project_switching_message"))
                    .padding(18)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
                    .zIndex(90)
            }
        }
        .sheet(isPresented: $sessionDetailsSheetOpen, onDismiss: {
            if openTerminalAfterSessionDetails {
                openTerminalAfterSessionDetails = false
                openCurrentWorkspaceTerminal()
            }
        }) {
            NavigationStack {
                SessionDetailsView(
                    session: session,
                    source: source,
                    workspacePath: currentWorkspaceGuestPath,
                    onOpenTerminal: {
                        openTerminalAfterSessionDetails = true
                        sessionDetailsSheetOpen = false
                    }
                )
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("common_close") { sessionDetailsSheetOpen = false }
                            .accessibilityIdentifier("conversation.session-details.close")
                    }
                }
            }
            .environment(\.theme, theme)
            .environment(\.openPlanDocument, nil)
            .presentationDetents([.large])
            .presentationDragIndicator(.visible)
            .accessibilityIdentifier("conversation.session-details-sheet")
        }
        .sheet(item: $filesBrowserRequest) { request in
            let paths = workspaceDirectoryPaths
            WorkspaceFilesView(repository: WorkspaceDirectoryRepository(paths: paths), paths: paths,
                initialPath: request.path, protectedPaths: { workspaceProtectedDirectories },
                canMutate: { request.source === source && request.sessionID == source.model.activeSessionId &&
                    workspaceDirectoryPaths.managedWorkspaces == paths.managedWorkspaces &&
                    !source.model.requiresBackgroundExecution && !source.model.isCancelling && !source.model.sessionTransitionPending })
        }
        .sheet(isPresented: $workspacePickerOpen, onDismiss: {
            workspacePickerSource = nil
            workspacePickerSessionID = ""
        }) {
            let paths = workspaceDirectoryPaths
            let pickerMode = activeMode
            WorkspaceDirectoryPickerSheet(
                paths: paths,
                projects: RecentSessionsBuilder.sortedProjects(
                    ProjectSnapshot.visible(projectStore.projects, in: pickerMode),
                    pinnedAt: workspacePinnedAt),
                activeScope: activeScope,
                currentDirectory: currentWorkspaceGuestPath,
                contextDirectories: source.model.workspaceState?.additionalDirectories
                    ?? projectStore.activeProject?.record.additionalDirectories ?? [],
                protectedPaths: { workspaceProtectedDirectories },
                canMutate: { workspacePickerSource === source && workspacePickerSessionID == source.model.activeSessionId &&
                    workspaceDirectoryPaths.managedWorkspaces == paths.managedWorkspaces &&
                    !source.model.requiresBackgroundExecution && !source.model.isCancelling && !source.model.sessionTransitionPending },
                runtimeStatus: settingsStore.linuxRuntime.rootfsState.label,
                onSelectScope: { scope in
                    guard workspacePickerSource === source, workspacePickerSessionID == source.model.activeSessionId,
                          activeMode == pickerMode else { return }
                    if case let .project(id) = scope,
                       !projectStore.projects.contains(where: { $0.id == id && $0.record.visibleModes.contains(pickerMode) }) {
                        return
                    }
                    startNewChat(in: scope, mode: pickerMode)
                },
                onPickDirectory: { path in
                    guard let pickerSource = workspacePickerSource, pickerSource === source else {
                        throw WorkspaceDirectoryError.sessionChanged
                    }
                    try await pickerSource.setWorkspaceDirectory(path, expectedSessionID: workspacePickerSessionID)
                    guard pickerSource === source else { throw WorkspaceDirectoryError.sessionChanged }
                    liveWorkspaceState = pickerSource.model.workspaceState
                }
            )
            .environment(\.theme, theme)
        }
        .onChange(of: session.id) { _, _ in
            openTerminalAfterSessionDetails = false
            sessionDetailsSheetOpen = false
        }
    }

    /// The workspace's display name for the top bar and the new-chat strip.
    private var currentWorkspaceName: String {
        if case .remote = activeScope { return String(localized: "remote_title") }
        let path = currentWorkspaceGuestPath
        if path == LXISHGuestPaths.home { return "~" }
        if let project = projectStore.activeProject, path == project.record.directory { return project.record.name }
        return (path as NSString).lastPathComponent
    }

    private var workspaceProtectedDirectories: [String] {
        let state = source.model.workspaceState
        return [currentWorkspaceGuestPath, state?.projectRoot].compactMap { $0 }
            + (state?.additionalDirectories ?? [])
            + projectStore.projects.flatMap { project in
                [project.record.directory, project.record.activeDirectory, (project.record.directory as NSString).deletingLastPathComponent]
                    + project.record.additionalDirectories + project.sessions.compactMap(\.workspaceDirectory)
            }
            + projectStore.globalSessions.compactMap(\.workspaceDirectory)
    }

    private var workspaceDirectoryPaths: WorkspaceDirectoryPaths {
        let owner = canonicalSessionID(activeSession)
        let scheduledOwner = activeScope == .scheduled && !owner.isEmpty &&
            canonicalSessionID(source.model.activeSessionId) == owner ? owner : nil
        return .current(mode: activeMode, projects: projectStore.fileWorkspaceProjects(), scheduledSessionID: scheduledOwner)
    }

    private enum UserConversationNavigation {
        case mode(SessionMode)
        case scope(ConversationScope, SessionMode, String?, Bool, String?)
        case recoverAuthoring(String)
    }

    private func requestUserNavigation(_ request: UserConversationNavigation) {
        if case .recoverAuthoring = request, requestLocalAppDevelopmentMode() == nil { return }
        if case let .scope(scope, mode, _, _, _) = request, mode == .chat,
           let projectID = scope.projectID,
           let project = projectStore.projects.first(where: { $0.id == projectID }),
           !MobileUserStorage.localAppSourceDirectories(for: project).isEmpty {
            externalLinkError = String(localized: "local_app_development_requires_code")
            return
        }
        localAppSelectionAuthority.invalidate()
        let projectID: String?
        switch request {
        case let .scope(scope, _, _, _, _): projectID = scope.projectID
        case let .mode(mode): projectID = scopedPreferences.lastScope(mode: mode)?.projectID
        case let .recoverAuthoring(id): projectID = id
        }
        if let projectID { localAppRegistrationOrder.invalidateProjectSelection(projectID: projectID) }
        if userNavigation.submit(request, whileBusy: projectSwitching) {
            performUserNavigation(request)
        } else {
            markHandoffNavigationChanged()
        }
    }

    private func performUserNavigation(_ request: UserConversationNavigation) {
        switch request {
        case let .mode(mode):
            if mode != activeMode { _ = switchMode(to: mode) }
        case let .recoverAuthoring(projectID):
            recoverLocalAppAuthoring(projectID: projectID)
        case let .scope(scope, mode, sessionID, startNew, transcriptPath):
            let generation = userNavigation.generation
            Task { @MainActor in
                do {
                    if !startNew, let sessionID {
                        #if canImport(harness_runtimeFFI)
                        try await drainFailedPreparation(requestID: sessionID)
                        #endif
                        try await drainFailedAuthoringOwner(sessionID: sessionID, scope: scope, mode: mode, generation: generation)
                    }
                    guard userNavigation.isCurrent(generation) else { return }
                    _ = switchScope(to: scope, mode: mode, resumeSessionID: sessionID, startNew: startNew,
                                    selectedTranscriptPath: transcriptPath)
                } catch {
                    if userNavigation.isCurrent(generation) { projectStore.errorMessage = error.localizedDescription }
                }
            }
        }
    }

    private func recoverLocalAppAuthoring(projectID: String) {
        let generation = userNavigation.generation
        Task { @MainActor in
            do {
                guard let intent = try await projectStore.selectedLocalAppAuthoringIntent(projectID: projectID),
                      userNavigation.isCurrent(generation) else { return }
                #if canImport(harness_runtimeFFI)
                try await drainFailedPreparation(requestID: intent.intentId)
                guard userNavigation.isCurrent(generation) else { return }
                #endif
                let scope = ConversationScope.project(projectID)
                if let sessionID = intent.authoringSessionId {
                    #if canImport(harness_runtimeFFI)
                    try await drainFailedPreparation(requestID: sessionID)
                    #endif
                    try await drainFailedAuthoringOwner(sessionID: sessionID, scope: scope, mode: .code, generation: generation)
                }
                guard userNavigation.isCurrent(generation) else { return }
                if intent.authoringSessionId == nil {
                    let matches = pendingAuthoringAttempts.filter { intent.matchesInFlightAttempt($0.value.intent) }
                    guard matches.count <= 1 else { throw SessionHost.OwnershipError.conflictingOwner }
                    if let (entryID, _) = matches.first,
                       let retained = sessionHost.entries.first(where: { $0.id == entryID }) {
                        guard retained.scope == scope, retained.mode == .code,
                              retained.source.model.authoringBindingPending,
                              retained.workspace == intent.sourceDirectory else {
                            throw SessionHost.OwnershipError.sourceIdentityMismatch
                        }
                        if !retained.sessionID.isEmpty {
                            guard let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: retained),
                                  snapshot.state.cwd == intent.sourceDirectory,
                                  canonicalSessionID(snapshot.state.sessionId) == canonicalSessionID(retained.sessionID) else {
                                throw SessionHost.OwnershipError.sourceIdentityMismatch
                            }
                        } else if !retained.source.model.activeSessionId.isEmpty {
                            throw SessionHost.OwnershipError.sourceIdentityMismatch
                        }
                        guard userNavigation.isCurrent(generation), sessionHost.entry(for: retained.source) === retained else {
                            throw CancellationError()
                        }
                        if sessionHost.selectedEntryID == retained.id, source === retained.source {
                            navigation.path.removeAll()
                            navigation.closeSidebar()
                        } else {
                            _ = switchScope(to: scope, mode: .code, resumeSessionID: retained.sessionID, retainedEntry: retained)
                        }
                        return
                    }
                }
                if let sessionID = intent.authoringSessionId,
                   let retained = try sessionHost.entryForClaim(sessionID: sessionID, scope: scope, mode: .code) {
                    // An admitted source may already own the UUID even though
                    // its first catalog write has not finished. Select that
                    // source instead of constructing a competing placeholder.
                    let pendingMatches = pendingAuthoringAttempts[retained.id].map { intent.matchesInFlightAttempt($0.intent) } ?? false
                    if !pendingMatches {
                        let binding = try await LocalApps.library.exactAuthoringBinding(sourceDirectory: intent.sourceDirectory,
                            projectID: intent.projectId, sessionID: sessionID, mode: retained.mode,
                            expectedSourceID: intent.sourceId)
                        let owner = LocalAppAuthoringOwner(sourceDirectory: intent.sourceDirectory, projectID: intent.projectId,
                                                           sessionID: sessionID, appID: intent.appId)
                        guard owner.matches(binding) else { throw SessionHost.OwnershipError.sourceIdentityMismatch }
                    }
                    guard let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: retained),
                          intent.matchesRetainedOwner(scope: retained.scope, mode: retained.mode,
                                                      sessionID: retained.sessionID, workspace: snapshot.state),
                          userNavigation.isCurrent(generation),
                          sessionHost.entry(for: retained.source) === retained else {
                        throw SessionHost.OwnershipError.sourceIdentityMismatch
                    }
                    if !pendingMatches {
                        do {
                            if try await projectStore.localAppSourceCreateIntent(requestID: intent.intentId) != nil {
                                try await projectStore.completeLocalAppSourceCreateIntent(requestID: intent.intentId, owner: intent)
                            }
                        } catch { projectStore.pendingLocalAppError = error.localizedDescription }
                        guard userNavigation.isCurrent(generation), sessionHost.entry(for: retained.source) === retained else {
                            throw CancellationError()
                        }
                    }
                    if sessionHost.selectedEntryID == retained.id, source === retained.source {
                        navigation.path.removeAll()
                        navigation.closeSidebar()
                        return
                    }
                    _ = switchScope(to: scope, mode: .code, resumeSessionID: sessionID,
                                    selectedTranscriptPath: intent.transcriptPath,
                                    expectedAuthoringWorkspace: pendingMatches ? nil : ExistingAuthoringWorkspace(
                                        sessionID: sessionID, sourceDirectory: intent.sourceDirectory,
                                        transcriptPath: intent.transcriptPath ?? ""))
                } else {
                    guard userNavigation.isCurrent(generation) else { return }
                    _ = switchScope(to: scope, mode: .code, recoverAuthoring: true)
                }
            } catch {
                if userNavigation.isCurrent(generation) {
                    projectStore.errorMessage = String(localized: "project_switch_failed_message \(error.localizedDescription)")
                }
            }
        }
    }

    private func drainFailedAuthoringOwner(sessionID: String, scope: ConversationScope,
                                           mode: SessionMode, generation: UInt64) async throws {
        #if canImport(harness_runtimeFFI)
        guard let entry = try sessionHost.entryForClaim(sessionID: sessionID, scope: scope, mode: mode),
              retiredAuthoringEntriesAwaitingRelease.contains(entry.id) else { return }
        guard sessionHost.selectedEntryID != entry.id, let engine = entry.source as? EngineConversationSource,
              engine.isRetiredForReconnect else { throw SessionHost.OwnershipError.conflictingOwner }
        let released = await sessionHost.releaseRetiredAuthoringSourceAfterDrain(engine, entry: entry,
            stillCurrent: { userNavigation.isCurrent(generation) }, waitForRelease: {
                await waitForReleasedOwner(engine, sessionID: sessionID)
            })
        guard released else {
            throw LocalAppServiceFailure("OWNER_RELEASE_PENDING", String(localized: "conversation_reconnect_release_pending"))
        }
        retiredAuthoringEntriesAwaitingRelease.remove(entry.id)
        #endif
    }

    private func resumePendingUserNavigation() {
        guard !projectSwitching, let request = userNavigation.takePending() else { return }
        performUserNavigation(request)
    }

    private func requestScopeNavigation(to scope: ConversationScope, mode: SessionMode? = nil,
                                        resumeSessionID: String? = nil, startNew: Bool = false,
                                        selectedTranscriptPath: String? = nil) {
        requestUserNavigation(.scope(scope, mode ?? activeMode, resumeSessionID, startNew, selectedTranscriptPath))
    }

    /// Start a fresh conversation with the current composer contents.
    private func startNewChat(in scope: ConversationScope, mode: SessionMode) {
        requestScopeNavigation(to: scope, mode: mode, startNew: true)
    }

    private func adoptSessionMode(_ mode: SessionMode) {
        // The mode belongs to the session and can never rewrite another space's slot.
        guard !source.model.sessionTransitionPending, mode == activeMode else { return }
        let owner = source.model.activeSessionId
        if let pendingSessionRestoreID, owner != pendingSessionRestoreID { return }
        if !owner.isEmpty, draftOwnerSessionID != owner {
            scopedPreferences.setDraft(draft, scope: activeScope, mode: activeMode, ownerSessionID: owner)
            scopedPreferences.setDraftSourceCodeUnits(sessionHost.entry(for: source)?.draftSourceCodeUnits,
                                                       scope: activeScope, mode: activeMode, ownerSessionID: owner)
            if draftOwnerSessionID.isEmpty && isRestorableSession(owner, scope: activeScope, mode: mode) {
                scopedPreferences.setDraft("", scope: activeScope, mode: activeMode, ownerSessionID: "")
                scopedPreferences.setDraftSourceCodeUnits(nil, scope: activeScope, mode: activeMode, ownerSessionID: "")
                scopedPreferences.setHandoffReference(nil, sessionID: "", scope: activeScope, mode: activeMode)
            }
            draftOwnerSessionID = owner
        }
        scopedPreferences.setCurrentMode(mode)
        persistActiveSessionIfRestorable(owner.isEmpty ? confirmedSession : owner, scope: activeScope, mode: mode)
    }

    private func selectSessionMode(_ mode: SessionMode) {
        guard projectSwitching || mode != activeMode else { return }
        scopedPreferences.setLastMode(mode)
        requestUserNavigation(.mode(mode))
    }

    private func continueCurrentSessionInOtherMode() {
        let target: SessionMode = activeMode == .chat ? .code : .chat
        let sessionID = confirmedSession.isEmpty ? activeSession : confirmedSession
        if sessionID.isEmpty {
            startNewChat(in: .global, mode: target)
        } else {
            requestContinueInMode(
                scope: activeScope,
                sessionID: sessionID,
                sourceMode: activeMode,
                targetMode: target
            )
        }
    }

    /// "Make it a scheduled task": the first request of this conversation
    /// becomes the new task's instructions.
    private func scheduleCurrentSession() {
        let first = source.model.messages.first { $0.role == .user }?.text ?? ""
        navigation.openCronCreate(prompt: String(first.prefix(500)))
    }

    private func archiveCurrentSession() {
        let sessionID = activeSession
        let projectID = activeScope.projectID
        let generation = userNavigation.generation
        guard !sessionID.isEmpty else { return }
        Task { @MainActor in
            do {
                try await projectStore.setSessionArchived(
                    projectId: projectID,
                    sessionId: sessionID,
                    archived: true
                )
                if userNavigation.isCurrent(generation) {
                    requestScopeNavigation(to: .global, mode: activeMode, startNew: true)
                }
            } catch {
                projectStore.errorMessage = error.localizedDescription
            }
        }
    }

    private func handleSettingsDismissed() {
        defer { retryDeferredAppActions() }
        Task { @MainActor in
            await VoicePreviewPlayback.shared.stop()
            guard !navigation.settingsOpen,
                  navigation.path.isEmpty
            else { return }
            voiceInteraction.reloadConfigurationAndResumeIfPossible()
        }
    }

    @ViewBuilder
    private func destination(_ route: AppRoute) -> some View {
        switch route {
        case .terminal(let sessionID, let initialCommand, let projectID, let requestedCwd):
            let project = terminalProject(for: projectID)
            let descriptor = TerminalRuntimeDescriptor.make(
                appSandboxRoot: appSandboxRoot,
                project: project,
                linuxRuntime: settingsStore.linuxRuntime,
                initialCommand: initialCommand,
                requestedCwd: requestedCwd,
                requestedSessionID: sessionID != "interactive" && requestedCwd == nil ? sessionID : nil
            )
            TerminalView(
                descriptor: descriptor,
                onDismiss: { popRoute() },
                onOpenRuntimeSettings: { popRoute(); navigation.showSettings(.linuxRuntime) },
                onDraftChanged: { hasUnsavedTerminalInput = $0 }
            )
        case .cron(let scopeID, let taskID):
            let route = scopeID.map { CronRoute.task(scopeID: $0, taskID: taskID) }
            CronRootView(repository: cronRepository, initialRoute: route, onDismiss: { popRoute() })
                .onAppear {
                    configureCronEditor()
                    markCronResultsSeen()
                }
                .onChange(of: cronRepository.state.generatedSessions) { _, _ in configureCronEditor() }
        case .cronRun(let runID):
            CronRootView(repository: cronRepository, initialRoute: .run(runID: runID), onDismiss: { popRoute() })
                .onAppear {
                    configureCronEditor()
                    markCronResultsSeen()
                }
                .onChange(of: cronRepository.state.generatedSessions) { _, _ in configureCronEditor() }
        case .cronNew(let prompt):
            CronRootView(
                repository: cronRepository,
                initialRoute: .task(scopeID: activeScope.projectID ?? globalCronScopeID, taskID: nil),
                initialPrompt: prompt,
                onDismiss: { popRoute() }
            )
            .onAppear {
                configureCronEditor()
                markCronResultsSeen()
            }
            .onChange(of: cronRepository.state.generatedSessions) { _, _ in configureCronEditor() }
        case .teams(let invite):
            TeamsPage(initialInvite: invite, onOpenAccount: { navigation.showSettings(.account) })
        case .apps:
            AppsPage(library: LocalApps.library, market: LocalApps.market, projectStore: projectStore,
                onOpenApp: { navigation.runApp($0) }, onShowDetails: { navigation.openLocalApp($0) },
                currentMode: { activeMode },
                captureImportPresentation: { captureLocalAppActionPresentation(route: .apps) },
                onCreate: { name, prompt in try await queueLocalAppCreation(name: name, prompt: prompt) },
                onRecoverCreation: { resumeLocalAppCreation($0) })
        case .localApp(let appID):
            LocalAppDetailPage(library: LocalApps.library, market: LocalApps.market, appId: appID,
                onOpenApp: { navigation.runApp($0) }, onDevelop: continueLocalAppDevelopment,
                onRestore: queueLocalAppRestore, onRemix: queueInstalledLocalAppRemix, currentMode: { activeMode },
                captureActionPresentation: { captureLocalAppActionPresentation(route: .localApp(id: appID)) })
        case .marketApp(let appID):
            MarketInstallPage(library: LocalApps.library, market: LocalApps.market, appId: appID,
                onOpenApp: { navigation.openLocalApp($0) }, currentMode: { activeMode },
                onRemix: queueLocalAppMarketRemix)
        case .projects:
            ProjectsPage(
                projectStore: projectStore,
                workspacePinnedAt: workspacePinnedAt,
                mode: activeMode,
                onOpenProject: { navigation.openProject($0) }
            )
        case .project(let projectID):
            ProjectPage(
                projectStore: projectStore,
                projectID: projectID,
                activeScope: activeScope,
                mode: activeMode,
                source: source,
                cronState: cronRepository.state,
                isPinned: workspacePinnedAt[ConversationScope.project(projectID).workspaceKey] != nil,
                activeSession: activeSession,
                onTogglePinned: { toggleWorkspacePinned(ConversationScope.project(projectID).workspaceKey) },
                onNewSession: { mode in
                    if let mode { scopedPreferences.setLastMode(mode) }
                    requestScopeNavigation(to: .project(projectID), mode: mode ?? scopedPreferences.lastMode(), startNew: true)
                },
                onOpenSession: { mode, sessionID, transcriptPath in
                    openRecentSession(scope: .project(projectID), mode: mode, sessionID: sessionID,
                                      transcriptPath: transcriptPath)
                },
                onOpenCron: { scopeID, taskID in navigation.openCron(scopeID: scopeID, taskID: taskID) },
                onOpenTerminal: {
                    navigation.openTerminal(
                        projectID: projectID,
                        requestedCwd: projectStore.projects.first { $0.id == projectID }
                            .map { .guestPath($0.workspace.guestPath) }
                    )
                },
                onRecoverAuthoring: { requestUserNavigation(.recoverAuthoring(projectID)) }
            )
        case .sessionDetails(let sessionID):
            SessionDetailsView(
                session: session.id == sessionID ? session : SessionRef(id: sessionID, title: session.title),
                source: source,
                workspacePath: currentWorkspaceGuestPath,
                onOpenTerminal: openCurrentWorkspaceTerminal
            )
        }
    }

    /// Explain a refused switch while preserving the source that still owns
    /// background work, durable recovery, or the current cancellation barrier.
    private var scopeSwitchRefusalMessage: String {
        if source.hasLiveSideChats { return String(localized: "side_chat_end_before_restart") }
        if !ConversationSourceReplacementPolicy.allowsReplacement(of: source.model) {
            return String(localized: "chat_error_finish_background_turn_first")
        }
        return ConversationSessionMutationPolicy.allowsCallerMutation(
            hasInactiveDurableRecovery: source.model.hasInactiveDurableRecovery,
            hasUnresolvedTurnRecovery: source.model.hasUnresolvedTurnRecovery,
            isCancelling: source.model.isCancelling
        )
            // The policy is clear, so `projectSwitching` is what refused: a
            // switch is already in flight.
            ? String(localized: "chat_error_stop_before_switch_project")
            : String(localized: "chat_error_finish_background_turn_first")
    }

    private var currentWorkspaceGuestPath: String {
        if let liveWorkspaceState, liveWorkspaceState.sessionId == source.model.activeSessionId {
            return liveWorkspaceState.cwd
        }
        return source.model.workspaceState?.cwd ?? projectStore.activeProject?.workspace.guestPath
            ?? (activeScope == .scheduled ? MobileUserStorage.directories().scheduledGuestCwd
                : LXISHGuestPaths.conversationWorkspace(for: activeMode))
    }

    private func openCurrentWorkspaceTerminal() {
        navigation.openTerminal(
            projectID: projectStore.activeProjectId,
            requestedCwd: .guestPath(currentWorkspaceGuestPath)
        )
    }

    private func popRoute() {
        guard !navigation.path.isEmpty else { return }
        navigation.path.removeLast()
    }

    private func terminalProject(for requestedProjectID: String?) -> ProjectSnapshot? {
        guard let requestedProjectID else { return projectStore.activeProject }
        // An invalid or removed requested project deliberately produces an
        // unavailable workspace. Never fall through to a different project.
        return projectStore.projects.first(where: { $0.record.id == requestedProjectID })
    }

    #if canImport(harness_runtimeFFI)
        private func installExternalEventHandler(on conversation: any ConversationSource) {
            conversation.setExternalEventHandler { [weak conversation] event in
                Task { @MainActor in
                    guard let conversation else { return }
                    clientEventCenter.publish(event, owner: conversation, selectedOwner: source)
                    // Account usage/health belongs to the shared local store,
                    // including a turn that completes in a hidden owner. Its
                    // model and plugin events still belong to that owner alone.
                    if conversation !== source,
                       conversation is EngineConversationSource, source is EngineConversationSource {
                        switch event {
                        case .providerAccountRouting, .turnEnded:
                            providerRepository.handle(event: event)
                        default: break
                        }
                    }
                    handleExternalEvent(event, owner: conversation)
                }
            }
        }

        private func handleExternalEvent(_ event: ClientEvent, owner: any ConversationSource) {
            switch event {
            case let .sessionStarted(sessionId, _):
                triggerPendingForkIfReady(for: sessionId)
            case let .sessionResumed(sessionId, _, _):
                triggerPendingForkIfReady(for: sessionId)
            case let .sessionHandoffPrepared(requestId, sourceSessionId, sourceMode, targetMode, cutoff, summary):
                guard let pending = pendingSessionFork, pending.requestID == requestId,
                      pending.ownerEntryID == sessionHost.entry(for: owner)?.id,
                      canonicalSessionID(pending.sourceSessionID) == canonicalSessionID(sourceSessionId),
                      pending.sourceMode == SessionMode(dto: sourceMode),
                      pending.targetMode == SessionMode(dto: targetMode) else { return }
                pendingSessionFork = nil
                let reference = SessionHandoffReference(sourceScopeKey: pending.sourceScope.workspaceKey,
                    sourceSessionID: sourceSessionId, sourceMode: pending.sourceMode,
                    targetMode: pending.targetMode, cutoff: cutoff, requestID: requestId)
                let handoff = SessionHandoffDraft(mode: pending.targetMode, sourceCodeUnits: summary.codeUnits,
                                                  reference: reference)
                pendingHandoffDrafts.append(handoff)
                if SessionHandoffPresentationPolicy.mayOpenAutomatically(
                    requestRevision: pending.navigationRevision, currentRevision: handoffNavigationRevision,
                    ownerMatches: sessionHost.entry(for: source)?.id == pending.ownerEntryID,
                    conversationVisible: isConversationSurfaceVisible, switching: projectSwitching
                ) {
                    activatePendingHandoffDraft(handoff)
                }
            case let .sessionHandoffFailed(requestId, message):
                guard let pending = pendingSessionFork, pending.requestID == requestId,
                      pending.ownerEntryID == sessionHost.entry(for: owner)?.id else { return }
                pendingSessionFork = nil
                projectStore.errorMessage = message
            default:
                break
            }
        }
    #endif

    private func configureCronEditor() {
        cronRepository.modelChoices = source.model.availableModels
        cronRepository.modelDetails = source.model.availableModelDetails
        let selected = source.model.activeModelId
        cronRepository.defaultModel = selected.isEmpty ? ProviderRepository.shared.makeLaunchSnapshot().defaultModelID : selected
        cronRepository.defaultReasoning = CronReasoning(selection: source.model.reasoningSelection)
        cronRepository.editorMode = activeMode
        let cron = cronRepository
        let projects = projectStore
        cron.sessionChoiceProvider = { [weak cron, weak projects] mode in
            guard let cron, let projects else { return [] }
            guard let generated = cron.generatedSessionsForAdmission() else {
                cron.sessionChoiceFailure = cron.state.errorMessage ?? "scheduled: session index unavailable"
                return []
            }
            guard projects.admitsUnclaimedOwner() else {
                cron.sessionChoiceFailure = projects.errorMessage
                return []
            }
            cron.sessionChoiceFailure = nil
            return projects.cronSessionChoices(mode: mode, generated: generated)
        }
        cronRepository.onOpenSession = { run in
            guard let id = run.sessionID, let key = run.sessionScopeKey,
                  let scope = ConversationScope(workspaceKey: key), scope.projectID == run.projectID,
                  let mode = run.sessionMode, let path = run.sessionTranscriptPath else {
                projectStore.errorMessage = ProjectRepositoryError.missingSessionTranscriptPath.localizedDescription
                return
            }
            let currentPath: String?
            if scope == .scheduled {
                currentPath = try? cronRepository.scheduledTranscriptPath(sessionID: id, mode: mode)
            } else {
                currentPath = try? projectStore.transcriptPathForSession(id, in: scope, mode: mode)
            }
            guard currentPath == path, projectStore.admitsSessionIdentity(id, in: scope, mode: mode) else {
                projectStore.errorMessage = projectStore.errorMessage ?? ProjectRepositoryError.conflictingSessionScope.localizedDescription
                return
            }
            requestScopeNavigation(to: scope, mode: mode, resumeSessionID: id, selectedTranscriptPath: path)
        }
    }

    private func validateBootstrapRestore(_ current: any ConversationSource) throws {
        #if canImport(harness_runtimeFFI)
        guard let engine = current as? EngineConversationSource, engine.needsInitialRestoreValidation,
              let entry = sessionHost.entry(for: current), !entry.sessionID.isEmpty else { return }
        guard cronOwnerGate.allowsNewOwner(sessionID: entry.sessionID) else {
            throw ProjectRepositoryError.invalidSessionOwnership("A scheduled conversation is being prepared. Try again.")
        }
        guard projectStore.admitsSessionIdentity(entry.sessionID, in: entry.scope, mode: entry.mode) else {
            throw ProjectRepositoryError.invalidSessionOwnership(projectStore.errorMessage ?? ProjectRepositoryError.conflictingSessionScope.localizedDescription)
        }
        let path = try entry.scope == .scheduled
            ? cronRepository.scheduledTranscriptPath(sessionID: entry.sessionID, mode: entry.mode)
            : projectStore.transcriptPathForSession(entry.sessionID, in: entry.scope, mode: entry.mode)
        try engine.validateInitialRestorePath(path)
        if projectStore.isSessionIndexed(entry.sessionID, in: entry.scope),
           !scopedPreferences.claimSessionScope(sessionID: entry.sessionID, scope: entry.scope) {
            throw ProjectRepositoryError.conflictingSessionScope
        }
        #endif
    }

    private func wireCurrentSource(preserveCatalog: Bool = false) {
        let current = source
        let generation = sourceGeneration
        #if canImport(harness_runtimeFFI)
            installExternalEventHandler(on: current)
            // Startup events can precede selection. Seed the global view from
            // the selected owner's already-projected model before later events.
            clientEventCenter.publish(.modelList(models: current.model.availableModels,
                current: current.model.activeModelId, currentModelSource: nil, details: []),
                owner: current, selectedOwner: source)
            ProviderAccountRepository.shared.configure(load: {
                guard source === current, generation == sourceGeneration else { throw CancellationError() }
                return try await current.providerAccountsSnapshot()
            }, command: { request, credential in
                // Teardown cancels only the captured Host's exact grant even
                // after navigation has installed a replacement source.
                let action = (try? JSONSerialization.jsonObject(with: Data(request.utf8))) as? [String: Any]
                if action?["action"] as? String == "cancel_oauth" {
                    return try await current.providerAccountCommand(requestJSON: request, credential: nil)
                }
                guard source === current, generation == sourceGeneration else { throw CancellationError() }
                return try await current.providerAccountCommand(requestJSON: request, credential: credential)
            }, oauthSupported: !(current is RemoteConversationSource),
               localStoreID: current is EngineConversationSource ? MobileUserStorage.directories().configHome : nil)
            ProviderAccountRepository.shared.setForeground(scenePhase == .active)
            providerRepository.configure(
                submitCommand: { command in
                    guard source === current, generation == sourceGeneration else { throw CancellationError() }
                    try await current.submitEngineCommand(command)
                },
                applyReconnect: { snapshot in
                    try await rebuildSource(snapshot: snapshot)
                },
                providerCatalog: {
                    try await current.providerCatalog()
                },
                resetCatalog: !preserveCatalog
            )
        #else
            ProviderAccountRepository.shared.configure(load: nil, command: nil)
            providerRepository.configure(submitCommand: nil)
        #endif
    }

    private struct ProvisionalLocalAppGrant {
        let intent: LocalAppAuthoringIntent
    }

    private struct ExistingAuthoringWorkspace {
        let sessionID: String
        let sourceDirectory: String
        let transcriptPath: String

        func matches(_ state: SessionWorkspaceState) -> Bool {
            canonicalSessionID(state.sessionId) == canonicalSessionID(sessionID)
                && state.cwd == sourceDirectory && state.transcriptPath == transcriptPath
        }
    }

    private func savedAuthoringIntent(project: ProjectSnapshot, sessionID: String) async throws -> LocalAppAuthoringIntent {
        guard let intent = try await savedAuthoringIntentIfPresent(project: project, sessionID: sessionID) else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app authoring intent is missing or ambiguous.")
        }
        return intent
    }

    private func savedAuthoringIntentIfPresent(project: ProjectSnapshot, sessionID: String) async throws -> LocalAppAuthoringIntent? {
        let intents = try await projectStore.localAppAuthoringIntents(projectID: project.record.id)
        return try LocalAppAuthoringIntent.recoveryCandidate(in: intents, projectID: project.record.id,
            sessionID: sessionID, sourceDirectories: MobileUserStorage.localAppSourceDirectories(for: project))
    }

    private func reconcileAuthoringIndexIfNeeded(entry: SessionHost.Entry, project: ProjectSnapshot,
                                                 intent: LocalAppAuthoringIntent) async throws {
        guard entry.mode == .code, entry.scope == .project(project.record.id),
              let sessionID = intent.authoringSessionId, let transcriptPath = intent.transcriptPath,
              canonicalSessionID(entry.sessionID) == sessionID,
              sessionHost.entry(for: entry.source) === entry else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app session conflicts with an existing owner.")
        }
        try await reconcileAuthoringIndexIfNeeded(project: project, intent: intent)
        guard sessionHost.entry(for: entry.source) === entry,
              canonicalSessionID(entry.sessionID) == sessionID,
              try projectStore.transcriptPathForSession(sessionID, in: entry.scope, mode: .code) == transcriptPath else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app owner changed while its index was restored.")
        }
    }

    /// The persisted intent contains the UUID and transcript observed from
    /// the original native handle. Publishing this exact index row does not
    /// authorize a turn: the reopened engine still validates the transcript
    /// and obtains its immutable authoring receipt before the gate opens.
    private func reconcileAuthoringIndexIfNeeded(project: ProjectSnapshot,
                                                 intent: LocalAppAuthoringIntent) async throws {
        let scope = ConversationScope.project(project.record.id)
        guard intent.projectId == project.record.id, intent.mode == .code,
              let sessionID = intent.authoringSessionId, UUID(uuidString: sessionID) != nil,
              let transcriptPath = intent.transcriptPath, !transcriptPath.isEmpty,
              (transcriptPath as NSString).isAbsolutePath,
              cronOwnerGate.allowsNewOwner(sessionID: sessionID),
              projectStore.admitsSessionEvidence(sessionID, owner: SessionOwnerEvidence(
                scope: scope, mode: .code, transcriptPath: transcriptPath)) else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app session conflicts with an existing owner.")
        }
        if !projectStore.isSessionIndexed(sessionID, in: scope) {
            try await projectStore.recordStartedSession(projectId: project.record.id, sessionId: sessionID,
                title: project.record.name, mode: .code, workspaceDirectory: intent.sourceDirectory,
                transcriptPath: transcriptPath)
        }
        guard try projectStore.transcriptPathForSession(sessionID, in: scope, mode: .code) == transcriptPath else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app transcript conflicts with its owner.")
        }
    }

    private func resolveProjectMountGrants(
        _ project: ProjectSnapshot, mode: SessionMode, sessionID: String?,
        provisional: ProvisionalLocalAppGrant? = nil
    ) async throws -> (grants: [String], owner: LocalAppAuthoringOwner?, missingIntent: LocalAppAuthoringIntent?) {
        let sources = MobileUserStorage.localAppSourceDirectories(for: project)
        guard !sources.isEmpty else {
            return (try MobileUserStorage.projectMountGrantPaths(for: project), nil, nil)
        }
        guard sources.count == 1, let appSource = sources.first, mode == .code else {
            throw LocalAppServiceFailure("INVALID_AUTHORING", "The app authoring project has an ambiguous source or mode.")
        }
        if let provisional {
            guard sessionID == nil, provisional.intent.projectId == project.record.id,
                  provisional.intent.sourceDirectory == appSource,
                  provisional.intent.authoringSessionId == nil else {
                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The app authoring request changed before it was opened.")
            }
            return (try MobileUserStorage.projectMountGrantPaths(for: project, authorizedLocalAppSource: appSource), nil, nil)
        }
        guard let sessionID, UUID(uuidString: sessionID) != nil else {
            throw LocalAppServiceFailure("AUTHORING_PENDING", "The app authoring session has no confirmed owner.")
        }
        let owner: LocalAppAuthoringOwner
        let savedIntent = try await savedAuthoringIntentIfPresent(project: project, sessionID: sessionID)
        if let savedIntent {
            guard savedIntent.sourceDirectory == appSource,
                  try projectStore.transcriptPathForSession(sessionID, in: .project(project.record.id), mode: .code)
                    == savedIntent.transcriptPath else {
                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app transcript conflicts with its owner.")
            }
        }
        do {
            let binding = try await LocalApps.library.exactAuthoringBinding(
                sourceDirectory: appSource, projectID: project.record.id, sessionID: sessionID, mode: mode,
                expectedSourceID: savedIntent?.sourceId)
            let matchedOwner = LocalAppAuthoringOwner(sourceDirectory: appSource, projectID: project.record.id,
                                                      sessionID: sessionID, appID: binding.appId)
            guard matchedOwner.matches(binding) else {
                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app binding no longer matches this owner.")
            }
            owner = matchedOwner
        } catch let failure as LocalAppServiceFailure where failure.code == "notFound" {
            guard let intent = savedIntent else {
                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app authoring intent is missing.")
            }
            _ = try await LocalApps.library.prepareAuthoringSource(sourceID: intent.sourceId,
                                                                   sourceDirectory: intent.sourceDirectory, mode: intent.mode)
            owner = LocalAppAuthoringOwner(sourceDirectory: appSource, projectID: project.record.id,
                                           sessionID: sessionID, appID: intent.appId)
        }
        guard projectStore.projects.first(where: { $0.record.id == project.record.id })?.record == project.record else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The app authoring binding no longer matches this project and session.")
        }
        return (try MobileUserStorage.projectMountGrantPaths(for: project, authorizedLocalAppSource: appSource), owner, savedIntent)
    }

    #if canImport(harness_runtimeFFI)
    private func waitForReleasedOwner(_ engine: EngineConversationSource, sessionID: String) async -> Bool {
        let identity = canonicalSessionID(sessionID)
        return await SessionOwnerDrainPolicy.confirmed(waitForMedia: {
            await engine.waitForMediaAttachmentDrain(timeoutMs: 8_000)
        }, waitForSession: {
            guard UUID(uuidString: identity) != nil else { return !engine.nativeConstructionStarted }
            return await waitForMobileSessionRelease(lingxiHome: MobileUserStorage.directories().configHome,
                                                     sessionId: identity, timeoutMs: 8_000)
        })
    }

    /// The source remains turn-gated until the real engine confirms the
    /// transcript's UUID and fixed Code mode. Only then may a missing native
    /// receipt be repaired for that captured owner.
    private func confirmRecoveredAuthoringOwner(_ engine: EngineConversationSource, entry: SessionHost.Entry) async throws {
        guard let owner = engine.pendingRecoveredAuthoringOwner,
              entry.scope == .project(owner.projectID), entry.mode == .code,
              canonicalSessionID(entry.sessionID) == canonicalSessionID(owner.sessionID),
              sessionHost.entry(for: engine) === entry else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The recovered app owner is unavailable.")
        }
        let expectedPath = try projectStore.transcriptPathForSession(owner.sessionID, in: entry.scope, mode: entry.mode)
        var verified: SessionWorkspaceState?
        for _ in 0..<100 {
            if let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: entry) {
                verified = snapshot.state
                break
            }
            guard sessionHost.entry(for: engine) === entry else { throw CancellationError() }
            try await Task.sleep(for: .milliseconds(50))
        }
        guard let verified, verified.transcriptPath == expectedPath,
              canonicalSessionID(verified.sessionId) == canonicalSessionID(owner.sessionID),
              engine.model.sessionModeKnown, engine.model.sessionMode == entry.mode,
              sessionHost.entry(for: engine) === entry else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The restored app transcript does not match its native owner.")
        }
        let binding: LocalAppAuthoring
        var requiresSourceCwd = false
        do {
            binding = try await LocalApps.library.exactAuthoringBinding(sourceDirectory: owner.sourceDirectory,
                projectID: owner.projectID, sessionID: owner.sessionID, mode: entry.mode,
                expectedSourceID: engine.recoveredAuthoringIntent?.sourceId)
        } catch let failure as LocalAppServiceFailure where failure.code == "notFound" {
            guard let intent = engine.recoveredAuthoringIntent,
                  intent.projectId == owner.projectID, intent.sourceDirectory == owner.sourceDirectory,
                  intent.authoringSessionId == canonicalSessionID(owner.sessionID),
                  intent.transcriptPath == verified.transcriptPath, intent.mode == entry.mode,
                  verified.cwd == owner.sourceDirectory else {
                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app intent does not match the native owner.")
            }
            requiresSourceCwd = true
            let authoring = LocalAppAuthoring(sourceDirectory: owner.sourceDirectory, workspaceId: owner.projectID,
                projectRef: .mobileProject(owner.projectID), authoringSessionId: owner.sessionID, appId: intent.appId)
            let registration = Task { @MainActor in
                try await LocalApps.library.registerAuthoring(authoring, sourceID: intent.sourceId,
                    requestID: intent.sourceRequestId, mode: entry.mode, setCurrent: false)
                return try await LocalApps.library.exactAuthoringBinding(sourceDirectory: owner.sourceDirectory,
                    projectID: owner.projectID, sessionID: owner.sessionID, mode: entry.mode,
                    expectedSourceID: intent.sourceId)
            }
            binding = try await registration.value
        }
        guard owner.matches(binding), sessionHost.entry(for: engine) === entry,
              let current = await sessionHost.authenticatedWorkspaceSnapshot(for: entry),
              current.state.transcriptPath == expectedPath,
              canonicalSessionID(current.state.sessionId) == canonicalSessionID(owner.sessionID),
              !requiresSourceCwd || current.state.cwd == owner.sourceDirectory else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The recovered app binding no longer matches its native owner.")
        }
        var intent = engine.recoveredAuthoringIntent
        if intent == nil,
           let selected = try? await projectStore.selectedLocalAppAuthoringIntent(projectID: owner.projectID),
           selected.sourceDirectory == owner.sourceDirectory,
           selected.authoringSessionId == canonicalSessionID(owner.sessionID),
           selected.transcriptPath == expectedPath {
            intent = selected
        }
        guard sessionHost.entry(for: engine) === entry,
              let latest = await sessionHost.authenticatedWorkspaceSnapshot(for: entry),
              latest.state.transcriptPath == expectedPath,
              !requiresSourceCwd || latest.state.cwd == owner.sourceDirectory else { throw CancellationError() }
        try engine.confirmRecoveredAuthoringOwner(owner, binding: binding)
        if let intent {
            do {
                if try await projectStore.localAppSourceCreateIntent(requestID: intent.intentId) != nil {
                    try await projectStore.completeLocalAppSourceCreateIntent(requestID: intent.intentId, owner: intent)
                }
            } catch { projectStore.pendingLocalAppError = error.localizedDescription }
        }
    }

    /// A durable intent can acquire its UUID while the task that wrote it is
    /// suspended before updating its in-memory attempt. Match the immutable
    /// request first, then authenticate the retained source itself.
    private func validatePendingAuthoringEntry(_ entry: SessionHost.Entry, intent: LocalAppAuthoringIntent) async throws {
        guard entry.scope == .project(intent.projectId), entry.mode == .code,
              entry.workspace == intent.sourceDirectory, entry.source.model.authoringBindingPending,
              pendingAuthoringAttempts[entry.id].map({ intent.matchesInFlightAttempt($0.intent) }) == true,
              sessionHost.entry(for: entry.source) === entry else {
            throw SessionHost.OwnershipError.sourceIdentityMismatch
        }
        let sessionID = canonicalSessionID(entry.sessionID)
        if sessionID.isEmpty {
            guard intent.authoringSessionId == nil, entry.source.model.activeSessionId.isEmpty else {
                throw SessionHost.OwnershipError.sourceIdentityMismatch
            }
            return
        }
        guard intent.authoringSessionId.map(canonicalSessionID).map({ $0 == sessionID }) ?? true else {
            throw SessionHost.OwnershipError.sourceIdentityMismatch
        }
        for _ in 0..<100 {
            if let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: entry) {
                guard snapshot.state.cwd == intent.sourceDirectory,
                      intent.transcriptPath.map({ $0 == snapshot.state.transcriptPath }) ?? true,
                      !snapshot.state.transcriptPath.isEmpty,
                      (snapshot.state.transcriptPath as NSString).isAbsolutePath,
                      pendingAuthoringAttempts[entry.id].map({ intent.matchesInFlightAttempt($0.intent) }) == true,
                      sessionHost.entry(for: entry.source) === entry else {
                    throw SessionHost.OwnershipError.sourceIdentityMismatch
                }
                return
            }
            guard sessionHost.entry(for: entry.source) === entry else { throw CancellationError() }
            try await Task.sleep(for: .milliseconds(50))
        }
        throw LocalAppServiceFailure("AUTHORING_CHANGED", "The authoring workspace could not be authenticated.")
    }

    private func prepareExistingAuthoringDraft(_ entry: SessionHost.Entry, request: LocalAppDevelopmentRequest,
                                               transcriptPath: String) async throws {
        guard entry.mode == .code, !entry.voice.hasOwnedAudio, !entry.source.hasLiveSideChats,
              let engine = entry.source as? EngineConversationSource,
              canonicalSessionID(request.authoring?.authoringSessionId ?? "") == canonicalSessionID(entry.sessionID) else {
            throw SessionHost.OwnershipError.sourceIdentityMismatch
        }
        let lease = try engine.beginAuthoringWorkspaceValidation()
        defer { engine.endAuthoringWorkspaceValidation(lease) }
        let revision = entry.draftRevision
        guard let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: entry),
              snapshot.state.cwd == request.sourceDirectory, snapshot.state.transcriptPath == transcriptPath,
              sessionHost.entry(for: engine) === entry else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "This conversation is no longer in the app source directory.")
        }
        if entry.draftRevision == revision,
           !scopedPreferences.hasDraft(scope: entry.scope, mode: entry.mode, ownerSessionID: entry.sessionID) {
            seedLocalAppDraft(request, entry: entry)
        }
    }

    private func discardFailedUnselectedAuthoringEntry(_ entry: SessionHost.Entry,
                                                        engine: EngineConversationSource) async {
        guard sessionHost.selectedEntryID != entry.id, sessionHost.entry(for: engine) === entry,
              !entry.voice.hasOwnedAudio else { return }
        do {
            try await engine.retireForReconnect(allowUnconfirmedAuthoring: engine.pendingRecoveredAuthoringOwner != nil)
            let ownerID = canonicalSessionID(entry.sessionID)
            retiredAuthoringEntriesAwaitingRelease.insert(entry.id)
            let released = await sessionHost.releaseRetiredAuthoringSourceAfterDrain(engine, entry: entry,
                stillCurrent: { true }, waitForRelease: {
                    await waitForReleasedOwner(engine, sessionID: ownerID)
                })
            if released { retiredAuthoringEntriesAwaitingRelease.remove(entry.id) }
        } catch {
            // Keep the exact owner whenever activity or release is uncertain.
        }
    }

    private func quarantineFailedPreparation(_ source: any ConversationSource, requestID: String,
                                             scope: ConversationScope, mode: SessionMode, sessionID: String?) async {
        guard let engine = source as? EngineConversationSource, engine.nativeConstructionStarted,
              sessionHost.entry(for: source) == nil else { return }
        do {
            // Strongly retain the handle and its preparation lease before
            // awaiting anything, including the native workspace query.
            try sessionHost.quarantineFailedPreparation(source, requestID: requestID,
                                                        scope: scope, mode: mode, sessionID: sessionID)
            try await drainFailedPreparation(requestID: requestID)
        } catch {
            projectStore.errorMessage = error.localizedDescription
        }
    }

    private func drainFailedPreparation(requestID: String) async throws {
        try await sessionHost.drainFailedPreparation(requestID: requestID) { source in
            guard let engine = source as? EngineConversationSource else {
                throw SessionHost.OwnershipError.conflictingOwner
            }
            // Keep the fence global while an in-flight builder is retiring.
            // Only its authenticated UUID can narrow the fence.
            try await engine.retireFailedPreparationForRelease()
            guard let identity = await engine.preparedSessionIDForRelease() else {
                throw LocalAppServiceFailure("OWNER_RELEASE_PENDING", String(localized: "conversation_reconnect_release_pending"))
            }
            try self.sessionHost.identifyFailedPreparation(requestID: requestID, source: engine, sessionID: identity)
            let released = await self.sessionHost.releaseFailedPreparationAfterDrain(requestID: requestID,
                source: engine, sessionID: identity, waitForRelease: {
                    await self.waitForReleasedOwner(engine, sessionID: identity)
                })
            guard released else {
                throw LocalAppServiceFailure("OWNER_RELEASE_PENDING", String(localized: "conversation_reconnect_release_pending"))
            }
        }
    }

    private func drainFailedPreparations(scope: ConversationScope, mode: SessionMode, sessionID: String) async throws {
        for requestID in sessionHost.failedPreparationIDs(scope: scope, mode: mode, sessionID: sessionID) {
            try await drainFailedPreparation(requestID: requestID)
        }
    }
    #endif

    private func makeSource(
        scope: ConversationScope,
        snapshot: ProviderLaunchSnapshot,
        mode: SessionMode,
        sessionID: String? = nil,
        selectedTranscriptPath: String? = nil,
        provisionalAuthoring: ProvisionalLocalAppGrant? = nil,
        allowAuthoringRecovery: Bool = false
    ) async throws -> any ConversationSource {
        var projectCwd: String?
        let transcriptPath: String?
        let project: ProjectSnapshot?
        switch scope {
        case .remote:
            throw NSError(domain: "ConversationSource", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "Remote sessions use their dedicated view."])
        case .global, .project:
            project = scope.projectID.flatMap { id in
                projectStore.projects.first(where: { $0.record.id == id })
            }
            let rows = project?.sessions ?? projectStore.globalSessions
            projectCwd = rows.first(where: { canonicalSessionID($0.sessionId) == canonicalSessionID(sessionID ?? "") })?.workspaceDirectory
                ?? project?.record.activeDirectory ?? LXISHGuestPaths.conversationWorkspace(for: mode)
            transcriptPath = try sessionID.flatMap { id in
                id.isEmpty ? nil : try projectStore.transcriptPathForSession(id, in: scope, mode: mode)
            }
        case .scheduled:
            project = nil
            projectCwd = try MobileUserStorage.scheduledWorkspace(sessionID: sessionID)
            transcriptPath = try sessionID.flatMap { id in
                id.isEmpty ? nil : try cronRepository.scheduledTranscriptPath(sessionID: id, mode: mode)
            }
        }
        if let selectedTranscriptPath, transcriptPath != selectedTranscriptPath {
            throw SessionHost.OwnershipError.sourceIdentityMismatch
        }
        let runtime: TerminalRuntimeConfig? = TerminalRuntimeDescriptor.make(
            appSandboxRoot: appSandboxRoot,
            project: project,
            linuxRuntime: settingsStore.linuxRuntime
        ).config
        let resolvedGrants: (grants: [String], owner: LocalAppAuthoringOwner?, missingIntent: LocalAppAuthoringIntent?)
        var needsAuthoringRecovery = false
        if let project {
            let sources = MobileUserStorage.localAppSourceDirectories(for: project)
            if allowAuthoringRecovery, sessionID == nil, provisionalAuthoring == nil, !sources.isEmpty {
                guard mode == .code,
                      let selected = try await projectStore.selectedLocalAppAuthoringIntent(projectID: project.record.id),
                      sources == [selected.sourceDirectory] else {
                    throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app authoring intent is missing or ambiguous.")
                }
                projectCwd = selected.sourceDirectory
                resolvedGrants = ([], nil, nil)
                needsAuthoringRecovery = true
            } else {
                resolvedGrants = try await resolveProjectMountGrants(project, mode: mode, sessionID: sessionID,
                                                                    provisional: provisionalAuthoring)
            }
        } else {
            resolvedGrants = ([], nil, nil)
        }
        var launchOptions = ConversationSourceFactory.LaunchOptions(
            projectCwd: projectCwd,
            additionalWorkspaceDirectories: project?.record.additionalDirectories ?? [],
            sessionMode: mode,
            providerConfigured: !snapshot.enabledProfileIDs.isEmpty,
            providerProfilesJson: snapshot.providerProfilesJSON,
            providerRoutingJson: snapshot.routingJSON,
            defaultModelID: snapshot.defaultModelID,
            visionDelegationEnabled: snapshot.visionDelegationEnabled,
            mobileLinux: runtime,
            projectMountGrants: resolvedGrants.grants,
            initialSessionId: sessionID.flatMap { $0.isEmpty ? nil : $0 },
            initialSessionTranscriptPath: transcriptPath
        )
        #if canImport(harness_runtimeFFI)
        launchOptions.mediaHostComposition = mediaHostComposition
        #endif
        let replacement = ConversationSourceFactory.make(options: launchOptions)
        #if canImport(harness_runtimeFFI)
        if let engine = replacement as? EngineConversationSource {
            if needsAuthoringRecovery { engine.requireAuthoringMountAuthorization() }
            else if let owner = resolvedGrants.owner { try engine.setVerifiedAuthoringOwner(owner, intent: resolvedGrants.missingIntent) }
            else if let provisionalAuthoring {
                try engine.requireAuthoringBindingBeforeTurns(LocalAppProvisionalAuthoringOwner(
                    sourceDirectory: provisionalAuthoring.intent.sourceDirectory,
                    projectID: provisionalAuthoring.intent.projectId, appID: provisionalAuthoring.intent.appId))
            }
        } else if needsAuthoringRecovery {
            throw LocalAppServiceFailure("ENGINE_UNAVAILABLE", "App development requires the local conversation engine.")
        }
        #endif
        #if DEBUG
        if sessionID == nil, ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] == "1",
           let mock = replacement as? MockConversationSource {
            // Every new UI-test conversation needs its own owner, just like
            // a native session. Reusing the canned fixture's fixed ID makes
            // SessionHost correctly reject a cross-scope switch.
            mock.startNewConversation()
            mock.model.activeSessionId = UUID().uuidString
            mock.model.workspaceState = nil
        }
        #endif
        return replacement
    }

    private func rebuildSource(
        snapshot: ProviderLaunchSnapshot,
        preserveSourceGeneration: Bool = false
    ) async throws {
        guard !projectSwitching else { throw CancellationError() }
        guard cronOwnerGate.allowsNewOwner(sessionID: activeSession) else {
            throw ProjectRepositoryError.invalidSessionOwnership("A scheduled conversation is being prepared. Try again.")
        }
        projectSwitching = true
        defer {
            projectSwitching = false
            resumePendingUserNavigation()
        }
        persistConversationScope()
        let old = source
        try requireSourceReplacementAllowed(old)
        #if canImport(harness_runtimeFFI)
        try await drainFailedPreparations(scope: activeScope, mode: activeMode, sessionID: activeSession)
        try requireSourceReplacementAllowed(old)
        #endif
        try await cronRepository.prepareOwnerAdmission()
        try requireSourceReplacementAllowed(old)
        #if canImport(harness_runtimeFFI)
        let unsentTranscriptPath: String?
        let hasExpiredSideArchives = (old as? EngineConversationSource)?.sideChats.expiredArchives.isEmpty == false
        if !old.model.unsentPrompts.isEmpty || hasExpiredSideArchives {
            guard let state = old.model.workspaceState,
                  canonicalSessionID(state.sessionId) == canonicalSessionID(activeSession),
                  !state.transcriptPath.isEmpty, (state.transcriptPath as NSString).isAbsolutePath else {
                throw SessionHost.OwnershipError.sourceIdentityMismatch
            }
            unsentTranscriptPath = state.transcriptPath
        } else { unsentTranscriptPath = nil }
        #else
        let unsentTranscriptPath: String? = nil
        #endif
        let replacement = try await makeSource(scope: activeScope, snapshot: snapshot, mode: activeMode,
                                               sessionID: activeSession, selectedTranscriptPath: unsentTranscriptPath)
        guard sessionHost.beginReplacement(of: old, with: replacement, sessionID: activeSession,
                                           scope: activeScope, mode: activeMode) else {
            throw SessionHost.OwnershipError.conflictingOwner
        }
        let preparationID = UUID().uuidString.lowercased()
        defer { sessionHost.endPreparingClaim(of: replacement) }
        sessionHost.protectIdentity(of: replacement, scope: activeScope, mode: activeMode, sessionID: activeSession)
        #if canImport(harness_runtimeFFI)
            installExternalEventHandler(on: replacement)
            if let engine = replacement as? EngineConversationSource {
                let restartScope = activeScope
                let restartMode = activeMode
                let restartID = canonicalSessionID(activeSession)
                let projects = projectStore
                let gate = cronOwnerGate
                let cron = cronRepository
                let host = sessionHost
                engine.setSessionStartupAdmission { [weak projects, weak gate, weak cron] in
                    guard let projects, gate?.allowsNewOwner(sessionID: restartID) == true,
                          cron?.ownerAdmissionIsCurrent == true else { return false }
                    return restartID.isEmpty ? projects.admitsUnclaimedOwner()
                        : projects.admitsSessionIdentity(restartID, in: restartScope, mode: restartMode)
                }
                engine.setSessionOwnerAdmission { [weak projects, weak host, weak gate, weak replacement] id in
                    guard let projects, let host, let replacement, gate?.allowsNewOwner(sessionID: id) == true else { return false }
                    if !restartID.isEmpty {
                        return canonicalSessionID(id) == restartID &&
                            projects.admitsSessionIdentity(id, in: restartScope, mode: restartMode)
                    }
                    return host.claimIdentity(id, for: replacement, scope: restartScope, mode: restartMode)
                }
            }
        #endif
        let entry: SessionHost.Entry
        do {
            #if canImport(harness_runtimeFFI)
            if let engine = old as? EngineConversationSource {
                var ownerID = canonicalSessionID(sessionHost.entry(for: old)?.sessionID ?? engine.model.activeSessionId)
                try await engine.retireForReconnect()
                if ownerID.isEmpty { ownerID = await engine.preparedSessionIDForRelease() ?? "" }
                guard await waitForReleasedOwner(engine, sessionID: ownerID) else {
                    throw NSError(domain: "ConversationSourceReplacement", code: 3,
                        userInfo: [NSLocalizedDescriptionKey: String(localized: "conversation_reconnect_release_pending")])
                }
            }
            #endif
            try requireSourceReplacementAllowed(old)
            try await replacement.prepare()
            try requireSourceReplacementAllowed(old)
            let workspace = sessionHost.entry(for: old)?.workspace ?? currentWorkspaceGuestPath
            entry = try sessionHost.replaceIdleSource(old, with: replacement, scope: activeScope,
                mode: activeMode, sessionID: confirmedSession, workspace: workspace, draft: draft,
                validateUnsentOwner: {
                    guard let unsentTranscriptPath else { throw SessionHost.OwnershipError.sourceIdentityMismatch }
                    let currentPath: String?
                    switch activeScope {
                    case .global, .project:
                        currentPath = try projectStore.transcriptPathForSession(activeSession, in: activeScope, mode: activeMode)
                    case .scheduled:
                        currentPath = try cronRepository.scheduledTranscriptPath(sessionID: activeSession, mode: activeMode)
                    case .remote:
                        throw SessionHost.OwnershipError.sourceIdentityMismatch
                    }
                    guard currentPath == unsentTranscriptPath else {
                        throw SessionHost.OwnershipError.sourceIdentityMismatch
                    }
                })
        } catch {
            #if canImport(harness_runtimeFFI)
            await quarantineFailedPreparation(replacement, requestID: preparationID, scope: activeScope,
                                              mode: activeMode, sessionID: activeSession.isEmpty ? nil : activeSession)
            #endif
            if !old.isAvailableForPrompts {
                old.model.error = ConversationError(kind: .host,
                    message: String(localized: "conversation_reconnect_disconnected"))
            }
            throw error
        }
        activeSession = confirmedSession
        pendingSessionRestoreID = confirmedSession.isEmpty ? nil : confirmedSession
        source.setSettingsActive(false)
        source = replacement
        sessionHost.select(entry)
        sessionHost.refreshWorkspace(entry)
        source.setSettingsActive(true)
        // The catalog belongs to the engine build, not to a single source
        // instance. Keep it across normal reconnects so changing a provider
        // cannot trigger a second catalog bootstrap/rebuild loop.
        wireCurrentSource(preserveCatalog: true)
        if !preserveSourceGeneration {
            sourceGeneration = UUID()
        }
    }

    @MainActor
    private func requireSourceReplacementAllowed(
        _ previous: any ConversationSource
    ) throws {
        guard source === previous else { throw CancellationError() }
        guard !previous.model.authoringBindingPending else {
            throw LocalAppServiceFailure("AUTHORING_PENDING", "Wait for the app authoring binding to finish.")
        }
        guard sessionHost.entry(for: previous)?.voice.hasOwnedAudio != true else {
            throw NSError(domain: "ConversationSourceReplacement", code: 1,
                userInfo: [NSLocalizedDescriptionKey: String(localized: "chat_error_finish_background_turn_first")])
        }
        guard !previous.hasLiveSideChats else {
            throw NSError(domain: "ConversationSourceReplacement", code: 2,
                userInfo: [NSLocalizedDescriptionKey: String(localized: "side_chat_end_before_restart")])
        }
        guard ConversationSourceReplacementPolicy.allowsReplacement(
            of: previous.model, hasActiveRealtimeAudio: previous.hasActiveRealtimeAudio
        ) else {
            throw NSError(
                domain: "ConversationSourceReplacement", code: 1,
                userInfo: [NSLocalizedDescriptionKey: String(localized: "chat_error_finish_background_turn_first")]
            )
        }
    }

    /// Thin project-flavored wrapper over `switchScope` — the drawer's
    /// project callbacks keep their optional-projectID spelling (`nil` ==
    /// global scope).
    private func switchProject(to projectID: String?, resumeSessionID: String? = nil, startNew: Bool = false) {
        switchScope(
            to: projectID.map(ConversationScope.project) ?? .global,
            resumeSessionID: resumeSessionID,
            startNew: startNew
        )
    }

    struct ConversationSelectionSnapshot: Equatable {
        let scope: ConversationScope
        let mode: SessionMode
        let activeSession: String
        let confirmedSession: String
        let pendingRestoreID: String?
        let draft: String

        var resumeSessionID: String? {
            if let pendingRestoreID, !pendingRestoreID.isEmpty { return pendingRestoreID }
            if !activeSession.isEmpty { return activeSession }
            if !confirmedSession.isEmpty { return confirmedSession }
            return nil
        }

        var startsNewConversation: Bool { resumeSessionID == nil }
    }

    struct SessionForkRollbackRequest: Equatable {
        let snapshot: ConversationSelectionSnapshot
    }

    enum SessionForkRollbackPlanner {
        static func rollbackRequest(
            origin: ConversationSelectionSnapshot,
            sourceScope: ConversationScope,
            sourceMode: SessionMode,
            sourceSessionID: String
        ) -> SessionForkRollbackRequest? {
            guard origin.scope != sourceScope
                || origin.mode != sourceMode
                || origin.activeSession != sourceSessionID
                || origin.confirmedSession != sourceSessionID
            else { return nil }
            return SessionForkRollbackRequest(snapshot: origin)
        }
    }

    private struct PendingSessionFork: Equatable {
        let sourceScope: ConversationScope
        let sourceSessionID: String
        let sourceMode: SessionMode
        let targetMode: SessionMode
        let origin: ConversationSelectionSnapshot
        let requestID: String
        var navigationRevision: UUID
        var ownerEntryID: UUID? = nil
        var submitted = false

        var rollbackRequest: SessionForkRollbackRequest? {
            SessionForkRollbackPlanner.rollbackRequest(
                origin: origin,
                sourceScope: sourceScope,
                sourceMode: sourceMode,
                sourceSessionID: sourceSessionID
            )
        }
    }

    private struct PendingConversationSelectionRestore: Equatable {
        let snapshot: ConversationSelectionSnapshot
        let message: String?
    }

    /// Returns `false` when the switch was refused because another one is
    /// still in flight — callers holding a one-shot signal (the create-flow
    /// landing) must retry rather than drop it.
    @discardableResult
    private func switchMode(to mode: SessionMode) -> Bool {
        let remembered = scopedPreferences.lastScope(mode: mode) ?? .global
        let destination: ConversationScope
        if let projectID = remembered.projectID {
            if let project = projectStore.projects.first(where: { $0.id == projectID && $0.record.visibleModes.contains(mode) }),
               mode != .chat || MobileUserStorage.localAppSourceDirectories(for: project).isEmpty {
                destination = remembered
            } else {
                destination = .global
            }
        } else {
            destination = remembered
        }
        return switchScope(to: destination, mode: mode)
    }

    @discardableResult
    private func switchScope(
        to scope: ConversationScope,
        mode: SessionMode? = nil,
        resumeSessionID: String? = nil,
        startNew: Bool = false,
        handoffDraft: SessionHandoffDraft? = nil,
        selectedTranscriptPath: String? = nil,
        recoverAuthoring: Bool = false,
        expectedAuthoringWorkspace: ExistingAuthoringWorkspace? = nil,
        retainedEntry: SessionHost.Entry? = nil,
        sourceSnapshot: ProviderLaunchSnapshot? = nil,
        onFailure: ((Error) -> Void)? = nil
    ) -> Bool {
        localAppSelectionAuthority.invalidate()
        if let projectID = scope.projectID {
            localAppRegistrationOrder.invalidateProjectSelection(projectID: projectID)
        }
        guard !projectSwitching else { return false }
        let navigationGeneration = userNavigation.generation
        if case .remote = scope { return false }
        let targetMode = mode ?? activeMode
        let targetID = (startNew || recoverAuthoring) ? "" : canonicalSessionID(resumeSessionID ?? restoredSessionID(scope: scope, mode: targetMode))
        let existing: SessionHost.Entry?
        do {
            if let retainedEntry {
                guard !startNew, sessionHost.entry(for: retainedEntry.source) === retainedEntry,
                      retainedEntry.scope == scope, retainedEntry.mode == targetMode,
                      targetID.isEmpty || canonicalSessionID(retainedEntry.sessionID) == targetID else {
                    throw SessionHost.OwnershipError.sourceIdentityMismatch
                }
                existing = retainedEntry
            } else {
                existing = try sessionHost.entryForClaim(sessionID: targetID, scope: scope, mode: targetMode)
            }
        } catch {
            projectStore.errorMessage = String(localized: "project_switch_failed_message \(error.localizedDescription)")
            return false
        }
        if let existing, retiredAuthoringEntriesAwaitingRelease.contains(existing.id) {
            projectStore.errorMessage = String(localized: "conversation_reconnect_release_pending")
            return false
        }
        guard existing != nil || cronOwnerGate.allowsNewOwner(sessionID: targetID) else {
            projectStore.errorMessage = "A scheduled conversation is being prepared. Try again."
            return false
        }
        guard existing != nil || targetID.isEmpty || projectStore.admitsSessionIdentity(targetID, in: scope, mode: targetMode) else {
            return false
        }
        #if canImport(harness_runtimeFFI)
        var initialValidation: (EngineConversationSource, UUID)?
        if expectedAuthoringWorkspace != nil, let existing {
            do {
                guard !existing.voice.hasOwnedAudio, !existing.source.hasLiveSideChats,
                      let engine = existing.source as? EngineConversationSource else {
                    throw LocalAppServiceFailure("ENGINE_UNAVAILABLE", "App development requires the local conversation engine.")
                }
                initialValidation = (engine, try engine.beginAuthoringWorkspaceValidation())
            } catch {
                onFailure?(error)
                return false
            }
        }
        #endif
        markHandoffNavigationChanged()
        #if canImport(harness_runtimeFFI)
        if recoverAuthoring, scope == activeScope, targetMode == activeMode,
           let entry = sessionHost.entry(for: source),
           (source as? EngineConversationSource)?.needsAuthoringMountAuthorization == true {
            allowBlankAuthoringRetryFor = entry.id
            sourceGeneration = UUID()
            navigation.path.removeAll()
            navigation.closeSidebar()
            return true
        }
        #endif
        if selectedTranscriptPath == nil, expectedAuthoringWorkspace == nil, !startNew, !recoverAuthoring, scope == activeScope, targetMode == activeMode,
           (targetID.isEmpty || canonicalSessionID(targetID) == canonicalSessionID(source.model.activeSessionId)) {
            navigation.path.removeAll()
            navigation.closeSidebar()
            return true
        }
        navigation.path.removeAll()
        navigation.closeSidebar()
        scopedPreferences.setDraft(draft, scope: activeScope, mode: activeMode, ownerSessionID: draftOwnerSessionID)
        sessionHost.entry(for: source)?.draft = draft
        let previousSource = source
        let rows = scope.projectID.flatMap { id in projectStore.projects.first(where: { $0.id == id })?.sessions }
            ?? projectStore.globalSessions
        let workspace = rows.first(where: { canonicalSessionID($0.sessionId) == targetID })?.workspaceDirectory
            ?? scope.projectID.flatMap { id in projectStore.projects.first(where: { $0.id == id })?.record.activeDirectory }
            ?? (scope == .scheduled ? ((try? MobileUserStorage.scheduledWorkspace(sessionID: targetID)) ?? MobileUserStorage.directories().scheduledGuestCwd)
                : LXISHGuestPaths.conversationWorkspace(for: targetMode))
        projectSwitching = true
        Task { @MainActor in
            #if canImport(harness_runtimeFFI)
            var validation = initialValidation
            defer { if let validation { validation.0.endAuthoringWorkspaceValidation(validation.1) } }
            #endif
            var rollback: ProjectActiveSelectionRollback?
            var newlyRetained: SessionHost.Entry?
            do {
                if startNew, let projectID = scope.projectID {
                    try await projectStore.addMode(targetMode, projectID: projectID)
                }
                guard userNavigation.isCurrent(navigationGeneration) else { throw CancellationError() }
                if let selectedTranscriptPath, let existing {
                    guard let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: existing),
                          snapshot.state.transcriptPath == selectedTranscriptPath else {
                        throw SessionHost.OwnershipError.sourceIdentityMismatch
                    }
                    guard userNavigation.isCurrent(navigationGeneration) else { throw CancellationError() }
                }
                if scope != activeScope {
                    rollback = try await projectStore.persistActiveForSwitch(projectId: scope.projectID)
                }
                guard userNavigation.isCurrent(navigationGeneration) else { throw CancellationError() }
                let entry: SessionHost.Entry
                if let existing {
                    entry = existing
                } else {
                    #if canImport(harness_runtimeFFI)
                    try await drainFailedPreparations(scope: scope, mode: targetMode, sessionID: targetID)
                    guard userNavigation.isCurrent(navigationGeneration) else { throw CancellationError() }
                    #endif
                    try await cronRepository.prepareOwnerAdmission()
                    try await cronOwnerGate.waitUntilAvailable(sessionID: targetID)
                    guard userNavigation.isCurrent(navigationGeneration) else { throw CancellationError() }
                    let replacement = try await makeSource(scope: scope, snapshot: sourceSnapshot ?? providerRepository.makeLaunchSnapshot(),
                                                     mode: targetMode, sessionID: targetID.isEmpty ? nil : targetID,
                                                     selectedTranscriptPath: selectedTranscriptPath,
                                                     allowAuthoringRecovery: !startNew)
                    guard userNavigation.isCurrent(navigationGeneration) else { throw CancellationError() }
                    let preparationID = UUID().uuidString.lowercased()
                    defer { sessionHost.endPreparingClaim(of: replacement) }
                    sessionHost.protectIdentity(of: replacement, scope: scope, mode: targetMode,
                                                sessionID: targetID)
                    #if canImport(harness_runtimeFFI)
                    installExternalEventHandler(on: replacement)
                    #endif
                    let restoredDraft = startNew ? "" : scopedPreferences.draft(scope: scope, mode: targetMode, ownerSessionID: targetID)
                    // An unprepared source must never be published for reuse.
                    // Keep the old owner current if preparation fails.
                    do {
                        #if canImport(harness_runtimeFFI)
                        if (replacement as? EngineConversationSource)?.needsAuthoringMountAuthorization != true {
                            try await replacement.prepare()
                        }
                        #else
                        try await replacement.prepare()
                        #endif
                        entry = try sessionHost.retain(source: replacement, scope: scope, mode: targetMode,
                                                       sessionID: targetID, workspace: workspace, draft: restoredDraft)
                    } catch {
                        #if canImport(harness_runtimeFFI)
                        await quarantineFailedPreparation(replacement, requestID: preparationID,
                            scope: scope, mode: targetMode, sessionID: targetID.isEmpty ? nil : targetID)
                        #endif
                        throw error
                    }
                    newlyRetained = entry
                    if !startNew {
                        entry.draftSourceCodeUnits = scopedPreferences.draftSourceCodeUnits(
                            scope: scope, mode: targetMode, ownerSessionID: targetID)
                        entry.handoffReference = scopedPreferences.handoffReference(sessionID: targetID, scope: scope, mode: targetMode)
                    }
                }
                #if canImport(harness_runtimeFFI)
                if let engine = entry.source as? EngineConversationSource, engine.pendingRecoveredAuthoringOwner != nil {
                    try await confirmRecoveredAuthoringOwner(engine, entry: entry)
                    guard userNavigation.isCurrent(navigationGeneration) else { throw CancellationError() }
                }
                #endif
                if let expected = expectedAuthoringWorkspace {
                    #if canImport(harness_runtimeFFI)
                    guard let engine = entry.source as? EngineConversationSource else {
                        throw LocalAppServiceFailure("ENGINE_UNAVAILABLE", "App development requires the local conversation engine.")
                    }
                    if validation == nil {
                        // A newly prepared source can receive its resume
                        // event after prepare returns. It is still hidden and
                        // cannot accept UI input during this wait.
                        for _ in 0..<100 where engine.model.sessionTransitionPending {
                            guard userNavigation.isCurrent(navigationGeneration) else { throw CancellationError() }
                            try await Task.sleep(for: .milliseconds(50))
                        }
                        validation = (engine, try engine.beginAuthoringWorkspaceValidation())
                    }
                    guard validation?.0 === engine else { throw SessionHost.OwnershipError.sourceIdentityMismatch }
                    var authenticated: SessionWorkspaceState?
                    for _ in 0..<100 {
                        if let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: entry) {
                            authenticated = snapshot.state
                            break
                        }
                        guard userNavigation.isCurrent(navigationGeneration) else { throw CancellationError() }
                        try await Task.sleep(for: .milliseconds(50))
                    }
                    guard let authenticated, expected.matches(authenticated),
                          sessionHost.entry(for: engine) === entry,
                          userNavigation.isCurrent(navigationGeneration) else {
                        throw LocalAppServiceFailure("AUTHORING_CHANGED", "This conversation is no longer in the app source directory.")
                    }
                    #else
                    throw LocalAppServiceFailure("ENGINE_UNAVAILABLE", "App development requires the local conversation engine.")
                    #endif
                }
                guard userNavigation.isCurrent(navigationGeneration) else { throw CancellationError() }
                scopedPreferences.setDraft(draft, scope: activeScope, mode: activeMode, ownerSessionID: draftOwnerSessionID)
                sessionHost.entry(for: previousSource)?.draft = draft
                voiceInteraction.handleContextChange()
                activeScope = scope
                activeMode = targetMode
                scopedPreferences.setCurrentMode(targetMode)
                scopedPreferences.setLastScope(scope, mode: targetMode)
                activeSession = entry.sessionID
                confirmedSession = entry.source.model.sessionTransitionPending ? "" : entry.sessionID
                pendingSessionRestoreID = entry.source.model.sessionTransitionPending ? targetID : nil
                draftOwnerSessionID = entry.sessionID
                draft = handoffDraft?.displayText ?? pendingBeginActionDraft ?? entry.draft
                pendingBeginActionDraft = nil
                entry.draft = draft
                if let handoffDraft {
                    entry.draftSourceCodeUnits = handoffDraft.sourceCodeUnits
                    entry.handoffReference = handoffDraft.reference
                    scopedPreferences.setDraft(draft, scope: scope, mode: targetMode, ownerSessionID: entry.sessionID)
                    scopedPreferences.setDraftSourceCodeUnits(handoffDraft.sourceCodeUnits, scope: scope,
                                                               mode: targetMode, ownerSessionID: entry.sessionID)
                    scopedPreferences.setHandoffReference(handoffDraft.reference, sessionID: entry.sessionID, scope: scope, mode: targetMode)
                    if entry.sessionID.isEmpty || !isRestorableSession(entry.sessionID, scope: scope, mode: targetMode) {
                        // Until the exact native source is indexed, a cold
                        // launch can recover the handoff as a blank-owner draft.
                        scopedPreferences.prepareUnindexedHandoffDraft(draft, sourceCodeUnits: handoffDraft.sourceCodeUnits,
                            reference: handoffDraft.reference, scope: scope, mode: targetMode)
                    }
                    pendingHandoffDrafts.removeAll { $0.id == handoffDraft.id }
                } else if startNew {
                    scopedPreferences.setDraft("", scope: scope, mode: targetMode, ownerSessionID: "")
                    scopedPreferences.setDraftSourceCodeUnits(nil, scope: scope, mode: targetMode, ownerSessionID: "")
                    scopedPreferences.setHandoffReference(nil, sessionID: "", scope: scope, mode: targetMode)
                    if entry.sessionID.isEmpty || !isRestorableSession(entry.sessionID, scope: scope, mode: targetMode) {
                        scopedPreferences.setActiveSessionID("", scope: scope, mode: targetMode)
                    }
                }
                persistActiveSessionIfRestorable(confirmedSession, scope: scope, mode: targetMode)
                previousSource.setSettingsActive(false)
                source = entry.source
                sessionHost.select(entry)
                sessionHost.refreshWorkspace(entry)
                sessionHost.sync(entry)
                voiceInteraction = entry.voice
                source.setSettingsActive(true)
                hasUnsavedConversationMedia = entry.attachment != nil
                liveWorkspaceState = source.model.workspaceState
                sourceGeneration = entry.id
                if recoverAuthoring { allowBlankAuthoringRetryFor = entry.id }
                #if canImport(harness_runtimeFFI)
                if let held = validation {
                    held.0.endAuthoringWorkspaceValidation(held.1)
                    validation = nil
                }
                #endif
                await cronRepository.refresh()
            } catch {
                #if canImport(harness_runtimeFFI)
                if let entry = newlyRetained,
                   sessionHost.selectedEntryID != entry.id, sessionHost.entry(for: entry.source) === entry,
                   !entry.voice.hasOwnedAudio, let engine = entry.source as? EngineConversationSource,
                   expectedAuthoringWorkspace != nil || engine.pendingRecoveredAuthoringOwner != nil {
                    if let held = validation, held.0 === engine {
                        held.0.endAuthoringWorkspaceValidation(held.1)
                        validation = nil
                    }
                    // Retire only this unselected, idle source. A permission,
                    // turn or audio owner keeps its entry and is never
                    // cancelled merely because navigation failed.
                    do {
                        try await engine.retireForReconnect(allowUnconfirmedAuthoring: engine.pendingRecoveredAuthoringOwner != nil)
                        let ownerID = canonicalSessionID(entry.sessionID)
                        retiredAuthoringEntriesAwaitingRelease.insert(entry.id)
                        let released = await sessionHost.releaseRetiredAuthoringSourceAfterDrain(engine, entry: entry,
                            stillCurrent: { true }, waitForRelease: {
                                await waitForReleasedOwner(engine, sessionID: ownerID)
                            })
                        if released { retiredAuthoringEntriesAwaitingRelease.remove(entry.id) }
                    } catch {
                        // Keeping the live entry is safer than replacing a
                        // source whose activity could not be drained.
                    }
                }
                #endif
                onFailure?(error)
                if let rollback, source === previousSource {
                    do { _ = try await projectStore.rollbackActiveSwitch(rollback) }
                    catch {
                        projectStore.errorMessage = String(localized: "project_switch_failed_message \(error.localizedDescription)")
                    }
                }
                if userNavigation.isCurrent(navigationGeneration), !(error is CancellationError) {
                    projectStore.errorMessage = String(localized: "project_switch_failed_message \(error.localizedDescription)")
                }
                pendingBeginActionDraft = nil
            }
            projectSwitching = false
            resumePendingUserNavigation()
            applyPendingConversationSelectionRestoreIfPossible()
        }
        return true
    }

    @discardableResult
    private func adoptEngineSession(_ sessionID: String) -> Bool {
        guard ConversationSessionRestorePolicy.shouldAdopt(
            candidateSessionID: sessionID,
            pendingRestoreID: pendingSessionRestoreID
        ) else {
            return false
        }
        // SwiftUI may observe the owner and mode in either order. Reconcile
        // them together before either callback persists the destination slot.
        adoptSessionMode(source.model.sessionMode)
        activeSession = sessionID
        confirmedSession = sessionID
        if isRestorableSession(sessionID, scope: activeScope, mode: activeMode) {
            if let reference = sessionHost.entry(for: source)?.handoffReference {
                scopedPreferences.setHandoffReference(reference, sessionID: sessionID, scope: activeScope, mode: activeMode)
            }
            scopedPreferences.setActiveSessionID(sessionID, scope: activeScope, mode: activeMode)
        }
        if pendingSessionRestoreID == sessionID {
            pendingSessionRestoreID = nil
        }
        triggerPendingForkIfReady(for: sessionID)
        return true
    }

    @discardableResult
    private func rollbackFailedSession(_ sessionID: String) -> Bool {
        guard let rollbackSelection = ConversationSessionRestorePolicy.rollbackSelection(
            failedSessionID: sessionID,
            pendingRestoreID: pendingSessionRestoreID,
            activeSessionID: activeSession,
            confirmedSessionID: confirmedSession
        ) else {
            return false
        }
        pendingSessionRestoreID = nil
        if draftOwnerSessionID != rollbackSelection {
            let restoredDraft = scopedPreferences.draftForFailedSelection(
                draft, scope: activeScope, mode: activeMode,
                fromOwner: draftOwnerSessionID, toOwner: rollbackSelection
            )
            draftOwnerSessionID = rollbackSelection
            draft = restoredDraft
        }
        activeSession = rollbackSelection
        if pendingLinkedSessionID == sessionID {
            pendingLinkedSessionID = nil; pendingMessageLink = nil
            externalLinkError = String(localized: "deep_link_conversation_open_failed")
        }
        persistActiveSessionIfRestorable(confirmedSession, scope: activeScope, mode: activeMode)
        if let pending = pendingSessionFork,
           pending.sourceSessionID == sessionID {
            rollbackPendingSessionFork(pending, message: nil)
        }
        return true
    }

    private func toggleWorkspacePinned(_ workspaceKey: String) {
        let pinned = workspacePinnedAt[workspaceKey] == nil
        workspacePinnedAt = scopedPreferences.setWorkspacePinned(
            pinned,
            workspaceKey: workspaceKey
        )
    }

    private func toggleSessionPinned(_ sessionID: String) {
        guard !sessionID.isEmpty else { return }
        sessionPinnedAt = scopedPreferences.setSessionPinned(
            sessionPinnedAt[sessionID] == nil,
            sessionID: sessionID
        )
    }

    /// A recents row carries its own workspace and mode, so one switch lands on
    /// the right engine binding — no mode change first, session pick second.
    private func openRecentSession(scope: ConversationScope, mode: SessionMode, sessionID: String,
                                   transcriptPath: String?) {
        guard let transcriptPath, !transcriptPath.isEmpty, (transcriptPath as NSString).isAbsolutePath else {
            projectStore.errorMessage = ProjectRepositoryError.missingSessionTranscriptPath.localizedDescription
            return
        }
        requestScopeNavigation(
            to: scope,
            mode: mode,
            resumeSessionID: sessionID,
            selectedTranscriptPath: transcriptPath
        )
    }

    /// Scheduled results that finished after the user last opened the page.
    private var newCronResultCount: Int {
        let seen = cronResultsSeenAt ?? .distantPast
        return cronRepository.state.history.filter { run in
            guard run.sessionID != nil, let finished = run.finishedAtMs else { return false }
            return Date(timeIntervalSince1970: TimeInterval(finished) / 1_000) > seen
        }.count
    }

    private func markCronResultsSeen() {
        let now = Date()
        cronResultsSeenAt = now
        scopedPreferences.setCronResultsSeenAt(now)
    }

    private func requestContinueInMode(
        scope: ConversationScope,
        sessionID: String,
        sourceMode: SessionMode,
        targetMode: SessionMode
    ) {
        guard !sessionID.isEmpty, sourceMode != targetMode else { return }
        cancelPendingHandoff()
        let origin = ConversationSelectionSnapshot(
            scope: activeScope,
            mode: activeMode,
            activeSession: activeSession,
            confirmedSession: confirmedSession,
            pendingRestoreID: pendingSessionRestoreID,
            draft: draft
        )
        pendingSessionFork = PendingSessionFork(
            sourceScope: scope,
            sourceSessionID: sessionID,
            sourceMode: sourceMode,
            targetMode: targetMode,
            origin: origin,
            requestID: UUID().uuidString,
            navigationRevision: handoffNavigationRevision
        )
        if scope == activeScope, activeMode == sourceMode, confirmedSession == sessionID {
            triggerPendingForkIfReady(for: sessionID)
            return
        }
        guard switchScope(to: scope, mode: sourceMode, resumeSessionID: sessionID) else {
            pendingSessionFork = nil
            return
        }
        pendingSessionFork?.navigationRevision = handoffNavigationRevision
    }

    private func triggerPendingForkIfReady(for sessionID: String) {
        #if canImport(harness_runtimeFFI)
            guard var pending = pendingSessionFork,
                  !pending.submitted,
                  pending.sourceScope == activeScope,
                  pending.sourceMode == activeMode,
                  pending.sourceSessionID == sessionID,
                  confirmedSession == sessionID,
                  !source.model.sessionTransitionPending
            else { return }
            guard let entry = sessionHost.entry(for: source) else { return }
            pending.submitted = true
            pending.ownerEntryID = entry.id
            pendingSessionFork = pending
            let owner = entry.source
            Task { @MainActor in
                do {
                    try await owner.submitEngineCommand(.prepareSessionHandoff(requestId: pending.requestID, targetMode: pending.targetMode.dto))
                } catch {
                    rollbackPendingSessionFork(pending, message: error.localizedDescription)
                }
            }
        #endif
    }

    private func cancelPendingHandoff() {
        guard let pending = pendingSessionFork else { return }
        pendingSessionFork = nil
        #if canImport(harness_runtimeFFI)
        guard pending.submitted, let ownerID = pending.ownerEntryID,
              let owner = sessionHost.entries.first(where: { $0.id == ownerID })?.source else { return }
        Task { @MainActor in
            try? await owner.submitEngineCommand(.cancelSessionHandoff(requestId: pending.requestID))
        }
        #endif
    }

    private var isConversationSurfaceVisible: Bool {
        navigation.path.isEmpty && !navigation.settingsOpen && navigation.runningAppID == nil
            && remoteDestination == nil && externalImport == nil && externalActionApproval == nil
    }

    private func markHandoffNavigationChanged() {
        handoffNavigationRevision = UUID()
    }

    private func activatePendingHandoffDraft(_ handoff: SessionHandoffDraft) {
        guard !projectSwitching, pendingHandoffDrafts.contains(where: { $0.id == handoff.id }) else { return }
        _ = switchScope(to: .global, mode: handoff.mode, startNew: true, handoffDraft: handoff)
    }

    private func rollbackPendingSessionFork(
        _ pending: PendingSessionFork,
        message: String?
    ) {
        guard pendingSessionFork == pending else {
            if let message { projectStore.errorMessage = message }
            return
        }
        pendingSessionFork = nil
        if let request = pending.rollbackRequest,
           pending.navigationRevision == handoffNavigationRevision,
           isConversationSurfaceVisible {
            restoreConversationSelection(request.snapshot, message: message)
        } else if let message {
            projectStore.errorMessage = message
        }
    }

    private func restoreConversationSelection(
        _ snapshot: ConversationSelectionSnapshot,
        message: String?
    ) {
        pendingConversationSelectionRestore = PendingConversationSelectionRestore(
            snapshot: snapshot,
            message: message
        )
        applyPendingConversationSelectionRestoreIfPossible()
    }

    private func applyPendingConversationSelectionRestoreIfPossible() {
        guard !projectSwitching, let pending = pendingConversationSelectionRestore else { return }
        if let message = pending.message { projectStore.errorMessage = message }
        let snapshot = pending.snapshot
        let ownerID = snapshot.resumeSessionID ?? ""
        scopedPreferences.setDraft(snapshot.draft, scope: snapshot.scope, mode: snapshot.mode, ownerSessionID: ownerID)
        if activeScope == snapshot.scope, activeMode == snapshot.mode,
           canonicalSessionID(source.model.activeSessionId) == canonicalSessionID(ownerID) {
            draft = snapshot.draft
            pendingConversationSelectionRestore = nil
            return
        }
        pendingConversationSelectionRestore = nil
        if snapshot.startsNewConversation { pendingBeginActionDraft = snapshot.draft }
        _ = switchScope(to: snapshot.scope, mode: snapshot.mode,
                        resumeSessionID: snapshot.resumeSessionID, startNew: snapshot.startsNewConversation)
    }

    private func restoredSessionID(scope: ConversationScope, mode: SessionMode) -> String {
        return ConversationModeRestorePolicy.sessionID(
            mode: mode,
            scoped: scopedPreferences.storedActiveSessionID(scope: scope, mode: mode),
            sessions: scope == .scheduled ? [] : (scope.projectID.flatMap { id in projectStore.projects.first(where: { $0.id == id })?.sessions } ?? projectStore.globalSessions)
        )
    }

    private func persistConversationScope() {
        scopedPreferences.setDraft(draft, scope: activeScope, mode: activeMode, ownerSessionID: draftOwnerSessionID)
        persistActiveSessionIfRestorable(confirmedSession, scope: activeScope, mode: activeMode)
    }

    private func isRestorableSession(_ sessionID: String, scope: ConversationScope, mode: SessionMode) -> Bool {
        sessionID.isEmpty || scope == .scheduled ||
            (try? projectStore.transcriptPathForSession(sessionID, in: scope, mode: mode)) != nil
    }

    private func persistActiveSessionIfRestorable(_ sessionID: String, scope: ConversationScope, mode: SessionMode) {
        guard isRestorableSession(sessionID, scope: scope, mode: mode) else { return }
        scopedPreferences.setActiveSessionID(sessionID, scope: scope, mode: mode)
    }

    private func handleScenePhase(_ oldPhase: ScenePhase, _ phase: ScenePhase) {
        sessionHost.setScenePhase(phase)
        ProviderAccountRepository.shared.setForeground(phase == .active)
        switch phase {
        case .background:
            persistConversationScope()
            Task {
                await VoicePreviewPlayback.shared.stop()
                await VoiceAudioSessionCoordinator.shared.suspendForBackground()
            }
            Task { await cronRepository.handleSceneDidEnterBackground() }
        case .active:
            Task { await VoiceAudioSessionCoordinator.shared.resumeAfterForeground() }
            Task { await cronRepository.handleSceneBecameActive() }
            Task { await consumePendingAppActions() }
        default: break
        }
    }

    private func consumePendingAppActions() async {
        let actions = await LingxiAppActionStore.shared.drain()
        for action in actions {
            parkRefusedAppAction(action, applied: applyAppAction(action))
        }
    }

    /// Park an action the session-mutation gate refused instead of losing it.
    private func parkRefusedAppAction(_ action: LingxiAppAction, applied: Bool) {
        guard !applied else { approvedExternalActions.removeAll { $0 == action }; return }
        if !deferredAppActions.contains(action) { deferredAppActions.append(action) }
        if deferredAppActions.count > Self.maxDeferredAppActions {
            deferredAppActions.removeFirst(
                deferredAppActions.count - Self.maxDeferredAppActions
            )
        }
    }

    /// Re-run whatever the mutation gate refused earlier. Called from the
    /// `onChange` hooks on the three gate inputs, so a notification tapped
    /// while a durable turn still owned the engine slot lands as soon as that
    /// turn resolves rather than being silently discarded.
    private func retryDeferredAppActions() {
        guard !deferredAppActions.isEmpty else { return }
        let pending = deferredAppActions
        deferredAppActions = []
        for action in pending {
            parkRefusedAppAction(action, applied: applyAppAction(action))
        }
    }

    /// Returns `false` when the session-mutation gate refused the action, so
    /// the caller can retry it later. `true` means the action was consumed
    /// (including the cases that never touch session selection).
    @discardableResult
    private func applyAppAction(_ action: LingxiAppAction, approvedDraftChange: Bool = false) -> Bool {
        // A remote overlay preserves the local source and draft. It also works
        // when the local engine has not booted or is waiting for an answer.
        if case let .canonical(target) = action, ["remote", "team"].contains(target.parts.first ?? "") {
            guard app.setupDone else { return false }
            return applyCanonicalLink(target)
        }
        guard app.setupDone, externalEngineReady, !projectSwitching, !source.model.sessionTransitionPending,
              !navigation.settingsOpen, externalImport == nil, externalActionApproval == nil else { return false }
        if !approvedDraftChange && !approvedExternalActions.contains(action) && (!draft.isEmpty || hasUnsavedConversationMedia || (hasUnsavedTerminalInput && navigation.path.contains(where: { if case .terminal = $0 { return true }; return false })) || cronRepository.hasUnsavedChanges) {
            externalActionApproval = action
            return true
        }

        // Resolve a session's fixed owner before dismissing the current
        // navigation. A conflicting link must not move the user's view.
        if case let .canonical(target) = action, target.parts.first == "session" {
            return applyCanonicalLink(target)
        }

        externalAsyncRouteID = UUID()
        navigation.closeSidebar()
        navigation.closeSettings()
        navigation.path.removeAll()

        switch action {
        case let .canonical(target):
            return applyCanonicalLink(target)
        case .openApp:
            return true
        case .newConversation:
            return beginAppIntegratedConversation(draftText: "")
        case let .ask(question):
            let trimmed = question.trimmingCharacters(in: .whitespacesAndNewlines)
            return beginAppIntegratedConversation(draftText: trimmed)
        // Older notification turn counters are process-local. Canonical message
        // links resolve persisted UUIDs after the exact local session resumes.
        case let .openConversation(sessionID, _, workspaceKey, mode):
            let targetScope: ConversationScope
            if let workspaceKey {
                guard let parsedScope = ConversationScope(workspaceKey: workspaceKey) else {
                    return true
                }
                targetScope = parsedScope
            } else {
                targetScope = activeScope
            }
            switch targetScope {
            case .remote: return true
            case .global, .scheduled:
                break
            case let .project(projectID):
                guard projectStore.projects.contains(where: { $0.record.id == projectID }) else {
                    return true
                }
            }
            if targetScope == activeScope {
                let currentSessionID = source.model.activeSessionId.isEmpty
                    ? confirmedSession
                    : source.model.activeSessionId
                if canonicalSessionID(currentSessionID) == canonicalSessionID(sessionID), activeMode == mode { return true }
            }
            return switchScope(
                to: targetScope,
                mode: mode,
                resumeSessionID: sessionID
            )
        case let .openTerminal(sessionID, initialCommand):
            navigation.openTerminal(
                sessionID: sessionID,
                initialCommand: initialCommand,
                projectID: projectStore.activeProjectId
            )
            return true
        }
    }

    private func applyCanonicalLink(_ target: CanonicalDeepLink) -> Bool {
        let parts = target.parts
        if parts.count == 3, parts[0] == "remote" {
            navigation.closeSidebar()
            remoteDestination = RemoteControlDestination(hostID: parts[1], sessionID: parts[2])
            return true
        }
        let id = parts.last ?? ""
        var requestedScope = activeScope
        if let workspace = target.params["workspace"] {
            guard let scope = ConversationScope(workspaceKey: workspace) else { return missingLinkedResource("workspace") }
            if case let .project(projectID) = scope, !projectStore.projects.contains(where: { $0.record.id == projectID }) { return missingLinkedResource("workspace") }
            requestedScope = scope
        }
        if target.needsContextRebind(workspace: activeScope.workspaceKey, mode: activeMode.rawValue) {
            guard switchScope(to: requestedScope, mode: target.params["mode"].flatMap(SessionMode.init(rawValue:)) ?? activeMode) else { return false }
            return false
        }
        switch parts[0] {
        case "session":
            let indexedRows = requestedScope.projectID.flatMap { projectID in
                projectStore.projects.first(where: { $0.id == projectID })?.sessions
            } ?? (requestedScope == .global ? projectStore.globalSessions : [])
            let indexedMode = indexedRows.first(where: {
                canonicalSessionID($0.sessionId) == canonicalSessionID(id)
            })?.mode
            let liveMode = requestedScope == activeScope
                ? source.model.engineSessions.first(where: { canonicalSessionID($0.id) == canonicalSessionID(id) })?.mode
                : nil
            let currentMode = requestedScope == activeScope && canonicalSessionID(source.model.activeSessionId) == canonicalSessionID(id)
                ? activeMode : nil
            guard let mode = target.params["mode"].flatMap(SessionMode.init(rawValue:)) else {
                externalLinkError = String(localized: "deep_link_conversation_workspace_missing")
                return true
            }
            if let knownMode = indexedMode ?? liveMode ?? currentMode, knownMode != mode {
                externalLinkError = String(localized: "deep_link_conversation_workspace_missing")
                return true
            }
            if target.params["workspace"] == nil || target.params["workspace"] == activeScope.workspaceKey {
                guard source.model.engineSessionsLoaded else { return false }
                guard source.model.engineSessions.contains(where: { $0.id == id }) || source.model.activeSessionId == id else {
                    externalLinkError = String(localized: "deep_link_conversation_workspace_missing"); return true
                }
            }
            let applied = applyAppAction(.openConversation(sessionID: id, turnID: nil, workspaceKey: target.params["workspace"], mode: mode), approvedDraftChange: true)
            if applied { pendingLinkedSessionID = id }
            if applied, let raw = target.params["message"], let uuid = UUID(uuidString: raw) {
                pendingMessageLink = (id, uuid)
                resolvePendingMessageLink()
            }
            return applied
        case "new":
            let text = target.payload["prompt"] as? String ?? ""
            let newMode = target.newConversationMode(defaultMode: activeMode)
            if let workspace = target.params["workspace"], let scope = ConversationScope(workspaceKey: workspace) {
                pendingBeginActionDraft = text
                let accepted = switchScope(to: scope, mode: newMode, startNew: true)
                if !accepted { pendingBeginActionDraft = nil }
                return accepted
            }
            return beginAppIntegratedConversation(draftText: text, mode: newMode)
        case "project":
            guard projectStore.projects.contains(where: { $0.record.id == id }) else { return missingLinkedResource("project") }
            navigation.openProject(id)
        case "settings":
            guard let entry = DesktopSettingsEntry.all.first(where: { $0.id == id }) else { return missingLinkedResource("settings page") }
            navigation.showSettings(entry.page)
        case "provider":
            if target.params["scope"] == nil, let profile = providerRepository.profiles.first(where: { $0.id == id }) {
                navigation.showSettings(.providerEdit(.init(.llm), profile.id))
            } else { navigation.showSettings(.linkedProvider(id, scope: target.params["scope"])) }
        case "extensions":
            if parts[1] == "plugin" {
                navigation.showSettings(.pluginDetail(id))
            } else if parts[1] == "mcp" {
                navigation.showSettings(.linkedMcp(id, scope: target.params["scope"]))
            } else { navigation.showSettings(.linkedSkill(id, scope: target.params["scope"])) }
        case "cron":
            if parts[1] == "run" { navigation.openCronRun(id) }
            else { navigation.openCron(scopeID: target.params["scope"] ?? activeScope.projectID ?? globalCronScopeID, taskID: id) }
        case "terminal":
            navigation.openTerminal(sessionID: id, initialCommand: target.payload["command"] as? String, projectID: activeScope.projectID)
        case "team":
            navigation.openTeams(invite: target.params["code"])
        case "apps":
            let requestID = externalAsyncRouteID
            Task { @MainActor in
                await LocalApps.initializeOnce()
                await LocalApps.library.refresh()
                guard requestID == externalAsyncRouteID else { return }
                if LocalApps.library.record(id) != nil {
                    if target.params["review"] == "agent" {
                        LocalApps.library.browserReviewAppID = id
                        if let runID = target.params["runId"],
                           let revision = target.params["revision"].flatMap(UInt64.init) {
                            LocalApps.library.queueBrowserAgentWake(
                                appID: id,
                                revision: revision,
                                runID: runID
                            )
                        }
                    }
                    navigation.runApp(id)
                } else {
                    _ = missingLinkedResource("installed app")
                }
            }
        case "market": navigation.openMarketApp(id)
        case "import":
            guard target.payloadJSON != nil else { externalLinkError = String(localized: "deep_link_import_no_configuration"); return true }
            externalImport = target
        case "runtime":
            if parts[1] == "agent" {
                guard source.model.agentSummaries.contains(where: { $0.id == id }) else { return missingLinkedResource("agent") }
                source.selectAgent(id)
            } else {
                source.model.deepLinkRuntimeTarget = target
            }
        default: return missingLinkedResource("destination")
        }
        return true
    }

    private func missingLinkedResource(_ kind: String) -> Bool {
        externalLinkError = String(localized: "deep_link_target_missing")
        return true
    }

    private func resolvePendingMessageLink() {
        if pendingLinkedSessionID == source.model.activeSessionId, !source.model.sessionTransitionPending { pendingLinkedSessionID = nil }
        guard let pending = pendingMessageLink,
              source.model.activeSessionId == pending.sessionID,
              !source.model.sessionTransitionPending else { return }
        if let message = source.model.messages.first(where: { $0.transcriptUUID == pending.uuid }) {
            source.model.deepLinkScrollRowID = "message:\(message.id.uuidString)"
            source.model.deepLinkScrollRevision &+= 1
            pendingMessageLink = nil
        } else if let toolID = source.model.transcriptUUIDToolIDs[pending.uuid],
                  let group = ConversationRenderLayout.timelineGroups(source.model.items).first(where: { group in
                      group.rows.contains { if case let .tool(_, tool) = $0 { return tool.id == toolID }; return false }
                  }), let firstToolID = group.rows.compactMap({ row -> String? in if case let .tool(_, tool) = row { return tool.id }; return nil }).first {
            source.model.deepLinkScrollRowID = ConversationTimelineView.toolSegmentID(firstToolID: firstToolID)
            source.model.deepLinkScrollRevision &+= 1
            pendingMessageLink = nil
        } else if !projectSwitching && pendingSessionRestoreID == nil {
            pendingMessageLink = nil
            externalLinkError = String(localized: "deep_link_message_missing")
        }
    }

    private func handleIncomingURL(_ url: URL) {
        guard let action = LingxiDeepLink.action(from: url) else {
            // Account sign-in callbacks belong to the app delegate's SDK handlers.
            if url.scheme?.lowercased() == "lingxi" || (url.scheme == "https" && url.host == "lingxios.net") {
                externalLinkError = String(localized: "deep_link_invalid")
            }
            return
        }
        let now = Date()
        if let previous = lastExternalURLAction, previous.action == action, now.timeIntervalSince(previous.receivedAt) < 2 { return }
        lastExternalURLAction = (action, now)
        parkRefusedAppAction(action, applied: applyAppAction(action))
    }

    /// Draft text waiting to be loaded into the composer once a MODE-PINNING
    /// `switchScope` (below) finishes minting the fresh conversation it was
    /// asked to start. The text is left for the user to review, not auto-sent.
    @State private var pendingBeginActionDraft: String?

    @discardableResult
    private func beginAppIntegratedConversation(draftText: String, mode: SessionMode? = nil) -> Bool {
        pendingBeginActionDraft = draftText
        let accepted = switchScope(to: .global, mode: mode ?? activeMode, startNew: true)
        if !accepted { pendingBeginActionDraft = nil }
        return accepted
    }

    private func continueLocalAppDevelopment(_ app: UnifiedAppRecord) {
        guard let initiatingMode = requestLocalAppDevelopmentMode() else { return }
        guard draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              !hasUnsavedConversationMedia, !projectSwitching else {
            externalLinkError = "Finish or save the current conversation draft before opening app development."
            return
        }
        let context = beginLocalAppNavigation(key: "app:" + app.appId)
        Task { @MainActor in
            do {
                var sessions: [String: LocalAppAuthoringSessionBinding] = [:]
                for project in projectStore.projects {
                    for row in project.sessions {
                        sessions[canonicalSessionID(row.sessionId)] = .init(
                            workspaceId: project.record.id,
                            sourceDirectory: row.workspaceDirectory ?? project.record.activeDirectory,
                            projectRef: .mobileProject(project.record.id),
                            mode: row.mode.rawValue
                        )
                    }
                }
                let result = try await LocalApps.library.prepareAuthoring(app.appId, sessions: sessions, mode: initiatingMode)
                guard result.action != "sourceMissing" else { throw LocalAppServiceFailure("SOURCE_MISSING", "Source folder is missing. Import an editable copy before continuing development.") }
                guard let sourceID = result.sourceId, let sourcePath = result.sourcePath else {
                    throw LocalAppServiceFailure("INVALID_AUTHORING", "The app service did not return its source receipt.")
                }
                try UnifiedLocalApps.validateAuthoringSource(sourceID: sourceID,
                    sourceDirectory: result.sourceDirectory, sourcePath: sourcePath)
                var authoring = result.authoring
                if let sessionID = result.authoringSessionId {
                    authoring?.authoringSessionId = sessionID
                }
                let project: ProjectSnapshot? = {
                    guard let authoring, authoring.sourceDirectory == result.sourceDirectory,
                          let workspaceID = authoring.workspaceId,
                          case .mobileProject(let projectID)? = authoring.projectRef,
                          canonicalSessionID(workspaceID) == canonicalSessionID(projectID) else { return nil }
                    return projectStore.projects.first { $0.id == projectID && $0.record.activeDirectory == result.sourceDirectory }
                }()
                if let project, let sessionID = authoring?.authoringSessionId,
                   let row = project.sessions.first(where: { canonicalSessionID($0.sessionId) == canonicalSessionID(sessionID)
                       && $0.mode == .code && $0.workspaceDirectory == result.sourceDirectory }) {
                    let saved = try await LocalApps.library.exactAuthoringBinding(sourceDirectory: result.sourceDirectory,
                        projectID: project.id, sessionID: sessionID, mode: .code, expectedSourceID: sourceID)
                    let owner = LocalAppAuthoringOwner(sourceDirectory: result.sourceDirectory, projectID: project.id,
                                                       sessionID: sessionID, appID: app.appId)
                    guard owner.matches(saved) else { throw SessionHost.OwnershipError.sourceIdentityMismatch }
                    let request = LocalAppDevelopmentRequest(appId: app.appId, name: app.name,
                        sourceDirectory: result.sourceDirectory, sourceID: sourceID, sourceRequestID: nil,
                        authoring: authoring, prompt: "继续开发这个应用。")
                    let entry = try await prepareParkedExistingAuthoringOwner(project: project, row: row,
                                                                              request: request, context: context)
                    if isCurrentLocalAppNavigation(context),
                       localAppRegistrationOrder.isCurrent(context.registrationTicket) {
                        guard let transcriptPath = row.transcriptPath else { throw ProjectRepositoryError.missingSessionTranscriptPath }
                        let accepted = switchScope(to: entry.scope, mode: .code, resumeSessionID: sessionID,
                            selectedTranscriptPath: transcriptPath,
                            expectedAuthoringWorkspace: ExistingAuthoringWorkspace(sessionID: sessionID,
                                sourceDirectory: request.sourceDirectory, transcriptPath: transcriptPath),
                            retainedEntry: entry, sourceSnapshot: context.providerSnapshot,
                            onFailure: { error in reportLocalAppNavigationFailure(error, context: context) })
                        if !accepted {
                            throw LocalAppServiceFailure("CONVERSATION_BUSY", "The conversation cannot switch to the app source project right now.")
                        }
                    }
                    return
                }
                let intent = LocalAppSourceCreateIntent(requestId: UUID().uuidString.lowercased(),
                    projectId: project?.id ?? UUID().uuidString.lowercased(), mode: initiatingMode,
                    name: app.name, prompt: "继续开发这个应用。",
                    operation: .bind, appId: app.appId, sourceId: canonicalSessionID(sourceID),
                    sourceDirectory: result.sourceDirectory)
                try await projectStore.storeLocalAppSourceCreateIntent(intent)
                resumeLocalAppCreation(intent, context: context)
            } catch { reportLocalAppNavigationFailure(error, context: context) }
        }
    }

    private func queueLocalAppRestore(_ app: UnifiedAppRecord) {
        guard let initiatingMode = requestLocalAppDevelopmentMode() else { return }
        guard draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              !hasUnsavedConversationMedia, !projectSwitching else {
            externalLinkError = "Finish or save the current conversation draft before opening app development."
            return
        }
        guard let revision = Int(exactly: app.currentRevision), revision > 0 else {
            externalLinkError = "The app has no restorable release."
            return
        }
        let context = beginLocalAppNavigation(key: "app:" + app.appId)
        let intent = LocalAppSourceCreateIntent(requestId: UUID().uuidString.lowercased(),
            projectId: UUID().uuidString.lowercased(), mode: initiatingMode, name: app.name, prompt: "继续开发这个应用。",
            operation: .restore, appId: app.appId, revision: revision)
        Task { @MainActor in
            do {
                try await projectStore.storeLocalAppSourceCreateIntent(intent)
                resumeLocalAppCreation(intent, context: context)
            } catch { reportLocalAppNavigationFailure(error, context: context) }
        }
    }

    private func beginMarketRemixNavigation(key: String) -> LocalAppNavigationContext? {
        guard requestLocalAppDevelopmentMode() != nil else { return nil }
        guard draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              !hasUnsavedConversationMedia, !projectSwitching else {
            externalLinkError = "Finish or save the current conversation draft before opening app development."
            return nil
        }
        return beginLocalAppNavigation(key: key)
    }

    private func queueLocalAppMarketRemix(_ release: MarketDistribution) {
        let requestID = UUID().uuidString.lowercased()
        guard let context = beginMarketRemixNavigation(key: "market-request:" + requestID) else { return }
        Task { @MainActor in
            do { try await persistMarketRemix(release, requestID: requestID, context: context) }
            catch { reportLocalAppNavigationFailure(error, context: context) }
        }
    }

    private func queueInstalledLocalAppRemix(_ app: UnifiedAppRecord) {
        let requestID = UUID().uuidString.lowercased()
        guard let origin = app.marketOrigin, origin.remixedFrom == nil,
              let context = beginMarketRemixNavigation(key: "market-request:" + requestID) else { return }
        Task { @MainActor in
            do {
                let release = try await LocalApps.market.distribution(origin.appId, versionID: origin.releaseId)
                guard release.releaseDigest == origin.digest else { throw MarketFailure("RELEASE_CHANGED") }
                try await persistMarketRemix(release, requestID: requestID, context: context)
            } catch { reportLocalAppNavigationFailure(error, context: context) }
        }
    }

    private func persistMarketRemix(_ release: MarketDistribution, requestID: String,
                                    context: LocalAppNavigationContext) async throws {
        let configHome = URL(fileURLWithPath: MobileUserStorage.directories().configHome, isDirectory: true)
            .resolvingSymlinksInPath().standardizedFileURL
        let archivePath = configHome.appendingPathComponent("local-app-imports", isDirectory: true)
            .appendingPathComponent("market-" + requestID, isDirectory: true).appendingPathComponent("source.zip").path
        let input = try LocalApps.market.remixInput(release, requestID: requestID, archivePath: archivePath, mode: .code)
        let name = input.manifest.name.trimmingCharacters(in: .whitespacesAndNewlines)
        let intent = LocalAppSourceCreateIntent(requestId: requestID, projectId: UUID().uuidString.lowercased(),
            mode: .code, name: name, prompt: "", operation: .marketRemix,
            remixInputDigest: try ProjectRepository.localAppRemixInputDigest(input))
        try await projectStore.storeLocalAppRemixRequest(intent, input: input, configHome: configHome)
        // The captured Code owner completes even if another conversation is
        // selected before download, import, or native owner admission finishes.
        resumeLocalAppCreation(intent, context: context)
    }

    private func beginLocalAppNavigation(key: String) -> LocalAppNavigationContext {
        userNavigation.invalidate()
        localAppSelectionAuthority.invalidate()
        return LocalAppNavigationContext(generation: userNavigation.generation,
            presentationRevision: navigation.presentationRevision,
            selectionRevision: localAppSelectionAuthority.currentRevision(),
            sequence: localAppRegistrationOrder.issueNavigation(),
            providerSnapshot: providerRepository.makeLaunchSnapshot(),
            registrationTicket: localAppRegistrationOrder.issue(key: key),
            originatingSource: source, originatingEntry: sessionHost.entry(for: source))
    }

    private func reportLocalAppNavigationFailure(_ error: Error, context: LocalAppNavigationContext) {
        guard !(error is CancellationError) else { return }
        if isCurrentLocalAppNavigation(context) {
            externalLinkError = error.localizedDescription
        } else {
            let failure = ConversationError(kind: .host, message: error.localizedDescription)
            if let entry = context.originatingEntry { sessionHost.deliverOperationFailure(failure, to: entry) }
            else { context.originatingSource.model.error = failure }
        }
    }

    private func isCurrentLocalAppNavigation(_ context: LocalAppNavigationContext) -> Bool {
        userNavigation.isCurrent(context.generation) && navigation.presentationRevision == context.presentationRevision
    }

    private func captureLocalAppActionPresentation(route: AppRoute) -> LocalAppActionPresentation {
        let revision = navigation.presentationRevision
        let generation = userNavigation.generation
        let owner = source
        let entry = sessionHost.entry(for: owner)
        return LocalAppActionPresentation(mayPresent: {
            navigation.mayPresent(capturedRevision: revision, on: route) && userNavigation.isCurrent(generation)
        }, reportOwnerFailure: { message in
            let failure = ConversationError(kind: .host, message: message)
            if let entry { sessionHost.deliverOperationFailure(failure, to: entry) }
            else { owner.model.error = failure }
        })
    }

    private func requestLocalAppDevelopmentMode() -> SessionMode? {
        do {
            try LocalAppDevelopmentAdmission.requireCode(activeMode)
            return activeMode
        } catch {
            externalLinkError = error.localizedDescription
            return nil
        }
    }

    private func queueLocalAppCreation(name: String, prompt: String) async throws {
        let initiatingMode = activeMode
        try LocalAppDevelopmentAdmission.requireCode(initiatingMode)
        guard draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              !hasUnsavedConversationMedia, !projectSwitching else {
            throw LocalAppServiceFailure("CONVERSATION_BUSY", "Finish or save the current conversation draft before opening app development.")
        }
        let cleanName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanName.isEmpty, cleanName.unicodeScalars.count <= 80 else {
            throw LocalAppServiceFailure("INVALID_NAME", String(localized: "local_app_authoring_name_invalid"))
        }
        let intent = LocalAppSourceCreateIntent(requestId: UUID().uuidString.lowercased(),
            projectId: UUID().uuidString.lowercased(), mode: initiatingMode, name: cleanName, prompt: prompt)
        let context = beginLocalAppNavigation(key: "source-request:" + intent.requestId)
        // This must reach durable storage before native can create a source.
        do { try await projectStore.storeLocalAppSourceCreateIntent(intent) }
        catch {
            if !isCurrentLocalAppNavigation(context) { reportLocalAppNavigationFailure(error, context: context) }
            throw error
        }
        resumeLocalAppCreation(intent, context: context)
    }

    private func resumeLocalAppCreation(_ intent: LocalAppSourceCreateIntent, context supplied: LocalAppNavigationContext? = nil) {
        if supplied == nil, requestLocalAppDevelopmentMode() == nil { return }
        if supplied == nil, !(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty &&
                              !hasUnsavedConversationMedia && !projectSwitching) {
            externalLinkError = "Finish or save the current conversation draft before opening app development."
            return
        }
        let context = supplied ?? beginLocalAppNavigation(key: intent.appId.map { "app:" + $0 } ?? "source-request:" + intent.requestId)
        guard processingSourceIntents.insert(intent.requestId).inserted else { return }
        Task { @MainActor in
            defer { processingSourceIntents.remove(intent.requestId) }
            do {
                // The service returns the original proof if this exact request
                // was committed before a crash or an unknown response.
                guard try await projectStore.localAppSourceCreateIntent(requestID: intent.requestId) == intent else {
                    throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app creation request changed.")
                }
                #if canImport(harness_runtimeFFI)
                try await drainFailedPreparation(requestID: intent.requestId)
                #endif
                if projectStore.projects.contains(where: { $0.id == intent.projectId }) {
                    let ownerIntents = try await projectStore.localAppAuthoringIntents(projectID: intent.projectId)
                    if ownerIntents.contains(where: { $0.intentId == intent.requestId }),
                       isCurrentLocalAppNavigation(context),
                       let (authority, revision) = localAppRegistrationOrder.reserveProjectSelection(
                            projectID: intent.projectId, sequence: context.sequence) {
                        do {
                            try await projectStore.selectLocalAppAuthoringIntent(projectID: intent.projectId, intentID: intent.requestId,
                                authority: localAppSelectionAuthority, revision: context.selectionRevision,
                                projectAuthority: authority, projectRevision: revision)
                            if isCurrentLocalAppNavigation(context) {
                                requestUserNavigation(.recoverAuthoring(intent.projectId))
                                return
                            }
                        } catch WorkspaceWriteAuthority.Failure.stale {
                            // Another navigation owns selection now. Finish
                            // the captured request without changing it.
                        }
                    }
                }
                let created: LocalAppCreatedAuthoringSource
                var resultingAppID = intent.appId
                switch intent.operation {
                case .create:
                    created = try await LocalApps.library.createAuthoring(name: intent.name, requestID: intent.requestId, mode: intent.mode)
                case .restore:
                    guard let appID = intent.appId, let revision = intent.revision else {
                        throw LocalAppServiceFailure("INVALID_AUTHORING", "The restore app or revision is missing.")
                    }
                    created = try await LocalApps.library.restoreAuthoringSource(appID: appID, revision: revision,
                                                                                 requestID: intent.requestId, mode: intent.mode)
                case .bind:
                    guard let appID = intent.appId, let sourceID = intent.sourceId, let sourceDirectory = intent.sourceDirectory else {
                        throw LocalAppServiceFailure("INVALID_AUTHORING", "The app source binding is incomplete.")
                    }
                    created = try await LocalApps.library.bindAuthoringSource(requestID: intent.requestId, sourceID: sourceID,
                        sourceDirectory: sourceDirectory, appID: appID, mode: intent.mode)
                case .marketRemix:
                    let configHome = URL(fileURLWithPath: MobileUserStorage.directories().configHome, isDirectory: true)
                        .resolvingSymlinksInPath().standardizedFileURL
                    let (input, archive) = try await projectStore.localAppRemixInput(intent, configHome: configHome)
                    let result = try await LocalAppRemixSubmission.run(mode: intent.mode, submit: {
                        try await LocalApps.library.importMarketRemix(input, archivePath: archive.path, mode: intent.mode)
                    }, restoreArchive: {
                        try await LocalApps.market.downloadRemixArchive(input, intent: intent,
                                                                        store: projectStore, configHome: configHome)
                    })
                    resultingAppID = result.record.appId
                    created = LocalAppCreatedAuthoringSource(sourceId: result.sourceId, requestId: result.requestId,
                        sourceDirectory: result.sourceDirectory, sourcePath: result.sourcePath)
                }
                let request = LocalAppDevelopmentRequest(appId: resultingAppID, name: intent.name,
                    sourceDirectory: created.sourceDirectory, sourceID: created.sourceId,
                    sourceRequestID: created.requestId, authoring: nil, prompt: intent.prompt)
                let entry = try await prepareParkedLocalAppOwner(request: request, intent: intent, context: context,
                                                                 maySetCurrent: supplied != nil)
                if isCurrentLocalAppNavigation(context), !projectSwitching {
                    _ = switchScope(to: entry.scope, mode: entry.mode, resumeSessionID: entry.sessionID,
                                    retainedEntry: entry, sourceSnapshot: context.providerSnapshot)
                }
            } catch { reportLocalAppNavigationFailure(error, context: context) }
        }
    }

    /// A previously bound owner remains independent of the foreground after
    /// the user moves away during its read-only prepare. Reopen only its exact
    /// indexed transcript; never interrupt an already retained source.
    private func prepareParkedExistingAuthoringOwner(project: ProjectSnapshot, row: ProjectSessionSummary,
                                                     request: LocalAppDevelopmentRequest,
                                                     context: LocalAppNavigationContext) async throws -> SessionHost.Entry {
        #if canImport(harness_runtimeFFI)
        let scope = ConversationScope.project(project.id)
        let sessionID = canonicalSessionID(row.sessionId)
        guard row.mode == .code, UUID(uuidString: sessionID) != nil,
              let transcriptPath = row.transcriptPath, !transcriptPath.isEmpty,
              (transcriptPath as NSString).isAbsolutePath else {
            throw ProjectRepositoryError.missingSessionTranscriptPath
        }
        try await drainFailedPreparation(requestID: sessionID)
        if let entry = try sessionHost.entryForClaim(sessionID: sessionID, scope: scope, mode: .code) {
            try await prepareExistingAuthoringDraft(entry, request: request, transcriptPath: transcriptPath)
            return entry
        }
        try await cronRepository.prepareOwnerAdmission()
        try await cronOwnerGate.waitUntilAvailable(sessionID: sessionID)
        if let entry = try sessionHost.entryForClaim(sessionID: sessionID, scope: scope, mode: .code) {
            try await prepareExistingAuthoringDraft(entry, request: request, transcriptPath: transcriptPath)
            return entry
        }
        guard !sessionHost.hasPreparingClaim(sessionID: sessionID),
              try projectStore.transcriptPathForSession(sessionID, in: scope, mode: .code) == transcriptPath else {
            throw SessionHost.OwnershipError.conflictingOwner
        }
        let replacement = try await makeSource(scope: scope, snapshot: context.providerSnapshot, mode: .code,
            sessionID: sessionID, selectedTranscriptPath: transcriptPath)
        defer { sessionHost.endPreparingClaim(of: replacement) }
        sessionHost.protectIdentity(of: replacement, scope: scope, mode: .code, sessionID: sessionID)
        installExternalEventHandler(on: replacement)
        var retained = false
        do {
            try await replacement.prepare()
            guard let engine = replacement as? EngineConversationSource,
                  engine.pendingRecoveredAuthoringOwner != nil else {
                throw LocalAppServiceFailure("ENGINE_UNAVAILABLE", "App development requires the local conversation engine.")
            }
            let restoredDraft = scopedPreferences.draft(scope: scope, mode: .code, ownerSessionID: sessionID)
            let entry = try sessionHost.retain(source: replacement, scope: scope, mode: .code,
                                               sessionID: sessionID, workspace: request.sourceDirectory, draft: restoredDraft)
            retained = true
            entry.draftSourceCodeUnits = scopedPreferences.draftSourceCodeUnits(scope: scope, mode: .code, ownerSessionID: sessionID)
            entry.handoffReference = scopedPreferences.handoffReference(sessionID: sessionID, scope: scope, mode: .code)
            let draftRevision = entry.draftRevision
            do {
                try await confirmRecoveredAuthoringOwner(engine, entry: entry)
            } catch {
                await discardFailedUnselectedAuthoringEntry(entry, engine: engine)
                throw error
            }
            guard let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: entry),
                  snapshot.state.transcriptPath == transcriptPath,
                  snapshot.state.cwd == request.sourceDirectory else {
                // Keep a valid existing owner and its actual /cd state. Never
                // inject an app prompt into a different workspace.
                engine.model.error = ConversationError(kind: .host,
                    message: "This conversation is no longer in the app source directory.")
                throw LocalAppServiceFailure("AUTHORING_CHANGED", "This conversation is no longer in the app source directory.")
            }
            if sessionHost.entry(for: engine) === entry, entry.draftRevision == draftRevision,
               !scopedPreferences.hasDraft(scope: scope, mode: .code, ownerSessionID: sessionID) {
                seedLocalAppDraft(request, entry: entry)
            }
            return entry
        } catch {
            if !retained {
                await quarantineFailedPreparation(replacement, requestID: sessionID,
                                                  scope: scope, mode: .code, sessionID: sessionID)
            }
            throw error
        }
        #else
        throw LocalAppServiceFailure("ENGINE_UNAVAILABLE", "App development requires the local conversation engine.")
        #endif
    }

    /// Finish an already admitted Code request without changing whichever
    /// conversation the user selected while its source operation was pending.
    private func prepareParkedLocalAppOwner(request: LocalAppDevelopmentRequest, intent: LocalAppSourceCreateIntent,
                                            context: LocalAppNavigationContext, maySetCurrent: Bool) async throws -> SessionHost.Entry {
        #if canImport(harness_runtimeFFI)
        guard intent.mode == .code, request.sourceRequestID.map(canonicalSessionID) == intent.requestId,
              (intent.operation == .marketRemix ? (intent.appId == nil && request.appId?.isEmpty == false)
                                                 : request.appId == intent.appId) else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app source request changed.")
        }
        let project: ProjectSnapshot
        var savedIntent: LocalAppAuthoringIntent
        if let existing = projectStore.projects.first(where: { $0.id == intent.projectId }) {
            guard existing.record.activeDirectory == request.sourceDirectory else {
                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved project now uses a different source.")
            }
            project = existing
            let matching = try await projectStore.localAppAuthoringIntents(projectID: intent.projectId)
                .filter { $0.intentId == intent.requestId }
            guard matching.count <= 1 else { throw SessionHost.OwnershipError.conflictingOwner }
            savedIntent = matching.first ?? LocalAppAuthoringIntent(intentId: intent.requestId,
                sourceId: canonicalSessionID(request.sourceID), sourceRequestId: intent.requestId,
                sourceDirectory: request.sourceDirectory, projectId: intent.projectId, appId: request.appId)
        } else {
            let created = try await projectStore.createLocalAppAuthoringProject(name: request.name,
                sourceDirectory: request.sourceDirectory, sourceID: canonicalSessionID(request.sourceID),
                sourceRequestID: intent.requestId, appID: request.appId, projectID: intent.projectId, intentID: intent.requestId)
            project = created.0
            savedIntent = created.1
        }
        guard savedIntent.sourceId == canonicalSessionID(request.sourceID),
              savedIntent.sourceRequestId == intent.requestId, savedIntent.sourceDirectory == request.sourceDirectory,
              savedIntent.projectId == project.id, savedIntent.appId == request.appId else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app intent no longer matches its source.")
        }
        try await projectStore.storeLocalAppAuthoringIntent(savedIntent)
        if isCurrentLocalAppNavigation(context),
           let (authority, revision) = localAppRegistrationOrder.reserveProjectSelection(projectID: project.id,
                                                                                           sequence: context.sequence) {
            do {
                try await projectStore.selectLocalAppAuthoringIntent(projectID: project.id, intentID: savedIntent.intentId,
                    authority: localAppSelectionAuthority, revision: context.selectionRevision,
                    projectAuthority: authority, projectRevision: revision)
            } catch WorkspaceWriteAuthority.Failure.stale {
                // Selection belongs to a newer navigation; this admitted
                // owner still completes in the background.
            }
        }
        if let sessionID = savedIntent.authoringSessionId {
            if let retained = try sessionHost.entryForClaim(sessionID: sessionID, scope: .project(project.id), mode: .code) {
                if retained.source.model.authoringBindingPending {
                    try await validatePendingAuthoringEntry(retained, intent: savedIntent)
                } else {
                    let binding = try await LocalApps.library.exactAuthoringBinding(
                        sourceDirectory: savedIntent.sourceDirectory, projectID: project.id, sessionID: sessionID,
                        mode: .code, expectedSourceID: savedIntent.sourceId)
                    let owner = LocalAppAuthoringOwner(sourceDirectory: savedIntent.sourceDirectory, projectID: project.id,
                                                       sessionID: sessionID, appID: savedIntent.appId)
                    guard owner.matches(binding), let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: retained),
                          savedIntent.matchesRetainedOwner(scope: retained.scope, mode: retained.mode,
                                                           sessionID: retained.sessionID, workspace: snapshot.state),
                          sessionHost.entry(for: retained.source) === retained else {
                        throw SessionHost.OwnershipError.sourceIdentityMismatch
                    }
                }
                return retained
            }
            // A process can stop after persisting the native UUID but before
            // publishing the session index or registering its receipt. Reuse
            // that exact owner in the background, never mint a second UUID.
            try await cronRepository.prepareOwnerAdmission()
            try await cronOwnerGate.waitUntilAvailable(sessionID: sessionID)
            try await reconcileAuthoringIndexIfNeeded(project: project, intent: savedIntent)
            if let retained = try sessionHost.entryForClaim(sessionID: sessionID, scope: .project(project.id), mode: .code) {
                try await validatePendingAuthoringEntry(retained, intent: savedIntent)
                return retained
            }
            guard !sessionHost.hasPreparingClaim(sessionID: sessionID), let transcriptPath = savedIntent.transcriptPath else {
                throw SessionHost.OwnershipError.conflictingOwner
            }
            let scope = ConversationScope.project(project.id)
            let replacement = try await makeSource(scope: scope, snapshot: context.providerSnapshot, mode: .code,
                sessionID: sessionID, selectedTranscriptPath: transcriptPath)
            defer { sessionHost.endPreparingClaim(of: replacement) }
            sessionHost.protectIdentity(of: replacement, scope: scope, mode: .code, sessionID: sessionID)
            installExternalEventHandler(on: replacement)
            var retained = false
            do {
                try await replacement.prepare()
                guard let engine = replacement as? EngineConversationSource,
                      engine.pendingRecoveredAuthoringOwner != nil else {
                    throw LocalAppServiceFailure("ENGINE_UNAVAILABLE", "App development requires the local conversation engine.")
                }
                let restoredDraft = scopedPreferences.draft(scope: scope, mode: .code, ownerSessionID: sessionID)
                let entry = try sessionHost.retain(source: replacement, scope: scope, mode: .code,
                                                   sessionID: sessionID, workspace: savedIntent.sourceDirectory, draft: restoredDraft)
                retained = true
                entry.draftSourceCodeUnits = scopedPreferences.draftSourceCodeUnits(scope: scope, mode: .code, ownerSessionID: sessionID)
                entry.handoffReference = scopedPreferences.handoffReference(sessionID: sessionID, scope: scope, mode: .code)
                if !scopedPreferences.hasDraft(scope: scope, mode: .code, ownerSessionID: sessionID) {
                    seedLocalAppDraft(request, entry: entry)
                }
                do {
                    try await confirmRecoveredAuthoringOwner(engine, entry: entry)
                    return entry
                } catch {
                    await discardFailedUnselectedAuthoringEntry(entry, engine: engine)
                    throw error
                }
            } catch {
                if !retained {
                    await quarantineFailedPreparation(replacement, requestID: intent.requestId,
                                                      scope: scope, mode: .code, sessionID: sessionID)
                }
                throw error
            }
        }
        let pending = pendingAuthoringAttempts.filter { savedIntent.matchesInFlightAttempt($0.value.intent) }
        guard pending.count <= 1 else { throw SessionHost.OwnershipError.conflictingOwner }
        if let (id, _) = pending.first, let retained = sessionHost.entries.first(where: { $0.id == id }) {
            try await validatePendingAuthoringEntry(retained, intent: savedIntent)
            return retained
        }
        try await cronRepository.prepareOwnerAdmission()
        try await cronOwnerGate.waitUntilAvailable(sessionID: "")
        let scope = ConversationScope.project(project.id)
        let provisional = ProvisionalLocalAppGrant(intent: savedIntent)
        let replacement = try await makeSource(scope: scope, snapshot: context.providerSnapshot,
            mode: .code, provisionalAuthoring: provisional)
        defer { sessionHost.endPreparingClaim(of: replacement) }
        sessionHost.protectIdentity(of: replacement, scope: scope, mode: .code)
        installExternalEventHandler(on: replacement)
        var retained = false
        var preparedSessionID: String?
        do {
            try await replacement.prepare()
            guard let engine = replacement as? EngineConversationSource else {
                throw LocalAppServiceFailure("ENGINE_UNAVAILABLE", "App development requires the local conversation engine.")
            }
            let owner = LocalAppProvisionalAuthoringOwner(sourceDirectory: savedIntent.sourceDirectory,
                projectID: savedIntent.projectId, appID: savedIntent.appId)
            let workspace = try await engine.preparedProvisionalAuthoringWorkspace(owner)
            preparedSessionID = workspace.sessionId
            // Transfer the native claim to SessionHost before any asynchronous
            // index write. A failed write then leaves the same gated owner alive
            // and retryable instead of remounting its UUID.
            let entry = try sessionHost.retain(source: replacement, scope: scope, mode: .code,
                                               sessionID: workspace.sessionId, workspace: workspace.cwd, draft: "")
            retained = true
            pendingAuthoringAttempts[entry.id] = PendingAuthoringAttempt(request: request, project: project,
                intent: savedIntent, setCurrentOnFirstAttempt: maySetCurrent, registrationTicket: context.registrationTicket)
            seedLocalAppDraft(request, entry: entry)
            retryAuthoringBinding(for: entry)
            return entry
        } catch {
            if !retained {
                await quarantineFailedPreparation(replacement, requestID: intent.requestId,
                                                  scope: scope, mode: .code, sessionID: preparedSessionID)
            }
            throw error
        }
        #else
        throw LocalAppServiceFailure("ENGINE_UNAVAILABLE", "App development requires the local conversation engine.")
        #endif
    }

    private func seedLocalAppDraft(_ request: LocalAppDevelopmentRequest, entry: SessionHost.Entry) {
        guard entry.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              entry.attachment == nil, entry.draftSourceCodeUnits == nil, entry.handoffReference == nil else { return }
        let sessionID = entry.sessionID
        let ownerDescription = sessionID.isEmpty ? "" : " This ordinary authoring session is \(sessionID)."
        let text = """
        Use the local-app skill to develop \(request.name) in \(request.sourceDirectory). Use the schema 2 standalone LocalApp service.\(ownerDescription) Existing app ID: \(request.appId ?? "new app"). Publish locally only; publishing to the market is a separate user action.

        If the app uses an Agent, persist recoverable app state and the pending runId and threadId before submission. On reopen, read host task status and event or chat history, and reconnect to the same run and thread read-only. Do not automatically submit, resend or replay tools after an uncertain request or interruption. Show running only when the host confirms it, and show interrupted or unknown state and the action needed from the user; after the user reviews any side effects, a new attempt requires an explicit user action and a new runId. Only show completion when supported by the host's terminal result. iOS may suspend or terminate background work; do not promise indefinite execution.

        \(request.prompt)
        """
        entry.draft = text
        scopedPreferences.setDraft(text, scope: entry.scope, mode: entry.mode, ownerSessionID: sessionID)
        let hasOtherBlankOwner = sessionHost.entries.contains {
            $0 !== entry && $0.scope == entry.scope && $0.mode == entry.mode && $0.sessionID.isEmpty
        }
        if !sessionID.isEmpty && !isRestorableSession(sessionID, scope: entry.scope, mode: entry.mode),
           !hasOtherBlankOwner,
           !scopedPreferences.hasDraft(scope: entry.scope, mode: entry.mode, ownerSessionID: "") {
            scopedPreferences.setDraft(text, scope: entry.scope, mode: entry.mode, ownerSessionID: "")
        }
        if source === entry.source, sessionHost.selectedEntryID == entry.id { draft = text }
    }

    private func retryAuthoringBinding(for entry: SessionHost.Entry) {
        guard sessionHost.entry(for: entry.source) === entry else { return }
        if var attempt = pendingAuthoringAttempts[entry.id] {
            guard !attempt.inFlight else { return }
            attempt.inFlight = true
            pendingAuthoringAttempts[entry.id] = attempt
            entry.source.model.authoringBindingRetryAvailable = false
            Task { @MainActor in
                await finishLocalAppDevelopment(attempt.request, project: attempt.project, entry: entry)
            }
            return
        }
        #if canImport(harness_runtimeFFI)
        if source === entry.source, let engine = entry.source as? EngineConversationSource,
           engine.needsAuthoringMountAuthorization || engine.pendingRecoveredAuthoringOwner != nil {
            entry.source.model.authoringBindingRetryAvailable = false
            entry.source.model.error = nil
            if entry.sessionID.isEmpty { allowBlankAuthoringRetryFor = entry.id }
            sourceGeneration = UUID()
        }
        #endif
    }

    private func finishLocalAppDevelopment(
        _ request: LocalAppDevelopmentRequest, project: ProjectSnapshot,
        entry: SessionHost.Entry
    ) async {
        #if canImport(harness_runtimeFFI)
        defer {
            if var attempt = pendingAuthoringAttempts[entry.id] {
                attempt.inFlight = false
                pendingAuthoringAttempts[entry.id] = attempt
            }
        }
        do {
            guard let engine = entry.source as? EngineConversationSource, entry.mode == .code,
                  entry.scope == .project(project.record.id) else {
                throw LocalAppServiceFailure("ENGINE_UNAVAILABLE", "App development requires the local conversation engine.")
            }
            for _ in 0..<400 {
                if !engine.model.sessionTransitionPending, !engine.model.activeSessionId.isEmpty,
                   canonicalSessionID(engine.model.activeSessionId) == canonicalSessionID(entry.sessionID),
                   engine.model.workspaceState?.cwd == request.sourceDirectory { break }
                guard sessionHost.entry(for: engine) === entry else {
                    throw LocalAppServiceFailure("AUTHORING_CHANGED", "The authoring owner is no longer available.")
                }
                try await Task.sleep(for: .milliseconds(50))
            }
            let sessionID = entry.sessionID
            let owner = LocalAppAuthoringOwner(sourceDirectory: request.sourceDirectory, projectID: project.record.id,
                                               sessionID: sessionID, appID: request.appId)
            guard UUID(uuidString: sessionID) != nil,
                  canonicalSessionID(engine.model.activeSessionId) == canonicalSessionID(sessionID),
                  engine.model.workspaceState?.cwd == request.sourceDirectory,
                  !engine.model.sessionTransitionPending,
                  sessionHost.entry(for: engine) === entry,
                  let snapshot = await sessionHost.authenticatedWorkspaceSnapshot(for: entry),
                  snapshot.state.cwd == request.sourceDirectory,
                  entry.workspaceWriteAuthority.currentRevision() == snapshot.revision else {
                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The authoring workspace could not be authenticated.")
            }
            do {
                guard var attempt = pendingAuthoringAttempts[entry.id], let sourceRequestID = request.sourceRequestID,
                      attempt.intent.projectId == project.record.id,
                      attempt.intent.sourceDirectory == request.sourceDirectory,
                      attempt.intent.sourceId == canonicalSessionID(request.sourceID),
                      attempt.intent.sourceRequestId == canonicalSessionID(sourceRequestID) else {
                    throw LocalAppServiceFailure("AUTHORING_CHANGED", "The authoring intent no longer matches this owner.")
                }
                if attempt.intent.authoringSessionId == nil {
                    let previous = attempt.intent
                    attempt.intent.authoringSessionId = canonicalSessionID(sessionID)
                    attempt.intent.transcriptPath = snapshot.state.transcriptPath
                    try await projectStore.storeLocalAppAuthoringIntent(attempt.intent, replacing: previous)
                    guard sessionHost.entry(for: engine) === entry,
                          canonicalSessionID(engine.model.activeSessionId) == canonicalSessionID(sessionID) else {
                        throw LocalAppServiceFailure("AUTHORING_CHANGED", "The authoring owner changed before its intent was saved.")
                    }
                    pendingAuthoringAttempts[entry.id] = attempt
                }
                guard attempt.intent.authoringSessionId == canonicalSessionID(sessionID) else {
                    throw LocalAppServiceFailure("AUTHORING_CHANGED", "The authoring intent belongs to another session.")
                }
                try await projectStore.recordStartedSession(
                    projectId: project.record.id, sessionId: sessionID, title: project.record.name, mode: .code,
                    workspaceDirectory: snapshot.state.cwd, transcriptPath: snapshot.state.transcriptPath)
                guard sessionHost.entry(for: engine) === entry,
                      canonicalSessionID(engine.model.activeSessionId) == canonicalSessionID(sessionID),
                      let current = await sessionHost.authenticatedWorkspaceSnapshot(for: entry),
                      current.state.cwd == request.sourceDirectory,
                      current.state.transcriptPath == snapshot.state.transcriptPath else {
                    throw LocalAppServiceFailure("AUTHORING_CHANGED", "The authoring owner changed before registration.")
                }
                try await projectStore.recordAuthenticatedWorkspace(
                    current.state, in: entry.scope, mode: .code,
                    authority: entry.workspaceWriteAuthority, revision: current.revision)
                guard sessionHost.entry(for: engine) === entry,
                      canonicalSessionID(engine.model.activeSessionId) == canonicalSessionID(sessionID),
                      engine.model.workspaceState?.cwd == request.sourceDirectory else {
                    throw LocalAppServiceFailure("AUTHORING_CHANGED", "The authoring owner changed before registration.")
                }
                guard var attempt = pendingAuthoringAttempts[entry.id] else {
                    throw LocalAppServiceFailure("AUTHORING_CHANGED", "The authoring attempt is unavailable.")
                }
                var savedBinding: LocalAppAuthoring?
                if attempt.registrationAttempted {
                    do {
                        let existing = try await LocalApps.library.exactAuthoringBinding(
                            sourceDirectory: request.sourceDirectory, projectID: project.record.id, sessionID: sessionID,
                            mode: entry.mode, expectedSourceID: attempt.intent.sourceId)
                        guard owner.matches(existing) else {
                            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app binding does not match this owner.")
                        }
                        savedBinding = existing
                    } catch let error as LocalAppServiceFailure where error.code == "notFound" && !attempt.registrationConfirmed {
                        // A previous write had an unknown result. Only an
                        // authoritative absence permits an idempotent retry.
                    }
                }
                if savedBinding == nil && !attempt.registrationConfirmed {
                    let lease = await localAppRegistrationOrder.acquire(
                        key: attempt.registrationTicket?.key ?? "source:" + attempt.intent.sourceId)
                    defer { localAppRegistrationOrder.release(lease) }
                    let setCurrent = !attempt.registrationAttempted && attempt.setCurrentOnFirstAttempt
                        && (attempt.registrationTicket.map(localAppRegistrationOrder.isCurrent) ?? false)
                    attempt.registrationAttempted = true
                    pendingAuthoringAttempts[entry.id] = attempt
                    let authoring = LocalAppAuthoring(sourceDirectory: request.sourceDirectory, workspaceId: project.record.id,
                        projectRef: .mobileProject(project.record.id), authoringSessionId: sessionID, appId: request.appId)
                    try await LocalApps.library.registerAuthoring(authoring, sourceID: attempt.intent.sourceId,
                                                                  requestID: attempt.intent.sourceRequestId, mode: entry.mode, setCurrent: setCurrent)
                    guard var currentAttempt = pendingAuthoringAttempts[entry.id] else {
                        throw LocalAppServiceFailure("AUTHORING_CHANGED", "The authoring attempt is unavailable.")
                    }
                    currentAttempt.registrationConfirmed = true
                    pendingAuthoringAttempts[entry.id] = currentAttempt
                }
            }
            // A successful write alone is not authority to unlock another
            // session. Read back the exact, native-owned tuple after the ACK.
            let binding = try await LocalApps.library.exactAuthoringBinding(
                sourceDirectory: request.sourceDirectory, projectID: project.record.id, sessionID: sessionID,
                mode: entry.mode, expectedSourceID: request.sourceID)
            guard owner.matches(binding), sessionHost.entry(for: engine) === entry,
                  canonicalSessionID(engine.model.activeSessionId) == canonicalSessionID(sessionID),
                  engine.model.workspaceState?.cwd == request.sourceDirectory else {
                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app binding does not match this owner.")
            }
            try engine.confirmAuthoringBinding(owner, binding: binding)
            if let attempt = pendingAuthoringAttempts[entry.id] {
                do {
                    if try await projectStore.localAppSourceCreateIntent(requestID: attempt.intent.intentId) != nil {
                        try await projectStore.completeLocalAppSourceCreateIntent(requestID: attempt.intent.intentId, owner: attempt.intent)
                    }
                } catch {
                    // The owner is already confirmed. Leave the explicit
                    // recovery offer visible if its advisory marker fails.
                    projectStore.pendingLocalAppError = error.localizedDescription
                }
            }
            let previousFailure = pendingAuthoringAttempts[entry.id]?.failureID
            pendingAuthoringAttempts.removeValue(forKey: entry.id)
            engine.model.authoringBindingRetryAvailable = false
            if let previousFailure, engine.model.error?.id == previousFailure { engine.model.error = nil }
        } catch {
            let failure = ConversationError(kind: .host, message: error.localizedDescription)
            entry.source.model.error = failure
            if var attempt = pendingAuthoringAttempts[entry.id] {
                attempt.failureID = failure.id
                pendingAuthoringAttempts[entry.id] = attempt
                entry.source.model.authoringBindingRetryAvailable = true
            }
            if source === entry.source { externalLinkError = error.localizedDescription }
        }
        #else
        entry.source.model.error = ConversationError(kind: .host, message: "App development requires the local conversation engine.")
        #endif
    }

    private func syncConversationBackgroundSurfaces() {
        if let entry = sessionHost.entry(for: source) { sessionHost.sync(entry) }
    }

}

enum ConversationSessionIndexPolicy {
    static func shouldSynchronize(
        transitionPending: Bool,
        restorePending: Bool = false,
        activeSessionID: String,
        listedSessionIDs: Set<String>
    ) -> Bool {
        guard !transitionPending, !restorePending else { return false }
        // Even with a full catalog request, an in-flight new/restore transition
        // can briefly make the active row absent. Preserve the cached index until
        // the active row is enumerated.
        let activeSessionMayNotBeListed = !activeSessionID.isEmpty
            && !listedSessionIDs.contains(activeSessionID)
        return !activeSessionMayNotBeListed
    }
}

/// Caller-side gate for actions that optimistically mutate the selected
/// session. The source remains the authority for accepting New/Resume, but
/// RootView must not change active/confirmed/persisted selection while a
/// correlated durable turn still owns the engine slot.
enum ConversationSessionMutationPolicy {
    static func allowsCallerMutation(
        hasInactiveDurableRecovery: Bool,
        hasUnresolvedTurnRecovery: Bool,
        isCancelling: Bool
    ) -> Bool {
        !hasInactiveDurableRecovery && !hasUnresolvedTurnRecovery && !isCancelling
    }
}

/// Replacing an engine discards its connection-scoped callbacks. The owner
/// remains intact while any admitted turn or interactive request is live.
enum ConversationSourceReplacementPolicy {
    static func shouldRollbackSelection(
        previousSourceIsCurrent: Bool,
        currentProjectID: String?,
        requestedProjectID: String?
    ) -> Bool {
        previousSourceIsCurrent && currentProjectID == requestedProjectID
    }

    @MainActor
    static func allowsReplacement(of model: ConversationModel, hasActiveRealtimeAudio: Bool = false,
                                  allowUnconfirmedAuthoring: Bool = false,
                                  allowPendingSessionTransition: Bool = false) -> Bool {
        guard !hasActiveRealtimeAudio, (!model.authoringBindingPending || allowUnconfirmedAuthoring),
              !model.streaming, !model.isCancelling,
              (!model.sessionTransitionPending || allowPendingSessionTransition),
              !model.slashCommandPending,
              !model.hasUnresolvedTurnRecovery, !model.hasInactiveDurableRecovery else { return false }
        if model.backgroundTasks.contains(where: { $0.status.requiresExecutionLease }) {
            return false
        }
        if model.agentSummaries.contains(where: { agent in
            guard agent.id != ConversationModel.mainAgentID else { return false }
            let status = agent.status.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            return AgentStatusPresentation(rawValue: status) == .running
                || ["queued", "initializing"].contains(status)
        }) {
            return false
        }
        if model.items.contains(where: { item in
            guard case let .run(run) = item else { return false }
            return run.activeWorkers > 0
        }) {
            return false
        }
        if model.requiresBackgroundExecution || !model.pendingQuestions.isEmpty { return false }
        #if canImport(harness_runtimeFFI)
            if !model.pendingPermissions.isEmpty { return false }
        #endif
        return true
    }
}

enum ConversationSessionRestorePolicy {
    static func shouldAdopt(candidateSessionID: String, pendingRestoreID: String?) -> Bool {
        guard !candidateSessionID.isEmpty else { return false }
        return pendingRestoreID == nil || pendingRestoreID == candidateSessionID
    }

    static func rollbackSelection(
        failedSessionID: String,
        pendingRestoreID: String?,
        activeSessionID: String,
        confirmedSessionID: String
    ) -> String? {
        guard !failedSessionID.isEmpty,
              pendingRestoreID == failedSessionID || activeSessionID == failedSessionID else {
            return nil
        }
        return confirmedSessionID
    }
}

/// Mirrors only the current engine model into the matching project session
/// index. Its identity is replaced with the conversation source, so a stale
/// source can never mutate the newly-selected project.
private struct ConversationProjectBridge: View {
    @ObservedObject var model: ConversationModel
    @Bindable var projectStore: ProjectStore
    /// The scheduled scope still adopts/persists the engine session id via the
    /// callbacks, but never writes into the PROJECT session index.
    let scope: ConversationScope
    let pendingRestoreID: String?
    let onSessionChanged: (String) -> Bool
    let onSessionTransitionFailed: (String) -> Bool
    let onModeChanged: (SessionMode) -> Void
    let onRefreshSessions: () -> Void
    var onWorkspaceChanged: (SessionWorkspaceState?) -> Void = { _ in }
    @State private var sessionSyncTask: Task<Void, Never>?

    private var projectID: String? { scope.projectID }

    var body: some View {
        Color.clear
            .frame(width: 0, height: 0)
            .onAppear {
                onModeChanged(model.sessionMode)
                consumeSessionTransitionFailure(model.sessionTransitionFailure)
                handleActiveSession(model.activeSessionId)
                synchronizeWorkspace()
                guard model.engineSessionsLoaded else { return }
                synchronizeSessions(model.engineSessions)
            }
            .onChange(of: model.workspaceState) { _, _ in
                synchronizeWorkspace()
            }
            .onChange(of: model.sessionMode) { _, mode in
                onModeChanged(mode)
            }
            .onChange(of: model.sessionTransitionFailure) { _, failure in
                consumeSessionTransitionFailure(failure)
            }
            .onChange(of: model.engineSessionsLoaded) { _, loaded in
                guard loaded else { return }
                synchronizeSessions(model.engineSessions)
            }
            .onChange(of: model.engineSessions) { _, rows in
                guard model.engineSessionsLoaded else { return }
                synchronizeSessions(rows)
            }
            .onChange(of: model.sessionTransitionPending) { _, pending in
                guard !pending else { return }
                onModeChanged(model.sessionMode)
                handleActiveSession(model.activeSessionId)
                if model.engineSessionsLoaded { synchronizeSessions(model.engineSessions) }
            }
            .onChange(of: model.authoringBindingPending) { _, pending in
                guard !pending else { return }
                handleActiveSession(model.activeSessionId)
                if model.engineSessionsLoaded { synchronizeSessions(model.engineSessions) }
            }
            .onChange(of: pendingRestoreID) { _, pending in
                guard pending == nil, model.engineSessionsLoaded else { return }
                synchronizeSessions(model.engineSessions)
            }
            .onChange(of: model.sessionRefreshRevision) { _, _ in
                onRefreshSessions()
            }
            .onDisappear {
                sessionSyncTask?.cancel()
                sessionSyncTask = nil
            }
            .onChange(of: model.activeSessionId) { _, sessionID in
                handleActiveSession(sessionID)
                synchronizeWorkspace()
            }
            .onChange(of: model.messages.first(where: { $0.role == .user })?.id) { _, _ in
                handleActiveSession(model.activeSessionId)
            }
    }

    private func synchronizeWorkspace() {
        onWorkspaceChanged(model.workspaceState)
    }

    private func handleActiveSession(_ sessionID: String) {
        // A replacement engine can emit its bootstrap SessionStarted before an
        // explicit New/Resume command confirms. Never persist that transient id.
        guard !model.authoringBindingPending, !model.sessionTransitionPending,
              !sessionID.isEmpty,
              onSessionChanged(sessionID)
        else { return }
    }

    private func consumeSessionTransitionFailure(_ failure: SessionTransitionFailure?) {
        guard let failure else { return }
        _ = onSessionTransitionFailed(failure.requestedSessionID)
        model.sessionTransitionFailure = nil
    }

    private func synchronizeSessions(_ rows: [EngineSession]) {
        // The scheduled scope's engine catalog must never replace a project's
        // cached session index.
        guard scope != .scheduled, !model.authoringBindingPending else { return }
        guard ConversationSessionIndexPolicy.shouldSynchronize(
            transitionPending: model.sessionTransitionPending,
            restorePending: pendingRestoreID != nil,
            activeSessionID: model.activeSessionId,
            listedSessionIDs: Set(rows.map(\.id))
        ) else { return }
        let summaries = rows.filter { projectStore.allowsSession($0.id, in: scope) }.map {
            ProjectSessionSummary(
                sessionId: $0.id,
                title: $0.title,
                messageCount: $0.messageCount,
                relativeTime: $0.relativeTime,
                updatedAt: $0.modifiedAt ?? Date(),
                mode: $0.mode,
                workspaceDirectory: $0.workspaceDirectory,
                transcriptPath: $0.transcriptPath
            )
        }
        sessionSyncTask?.cancel()
        sessionSyncTask = Task { @MainActor in
            guard !Task.isCancelled else { return }
            do { try await projectStore.syncEngineSessions(projectId: projectID, rows: summaries) }
            catch { if !Task.isCancelled { model.error = ConversationError(kind: .host, message: error.localizedDescription) } }
        }
    }
}

/// The app running full screen, as the item of its cover.
private struct RunningApp: Identifiable {
    let id: String
}
