import SwiftUI

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
    @State private var activeSession: String
    /// Last session confirmed by SessionStarted/SessionResumed. Drawer taps may
    /// update `activeSession` optimistically, but only this value is persisted.
    @State private var confirmedSession: String
    @State private var pendingSessionRestoreID: String?
    @State private var draft: String
    @State private var voiceInteraction = VoiceInteractionController()
    @State private var projectSwitching = false

    private let appSandboxRoot: String
    private let scopedPreferences: ProjectScopedPreferences

    init() {
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
        let runtime = TerminalRuntimeDescriptor.make(
            appSandboxRoot: root,
            project: activeProject,
            linuxRuntime: settings.linuxRuntime
        ).config
        let conversation = ConversationSourceFactory.make(
            projectCwd: activeProject?.workspace.hostURL.path,
            providerConfigured: !snapshot.enabledProfileIDs.isEmpty,
            providerProfilesJson: snapshot.providerProfilesJSON,
            providerRoutingJson: snapshot.routingJSON,
            defaultModelID: snapshot.defaultModelID,
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
        _providerRepository = State(initialValue: providers)
        _localAppsStore = State(initialValue: localApps)
        _clientEventCenter = State(initialValue: eventCenter)
        _source = State(initialValue: conversation)
        let storedSessionID = preferences.storedActiveSessionID(projectID: projectID)
            ?? projects.activeProject?.record.lastActiveSessionId
            ?? ""
        _activeSession = State(initialValue: storedSessionID)
        _confirmedSession = State(initialValue: storedSessionID)
        _pendingSessionRestoreID = State(initialValue: storedSessionID.isEmpty ? nil : storedSessionID)
        let initialDraft = ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] == "1"
            ? ""
            : preferences.draft(projectID: projectID)
        _draft = State(initialValue: initialDraft)
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
        return SessionRef(id: activeSession, title: "新对话")
    }

    var body: some View {
        @Bindable var navigation = navigation
        NavigationStack(path: $navigation.path) {
            rootSurface
                .navigationDestination(for: AppRoute.self, destination: destination)
                .id(localization.language)
        }
        .environment(\.locale, localization.effectiveLocale())
        .onChange(of: scenePhase, handleScenePhase)
        .onChange(of: draft) { _, value in
            scopedPreferences.setDraft(value, projectID: projectStore.activeProjectId)
        }
        .onChange(of: providerRepository.syncRevision) { _, _ in
            settingsStore.llmProviders = providerRepository.legacyProviders()
        }
        .onChange(of: localAppsStore.requestedPresentationAppID) { _, appID in
            guard let requestedAppID = appID else { return }
            navigation.openLocalApps(appID: requestedAppID)
            _ = localAppsStore.consumeRequestedPresentationAppID()
        }
        .onReceive(NotificationCenter.default.publisher(for: .lingxiCronNotificationOpened)) { note in
            cronRepository.handleNotificationUserInfo(note.userInfo ?? [:])
            if let runID = note.userInfo?["lingxi.cron.run_id"] as? String {
                navigation.openCronRun(runID)
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: .lingxiAppActionPending)) { _ in
            Task { await consumePendingAppActions() }
        }
        .onReceive(NotificationCenter.default.publisher(for: UIApplication.didReceiveMemoryWarningNotification)) { _ in
            Task { await localAppsStore.handleMemoryWarning() }
        }
        .onOpenURL(perform: handleIncomingURL)
        .task(id: sourceGeneration) {
            let generation = sourceGeneration
            let current = source
            let sessionToRestore = pendingSessionRestoreID ?? activeSession
            wireCurrentSource()
            do {
                try await current.prepare()
                guard generation == sourceGeneration else { return }
                current.listSessions()
                if !sessionToRestore.isEmpty, !current.model.sessionTransitionPending {
                    requestSessionResume(
                        sessionToRestore,
                        projectID: projectStore.activeProjectId,
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
        .task {
            await cronRepository.handleLaunch()
        }
        .task {
            await consumePendingAppActions()
        }
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
                onRefreshMcp: { source.refreshMcpServers() },
                openTerminal: {
                    navigation.closeSettings()
                    navigation.openTerminal(projectID: projectStore.activeProjectId)
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
    }

    private var rootSurface: some View {
        ZStack {
            theme.windowBg.ignoresSafeArea()
            ChatView(
                session: session,
                openDrawer: { withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) { navigation.showDrawer() } },
                draft: $draft,
                voiceInteraction: voiceInteraction,
                source: source,
                onOpenVoiceSettings: { navigation.showSettings(.voice) },
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
                projectID: projectStore.activeProjectId,
                pendingRestoreID: pendingSessionRestoreID,
                onSessionChanged: adoptEngineSession,
                onUnavailableSession: clearUnavailableSession,
                onSessionTransitionFailed: rollbackFailedSession,
                onRefreshSessions: { source.listSessions() }
            )
            .id(sourceGeneration)

            if navigation.drawerOpen {
                Drawer(
                    projectStore: projectStore,
                    cronRepository: cronRepository,
                    localAppsStore: localAppsStore,
                    activeSession: $activeSession,
                    source: source,
                    onClose: { withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) { navigation.closeDrawer() } },
                    openSettings: { closeDrawerThen { navigation.showSettings() } },
                    openTerminal: {
                        closeDrawerThen {
                            navigation.openTerminal(projectID: projectStore.activeProjectId)
                        }
                    },
                    openCron: { scopeID, taskID in
                        closeDrawerThen { navigation.openCron(scopeID: scopeID, taskID: taskID) }
                    },
                    openApps: { appID in
                        closeDrawerThen { navigation.openLocalApps(appID: appID) }
                    },
                    onSelectProject: { switchProject(to: $0) },
                    onSelectSession: { switchProject(to: $0, resumeSessionID: $1) },
                    onNewChat: { switchProject(to: $0, startNew: true) }
                )
                .id(sourceGeneration)
                // The conditional insertion happens here, so the transition
                // must live on this boundary (a transition inside Drawer never
                // participates in RootView's if/else transaction).
                .transition(.move(edge: .leading).combined(with: .opacity))
                .zIndex(50)
            }

            if projectSwitching {
                ProgressView("正在切换项目…")
                    .padding(18)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
                    .zIndex(90)
            }

            if !app.setupDone {
                SetupWizardView(convo: source.model, onSetModel: { source.setModel($0) })
                    .zIndex(100)
                    .transition(.opacity)
            }
        }
        .animation(.spring(response: 0.32, dampingFraction: 0.86), value: navigation.drawerOpen)
    }

    /// Presentation state must change in a fresh transaction after the drawer
    /// starts leaving. Updating a sheet/full-screen route in the same animated
    /// transaction can make SwiftUI discard the presentation on iOS 26.
    private func closeDrawerThen(_ action: @escaping @MainActor () -> Void) {
        withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) {
            navigation.closeDrawer()
        }
        Task { @MainActor in
            await Task.yield()
            guard !navigation.drawerOpen else { return }
            action()
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
                onOpenRuntimeSettings: { popRoute(); navigation.showSettings(.linuxRuntime) },
                onRepairRuntime: { popRoute(); navigation.showSettings(.linuxRuntime) }
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
                onDismiss: { navigation.closePresentedRoute() }
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
                onDismiss: { navigation.closePresentedRoute() }
            )
        }
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

    private func wireCurrentSource() {
        let current = source
        #if canImport(engine_mobileFFI)
            current.setExternalEventHandler { event in
                Task { @MainActor in
                    clientEventCenter.publish(event)
                }
            }
            localAppsStore.configure { command in
                try await current.submitEngineCommand(command)
            }
            providerRepository.configure(
                submitCommand: { command in try await current.submitEngineCommand(command) },
                testConnection: { profile, secret in
                    try await current.testProviderConnection(profile: profile, credentialOverride: secret)
                },
                applyReconnect: { snapshot in
                    try await rebuildSource(snapshot: snapshot)
                }
            )
        #else
            providerRepository.configure(submitCommand: nil)
        #endif
    }

    private func makeSource(projectID: String?, snapshot: ProviderLaunchSnapshot) -> any ConversationSource {
        let project = projectID.flatMap { id in
            projectStore.projects.first(where: { $0.record.id == id })
        }
        let runtime = TerminalRuntimeDescriptor.make(
            appSandboxRoot: appSandboxRoot,
            project: project,
            linuxRuntime: settingsStore.linuxRuntime
        ).config
        return ConversationSourceFactory.make(
            projectCwd: project?.workspace.hostURL.path,
            providerConfigured: !snapshot.enabledProfileIDs.isEmpty,
            providerProfilesJson: snapshot.providerProfilesJSON,
            providerRoutingJson: snapshot.routingJSON,
            defaultModelID: snapshot.defaultModelID,
            mobileLinux: runtime
        )
    }

    private func rebuildSource(snapshot: ProviderLaunchSnapshot) async throws {
        persistConversationScope()
        let old = source
        try await old.cancelAndWait()
        let replacement = makeSource(projectID: projectStore.activeProjectId, snapshot: snapshot)
        #if canImport(engine_mobileFFI)
            replacement.setExternalEventHandler { event in
                Task { @MainActor in clientEventCenter.publish(event) }
            }
        #endif
        try await replacement.prepare()
        activeSession = confirmedSession
        pendingSessionRestoreID = confirmedSession.isEmpty ? nil : confirmedSession
        source = replacement
        sourceGeneration = UUID()
        if !confirmedSession.isEmpty {
            requestSessionResume(
                confirmedSession,
                projectID: projectStore.activeProjectId,
                using: replacement
            )
        }
        old.handleBackground()
    }

    private func switchProject(to projectID: String?, resumeSessionID: String? = nil, startNew: Bool = false) {
        guard !projectSwitching else { return }
        voiceInteraction.handleContextChange()
        if projectID == projectStore.activeProjectId {
            navigation.closeDrawer()
            if let resumeSessionID {
                pendingSessionRestoreID = resumeSessionID
                activeSession = resumeSessionID
                requestSessionResume(resumeSessionID, projectID: projectID, using: source)
            } else if startNew {
                pendingSessionRestoreID = nil
                activeSession = ""
                confirmedSession = ""
                scopedPreferences.setActiveSessionID("", projectID: projectID)
                source.startNewConversation()
            }
            return
        }

        persistConversationScope()
        let previousSource = source
        projectSwitching = true
        navigation.closeDrawer()
        Task { @MainActor in
            var rollback: ProjectActiveSelectionRollback?
            do {
                try await previousSource.cancelAndWait()
                rollback = try await projectStore.persistActiveForSwitch(projectId: projectID)
                let replacement = makeSource(projectID: projectID, snapshot: providerRepository.makeLaunchSnapshot())
                #if canImport(engine_mobileFFI)
                    replacement.setExternalEventHandler { event in
                        Task { @MainActor in clientEventCenter.publish(event) }
                    }
                #endif
                try await replacement.prepare()
                draft = scopedPreferences.draft(projectID: projectID)
                let restoredSession = restoredSessionID(projectID: projectID)
                confirmedSession = restoredSession
                activeSession = resumeSessionID ?? restoredSession
                pendingSessionRestoreID = activeSession.isEmpty ? nil : activeSession
                source = replacement
                sourceGeneration = UUID()
                if startNew {
                    pendingSessionRestoreID = nil
                    activeSession = ""
                    confirmedSession = ""
                    scopedPreferences.setActiveSessionID("", projectID: projectID)
                    replacement.startNewConversation()
                } else if !activeSession.isEmpty {
                    requestSessionResume(activeSession, projectID: projectID, using: replacement)
                }
                previousSource.handleBackground()
                await cronRepository.refresh()
            } catch {
                if let rollback { _ = try? await projectStore.rollbackActiveSwitch(rollback) }
                projectStore.errorMessage = "项目切换失败：\(error.localizedDescription)"
                previousSource.handleForeground()
                previousSource.warmUp()
            }
            projectSwitching = false
        }
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
        scopedPreferences.setActiveSessionID(sessionID, projectID: projectStore.activeProjectId)
        if pendingSessionRestoreID == sessionID {
            pendingSessionRestoreID = nil
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
        scopedPreferences.setActiveSessionID(confirmedSession, projectID: projectStore.activeProjectId)
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
        scopedPreferences.setActiveSessionID(confirmedSession, projectID: projectStore.activeProjectId)
        return true
    }

    private func restoredSessionID(projectID: String?) -> String {
        if let stored = scopedPreferences.storedActiveSessionID(projectID: projectID) {
            return stored
        }
        return projectID.flatMap { id in
            projectStore.projects.first(where: { $0.record.id == id })?.record.lastActiveSessionId
        } ?? ""
    }

    /// A zero message count in the durable project index is the proof required
    /// by the migration-safe engine entrypoint. If a live catalog row is
    /// available it wins over the cache; otherwise startup/project switching can
    /// still restore legacy empty sessions before ListSessions replies.
    private func requestSessionResume(
        _ sessionID: String,
        projectID: String?,
        using conversation: any ConversationSource
    ) {
        let emptyTitle: String?
        if let live = conversation.model.engineSessions.first(where: { $0.id == sessionID }) {
            emptyTitle = live.messageCount == 0 ? live.title : nil
        } else {
            let cachedRows = projectID.flatMap { id in
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
        scopedPreferences.setDraft(draft, projectID: projectStore.activeProjectId)
        scopedPreferences.setActiveSessionID(confirmedSession, projectID: projectStore.activeProjectId)
    }

    private func handleScenePhase(_ oldPhase: ScenePhase, _ phase: ScenePhase) {
        switch phase {
        case .background:
            persistConversationScope()
            localAppsStore.sceneDidEnterBackground()
            voiceInteraction.handleBackground()
            source.handleBackground()
            Task {
                await VoicePreviewPlayback.shared.stop()
                await VoiceAudioSessionCoordinator.shared.suspendForBackground()
            }
        case .active:
            source.handleForeground()
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
            applyAppAction(action)
        }
    }

    private func applyAppAction(_ action: LingxiAppAction) {
        navigation.closeDrawer()
        navigation.closeSettings()
        navigation.closePresentedRoute()
        navigation.path.removeAll()

        switch action {
        case .openApp:
            break
        case .newConversation:
            beginAppIntegratedConversation(draftText: "")
        case let .ask(question):
            let trimmed = question.trimmingCharacters(in: .whitespacesAndNewlines)
            beginAppIntegratedConversation(draftText: trimmed)
        case let .openTerminal(sessionID, initialCommand):
            navigation.openTerminal(
                sessionID: sessionID,
                initialCommand: initialCommand,
                projectID: projectStore.activeProjectId
            )
        }
    }

    private func handleIncomingURL(_ url: URL) {
        guard let action = LingxiDeepLink.action(from: url) else { return }
        applyAppAction(action)
    }

    private func beginAppIntegratedConversation(draftText: String) {
        voiceInteraction.handleContextChange()
        pendingSessionRestoreID = nil
        activeSession = ""
        confirmedSession = ""
        scopedPreferences.setActiveSessionID("", projectID: projectStore.activeProjectId)
        draft = draftText
        source.startNewConversation()
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
    let projectID: String?
    let pendingRestoreID: String?
    let onSessionChanged: (String) -> Bool
    let onUnavailableSession: (String) -> Bool
    let onSessionTransitionFailed: (String) -> Bool
    let onRefreshSessions: () -> Void
    @State private var sessionSyncTask: Task<Void, Never>?

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
                    title: "新对话"
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
                updatedAt: Date()
            )
        }
        sessionSyncTask?.cancel()
        sessionSyncTask = Task { @MainActor in
            guard !Task.isCancelled else { return }
            try? await projectStore.syncEngineSessions(projectId: projectID, rows: summaries)
        }
    }
}
