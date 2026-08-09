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
        let knownAppIDs: Set<String>
    }

    private(set) var apps: [LocalAppSummary] = []
    private(set) var designers: [String: LocalAppDesignerSession] = [:]
    /// The LLM-authored questionnaire per app (local-apps#questionnaire, Task
    /// 13). Replaces the deleted static `templates` cache — the questionnaire
    /// is authored per-app from its brief, not looked up from a catalog.
    private(set) var questionnaires: [String: [LocalAppDesignStep]] = [:]
    /// The LLM-derived plan awaiting confirmation, keyed by app id. Cleared by
    /// the engine (and mirrored here) the moment an answer edit invalidates a
    /// previously-derived plan.
    private(set) var plans: [String: LocalAppPlan] = [:]
    private(set) var suggestions: [String: LocalAppSuggestionDiff] = [:]
    private(set) var previews: [String: LocalAppPreviewSession] = [:]
    private(set) var runtimes: [String: LocalAppRuntimeStatus] = [:]
    private(set) var generationProgress: [String: LocalAppGenerationProgress] = [:]
    /// Live model output for an app whose generation is running, oldest block
    /// first. Rendered by the same `MessageBubble` the conversation uses.
    ///
    /// Separate from `generationProgress` because the two answer different
    /// questions and have different lifetimes: that map holds ONE latest value
    /// ("which pipeline stage, how far"), this one ACCUMULATES ("what has the
    /// model said"). Folding the live chunks into `generationProgress` would
    /// have each chunk erase the stage the UI needs to keep showing.
    private(set) var generationTranscript: [String: [LocalAppTranscriptBlock]] = [:]
    /// Apps with an in-flight `llm.chat`. Drives the "calling AI" indicator;
    /// the engine emits this in on/off pairs.
    private(set) var llmActiveAppIDs: Set<String> = []
    /// Unread mailbox events per app, for a badge. The event carries no body
    /// on purpose — the payload is read by the assistant through MCP.
    private(set) var unreadAgentEvents: [String: Int] = [:]

    /// Append one live chunk, growing the trailing block when the kind is
    /// unchanged so the transcript reads as prose rather than as packets.
    private func appendTranscript(appId: String, kind: LocalAppTranscriptBlock.Kind, chunk: String) {
        var blocks = generationTranscript[appId] ?? []
        if var last = blocks.last, last.kind == kind {
            last.text += chunk
            blocks[blocks.count - 1] = last
        } else {
            blocks.append(LocalAppTranscriptBlock(id: blocks.count, kind: kind, text: chunk))
        }
        generationTranscript[appId] = blocks
    }

    /// Drop an app's transcript. Called when a NEW run starts, never when one
    /// ends: after a failure the last thing the model said is exactly what the
    /// user needs to read.
    func clearTranscript(appID: String) {
        generationTranscript[appID] = nil
    }
    private(set) var checkpoints: [String: [LocalAppCheckpoint]] = [:]
    private(set) var isRefreshing = false
    private(set) var errorMessage: String?
    private(set) var lastRefreshAt: Date?
    private(set) var createdAppIDForDesigner: String?
    private(set) var pendingPermission: LocalAppPermissionPrompt?
    private(set) var requestedPresentationAppID: String?
    private(set) var activeUIRequestAppID: String?
    /// Set alongside `requestedPresentationAppID` only for a preview gate
    /// armed by a live generation this session (see `appPreviewReady`'s use
    /// of `liveGenerationAppIDs`). `requestedPresentationAppID` itself is
    /// consumed synchronously by `RootView`'s `onChange` before a cold
    /// `LocalAppsRootView`'s `.task` ever runs, so it cannot be read there;
    /// this field survives until `consumePendingPreviewRouteAppID` reads it.
    private(set) var pendingPreviewRouteAppID: String?

    var searchQuery = ""

    @ObservationIgnored private var debounceTasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var pendingEdits: [String: [String: PendingEdit]] = [:]
    @ObservationIgnored private var inFlightEdits: [String: PendingEdit] = [:]
    @ObservationIgnored private var runningBeforeSuspension = Set<String>()
    @ObservationIgnored private var pendingCreation: PendingCreation?
    /// App ids with a generation actively reporting progress IN THIS
    /// PROCESS this session — inserted ONLY by `appGenerationProgress` and
    /// `appGenerationJobChanged`, never by `appDetailsChanged`'s mirroring
    /// of a snapshot's `generationJob` (review NEW-1, round 2).
    ///
    /// `generationProgress` (the `@Observable` map the UI renders progress
    /// from) has TWO producers: live stage reports, AND `appDetailsChanged`
    /// attaching whatever durable job the engine still has on disk for an
    /// app the user merely opened the detail screen for
    /// (`AppService::load_jobs`/`handle_get_app_details`, `host.rs:2891`) —
    /// an app parked at `awaiting_preview_confirmation` keeps a durable job
    /// in `AwaitingApproval` forever, so simply viewing its detail screen
    /// populated `generationProgress` for it even though nothing was
    /// running. Gating `appPreviewReady`'s navigation on `generationProgress`
    /// alone (the round-1 fix) was therefore still wrong: a bootstrap resync
    /// AFTER the user had merely looked at that app's details this session
    /// would incorrectly arm navigation. This set has exactly one producer
    /// (a truly live job) and is consumed (removed) the moment a
    /// `PreviewReady` for that app uses it — see `appPreviewReady`'s handler
    /// — so a LATER re-announcement of the same still-pending gate (e.g. a
    /// second `resync_pending_gates` from a project/provider switch that
    /// re-wires the engine source without recreating this store) no longer
    /// finds a stale "live" marker either.
    @ObservationIgnored private var liveGenerationAppIDs = Set<String>()
    /// Apps for which an `open_app_designer` has been sent and whose gate has
    /// not come back yet.
    ///
    /// `open_designer` is only legal from `collecting_spec`/`generation_failed`;
    /// the first one to land moves the app to `awaiting_spec_confirmation` and
    /// the engine rejects every later one outright. `openDesigner(appID:)` can
    /// still be invoked more than once as a view re-renders or re-appears
    /// (e.g. the designer's `prepare()` running again), so this marker still
    /// spans send → gate-arrival even though creation itself no longer opens
    /// the designer eagerly (local-apps#questionnaire, Task 13: a freshly
    /// created app starts in `authoring_questionnaire`, not `collecting_spec`,
    /// so an immediate open would just be rejected).
    ///
    /// A guard on `apps.first(where:)?.workflow` cannot catch this: the summary
    /// the view reads comes from the very `appsChanged` batch that triggered the
    /// open, so on the create path it is stale by construction and always reads
    /// `collecting_spec`. This marker instead spans send → gate-arrival, which
    /// is exactly the interval in which a second request is illegal.
    @ObservationIgnored private var designerOpenSent = Set<String>()
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
                    createdAppIDForDesigner = created.id
                    // A freshly created app starts in `authoring_questionnaire`,
                    // not `collecting_spec` — `open_designer` is only legal from
                    // the latter (and from `generation_failed`). This dictionary
                    // entry still fires the navigation to `.designer(appID)`
                    // (LocalAppsLibraryView.openCreatedAppIfNeeded) immediately,
                    // but LocalAppDesignerView.prepare() is what actually decides
                    // whether to call `openDesigner` — it now no-ops while the
                    // workflow is still `.generating`-mapped and retries itself
                    // via `.task(id: app?.workflow)` once the questionnaire
                    // arrives, instead of eagerly issuing a doomed command
                    // (caught in Task 13 review: this exact path fired on every
                    // app creation).
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
                // The gate is here, so a later caller can read the
                // interaction id instead of asking for one.
                designerOpenSent.remove(appId)

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
                    summary: patch.note ?? String(localized: "local_apps_suggestion_note"),
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
                errorMessage = String(localized: "local_apps_error_design_conflict")
                flushNextEdit(appID: appId)

            case let .appWorkflowChanged(appId, state, detail):
                let workflow = LocalAppsProtocolAdapter.workflow(state)
                updateApp(appID: appId) { app in
                    app.workflow = workflow
                    app.updatedAt = .now
                }
                if workflow == .generating {
                    // The engine's own ACCEPTANCE of `confirm_design` — not
                    // `confirmDesign()`'s `send()` returning `true`, which only
                    // means the command didn't throw on dispatch (review
                    // Important 1). `confirm_design` consumes the pending
                    // interaction server-side (`self.interactions.pending =
                    // None`, state.rs) whenever it lands here; `retry_generation`
                    // reaches `.generating` too and never needed one. Either
                    // way a cached `interactionID` is stale once the workflow
                    // is actually `.generating` — clear it here so
                    // `prepare()`'s top-level reentry guard is not wedged by an
                    // id that no longer corresponds to any live interaction.
                    designers[appId]?.interactionID = nil
                }
                if let detail, !detail.isEmpty { errorMessage = detail }

            case let .appGenerationProgress(appId, stage, percent, detail):
                // A live model chunk, not a pipeline stage. It carries no
                // percent and must NOT land in `generationProgress`: the UI
                // keeps showing "Generating 25%" underneath while the words
                // scroll past above it.
                if let kind = LocalAppTranscriptBlock.Kind(stage: stage) {
                    if let detail, !detail.isEmpty {
                        appendTranscript(appId: appId, kind: kind, chunk: detail)
                    }
                    liveGenerationAppIDs.insert(appId)
                    break
                }
                generationProgress[appId] = LocalAppGenerationProgress(
                    stage: stage,
                    percent: percent,
                    detail: detail
                )
                // A live stage report — see `liveGenerationAppIDs`'s doc
                // comment for why this is a DIFFERENT signal from the map
                // above.
                liveGenerationAppIDs.insert(appId)

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
                // Mirrors `appUiRequest`'s use of this same field below: the
                // preview confirm gate just armed for `appId`, so route the
                // app the same way an inbound UI-automation request already
                // does (`RootView`'s `onChange(of: requestedPresentationAppID)`
                // -> `navigation.openLocalApps`). Previously only
                // `appUiRequest` set this, so a user who confirmed a design
                // and was not already sitting on `.preview(appId)` (e.g. they
                // backed out of the local-apps cover, or `LocalAppDesignerView`
                // never navigates `path` on its own) had no way back to the
                // gate that just opened except manually re-tapping the app row
                // from the library (review F2).
                //
                // BUT `PreviewReady` is not a one-shot live event:
                // `AppService::resync_pending_gates` (service.rs) re-announces
                // the pending gate of every app at engine bootstrap, on the
                // documented assumption that clients treat gate announcements
                // idempotently. Unconditionally arming navigation here breaks
                // that assumption — an app that has sat at
                // `awaiting_preview_confirmation` for days gets its gate
                // replayed on every relaunch, hijacking the screen into the
                // local-apps cover for an app the user never touched this
                // session (review NEW-1). Two parked apps in the same
                // bootstrap batch each fire this, so the cover is presented,
                // its `path` reset, and re-presented.
                //
                // Gate on `liveGenerationAppIDs`, NOT `generationProgress`
                // (round-1 fix used the latter and was still wrong — see
                // `liveGenerationAppIDs`'s doc comment for the full story):
                // `generationProgress` also gets populated just by
                // `appDetailsChanged` mirroring a durable job the engine
                // still has on disk, which happens merely from opening an
                // app's detail screen — nothing live required. Removing
                // (not just reading) on match also closes the within-session
                // repeat: a SECOND `resync_pending_gates` later in the same
                // process (e.g. a project/provider switch that re-wires the
                // engine source without recreating this store) re-announces
                // the same still-pending gate, and must not re-arm just
                // because this app WAS live earlier this session.
                if liveGenerationAppIDs.remove(appId) != nil {
                    requestedPresentationAppID = appId
                    // Lets a COLD local-apps cover (its `.task` runs after
                    // `requestedPresentationAppID` has already been consumed
                    // by `RootView`'s synchronous `onChange`) land straight on
                    // `.preview(appId)` instead of one tap short at
                    // `.details(appId)` — see
                    // `LocalAppsLibraryView.swift`'s use of
                    // `consumePendingPreviewRouteAppID`.
                    pendingPreviewRouteAppID = appId
                }

            case let .appCheckpointCreated(appId, checkpoint):
                let item = LocalAppsProtocolAdapter.checkpoint(checkpoint)
                var values = checkpoints[appId] ?? []
                values.removeAll { $0.id == item.id }
                values.append(item)
                checkpoints[appId] = values.sorted { $0.createdAt > $1.createdAt }

            case let .appOperationFailed(appId, code, message):
                if let appId {
                    generationProgress[appId] = nil
                    liveGenerationAppIDs.remove(appId)
                }
                isRefreshing = false
                // A failed open never produces a gate, so nothing else would
                // clear the marker and the app could never be opened again.
                // `AppOperationFailed` carries no correlation id, so this
                // clears on any failure for the app rather than on the one
                // that was the open — releasing too eagerly costs at most one
                // redundant request; holding it forever wedges the designer.
                if let appId { designerOpenSent.remove(appId) }
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

    /// One-shot read of a preview gate armed this session for `appID` (see
    /// `pendingPreviewRouteAppID`'s doc comment). Consumes on match so a
    /// later cold-open of the same app doesn't spuriously skip to
    /// `.preview` again.
    func consumePendingPreviewRouteAppID(appID: String) -> Bool {
        guard pendingPreviewRouteAppID == appID else { return false }
        pendingPreviewRouteAppID = nil
        return true
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
    /// characters) when none is supplied. This replaces the old
    /// `createApp(name:template:)`, which sent the app's display NAME as its
    /// BRIEF — the exact fabrication the Rust side spent two tasks
    /// eliminating (local-apps#questionnaire, Task 11). A dedicated name
    /// input is Task 16's "创建入口" job; this method itself no longer
    /// fabricates anything.
    func createApp(brief: String) async -> Bool {
        let trimmed = brief.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            errorMessage = String(localized: "local_apps_error_brief_required")
            return false
        }
        #if canImport(engine_mobileFFI)
            pendingCreation = PendingCreation(knownAppIDs: Set(apps.map(\.id)))
            let succeeded = await send(
                .createApp(
                    name: "",
                    origin: .library,
                    brief: trimmed,
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

    /// Replaces an app's brief and re-authors its questionnaire from scratch,
    /// discarding any prior questionnaire/answers/plan. Valid from
    /// `collecting_spec`, `questionnaire_failed`, or `plan_failed`.
    func updateBrief(appID: String, brief: String) async -> Bool {
        let trimmed = brief.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            errorMessage = String(localized: "local_apps_error_brief_required")
            return false
        }
        #if canImport(engine_mobileFFI)
            return await send(.updateAppBrief(appId: appID, brief: trimmed))
        #else
            return false
        #endif
    }

    /// Retries questionnaire authoring after it failed, reusing the same brief.
    func retryQuestionnaire(appID: String) async {
        #if canImport(engine_mobileFFI)
            _ = await send(.retryAppQuestionnaire(appId: appID))
        #endif
    }

    /// Begins planning from the collected answers (`collecting_spec ->
    /// planning`). The engine validates the answers are self-consistent
    /// before starting the background plan round trip.
    func beginPlanning(appID: String) async -> Bool {
        #if canImport(engine_mobileFFI)
            return await send(.beginAppPlanning(appId: appID))
        #else
            return false
        #endif
    }

    /// Retries planning after it failed, reusing the same answers.
    func retryPlan(appID: String) async {
        #if canImport(engine_mobileFFI)
            _ = await send(.retryAppPlan(appId: appID))
        #endif
    }

    /// Opens the designer gate at most once per app until that gate arrives.
    ///
    /// A second `open_app_designer` between the first one landing and its
    /// `appDesignerRequested` coming back is rejected by the engine with
    /// `workflow state invalid`, because the first already moved the app to
    /// `awaiting_spec_confirmation`. The details refresh still runs for the
    /// caller that is turned away, so it is not left with an empty view — it
    /// simply stops asking for a transition someone else already made.
    func openDesigner(appID: String) async {
        #if canImport(engine_mobileFFI)
            let alreadySent = designerOpenSent.contains(appID)
            if !alreadySent { designerOpenSent.insert(appID) }
            _ = await send(.getAppDetails(appId: appID))
            guard !alreadySent else { return }
            let opened = await send(.openAppDesigner(appId: appID))
            if !opened {
                // The command never reached the engine, so no gate is coming
                // and nothing else will clear the marker.
                designerOpenSent.remove(appID)
            }
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
            errorMessage = String(localized: "local_apps_error_design_loading")
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
            errorMessage = String(localized: "local_apps_error_design_not_ready")
            return false
        }
        #if canImport(engine_mobileFFI)
            guard await drainPendingEdits(appID: appID) else {
                // Commands are delivered in order, so a patch still in flight
                // reaches the engine BEFORE this confirm and moves the revision
                // under it — the confirm could only fail with a revision
                // conflict. Keep the user on the designer with the answer queued.
                errorMessage = String(localized: "local_apps_error_design_unsaved")
                return false
            }
            // NOTE: `send()` returning `true` means the FFI call didn't throw —
            // i.e. the command was *submitted*, not that the engine accepted
            // it. `confirm_design` can still be refused (`validate_pending`,
            // `ensure_current_revision`, the plan-freshness check), surfacing
            // only as a later `appOperationFailed`. The cached `interactionID`
            // must therefore NOT be cleared here on dispatch — see the
            // `appWorkflowChanged` handler below, which clears it only once
            // the workflow actually reaches `.generating` (review Important 1
            // on an earlier revision of this method, which cleared here and
            // could wedge `prepare()`'s reentry guard for the rest of the
            // session on a rejected confirm).
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
            errorMessage = String(localized: "local_apps_error_preview_not_ready")
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
            errorMessage = String(localized: "local_apps_error_feedback_required")
            return false
        }
        #if canImport(engine_mobileFFI)
            return await send(.requestAppRevision(appId: appID, prompt: trimmed))
        #else
            return false
        #endif
    }

    /// Retry a failed generation, optionally carrying the user's own words.
    ///
    /// `prompt` reaches the generator as the revision prompt, which is what
    /// lets a failure be talked out of rather than only re-run. `nil` replays
    /// the job unchanged and, deliberately, leaves any prompt the job already
    /// carried in place.
    @discardableResult
    func retryGeneration(appID: String, prompt: String? = nil) async -> Bool {
        #if canImport(engine_mobileFFI)
            return await send(.retryAppGeneration(appId: appID, prompt: prompt))
        #else
            return false
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
                // The full-details snapshot carries its own questionnaire/plan
                // (Task 6's bridge lowering), independent of the incremental
                // `appQuestionnaireChanged`/`appPlanChanged` events below — a
                // caller that only ever calls `getDetails` (e.g. on relaunch,
                // before any incremental event has arrived) must still see them.
                questionnaires[summary.id] = LocalAppsProtocolAdapter.questionnaire(details.questionnaire)
                plans[summary.id] = details.plan.map(LocalAppsProtocolAdapter.plan)
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

            case let .appQuestionnaireChanged(appId, _, steps):
                questionnaires[appId] = LocalAppsProtocolAdapter.questionnaire(steps)

            // An answer edit voids a previously-derived plan on the engine
            // side too (see `local-apps` `update_draft`), announced here with
            // `plan: nil` — mirror that by clearing the client's copy rather
            // than leaving a stale plan on screen.
            case let .appPlanChanged(appId, _, plan):
                plans[appId] = plan.map(LocalAppsProtocolAdapter.plan)

            case let .appGenerationJobChanged(job):
                updateGenerationJob(job)
                // Unlike `appDetailsChanged`'s call to the same helper just
                // above (a passive snapshot mirror), this event is only ever
                // emitted for a job actually progressing in this process —
                // see `liveGenerationAppIDs`'s doc comment. Do NOT hoist this
                // insert into `updateGenerationJob` itself: that would also
                // fire for the `appDetailsChanged` call site, which is
                // exactly the false-positive producer this set exists to
                // exclude.
                liveGenerationAppIDs.insert(job.appId)

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
                errorMessage = String(localized: "local_apps_error_field_unsupported \(edit.field.label)")
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
