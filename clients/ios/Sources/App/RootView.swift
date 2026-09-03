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
    @State private var localAppsStore: LocalAppsStore
    @State private var clientEventCenter: ClientEventCenter
    @State private var source: any ConversationSource
    @State private var sourceGeneration = UUID()
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

        #if canImport(engine_mobileFFI)
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
            let cron = CronRepository(
                appSandboxRoot: root,
                scopeProvider: scopes,
                storeProvider: FfiCronStoreProvider(),
                executor: executor,
                notifier: UserNotificationCronNotifier(),
                scheduler: BestEffortBackgroundCronScheduler()
            )
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

        let preferences = ProjectScopedPreferences()
        let projectID = projects.activeProjectId
        _settingsStore = State(initialValue: settings)
        _navigation = State(initialValue: AppNavigationModel())
        _projectStore = State(initialValue: projects)
        _cronRepository = State(initialValue: cron)
        let localApps = LocalAppsStore()
        let eventCenter = ClientEventCenter()
        #if canImport(engine_mobileFFI)
            eventCenter.subscribe { event in providers.handle(event: event) }
            eventCenter.subscribe { event in localApps.handle(event: event) }
        #endif
        #if DEBUG
            // A UI test asking for a non-empty app catalog. Under
            // `LINGXI_UI_TESTING=1` the conversation source is the mock, whose
            // `submitEngineCommand` is a no-op, so `.listApps` never resolves
            // and nothing else can ever put a row in `localApps.apps` — see
            // `LocalAppsStore.seedForUITesting`. Seeded HERE, before the store
            // reaches the view tree, so the drawer's apps tab is already
            // populated on first render and no test has to wait on an engine
            // round-trip that will not happen.
            if ProcessInfo.processInfo.environment[LocalAppsStore.uiTestSeedEnvironmentKey] == "1" {
                localApps.seedForUITesting()
            }
        #endif
        _providerRepository = State(initialValue: providers)
        _localAppsStore = State(initialValue: localApps)
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
                speechPlayer: SystemVoiceSpeechPlayer(),
                bargeInRecognizer: VoiceBargeInRecognizer(),
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
    }

    private var rootStack: some View {
        @Bindable var navigation = navigation
        return ZStack {
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
                    detailSurface
                        .navigationDestination(for: AppRoute.self, destination: destination)
                        .id(localization.language)
                }
            }

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

            #if canImport(engine_mobileFFI)
                // Engine permissions are app-scoped, not chat-scoped. The host
                // presents above the current UIKit surface, including sheets and
                // full-screen covers opened after a workflow starts.
                EnginePermissionPromptHost(
                    model: source.model,
                    onApprove: { source.approvePermission($0, $1) },
                    onDeny: { source.denyPermission($0) }
                )
            #endif
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
            }
            .onChange(of: source.model.pendingQuestions) { _, _ in
                syncConversationBackgroundSurfaces()
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
            // Land the user in the new app. See `landCreatedAppIfReady` for why
            // all three of these re-check the same gate rather than one of them
            // driving.
            // `@Published` publishes from `willSet`, so `source.model.streaming`
            // still reads the OLD value inside this sink — on the true→false
            // edge it reads `true` and the gate below would refuse forever.
            // Pass the DELIVERED value instead of re-reading the property.
            .onReceive(source.model.$streaming) { isStreaming in
                landCreatedAppIfReady(streaming: isStreaming)
            }
        let presentationLifecycle = stateLifecycle
            .onChange(of: localAppsStore.createdAppLanding) { _, _ in
                landCreatedAppIfReady()
            }
            .onChange(of: localAppsStore.pendingWidgetSetup) { _, _ in
                landCreatedAppIfReady()
            }
            .onChange(of: localAppsStore.requestedPresentationAppID) { _, appID in
                guard let requestedAppID = appID else { return }
                navigation.openLocalApps(appID: requestedAppID)
                _ = localAppsStore.consumeRequestedPresentationAppID()
            }
        let notificationLifecycle = presentationLifecycle
            .onReceive(NotificationCenter.default.publisher(for: .lingxiCronNotificationOpened)) { note in
                cronRepository.handleNotificationUserInfo(note.userInfo ?? [:])
                if let runID = note.userInfo?["lingxi.cron.run_id"] as? String {
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
            .onReceive(NotificationCenter.default.publisher(for: UIApplication.didReceiveMemoryWarningNotification)) { _ in
                Task { await localAppsStore.handleMemoryWarning() }
            }
            .onOpenURL(perform: handleIncomingURL)
        let bootstrapLifecycle = notificationLifecycle
            .task(id: sourceGeneration) {
                let generation = sourceGeneration
                let sessionToRestore = pendingSessionRestoreID ?? activeSession
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
                    await localAppsStore.refreshAfterEngineRebind()
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
            .sheet(
                isPresented: Binding(
                    get: { navigation.settingsOpen },
                    set: { if !$0 { navigation.closeSettings() } }
                ),
                onDismiss: handleSettingsDismissed
            ) {
                SettingsHost(
                    store: settingsStore,
                    convo: source.model,
                    localAppsStore: localAppsStore,
                    projectCwd: projectStore.activeProject?.workspace.hostURL.path,
                    onRefreshMcp: { source.refreshMcpServers() },
                    onRefreshSkills: {
                        #if canImport(engine_mobileFFI)
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
                        #if canImport(engine_mobileFFI)
                            try await source.submitEngineCommand(.setPermissionMode(mode: mode))
                        #endif
                    },
                    onTypescriptLspModeChanged: { mode in
                        #if canImport(engine_mobileFFI)
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
            // One controller presents one modal. While the local-apps cover is up it
            // owns the prompt (LocalAppsRootView); this presenter only covers requests
            // raised with the cover down — e.g. a destructive data migration approved
            // during background generation.
            .sheet(
                item: Binding(
                    get: { navigation.presentedRoute == nil ? localAppsStore.pendingPermission : nil },
                    set: { _ in }
                )
            ) { prompt in
                LocalAppPermissionSheet(store: localAppsStore, prompt: prompt)
            }
            // The store's error channel needs a presenter with the cover down:
            // a create started from the drawer has no `LocalAppsRootView`
            // mounted, so without this the failure is silent.
            .alert(
                "local_apps_error_title",
                isPresented: localAppErrorPresented
            ) {
                Button("common_ok", role: .cancel, action: localAppsStore.clearError)
            } message: {
                Text(localAppsStore.errorMessage ?? String(localized: "common_unknown_error"))
            }
            .sheet(item: localAppCreateConfirmationItem) { prompt in
                LocalAppCreateConfirmationSheet(store: localAppsStore, prompt: prompt)
            }
            .sheet(item: localAppMcpProposalApprovalItem) { prompt in
                LocalAppMcpProposalApprovalSheet(store: localAppsStore, prompt: prompt)
            }
            // Same rehoming as the alert, for the same reason. The per-app MCP
            // tool `<app>_agent_profile_propose_update` returns
            // `approval_required: true` and the engine holds the approval token
            // until this is answered. Its only presenter was the sheet inside
            // `LocalAppsRootView`, so on the new default path — create from the
            // drawer, land in the conversation, cover never mounted — no sheet
            // appeared anywhere and the agent waited forever on an approval the
            // user could not give.
            .sheet(item: localAppProfileProposalItem) { proposal in
                LocalAppProfileProposalSheet(store: localAppsStore, proposal: proposal)
            }
    }

    /// Whether THIS presenter owns the local-apps error channel right now.
    ///
    /// One controller presents one modal. While the local-apps cover is up its
    /// own alert owns the message (`LocalAppsLibraryView`), and UIKit refuses
    /// to raise an alert from a controller that is already presenting — so
    /// this one also yields to the settings sheet and to the permission sheet
    /// and picks the message up once they are down. `errorMessage` is not
    /// cleared by yielding, so nothing is lost in the meantime.
    private var localAppErrorPresenterIsFree: Bool {
        navigation.presentedRoute == nil
            && !navigation.settingsOpen
            && localAppsStore.pendingPermission == nil
    }

    /// The same gate guards the SETTER, not only the getter. Without it, the
    /// cover coming up flips `get` to false, SwiftUI writes that `false` back,
    /// and `clearError()` destroys the very message the cover's own alert was
    /// about to show.
    private var localAppErrorPresented: Binding<Bool> {
        Binding(
            get: { localAppErrorPresenterIsFree && localAppsStore.errorMessage != nil },
            set: { if !$0, localAppErrorPresenterIsFree { localAppsStore.clearError() } }
        )
    }

    /// Whether THIS presenter owns the profile-proposal channel right now.
    ///
    /// Built ON TOP of `localAppErrorPresenterIsFree` rather than by repeating
    /// its three terms, which is what makes the two RootView presenters
    /// PROVABLY exclusive: the alert shows when
    /// `localAppErrorPresenterIsFree && errorMessage != nil`, this sheet when
    /// `localAppErrorPresenterIsFree && errorMessage == nil` — the same prefix
    /// and contradictory second terms, so the conjunction is `false` for every
    /// state. Strict priority (the alert wins) rather than mutual yielding is
    /// deliberate: two gates each waiting for the other to be empty would
    /// deadlock both channels the moment an error and a proposal are pending at
    /// once. Yielding costs nothing — `pendingProfileProposal` is not cleared
    /// while this is false (see the setter), so the sheet comes up as soon as
    /// the alert, the settings sheet, the permission sheet and the local-apps
    /// cover are all down.
    private var localAppProfileProposalPresenterIsFree: Bool {
        localAppErrorPresenterIsFree
            && localAppsStore.errorMessage == nil
            && localAppsStore.pendingCreateConfirmation == nil
            && localAppsStore.pendingMcpProposalApproval == nil
    }

    /// Root-owned approval sheets must survive page-level Local App UI
    /// lifetimes, so unlike the error alert / permission presenter they do not
    /// yield to `presentedRoute`.
    private var localAppApprovalPresenterIsFree: Bool {
        !navigation.settingsOpen
            && localAppsStore.pendingPermission == nil
            && localAppsStore.errorMessage == nil
    }

    private var localAppCreateConfirmationItem: Binding<LocalAppCreateConfirmationPrompt?> {
        Binding(
            get: {
                localAppApprovalPresenterIsFree
                    ? localAppsStore.pendingCreateConfirmation
                    : nil
            },
            set: { prompt in
                guard prompt == nil, localAppApprovalPresenterIsFree else { return }
                Task { await localAppsStore.resolvePendingCreateConfirmation(false) }
            }
        )
    }

    private var localAppMcpProposalApprovalItem: Binding<LocalAppMcpProposalApprovalPrompt?> {
        Binding(
            get: {
                guard localAppApprovalPresenterIsFree,
                      localAppsStore.pendingCreateConfirmation == nil
                else { return nil }
                return localAppsStore.pendingMcpProposalApproval
            },
            set: { prompt in
                guard prompt == nil,
                      localAppApprovalPresenterIsFree,
                      localAppsStore.pendingCreateConfirmation == nil
                else { return }
                Task { await localAppsStore.resolvePendingMcpProposalApproval(false) }
            }
        )
    }

    /// The proposal to present from the root, or `nil` while another presenter
    /// owns the controller.
    ///
    /// The SETTER carries the same gate as the getter, exactly like
    /// `localAppErrorPresented`. When the cover comes up (or an error arrives)
    /// the gate flips, SwiftUI dismisses this sheet and writes `nil` back —
    /// ungated, that write would answer the proposal on the user's behalf and
    /// destroy the very thing the cover's own sheet was about to present.
    ///
    /// A dismissal that IS this presenter's own — the user swiping the sheet
    /// away — declines. Silently dropping it would leave the engine holding the
    /// approval token, which is the hang this whole presenter exists to end.
    /// `resolveProfileProposal` no-ops when nothing is pending, so the sheet's
    /// own Apply/Cancel buttons cannot double-answer through this write-back.
    private var localAppProfileProposalItem: Binding<LocalAppProfileProposal?> {
        Binding(
            get: {
                localAppProfileProposalPresenterIsFree
                    ? localAppsStore.pendingProfileProposal
                    : nil
            },
            set: { proposal in
                guard proposal == nil,
                      localAppProfileProposalPresenterIsFree else { return }
                localAppsStore.resolveProfileProposal(false)
            }
        )
    }

    /// The drawer's create affordances. Extracted from the `Drawer(...)`
    /// argument list rather than inlined — this file's body has blown the
    /// type-checker's budget before.
    ///
    /// `closeSidebar()` is explicit because this path does not go through
    /// `navigation.openLocalApps`, which is what collapsed the sidebar for
    /// free while these buttons merely opened the library. `switchScope` does
    /// close it, but only once the landing fires — seconds later, and never at
    /// all if the create fails.
    ///
    /// No landing logic here on purpose: `landCreatedAppIfReady` already
    /// observes `createdAppLanding` and hands off to `openCreatedAppSession`.
    ///
    /// `armLibraryFallback: false` because no `LocalAppsRootView` is mounted on
    /// this path. `createdAppID` — the library's fallback landing — has exactly
    /// one consumer, and it lives inside that cover; armed from here the id is
    /// never consumed and survives for the process lifetime, so the user's next
    /// unrelated "View all" would be hijacked onto this app's details page. The
    /// library's "+" still arms it, and its `pendingWidgetSetup` drain
    /// (`LocalAppsLibraryView`) is untouched.
    private func createLocalAppFromDrawer() {
        navigation.closeSidebar()
        Task { _ = await localAppsStore.createShellApp(armLibraryFallback: false) }
    }

    private var sidebar: some View {
        Drawer(
            projectStore: projectStore,
            localAppsStore: localAppsStore,
            activeScope: activeScope,
            activeSession: $activeSession,
            source: source,
            cronState: cronRepository.state,
            activeMode: activeMode,
            workspacePinnedAt: workspacePinnedAt,
            collapsedWorkspaceKeys: collapsedWorkspaceKeys,
            openSettings: { navigation.showSettings() },
            openTerminal: openCurrentWorkspaceTerminal,
            openApps: { appID in navigation.openLocalApps(appID: appID) },
            closeSidebar: { navigation.closeSidebar() },
            createApp: createLocalAppFromDrawer,
            onSelectProject: { switchProject(to: $0) },
            onSelectSession: { switchProject(to: $0, resumeSessionID: $1) },
            onNewChat: { switchProject(to: $0, startNew: true) },
            onSelectAppSession: { appID, sessionID in
                switchScope(to: .localApp(appID), mode: activeMode, resumeSessionID: sessionID)
            },
            onNewAppChat: { appID in
                switchScope(to: .localApp(appID), mode: activeMode, startNew: true)
            },
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
                onOpenVoiceSettings: { navigation.showSettings(.voice) },
                onOpenSessionDetails: {
                    navigation.openSessionDetails(sessionID: session.id)
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
        case .cronRun(let runID):
            CronRootView(repository: cronRepository, initialRoute: .run(runID: runID))
        case .localApps(let appID):
            LocalAppsRootView(
                store: localAppsStore,
                initialAppID: appID,
                activeConversationID: activeSession,
                onDismiss: { navigation.closePresentedRoute() },
                onOpenAppSession: openAppSession,
                onNewAppSession: startNewAppSession
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
        case .cronRun(let runID):
            CronRootView(
                repository: cronRepository,
                initialRoute: .run(runID: runID),
                onDismiss: { navigation.closePresentedRoute() }
            )
        case .terminal:
            EmptyView()
        case .localApps(let appID):
            LocalAppsRootView(
                store: localAppsStore,
                initialAppID: appID,
                activeConversationID: activeSession,
                onDismiss: { navigation.closePresentedRoute() },
                onOpenAppSession: openAppSession,
                onNewAppSession: startNewAppSession
            )
        case .sessionDetails:
            EmptyView()
        }
    }

    /// A tapped row of an app's session catalog: dismiss the local-apps cover
    /// and continue that conversation inside the app's scope.
    private func openAppSession(appID: String, sessionID: String, mode: SessionMode) {
        navigation.closePresentedRoute()
        switchScope(to: .localApp(appID), mode: mode, resumeSessionID: sessionID)
    }

    /// Open the intake conversation the create sheet armed.
    ///
    /// NEVER inherits a `.localApp` scope. That workspace belongs to a DIFFERENT
    /// app: its auto-loaded `LINGXI.md` tells the agent it IS that app and must
    /// not call `LocalAppCreate` again, and the session cwd points at that app's
    /// source. Observed on device — the intake ran inside an existing app, so
    /// the agent hand-rolled a package.json/vite.config.js scaffold by copying
    /// another app, and every build after that failed on the workspace.
    ///
    /// Extracted from the view body for the same reason as the landing below:
    /// inlined, the body exceeded the type-checker's budget.
    /// Take the user into the app that was just created.
    ///
    /// Three things can be the last to arrive, so all three re-check rather
    /// than one of them driving:
    /// - the landing itself (a create from the library sheet: no turn is
    ///   running, so nothing else will fire afterwards),
    /// - the end of a turn (an AGENT-driven create: switching scope submits
    ///   `cancelAndWait()`, which would kill the turn that made the app),
    /// - the widget-setup sheet closing (it must not be yanked out from under
    ///   the user by a scope switch).
    ///
    /// Extracted from the view body on purpose: inlined, it pushed the body
    /// past the Swift type-checker's budget and the build failed with "unable
    /// to type-check this expression in reasonable time".
    /// `streaming` overrides the stored property for the one caller that is a
    /// `@Published` sink — see the comment at that `.onReceive`.
    private func landCreatedAppIfReady(streaming: Bool? = nil) {
        guard !(streaming ?? source.model.streaming),
              localAppsStore.pendingWidgetSetup == nil,
              let landing = localAppsStore.consumeCreatedAppLanding() else { return }
        openCreatedAppSession(
            appID: landing.appID,
            sessionID: landing.initSessionID
        )
    }

    /// The create-flow landing: same as `openAppSession`, plus the queued
    /// kickoff message that starts the create-local-app flow once the empty
    /// init session is live.
    private func openCreatedAppSession(
        appID: String,
        sessionID: String?
    ) {
        navigation.closePresentedRoute()
        // The key is `local_apps_kickoff`, the ONE key both clients read
        // (Android: `R.string.local_apps_kickoff`, `RootScreen.kt`), so the
        // copy cannot drift per platform. Held in `LocalAppKickoff` rather
        // than spelled here so `testTheKickoffMessageResolvesToRealCopy` can
        // assert the string this line actually sends: nothing in Swift fails
        // to compile over a missing localization key, and this call site spent
        // this branch reading `local_apps_kickoff_message` — a key in no
        // catalog, which would have sent the raw key as the user's first
        // message.
        let kickoff = LocalAppKickoff.message
        // The library consumed its one-shot signal to call this, so a refused
        // switch would lose the created app with nothing left to re-arm it.
        // `switchScope` refuses for EITHER of two reasons (see its own guard):
        // another switch is in flight (`projectSwitching`), or
        // `ConversationSessionMutationPolicy` is holding the session — an
        // unresolved turn recovery, an inactive durable recovery, or a
        // cancellation in progress.
        // `sessionID` is nil only when the engine's best-effort init-session
        // mint failed. Landing on a fresh conversation is still correct: the
        // scope, not the session, is what roots the agent in the app workspace.
        guard switchScope(
            to: .localApp(appID),
            mode: .code,
            resumeSessionID: sessionID,
            startNew: sessionID == nil,
            initialPrompt: kickoff
        ) else {
            // Always delay, and retry the SWITCH rather than tail-calling this
            // function, so one bound covers both refusals.
            //
            // The previous shape — `for _ in 0..<40 where projectSwitching` —
            // read as a wait but is a FILTER: under the mutation-policy refusal
            // `projectSwitching` is false, so the body never ran, nothing
            // slept, the `guard` below it passed, and the tail call re-entered
            // immediately. That is an unbounded zero-delay recursion on the
            // MainActor for as long as the policy holds the session.
            Task { @MainActor in
                for _ in 0..<40 {
                    try? await Task.sleep(for: .milliseconds(250))
                    if switchScope(
                        to: .localApp(appID),
                        mode: .code,
                        resumeSessionID: sessionID,
                        startNew: sessionID == nil,
                        initialPrompt: kickoff
                    ) {
                        return
                    }
                }
                // Bounding the retry introduces a give-up path the recursive
                // version never had, so it must not drop the app silently: the
                // record exists and the landing signal is spent.
                localAppsStore.reportCreatedAppLandingExhausted()
            }
            return
        }
    }

    /// 「新会话」in an app's session catalog: dismiss the cover and start a
    /// fresh conversation in the app's scope.
    private func startNewAppSession(appID: String) {
        navigation.closePresentedRoute()
        switchScope(to: .localApp(appID), mode: .code, startNew: true)
    }

    private var currentWorkspaceGuestPath: String {
        if case let .localApp(appID) = activeScope,
           (try? LocalAppWorkspacePath.validatedRoot(appID: appID)) != nil {
            return LXISHGuestPaths.workspace("local-app-\(appID)")
        }
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

    #if canImport(engine_mobileFFI)
        private func installExternalEventHandler(on conversation: any ConversationSource) {
            conversation.setExternalEventHandler { event in
                Task { @MainActor in
                    clientEventCenter.publish(event)
                    handleExternalEvent(event)
                }
            }
        }

        private func handleExternalEvent(_ event: ClientEvent) {
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

    private func wireCurrentSource(preserveCatalog: Bool = false) {
        let current = source
        #if canImport(engine_mobileFFI)
            installExternalEventHandler(on: current)
            localAppsStore.configure { command in
                try await current.submitEngineCommand(command)
            }
            localAppsStore.configureManagedMcpCommands { command in
                let pluginCommand: PluginCommandDto
                switch command {
                case let .startAuthoring(appID, userGoal):
                    pluginCommand = .startLocalAppMcpAuthoring(
                        appId: appID,
                        userGoal: userGoal
                    )
                case let .setEnabled(appID, enabled, expectedRevision):
                    pluginCommand = .setLocalAppMcpEnabled(
                        appId: appID,
                        enabled: enabled,
                        expectedRevision: expectedRevision
                    )
                case let .setToolEnabled(appID, toolName, enabled, expectedRevision):
                    pluginCommand = .setLocalAppMcpToolEnabled(
                        appId: appID,
                        toolName: toolName,
                        enabled: enabled,
                        expectedRevision: expectedRevision
                    )
                case let .setConversationPinned(conversationID, appID, pinned):
                    pluginCommand = .setLocalAppMcpConversationPinned(
                        conversationId: conversationID,
                        appId: appID,
                        pinned: pinned
                    )
                }
                do {
                    try await current.submitEngineCommand(
                        .pluginCommand(command: pluginCommand)
                    )
                    return true
                } catch {
                    return false
                }
            }
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
        case let .localApp(appID):
            // The app's workspace directory is the session cwd, resolved via
            // the SAME validated derivation the code browser uses
            // (`LocalAppWorkspacePath`) — never a second hand-rolled path.
            project = nil
            // Validate via the shared derivation, but hand the ENGINE the
            // raw `appSandboxRoot + rel` composition — `validatedRoot`'s
            // symlink resolution can rewrite `/var/...` to `/private/var/...`
            // on device, and a cwd string that differs from the engine's own
            // composition by one byte lands the session catalog in a
            // different sanitized directory (the canonical-cwd fork).
            if (try? LocalAppWorkspacePath.validatedRoot(appID: appID)) != nil {
                projectCwd = ConversationSourceFactory.appSandboxRoot()
                    + "/" + LocalAppWorkspacePath.relativePath(appID: appID)
            } else {
                projectCwd = nil
            }
        }
        let runtime: TerminalRuntimeConfig?
        if let appID = scope.appID {
            // Mount this app's own workspace, never the terminal's default
            // workspace. The host cwd remains the session-catalog authority;
            // file and Shell tools receive its guest twin from PathAtlas.
            runtime = LocalAppWorkspacePath.mobileLinuxRuntimeConfig(appID: appID)
        } else {
            runtime = TerminalRuntimeDescriptor.make(
                appSandboxRoot: appSandboxRoot,
                project: project,
                linuxRuntime: settingsStore.linuxRuntime
            ).config
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
        let replacement = makeSource(scope: activeScope, snapshot: snapshot, mode: activeMode)
        #if canImport(engine_mobileFFI)
            installExternalEventHandler(on: replacement)
        #endif
        // Prepare the replacement before cancelling the current source. A
        // failed provider/catalog rebuild must leave the live conversation
        // usable instead of parking a cancelled source in the root view.
        try await replacement.prepare()
        try await old.cancelAndWait()
        activeSession = confirmedSession
        pendingSessionRestoreID = confirmedSession.isEmpty ? nil : confirmedSession
        source = replacement
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

    /// Thin project-flavored wrapper over `switchScope` — the drawer's
    /// project callbacks keep their optional-projectID spelling (`nil` ==
    /// global scope).
    private func switchProject(to projectID: String?, resumeSessionID: String? = nil, startNew: Bool = false) {
        switchScope(
            to: ConversationScope(projectID: projectID),
            resumeSessionID: resumeSessionID,
            startNew: startNew
        )
    }

    /// A first message queued for a freshly-created app's init session: sent
    /// once the engine confirms that session is live and the transcript is
    /// empty — the kickoff that starts the create-local-app flow (an empty
    /// anchor never runs a turn on its own).
    private struct PendingInitKickoff {
        let scope: ConversationScope
        /// The session to fire into, or `nil` when the switch started a FRESH
        /// conversation and the id does not exist yet. `nil` must still arm:
        /// the create landing takes this path whenever the engine's
        /// best-effort init-session mint failed, and gating on a known id
        /// dropped the brief that is the whole point of the create flow.
        let sessionID: String?
        let text: String
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

    @State private var pendingInitKickoff: PendingInitKickoff?

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
        initialPrompt: String? = nil,
        allowRecoveryRouting: Bool = false
    ) -> Bool {
        let targetMode = mode ?? activeMode
        guard !projectSwitching,
              allowRecoveryRouting || ConversationSessionMutationPolicy.allowsCallerMutation(
                  hasInactiveDurableRecovery: source.model.hasInactiveDurableRecovery,
                  hasUnresolvedTurnRecovery: source.model.hasUnresolvedTurnRecovery,
                  isCancelling: source.model.isCancelling
              )
        else { return false }
        voiceInteraction.handleContextChange()
        if let initialPrompt, resumeSessionID != nil || startNew {
            pendingInitKickoff = PendingInitKickoff(
                scope: scope, sessionID: resumeSessionID, text: initialPrompt)
        }
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
                // Only a project/global switch moves the durable active-project
                // selection. Entering a local-app scope leaves the project
                // selection untouched — leaving the app returns to it.
                if scope != activeScope, !scope.isLocalApp {
                    rollback = try await projectStore.persistActiveForSwitch(projectId: scope.projectID)
                }
                let replacement = makeSource(
                    scope: scope,
                    snapshot: providerRepository.makeLaunchSnapshot(),
                    mode: targetMode
                )
                #if canImport(engine_mobileFFI)
                    installExternalEventHandler(on: replacement)
                #endif
                try await replacement.prepare()
                activeScope = scope
                activeMode = targetMode
                collapsedWorkspaceKeys = scopedPreferences.workspaceCollapsedKeys(mode: targetMode)
                draft = scopedPreferences.draft(scope: scope, mode: targetMode)
                let restoredSession = restoredSessionID(scope: scope, mode: targetMode)
                confirmedSession = restoredSession
                activeSession = resumeSessionID ?? restoredSession
                pendingSessionRestoreID = activeSession.isEmpty ? nil : activeSession
                source = replacement
                sourceGeneration = UUID()
                if startNew {
                    pendingSessionRestoreID = nil
                    activeSession = ""
                    confirmedSession = ""
                    scopedPreferences.setActiveSessionID("", scope: scope, mode: targetMode)
                    replacement.startNewConversation()
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
                if let rollback { _ = try? await projectStore.rollbackActiveSwitch(rollback) }
                projectStore.errorMessage = String(localized: "project_switch_failed_message \(error.localizedDescription)")
                // The scope we armed the kickoff for is not the one we are in;
                // leaving it armed would fire the create-flow opener into
                // whatever session happens to match later.
                if pendingInitKickoff?.scope == scope { pendingInitKickoff = nil }
                if let pending = pendingSessionFork,
                   pending.sourceScope == scope,
                   pending.sourceMode == targetMode {
                    rollbackPendingSessionFork(pending, message: nil)
                }
                previousSource.handleForeground()
                previousSource.warmUp()
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
        // A `nil` sessionID means the switch started a fresh conversation, so
        // the FIRST session adopted in that scope is the one to fire into.
        if let kickoff = pendingInitKickoff,
           kickoff.sessionID == nil || kickoff.sessionID == sessionID,
           kickoff.scope == activeScope {
            pendingInitKickoff = nil
            // Only an EMPTY init session gets the kickoff — re-entering one
            // that already has a conversation must not re-trigger it.
            if source.model.items.isEmpty {
                _ = source.send(kickoff.text)
            }
        }
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
        // The session the kickoff was waiting for does not exist (the engine
        // replaced it with a fresh one). Fire into that replacement while the
        // transcript is still empty — the user's brief is the whole point of
        // the create flow — and drop the pending state either way so it can
        // never fire into an unrelated session later.
        if let kickoff = pendingInitKickoff, kickoff.sessionID == sessionID {
            pendingInitKickoff = nil
            if kickoff.scope == activeScope, source.model.items.isEmpty {
                _ = source.send(kickoff.text)
            }
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
        #if canImport(engine_mobileFFI)
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
            localAppsStore.sceneDidEnterBackground()
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
        case .active:
            conversationBackgroundExecution.setTurnActive(
                source.model.requiresBackgroundExecution,
                turnID: source.model.activeTurnToken?.clientTurnId
            )
            source.handleForeground()
            syncConversationBackgroundSurfaces()
            Task { await localAppsStore.sceneWillEnterForeground() }
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
            case .global:
                break
            case let .project(projectID):
                guard projectStore.projects.contains(where: { $0.record.id == projectID }) else {
                    return true
                }
            case let .localApp(appID):
                guard (try? LocalAppWorkspacePath.validatedRoot(appID: appID)) != nil else {
                    return true
                }
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
        case let .openLocalApp(appID, destination, autostart, _):
            Task {
                await openLocalAppFromDeepLink(
                    appID: appID,
                    destination: destination,
                    autostart: autostart
                )
            }
            return true
        }
    }

    private func handleIncomingURL(_ url: URL) {
        #if canImport(engine_mobileFFI)
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

    @discardableResult
    private func beginAppIntegratedConversation(draftText: String) -> Bool {
        guard ConversationSessionMutationPolicy.allowsCallerMutation(
            hasInactiveDurableRecovery: source.model.hasInactiveDurableRecovery,
            hasUnresolvedTurnRecovery: source.model.hasUnresolvedTurnRecovery,
            isCancelling: source.model.isCancelling
        ) else { return false }
        voiceInteraction.handleContextChange()
        pendingSessionRestoreID = nil
        activeSession = ""
        confirmedSession = ""
        scopedPreferences.setActiveSessionID("", scope: activeScope, mode: activeMode)
        draft = draftText
        source.startNewConversation()
        return true
    }

    private func openLocalAppFromDeepLink(
        appID: String,
        destination _: String,
        autostart: Bool
    ) async {
        await localAppsStore.refresh()
        guard let app = localAppsStore.app(id: appID) else {
            navigation.openLocalApps()
            localAppsStore.presentUnavailableAppError()
            return
        }
        let launchDestination: LocalAppLaunchDestination =
            app.workflow.isPublished ? .preview : .details
        localAppsStore.requestLaunch(appID: appID, destination: launchDestination)
        navigation.openLocalApps(appID: appID)
        guard app.workflow.isPublished, autostart else { return }
        await localAppsStore.start(appID: appID)
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
        guard !scope.isLocalApp else { return }
        let isIndexed: Bool
        if let projectID {
            isIndexed = projectStore.projects
                .first(where: { $0.record.id == projectID })?
                .sessions.contains(where: { $0.sessionId == sessionID }) == true
        } else {
            isIndexed = projectStore.globalSessions.contains(where: { $0.sessionId == sessionID })
        }
        Task { @MainActor in
            if !isIndexed {
                try? await projectStore.recordStartedSession(
                    projectId: projectID,
                    sessionId: sessionID,
                    title: String(localized: "chat_new_conversation"),
                    mode: sessionMode
                )
            }
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
        guard !scope.isLocalApp else { return }
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
