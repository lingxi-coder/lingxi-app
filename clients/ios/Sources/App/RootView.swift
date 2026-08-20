import SwiftUI
import UIKit

/// Holds UIKit's finite background-execution assertion while a user-started
/// conversation turn is active. Expiration only releases the assertion: it must
/// never manufacture a user cancellation or corrupt the engine's turn state.
@MainActor
final class ConversationBackgroundExecutionController {
    typealias ExpirationHandler = @MainActor @Sendable () -> Void
    typealias TaskIdentifier = UIBackgroundTaskIdentifier
    typealias BeginTask = (@escaping ExpirationHandler) -> TaskIdentifier
    typealias EndTask = (TaskIdentifier) -> Void

    private let beginTask: BeginTask
    private let endTask: EndTask
    private var taskIdentifier: TaskIdentifier?
    private var generation = 0

    init(beginTask: @escaping BeginTask, endTask: @escaping EndTask) {
        self.beginTask = beginTask
        self.endTask = endTask
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

    func setTurnActive(_ active: Bool) {
        if active {
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
        } else {
            endCurrentTask()
        }
    }

    private func expire(generation expiredGeneration: Int) {
        guard expiredGeneration == generation else { return }
        endCurrentTask()
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
    @State private var activeSession: String
    /// Last session confirmed by SessionStarted/SessionResumed. Drawer taps may
    /// update `activeSession` optimistically, but only this value is persisted.
    @State private var confirmedSession: String
    @State private var pendingSessionRestoreID: String?
    @State private var draft: String
    @State private var voiceInteraction: VoiceInteractionController
    @State private var conversationBackgroundExecution =
        ConversationBackgroundExecutionController.live()
    @State private var projectSwitching = false

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
        _providerRepository = State(initialValue: providers)
        _localAppsStore = State(initialValue: localApps)
        _clientEventCenter = State(initialValue: eventCenter)
        _source = State(initialValue: conversation)
        _activeScope = State(initialValue: ConversationScope(projectID: projectID))
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
        _voiceInteraction = State(
            initialValue: VoiceInteractionController(
                voiceCapture: VoiceCapture(),
                capability: voiceCapability,
                speechPlayer: SystemVoiceSpeechPlayer(),
                bargeInRecognizer: VoiceBargeInRecognizer()
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
        @Bindable var navigation = navigation
        ZStack {
            // Sidebar + chat. In compact width this collapses to a stack whose
            // root is the sidebar, which is what gives the chat a system back
            // button and a back-swipe for free.
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
                SetupWizardView(convo: source.model, onSetModel: { source.setModel($0) })
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
        .environment(\.locale, localization.effectiveLocale())
        .onChange(of: scenePhase, handleScenePhase)
        .onReceive(source.model.backgroundExecutionActivity) { active in
            conversationBackgroundExecution.setTurnActive(active)
        }
        .onChange(of: draft) { _, value in
            scopedPreferences.setDraft(value, scope: activeScope)
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

    private var sidebar: some View {
        Drawer(
            projectStore: projectStore,
            localAppsStore: localAppsStore,
            activeScope: activeScope,
            activeSession: $activeSession,
            source: source,
            openSettings: { navigation.showSettings() },
            openTerminal: openCurrentWorkspaceTerminal,
            openApps: { appID in navigation.openLocalApps(appID: appID) },
            onSelectProject: { switchProject(to: $0) },
            onSelectSession: { switchProject(to: $0, resumeSessionID: $1) },
            onNewChat: { switchProject(to: $0, startNew: true) },
            onSelectAppSession: { appID, sessionID in
                switchScope(to: .localApp(appID), resumeSessionID: sessionID)
            },
            onNewAppChat: { appID in
                switchScope(to: .localApp(appID), startNew: true)
            }
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
                availableModels: source.model.availableModels,
                activeModelID: source.model.activeModelId,
                onDismiss: { navigation.closePresentedRoute() },
                onOpenAppSession: openAppSession,
                onOpenCreatedAppSession: openCreatedAppSession,
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
                availableModels: source.model.availableModels,
                activeModelID: source.model.activeModelId,
                onDismiss: { navigation.closePresentedRoute() },
                onOpenAppSession: openAppSession,
                onOpenCreatedAppSession: openCreatedAppSession,
                onNewAppSession: startNewAppSession
            )
        case .sessionDetails:
            EmptyView()
        }
    }

    /// A tapped row of an app's session catalog: dismiss the local-apps cover
    /// and continue that conversation inside the app's scope.
    private func openAppSession(appID: String, sessionID: String) {
        navigation.closePresentedRoute()
        switchScope(to: .localApp(appID), resumeSessionID: sessionID)
    }

    /// The create-flow landing: same as `openAppSession`, plus the queued
    /// kickoff message that starts the create-local-app flow once the empty
    /// init session is live.
    private func openCreatedAppSession(
        appID: String,
        sessionID: String,
        brief: String,
        modelOverride _: String?
    ) {
        navigation.closePresentedRoute()
        let kickoff = String(localized: "local_apps_init_kickoff \(brief)")
        // The library consumed its one-shot signal to call this, so a refused
        // switch would lose the created app with nothing left to re-arm it.
        // `switchScope` refuses only while another switch is in flight, so
        // retry once that finishes.
        guard switchScope(
            to: .localApp(appID),
            resumeSessionID: sessionID,
            initialPrompt: kickoff
        ) else {
            Task { @MainActor in
                for _ in 0..<40 where projectSwitching {
                    try? await Task.sleep(for: .milliseconds(250))
                }
                guard !projectSwitching else { return }
                openCreatedAppSession(
                    appID: appID,
                    sessionID: sessionID,
                    brief: brief,
                    modelOverride: nil
                )
            }
            return
        }
    }

    /// 「新会话」in an app's session catalog: dismiss the cover and start a
    /// fresh conversation in the app's scope.
    private func startNewAppSession(appID: String) {
        navigation.closePresentedRoute()
        switchScope(to: .localApp(appID), startNew: true)
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

    private func wireCurrentSource(preserveCatalog: Bool = false) {
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

    private func makeSource(scope: ConversationScope, snapshot: ProviderLaunchSnapshot) -> any ConversationSource {
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
            providerConfigured: !snapshot.enabledProfileIDs.isEmpty,
            providerProfilesJson: snapshot.providerProfilesJSON,
            providerRoutingJson: snapshot.routingJSON,
            defaultModelID: snapshot.defaultModelID,
            mobileLinux: runtime
        )
    }

    private func rebuildSource(
        snapshot: ProviderLaunchSnapshot,
        preserveSourceGeneration: Bool = false
    ) async throws {
        persistConversationScope()
        let old = source
        let replacement = makeSource(scope: activeScope, snapshot: snapshot)
        #if canImport(engine_mobileFFI)
            replacement.setExternalEventHandler { event in
                Task { @MainActor in clientEventCenter.publish(event) }
            }
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
        let sessionID: String
        let text: String
    }

    @State private var pendingInitKickoff: PendingInitKickoff?

    /// Returns `false` when the switch was refused because another one is
    /// still in flight — callers holding a one-shot signal (the create-flow
    /// landing) must retry rather than drop it.
    @discardableResult
    private func switchScope(
        to scope: ConversationScope,
        resumeSessionID: String? = nil,
        startNew: Bool = false,
        initialPrompt: String? = nil
    ) -> Bool {
        guard !projectSwitching else { return false }
        voiceInteraction.handleContextChange()
        if let initialPrompt, let resumeSessionID {
            pendingInitKickoff = PendingInitKickoff(
                scope: scope, sessionID: resumeSessionID, text: initialPrompt)
        }
        if scope == activeScope {
            navigation.closeSidebar()
            if let resumeSessionID {
                pendingSessionRestoreID = resumeSessionID
                activeSession = resumeSessionID
                requestSessionResume(resumeSessionID, scope: scope, using: source)
            } else if startNew {
                pendingSessionRestoreID = nil
                activeSession = ""
                confirmedSession = ""
                scopedPreferences.setActiveSessionID("", scope: scope)
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
                try await previousSource.cancelAndWait()
                // Only a project/global switch moves the durable active-project
                // selection. Entering a local-app scope leaves the project
                // selection untouched — leaving the app returns to it.
                if !scope.isLocalApp {
                    rollback = try await projectStore.persistActiveForSwitch(projectId: scope.projectID)
                }
                let replacement = makeSource(scope: scope, snapshot: providerRepository.makeLaunchSnapshot())
                #if canImport(engine_mobileFFI)
                    replacement.setExternalEventHandler { event in
                        Task { @MainActor in clientEventCenter.publish(event) }
                    }
                #endif
                try await replacement.prepare()
                activeScope = scope
                draft = scopedPreferences.draft(scope: scope)
                let restoredSession = restoredSessionID(scope: scope)
                confirmedSession = restoredSession
                activeSession = resumeSessionID ?? restoredSession
                pendingSessionRestoreID = activeSession.isEmpty ? nil : activeSession
                source = replacement
                sourceGeneration = UUID()
                if startNew {
                    pendingSessionRestoreID = nil
                    activeSession = ""
                    confirmedSession = ""
                    scopedPreferences.setActiveSessionID("", scope: scope)
                    replacement.startNewConversation()
                } else if !activeSession.isEmpty {
                    requestSessionResume(activeSession, scope: scope, using: replacement)
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
                previousSource.handleForeground()
                previousSource.warmUp()
            }
            projectSwitching = false
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
        scopedPreferences.setActiveSessionID(sessionID, scope: activeScope)
        if pendingSessionRestoreID == sessionID {
            pendingSessionRestoreID = nil
        }
        if let kickoff = pendingInitKickoff, kickoff.sessionID == sessionID,
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
        scopedPreferences.setActiveSessionID(confirmedSession, scope: activeScope)
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
        scopedPreferences.setActiveSessionID(confirmedSession, scope: activeScope)
        return true
    }

    private func restoredSessionID(scope: ConversationScope) -> String {
        if let stored = scopedPreferences.storedActiveSessionID(scope: scope) {
            return stored
        }
        // Only managed projects carry a durable last-active-session record;
        // app scopes fall back to a fresh conversation.
        return scope.projectID.flatMap { id in
            projectStore.projects.first(where: { $0.record.id == id })?.record.lastActiveSessionId
        } ?? ""
    }

    /// A zero message count in the durable project index is the proof required
    /// by the migration-safe engine entrypoint. If a live catalog row is
    /// available it wins over the cache; otherwise startup/project switching can
    /// still restore legacy empty sessions before ListSessions replies.
    private func requestSessionResume(
        _ sessionID: String,
        scope: ConversationScope,
        using conversation: any ConversationSource
    ) {
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
        scopedPreferences.setDraft(draft, scope: activeScope)
        scopedPreferences.setActiveSessionID(confirmedSession, scope: activeScope)
    }

    private func handleScenePhase(_ oldPhase: ScenePhase, _ phase: ScenePhase) {
        switch phase {
        case .background:
            persistConversationScope()
            localAppsStore.sceneDidEnterBackground()
            voiceInteraction.handleBackground()
            conversationBackgroundExecution.setTurnActive(
                source.model.requiresBackgroundExecution)
            source.handleBackground()
            Task {
                await VoicePreviewPlayback.shared.stop()
                await VoiceAudioSessionCoordinator.shared.suspendForBackground()
            }
        case .active:
            conversationBackgroundExecution.setTurnActive(
                source.model.requiresBackgroundExecution)
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
        navigation.closeSidebar()
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
        case let .openLocalApp(appID, destination, autostart, _):
            Task {
                await openLocalAppFromDeepLink(
                    appID: appID,
                    destination: destination,
                    autostart: autostart
                )
            }
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
        applyAppAction(action)
    }

    private func beginAppIntegratedConversation(draftText: String) {
        voiceInteraction.handleContextChange()
        pendingSessionRestoreID = nil
        activeSession = ""
        confirmedSession = ""
        scopedPreferences.setActiveSessionID("", scope: activeScope)
        draft = draftText
        source.startNewConversation()
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
            app.workflow == .ready ? .preview : .details
        localAppsStore.requestLaunch(appID: appID, destination: launchDestination)
        navigation.openLocalApps(appID: appID)
        guard app.workflow == .ready, autostart else { return }
        await localAppsStore.start(appID: appID)
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
    /// A `.localApp` scope still adopts/persists the engine session id via the
    /// callbacks, but never writes into the PROJECT session index — an app's
    /// catalog is engine-owned (`ListAppSessions`), not project state.
    let scope: ConversationScope
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
                    title: String(localized: "chat_new_conversation")
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
