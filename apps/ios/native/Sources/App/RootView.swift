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
    @State private var sourceGeneration = UUID()
    @State private var workspaceWidth: CGFloat = 0
    @State private var sessionInspectorOpen = false
    @State private var sessionDetailsSheetOpen = false
    @State private var openTerminalAfterSessionDetails = false
    @State private var selectedPlanDocument: PlanDocument?
    @State private var providerCatalogBootstrapped = false
    /// The workspace the conversation currently runs in: global, a managed
    /// project, or a local app (v3 — each app is a conversation scope whose
    /// workspace directory is the session cwd).
    @State private var activeScope: ConversationScope
    @State private var activeMode: SessionMode
    @State private var activeSession: String
    /// Last session confirmed by SessionStarted/SessionResumed. Drawer taps may
    /// update `activeSession` optimistically, but only this value is persisted.
    @State private var confirmedSession: String
    @State private var pendingSessionRestoreID: String?
    @State private var draft: String
    @State private var voiceInteraction: VoiceInteractionController
    @State private var conversationBackgroundExecution =
        ConversationBackgroundExecutionController.live()
    @State private var conversationBackgroundAlerts =
        ConversationBackgroundAlertController.live()
    @State private var projectSwitching = false
    /// `LingxiAppActionStore.drain()` REMOVES what it hands back, so an action
    /// refused by `ConversationSessionMutationPolicy` used to be destroyed by
    /// the very act of reading it. A conversation notification is posted
    /// exactly for `waitingForUser` / `pausedRecoverable` turns, which is
    /// precisely when `hasUnresolvedTurnRecovery` is true and the gate refuses
    /// — so the marquee "tap the notification to reopen the conversation" path
    /// dropped its own action every time. Park refused actions here and retry
    /// them the moment the gate opens.
    @State private var deferredAppActions: [LingxiAppAction] = []
    /// A stuck gate plus a notification storm must not grow this without
    /// bound; only the newest few taps are worth replaying.
    private static let maxDeferredAppActions = 8
    @State private var workspacePinnedAt: [String: Date]
    @State private var collapsedWorkspaceKeys: Set<String>
    @State private var pendingSessionFork: PendingSessionFork?
    @State private var pendingConversationSelectionRestore: PendingConversationSelectionRestore?

    private let appSandboxRoot: String
    private let scopedPreferences: ProjectScopedPreferences

    init(voiceCapability: VoiceCapabilityModel) {
        let root = ConversationSourceFactory.appSandboxRoot()
        let rootURL = URL(fileURLWithPath: root, isDirectory: true)
        let projects = ProjectStore(appSandboxRoot: rootURL)
        let settings = SettingsStore()
        let providers = ProviderRepository.shared
        settings.llmProviders = providers.legacyProviders()
        settings.searchProviders = []
        settings.fetchProviders = []

        let snapshot = providers.makeLaunchSnapshot()
        let activeProject = projects.activeProject
        let initialScope = ConversationScope(projectID: activeProject?.record.id)
        let initialMode: SessionMode = .code
        settings.mcpServers = MCPConfigurationRepository.shared.loadServers(
            projectCwd: activeProject?.workspace.hostURL.path
        )
        let runtime = TerminalRuntimeDescriptor.make(
            appSandboxRoot: root,
            project: activeProject,
            linuxRuntime: settings.linuxRuntime
        ).config
        let conversation = ConversationSourceFactory.make(
            projectCwd: activeProject?.workspace.hostURL.path,
            sessionMode: initialMode,
            providerConfigured: !snapshot.enabledProfileIDs.isEmpty,
            providerProfilesJson: snapshot.providerProfilesJSON,
            providerRoutingJson: snapshot.routingJSON,
            defaultModelID: snapshot.defaultModelID,
            visionDelegationEnabled: snapshot.visionDelegationEnabled,
            mobileLinux: runtime
        )

        #if canImport(harness_runtimeFFI)
            let scopes = ProjectCronScopeProvider {
                (projects.projects, projects.activeProjectId)
            }
            let executor = FfiCronExecutor(
                appSandboxRoot: root,
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
                }
            )
            let cron: CronRepository
            if ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] == "1" {
                cron = CronRepository(appSandboxRoot: root + "/ui-test-cron")
            } else {
                cron = CronRepository(
                    appSandboxRoot: root,
                    scopeProvider: scopes,
                    storeProvider: FfiCronStoreProvider { (providers.makeLaunchSnapshot().defaultModelID, CronReasoning()) },
                    executor: executor,
                    notifier: UserNotificationCronNotifier(),
                    scheduler: BestEffortBackgroundCronScheduler()
                )
            }
            LocalAppBackgroundTaskBridge.shared.bind(
                { await executor.runLocalAppBackgroundTasks() },
                rescheduler: { await executor.rescheduleLocalAppBackgroundWake() }
            )
        #else
            let cron = CronRepository(
                appSandboxRoot: root,
                notifier: UserNotificationCronNotifier(),
                scheduler: BestEffortBackgroundCronScheduler()
            )
        #endif

        projects.onWillArchiveSession = { projectID, sessionID in
            try await cron.pauseTasksForArchivedSession(projectID: projectID, sessionID: sessionID)
        }
        cron.defaultModel = snapshot.defaultModelID
        let preferences = ProjectScopedPreferences()
        let projectID = projects.activeProjectId
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
        _source = State(initialValue: conversation)
        _activeScope = State(initialValue: initialScope)
        _activeMode = State(initialValue: initialMode)
        _workspacePinnedAt = State(initialValue: preferences.workspacePinnedAt())
        _collapsedWorkspaceKeys = State(
            initialValue: preferences.workspaceCollapsedKeys(mode: initialMode)
        )
        _pendingSessionFork = State(initialValue: nil)
        let storedSessionID = ConversationModeRestorePolicy.sessionID(
            mode: initialMode,
            scoped: preferences.storedActiveSessionID(scope: initialScope, mode: initialMode),
            legacyScoped: initialScope.isLocalApp
                ? nil
                : preferences.storedActiveSessionID(projectID: projectID),
            projectLastActive: initialScope.isLocalApp
                ? nil
                : projects.activeProject?.record.lastActiveSessionId
        )
        _activeSession = State(initialValue: storedSessionID)
        _confirmedSession = State(initialValue: storedSessionID)
        _pendingSessionRestoreID = State(initialValue: storedSessionID.isEmpty ? nil : storedSessionID)
        let initialDraft = ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] == "1"
            ? ""
            : preferences.draft(scope: initialScope, mode: initialMode)
        _draft = State(initialValue: initialDraft)
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
        _voiceInteraction = State(
            initialValue: VoiceInteractionController(
                voiceCapture: VoiceCapture(),
                capability: voiceCapability,
                speechPlayer: IOSAudioService.shared.speechPlayer,
                bargeInRecognizer: IOSAudioService.shared.bargeInRecognizer,
                readinessOverride: voiceReadinessOverride
            )
        )
        appSandboxRoot = root
        scopedPreferences = preferences
    }

    private var session: SessionRef {
        if let row = source.model.engineSessions.first(where: { $0.id == activeSession }) {
            return row.ref
        }
        if let row = projectStore.activeProject?.sessions.first(where: { $0.sessionId == activeSession }) {
            return SessionRef(id: row.sessionId, title: row.title)
        }
        return SessionRef(id: activeSession, title: String(localized: "chat_new_conversation"))
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
            // Sidebar + chat. In compact width this collapses to a stack whose
            // root is the sidebar: the system back button/back-swipe opens it,
            // while Drawer closes it through the preferred-column binding.
            NavigationSplitView(
                columnVisibility: $navigation.columnVisibility,
                preferredCompactColumn: $navigation.compactColumn
            ) {
                sidebar
                    .id(localization.language)
            } detail: {
                NavigationStack(path: $navigation.path) {
                    HStack(spacing: 0) {
                        detailSurface.frame(maxWidth: .infinity)
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
                        .navigationDestination(for: AppRoute.self, destination: destination)
                        .id(localization.language)
                }
            }

            .environment(\.horizontalSizeClass, geometry.size.width >= 840 ? .regular : .compact)

            // Onboarding owns the whole window, sidebar and navigation bars
            // included, so it sits outside the split view rather than in a column.
            if !app.setupDone {
                SetupWizardView(
                    store: settingsStore,
                    onOpenSettings: { navigation.showSettings($0) }
                )
                    .zIndex(100)
                    .transition(.opacity)
            }

            #if canImport(harness_runtimeFFI)
                // Keep one root-mounted native sheet presenter so permission
                // requests work over both the chat and any active SwiftUI sheet.
                EnginePermissionPromptHost(
                    model: source.model,
                    onApprove: { source.approvePermission($0, $1) },
                    onDeny: { source.denyPermission($0) }
                )
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

    private func withLifecycleObservers(_ content: some View) -> some View {
        let backgroundLifecycle = content
            .environment(\.locale, localization.effectiveLocale())
            .onChange(of: scenePhase, handleScenePhase)
            .onReceive(source.model.backgroundExecutionActivity) { active in
                conversationBackgroundExecution.setTurnActive(
                    active,
                    turnID: source.model.activeTurnToken?.clientTurnId
                )
                syncConversationBackgroundSurfaces()
            }
            .onChange(of: source.model.turnCompletion) { _, _ in
                syncConversationBackgroundSurfaces()
                if cronRepository.state.history.contains(where: { $0.status == .queued }) {
                    Task { await cronRepository.reconcile(reason: "chat-idle") }
                }
            }
            .onChange(of: source.model.pendingQuestions) { _, _ in
                syncConversationBackgroundSurfaces()
            }
            .onChange(of: source.model.pendingPermissions) { _, _ in
                syncConversationBackgroundSurfaces()
            }
            .onChange(of: settingsStore.notifs) { _, config in
                conversationBackgroundAlerts.setPreferences(config)
            }
            .onChange(of: source.model.backgroundTasks) { _, _ in
                syncConversationBackgroundSurfaces()
            }
            .onChange(of: source.model.activeTurnToken) { _, token in
                conversationBackgroundExecution.setTurnActive(
                    source.model.requiresBackgroundExecution,
                    turnID: token?.clientTurnId
                )
                syncConversationBackgroundSurfaces()
            }
            .onReceive(
                NotificationCenter.default.publisher(
                    for: .lingxiConversationContinuedProcessingLeaseChanged
                )
            ) { notification in
                let attached = notification.userInfo?["attached"] as? Bool ?? false
                let turnID = (notification.userInfo?[ConversationContinuedProcessingExpiration.turnIDKey] as? String)
                    .flatMap(UInt64.init)
                conversationBackgroundExecution.setContinuedProcessingLeaseAttached(
                    attached,
                    turnID: turnID
                )
            }
            .onReceive(
                NotificationCenter.default.publisher(
                    for: .lingxiConversationContinuedProcessingExpired
                )
            ) { notification in
                let expiration = ConversationContinuedProcessingExpiration(
                    userInfo: notification.userInfo
                )
                conversationBackgroundExecution.setContinuedProcessingLeaseAttached(
                    false,
                    turnID: expiration?.turnID
                )
                let turnToken = source.model.activeTurnToken
                if let expiredTurnID = expiration?.turnID,
                   expiredTurnID != turnToken?.clientTurnId
                {
                    return
                }
                requestRecoverablePause(
                    sessionID: source.model.activeSessionId,
                    turnToken: turnToken
                )
                syncConversationBackgroundSurfaces()
            }
        let stateLifecycle = backgroundLifecycle
            .onChange(of: draft) { _, value in
                scopedPreferences.setDraft(value, scope: activeScope, mode: activeMode)
            }
            .onChange(of: providerRepository.syncRevision) { _, _ in
                settingsStore.llmProviders = providerRepository.legacyProviders()
            }
        let presentationLifecycle = stateLifecycle
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
            }
            .onOpenURL(perform: handleIncomingURL)
        let bootstrapLifecycle = notificationLifecycle
            .task(id: sourceGeneration) {
                let generation = sourceGeneration
                let sessionToRestore = pendingSessionRestoreID ?? activeSession
                // `providerCatalogBootstrapped` flips true exactly once, on
                // this task's very first run, and never resets — so reading
                // it BEFORE that flip is a reliable "is this the app's first
                // ever bootstrap, not a rebind" signal. The very first run
                // has no prior engine session to disown a create FROM: a
                // create started during this same task's async gap (the UI
                // is already interactive while `prepare()`/catalog refresh
                // are in flight) must not be mistaken for one that predates
                // a rebind and discarded.
                let isInitialBootstrap = !providerCatalogBootstrapped
                var current = source
                wireCurrentSource(preserveCatalog: providerCatalogBootstrapped)
                do {
                    var preparedByCatalogRebuild = false
                    if !providerCatalogBootstrapped {
                        providerCatalogBootstrapped = true
                        let catalogLoaded = await providerRepository.refreshCatalog()
                        if catalogLoaded, generation == sourceGeneration {
                            try await rebuildSource(
                                snapshot: providerRepository.makeLaunchSnapshot(),
                                preserveSourceGeneration: true
                            )
                            current = source
                            preparedByCatalogRebuild = true
                        }
                    }
                    if !preparedByCatalogRebuild {
                        try await current.prepare()
                    }
                    guard generation == sourceGeneration else { return }
                    current.listSessions()
                    if !sessionToRestore.isEmpty, !current.model.sessionTransitionPending {
                        requestSessionResume(
                            sessionToRestore,
                            scope: activeScope,
                            using: current
                        )
                    }
                    await providerRepository.refreshCredentialStatus()
                    await LocalAppPlugin.disable(on: current)
                } catch {
                    guard generation == sourceGeneration else { return }
                    current.warmUp()
                }
            }
        let launchLifecycle = bootstrapLifecycle
            .task { await cronRepository.handleLaunch() }
        return launchLifecycle
            .task {
                await consumePendingAppActions()
                syncConversationBackgroundSurfaces()
            }
    }

    private func withPresentations(_ content: some View) -> some View {
        content
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
                        _ = switchScope(to: .project(projectID))
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
            .fullScreenCover(
                item: Binding(
                    get: { navigation.presentedRoute },
                    set: { if $0 == nil { navigation.closePresentedRoute() } }
                )
            ) { route in
                modalDestination(route)
            }
    }

    private var sidebar: some View {
        @Bindable var navigation = navigation
        return Drawer(
            projectStore: projectStore,
            activeScope: activeScope,
            activeSession: $activeSession,
            source: source,
            cronState: cronRepository.state,
            activeMode: activeMode,
            section: $navigation.drawerSection,
            workspacePinnedAt: workspacePinnedAt,
            collapsedWorkspaceKeys: collapsedWorkspaceKeys,
            openSettings: { navigation.showSettings() },
            openTerminal: openCurrentWorkspaceTerminal,
            openCron: { scopeID, taskID in navigation.openCron(scopeID: scopeID, taskID: taskID) },
            closeSidebar: { navigation.closeSidebar() },
            onSelectProject: { switchProject(to: $0) },
            onSelectSession: { switchProject(to: $0, resumeSessionID: $1) },
            onNewChat: { switchProject(to: $0, startNew: true) },
            onModeChanged: { _ = switchMode(to: $0) },
            onToggleWorkspacePinned: toggleWorkspacePinned,
            onSetWorkspaceCollapsed: setWorkspaceCollapsed,
            onContinueInMode: requestContinueInMode
        )
        .id(sourceGeneration)
    }

    private var detailSurface: some View {
        ZStack {
            theme.windowBg.ignoresSafeArea()
            ChatView(
                session: session,
                draft: $draft,
                voiceInteraction: voiceInteraction,
                source: source,
                projectName: projectStore.activeProject?.record.name,
                projectPath: currentWorkspaceGuestPath,
                onOpenVoiceSettings: { navigation.showSettings(.voice) },
                onOpenSessionDetails: {
                    sessionDetailsSheetOpen = true
                },
                onOpenShellTask: { request in
                    navigation.openTerminal(
                        shellRequest: request,
                        projectID: projectStore.activeProjectId,
                        sessionID: request.taskId
                    )
                }
            )
            .id(sourceGeneration)
            ConversationProjectBridge(
                model: source.model,
                projectStore: projectStore,
                scope: activeScope,
                sessionMode: activeMode,
                pendingRestoreID: pendingSessionRestoreID,
                onSessionChanged: adoptEngineSession,
                onFirstMessageRecorded: { setWorkspaceCollapsed(false, workspaceKey: activeScope.workspaceKey) },
                onUnavailableSession: clearUnavailableSession,
                onSessionTransitionFailed: rollbackFailedSession,
                onRefreshSessions: { source.listSessions() }
            )
            .id(sourceGeneration)

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
        .onChange(of: session.id) { _, _ in
            openTerminalAfterSessionDetails = false
            sessionDetailsSheetOpen = false
        }
    }

    private func handleSettingsDismissed() {
        Task { @MainActor in
            await VoicePreviewPlayback.shared.stop()
            guard !navigation.settingsOpen,
                  navigation.path.isEmpty,
                  navigation.presentedRoute == nil
            else { return }
            voiceInteraction.reloadConfigurationAndResumeIfPossible()
        }
    }

    @ViewBuilder
    private func destination(_ route: AppRoute) -> some View {
        switch route {
        case .terminal(_, let initialCommand, let projectID, let requestedCwd):
            let project = terminalProject(for: projectID)
            let descriptor = TerminalRuntimeDescriptor.make(
                appSandboxRoot: appSandboxRoot,
                project: project,
                linuxRuntime: settingsStore.linuxRuntime,
                initialCommand: initialCommand,
                requestedCwd: requestedCwd
            )
            TerminalView(
                descriptor: descriptor,
                onDismiss: { popRoute() },
                onOpenRuntimeSettings: { popRoute(); navigation.showSettings(.linuxRuntime) }
            )
        case .cron(let scopeID, let taskID):
            let route = scopeID.map { CronRoute.task(scopeID: $0, taskID: taskID) }
            CronRootView(repository: cronRepository, initialRoute: route)
                .onAppear { configureCronEditor() }
            .onChange(of: cronRepository.state.generatedSessions) { _, _ in configureCronEditor() }
        case .cronRun(let runID):
            CronRootView(repository: cronRepository, initialRoute: .run(runID: runID))
                .onAppear { configureCronEditor() }
            .onChange(of: cronRepository.state.generatedSessions) { _, _ in configureCronEditor() }
        case .sessionDetails(let sessionID):
            SessionDetailsView(
                session: session.id == sessionID ? session : SessionRef(id: sessionID, title: session.title),
                source: source,
                workspacePath: currentWorkspaceGuestPath,
                onOpenTerminal: openCurrentWorkspaceTerminal
            )
        }
    }

    @ViewBuilder
    private func modalDestination(_ route: AppRoute) -> some View {
        switch route {
        case .cron(let scopeID, let taskID):
            let initialRoute = scopeID.map { CronRoute.task(scopeID: $0, taskID: taskID) }
            CronRootView(
                repository: cronRepository,
                initialRoute: initialRoute,
                onDismiss: { navigation.closePresentedRoute() }
            )
            .onAppear { configureCronEditor() }
            .onChange(of: cronRepository.state.generatedSessions) { _, _ in configureCronEditor() }
        case .cronRun(let runID):
            CronRootView(
                repository: cronRepository,
                initialRoute: .run(runID: runID),
                onDismiss: { navigation.closePresentedRoute() }
            )
            .onAppear { configureCronEditor() }
            .onChange(of: cronRepository.state.generatedSessions) { _, _ in configureCronEditor() }
        case .terminal:
            EmptyView()
        case .sessionDetails:
            EmptyView()
        }
    }

    /// Explain a refused switch while preserving the source that still owns
    /// background work, durable recovery, or the current cancellation barrier.
    private var scopeSwitchRefusalMessage: String {
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
        return projectStore.activeProject?.workspace.guestPath ?? LXISHDefaultWorkspace.guestHome
    }

    private func openCurrentWorkspaceTerminal() {
        // The app-scoped conversation owns a dedicated Mobile Linux mount, but
        // the terminal route constructs a separate runtime descriptor. Keep
        // opening the default shell until that route accepts app scope/mount
        // metadata instead of forwarding a guest path into another workspace.
        if activeScope.isLocalApp {
            navigation.openTerminal(projectID: nil, requestedCwd: nil)
            return
        }
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
            conversation.setExternalEventHandler { event in
                Task { @MainActor in
                    clientEventCenter.publish(event)
                    handleExternalEvent(event)
                }
            }
        }

        private func handleExternalEvent(_ event: ClientEvent) {
            providerRepository.handle(event: event)
            switch event {
            case let .sessionStarted(sessionId, mode):
                guard SessionMode(dto: mode) == activeMode else { return }
                triggerPendingForkIfReady(for: sessionId)
            case let .sessionResumed(sessionId, mode, _):
                guard SessionMode(dto: mode) == activeMode else { return }
                triggerPendingForkIfReady(for: sessionId)
            case let .sessionForked(sourceSessionId, sessionId, mode):
                let targetMode = SessionMode(dto: mode)
                guard let pending = pendingSessionFork,
                      pending.sourceSessionID == sourceSessionId,
                      pending.targetMode == targetMode
                else { return }
                pendingSessionFork = nil
                _ = switchScope(
                    to: pending.sourceScope,
                    mode: targetMode,
                    resumeSessionID: sessionId
                )
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
        var scheduledIDs = Set<String>()
        let archivedIDs = Set(projectStore.globalSessions.filter(\.isArchived).map(\.sessionId))
        let scheduledChoices = cronRepository.state.generatedSessions.compactMap { session -> CronSessionChoice? in
            guard session.projectID == nil, !archivedIDs.contains(session.id), scheduledIDs.insert(session.id).inserted else { return nil }
            return CronSessionChoice(id: session.id, title: session.title, scopeID: globalCronScopeID)
        }
        cronRepository.sessionChoices = scheduledChoices + projectStore.projects.flatMap { project in
            project.sessions.filter { !$0.isArchived }.map {
                CronSessionChoice(id: $0.sessionId, title: "\(project.record.name) · \($0.title)", scopeID: project.record.id)
            }
        }
        cronRepository.onOpenSession = { projectID, sessionID in
            navigation.closePresentedRoute()
            let mode = projectStore.projects.first(where: { $0.record.id == projectID })?
                .sessions.first(where: { $0.sessionId == sessionID })?.mode ?? .code
            switchScope(to: projectID.map(ConversationScope.project) ?? .scheduled, mode: mode, resumeSessionID: sessionID)
        }
    }

    private func wireCurrentSource(preserveCatalog: Bool = false) {
        let current = source
        #if canImport(harness_runtimeFFI)
            installExternalEventHandler(on: current)
            providerRepository.configure(
                submitCommand: { command in try await current.submitEngineCommand(command) },
                testConnection: { profile, secret in
                    try await current.testProviderConnection(profile: profile, credentialOverride: secret)
                },
                applyReconnect: { snapshot in
                    try await rebuildSource(snapshot: snapshot)
                },
                oauthLogin: { provider in
                    try await current.loginOAuth(provider: provider)
                },
                oauthState: { provider in
                    try await current.authState(provider: provider)
                },
                oauthLogout: { provider in
                    try await current.logoutOAuth(provider: provider)
                },
                testOAuthConnection: { provider, profile in
                    try await current.testOAuthConnection(provider: provider, profile: profile)
                },
                providerCatalog: {
                    try await current.providerCatalog()
                },
                resetCatalog: !preserveCatalog
            )
        #else
            providerRepository.configure(submitCommand: nil)
        #endif
    }

    private func makeSource(
        scope: ConversationScope,
        snapshot: ProviderLaunchSnapshot,
        mode: SessionMode
    ) -> any ConversationSource {
        let projectCwd: String?
        let project: ProjectSnapshot?
        switch scope {
        case .global, .project:
            project = scope.projectID.flatMap { id in
                projectStore.projects.first(where: { $0.record.id == id })
            }
            projectCwd = project?.workspace.hostURL.path
        case .scheduled:
            project = nil
            projectCwd = appSandboxRoot + "/scheduled/workspace"
        case .localApp:
            // No Local App scope can be entered from this client any more (it has no Local App UI),
            // so there is no app workspace to bind: this is the global workspace.
            project = nil
            projectCwd = nil
        }
        var runtime: TerminalRuntimeConfig? = TerminalRuntimeDescriptor.make(
            appSandboxRoot: appSandboxRoot,
            project: project,
            linuxRuntime: settingsStore.linuxRuntime
        ).config
        if scope == .scheduled {
            runtime?.workspaceHostPath = appSandboxRoot + "/scheduled/workspace"
            runtime?.stableWorkspaceId = "scheduled"
        }
        return ConversationSourceFactory.make(
            projectCwd: projectCwd,
            sessionMode: mode,
            providerConfigured: !snapshot.enabledProfileIDs.isEmpty,
            providerProfilesJson: snapshot.providerProfilesJSON,
            providerRoutingJson: snapshot.routingJSON,
            defaultModelID: snapshot.defaultModelID,
            visionDelegationEnabled: snapshot.visionDelegationEnabled,
            mobileLinux: runtime
        )
    }

    private func rebuildSource(
        snapshot: ProviderLaunchSnapshot,
        preserveSourceGeneration: Bool = false
    ) async throws {
        persistConversationScope()
        let old = source
        try requireSourceReplacementAllowed(old)
        let replacement = makeSource(scope: activeScope, snapshot: snapshot, mode: activeMode)
        #if canImport(harness_runtimeFFI)
            installExternalEventHandler(on: replacement)
        #endif
        // Prepare the replacement before cancelling the current source. A
        // failed provider/catalog rebuild must leave the live conversation
        // usable instead of parking a cancelled source in the root view.
        try await replacement.prepare()
        try requireSourceReplacementAllowed(old)
        try await old.cancelAndWait()
        try requireSourceReplacementAllowed(old)
        activeSession = confirmedSession
        pendingSessionRestoreID = confirmedSession.isEmpty ? nil : confirmedSession
        source.setSettingsActive(false)
        source = replacement
        source.setSettingsActive(true)
        // The catalog belongs to the engine build, not to a single source
        // instance. Keep it across normal reconnects so changing a provider
        // cannot trigger a second catalog bootstrap/rebuild loop.
        wireCurrentSource(preserveCatalog: true)
        if !preserveSourceGeneration {
            sourceGeneration = UUID()
        }
        if !confirmedSession.isEmpty {
            requestSessionResume(
                confirmedSession,
                scope: activeScope,
                using: replacement
            )
        }
        old.handleBackground()
    }

    @MainActor
    private func requireSourceReplacementAllowed(
        _ previous: any ConversationSource,
        allowInactiveRecovery: Bool = false
    ) throws {
        guard source === previous else { throw CancellationError() }
        guard ConversationSourceReplacementPolicy.allowsReplacement(
            of: previous.model,
            allowInactiveRecovery: allowInactiveRecovery
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
        let scheduled = projectID == nil && resumeSessionID.map { id in
            cronRepository.state.generatedSessions.contains { $0.projectID == nil && $0.id == id }
        } == true
        switchScope(
            to: scheduled ? .scheduled : ConversationScope(projectID: projectID),
            mode: scheduled ? .code : nil,
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
        switchScope(to: activeScope, mode: mode)
    }

    @discardableResult
    private func switchScope(
        to scope: ConversationScope,
        mode: SessionMode? = nil,
        resumeSessionID: String? = nil,
        startNew: Bool = false,
        allowRecoveryRouting: Bool = false
    ) -> Bool {
        // MINTING a conversation in a local-app scope is pinned to Code: the
        // app's `LINGXI.md` contract requires the create-local-app skill and
        // the `Workflow`/`LocalApp*`/`Write` tools, all of which
        // `apply_mobile_session_tool_policy` strips in Chat mode. Pinned HERE
        // so `onNewAppChat` (Drawer.swift), which passes whatever the global
        // `activeMode` happens to be at tap time, cannot mint an app-scoped
        // Chat conversation by omission.
        //
        // ONLY `startNew`. An explicit resume must keep the session's own
        // recorded mode — `host.rs` rejects a cross-mode resume outright
        // ("session … belongs to chat mode, but this source runs code mode"),
        // and the app's session catalog lists every row regardless of mode, so
        // pinning a resume made every pre-existing Chat-mode app session
        // un-openable. A bare same-scope mode toggle (`switchMode`, the
        // drawer's Chat/Code tabs) is likewise left alone: pinning it made the
        // tap a silent no-op that collapsed the sidebar and desynced the tab
        // from `activeMode`, and that tab is also the only route from inside an
        // app scope to a project's Chat sessions.
        let targetMode = (scope.isLocalApp && startNew) ? .code : (mode ?? activeMode)
        guard !projectSwitching,
              allowRecoveryRouting || ConversationSessionMutationPolicy.allowsCallerMutation(
                  hasInactiveDurableRecovery: source.model.hasInactiveDurableRecovery,
                  hasUnresolvedTurnRecovery: source.model.hasUnresolvedTurnRecovery,
                  isCancelling: source.model.isCancelling
              )
        else { return false }
        if scope != activeScope || targetMode != activeMode {
            guard ConversationSourceReplacementPolicy.allowsReplacement(
                of: source.model,
                allowInactiveRecovery: allowRecoveryRouting
            ) else {
                projectStore.errorMessage = String(localized: "chat_error_finish_background_turn_first")
                return false
            }
        }
        voiceInteraction.handleContextChange()
        if scope == activeScope && targetMode == activeMode {
            navigation.closeSidebar()
            if let resumeSessionID {
                pendingSessionRestoreID = resumeSessionID
                activeSession = resumeSessionID
                requestSessionResume(
                    resumeSessionID,
                    scope: scope,
                    using: source,
                    allowRecoveryRouting: allowRecoveryRouting
                )
            } else if startNew {
                pendingSessionRestoreID = nil
                activeSession = ""
                confirmedSession = ""
                scopedPreferences.setActiveSessionID("", scope: scope, mode: targetMode)
                source.startNewConversation()
            }
            return true
        }

        persistConversationScope()
        let previousSource = source
        projectSwitching = true
        navigation.closeSidebar()
        Task { @MainActor in
            var rollback: ProjectActiveSelectionRollback?
            do {
                if allowRecoveryRouting && previousSource.model.hasInactiveDurableRecovery {
                    previousSource.handleBackground()
                } else {
                    try await previousSource.cancelAndWait()
                }
                try requireSourceReplacementAllowed(previousSource, allowInactiveRecovery: allowRecoveryRouting)
                // Only a project/global switch moves the durable active-project
                // selection. Entering a local-app scope leaves the project
                // selection untouched — leaving the app returns to it.
                if scope != activeScope, !scope.isLocalApp {
                    rollback = try await projectStore.persistActiveForSwitch(projectId: scope.projectID)
                }
                try requireSourceReplacementAllowed(previousSource, allowInactiveRecovery: allowRecoveryRouting)
                let replacement = makeSource(
                    scope: scope,
                    snapshot: providerRepository.makeLaunchSnapshot(),
                    mode: targetMode
                )
                #if canImport(harness_runtimeFFI)
                    installExternalEventHandler(on: replacement)
                #endif
                try await replacement.prepare()
                try requireSourceReplacementAllowed(previousSource, allowInactiveRecovery: allowRecoveryRouting)
                activeScope = scope
                activeMode = targetMode
                collapsedWorkspaceKeys = scopedPreferences.workspaceCollapsedKeys(mode: targetMode)
                draft = scopedPreferences.draft(scope: scope, mode: targetMode)
                let restoredSession = restoredSessionID(scope: scope, mode: targetMode)
                confirmedSession = restoredSession
                activeSession = resumeSessionID ?? restoredSession
                pendingSessionRestoreID = activeSession.isEmpty ? nil : activeSession
                source.setSettingsActive(false)
                source = replacement
                source.setSettingsActive(true)
                sourceGeneration = UUID()
                if startNew {
                    pendingSessionRestoreID = nil
                    activeSession = ""
                    confirmedSession = ""
                    scopedPreferences.setActiveSessionID("", scope: scope, mode: targetMode)
                    replacement.startNewConversation()
                    if let pendingDraft = pendingBeginActionDraft {
                        pendingBeginActionDraft = nil
                        draft = pendingDraft
                    }
                } else if !activeSession.isEmpty {
                    requestSessionResume(
                        activeSession,
                        scope: scope,
                        using: replacement,
                        allowRecoveryRouting: allowRecoveryRouting
                    )
                }
                previousSource.handleBackground()
                await cronRepository.refresh()
            } catch {
                if let rollback,
                   ConversationSourceReplacementPolicy.shouldRollbackSelection(
                       previousSourceIsCurrent: source === previousSource,
                       currentProjectID: projectStore.activeProjectId,
                       requestedProjectID: scope.projectID
                   ) {
                    _ = try? await projectStore.rollbackActiveSwitch(rollback)
                }
                projectStore.errorMessage = String(localized: "project_switch_failed_message \(error.localizedDescription)")
                // The mode-pinning switch this armed for never landed either
                // — drop it rather than leave it to load into some LATER
                // unrelated `startNew`.
                pendingBeginActionDraft = nil
                if let pending = pendingSessionFork,
                   pending.sourceScope == scope,
                   pending.sourceMode == targetMode {
                    rollbackPendingSessionFork(pending, message: nil)
                }
                if source === previousSource {
                    previousSource.handleForeground()
                    previousSource.warmUp()
                }
            }
            projectSwitching = false
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
        activeSession = sessionID
        confirmedSession = sessionID
        scopedPreferences.setActiveSessionID(sessionID, scope: activeScope, mode: activeMode)
        if pendingSessionRestoreID == sessionID {
            pendingSessionRestoreID = nil
        }
        triggerPendingForkIfReady(for: sessionID)
        return true
    }

    @discardableResult
    private func clearUnavailableSession(_ sessionID: String) -> Bool {
        guard ConversationSessionRestorePolicy.shouldClearUnavailableSession(
            unavailableSessionID: sessionID,
            pendingRestoreID: pendingSessionRestoreID,
            activeSessionID: activeSession
        ) else {
            return false
        }
        if confirmedSession == sessionID {
            confirmedSession = ""
        }
        pendingSessionRestoreID = nil
        activeSession = confirmedSession
        scopedPreferences.setActiveSessionID(confirmedSession, scope: activeScope, mode: activeMode)
        if let pending = pendingSessionFork,
           pending.sourceSessionID == sessionID {
            rollbackPendingSessionFork(pending, message: nil)
        }
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
        activeSession = rollbackSelection
        scopedPreferences.setActiveSessionID(confirmedSession, scope: activeScope, mode: activeMode)
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

    private func setWorkspaceCollapsed(_ collapsed: Bool, workspaceKey: String) {
        collapsedWorkspaceKeys = scopedPreferences.setWorkspaceCollapsed(
            collapsed,
            workspaceKey: workspaceKey,
            mode: activeMode
        )
    }

    private func requestContinueInMode(
        scope: ConversationScope,
        sessionID: String,
        sourceMode: SessionMode,
        targetMode: SessionMode
    ) {
        guard !sessionID.isEmpty, sourceMode != targetMode else { return }
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
            origin: origin
        )
        if scope == activeScope, activeMode == sourceMode, confirmedSession == sessionID {
            triggerPendingForkIfReady(for: sessionID)
            return
        }
        guard switchScope(to: scope, mode: sourceMode, resumeSessionID: sessionID) else {
            pendingSessionFork = nil
            return
        }
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
            pending.submitted = true
            pendingSessionFork = pending
            Task { @MainActor in
                do {
                    try await source.forkSession(
                        pending.sourceSessionID,
                        targetMode: pending.targetMode
                    )
                } catch {
                    rollbackPendingSessionFork(pending, message: error.localizedDescription)
                }
            }
        #endif
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
        if let request = pending.rollbackRequest {
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
        guard let pending = pendingConversationSelectionRestore else { return }
        if let message = pending.message {
            projectStore.errorMessage = message
        }

        let snapshot = pending.snapshot
        let mutationAllowed = ConversationSessionMutationPolicy.allowsCallerMutation(
            hasInactiveDurableRecovery: source.model.hasInactiveDurableRecovery,
            hasUnresolvedTurnRecovery: source.model.hasUnresolvedTurnRecovery,
            isCancelling: source.model.isCancelling
        )

        if activeScope == snapshot.scope, activeMode == snapshot.mode {
            if draft != snapshot.draft {
                draft = snapshot.draft
            }
            scopedPreferences.setDraft(snapshot.draft, scope: snapshot.scope, mode: snapshot.mode)

            if let resumeSessionID = snapshot.resumeSessionID {
                let sessionAlreadySelected =
                    pendingSessionRestoreID == resumeSessionID
                    || activeSession == resumeSessionID
                    || confirmedSession == resumeSessionID
                if sessionAlreadySelected {
                    pendingConversationSelectionRestore = nil
                    return
                }
                guard mutationAllowed else { return }
                pendingConversationSelectionRestore = nil
                pendingSessionRestoreID = resumeSessionID
                activeSession = resumeSessionID
                requestSessionResume(resumeSessionID, scope: snapshot.scope, using: source)
                return
            }

            let alreadyAtFreshConversation =
                pendingSessionRestoreID == nil
                && activeSession.isEmpty
                && confirmedSession.isEmpty
            if alreadyAtFreshConversation {
                pendingConversationSelectionRestore = nil
                return
            }
            guard mutationAllowed else { return }
            pendingConversationSelectionRestore = nil
            pendingSessionRestoreID = nil
            activeSession = ""
            confirmedSession = ""
            scopedPreferences.setActiveSessionID("", scope: snapshot.scope, mode: snapshot.mode)
            source.startNewConversation()
            return
        }

        guard !projectSwitching, mutationAllowed else { return }
        _ = switchScope(
            to: snapshot.scope,
            mode: snapshot.mode,
            resumeSessionID: snapshot.resumeSessionID,
            startNew: snapshot.startsNewConversation
        )
    }

    private func restoredSessionID(scope: ConversationScope, mode: SessionMode) -> String {
        let projectLastActive = scope.projectID.flatMap { id in
            projectStore.projects.first(where: { $0.record.id == id })?.record.lastActiveSessionId
        }
        return ConversationModeRestorePolicy.sessionID(
            mode: mode,
            scoped: scopedPreferences.storedActiveSessionID(scope: scope, mode: mode),
            legacyScoped: scopedPreferences.storedActiveSessionID(scope: scope),
            projectLastActive: projectLastActive
        )
    }

    /// A zero message count in the durable project index is the proof required
    /// by the migration-safe engine entrypoint. If a live catalog row is
    /// available it wins over the cache; otherwise startup/project switching can
    /// still restore legacy empty sessions before ListSessions replies.
    private func requestSessionResume(
        _ sessionID: String,
        scope: ConversationScope,
        using conversation: any ConversationSource,
        allowRecoveryRouting: Bool = false
    ) {
        guard allowRecoveryRouting || ConversationSessionMutationPolicy.allowsCallerMutation(
            hasInactiveDurableRecovery: conversation.model.hasInactiveDurableRecovery,
            hasUnresolvedTurnRecovery: conversation.model.hasUnresolvedTurnRecovery,
            isCancelling: conversation.model.isCancelling
        ) else { return }
        let emptyTitle: String?
        if let live = conversation.model.engineSessions.first(where: { $0.id == sessionID }) {
            emptyTitle = live.messageCount == 0 ? live.title : nil
        } else if scope.isLocalApp {
            // App sessions have no cached project index to consult; the app's
            // own catalog rows are engine-listed and need no legacy-empty
            // migration hint.
            emptyTitle = nil
        } else {
            let cachedRows = scope.projectID.flatMap { id in
                projectStore.projects.first(where: { $0.record.id == id })?.sessions
            } ?? projectStore.globalSessions
            if let cached = cachedRows.first(where: { $0.sessionId == sessionID }),
               cached.messageCount == 0 {
                emptyTitle = cached.title
            } else {
                emptyTitle = nil
            }
        }
        conversation.resumeSession(sessionID, emptySessionTitle: emptyTitle)
    }

    private func persistConversationScope() {
        guard ConversationSessionMutationPolicy.allowsCallerMutation(
            hasInactiveDurableRecovery: source.model.hasInactiveDurableRecovery,
            hasUnresolvedTurnRecovery: source.model.hasUnresolvedTurnRecovery,
            isCancelling: source.model.isCancelling
        ) else { return }
        scopedPreferences.setDraft(draft, scope: activeScope, mode: activeMode)
        scopedPreferences.setActiveSessionID(confirmedSession, scope: activeScope, mode: activeMode)
    }

    private func handleScenePhase(_ oldPhase: ScenePhase, _ phase: ScenePhase) {
        conversationBackgroundAlerts.setScenePhase(phase)
        switch phase {
        case .background:
            persistConversationScope()
            voiceInteraction.handleBackground()
            conversationBackgroundExecution.setTurnActive(
                source.model.requiresBackgroundExecution,
                turnID: source.model.activeTurnToken?.clientTurnId
            )
            source.handleBackground()
            syncConversationBackgroundSurfaces()
            Task {
                await VoicePreviewPlayback.shared.stop()
                await VoiceAudioSessionCoordinator.shared.suspendForBackground()
            }
            Task { await cronRepository.handleSceneDidEnterBackground() }
        case .active:
            conversationBackgroundExecution.setTurnActive(
                source.model.requiresBackgroundExecution,
                turnID: source.model.activeTurnToken?.clientTurnId
            )
            source.handleForeground()
            syncConversationBackgroundSurfaces()
            Task { await VoiceAudioSessionCoordinator.shared.resumeAfterForeground() }
            Task { await cronRepository.handleSceneBecameActive() }
            Task { await consumePendingAppActions() }
        default:
            break
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
        guard !applied else { return }
        deferredAppActions.append(action)
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
        guard ConversationSessionMutationPolicy.allowsCallerMutation(
            hasInactiveDurableRecovery: source.model.hasInactiveDurableRecovery,
            hasUnresolvedTurnRecovery: source.model.hasUnresolvedTurnRecovery,
            isCancelling: source.model.isCancelling
        ) else { return }
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
    private func applyAppAction(_ action: LingxiAppAction) -> Bool {
        navigation.closeSidebar()
        navigation.closeSettings()
        navigation.closePresentedRoute()
        navigation.path.removeAll()

        switch action {
        case .openApp:
            return true
        case .newConversation:
            return beginAppIntegratedConversation(draftText: "")
        case let .ask(question):
            let trimmed = question.trimmingCharacters(in: .whitespacesAndNewlines)
            return beginAppIntegratedConversation(draftText: trimmed)
        // `turnID` is deliberately unused because the iOS transcript has no
        // navigate-to-turn API. Workspace and mode are still load-bearing:
        // the session must be resumed under the source that posted the alert.
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
            case .global, .scheduled:
                break
            case let .project(projectID):
                guard projectStore.projects.contains(where: { $0.record.id == projectID }) else {
                    return true
                }
            case .localApp:
                // This client no longer opens Local App workspaces; a link to one is ignored.
                return true
            }
            if targetScope == activeScope, mode == activeMode {
                let currentSessionID = source.model.activeSessionId.isEmpty
                    ? confirmedSession
                    : source.model.activeSessionId
                if currentSessionID == sessionID { return true }
            }
            guard ConversationSessionMutationPolicy.allowsCallerMutation(
                hasInactiveDurableRecovery: source.model.hasInactiveDurableRecovery,
                hasUnresolvedTurnRecovery: source.model.hasUnresolvedTurnRecovery,
                isCancelling: source.model.isCancelling
            ) || targetScope != activeScope || mode != activeMode else { return false }
            return switchScope(
                to: targetScope,
                mode: mode,
                resumeSessionID: sessionID,
                allowRecoveryRouting: true
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

    private func handleIncomingURL(_ url: URL) {
        #if canImport(harness_runtimeFFI)
            if url.scheme?.lowercased() == "lingxi",
               url.host?.lowercased() == "oauth",
               url.path == "/callback" {
                source.handleOAuthCallback(url)
                return
            }
        #endif
        guard let action = LingxiDeepLink.action(from: url) else { return }
        parkRefusedAppAction(action, applied: applyAppAction(action))
    }

    /// Draft text waiting to be loaded into the composer once a MODE-PINNING
    /// `switchScope` (below) finishes minting the fresh conversation it was
    /// asked to start. The text is left for the user to review, not auto-sent.
    @State private var pendingBeginActionDraft: String?

    @discardableResult
    private func beginAppIntegratedConversation(draftText: String) -> Bool {
        guard ConversationSessionMutationPolicy.allowsCallerMutation(
            hasInactiveDurableRecovery: source.model.hasInactiveDurableRecovery,
            hasUnresolvedTurnRecovery: source.model.hasUnresolvedTurnRecovery,
            isCancelling: source.model.isCancelling
        ) else { return false }
        // A Local App's own `LINGXI.md` contract requires the
        // create-local-app skill and the `Workflow`/`LocalApp*`/`Write`
        // tools, all of which `apply_mobile_session_tool_policy` strips in
        // Chat mode (see `switchScope`'s own pin, just below). `switchMode`
        // deliberately leaves an app scope free to sit in Chat, so a
        // `lingxi://` new-conversation/ask action reached while it does must
        // still pin here — otherwise it mints an app-scoped Chat
        // conversation whose interview cannot run.
        if activeScope.isLocalApp, activeMode != .code {
            pendingBeginActionDraft = draftText
            // `switchScope` refuses SYNCHRONOUSLY (`projectSwitching`, or the
            // mutation policy) BEFORE reaching either of the two sites that
            // drain this latch — the `startNew` branch of its async Task and
            // that Task's `catch`. Leaving the draft armed on a refusal lets
            // the NEXT unrelated `startNew` scope switch overwrite the
            // composer it restores, so clear it here.
            let switched = switchScope(to: activeScope, mode: .code, startNew: true)
            if !switched { pendingBeginActionDraft = nil }
            return switched
        }
        voiceInteraction.handleContextChange()
        pendingSessionRestoreID = nil
        activeSession = ""
        confirmedSession = ""
        scopedPreferences.setActiveSessionID("", scope: activeScope, mode: activeMode)
        draft = draftText
        source.startNewConversation()
        return true
    }

    private func syncConversationBackgroundSurfaces() {
        let sessionID = source.model.activeSessionId.isEmpty
            ? confirmedSession
            : source.model.activeSessionId
        let turnToken = source.model.activeTurnToken
        conversationBackgroundExecution.setExpirationObserver {
            self.requestRecoverablePause(
                sessionID: sessionID,
                turnToken: turnToken
            )
        }
        conversationBackgroundAlerts.sync(ConversationBackgroundSnapshot(
            sessionID: sessionID,
            turnToken: turnToken,
            turnCompletion: source.model.turnCompletion,
            pendingQuestions: source.model.pendingQuestions,
            // The tool name is carried, not re-derived: the prompt's own title
            // has already been through a localized format string.
            // A kind other than `toolUseConfirm` (`exitPlanMode`,
            // `bypassPermissionsMode`) has no tool to name, so the body renders
            // without one rather than inventing a label nobody would recognize.
            pendingPermissions: source.model.pendingPermissions.map { pending -> ConversationPendingPermission in
                var toolName = ""
                if case let .toolUseConfirm(name, _, _) = pending.kind { toolName = name }
                return ConversationPendingPermission(requestId: pending.requestId, toolName: toolName)
            },
            backgroundTasks: source.model.backgroundTasks,
            requiresExecutionLease: source.model.requiresBackgroundExecution,
            workspaceKey: activeScope.workspaceKey,
            sessionMode: activeMode
        ))
    }

    /// A lease expiration is only a platform signal. Keep the turn owned and
    /// non-sendable until the engine confirms `PausedRecoverable`; a rejected
    /// pause remains visible as a host error and never becomes a fake cancel.
    private func requestRecoverablePause(
        sessionID: String,
        turnToken: ConversationTurnToken?
    ) {
        guard let turnToken else { return }
        Task { @MainActor in
            do {
                try await source.markActiveTurnPausedRecoverable(turnToken)
                guard source.model.activeTurnToken == turnToken,
                      !source.model.streaming,
                      source.model.statusLine == String(localized: "chat_background_paused_text")
                else { return }
                conversationBackgroundAlerts.markRecoverablePause(
                    sessionID: sessionID,
                    turnToken: turnToken,
                    workspaceKey: activeScope.workspaceKey,
                    sessionMode: activeMode
                )
                syncConversationBackgroundSurfaces()
            } catch {
                // The source retains ownership and publishes the host error;
                // syncing here keeps activity/notification state from claiming
                // that the turn paused when the command was rejected.
                syncConversationBackgroundSurfaces()
            }
        }
    }
}

enum ConversationModeRestorePolicy {
    static func sessionID(
        mode: SessionMode,
        scoped: String?,
        legacyScoped: String?,
        projectLastActive: String?
    ) -> String {
        if let scoped { return scoped }
        guard mode == .code else { return "" }
        return legacyScoped ?? projectLastActive ?? ""
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

/// Replacing a native engine discards its connection-scoped callbacks. A
/// foreground turn can be explicitly cancelled first, but detached work and
/// its interactive requests must keep their original source until settled.
enum ConversationSourceReplacementPolicy {
    static func shouldRollbackSelection(
        previousSourceIsCurrent: Bool,
        currentProjectID: String?,
        requestedProjectID: String?
    ) -> Bool {
        previousSourceIsCurrent && currentProjectID == requestedProjectID
    }

    @MainActor
    static func allowsReplacement(
        of model: ConversationModel,
        allowInactiveRecovery: Bool = false
    ) -> Bool {
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
        // Includes idle shell/tool/compaction activity after the main stream
        // ended. A live main stream is handled by the cancellation barrier.
        if !model.streaming && model.requiresBackgroundExecution { return false }
        let mayRouteInactiveRecovery = allowInactiveRecovery && model.hasInactiveDurableRecovery
        if !model.streaming && !mayRouteInactiveRecovery && !model.pendingQuestions.isEmpty {
            return false
        }
        #if canImport(harness_runtimeFFI)
            if model.pendingPermissions.contains(where: {
                $0.worker != nil || (!model.streaming && !mayRouteInactiveRecovery)
            }) {
                return false
            }
        #endif
        return true
    }
}

enum ConversationSessionRestorePolicy {
    static func shouldAdopt(candidateSessionID: String, pendingRestoreID: String?) -> Bool {
        guard !candidateSessionID.isEmpty else { return false }
        return pendingRestoreID == nil || pendingRestoreID == candidateSessionID
    }

    static func shouldClearUnavailableSession(
        unavailableSessionID: String,
        pendingRestoreID: String?,
        activeSessionID: String
    ) -> Bool {
        guard !unavailableSessionID.isEmpty else { return false }
        return pendingRestoreID == unavailableSessionID
            || activeSessionID == unavailableSessionID
    }

    static func rollbackSelection(
        failedSessionID: String,
        pendingRestoreID: String?,
        activeSessionID: String,
        confirmedSessionID: String
    ) -> String? {
        guard shouldClearUnavailableSession(
            unavailableSessionID: failedSessionID,
            pendingRestoreID: pendingRestoreID,
            activeSessionID: activeSessionID
        ) else {
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
    /// A `.localApp` scope still adopts/persists the engine session id via the
    /// callbacks, but never writes into the PROJECT session index — an app's
    /// catalog is engine-owned (`ListAppSessions`), not project state.
    let scope: ConversationScope
    let sessionMode: SessionMode
    let pendingRestoreID: String?
    let onSessionChanged: (String) -> Bool
    let onFirstMessageRecorded: () -> Void
    let onUnavailableSession: (String) -> Bool
    let onSessionTransitionFailed: (String) -> Bool
    let onRefreshSessions: () -> Void
    @State private var sessionSyncTask: Task<Void, Never>?

    private var projectID: String? { scope.projectID }

    var body: some View {
        Color.clear
            .frame(width: 0, height: 0)
            .onAppear {
                consumeRestoreRecovery(model.sessionRestoreRecovery)
                consumeSessionTransitionFailure(model.sessionTransitionFailure)
                handleActiveSession(model.activeSessionId)
                guard model.engineSessionsLoaded else { return }
                synchronizeSessions(model.engineSessions)
            }
            .onChange(of: model.sessionRestoreRecovery) { _, recovery in
                consumeRestoreRecovery(recovery)
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
                guard !pending, model.engineSessionsLoaded else { return }
                synchronizeSessions(model.engineSessions)
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
            }
            .onChange(of: model.messages.first(where: { $0.role == .user })?.id) { _, _ in
                handleActiveSession(model.activeSessionId)
            }
    }

    private func handleActiveSession(_ sessionID: String) {
        consumeRestoreRecovery(model.sessionRestoreRecovery)
        // A replacement engine can emit its bootstrap SessionStarted before an
        // explicit New/Resume command confirms. Never persist that transient id.
        guard !model.sessionTransitionPending,
              !sessionID.isEmpty,
              onSessionChanged(sessionID)
        else { return }
        // App-scope sessions are adopted (above) but never recorded into the
        // project session index.
        guard !scope.isLocalApp && scope != .scheduled else { return }
        let indexedRow: ProjectSessionSummary?
        if let projectID {
            indexedRow = projectStore.projects.first(where: { $0.record.id == projectID })?
                .sessions.first(where: { $0.sessionId == sessionID })
        } else {
            indexedRow = projectStore.globalSessions.first(where: { $0.sessionId == sessionID })
        }
        let firstMessage = model.messages.first(where: { $0.role == .user })
        Task { @MainActor in
            guard model.activeSessionId == sessionID, !model.sessionTransitionPending else { return }
            if (indexedRow?.messageCount ?? 0) == 0, let firstMessage {
                do {
                    try await projectStore.recordStartedSession(
                        projectId: projectID,
                        sessionId: sessionID,
                        title: String(firstMessage.text.prefix(120)),
                        mode: sessionMode,
                        initialMessageCount: 1
                    )
                    if model.activeSessionId == sessionID { onFirstMessageRecorded() }
                } catch {
                    if model.activeSessionId == sessionID {
                        model.error = ConversationError(kind: .host, message: String(format: String(localized: "session_index_save_new_failed_fmt"), error.localizedDescription))
                    }
                    return
                }
            }
            guard model.activeSessionId == sessionID, !model.sessionTransitionPending else { return }
            if let projectID {
                try? await projectStore.markActiveSession(projectId: projectID, sessionId: sessionID)
            }
        }
    }

    private func consumeRestoreRecovery(_ recovery: SessionRestoreRecovery?) {
        guard let recovery,
              onUnavailableSession(recovery.unavailableSessionID)
        else { return }
        model.sessionRestoreRecovery = nil
    }

    private func consumeSessionTransitionFailure(_ failure: SessionTransitionFailure?) {
        guard let failure else { return }
        _ = onSessionTransitionFailed(failure.requestedSessionID)
        model.sessionTransitionFailure = nil
    }

    private func synchronizeSessions(_ rows: [EngineSession]) {
        // An app scope's engine catalog must never replace a project's cached
        // session index.
        guard !scope.isLocalApp && scope != .scheduled else { return }
        guard ConversationSessionIndexPolicy.shouldSynchronize(
            transitionPending: model.sessionTransitionPending,
            restorePending: pendingRestoreID != nil,
            activeSessionID: model.activeSessionId,
            listedSessionIDs: Set(rows.map(\.id))
        ) else { return }
        let summaries = rows.map {
            ProjectSessionSummary(
                sessionId: $0.id,
                title: $0.title,
                messageCount: $0.messageCount,
                relativeTime: $0.relativeTime,
                updatedAt: $0.modifiedAt ?? Date(),
                mode: $0.mode
            )
        }
        sessionSyncTask?.cancel()
        sessionSyncTask = Task { @MainActor in
            guard !Task.isCancelled else { return }
            try? await projectStore.syncEngineSessions(projectId: projectID, rows: summaries)
        }
    }
}
