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

    /// The host's answer to `ProposeAppIdentity` — a create sheet's editable
    /// defaults, never an authority. Both fields are fixed at creation (a
    /// surface is immutable once scaffolded and apps have no rename), so the
    /// sheet shows them and the user gets the last word.
    struct AppIdentityProposal: Equatable {
        let name: String
        let surface: LocalAppSurface
    }

    private(set) var apps: [LocalAppSummary] = []
    private(set) var runtimes: [String: LocalAppRuntimeStatus] = [:]
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
    private(set) var lastRefreshAt: Date?
    /// The id of the app the last `createApp` produced, consumed once by the
    /// library so the user lands on the new app's detail screen. The FALLBACK
    /// landing — the preferred signal is [`createdAppLanding`], which opens the
    /// app's own conversation.
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
        let brief: String
        let modelOverride: String?
    }

    struct PendingWidgetSetup: Identifiable, Equatable {
        let appID: String
        let appName: String

        var id: String { appID }
    }

    private(set) var pendingWidgetSetup: PendingWidgetSetup?
    private(set) var pendingPermission: LocalAppPermissionPrompt?
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

    /// In-flight `ProposeAppIdentity` calls, keyed by request id. Keyed rather
    /// than a single slot so a sheet that was retyped and re-submitted resolves
    /// its OWN answer instead of adopting the stale one.
    @ObservationIgnored private var identityProposalWaiters:
        [String: CheckedContinuation<AppIdentityProposal?, Never>] = [:]
    /// Request ids whose `ProposeAppIdentity` has been armed but whose waiter
    /// may not be installed yet, and the answers that arrived in that window.
    ///
    /// The engine emits `AppIdentityProposed` INSIDE the same `submit` call
    /// that asks for it, so the answer can reach `handle(event:)` while
    /// `proposeIdentity` is still suspended in `send` — before
    /// `withCheckedContinuation` has run. Without this the answer was dropped
    /// and the sheet sat on "naming…" for the full 20-second timeout. Bounded
    /// by the armed set: nothing is buffered for an id that is not in flight.
    @ObservationIgnored private var identityProposalsArmed: Set<String> = []
    @ObservationIgnored private var identityProposalAnswers: [String: AppIdentityProposal] = [:]
    /// Whether a create is armed. A COUNT of one, not a keyed claim: the engine
    /// now names the record it committed on `AppCreated`, so nothing has to be
    /// matched by brief or inferred from a catalog diff.
    /// The brief of the create this sheet submitted and has not yet seen land.
    ///
    /// KEYED, not a bare flag. `AppCreated` carries no correlator back to the
    /// client that asked, and an agent in another conversation can commit its
    /// own `LocalAppCreate` in the same window — claiming that record would
    /// open someone else's app and leave this one with no landing at all.
    /// Matching the brief is exact rather than inferred: create-first sends the
    /// user's own sentence and `AppService::create_app` stores it verbatim
    /// (both ends trim). The deferred flow could not do this — the agent
    /// rewrote the brief before creating anything.
    @ObservationIgnored private var creationBrief: String?
    @ObservationIgnored private var creationWantsWidget = false
    /// The workflow model the create sheet picked, held until `AppCreated` can
    /// put it on the landing. The record does not echo it back, and the app's
    /// first conversation is where it has to take effect.
    @ObservationIgnored private var creationModelOverride: String?
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
    #endif
    @ObservationIgnored private var approvedUIAutomation: [String: LocalAppCapabilityDecision] = [:]
    @ObservationIgnored private var runtimeLastUsedAt: [String: Date] = [:]
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
        @ObservationIgnored private var pendingBackgroundMutationRequests: Set<String> = []
    #endif

    init(websiteDataStoreRegistry: LocalAppWebsiteDataStoreRegistry? = nil) {
        self.websiteDataStoreRegistry = websiteDataStoreRegistry ?? .shared
    }

    var filteredApps: [LocalAppSummary] {
        let query = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return apps }
        return apps.filter { app in
            app.name.localizedStandardContains(query)
                || app.workflow.label.localizedStandardContains(query)
        }
    }

    var distributionMode: LocalAppsDistributionMode { .current }

    #if canImport(engine_mobileFFI)
        func configure(submit: @escaping (ClientCommand) async throws -> Void) {
            submitCommand = submit
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
                let updatedApps = records.map(LocalAppsProtocolAdapter.app).sorted {
                    $0.updatedAt > $1.updatedAt
                }
                apps = updatedApps
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

            case let .appIdentityProposed(requestId, name, surface):
                // Delivered to the ONE waiter that asked. An id with no waiter
                // is a proposal whose sheet already timed out or was dismissed;
                // dropping it is correct, and adopting it would overwrite a
                // name the user has since typed.
                let proposal = AppIdentityProposal(
                    name: name,
                    surface: LocalAppsProtocolAdapter.surface(surface)
                )
                if let waiter = identityProposalWaiters.removeValue(forKey: requestId) {
                    identityProposalsArmed.remove(requestId)
                    waiter.resume(returning: proposal)
                } else if identityProposalsArmed.contains(requestId) {
                    // The answer beat its waiter — see `identityProposalsArmed`.
                    identityProposalAnswers[requestId] = proposal
                }

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

            case let .appOperationFailed(appId, _, message):
                isRefreshing = false
                errorMessage = message
                // CreateApp failures carry no app id. Fail-closed: drop the
                // in-flight create claim so a later AppsChanged cannot arm
                // widget setup or steal landing. Per-app failures must not
                // cancel a create that already announced and is only waiting
                // for its init-session pin.
                if appId == nil {
                    creationBrief = nil
                    creationWantsWidget = false
                    creationModelOverride = nil
                }

            default:
                break
            }
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
        // claim — and nothing else clears `creationBrief`. Left armed it is a
        // permanent latch: every later Create returns
        // `local_apps_error_create_in_progress` for the process lifetime. The
        // app itself still gets created; only this session's landing is lost,
        // which the refresh below makes visible in the catalog anyway.
        creationBrief = nil
        creationWantsWidget = false
        creationModelOverride = nil
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

    /// How long the sheet waits for a proposal before showing its own defaults.
    ///
    /// A ceiling, not an expectation: this is one short model call. It exists so
    /// a dropped event cannot leave the sheet spinning with no way forward —
    /// the fields it would have filled are editable either way.
    static let identityProposalTimeout: Duration = .seconds(20)

    /// The name shown when no proposal arrives.
    ///
    /// Deliberately the SAME derivation `AppService::create_app` applies to a
    /// blank name (the brief's first 24 CHARACTERS, never bytes — a byte cut
    /// would split a CJK codepoint), so the field the user sees is what they
    /// would have got anyway.
    static func fallbackName(brief: String) -> String {
        String(brief.trimmingCharacters(in: .whitespacesAndNewlines).prefix(24))
    }

    /// Ask the host to name and shape an app from the brief, WITHOUT creating
    /// anything.
    ///
    /// Never fails: an unreachable engine, a refused command or a lost event
    /// all yield the same derived defaults the host itself falls back to. Both
    /// fields are editable in the sheet, so a bad proposal costs a correction,
    /// not a create.
    func proposeIdentity(brief: String) async -> AppIdentityProposal {
        let trimmed = brief.trimmingCharacters(in: .whitespacesAndNewlines)
        let fallback = AppIdentityProposal(
            name: Self.fallbackName(brief: trimmed), surface: .dom)
        guard !trimmed.isEmpty else { return fallback }
        #if canImport(engine_mobileFFI)
            let requestID = UUID().uuidString
            // Arm BEFORE submitting: the engine answers inside `submit`.
            identityProposalsArmed.insert(requestID)
            guard await send(.proposeAppIdentity(requestId: requestID, brief: trimmed))
            else {
                abandonIdentityProposal(requestID: requestID)
                return fallback
            }
            if let early = identityProposalAnswers.removeValue(forKey: requestID) {
                identityProposalsArmed.remove(requestID)
                return early
            }
            let answer = await withCheckedContinuation {
                (continuation: CheckedContinuation<AppIdentityProposal?, Never>) in
                identityProposalWaiters[requestID] = continuation
                Task { [weak self] in
                    try? await Task.sleep(for: Self.identityProposalTimeout)
                    self?.abandonIdentityProposal(requestID: requestID)
                }
            }
            return answer ?? fallback
        #else
            return fallback
        #endif
    }

    /// Resume a proposal waiter with nothing. Idempotent: the answering event
    /// removes the waiter first, so a timeout that fires afterwards is a no-op
    /// rather than a double resume.
    private func abandonIdentityProposal(requestID: String) {
        identityProposalsArmed.remove(requestID)
        identityProposalAnswers.removeValue(forKey: requestID)
        identityProposalWaiters.removeValue(forKey: requestID)?.resume(returning: nil)
    }

    /// Create the app outright, from choices the user has already seen.
    ///
    /// Creating BEFORE any conversation exists is the whole point: the app's
    /// first conversation is opened in the app's own scope, so its cwd is the
    /// app workspace. The previous flow ran an intake conversation in the
    /// project scope and handed off afterwards, which meant every agent step
    /// before the hand-off was rooted in the wrong directory.
    func createApp(
        brief: String,
        name: String,
        surface: LocalAppSurface,
        gitEnabled: Bool = true,
        modelOverride: String? = nil,
        addWidget: Bool = false
    ) async -> Bool {
        let trimmed = brief.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            errorMessage = String(localized: "local_apps_error_brief_required")
            return false
        }
        guard creationBrief == nil else {
            errorMessage = String(localized: "local_apps_error_create_in_progress")
            return false
        }
        #if canImport(engine_mobileFFI)
            let trimmedModel = modelOverride?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let workflowModel = trimmedModel.flatMap { $0.isEmpty ? nil : $0 }
            // An empty name is not an error: `AppService::create_app` derives
            // one from the brief, the same way `fallbackName` does.
            let trimmedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
            creationBrief = trimmed
            creationWantsWidget = addWidget
            creationModelOverride = workflowModel
            let succeeded = await send(
                .createApp(
                    name: trimmedName,
                    origin: .library,
                    brief: trimmed,
                    gitEnabled: gitEnabled,
                    workflowModel: workflowModel,
                    conversationId: nil,
                    surface: LocalAppsProtocolAdapter.surfaceDto(surface)
                )
            )
            if !succeeded {
                creationBrief = nil
                creationWantsWidget = false
                creationModelOverride = nil
            }
            return succeeded
        #else
            errorMessage = String(localized: "local_apps_error_engine_unavailable")
            return false
        #endif
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
            pendingSessionRequestOffsets[appID] = offset ?? 0
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
            let authorization = LocalAppsProtocolAdapter.authorizationDecision(decision)
            switch source {
            case let .ui(request):
                if decision == .deny {
                    await resolveUIRequest(
                        requestID: request.requestId,
                        decision: authorization,
                        resultJSON: nil,
                        error: String(localized: "local_apps_error_ui_rejected")
                    )
                    return
                }
                if decision == .session || decision == .always {
                    approvedUIAutomation[request.appId] = decision
                }
                let result = await executeUIRequestInPreview(request)
                await resolveUIRequest(
                    requestID: request.requestId,
                    decision: authorization,
                    resultJSON: result.resultJSON,
                    error: result.error
                )
            case let .capability(appID, kind):
                if kind == .uiControl, decision != .deny {
                    approvedUIAutomation[appID] = decision
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
            case let .appCreated(record):
                // The engine names the record it just committed, for BOTH
                // creation paths. Emitted after `AppsChanged`, so the catalog
                // already contains it.
                //
                // Deliberately does NOT switch conversation scope here. An
                // AGENT-driven create happens mid-turn, and
                // `RootView.switchScope` submits `cancelAndWait()` first — the
                // landing would kill the very turn that produced the app. The
                // library arms the landing and `RootView` walks in once the
                // turn is done (immediately, for a create from the sheet,
                // where there is no turn at all).
                // Claim only the record this sheet asked for. An unkeyed
                // claim opens whichever app committed first.
                guard let claimed = creationBrief, claimed == record.brief else { break }
                creationBrief = nil
                let summary = LocalAppsProtocolAdapter.app(record)
                upsertApp(summary)
                createdAppID = summary.id
                // The hand-off target. The engine minted the app's own session
                // at creation; the build has to run THERE, because a workflow
                // launched from the intake conversation inherits the project
                // cwd and edits whatever sits in it. RootView fires this once
                // the intake turn has finished — switching scope mid-turn
                // submits `cancelAndWait()` and would kill the turn that just
                // produced the app.
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
                    initSessionID: record.initSessionId,
                    brief: summary.brief,
                    modelOverride: creationModelOverride
                )
                creationModelOverride = nil
                if creationWantsWidget {
                    creationWantsWidget = false
                    pendingWidgetSetup = PendingWidgetSetup(
                        appID: summary.id,
                        appName: summary.name.isEmpty ? summary.brief : summary.name
                    )
                    publishWidgetSnapshotNow()
                }

            case let .appDetailsChanged(details):
                let summary = LocalAppsProtocolAdapter.app(details.app)
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
                let summary = LocalAppsProtocolAdapter.app(record)
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

    private func makeWidgetSnapshot() -> LocalAppWidgetSnapshot {
        LocalAppWidgetSnapshot(
            version: LocalAppWidgetSnapshot.currentVersion,
            apps: apps.map { app in
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
        lock.lock()
        if let handler {
            lock.unlock()
            return handler
        }
        let id = UUID()
        lock.unlock()
        return await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                lock.lock()
                if let handler {
                    lock.unlock()
                    continuation.resume(returning: handler)
                } else if Task.isCancelled {
                    lock.unlock()
                    continuation.resume(returning: nil)
                } else {
                    waiters[id] = continuation
                    lock.unlock()
                }
            }
        } onCancel: {
            lock.lock()
            let continuation = waiters.removeValue(forKey: id)
            lock.unlock()
            continuation?.resume(returning: nil)
        }
    }
}
