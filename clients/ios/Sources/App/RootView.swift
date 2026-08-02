import SwiftUI

/// App composition root. Every workspace-sensitive dependency is created from
/// the active project and replaced as one unit when the project or Provider
/// launch configuration changes.
@MainActor
struct RootView: View {
    @Environment(AppState.self) private var app
    @Environment(\.theme) private var theme
    @Environment(\.scenePhase) private var scenePhase

    @State private var settingsStore: SettingsStore
    @State private var navigation: AppNavigationModel
    @State private var projectStore: ProjectStore
    @State private var cronRepository: CronRepository
    @State private var providerRepository: ProviderRepository
    @State private var source: any ConversationSource
    @State private var sourceGeneration = UUID()
    @State private var activeSession: String
    @State private var draft: String
    @State private var voiceActive = false
    @State private var flowActive = false
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
        _providerRepository = State(initialValue: providers)
        _source = State(initialValue: conversation)
        _activeSession = State(initialValue: preferences.activeSessionID(projectID: projectID))
        _draft = State(initialValue: preferences.draft(projectID: projectID))
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
        }
        .onChange(of: scenePhase, handleScenePhase)
        .onChange(of: draft) { _, value in
            scopedPreferences.setDraft(value, projectID: projectStore.activeProjectId)
        }
        .onChange(of: providerRepository.syncRevision) { _, _ in
            settingsStore.llmProviders = providerRepository.legacyProviders()
        }
        .onReceive(NotificationCenter.default.publisher(for: .lingxiCronNotificationOpened)) { note in
            cronRepository.handleNotificationUserInfo(note.userInfo ?? [:])
            if let runID = note.userInfo?["lingxi.cron.run_id"] as? String {
                navigation.openCronRun(runID)
            }
        }
        .task(id: sourceGeneration) {
            wireCurrentSource()
            do {
                try await source.prepare()
                source.listSessions()
                await providerRepository.refreshCredentialStatus()
            } catch {
                source.warmUp()
            }
        }
        .task {
            await cronRepository.handleLaunch()
        }
        .sheet(
            isPresented: Binding(
                get: { navigation.settingsOpen },
                set: { if !$0 { navigation.closeSettings() } }
            )
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
    }

    private var rootSurface: some View {
        ZStack {
            theme.windowBg.ignoresSafeArea()
            ChatView(
                session: session,
                openDrawer: { withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) { navigation.showDrawer() } },
                voiceActive: $voiceActive,
                onEnterFlow: { withAnimation(.easeOut(duration: 0.4)) { flowActive = true } },
                draft: $draft,
                source: source,
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
                onSessionChanged: adoptEngineSession
            )
            .id(sourceGeneration)

            if navigation.drawerOpen {
                Drawer(
                    projectStore: projectStore,
                    cronRepository: cronRepository,
                    activeSession: $activeSession,
                    source: source,
                    onClose: { withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) { navigation.closeDrawer() } },
                    openSettings: { navigation.showSettings() },
                    openTerminal: { navigation.openTerminal(projectID: projectStore.activeProjectId) },
                    openCron: { scopeID, taskID in navigation.openCron(scopeID: scopeID, taskID: taskID) },
                    onSelectProject: { switchProject(to: $0) },
                    onSelectSession: { switchProject(to: $0, resumeSessionID: $1) },
                    onNewChat: { switchProject(to: $0, startNew: true) }
                )
                .id(sourceGeneration)
                .zIndex(50)
            }

            if voiceActive {
                VoiceFlowView(onRelease: { withAnimation(.easeOut(duration: 0.25)) { voiceActive = false } })
                    .zIndex(70)
                    .allowsHitTesting(false)
            }

            if flowActive {
                VoiceOrbView(
                    convo: source.model,
                    onSend: { source.send($0) },
                    onCancel: { source.cancel() },
                    onClose: { withAnimation(.easeOut(duration: 0.3)) { flowActive = false } }
                )
                .zIndex(72)
                .transition(.opacity)
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
                Task { @MainActor in providerRepository.handle(event: event) }
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
        old.cancel()
        let replacement = makeSource(projectID: projectStore.activeProjectId, snapshot: snapshot)
        #if canImport(engine_mobileFFI)
            replacement.setExternalEventHandler { event in
                Task { @MainActor in providerRepository.handle(event: event) }
            }
        #endif
        try await replacement.prepare()
        source = replacement
        sourceGeneration = UUID()
        if !activeSession.isEmpty { replacement.resumeSession(activeSession) }
        old.handleBackground()
    }

    private func switchProject(to projectID: String?, resumeSessionID: String? = nil, startNew: Bool = false) {
        guard !projectSwitching else { return }
        if projectID == projectStore.activeProjectId {
            navigation.closeDrawer()
            if let resumeSessionID {
                activeSession = resumeSessionID
                scopedPreferences.setActiveSessionID(resumeSessionID, projectID: projectID)
                source.resumeSession(resumeSessionID)
            } else if startNew {
                activeSession = ""
                scopedPreferences.setActiveSessionID("", projectID: projectID)
                source.startNewConversation()
            }
            return
        }

        persistConversationScope()
        let previousSource = source
        previousSource.cancel()
        projectSwitching = true
        navigation.closeDrawer()
        Task { @MainActor in
            var rollback: ProjectActiveSelectionRollback?
            do {
                rollback = try await projectStore.persistActiveForSwitch(projectId: projectID)
                let replacement = makeSource(projectID: projectID, snapshot: providerRepository.makeLaunchSnapshot())
                #if canImport(engine_mobileFFI)
                    replacement.setExternalEventHandler { event in
                        Task { @MainActor in providerRepository.handle(event: event) }
                    }
                #endif
                try await replacement.prepare()
                source = replacement
                sourceGeneration = UUID()
                draft = scopedPreferences.draft(projectID: projectID)
                activeSession = resumeSessionID ?? scopedPreferences.activeSessionID(projectID: projectID)
                if startNew {
                    activeSession = ""
                    replacement.startNewConversation()
                } else if !activeSession.isEmpty {
                    replacement.resumeSession(activeSession)
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

    private func adoptEngineSession(_ sessionID: String) {
        activeSession = sessionID
        scopedPreferences.setActiveSessionID(sessionID, projectID: projectStore.activeProjectId)
    }

    private func persistConversationScope() {
        scopedPreferences.setDraft(draft, projectID: projectStore.activeProjectId)
        scopedPreferences.setActiveSessionID(activeSession, projectID: projectStore.activeProjectId)
    }

    private func handleScenePhase(_ oldPhase: ScenePhase, _ phase: ScenePhase) {
        switch phase {
        case .background:
            persistConversationScope()
            voiceActive = false
            flowActive = false
            source.handleBackground()
            Task { await VoiceAudioSessionCoordinator.shared.suspendForBackground() }
        case .active:
            source.handleForeground()
            Task { await VoiceAudioSessionCoordinator.shared.resumeAfterForeground() }
            Task { await cronRepository.handleSceneBecameActive() }
        default:
            break
        }
    }
}

/// Mirrors only the current engine model into the matching project session
/// index. Its identity is replaced with the conversation source, so a stale
/// source can never mutate the newly-selected project.
private struct ConversationProjectBridge: View {
    @ObservedObject var model: ConversationModel
    @Bindable var projectStore: ProjectStore
    let projectID: String?
    let onSessionChanged: (String) -> Void
    @State private var sessionSyncTask: Task<Void, Never>?

    var body: some View {
        Color.clear
            .frame(width: 0, height: 0)
            .onAppear {
                guard model.engineSessionsLoaded else { return }
                synchronizeSessions(model.engineSessions)
            }
            .onChange(of: model.engineSessionsLoaded) { _, loaded in
                guard loaded else { return }
                synchronizeSessions(model.engineSessions)
            }
            .onChange(of: model.engineSessions) { _, rows in
                guard model.engineSessionsLoaded else { return }
                synchronizeSessions(rows)
            }
            .onDisappear {
                sessionSyncTask?.cancel()
                sessionSyncTask = nil
            }
            .onChange(of: model.activeSessionId) { _, sessionID in
                guard !sessionID.isEmpty else { return }
                onSessionChanged(sessionID)
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
    }

    private func synchronizeSessions(_ rows: [EngineSession]) {
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
