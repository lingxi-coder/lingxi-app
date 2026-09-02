import Foundation
import Observation

@Observable
@MainActor
final class LocalAppsStore {
    /// Prompts waiting behind the one on screen. A page cannot grow this
    /// without bound; past the cap the newest request is refused.
    private static let maxQueuedPermissions = 8

    private enum PendingPermissionSource {
        #if canImport(engine_mobileFFI)
            case ui(AppUiRequestDto)
            case capability(appID: String, kind: AppCapabilityKindDto)
        #endif
    }

    private(set) var apps: [LocalAppSummary] = []
    private(set) var runtimes: [String: LocalAppRuntimeStatus] = [:]
    /// Host-derived runtime-profile health cached from the latest details
    /// snapshot. App list records intentionally do not duplicate this field;
    /// retaining the cache lets cards show a known status without making the
    /// list endpoint guess or rescan the app workspace.
    private(set) var runtimeProfileStatuses: [String: LocalAppRuntimeProfileStatus] = [:]
    /// Manifest-declared data collections per app (v3: the manifest is the
    /// only surviving structured description of an app's data model — the
    /// LLM-derived plan is gone).
    private(set) var collections: [String: [LocalAppDataCollection]] = [:]
    /// One requested page of each app's workspace-scoped session catalog
    /// (`ListAppSessions` → `AppSessionsChanged`), init row pinned first.
    private(set) var sessionPages: [String: LocalAppSessionPage] = [:]
    /// Apps with an in-flight `llm.chat`. Drives the "calling AI" indicator;
    /// the engine emits this in on/off pairs.
    private(set) var llmActiveAppIDs: Set<String> = []
    /// Unread mailbox events per app, for a badge. The event carries no body
    /// on purpose — the payload is read by the assistant through MCP.
    private(set) var unreadAgentEvents: [String: Int] = [:]
    private(set) var checkpoints: [String: [LocalAppCheckpoint]] = [:]
    private(set) var isRefreshing = false
    private(set) var errorMessage: String?
    private(set) var builtinPluginInventory: LocalAppBuiltinPluginInventory?
    private(set) var builtinPluginStatus: LocalAppBuiltinPluginStatus?
    private(set) var builtinPluginCommandError: String?
    private(set) var managedMcpInventories: [String: LocalAppManagedMcpInventory] = [:]
    private(set) var managedMcpCommandErrors: [String: String] = [:]
    private(set) var pendingManagedMcpAppIDs: Set<String> = []
    private(set) var lastRefreshAt: Date?
    /// The id of the app the last `createApp` produced, consumed once by the
    /// library so the user lands on the new app's detail screen. The FALLBACK
    /// landing — the preferred signal is [`createdAppLanding`], which opens the
    /// app's own conversation.
    ///
    /// Armed ONLY for a create started from inside the library cover, the one
    /// surface that consumes it — see [`pendingCreateArmsLibraryFallback`].
    private(set) var createdAppID: String?
    /// Where a freshly created app hands the user off: into the app's OWN
    /// conversation, whose cwd is the app workspace.
    ///
    /// The init session is not known at `AppCreated` time — the service emits
    /// that event as part of the create transaction and the engine pins the
    /// app's own session AFTERWARDS (`local_apps_mcp.rs`, best-effort,
    /// announced by `AppRecordChanged`). Arming on `AppCreated` and filling the
    /// pin in later is what keeps the hand-off from being silently dropped.
    struct CreatedAppLanding: Equatable {
        let appID: String
        /// `nil` until the pin lands — and permanently `nil` if the best-effort
        /// mint failed, in which case the hand-off opens a fresh conversation
        /// in the app's scope. That is still rooted in the app workspace.
        var initSessionID: String?
    }

    struct PendingWidgetSetup: Identifiable, Equatable {
        let appID: String
        let appName: String

        var id: String { appID }
    }

    private(set) var pendingWidgetSetup: PendingWidgetSetup?
    private(set) var pendingPermission: LocalAppPermissionPrompt?
    private(set) var pendingDependencyChangeConfirmation: LocalAppDependencyChangeConfirmationPrompt?
    private(set) var pendingCreateConfirmation: LocalAppCreateConfirmationPrompt?
    private(set) var pendingMcpProposalApproval: LocalAppMcpProposalApprovalPrompt?
    private(set) var pendingProfileProposal: LocalAppProfileProposal?
    private(set) var requestedPresentationAppID: String?
    /// requestId → appID for every UI request still awaiting a decision.
    ///
    /// A MAP, not one slot: the permission queue is nine deep and multi-app, so
    /// a single slot let app B's request overwrite app A's, and resolving
    /// either one cleared it for both — leaving a queued request whose app no
    /// longer routed to its preview.
    private(set) var pendingUIRequestAppIDs: [String: String] = [:]

    var searchQuery = ""

    @ObservationIgnored private var runningBeforeSuspension = Set<String>()
    /// Where to take the user once the app exists — its own conversation.
    /// Consumed by `RootView`, which waits for any in-flight turn to end first:
    /// `switchScope` submits `cancelAndWait()`, so landing mid-turn would kill
    /// the very turn that produced the app.
    private(set) var createdAppLanding: CreatedAppLanding?
    /// The landing built on `AppCreated`, held until `AppRecordChanged` can
    /// attach the init-session pin the engine mints immediately afterwards.
    @ObservationIgnored private var landingAwaitingPin: CreatedAppLanding?

    /// The `request_id` of the ONE `CreateApp` this client started and has not
    /// yet seen resolve, or `nil` when nothing is in flight.
    ///
    /// A CORRELATION KEY, not a boolean and not a brief. The engine emits
    /// `AppCreated` for BOTH creation paths, so a bare flag claims whichever
    /// app committed first — under concurrent creation that is the one an
    /// agent created in another conversation, and the user is hijacked into
    /// someone else's app. Matching the brief (what this used to do) is no
    /// longer possible either: a shell create sends `brief: ""`, so every
    /// concurrent shell would match every other one.
    @ObservationIgnored private var pendingCreateRequestID: String?
    /// Whether the create in flight should arm [`createdAppID`] — the library's
    /// FALLBACK landing — when it resolves.
    ///
    /// `false` for a create started with NO `LocalAppsRootView` mounted: the
    /// drawer's affordances. That fallback has exactly one consumer,
    /// `LocalAppsLibraryView.openCreatedAppIfNeeded`, driven by the library
    /// screen's `onAppear`/`onChange(of: store.createdAppID)`. With the cover
    /// never mounted, nothing consumes the id and it survives for the process
    /// lifetime. It used to be harmless because a drawer create opened the
    /// library first, so the id was always drained while the app was still an
    /// unscaffolded shell and hit that fallback's own early-out. Now the create
    /// conversation SCAFFOLDS the app, so a later, unrelated "View all" would
    /// drain the stale id and drop the user on that old app's details page
    /// instead of the library list.
    ///
    /// Refused at the SOURCE rather than drained at the landing: the primary
    /// landing rides on `AppRecordChanged`, which is not guaranteed to arrive,
    /// and a drain that never runs leaves exactly the stale id this prevents.
    ///
    /// Initialised and reset to `false` — the REFUSING value — so the unsafe
    /// behaviour is never what a caller gets by accident. Every create sets it
    /// explicitly from its own argument (see `createShellApp`), so this value
    /// governs only an event arriving with no create in flight, where arming a
    /// landing nobody asked for is the bug. Android's twin field is `false`
    /// for the same reason.
    @ObservationIgnored private var pendingCreateArmsLibraryFallback = false
    /// Stop-loss timer for [`pendingCreateRequestID`]. Cancelled the moment
    /// the create resolves either way.
    @ObservationIgnored private var createResultTimeoutTask: Task<Void, Never>?

    /// Offset each in-flight `ListAppSessions` was issued at, so the reply can
    /// be reduced as a replace (offset 0) or an append (later pages) — the
    /// `AppSessionsChanged` event does not echo the requested offset.
    @ObservationIgnored private var pendingSessionRequestOffsets: [String: UInt64] = [:]
    @ObservationIgnored private var pendingPermissionSource: PendingPermissionSource?
    #if canImport(engine_mobileFFI)
        /// One page can raise several capability requests in a single tick (two
        /// `fetch()` calls to two unauthorized domains). Each one is waiting on
        /// its own 5-minute approval timeout, so a second request must queue
        /// behind the first rather than replace it.
        @ObservationIgnored private var permissionQueue: [(
            prompt: LocalAppPermissionPrompt,
            source: PendingPermissionSource
        )] = []
        @ObservationIgnored private var dependencyChangeConfirmationQueue: [LocalAppDependencyChangeConfirmationPrompt] = []
        @ObservationIgnored private var createConfirmationQueue: [LocalAppCreateConfirmationPrompt] = []
        @ObservationIgnored private var mcpProposalApprovalQueue: [LocalAppMcpProposalApprovalPrompt] = []
    #endif
    @ObservationIgnored private var approvedUIAutomation: [String: LocalAppCapabilityDecision] = [:]
    @ObservationIgnored private var runtimeLastUsedAt: [String: Date] = [:]
    @ObservationIgnored private var pendingBuiltinPluginEnabled: Bool?
    @ObservationIgnored private let websiteDataStoreRegistry: LocalAppWebsiteDataStoreRegistry
    @ObservationIgnored private var websiteDataCleanupTask: Task<Void, Never>?
    /// Coalesces simultaneous library/detail refreshes into one bridge call.
    @ObservationIgnored private var refreshTask: Task<Void, Never>?
    @ObservationIgnored private var detailsTasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var startTasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var pendingLaunchDestinations: [String: LocalAppLaunchDestination] = [:]
    @ObservationIgnored private var widgetSnapshotTask: Task<Void, Never>?
    /// Tests replace this to simulate App Group / write failures. Production
    /// keeps the default publisher that writes the shared snapshot file.
    @ObservationIgnored
    var widgetSnapshotPublisher: (LocalAppWidgetSnapshot) -> Error? = {
        LocalAppWidgetSnapshotStore.publish($0)
    }

    #if canImport(engine_mobileFFI)
        @ObservationIgnored private var submitCommand: ((ClientCommand) async throws -> Void)?
        @ObservationIgnored private var submitManagedMcpCommand:
            ((LocalAppManagedMcpCommand) async -> Bool)?
        @ObservationIgnored private var pendingBackgroundMutationRequests: Set<String> = []

        private enum PendingApprovalKind {
            case createConfirmation
            case mcpProposal
        }
    #endif

    init(websiteDataStoreRegistry: LocalAppWebsiteDataStoreRegistry? = nil) {
        self.websiteDataStoreRegistry = websiteDataStoreRegistry ?? .shared
    }

    var filteredApps: [LocalAppSummary] {
        let query = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return apps }
        return apps.filter { app in
            // `displayName`, so a query only ever matches text the user can
            // actually see on the card. A shell's stored `name` is an engine
            // placeholder that is never rendered; matching it would surface a
            // card whose visible title has nothing to do with the query.
            app.displayName.localizedStandardContains(query)
                || app.workflow.label.localizedStandardContains(query)
        }
    }

    var distributionMode: LocalAppsDistributionMode { .current }
    var builtinPluginDescriptor: LocalAppBuiltinPluginDescriptor {
        if let inventory = builtinPluginInventory {
            return LocalAppBuiltinPluginDescriptor(
                pluginID: inventory.pluginID,
                displayName: inventory.displayName,
                version: inventory.version,
                archiveDigest: inventory.bundleDigest,
                skillCount: inventory.skillCount,
                agentCount: inventory.agentCount,
                workflowCount: inventory.workflowCount,
                templateCount: inventory.templateCount,
                defaultEnabled: inventory.manifestDefaultEnabled
            )
        }
        return .current
    }

    var builtinPluginEffectiveEnabled: Bool {
        pendingBuiltinPluginEnabled
            ?? builtinPluginStatus?.isEnabled
            ?? builtinPluginDescriptor.defaultEnabled
    }

    func clearBuiltinPluginCommandError() {
        builtinPluginCommandError = nil
    }

    func managedMcpInventory(
        serverName: String,
        appSandboxRoot: String? = nil
    ) -> LocalAppManagedMcpInventory? {
        managedMcpInventories[serverName] ?? LocalAppManagedMcpInventoryReader(
            appSandboxRoot: appSandboxRoot ?? ConversationSourceFactory.appSandboxRoot()
        ).read(serverName: serverName, apps: apps)
    }

    func managedMcpInventory(
        appID: String,
        appSandboxRoot: String? = nil
    ) -> LocalAppManagedMcpInventory {
        if let inventory = managedMcpInventories.values.first(where: { $0.appID == appID }) {
            return inventory
        }
        if let persisted = managedMcpInventory(
            serverName: "local_app_\(appID)",
            appSandboxRoot: appSandboxRoot
        ) {
            return persisted
        }
        let publicationState = app(id: appID)?.workflow ?? .draft
        return LocalAppManagedMcpInventory.placeholder(
            appID: appID,
            appName: app(id: appID)?.displayName ?? appID,
            publicationState: publicationState
        )
    }

    func managedMcpCommandError(appID: String) -> String? {
        managedMcpCommandErrors[appID]
    }

    func clearManagedMcpCommandError(appID: String) {
        managedMcpCommandErrors.removeValue(forKey: appID)
    }

    func isManagedMcpPending(appID: String) -> Bool {
        pendingManagedMcpAppIDs.contains(appID)
    }

    #if canImport(engine_mobileFFI)
        func configure(submit: @escaping (ClientCommand) async throws -> Void) {
            submitCommand = submit
        }

        func configureManagedMcpCommands(
            submit: @escaping (LocalAppManagedMcpCommand) async -> Bool
        ) {
            // Wired by the engine integration once the dedicated managed-MCP
            // PluginCommandDto cases land in the generated bindings.
            submitManagedMcpCommand = submit
        }

        func refreshBuiltinPluginStatus() async {
            builtinPluginCommandError = nil
            async let statusSent = send(
                .pluginCommand(
                    command: .getStatus(pluginId: builtinPluginDescriptor.pluginID)
                )
            )
            async let inventorySent = send(
                .pluginCommand(
                    command: .getInventory(pluginId: builtinPluginDescriptor.pluginID)
                )
            )
            async let managedInventorySent = send(
                .pluginCommand(command: .getManagedMcpInventory)
            )
            let sent = await statusSent
            let inventory = await inventorySent
            let managed = await managedInventorySent
            if !sent || !inventory || !managed {
                builtinPluginCommandError = String(localized: "local_apps_plugin_status_unavailable")
            }
        }

        func setBuiltinPluginEnabled(_ enabled: Bool) async {
            pendingBuiltinPluginEnabled = enabled
            builtinPluginCommandError = nil
            let sent = await send(
                .pluginCommand(
                    command: .setEnabled(
                        pluginId: builtinPluginDescriptor.pluginID,
                        enabled: enabled
                    )
                )
            )
            if !sent {
                pendingBuiltinPluginEnabled = nil
                builtinPluginCommandError = String(localized: "local_apps_plugin_toggle_failed")
            }
        }

        func resolvePendingCreateConfirmation(_ approved: Bool) async {
            guard let prompt = pendingCreateConfirmation else { return }
            pendingCreateConfirmation = nil
            defer { presentNextCreateConfirmation() }
            _ = await send(
                .pluginCommand(
                    command: .resolveCreateConfirmation(
                        requestId: prompt.requestID,
                        approved: approved
                    )
                )
            )
        }

        func resolvePendingMcpProposalApproval(_ approved: Bool) async {
            guard let prompt = pendingMcpProposalApproval else { return }
            pendingMcpProposalApproval = nil
            defer { presentNextMcpProposalApproval() }
            _ = await send(
                .pluginCommand(
                    command: .resolveMcpProposalApproval(
                        requestId: prompt.requestID,
                        approved: approved
                    )
                )
            )
        }

        func refreshManagedMcpInventory() async {
            _ = await send(.pluginCommand(command: .getManagedMcpInventory))
        }

        func startManagedMcpAuthoring(appID: String, userGoal: String) async -> Bool {
            return await submitManagedMcpCommand(
                appID: appID,
                optimistic: nil,
                command: .startAuthoring(appID: appID, userGoal: userGoal)
            )
        }

        func setManagedMcpEnabled(appID: String, enabled: Bool) async -> Bool {
            let inventory = managedMcpInventory(appID: appID)
            return await submitManagedMcpCommand(
                appID: appID,
                optimistic: inventory.updatingEnabled(enabled),
                command: .setEnabled(
                    appID: appID,
                    enabled: enabled,
                    expectedRevision: inventory.settingsRevision
                )
            )
        }

        func setManagedMcpToolEnabled(
            appID: String,
            toolName: String,
            enabled: Bool
        ) async -> Bool {
            let inventory = managedMcpInventory(appID: appID)
            return await submitManagedMcpCommand(
                appID: appID,
                optimistic: inventory.updatingTool(toolName, enabled: enabled),
                command: .setToolEnabled(
                    appID: appID,
                    toolName: toolName,
                    enabled: enabled,
                    expectedRevision: inventory.settingsRevision
                )
            )
        }

        func setManagedMcpConversationPinned(
            conversationID: String,
            appID: String,
            pinned: Bool
        ) async -> Bool {
            let inventory = managedMcpInventory(appID: appID)
            return await submitManagedMcpCommand(
                appID: appID,
                optimistic: inventory.updatingConversationPinned(pinned),
                command: .setConversationPinned(
                    conversationID: conversationID,
                    appID: appID,
                    pinned: pinned
                )
            )
        }

        func resolveProfileProposal(_ approved: Bool) {
            guard let proposal = pendingProfileProposal else { return }
            pendingProfileProposal = nil
            guard let submitCommand else { return }
            Task {
                do {
                    try await submitCommand(.resolveAppProfileProposal(
                        appId: proposal.appID,
                        approvalToken: proposal.approvalToken,
                        approved: approved
                    ))
                } catch {
                    errorMessage = error.localizedDescription
                }
            }
        }

        func handle(event: ClientEvent) {
            switch event {
            case let .appsChanged(records):
                let updatedApps = records.map { record in
                    hydratedSummary(for: record)
                }.sorted {
                    $0.updatedAt > $1.updatedAt
                }
                let liveAppIDs = Set(updatedApps.map(\.id))
                runtimeProfileStatuses = runtimeProfileStatuses.filter { liveAppIDs.contains($0.key) }
                managedMcpInventories = managedMcpInventories.filter { liveAppIDs.contains($0.value.appID) }
                sessionPages = sessionPages.filter { liveAppIDs.contains($0.key) }
                pendingSessionRequestOffsets = pendingSessionRequestOffsets.filter {
                    liveAppIDs.contains($0.key)
                }
                apps = updatedApps
                let missingSessionAppIDs = updatedApps.map(\.id).filter {
                    sessionPages[$0] == nil && pendingSessionRequestOffsets[$0] == nil
                }
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    for appID in missingSessionAppIDs {
                        await self.listSessions(appID: appID)
                    }
                }
                scheduleWidgetSnapshotPublish()
                scheduleWebsiteDataCleanup(activeAppIDs: Set(updatedApps.map(\.id)))
                // No claim here any more.
                //
                // Identifying "the app I asked for" by diffing the catalog was
                // only ever an inference, and the deferred create flow
                // invalidates it: minutes pass between the brief and the create,
                // the agent rewrites the brief, and the user can create
                // something else meanwhile. `AppEventDto.appCreated` names the
                // record the engine just committed — see `handleAppEvent`.
                lastRefreshAt = .now
                isRefreshing = false

            case let .appWorkflowChanged(appId, state, detail):
                let workflow = LocalAppsProtocolAdapter.workflow(state)
                updateApp(appID: appId) { app in
                    app.workflow = workflow
                    app.updatedAt = .now
                }
                scheduleWidgetSnapshotPublish()
                if let detail, !detail.isEmpty { errorMessage = detail }

            case let .appRuntimeChanged(appId, state, details, lastError):
                runtimes[appId] = LocalAppsProtocolAdapter.runtime(
                    state,
                    details: details,
                    lastError: lastError,
                    knownURL: runtimes[appId]?.url
                )
                scheduleWidgetSnapshotPublish()
                if state == .running, runtimeLastUsedAt[appId] == nil {
                    runtimeLastUsedAt[appId] = .now
                }

            case let .appEvent(event):
                handleAppEvent(event)

            case let .appSessionsChanged(appId, sessions, nextOffset):
                let rows = sessions.map(LocalAppsProtocolAdapter.sessionRow)
                let requestedOffset = pendingSessionRequestOffsets.removeValue(forKey: appId) ?? 0
                var page = (requestedOffset > 0 ? sessionPages[appId] : nil) ?? LocalAppSessionPage()
                if requestedOffset > 0 {
                    let known = Set(page.rows.map(\.uuid))
                    page.rows.append(contentsOf: rows.filter { !known.contains($0.uuid) })
                } else {
                    page.rows = rows
                }
                // The pinned init session lists first; the rest keep the
                // wire's modified-descending order (a stable partition, so
                // paging appends never reshuffle earlier rows).
                page.rows = page.rows.filter(\.isInit) + page.rows.filter { !$0.isInit }
                page.nextOffset = nextOffset
                sessionPages[appId] = page

            case let .appCheckpointCreated(appId, checkpoint):
                let item = LocalAppsProtocolAdapter.checkpoint(checkpoint)
                var values = checkpoints[appId] ?? []
                values.removeAll { $0.id == item.id }
                values.append(item)
                checkpoints[appId] = values.sorted { $0.createdAt > $1.createdAt }

            case let .appOperationFailed(_, _, message, requestId):
                isRefreshing = false
                errorMessage = message
                // Disarm ONLY on the failure of the create this client
                // started. `app_id == nil` used to stand in for that, but it
                // is not a correlation key: an agent-tool create that fails
                // before it has a record also reports no app id, and letting
                // that disarm this create means the app the user just asked
                // for lands in the library with nobody waiting to open it.
                // A create failure with no `request_id` at all is left to the
                // 30-second stop-loss below.
                if let requestId { clearPendingCreate(requestID: requestId) }

            default:
                break
            }
        }
    #endif

    #if !canImport(engine_mobileFFI)
        func refreshManagedMcpInventory() async {}

        func startManagedMcpAuthoring(appID: String, userGoal: String) async -> Bool {
            managedMcpCommandErrors[appID] = "Local App MCP controls are unavailable until the engine bindings are installed."
            return false
        }

        func setManagedMcpEnabled(appID: String, enabled: Bool) async -> Bool {
            managedMcpCommandErrors[appID] = "Local App MCP controls are unavailable until the engine bindings are installed."
            return false
        }

        func setManagedMcpToolEnabled(
            appID: String,
            toolName: String,
            enabled: Bool
        ) async -> Bool {
            managedMcpCommandErrors[appID] = "Local App MCP controls are unavailable until the engine bindings are installed."
            return false
        }

        func setManagedMcpConversationPinned(
            conversationID: String,
            appID: String,
            pinned: Bool
        ) async -> Bool {
            managedMcpCommandErrors[appID] = "Local App MCP controls are unavailable until the engine bindings are installed."
            return false
        }
    #endif

    func app(id: String) -> LocalAppSummary? {
        apps.first { $0.id == id }
    }

    func clearError() {
        errorMessage = nil
    }

    func consumeCreatedAppID() -> String? {
        defer { createdAppID = nil }
        return createdAppID
    }

    func completeWidgetSetup() {
        pendingWidgetSetup = nil
    }

    func consumeRequestedPresentationAppID() -> String? {
        defer { requestedPresentationAppID = nil }
        return requestedPresentationAppID
    }

    func hasPendingUIRequest(appID: String) -> Bool {
        pendingUIRequestAppIDs.values.contains(appID)
    }

    func requestLaunch(appID: String, destination: LocalAppLaunchDestination) {
        pendingLaunchDestinations[appID] = destination
    }

    func consumeLaunchDestination(appID: String) -> LocalAppLaunchDestination? {
        defer { pendingLaunchDestinations.removeValue(forKey: appID) }
        return pendingLaunchDestinations[appID]
    }

    func presentUnavailableAppError() {
        errorMessage = "应用不存在或已删除"
    }

    #if DEBUG
        /// The environment variable that asks for [`seedForUITesting`].
        ///
        /// Opt-in per launch, like every other `LINGXI_UI_TEST_*` switch, and
        /// deliberately NOT folded into `LINGXI_UI_TESTING=1`: the apps tab's
        /// empty state is itself pinned by
        /// `testEverySidebarRoutePresentsAndLeavesTheSidebar`, which asserts
        /// `drawer.apps.view-all` is absent. Seeding unconditionally would
        /// turn that assertion red.
        static let uiTestSeedEnvironmentKey = "LINGXI_UI_TEST_LOCAL_APPS"

        /// The id of the app [`seedForUITesting`] plants. The drawer renders it
        /// as `drawer.apps.row.<id>`.
        static let uiTestSeedAppID = "ui-test-seeded-app"

        /// Plant one app in the catalog so the surfaces gated on a non-empty
        /// catalog are reachable from a UI test.
        ///
        /// Under `LINGXI_UI_TESTING=1`, `ConversationSource.make` returns
        /// `MockConversationSource`, whose `submitEngineCommand` is the no-op
        /// protocol-extension default. `apps` is written in exactly two
        /// places — `handle(event:)`'s `.appsChanged` arm and `upsertApp`,
        /// both engine-event handlers — so with no engine behind the mock,
        /// `.listApps` resolves into nothing and the catalog is permanently
        /// empty. Everything gated on it is then unreachable: `Drawer`'s
        /// `appsSection` (hence `drawer.apps.view-all`, the only surviving
        /// drawer route to `LocalAppsRootView`) and the library's own list.
        ///
        /// Assigns `apps` directly rather than replaying an `.appsChanged`
        /// event: this is a fixture, not an engine, and that arm also fires
        /// `scheduleWidgetSnapshotPublish` and `scheduleWebsiteDataCleanup`,
        /// which write to app-group storage a UI test has no business
        /// touching.
        ///
        /// `scaffolded: true` on purpose — a shell renders as the placeholder
        /// draft card, and this fixture exists to be an ordinary listed app.
        func seedForUITesting() {
            apps = [
                LocalAppSummary(
                    id: Self.uiTestSeedAppID,
                    name: "UI 测试应用",
                    brief: "UI 测试用的本地应用",
                    updatedAt: Date(timeIntervalSince1970: 1_700_000_000),
                    workflow: .publishedVerified,
                    workspaceRelativePath: "apps/\(Self.uiTestSeedAppID)/workspace"
                )
            ]
        }
    #endif

    func refresh() async {
        if let refreshTask {
            await refreshTask.value
            return
        }
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer { self.refreshTask = nil }
            await self.performRefresh()
        }
        refreshTask = task
        await task.value
    }

    private func performRefresh() async {
        #if canImport(engine_mobileFFI)
            guard let submitCommand else {
                errorMessage = String(localized: "local_apps_error_engine_not_connected")
                return
            }
            isRefreshing = true
            do {
                try await submitCommand(.listApps)
            } catch {
                isRefreshing = false
                errorMessage = error.localizedDescription
            }
        #else
            isRefreshing = false
        #endif
    }

    func refreshAfterEngineRebind() async {
        // The create claim does NOT survive a rebind. `AppCreated` is a
        // one-shot event on the source that was just torn down, so a create
        // still in flight across a project/scope switch can never resolve its
        // claim — and nothing else clears `pendingCreateRequestID`. Left armed
        // it is a permanent latch: every later create returns
        // `local_apps_error_create_in_progress` for the process lifetime.
        //
        // The app itself was probably still created, which is exactly why the
        // user is TOLD rather than left to assume it failed — and why the
        // client must not go on to claim some later `AppCreated`. Nothing that
        // arrives after this point carries a request id this store is still
        // waiting on, so nothing can be claimed.
        if let pending = pendingCreateRequestID {
            clearPendingCreate(requestID: pending)
            errorMessage = String(localized: "local_apps_creation_result_unknown")
        }
        await refresh()
        let appIDs = apps.map(\.id)
        // Rebind used to issue one bridge round-trip after another. Keep a
        // small batch so large libraries do not flood the FFI queue while the
        // visible app details still hydrate concurrently.
        for start in stride(from: 0, to: appIDs.count, by: 8) {
            let end = min(start + 8, appIDs.count)
            await withTaskGroup(of: Void.self) { group in
                for appID in appIDs[start..<end] {
                    group.addTask { [weak self] in
                        await self?.getDetails(appID: appID)
                    }
                }
            }
        }
    }

    /// The default stop-loss for one in-flight `CreateApp`.
    ///
    /// A NEW constant, deliberately not anchored to the 20-second identity
    /// proposal timeout this commit deletes: that one bounded a single model
    /// call. This one bounds a local create that also mints a session, and
    /// forks a source conversation's history into the new workspace when one
    /// is bound — which a cold device can take real time over. Thirty seconds
    /// is stop-loss, not an expectation: the app is almost certainly already
    /// in the library, which is what the user is told to check.
    static let defaultCreateResultTimeout: Duration = .seconds(30)

    /// The stop-loss this instance actually uses.
    ///
    /// Overridable so a test can exercise the REAL timer instead of asserting
    /// that a constant equals thirty. `createResultTimeoutIsThirtySeconds`
    /// pins the production value so shortening it here cannot go unnoticed.
    @ObservationIgnored var createResultTimeout: Duration = LocalAppsStore.defaultCreateResultTimeout

    /// Create the empty SHELL app that the conversational create flow
    /// interviews the user inside.
    ///
    /// There is no form: no brief, no name, no surface, no model picker. The
    /// record is committed with `mode: .shell`, which writes
    /// `scaffolded: false` and lays down NO scaffold; the agent proposes the
    /// name and the surface inside the app's own conversation and
    /// `LocalAppScaffold` lands the scaffolding only after the user confirms.
    ///
    /// Sends NO conversation binding, deliberately. The wire field exists, but
    /// a library-origin create provably discards it: `handle_create_app`
    /// (`host.rs`) derives the record's binding from
    /// `AppCreateOrigin::conversation_binding`, whose `Library` arm returns
    /// `None` for ANY input, and `mint_app_init_session` forks a source chat
    /// only when `record.conversation_id` is `Some`. So the "+" button's app
    /// always starts from an empty anchor session; forwarding the active
    /// conversation here would be a value the engine throws away. Android's
    /// `createShellApp` sends `conversationId = null` for the same reason.
    ///
    /// Returns whether the command reached the engine — NOT whether the app
    /// was created. The outcome arrives out of band on `AppCreated` /
    /// `AppOperationFailed`, correlated by `request_id`.
    ///
    /// `armLibraryFallback` says whether a `LocalAppsRootView` is mounted to
    /// consume [`createdAppID`]. The library's "+" is the only caller that can
    /// answer yes; the drawer's affordances create with the cover down and pass
    /// `false`. See [`pendingCreateArmsLibraryFallback`].
    ///
    /// Deliberately UNDEFAULTED, matching Android's
    /// `LocalAppsViewModel.createShellApp(armLibraryFallback:)`. The only
    /// value a default could plausibly carry is `true` — the library's — and
    /// `true` is the DANGEROUS one: it arms a one-shot landing whose sole
    /// consumer lives inside a cover that may never be mounted, and an id
    /// armed with nothing to drain it survives for the process lifetime and
    /// hijacks the user's next unrelated "View all". A caller that says
    /// nothing would inherit exactly that. So every caller states where it is
    /// creating from.
    func createShellApp(armLibraryFallback: Bool) async -> Bool {
        guard pendingCreateRequestID == nil else {
            errorMessage = String(localized: "local_apps_error_create_in_progress")
            return false
        }
        #if canImport(engine_mobileFFI)
            let requestID = UUID().uuidString
            // Armed BEFORE the command goes out: `send` awaits the engine, and
            // the engine can emit `AppCreated` from inside that call. Arming
            // afterwards drops the event this whole mechanism exists to catch.
            pendingCreateRequestID = requestID
            // Recorded with the correlation key, not read at event time from a
            // view: by the time `AppCreated` lands the cover may have been
            // opened or closed for unrelated reasons, and what decides this is
            // where the create STARTED.
            pendingCreateArmsLibraryFallback = armLibraryFallback
            armCreateResultTimeout(requestID: requestID)
            let succeeded = await send(
                .createApp(
                    // A shell has no name and no brief to send: both are what
                    // the conversation is FOR. The engine writes a placeholder
                    // name that no client is allowed to render (see
                    // `LocalAppSummary.displayName`).
                    name: "",
                    origin: .library,
                    brief: "",
                    gitEnabled: true,
                    workflowModel: nil,
                    // See the doc comment: a `.library` origin binds no
                    // conversation, whatever is sent here.
                    conversationId: nil,
                    // The shape is decided when the scaffold lands, not now.
                    surface: nil,
                    mode: .shell,
                    requestId: requestID
                )
            )
            if !succeeded { clearPendingCreate(requestID: requestID) }
            return succeeded
        #else
            errorMessage = String(localized: "local_apps_error_engine_unavailable")
            return false
        #endif
    }

    /// Arm the stop-loss for one pending create.
    private func armCreateResultTimeout(requestID: String) {
        createResultTimeoutTask?.cancel()
        let timeout = createResultTimeout
        createResultTimeoutTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: timeout)
            guard !Task.isCancelled else { return }
            self?.reportUnknownCreateResult(requestID: requestID)
        }
    }

    /// The stop-loss fired: the create never reported back. The app is very
    /// likely in the library anyway, so the user is pointed at it rather than
    /// told it failed.
    private func reportUnknownCreateResult(requestID: String) {
        guard pendingCreateRequestID == requestID else { return }
        clearPendingCreate(requestID: requestID)
        errorMessage = String(localized: "local_apps_creation_result_unknown")
    }

    /// Disarm the pending create — but ONLY if `requestID` is the one still in
    /// flight. Every caller passes a key that came off the wire, so this guard
    /// is what makes "ignore any event whose request id does not match" true
    /// rather than merely intended.
    private func clearPendingCreate(requestID: String) {
        guard pendingCreateRequestID == requestID else { return }
        pendingCreateRequestID = nil
        pendingCreateArmsLibraryFallback = false
        createResultTimeoutTask?.cancel()
        createResultTimeoutTask = nil
    }

    /// Arm the home-screen widget setup guide for one app.
    ///
    /// Reached from the app's detail screen. It used to be a toggle inside the
    /// create form; deleting that form without rehoming this entry would have
    /// removed the only way to put an app on the home screen.
    ///
    /// Refuses for a shell: an unscaffolded app is excluded from the widget
    /// snapshot on purpose (see `makeWidgetSnapshot`), so the widget would
    /// have nothing to show.
    func requestWidgetSetup(appID: String) {
        guard let app = app(id: appID), app.scaffolded else { return }
        pendingWidgetSetup = PendingWidgetSetup(
            appID: app.id,
            appName: app.displayName
        )
        publishWidgetSnapshotNow()
    }

    /// Take the post-creation landing, if any. One-shot.
    func consumeCreatedAppLanding() -> CreatedAppLanding? {
        defer { createdAppLanding = nil }
        return createdAppLanding
    }

    func getDetails(appID: String) async {
        #if canImport(engine_mobileFFI)
            if let detailsTask = detailsTasks[appID] {
                await detailsTask.value
                return
            }
            let task = Task { @MainActor [weak self] in
                guard let self else { return }
                defer { self.detailsTasks[appID] = nil }
                _ = await self.send(.getAppDetails(appId: appID))
            }
            detailsTasks[appID] = task
            await task.value
        #endif
    }

    /// Pages through the app's workspace-scoped session catalog. `offset`
    /// `nil`/0 replaces the cached page; a later offset appends to it. The
    /// reply arrives out-of-band as `AppSessionsChanged`.
    func listSessions(appID: String, offset: UInt64? = nil, limit: UInt32? = nil) async {
        #if canImport(engine_mobileFFI)
            let normalizedOffset = offset ?? 0
            if pendingSessionRequestOffsets[appID] == normalizedOffset { return }
            pendingSessionRequestOffsets[appID] = normalizedOffset
            let submitted = await send(.listAppSessions(appId: appID, offset: offset, limit: limit))
            if !submitted { pendingSessionRequestOffsets[appID] = nil }
        #endif
    }

    /// Requests the next catalog page, as reported by the last reply's
    /// `nextOffset`. A no-op on the last page.
    func loadMoreSessions(appID: String) async {
        guard let nextOffset = sessionPages[appID]?.nextOffset else { return }
        await listSessions(appID: appID, offset: nextOffset)
    }

    func start(appID: String) async {
        #if canImport(engine_mobileFFI)
            switch runtimes[appID] {
            case .running:
                return
            default:
                break
            }
            if let startTask = startTasks[appID] {
                await startTask.value
                return
            }
            let task = Task { @MainActor [weak self] in
                guard let self else { return }
                defer { startTasks[appID] = nil }
                if await send(.startApp(appId: appID)) {
                    runtimeLastUsedAt[appID] = .now
                }
            }
            startTasks[appID] = task
            await task.value
        #endif
    }

    func stop(appID: String) async {
        #if canImport(engine_mobileFFI)
            _ = await send(.stopApp(appId: appID))
        #endif
    }

    func restart(appID: String) async {
        #if canImport(engine_mobileFFI)
            _ = await send(.restartApp(appId: appID))
        #endif
    }

    func executeBridge(_ request: LocalAppBridgeRequest) async {
        #if canImport(engine_mobileFFI)
            runtimeLastUsedAt[request.appID] = .now
            let operation: AppBridgeOperationDto?
            switch (request.namespace, request.operation) {
            case ("data", "query"): operation = .queryData
            case ("data", "mutate"): operation = .mutateData
            case ("network", "fetch"): operation = .networkRequest
            case ("runtime", "info"): operation = .runtimeStatus
            case ("device", "capturePhoto"): operation = .capturePhoto
            case ("device", "pickImage"): operation = .pickImage
            case ("device", "recordAudioStart"): operation = .recordAudioStart
            case ("device", "recordAudioStop"): operation = .recordAudioStop
            case ("device", "getLocation"): operation = .getLocation
            case ("device", "transcribeSpeech"): operation = .transcribeSpeech
            case ("device", "postNotification"): operation = .postNotification
            case ("clipboard", "getText"): operation = .clipboardGetText
            case ("clipboard", "setText"): operation = .clipboardSetText
            case ("device", "share"): operation = .share
            case ("device", "synthesizeSpeech"): operation = .synthesizeSpeech
            case ("files", "read"): operation = .fileRead
            case ("files", "write"): operation = .fileWrite
            case ("device", "status"): operation = .deviceStatus
            case ("device", "haptics"): operation = .haptics
            case ("device", "deepLink"): operation = .deepLink
            case ("calendar", "listEvents"): operation = .calendarListEvents
            case ("contacts", "search"): operation = .contactsSearch
            case ("media", "get"): operation = .mediaGet
            case ("llm", "chat"): operation = .llmChat
            case ("llm", "stream"): operation = .llmStream
            case ("agent", "post"): operation = .agentPost
            case ("agent", "sessionCreate"): operation = .agentSessionCreate
            case ("agent", "sessionList"): operation = .agentSessionList
            case ("agent", "sessionResume"): operation = .agentSessionResume
            case ("agent", "sessionClose"): operation = .agentSessionClose
            case ("agent", "send"): operation = .agentSend
            case ("agent", "stream"): operation = .agentStream
            case ("agent", "cancel"): operation = .agentCancel
            case ("agent", "profileProposeUpdate"): operation = .agentProfileProposeUpdate
            case ("background", "schedule"): operation = .backgroundSchedule
            case ("background", "list"): operation = .backgroundList
            case ("background", "status"): operation = .backgroundStatus
            case ("background", "cancel"): operation = .backgroundCancel
            case ("background", "retry"): operation = .backgroundRetry
            default: operation = nil
            }
            guard let operation else {
                LocalAppWebViewRegistry.shared.resolveBridge(
                    appID: request.appID,
                    requestID: request.id,
                    resultJSON: nil,
                    error: String(localized: "local_apps_error_bridge_unsupported")
                )
                return
            }
            let reschedulesBackground = operation == .backgroundSchedule
                || operation == .backgroundCancel
                || operation == .backgroundRetry
            if reschedulesBackground {
                pendingBackgroundMutationRequests.insert(request.id)
            }
            let submitted = await send(
                .executeAppBridgeRequest(
                    request: AppBridgeRequestDto(
                        requestId: request.id,
                        appId: request.appID,
                        operation: operation,
                        payloadJson: request.payloadJSON
                    )
                )
            )
            if !submitted {
                pendingBackgroundMutationRequests.remove(request.id)
                LocalAppWebViewRegistry.shared.resolveBridge(
                    appID: request.appID,
                    requestID: request.id,
                    resultJSON: nil,
                    error: String(localized: "local_apps_error_bridge_rejected")
                )
            }
        #endif
    }

    func resolvePendingPermission(_ decision: LocalAppCapabilityDecision) async {
        guard let prompt = pendingPermission, let source = pendingPermissionSource else { return }
        pendingPermission = nil
        pendingPermissionSource = nil

        #if canImport(engine_mobileFFI)
            // However this one resolves — including the early return on a denied
            // UI request — the next queued request has to reach the sheet.
            defer { presentNextPermission() }
            let normalizedDecision: LocalAppCapabilityDecision
            if prompt.allowsPersistentGrant || decision == .deny || decision == .once {
                normalizedDecision = decision
            } else {
                normalizedDecision = .once
            }
            let authorization = LocalAppsProtocolAdapter.authorizationDecision(normalizedDecision)
            switch source {
            case let .ui(request):
                if normalizedDecision == .deny {
                    await resolveUIRequest(
                        requestID: request.requestId,
                        decision: authorization,
                        resultJSON: nil,
                        error: String(localized: "local_apps_error_ui_rejected")
                    )
                    return
                }
                if normalizedDecision == .session || normalizedDecision == .always {
                    approvedUIAutomation[request.appId] = normalizedDecision
                }
                let result = await executeUIRequestInPreview(request)
                await resolveUIRequest(
                    requestID: request.requestId,
                    decision: authorization,
                    resultJSON: result.resultJSON,
                    error: result.error
                )
            case let .capability(appID, kind):
                if kind == .uiControl, normalizedDecision != .deny {
                    approvedUIAutomation[appID] = normalizedDecision
                }
                _ = await send(
                    .resolveAppCapabilityRequest(
                        requestId: prompt.id,
                        decision: authorization
                    )
                )
            }
        #endif
    }

    func resolvePendingDependencyChangeConfirmation(_ approved: Bool) async {
        guard let prompt = pendingDependencyChangeConfirmation else { return }
        pendingDependencyChangeConfirmation = nil
        #if canImport(engine_mobileFFI)
            defer { presentNextDependencyChangeConfirmation() }
            _ = await send(
                .resolveAppDependencyChangeConfirmation(
                    requestId: prompt.id,
                    approved: approved
                )
            )
        #endif
    }

    func resetPermissions(appID: String) async -> Bool {
        #if canImport(engine_mobileFFI)
            approvedUIAutomation[appID] = nil
            return await send(.resetAppPermissions(appId: appID))
        #else
            return false
        #endif
    }

    func listCheckpoints(appID: String) async {
        #if canImport(engine_mobileFFI)
            _ = await send(.listAppCheckpoints(appId: appID))
        #endif
    }

    func restore(appID: String, checkpointID: String) async -> Bool {
        #if canImport(engine_mobileFFI)
            return await send(.restoreAppCheckpoint(appId: appID, checkpointId: checkpointID))
        #else
            return false
        #endif
    }

    func delete(appID: String) async -> Bool {
        #if canImport(engine_mobileFFI)
            // Journal first: if the process dies after Rust removes the app but
            // before WebKit finishes, the next authoritative apps snapshot will
            // retry the exact identified data-store removal.
            websiteDataStoreRegistry.prepareForDeletion(appID: appID)
            let submitted = await send(.deleteApp(appId: appID))
            guard submitted else {
                websiteDataStoreRegistry.cancelDeletion(appID: appID)
                return false
            }
            LocalAppWebViewRegistry.shared.close(appID: appID)
            return true
        #else
            return false
        #endif
    }

    func sceneDidEnterBackground() {
        runningBeforeSuspension = Set(runtimes.compactMap { appID, state in
            switch state {
            case .running, .starting: appID
            default: nil
            }
        })
        UserDefaults.standard.set(Array(runningBeforeSuspension), forKey: "local-apps.running-before-suspension")
    }

    func sceneWillEnterForeground() async {
        let saved = UserDefaults.standard.stringArray(forKey: "local-apps.running-before-suspension") ?? []
        // Consumed exactly once: scenePhase reaches .active on every .inactive bounce,
        // not only after a real background, and a stopped app must stay stopped.
        let restoring = runningBeforeSuspension.union(saved)
        runningBeforeSuspension.removeAll()
        UserDefaults.standard.removeObject(forKey: "local-apps.running-before-suspension")
        await refresh()
        for appID in restoring {
            await getDetails(appID: appID)
            await start(appID: appID)
        }
    }

    func handleMemoryWarning() async {
        let running = apps
            .filter { app in
                if case .running = runtimes[app.id] { return true }
                return false
            }
            .sorted {
                (runtimeLastUsedAt[$0.id] ?? $0.updatedAt)
                    < (runtimeLastUsedAt[$1.id] ?? $1.updatedAt)
            }
        guard let leastRecentlyUpdated = running.first else { return }
        await stop(appID: leastRecentlyUpdated.id)
    }

    private func updateApp(appID: String, mutation: (inout LocalAppSummary) -> Void) {
        guard let index = apps.firstIndex(where: { $0.id == appID }) else { return }
        mutation(&apps[index])
    }

    #if canImport(engine_mobileFFI)
        private func hydratedSummary(for record: AppRecordDto) -> LocalAppSummary {
            var summary = LocalAppsProtocolAdapter.app(record)
            summary.runtimeProfileStatus = runtimeProfileStatuses[record.id]
            if let existing = apps.first(where: { $0.id == record.id }) {
                summary.uiVerification = existing.uiVerification
                summary.mcpVerification = existing.mcpVerification
            } else if let inventory = managedMcpInventories.values.first(where: { $0.appID == record.id }) {
                summary.uiVerification = inventory.uiVerification
                summary.mcpVerification = inventory.mcpVerification
            }
            return summary
        }
    #endif

    /// Serializes WebKit cleanup and retries with the newest authoritative app
    /// set if another snapshot arrives while an async removal is in progress.
    private func scheduleWebsiteDataCleanup(activeAppIDs: Set<String>) {
        guard websiteDataCleanupTask == nil else { return }
        websiteDataCleanupTask = Task { @MainActor [weak self] in
            guard let self else { return }
            await websiteDataStoreRegistry.removeDataForDeletedApps(activeAppIDs: activeAppIDs)
            websiteDataCleanupTask = nil

            let latestAppIDs = Set(apps.map(\.id))
            if latestAppIDs != activeAppIDs {
                scheduleWebsiteDataCleanup(activeAppIDs: latestAppIDs)
            }
        }
    }

    #if canImport(engine_mobileFFI)
        private func handleAppEvent(_ event: AppEventDto) {
            switch event {
            case let .appCreated(record, requestId):
                // The engine names the record it just committed, for BOTH
                // creation paths. Emitted after `AppsChanged`, so the catalog
                // already contains it.
                //
                // Deliberately does NOT switch conversation scope here. An
                // AGENT-driven create happens mid-turn, and
                // `RootView.switchScope` submits `cancelAndWait()` first — the
                // landing would kill the very turn that produced the app. The
                // library arms the landing and `RootView` walks in once the
                // turn is done (immediately, for a "+" create, where there is
                // no turn at all).
                //
                // Claim ONLY the create this client started, by the correlation
                // key it generated. Every other `AppCreated` — an agent's
                // create in another conversation, a create this store already
                // resolved, anything arriving after a reconnect cleared the
                // pending id — falls through to `break` and is ignored.
                let summary = hydratedSummary(for: record)
                upsertApp(summary)
                guard let pending = pendingCreateRequestID,
                      let requestId,
                      requestId == pending
                else { break }
                // Read before `clearPendingCreate`, which restores the
                // default.
                let armsLibraryFallback = pendingCreateArmsLibraryFallback
                clearPendingCreate(requestID: pending)
                // The library's fallback landing, armed only when the library
                // is there to consume it.
                if armsLibraryFallback { createdAppID = summary.id }
                // The hand-off target: the app's own conversation, whose cwd is
                // the app workspace. Everything the agent does for this app has
                // to run THERE.
                //
                // HELD, not published: the pin is minted AFTER this event and
                // arrives on `AppRecordChanged`, which the engine emits either
                // way — with the id it minted, or without one when the
                // best-effort mint failed. Publishing here handed `RootView` a
                // landing with `initSessionID == nil`, which it consumed on the
                // very next runloop turn and answered by starting a FRESH
                // conversation, orphaning the session the engine had just
                // pinned. Waiting for the record (NOT for a non-nil pin, which
                // is what made an earlier gate unreachable) is what Android
                // already does via `landingAwaitingPin`.
                landingAwaitingPin = CreatedAppLanding(
                    appID: summary.id,
                    initSessionID: record.initSessionId
                )

            case let .appDetailsChanged(details):
                let status = details.runtimeProfileStatus.map(LocalAppsProtocolAdapter.runtimeProfileStatus)
                runtimeProfileStatuses[details.app.id] = status
                var summary = hydratedSummary(for: details.app)
                summary.runtimeProfileStatus = status
                upsertApp(summary)
                collections[summary.id] = details.manifest.map {
                    $0.collections.map(LocalAppsProtocolAdapter.collection)
                } ?? []
                runtimes[summary.id] = LocalAppsProtocolAdapter.runtime(
                    details.runtime.state,
                    details: details.runtime,
                    lastError: details.runtime.lastError,
                    knownURL: runtimes[summary.id]?.url
                )
                replaceCheckpoints(details.checkpoints, appID: summary.id)
                scheduleWidgetSnapshotPublish()

            case let .appRecordChanged(record):
                let summary = hydratedSummary(for: record)
                upsertApp(summary)
                // The create handshake's second half: the pin the engine minted
                // right after `AppCreated`. Publishing the landing HERE — with
                // whatever the record carries, pin or no pin — is what keeps
                // `RootView` from resuming a session id that does not exist yet.
                if let armed = landingAwaitingPin, armed.appID == summary.id {
                    landingAwaitingPin = nil
                    var landing = armed
                    landing.initSessionID = summary.initSessionId
                    createdAppLanding = landing
                }
                lastRefreshAt = .now
                scheduleWidgetSnapshotPublish()

            case let .appProfileProposal(proposal):
                pendingProfileProposal = LocalAppProfileProposal(
                    appID: proposal.appId,
                    approvalToken: proposal.approvalToken,
                    baseRevision: proposal.baseRevision,
                    currentRevision: proposal.currentRevision,
                    instructions: proposal.instructions,
                    reason: proposal.reason
                )

            case let .appBridgeResponse(response):
                let rescheduleBackground = pendingBackgroundMutationRequests.remove(response.requestId) != nil
                LocalAppWebViewRegistry.shared.resolveBridge(
                    appID: response.appId,
                    requestID: response.requestId,
                    resultJSON: response.resultJson,
                    error: response.ok ? nil : (response.error ?? String(localized: "local_apps_error_bridge_failed")),
                    code: response.errorCode
                )
                if rescheduleBackground {
                    LocalAppBackgroundTaskBridge.shared.rescheduleAfterForegroundMutation()
                }

            // Stream frames are consumed by the app bridge/session stream
            // owner; the library store must remain exhaustive without
            // misclassifying an ordered frame as a one-shot response.
            case let .appBridgeStreamFrame(_, frameJSON):
                guard let data = frameJSON.data(using: .utf8),
                      let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let appID = object["appId"] as? String
                else { break }
                LocalAppWebViewRegistry.shared.deliverStreamFrame(appID: appID, frameJSON: frameJSON)

            case let .appUiRequest(request):
                // Remember what these pointed at: an overflow drop must restore
                // them rather than clear them, so dropping app B's request does
                // not erase app A's genuinely pending one. The LRU stamp is
                // captured too — marking a dropped app most-recently-used would
                // protect it from eviction while a genuinely active runtime is
                // reclaimed instead.
                let previousRuntimeLastUsedAt = runtimeLastUsedAt[request.appId]
                let previousRequestedPresentationAppID = requestedPresentationAppID
                runtimeLastUsedAt[request.appId] = .now
                pendingUIRequestAppIDs[request.requestId] = request.appId
                requestedPresentationAppID = request.appId
                // `.captureView` rides with `.inspect` because it is read-only in
                // the same sense — the engine does not gate it on `ui_control`
                // at all (`capture_ui` never calls `authorize_capability`), and
                // the agent already had to clear the `LocalAppCaptureUi` prompt,
                // which is DenyByDefault. Falling through to the automation
                // prompt was worse than redundant: a `.session`/`.always` answer
                // is stored in `approvedUIAutomation`, so approving a screenshot
                // silently authorized click/fill/navigate for the whole session.
                if request.action == .inspect || request.action == .captureView {
                    executeUIRequest(request, decision: .allowOnce)
                } else if let decision = approvedUIAutomation[request.appId] {
                    if decision == .once { approvedUIAutomation[request.appId] = nil }
                    executeUIRequest(
                        request,
                        decision: LocalAppsProtocolAdapter.authorizationDecision(decision)
                    )
                } else {
                    let queued = enqueuePermission(
                        LocalAppPermissionPrompt(
                            id: request.requestId,
                            appID: request.appId,
                            kind: .uiAction(LocalAppsProtocolAdapter.uiActionLabel(request.action)),
                            reason: uiRequestReason(request),
                            domain: nil
                        ),
                        source: .ui(request)
                    )
                    if !queued {
                        // A dropped request never reaches `resolveUIRequest`,
                        // so its entry would linger forever and force-route
                        // every later open of this app to the preview tab,
                        // which then renders a bare "preview isn't ready yet".
                        pendingUIRequestAppIDs.removeValue(forKey: request.requestId)
                        requestedPresentationAppID = previousRequestedPresentationAppID
                        runtimeLastUsedAt[request.appId] = previousRuntimeLastUsedAt
                    }
                }

            case let .appCapabilityRequested(request):
                enqueuePermission(
                    LocalAppPermissionPrompt(
                        id: request.requestId,
                        appID: request.appId,
                        kind: LocalAppsProtocolAdapter.capabilityKind(request.capability),
                        reason: request.reason,
                        domain: request.domain
                    ),
                    source: .capability(appID: request.appId, kind: request.capability)
                )

            case let .appDependencyChangeConfirmationRequested(request):
                enqueueDependencyChangeConfirmation(
                    LocalAppsProtocolAdapter.dependencyChangeConfirmation(request)
                )

            case let .pluginStatusChanged(status):
                guard status.pluginId == builtinPluginDescriptor.pluginID else { break }
                pendingBuiltinPluginEnabled = nil
                builtinPluginStatus = LocalAppBuiltinPluginStatus(
                    state: status.state,
                    manifestDefaultEnabled: status.manifestDefaultEnabled,
                    validationError: builtinPluginInventory?.validationError ?? builtinPluginCommandError
                )
                builtinPluginCommandError = nil

            case let .pluginInventoryChanged(inventory):
                guard inventory.pluginId == builtinPluginDescriptor.pluginID else { break }
                builtinPluginInventory = LocalAppsProtocolAdapter.builtinPluginInventory(inventory)
                builtinPluginStatus = LocalAppBuiltinPluginStatus(
                    state: inventory.state,
                    manifestDefaultEnabled: inventory.manifestDefaultEnabled,
                    validationError: inventory.validationError ?? builtinPluginCommandError
                )

            case let .createConfirmationRequested(request):
                enqueueCreateConfirmation(
                    LocalAppsProtocolAdapter.createConfirmation(request)
                )

            case let .mcpProposalApprovalRequested(request):
                let prompt = LocalAppsProtocolAdapter.mcpProposalApproval(request)
                guard prompt.hasVisibleChanges else {
                    Task { await self.resolve(promptID: prompt.requestID, as: .mcpProposal, approved: false) }
                    break
                }
                enqueueMcpProposalApproval(prompt)

            case let .managedMcpInventoryChanged(servers):
                managedMcpInventories = Dictionary(
                    uniqueKeysWithValues: servers.map {
                        let inventory = LocalAppsProtocolAdapter.managedMcpInventory($0)
                        return (inventory.serverName, inventory)
                    }
                )
                let liveManagedAppIDs = Set(managedMcpInventories.values.map(\.appID))
                managedMcpCommandErrors = managedMcpCommandErrors.filter { liveManagedAppIDs.contains($0.key) }
                pendingManagedMcpAppIDs.subtract(liveManagedAppIDs)

            case let .verificationSummaryChanged(appId, publicationState, mcpVerification, uiVerification):
                updateApp(appID: appId) { app in
                    app.workflow = LocalAppsProtocolAdapter.workflow(publicationState)
                    app.mcpVerification = LocalAppsProtocolAdapter.verificationSummary(mcpVerification)
                    app.uiVerification = LocalAppsProtocolAdapter.verificationSummary(uiVerification)
                    app.updatedAt = .now
                }

            case let .localAppOperationFailed(appId, _, message, requestId):
                if let requestId {
                    discardPendingApproval(requestID: requestId)
                }
                if let appId {
                    updateManagedInventoryFailure(appID: appId, message: message)
                    managedMcpCommandErrors[appId] = message
                    pendingManagedMcpAppIDs.remove(appId)
                }
                errorMessage = message

            case let .appCheckpointsChanged(appId, checkpoints):
                replaceCheckpoints(checkpoints, appID: appId)

            case let .appLlmActivityChanged(appId, active):
                if active {
                    llmActiveAppIDs.insert(appId)
                } else {
                    llmActiveAppIDs.remove(appId)
                }

            case let .appAgentEventPosted(appId, _, _, _):
                unreadAgentEvents[appId, default: 0] += 1

            case .appBackgroundTaskChanged:
                // The durable lifecycle/result record is fetched on demand by
                // the local app bridge/MCP status APIs.
                // This event also covers schedules created by conversation MCP,
                // which do not pass through the WebView mutation bookkeeping.
                LocalAppBackgroundTaskBridge.shared.rescheduleAfterForegroundMutation()
            }
        }

        /// Strictly FIFO: the request the page issued first is answered first,
        /// because that is the one whose `fetch()` has been stalled longest.
        /// Returns `false` when the prompt was DROPPED on overflow. Callers that
        /// stamped per-request state before enqueuing must undo it on a drop —
        /// a dropped request never reaches `resolveUIRequest`, so anything left
        /// behind is never cleaned up.
        @discardableResult
        private func enqueuePermission(
            _ prompt: LocalAppPermissionPrompt,
            source: PendingPermissionSource
        ) -> Bool {
            guard pendingPermission != nil else {
                pendingPermission = prompt
                pendingPermissionSource = source
                return true
            }
            guard permissionQueue.count < Self.maxQueuedPermissions else {
                // Dropping the newest keeps every request that already has a
                // page waiting on it; the dropped one fails closed on the
                // engine's approval timeout.
                errorMessage = String(localized: "local_apps_error_permission_overflow")
                return false
            }
            permissionQueue.append((prompt: prompt, source: source))
            return true
        }

        private func presentNextPermission() {
            guard pendingPermission == nil, !permissionQueue.isEmpty else { return }
            let next = permissionQueue.removeFirst()
            pendingPermission = next.prompt
            pendingPermissionSource = next.source
        }

        private func enqueueDependencyChangeConfirmation(
            _ prompt: LocalAppDependencyChangeConfirmationPrompt
        ) {
            guard pendingDependencyChangeConfirmation != nil else {
                pendingDependencyChangeConfirmation = prompt
                return
            }
            dependencyChangeConfirmationQueue.append(prompt)
        }

        private func presentNextDependencyChangeConfirmation() {
            guard pendingDependencyChangeConfirmation == nil,
                  !dependencyChangeConfirmationQueue.isEmpty
            else { return }
            pendingDependencyChangeConfirmation = dependencyChangeConfirmationQueue.removeFirst()
        }

        private func enqueueCreateConfirmation(
            _ prompt: LocalAppCreateConfirmationPrompt
        ) {
            if pendingCreateConfirmation?.requestID == prompt.requestID
                || createConfirmationQueue.contains(where: { $0.requestID == prompt.requestID })
            {
                return
            }
            if let pendingCreateConfirmation, pendingCreateConfirmation.appID == prompt.appID {
                self.pendingCreateConfirmation = prompt
                Task { await self.resolve(promptID: pendingCreateConfirmation.requestID, as: .createConfirmation, approved: false) }
                return
            }
            if let index = createConfirmationQueue.firstIndex(where: { $0.appID == prompt.appID }) {
                let superseded = createConfirmationQueue[index]
                createConfirmationQueue[index] = prompt
                Task { await self.resolve(promptID: superseded.requestID, as: .createConfirmation, approved: false) }
                return
            }
            guard pendingCreateConfirmation != nil else {
                pendingCreateConfirmation = prompt
                return
            }
            createConfirmationQueue.append(prompt)
        }

        private func presentNextCreateConfirmation() {
            guard pendingCreateConfirmation == nil, !createConfirmationQueue.isEmpty else { return }
            pendingCreateConfirmation = createConfirmationQueue.removeFirst()
        }

        private func enqueueMcpProposalApproval(
            _ prompt: LocalAppMcpProposalApprovalPrompt
        ) {
            if pendingMcpProposalApproval?.requestID == prompt.requestID
                || mcpProposalApprovalQueue.contains(where: { $0.requestID == prompt.requestID })
            {
                return
            }
            if let pendingMcpProposalApproval, pendingMcpProposalApproval.appID == prompt.appID {
                self.pendingMcpProposalApproval = prompt
                Task { await self.resolve(promptID: pendingMcpProposalApproval.requestID, as: .mcpProposal, approved: false) }
                return
            }
            if let index = mcpProposalApprovalQueue.firstIndex(where: { $0.appID == prompt.appID }) {
                let superseded = mcpProposalApprovalQueue[index]
                mcpProposalApprovalQueue[index] = prompt
                Task { await self.resolve(promptID: superseded.requestID, as: .mcpProposal, approved: false) }
                return
            }
            guard pendingMcpProposalApproval != nil else {
                pendingMcpProposalApproval = prompt
                return
            }
            mcpProposalApprovalQueue.append(prompt)
        }

        private func presentNextMcpProposalApproval() {
            guard pendingMcpProposalApproval == nil, !mcpProposalApprovalQueue.isEmpty else { return }
            pendingMcpProposalApproval = mcpProposalApprovalQueue.removeFirst()
        }

        private func discardPendingApproval(requestID: String) {
            if pendingCreateConfirmation?.requestID == requestID {
                pendingCreateConfirmation = nil
                presentNextCreateConfirmation()
                return
            }
            if let index = createConfirmationQueue.firstIndex(where: { $0.requestID == requestID }) {
                createConfirmationQueue.remove(at: index)
                return
            }
            if pendingMcpProposalApproval?.requestID == requestID {
                pendingMcpProposalApproval = nil
                presentNextMcpProposalApproval()
                return
            }
            if let index = mcpProposalApprovalQueue.firstIndex(where: { $0.requestID == requestID }) {
                mcpProposalApprovalQueue.remove(at: index)
            }
        }

        private func resolve(
            promptID: String,
            as kind: PendingApprovalKind,
            approved: Bool
        ) async {
            let command: PluginCommandDto
            switch kind {
            case .createConfirmation:
                command = .resolveCreateConfirmation(requestId: promptID, approved: approved)
            case .mcpProposal:
                command = .resolveMcpProposalApproval(requestId: promptID, approved: approved)
            }
            _ = await send(.pluginCommand(command: command))
        }

        private func updateManagedInventoryFailure(appID: String, message: String) {
            for (serverName, inventory) in managedMcpInventories where inventory.appID == appID {
                managedMcpInventories[serverName] = LocalAppManagedMcpInventory(
                    serverName: inventory.serverName,
                    appID: inventory.appID,
                    appName: inventory.appName,
                    buildID: inventory.buildID,
                    catalogDigest: inventory.catalogDigest,
                    toolSurfaceDigest: inventory.toolSurfaceDigest,
                    authoringRevision: inventory.authoringRevision,
                    enabled: inventory.enabled,
                    status: .error,
                    settingsRevision: inventory.settingsRevision,
                    pinnedToCurrentConversation: inventory.pinnedToCurrentConversation,
                    publicationState: inventory.publicationState,
                    mcpVerification: LocalAppVerificationSummary(
                        status: .failed,
                        summary: message,
                        code: inventory.mcpVerification.code
                    ),
                    uiVerification: inventory.uiVerification,
                    enabledTools: inventory.enabledTools,
                    widget: inventory.widget,
                    tools: inventory.tools
                )
            }
        }

        private func submitManagedMcpCommand(
            appID: String,
            optimistic: LocalAppManagedMcpInventory?,
            command: LocalAppManagedMcpCommand
        ) async -> Bool {
            managedMcpCommandErrors.removeValue(forKey: appID)
            pendingManagedMcpAppIDs.insert(appID)
            let previousInventory = managedMcpInventories.values.first(where: { $0.appID == appID })
            if let optimistic {
                managedMcpInventories[optimistic.serverName] = optimistic
            }
            guard let submitManagedMcpCommand else {
                pendingManagedMcpAppIDs.remove(appID)
                if let previousInventory {
                    managedMcpInventories[previousInventory.serverName] = previousInventory
                }
                managedMcpCommandErrors[appID] = "Local App MCP controls need the newer engine protocol to send managed MCP commands."
                return false
            }
            let submitted = await submitManagedMcpCommand(command)
            if !submitted {
                pendingManagedMcpAppIDs.remove(appID)
                if let previousInventory {
                    managedMcpInventories[previousInventory.serverName] = previousInventory
                }
                managedMcpCommandErrors[appID] = "The host rejected the Local App MCP command."
                return false
            }
            return true
        }

        private func resolveUIRequest(
            requestID: String,
            decision: AppAuthorizationDecisionDto,
            resultJSON: String?,
            error: String?
        ) async {
            _ = await send(
                .resolveAppUiRequest(
                    requestId: requestID,
                    decision: decision,
                    resultJson: resultJSON,
                    error: error
                )
            )
            pendingUIRequestAppIDs.removeValue(forKey: requestID)
        }

        private func executeUIRequest(
            _ request: AppUiRequestDto,
            decision: AppAuthorizationDecisionDto
        ) {
            Task { [weak self] in
                guard let self else { return }
                let result = await self.executeUIRequestInPreview(request)
                await self.resolveUIRequest(
                    requestID: request.requestId,
                    decision: decision,
                    resultJSON: result.resultJSON,
                    error: result.error
                )
            }
        }

        /// Runtime events and UI requests travel through separate async engine
        /// paths, so an inspect can reach iOS before the `running` event that
        /// carries its loopback URL. Refreshing the authoritative details first
        /// lets the preview route mount its WebView while the registry waits for
        /// it, instead of stranding the request on the not-ready placeholder.
        private func executeUIRequestInPreview(
            _ request: AppUiRequestDto
        ) async -> LocalAppUIExecutionResult {
            await getDetails(appID: request.appId)
            return await LocalAppWebViewRegistry.shared.execute(request: request)
        }

        private func replaceCheckpoints(_ values: [AppCheckpointDto], appID: String) {
            checkpoints[appID] = values
                .map(LocalAppsProtocolAdapter.checkpoint)
                .sorted { $0.createdAt > $1.createdAt }
        }

        private func uiRequestReason(_ request: AppUiRequestDto) -> String {
            let target = [request.target?.elementId, request.target?.role, request.target?.name]
                .compactMap { $0 }
                .first
            if let target { return String(localized: "local_apps_ui_reason_target \(target)") }
            return String(localized: "local_apps_ui_reason")
        }
    #endif

    private func upsertApp(_ value: LocalAppSummary) {
        if let index = apps.firstIndex(where: { $0.id == value.id }) {
            apps[index] = value
        } else {
            apps.append(value)
        }
        apps.sort { $0.updatedAt > $1.updatedAt }
    }

    /// The home-screen snapshot.
    ///
    /// Shells are EXCLUDED, not relabelled. A `scaffolded == false` app has an
    /// engine placeholder for a name and nothing to launch, so relabelling it
    /// would put an un-openable icon on the user's home screen — outside the
    /// app, where none of the draft-card branches in this client can reach it.
    /// It reappears on its own once the scaffold lands and the record change
    /// republishes the snapshot.
    private func makeWidgetSnapshot() -> LocalAppWidgetSnapshot {
        LocalAppWidgetSnapshot(
            version: LocalAppWidgetSnapshot.currentVersion,
            apps: apps.filter(\.scaffolded).map { app in
                LocalAppWidgetSnapshot.App(
                    id: app.id,
                    name: app.name,
                    brief: app.brief,
                    workflow: app.workflow.rawValue,
                    runtimeState: widgetRuntimeState(for: runtimes[app.id] ?? .stopped),
                    updatedAtMs: Int64(app.updatedAt.timeIntervalSince1970 * 1000)
                )
            }
        )
    }

    private func publishWidgetSnapshotNow() {
        widgetSnapshotTask?.cancel()
        reportWidgetSnapshotError(widgetSnapshotPublisher(makeWidgetSnapshot()))
    }

    private func scheduleWidgetSnapshotPublish() {
        widgetSnapshotTask?.cancel()
        let snapshot = makeWidgetSnapshot()
        widgetSnapshotTask = Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            // Catalog-driven publishes are best-effort. A missing App Group
            // (unsigned debug build, or a host without the entitlement) must
            // not look like create/refresh failed.
            _ = widgetSnapshotPublisher(snapshot)
            widgetSnapshotTask = nil
        }
    }

    private func reportWidgetSnapshotError(_ error: Error?) {
        guard let error else { return }
        if error as? LocalAppWidgetSnapshotStore.SnapshotError == .containerUnavailable {
            return
        }
        errorMessage = String(localized: "local_apps_error_widget_snapshot")
    }

    private func widgetRuntimeState(for runtime: LocalAppRuntimeStatus) -> String {
        switch runtime {
        case .stopped:
            return "stopped"
        case .starting:
            return "starting"
        case .running:
            return "running"
        case .suspended:
            return "suspended"
        case .stopping:
            return "stopping"
        case .failed:
            return "failed"
        }
    }

    #if canImport(engine_mobileFFI)
        private func send(_ command: ClientCommand) async -> Bool {
            guard let submitCommand else {
                errorMessage = String(localized: "local_apps_error_engine_not_connected")
                return false
            }
            do {
                try await submitCommand(command)
                return true
            } catch {
                errorMessage = error.localizedDescription
                return false
            }
        }
    #endif
}

#if canImport(BackgroundTasks)
    import BackgroundTasks
#endif

let localAppBackgroundTaskIdentifier = "com.lingxi.code.localapps.background"

/// Process-global bridge between Apple's wake-up callback and the Host-owned
/// local-app background executor. Kept in an existing Xcode source so the
/// store/full targets share the same explicit project membership.
final class LocalAppBackgroundTaskBridge: @unchecked Sendable {
    static let shared = LocalAppBackgroundTaskBridge()

    typealias Handler = @Sendable () async -> Void

    private let lock = NSLock()
    private var registered = false
    private var handler: Handler?
    private var rescheduler: Handler?
    private var waiters: [UUID: CheckedContinuation<Handler?, Never>] = [:]

    func registerAtLaunch() {
        lock.lock()
        let shouldRegister = !registered
        registered = true
        lock.unlock()
        guard shouldRegister else { return }
        #if canImport(BackgroundTasks)
            BGTaskScheduler.shared.register(
                forTaskWithIdentifier: localAppBackgroundTaskIdentifier,
                using: nil
            ) { [weak self] task in
                guard let processing = task as? BGProcessingTask else {
                    task.setTaskCompleted(success: false)
                    return
                }
                let worker = Task {
                    await self?.runHandler()
                    processing.setTaskCompleted(success: !Task.isCancelled)
                }
                processing.expirationHandler = {
                    worker.cancel()
                    self?.schedule(
                        earliestAtMs: UInt64(Date().timeIntervalSince1970 * 1000) + 15 * 60 * 1_000
                    )
                }
            }
        #endif
        schedule(earliestAtMs: UInt64(Date().timeIntervalSince1970 * 1000) + 15 * 60 * 1_000)
    }

    func bind(_ handler: @escaping Handler, rescheduler: Handler? = nil) {
        lock.lock()
        self.handler = handler
        self.rescheduler = rescheduler
        let continuations = Array(waiters.values)
        waiters.removeAll()
        lock.unlock()
        continuations.forEach { $0.resume(returning: handler) }
    }

    func rescheduleAfterForegroundMutation() {
        lock.lock()
        let rescheduler = self.rescheduler
        lock.unlock()
        guard let rescheduler else { return }
        Task { await rescheduler() }
    }

    func schedule(earliestAtMs: UInt64?) {
        #if canImport(BackgroundTasks)
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: localAppBackgroundTaskIdentifier)
            guard let earliestAtMs else { return }
            let request = BGProcessingTaskRequest(identifier: localAppBackgroundTaskIdentifier)
            request.requiresNetworkConnectivity = false
            request.requiresExternalPower = false
            request.earliestBeginDate = Date(timeIntervalSince1970: TimeInterval(earliestAtMs) / 1000)
            try? BGTaskScheduler.shared.submit(request)
        #else
            _ = earliestAtMs
        #endif
    }

    private func runHandler() async {
        guard let handler = await resolveHandler() else { return }
        await handler()
    }

    private func resolveHandler() async -> Handler? {
        if let handler = currentHandler() {
            return handler
        }
        let id = UUID()
        return await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                if let handler = currentHandler() {
                    continuation.resume(returning: handler)
                } else if Task.isCancelled {
                    continuation.resume(returning: nil)
                } else {
                    storeWaiter(continuation, id: id)
                }
            }
        } onCancel: {
            let continuation = removeWaiter(id: id)
            continuation?.resume(returning: nil)
        }
    }

    private func currentHandler() -> Handler? {
        lock.lock()
        defer { lock.unlock() }
        return handler
    }

    private func storeWaiter(_ continuation: CheckedContinuation<Handler?, Never>, id: UUID) {
        lock.lock()
        defer { lock.unlock() }
        waiters[id] = continuation
    }

    private func removeWaiter(id: UUID) -> CheckedContinuation<Handler?, Never>? {
        lock.lock()
        defer { lock.unlock() }
        return waiters.removeValue(forKey: id)
    }
}

struct LocalAppManagedMcpInventoryReader {
    let appSandboxRoot: String

    func read(serverName: String, apps: [LocalAppSummary]) -> LocalAppManagedMcpInventory? {
        guard let appID = managedLocalAppID(serverName: serverName),
              let app = apps.first(where: { $0.id == appID })
        else { return nil }
        let manifestURL = URL(fileURLWithPath: appSandboxRoot, isDirectory: true)
            .appendingPathComponent("apps/\(appID)/workspace/.lingxi/manifest.json")
        guard let manifestObject = jsonObject(at: manifestURL),
              let activeCatalog = manifestObject["activeMcpCatalog"] as? [String: Any],
              let buildID = activeCatalog["buildId"] as? String,
              let catalogDigest = activeCatalog["catalogSha256"] as? String,
              let toolSurfaceDigest = activeCatalog["toolSurfaceSha256"] as? String,
              let verificationDigest = activeCatalog["mcpVerificationSha256"] as? String,
              let authoringRevision = activeCatalog["authoringRevision"] as? NSNumber
        else { return nil }

        let catalogURL = URL(fileURLWithPath: appSandboxRoot, isDirectory: true)
            .appendingPathComponent("apps/\(appID)/mcp/catalogs/\(catalogDigest).json")
        guard let catalogObject = jsonObject(at: catalogURL),
              let toolObjects = catalogObject["tools"] as? [[String: Any]]
        else { return nil }
        let parsedTools = toolObjects.compactMap(tool)
        let settingsURL = URL(fileURLWithPath: appSandboxRoot, isDirectory: true)
            .appendingPathComponent("apps/\(appID)/mcp/settings.json")
        let settings = jsonObject(at: settingsURL) ?? [:]
        let enabled = settings["enabled"] as? Bool ?? false
        let settingsRevision = (settings["revision"] as? NSNumber)?.uint64Value ?? 0
        let enabledTools = Set(settings["enabledTools"] as? [String] ?? [])

        return LocalAppManagedMcpInventory(
            serverName: serverName,
            appID: appID,
            appName: app.displayName,
            buildID: buildID,
            catalogDigest: catalogDigest,
            toolSurfaceDigest: toolSurfaceDigest,
            authoringRevision: authoringRevision.uint64Value,
            enabled: enabled && !enabledTools.isEmpty,
            status: enabled && !enabledTools.isEmpty ? .enabled : .disabled,
            settingsRevision: settingsRevision,
            pinnedToCurrentConversation: false,
            publicationState: app.workflow,
            mcpVerification: LocalAppVerificationSummary(
                status: verificationDigest.isEmpty ? .unverified : .passed,
                summary: verificationDigest.isEmpty ? "MCP verification pending." : "MCP verification evidence available.",
                code: verificationDigest.isEmpty ? nil : verificationDigest
            ),
            uiVerification: LocalAppVerificationSummary(
                status: app.workflow == .publishedVerified ? .passed : .unverified,
                summary: app.workflow == .publishedVerified ? "Published UI verification passed." : "UI verification pending.",
                code: nil
            ),
            enabledTools: enabledTools,
            widget: nil,
            tools: parsedTools
        )
    }

    func managedLocalAppID(serverName: String) -> String? {
        let prefix = "local_app_"
        guard serverName.hasPrefix(prefix) else { return nil }
        let appID = String(serverName.dropFirst(prefix.count))
        return appID.isEmpty ? nil : appID
    }

    private func tool(_ object: [String: Any]) -> LocalAppManagedMcpTool? {
        guard let definition = object["definition"] as? [String: Any],
              let name = definition["name"] as? String
        else { return nil }
        return LocalAppManagedMcpTool(
            name: name,
            title: definition["title"] as? String,
            description: definition["description"] as? String,
            inputSchemaSummary: jsonSummary(definition["inputSchema"]) ?? "{}",
            outputSchemaSummary: jsonSummary(definition["outputSchema"]),
            annotationsSummary: jsonSummary(definition["annotations"]),
            executionSummary: jsonSummary(definition["execution"] ?? object["execution"]),
            visibleMetaSummary: jsonSummary(definition["_meta"] ?? definition["meta"]),
            semanticFlowSummary: jsonSummary(object["flow"]) ?? "{}",
            ceilingSummary: jsonSummary(object["ceiling"]) ?? String(describing: object["ceiling"] ?? "deny")
        )
    }

    private func jsonObject(at url: URL) -> [String: Any]? {
        guard let data = try? Data(contentsOf: url),
              let json = try? JSONSerialization.jsonObject(with: data),
              let object = json as? [String: Any]
        else { return nil }
        return object
    }

    private func jsonSummary(_ value: Any?) -> String? {
        guard let value else { return nil }
        guard JSONSerialization.isValidJSONObject(value),
              let data = try? JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys]),
              let string = String(data: data, encoding: .utf8)
        else {
            return String(describing: value)
        }
        return string
    }
}
