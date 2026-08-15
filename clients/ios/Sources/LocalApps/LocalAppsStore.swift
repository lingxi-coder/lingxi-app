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

    private struct PendingCreation {
        let knownAppIDs: Set<String>
        let modelOverride: String?
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
    /// library so the user lands on the new app's detail screen. v3: this is
    /// the FALLBACK landing — the preferred signal is
    /// [`createdAppSession`], which fires once the engine pins the init
    /// session so creation jumps straight into the init chat.
    private(set) var createdAppID: String?
    /// The just-created app once BOTH its id and its pinned init session are
    /// known — consumed once by the library to dismiss the cover and open
    /// the init conversation directly.
    struct CreatedAppSession: Equatable {
        let appID: String
        let initSessionID: String
        /// The one-line brief the user entered at create time — carried into
        /// the init-chat kickoff so the agent starts from the ACTUAL ask,
        /// not a generic opener.
        let brief: String
        /// Optional provider-qualified workflow model selected in the create UI.
        /// `nil` means inherit the new conversation's live model.
        let modelOverride: String?
    }

    private(set) var createdAppSession: CreatedAppSession?
    /// Created apps still waiting for their `init_session_id` pin (the
    /// engine's second `AppsChanged` announce). A SET, not one slot: two
    /// creates inside the fallback window would otherwise overwrite each
    /// other, stranding the first with no landing at all.
    @ObservationIgnored private var awaitingInitPinAppIDs: Set<String> = []
    @ObservationIgnored private var awaitingInitPinModelOverrides: [String: String] = [:]
    private(set) var pendingPermission: LocalAppPermissionPrompt?
    private(set) var requestedPresentationAppID: String?
    private(set) var activeUIRequestAppID: String?

    var searchQuery = ""

    @ObservationIgnored private var runningBeforeSuspension = Set<String>()
    @ObservationIgnored private var pendingCreation: PendingCreation?
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

    #if canImport(engine_mobileFFI)
        @ObservationIgnored private var submitCommand: ((ClientCommand) async throws -> Void)?
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

        func handle(event: ClientEvent) {
            switch event {
            case let .appsChanged(records):
                let updatedApps = records.map(LocalAppsProtocolAdapter.app).sorted {
                    $0.updatedAt > $1.updatedAt
                }
                apps = updatedApps
                scheduleWebsiteDataCleanup(activeAppIDs: Set(updatedApps.map(\.id)))
                // `createApp(brief:)` sends an empty `name`, letting the engine
                // derive the display name from the brief (`AppService::create_app`,
                // first 24 chars) — so the created row can no longer be matched by
                // name. A single pending creation only ever produces one new id, so
                // "not in the pre-create snapshot" is sufficient on its own.
                if let pendingCreation,
                   let created = updatedApps.first(where: { !pendingCreation.knownAppIDs.contains($0.id) }) {
                    self.pendingCreation = nil
                    if let initSession = created.initSessionId {
                        // The pin arrived with the first announce (fast path).
                        createdAppSession = CreatedAppSession(
                            appID: created.id, initSessionID: initSession,
                            brief: created.brief,
                            modelOverride: pendingCreation.modelOverride)
                    } else {
                        // The engine announces twice: first the record, then
                        // the `init_session_id` pin (`set_init_session`
                        // re-announce). Wait briefly for the pin so creation
                        // can land DIRECTLY in the init chat; if the mint
                        // failed engine-side, fall back to the details page.
                        awaitingInitPinAppIDs.insert(created.id)
                        if let modelOverride = pendingCreation.modelOverride {
                            awaitingInitPinModelOverrides[created.id] = modelOverride
                        }
                        let appID = created.id
                        Task { [weak self] in
                            try? await Task.sleep(for: .seconds(3))
                            guard let self,
                                  self.awaitingInitPinAppIDs.remove(appID) != nil else { return }
                            self.awaitingInitPinModelOverrides.removeValue(forKey: appID)
                            self.createdAppID = appID
                        }
                    }
                }
                // Land the FIRST awaited app whose pin has arrived. Others
                // stay armed for their own announce (or their own fallback).
                if let pinned = updatedApps.first(where: {
                    awaitingInitPinAppIDs.contains($0.id) && $0.initSessionId != nil
                }), let initSession = pinned.initSessionId {
                    awaitingInitPinAppIDs.remove(pinned.id)
                    createdAppSession = CreatedAppSession(
                        appID: pinned.id, initSessionID: initSession,
                        brief: pinned.brief,
                        modelOverride: awaitingInitPinModelOverrides.removeValue(forKey: pinned.id))
                }
                lastRefreshAt = .now
                isRefreshing = false

            case let .appWorkflowChanged(appId, state, detail):
                let workflow = LocalAppsProtocolAdapter.workflow(state)
                updateApp(appID: appId) { app in
                    app.workflow = workflow
                    app.updatedAt = .now
                }
                if let detail, !detail.isEmpty { errorMessage = detail }

            case let .appRuntimeChanged(appId, state, details, lastError):
                runtimes[appId] = LocalAppsProtocolAdapter.runtime(
                    state,
                    details: details,
                    lastError: lastError,
                    knownURL: runtimes[appId]?.url
                )
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

            case let .appOperationFailed(_, _, message):
                isRefreshing = false
                errorMessage = message
                // Disarm the create claim: it matches "an app id absent from
                // the pre-create snapshot", so leaving it armed after a failed
                // create makes the NEXT app to appear — including one the
                // assistant creates through the MCP tool minutes later — look
                // like the user's pending creation and yank them out of their
                // conversation into its init chat.
                pendingCreation = nil
                awaitingInitPinAppIDs.removeAll()

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

    func consumeCreatedAppSession() -> CreatedAppSession? {
        defer { createdAppSession = nil }
        return createdAppSession
    }

    func consumeRequestedPresentationAppID() -> String? {
        defer { requestedPresentationAppID = nil }
        return requestedPresentationAppID
    }

    func hasPendingUIRequest(appID: String) -> Bool {
        activeUIRequestAppID == appID
    }

    func refresh() async {
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
        await refresh()
        for appID in apps.map(\.id) {
            await getDetails(appID: appID)
        }
    }

    /// Creates an app from a real one-line brief — no display name is
    /// collected here. `name` goes over the wire empty, and `AppService::
    /// create_app` derives a display name from the brief itself (first 24
    /// characters) when none is supplied.
    func createApp(
        brief: String,
        gitEnabled: Bool = true,
        modelOverride: String? = nil
    ) async -> Bool {
        let trimmed = brief.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            errorMessage = String(localized: "local_apps_error_brief_required")
            return false
        }
        #if canImport(engine_mobileFFI)
            let trimmedModel = modelOverride?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let workflowModel = trimmedModel.flatMap { $0.isEmpty ? nil : $0 }
            pendingCreation = PendingCreation(
                knownAppIDs: Set(apps.map(\.id)),
                modelOverride: workflowModel
            )
            let succeeded = await send(
                .createApp(
                    name: "",
                    origin: .library,
                    brief: trimmed,
                    gitEnabled: gitEnabled,
                    workflowModel: workflowModel,
                    conversationId: nil
                )
            )
            if !succeeded { pendingCreation = nil }
            return succeeded
        #else
            errorMessage = String(localized: "local_apps_error_engine_unavailable")
            return false
        #endif
    }

    func getDetails(appID: String) async {
        #if canImport(engine_mobileFFI)
            _ = await send(.getAppDetails(appId: appID))
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
            if await send(.startApp(appId: appID)) {
                runtimeLastUsedAt[appID] = .now
            }
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
            case ("llm", "chat"): operation = .llmChat
            case ("agent", "post"): operation = .agentPost
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

            case let .appBridgeResponse(response):
                LocalAppWebViewRegistry.shared.resolveBridge(
                    appID: response.appId,
                    requestID: response.requestId,
                    resultJSON: response.resultJson,
                    error: response.ok ? nil : (response.error ?? String(localized: "local_apps_error_bridge_failed")),
                    code: response.errorCode
                )

            case let .appUiRequest(request):
                runtimeLastUsedAt[request.appId] = .now
                activeUIRequestAppID = request.appId
                requestedPresentationAppID = request.appId
                if request.action == .inspect {
                    executeUIRequest(request, decision: .allowOnce)
                } else if let decision = approvedUIAutomation[request.appId] {
                    if decision == .once { approvedUIAutomation[request.appId] = nil }
                    executeUIRequest(
                        request,
                        decision: LocalAppsProtocolAdapter.authorizationDecision(decision)
                    )
                } else {
                    enqueuePermission(
                        LocalAppPermissionPrompt(
                            id: request.requestId,
                            appID: request.appId,
                            kind: .uiAction(LocalAppsProtocolAdapter.uiActionLabel(request.action)),
                            reason: uiRequestReason(request),
                            domain: nil
                        ),
                        source: .ui(request)
                    )
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
            }
        }

        /// Strictly FIFO: the request the page issued first is answered first,
        /// because that is the one whose `fetch()` has been stalled longest.
        private func enqueuePermission(
            _ prompt: LocalAppPermissionPrompt,
            source: PendingPermissionSource
        ) {
            guard pendingPermission != nil else {
                pendingPermission = prompt
                pendingPermissionSource = source
                return
            }
            guard permissionQueue.count < Self.maxQueuedPermissions else {
                // Dropping the newest keeps every request that already has a
                // page waiting on it; the dropped one fails closed on the
                // engine's approval timeout.
                errorMessage = String(localized: "local_apps_error_permission_overflow")
                return
            }
            permissionQueue.append((prompt: prompt, source: source))
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
            activeUIRequestAppID = nil
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
