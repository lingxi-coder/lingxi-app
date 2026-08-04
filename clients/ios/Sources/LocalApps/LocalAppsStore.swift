import Foundation
import Observation

@Observable
@MainActor
final class LocalAppsStore {
    /// Prompts waiting behind the one on screen. A page cannot grow this
    /// without bound; past the cap the newest request is refused.
    private static let maxQueuedPermissions = 8

    /// How long a sent draft patch may hold the single in-flight slot before it is
    /// assumed lost and re-queued. Far longer than a healthy ack, so it normally
    /// only fires on a failure the event stream could not attribute — but it is a
    /// heuristic, not a guarantee: a slow engine, or a device resumed after a long
    /// suspension, can trip it early. That is tolerable only because the aged-out
    /// edit is RE-SENT rather than dropped, so an early trip costs one idempotent
    /// set-op. Settable so a test can reach the path without waiting out the
    /// production budget.
    @ObservationIgnored var inFlightEditBudget: Duration = .seconds(15)

    private enum PendingPermissionSource {
        #if canImport(engine_mobileFFI)
            case ui(AppUiRequestDto)
            case capability(appID: String, kind: AppCapabilityKindDto)
        #endif
    }

    private struct PendingEdit {
        let field: LocalAppDesignField
        let value: LocalAppDesignValue
        /// A debounced text edit is queued immediately so an inbound draft snapshot
        /// cannot wipe it; only the SEND waits for the 400 ms timer.
        var isReady: Bool
        /// Draft revision the patch was sent at, so its own ack is recognisable.
        var sentAtRevision: UInt64 = 0
        /// When the patch took the in-flight slot, so a stranded one can be aged out.
        var sentAt: ContinuousClock.Instant = ContinuousClock.now
    }

    private struct PendingCreation {
        let name: String
        let template: LocalAppTemplateKind
        let knownAppIDs: Set<String>
    }

    private(set) var apps: [LocalAppSummary] = []
    private(set) var templates: [LocalAppTemplate] = []
    private(set) var designers: [String: LocalAppDesignerSession] = [:]
    private(set) var suggestions: [String: LocalAppSuggestionDiff] = [:]
    private(set) var previews: [String: LocalAppPreviewSession] = [:]
    private(set) var runtimes: [String: LocalAppRuntimeStatus] = [:]
    private(set) var generationProgress: [String: LocalAppGenerationProgress] = [:]
    private(set) var checkpoints: [String: [LocalAppCheckpoint]] = [:]
    private(set) var isRefreshing = false
    private(set) var errorMessage: String?
    private(set) var lastRefreshAt: Date?
    private(set) var createdAppIDForDesigner: String?
    private(set) var pendingPermission: LocalAppPermissionPrompt?
    private(set) var requestedPresentationAppID: String?
    private(set) var activeUIRequestAppID: String?

    var searchQuery = ""
    var templateFilter: LocalAppTemplateKind?

    @ObservationIgnored private var debounceTasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var pendingEdits: [String: [String: PendingEdit]] = [:]
    @ObservationIgnored private var inFlightEdits: [String: PendingEdit] = [:]
    @ObservationIgnored private var runningBeforeSuspension = Set<String>()
    @ObservationIgnored private var pendingCreation: PendingCreation?
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

    #if canImport(engine_mobileFFI)
        @ObservationIgnored private var submitCommand: ((ClientCommand) async throws -> Void)?
    #endif

    var filteredApps: [LocalAppSummary] {
        let query = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        return apps.filter { app in
            let matchesTemplate = templateFilter == nil || app.templateKind == templateFilter
            let matchesText = query.isEmpty
                || app.name.localizedStandardContains(query)
                || app.workflow.label.localizedStandardContains(query)
            return matchesTemplate && matchesText
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
                if let pendingCreation,
                   let created = updatedApps.first(where: {
                       !pendingCreation.knownAppIDs.contains($0.id)
                           && $0.name == pendingCreation.name
                           && $0.templateKind == pendingCreation.template
                   }) {
                    self.pendingCreation = nil
                    createdAppIDForDesigner = created.id
                    Task { await openDesigner(appID: created.id) }
                }
                lastRefreshAt = .now
                isRefreshing = false

            case let .appDesignerRequested(appId, interactionId, revision):
                var designer = designers[appId] ?? LocalAppDesignerSession(
                    appID: appId,
                    revision: revision,
                    interactionID: nil,
                    fields: [:],
                    currentStep: 0
                )
                designer.revision = revision
                designer.interactionID = interactionId
                designers[appId] = designer

            case let .appDesignDraftChanged(appId, revision, fields):
                var designer = designers[appId] ?? LocalAppDesignerSession(
                    appID: appId,
                    revision: revision,
                    interactionID: nil,
                    fields: [:],
                    currentStep: 0
                )
                let serverFields = fields.mapValues(LocalAppsProtocolAdapter.designValue)
                designer.revision = revision
                designer.fields = serverFields
                if let inFlight = inFlightEdits[appId] {
                    designer.fields[inFlight.field.id] = inFlight.value
                }
                if let queued = pendingEdits[appId] {
                    for (fieldID, edit) in queued {
                        designer.fields[fieldID] = edit.value
                    }
                }
                designers[appId] = designer
                // The engine re-ships the whole draft on every change, so only the echo
                // carrying our own value at a newer revision acknowledges the patch.
                if let inFlight = inFlightEdits[appId],
                   revision > inFlight.sentAtRevision,
                   serverFields[inFlight.field.id] == inFlight.value {
                    inFlightEdits[appId] = nil
                }
                flushNextEdit(appID: appId)

            case let .appDesignSuggestionAvailable(appId, suggestionId, basedOnRevision, patch):
                let fields = designers[appId]?.fields ?? [:]
                suggestions[appId] = LocalAppSuggestionDiff(
                    id: suggestionId,
                    summary: patch.note ?? "Agent 建议调整以下设计字段",
                    basedOnRevision: basedOnRevision,
                    changes: LocalAppsProtocolAdapter.patchChanges(patch, fields: fields)
                )

            case let .appDesignConflict(appId, _, actualRevision):
                if let edit = inFlightEdits.removeValue(forKey: appId),
                   pendingEdits[appId]?[edit.field.id] == nil {
                    // A newer local value for this field supersedes the rejected one.
                    pendingEdits[appId, default: [:]][edit.field.id] = edit
                }
                designers[appId]?.revision = actualRevision
                errorMessage = "设计已在其他位置更新，正在基于最新版本重试。"
                flushNextEdit(appID: appId)

            case let .appWorkflowChanged(appId, state, detail):
                updateApp(appID: appId) { app in
                    app.workflow = LocalAppsProtocolAdapter.workflow(state)
                    app.updatedAt = .now
                }
                if let detail, !detail.isEmpty { errorMessage = detail }

            case let .appGenerationProgress(appId, stage, percent, detail):
                generationProgress[appId] = LocalAppGenerationProgress(
                    stage: stage,
                    percent: percent,
                    detail: detail
                )

            case let .appRuntimeChanged(appId, state, details, lastError):
                runtimes[appId] = LocalAppsProtocolAdapter.runtime(
                    state,
                    details: details,
                    lastError: lastError,
                    knownURL: previews[appId]?.url
                )
                if state == .running, runtimeLastUsedAt[appId] == nil {
                    runtimeLastUsedAt[appId] = .now
                }

            case let .appEvent(event):
                handleAppEvent(event)

            case let .appPreviewReady(appId, interactionId, revision, url):
                // The gate announcement never carries a url (service.rs gate_announcement),
                // so fall back to the loopback url the runtime already reported.
                let previewURL = url.flatMap(URL.init(string:)) ?? runtimes[appId]?.url
                previews[appId] = LocalAppPreviewSession(
                    appID: appId,
                    revision: revision,
                    interactionID: interactionId,
                    url: previewURL
                )
                if case .running = runtimes[appId] {
                    runtimes[appId] = .running(previewURL)
                }

            case let .appCheckpointCreated(appId, checkpoint):
                let item = LocalAppsProtocolAdapter.checkpoint(checkpoint)
                var values = checkpoints[appId] ?? []
                values.removeAll { $0.id == item.id }
                values.append(item)
                checkpoints[appId] = values.sorted { $0.createdAt > $1.createdAt }

            case let .appOperationFailed(appId, code, message):
                if let appId { generationProgress[appId] = nil }
                isRefreshing = false
                if code == .revisionConflict,
                   let appId,
                   inFlightEdits[appId] != nil || pendingEdits[appId]?.isEmpty == false {
                    // The conflict arm owns this failure: it already queued the rebase
                    // retry and set the copy this raw engine string would replace.
                    return
                }
                if let appId, inFlightEdits[appId] != nil,
                   code == .invalidRequest || code == .workflowStateInvalid {
                    // A rejected patch must not wedge the single-slot draft queue.
                    // These two codes are the ones a rejected patch produces:
                    // validate_patch / validate_design_value produce InvalidRequest and
                    // ensure_workflow produces WorkflowStateInvalid.
                    //
                    // They are NOT the only codes `update_draft` can produce — it runs
                    // inside `AppService::with_app`, which also yields NotFound
                    // (position), Io (persist_mutation, and a JoinError lowered to Io)
                    // and StorageCorrupt. Those cannot be handled here: an Io from a
                    // failed persist and an Io from a failed runtime start are the same
                    // event, because `AppOperationFailed` carries only
                    // (app_id, code, message) and no correlation id. Widening the set
                    // would drop a live patch on every unrelated runtime failure — see
                    // `testUnrelatedAppFailureKeepsTheLiveDraftPatch`. The wedge those
                    // codes would otherwise cause is bounded by the in-flight watchdog
                    // in `flushNextEdit` instead.
                    inFlightEdits[appId] = nil
                    flushNextEdit(appID: appId)
                }
                errorMessage = message

            default:
                break
            }
        }
    #endif

    func installTemplates(_ values: [LocalAppTemplate]) {
        templates = values.sorted { lhs, rhs in
            lhs.name.localizedStandardCompare(rhs.name) == .orderedAscending
        }
    }

    func template(for app: LocalAppSummary) -> LocalAppTemplate? {
        templates.first { $0.kind == app.templateKind }
    }

    func app(id: String) -> LocalAppSummary? {
        apps.first { $0.id == id }
    }

    func clearError() {
        errorMessage = nil
    }

    func consumeCreatedAppID() -> String? {
        defer { createdAppIDForDesigner = nil }
        return createdAppIDForDesigner
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
                errorMessage = "本地应用引擎尚未连接。"
                return
            }
            isRefreshing = true
            do {
                try await submitCommand(.listApps)
                try await submitCommand(.listAppTemplates)
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

    func createApp(name: String, template: LocalAppTemplate) async -> Bool {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            errorMessage = "请输入应用名称。"
            return false
        }
        #if canImport(engine_mobileFFI)
            pendingCreation = PendingCreation(
                name: trimmed,
                template: template.kind,
                knownAppIDs: Set(apps.map(\.id))
            )
            let succeeded = await send(
                .createApp(
                    name: trimmed,
                    template: LocalAppsProtocolAdapter.templateKind(template.kind),
                    origin: .library,
                    conversationId: nil
                )
            )
            if !succeeded { pendingCreation = nil }
            return succeeded
        #else
            errorMessage = "此构建未包含本地应用引擎。"
            return false
        #endif
    }

    func openDesigner(appID: String) async {
        #if canImport(engine_mobileFFI)
            _ = await send(.getAppDetails(appId: appID))
            _ = await send(.openAppDesigner(appId: appID))
        #endif
    }

    func getDetails(appID: String) async {
        #if canImport(engine_mobileFFI)
            _ = await send(.getAppDetails(appId: appID))
        #endif
    }

    func setCurrentStep(_ step: Int, appID: String) {
        guard var designer = designers[appID] else { return }
        designer.currentStep = max(0, step)
        designers[appID] = designer
    }

    func edit(field: LocalAppDesignField, value: LocalAppDesignValue, appID: String) {
        var designer = designers[appID] ?? LocalAppDesignerSession(
            appID: appID,
            revision: 0,
            interactionID: nil,
            fields: [:],
            currentStep: 0
        )
        designer.fields[field.id] = value
        designers[appID] = designer

        let key = "\(appID):\(field.id)"
        debounceTasks[key]?.cancel()
        let debounced = field.type == .shortText || field.type == .longText
        pendingEdits[appID, default: [:]][field.id] = PendingEdit(
            field: field,
            value: value,
            isReady: !debounced
        )
        if debounced {
            debounceTasks[key] = Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(400))
                guard !Task.isCancelled else { return }
                self?.markEditReady(field: field, appID: appID)
            }
        } else {
            flushNextEdit(appID: appID)
        }
    }

    func applySuggestion(appID: String) async {
        guard let suggestion = suggestions[appID], let designer = designers[appID] else { return }
        #if canImport(engine_mobileFFI)
            if await send(
                .applyAgentDesignSuggestion(
                    appId: appID,
                    suggestionId: suggestion.id,
                    expectedRevision: designer.revision
                )
            ) {
                suggestions[appID] = nil
            }
        #endif
    }

    func requestDesignSuggestion(appID: String, prompt: String? = nil) async {
        guard let revision = designers[appID]?.revision else {
            errorMessage = "请等待应用设计详情加载完成。"
            return
        }
        #if canImport(engine_mobileFFI)
            _ = await send(
                .requestAppDesignSuggestion(
                    appId: appID,
                    expectedRevision: revision,
                    prompt: prompt
                )
            )
        #endif
    }

    func dismissSuggestion(appID: String) async {
        guard let suggestion = suggestions[appID] else { return }
        #if canImport(engine_mobileFFI)
            if await send(
                .dismissAppDesignSuggestion(appId: appID, suggestionId: suggestion.id)
            ) {
                suggestions[appID] = nil
            }
        #else
            suggestions[appID] = nil
        #endif
    }

    func confirmDesign(appID: String) async -> Bool {
        guard let designer = designers[appID], let interactionID = designer.interactionID else {
            errorMessage = "设计确认请求尚未准备好。"
            return false
        }
        #if canImport(engine_mobileFFI)
            guard await drainPendingEdits(appID: appID) else {
                // Commands are delivered in order, so a patch still in flight
                // reaches the engine BEFORE this confirm and moves the revision
                // under it — the confirm could only fail with a revision
                // conflict. Keep the user on the designer with the answer queued.
                errorMessage = "设计尚未保存完成，请重试。"
                return false
            }
            return await send(
                .confirmAppDesign(
                    appId: appID,
                    revision: designers[appID]?.revision ?? designer.revision,
                    interactionId: interactionID
                )
            )
        #else
            return false
        #endif
    }

    func cancelDesign(appID: String) async {
        #if canImport(engine_mobileFFI)
            if await send(.cancelAppDesign(appId: appID)) {
                designers[appID]?.interactionID = nil
            }
        #endif
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

    func approvePreview(appID: String) async -> Bool {
        guard let preview = previews[appID] else {
            errorMessage = "预览确认请求尚未准备好。"
            return false
        }
        #if canImport(engine_mobileFFI)
            return await send(
                .confirmAppPreview(
                    appId: appID,
                    revision: preview.revision,
                    interactionId: preview.interactionID
                )
            )
        #else
            return false
        #endif
    }

    func requestRevision(appID: String, feedback: String) async -> Bool {
        let trimmed = feedback.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            errorMessage = "请输入要修改的内容。"
            return false
        }
        #if canImport(engine_mobileFFI)
            return await send(.requestAppRevision(appId: appID, prompt: trimmed))
        #else
            return false
        #endif
    }

    func retryGeneration(appID: String) async {
        #if canImport(engine_mobileFFI)
            _ = await send(.retryAppGeneration(appId: appID))
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
            default: operation = nil
            }
            guard let operation else {
                LocalAppWebViewRegistry.shared.resolveBridge(
                    appID: request.appID,
                    requestID: request.id,
                    resultJSON: nil,
                    error: "不支持的 Lingxi Bridge 操作。"
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
                    error: "本地应用引擎未接受 Bridge 请求。"
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
                        error: "用户拒绝了界面操作。"
                    )
                    return
                }
                if decision == .session || decision == .always {
                    approvedUIAutomation[request.appId] = decision
                }
                let result = await LocalAppWebViewRegistry.shared.execute(request: request)
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
            return await send(.deleteApp(appId: appID))
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
        private func handleAppEvent(_ event: AppEventDto) {
            switch event {
            case let .appTemplatesChanged(templates):
                installTemplates(templates.map(LocalAppsProtocolAdapter.template))

            case let .appDetailsChanged(details):
                let summary = LocalAppsProtocolAdapter.app(details.app)
                upsertApp(summary)
                let existing = designers[summary.id]
                var fields = Dictionary(
                    uniqueKeysWithValues: details.designFields.map {
                        ($0.fieldId, LocalAppsProtocolAdapter.designValue($0.value))
                    }
                )
                if let inFlight = inFlightEdits[summary.id] {
                    fields[inFlight.field.id] = inFlight.value
                }
                for (fieldID, edit) in pendingEdits[summary.id] ?? [:] {
                    fields[fieldID] = edit.value
                }
                designers[summary.id] = LocalAppDesignerSession(
                    appID: summary.id,
                    revision: details.designRevision,
                    interactionID: existing?.interactionID,
                    fields: fields,
                    currentStep: existing?.currentStep ?? 0
                )
                runtimes[summary.id] = LocalAppsProtocolAdapter.runtime(
                    details.runtime.state,
                    details: details.runtime,
                    lastError: details.runtime.lastError,
                    knownURL: previews[summary.id]?.url
                )
                if let job = details.generationJob {
                    updateGenerationJob(job)
                } else {
                    generationProgress[summary.id] = nil
                }
                replaceCheckpoints(details.checkpoints, appID: summary.id)

            case let .appGenerationJobChanged(job):
                updateGenerationJob(job)

            case let .appBridgeResponse(response):
                LocalAppWebViewRegistry.shared.resolveBridge(
                    appID: response.appId,
                    requestID: response.requestId,
                    resultJSON: response.resultJson,
                    error: response.ok ? nil : (response.error ?? "Bridge 请求失败。")
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
                errorMessage = "应用同时请求了过多授权，已忽略最新的请求。"
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
                let result = await LocalAppWebViewRegistry.shared.execute(request: request)
                await self.resolveUIRequest(
                    requestID: request.requestId,
                    decision: decision,
                    resultJSON: result.resultJSON,
                    error: result.error
                )
            }
        }

        private func updateGenerationJob(_ job: AppGenerationJobDto) {
            generationProgress[job.appId] = LocalAppGenerationProgress(
                stage: LocalAppsProtocolAdapter.generationStage(job.state),
                percent: job.percent,
                detail: job.detail
            )
            if job.state == .failed, let detail = job.detail {
                errorMessage = detail
            }
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
            if let target { return "Agent 请求对 \(target) 执行结构化界面操作。" }
            return "Agent 请求执行结构化界面操作。"
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

    private func markEditReady(field: LocalAppDesignField, appID: String) {
        debounceTasks["\(appID):\(field.id)"] = nil
        pendingEdits[appID]?[field.id]?.isReady = true
        flushNextEdit(appID: appID)
    }

    /// The last designer step's required answer is a debounced text field, so a
    /// confirm that beats the timer would freeze a spec missing that answer.
    /// Returns `false` when the queue did not drain inside the budget; the
    /// queued edits are left intact so a retry re-flushes them.
    private func drainPendingEdits(appID: String) async -> Bool {
        for (key, task) in debounceTasks where key.hasPrefix("\(appID):") {
            task.cancel()
            debounceTasks[key] = nil
        }
        if let queued = pendingEdits[appID] {
            for fieldID in queued.keys { pendingEdits[appID]?[fieldID]?.isReady = true }
        }
        flushNextEdit(appID: appID)
        for _ in 0 ..< 100 {
            if inFlightEdits[appID] == nil, pendingEdits[appID]?.isEmpty != false { return true }
            try? await Task.sleep(for: .milliseconds(50))
        }
        return false
    }

    private func flushNextEdit(appID: String) {
        // A patch whose failure could not be attributed (see the `appOperationFailed`
        // arm) holds the slot forever, wedging every later edit for the app. Release
        // it lazily, here, rather than from a timer: this runs exactly when the user
        // types again, which is when the wedge starts to matter, and it keeps the
        // queue free of a background task whose sleep would outlive the screen.
        if let stale = inFlightEdits[appID],
           stale.sentAt.duration(to: ContinuousClock.now) >= inFlightEditBudget {
            // Re-queue rather than discard, guarded exactly like the conflict arm so a
            // newer local value for the same field wins. Discarding would lose the
            // answer the user typed while the optimistic copy in `designers` kept the
            // confirm gate satisfied, so a confirm could ship a spec the engine never
            // received. Resending is safe and is what makes the budget's own timing
            // slack harmless: a set-op is idempotent if the ack was merely lost, and a
            // patch sent against a moved revision comes back as `AppDesignConflict`,
            // which the arm above already rebases.
            if pendingEdits[appID]?[stale.field.id] == nil {
                var revived = stale
                revived.isReady = true
                pendingEdits[appID, default: [:]][stale.field.id] = revived
            }
            inFlightEdits[appID] = nil
        }
        guard inFlightEdits[appID] == nil,
              let designer = designers[appID],
              let fieldID = pendingEdits[appID]?.filter({ $0.value.isReady }).keys.sorted().first,
              let edit = pendingEdits[appID]?.removeValue(forKey: fieldID)
        else { return }

        #if canImport(engine_mobileFFI)
            guard let value = LocalAppsProtocolAdapter.designValue(edit.value, fieldType: edit.field.type) else {
                errorMessage = "当前引擎尚未支持字段 \(edit.field.label) 的结构化值。"
                return
            }
            var inFlight = edit
            inFlight.sentAtRevision = designer.revision
            inFlight.sentAt = ContinuousClock.now
            inFlightEdits[appID] = inFlight
            Task { [weak self] in
                guard let self else { return }
                let succeeded = await self.send(
                    .updateAppDesignDraft(
                        appId: appID,
                        expectedRevision: designer.revision,
                        patch: AppDesignPatchDto(
                            ops: [.set(fieldId: fieldID, value: value)],
                            note: nil
                        )
                    )
                )
                if !succeeded {
                    self.inFlightEdits[appID] = nil
                }
            }
        #endif
    }

    #if canImport(engine_mobileFFI)
        private func send(_ command: ClientCommand) async -> Bool {
            guard let submitCommand else {
                errorMessage = "本地应用引擎尚未连接。"
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
