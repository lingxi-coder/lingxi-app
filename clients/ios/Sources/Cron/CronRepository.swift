import Foundation
import Observation

private struct ActiveCronOccurrenceKey: Hashable {
    let scopeID: String
    let taskID: String
    let scheduledAtMs: UInt64
}

/// Attempts one wake may spend on a single occurrence before parking it, so an
/// outage that outlives the background window does not consume the whole budget.
private let inProcessAttemptBudget = 3

/// Total attempts a scheduled occurrence gets across wakes before its transient
/// failure becomes terminal. Matches Android `MAX_EXECUTION_ATTEMPTS`.
private let maxScheduledExecutionAttempts = 5

@MainActor
@Observable
final class CronRepository {
    var state: CronRepositoryState
    var draft: CronTaskDraft
    var routePath: [CronRoute]
    var modelChoices: [String] = []
    var modelDetails: [String: ModelRuntimeDetails] = [:]
    var sessionChoices: [CronSessionChoice] = []
    var defaultModel: String?
    var defaultReasoning = CronReasoning()
    var onOpenSession: ((String?, String) -> Void)?
    private(set) var savedDraft = CronTaskDraft.create()
    var hasUnsavedChanges: Bool { draft != savedDraft }


    @ObservationIgnored private let appSandboxRoot: String
    @ObservationIgnored private let backgroundTaskIdentifier: String
    @ObservationIgnored private let scopeProvider: any CronScopeProviding
    @ObservationIgnored private let storeProvider: any CronStoreProviding
    @ObservationIgnored private let historyStore: CronRunHistoryStore
    @ObservationIgnored private let executor: any CronTaskExecuting
    @ObservationIgnored private let notifier: any CronNotificationDelivering
    @ObservationIgnored private let scheduler: any CronBackgroundScheduling
    @ObservationIgnored private let now: @Sendable () -> UInt64
    @ObservationIgnored private let backgroundTaskBridge: CronBackgroundTaskBridge
    @ObservationIgnored private var boundBackgroundHandler = false
    // Acquired before the first suspension in an occurrence run. Because the
    // repository is MainActor-isolated, this identifies work owned by this
    // process without blocking and disappears automatically on task exit.
    @ObservationIgnored private var activeDueOccurrences = Set<ActiveCronOccurrenceKey>()
    @ObservationIgnored private var activeManualRetries = Set<String>()
    @ObservationIgnored private var activeExecutionRunIDs = Set<String>()

    init(
        appSandboxRoot: String,
        backgroundTaskIdentifier: String = cronBackgroundTaskIdentifier,
        scopeProvider: any CronScopeProviding = DefaultCronScopeProvider(),
        storeProvider: any CronStoreProviding = JsonCronStoreProvider(),
        historyStore: CronRunHistoryStore? = nil,
        executor: any CronTaskExecuting = UnavailableCronExecutor(),
        notifier: any CronNotificationDelivering = NoopCronNotifier(),
        scheduler: any CronBackgroundScheduling = ForegroundOnlyCronScheduler(),
        backgroundTaskBridge: CronBackgroundTaskBridge = .shared,
        now: @escaping @Sendable () -> UInt64 = { UInt64(Date().timeIntervalSince1970 * 1000) }
    ) {
        self.appSandboxRoot = appSandboxRoot
        self.backgroundTaskIdentifier = backgroundTaskIdentifier
        self.scopeProvider = scopeProvider
        self.storeProvider = storeProvider
        self.historyStore = historyStore ?? CronRunHistoryStore(
            fileURL: URL(fileURLWithPath: appSandboxRoot, isDirectory: true)
                .appendingPathComponent("cron", isDirectory: true)
                .appendingPathComponent("history.json", isDirectory: false),
            now: now
        )
        self.executor = executor
        self.notifier = notifier
        self.scheduler = scheduler
        self.backgroundTaskBridge = backgroundTaskBridge
        self.now = now
        self.state = CronRepositoryState(
            loading: false,
            tasks: [],
            scopes: [],
            activeScopeID: globalCronScopeID,
            history: [],
            scheduling: .initial(
                mode: scheduler.mode,
                note: scheduler.note,
                backgroundTaskIdentifier: backgroundTaskIdentifier
            ),
            errorMessage: nil,
            lastActionMessage: nil
        )
        self.draft = .create()
        self.routePath = []
    }

    func handleLaunch() async {
        bindBackgroundTaskHandlerIfNeeded()
        await refresh()
        await reconcile(reason: "launch")
    }

    func handleSceneBecameActive() async {
        await reconcile(reason: "foreground")
    }

    /// Re-arm the OS wake from the current task file without executing anything.
    /// A task created in chat (the model's `CronCreate`) is only on disk; if the
    /// user then backgrounds the app, this is the last chance to hand its next
    /// fire time to `BGTaskScheduler` before the process is suspended.
    func handleSceneDidEnterBackground() async {
        await loadState(lastReconciledAtMs: state.scheduling.lastReconciledAtMs)
    }

    func handleBackgroundWake() async {
        bindBackgroundTaskHandlerIfNeeded()
        await reconcile(reason: "background-task")
    }

    func refresh() async {
        await loadState(lastReconciledAtMs: state.scheduling.lastReconciledAtMs)
    }

    func beginCreate(scopeID: String? = nil) {
        let target = scopeID ?? state.activeScopeID
        draft = .create(scopeID: target)
        draft.automation.model = defaultModel
        draft.automation.reasoning = defaultReasoning
        savedDraft = draft
        routePath.append(.task(scopeID: target, taskID: nil))
    }

    func beginEdit(scopeID: String, taskID: String) {
        guard let scopedTask = state.tasks.first(where: { $0.scope.scopeID == scopeID && $0.task.id == taskID }) else {
            return
        }
        draft = .edit(scopeID: scopeID, task: scopedTask.task)
        savedDraft = draft
        routePath.append(.task(scopeID: scopeID, taskID: taskID))
    }

    func openRun(_ runID: String) {
        routePath.append(.run(runID: runID))
    }

    func discardDraft() {
        draft = savedDraft
        popRoute()
    }

    func popRoute() {
        guard !routePath.isEmpty else { return }
        routePath.removeLast()
    }

    func resetRoutes() {
        routePath.removeAll()
    }

    func saveDraft() async {
        do {
            state.errorMessage = nil
            let scope = try requireScope(draft.scopeID)
            let store = try await storeProvider.store(for: scope, appSandboxRoot: appSandboxRoot)
            guard draft.automation.version == 2 else {
                throw CronExecutionError(kind: .validation, message: "Update the app before editing this task’s settings.", statusOverride: .failed)
            }
            guard let model = draft.automation.model, !model.isEmpty else {
                throw CronExecutionError(kind: .validation, message: "Choose a model before saving.", statusOverride: .failed)
            }
            if draft.automation.runMode == .selectedSession {
                guard let target = sessionChoices.first(where: { $0.id == draft.automation.targetSessionId && $0.scopeID == draft.scopeID }) else {
                    throw CronExecutionError(kind: .validation, message: "Choose an available chat in this project.", statusOverride: .failed)
                }
                draft.automation.targetSessionId = target.id
            } else { draft.automation.targetSessionId = nil }
            if draft.automation.status == .active { draft.automation.statusReason = nil }
            _ = try await store.saveConfigured(draft: draft)
            state.lastActionMessage = String(localized: "cron_task_updated")
            draft = .create(scopeID: scope.scopeID)
            savedDraft = draft
            resetRoutes()
            await loadState(lastReconciledAtMs: state.scheduling.lastReconciledAtMs)
        } catch {
            state.errorMessage = error.localizedDescription
        }
    }

    func pauseTasksForArchivedSession(projectID: String?, sessionID: String) async throws {
        let scopes = await normalizedScopes()
        guard let scope = scopes.first(where: { $0.projectID == projectID }) else { return }
        let store = try await storeProvider.store(for: scope, appSandboxRoot: appSandboxRoot)
        for task in try await store.list() {
            var configuration = task.configuration
            guard configuration.version == 2,
                  configuration.status != .completed,
                  configuration.targetSessionId == sessionID || configuration.ownedSessionId == sessionID else { continue }
            configuration.status = .paused
            configuration.statusReason = "The selected chat was archived. Choose another chat to resume."
            _ = try await store.updateAutomation(taskID: task.id, automation: configuration)
            _ = try await historyStore.cancelQueued(scopeID: scope.scopeID, taskID: task.id)
        }
        await refresh()
    }

    func setStatus(scopeID: String, taskID: String, status: CronTaskStatus) async {
        do {
            let scope = try requireScope(scopeID)
            let task = try await requireTask(scopeID: scopeID, taskID: taskID)
            // A record this build cannot decode is surfaced as a synthetic
            // version-0 stub (paused, no model, no runs). Writing that stub back
            // would permanently destroy the real configuration, so refuse it
            // here the way `saveDraft` already does.
            guard task.configuration.version == 2 else {
                throw CronExecutionError(kind: .validation, message: "Update the app before changing this task’s status.", statusOverride: .failed)
            }
            var config = task.configuration
            config.status = status
            config.statusReason = nil
            let store = try await storeProvider.store(for: scope, appSandboxRoot: appSandboxRoot)
            _ = try await store.updateAutomation(taskID: taskID, automation: config)
            if status != .active {
                _ = try await historyStore.cancelQueued(scopeID: scopeID, taskID: taskID)
            }
            await refresh()
        } catch { state.errorMessage = error.localizedDescription }
    }

    func beginCopy() {
        draft.taskID = nil
        draft.automation.status = .active
        draft.automation.statusReason = nil
        draft.automation.runs = []
        draft.automation.ownedSessionId = nil
        draft.automation.targetSessionId = nil
        draft.automation.runMode = .newSession
        savedDraft = .create(scopeID: draft.scopeID)
    }

    func openResultSession(_ run: CronRunRecord) {
        if let id = run.sessionID { onOpenSession?(run.projectID, id) }
    }

    func deleteTask(scopeID: String, taskID: String) async {
        do {
            state.errorMessage = nil
            let scope = try requireScope(scopeID)
            let store = try await storeProvider.store(for: scope, appSandboxRoot: appSandboxRoot)
            let deleted = try await store.delete(taskID: taskID)
            if deleted {
                _ = try await historyStore.cancelUnfinished(
                    scopeID: scopeID,
                    taskID: taskID,
                    message: String(localized: "cron_task_deleted_message")
                )
            }
            state.lastActionMessage = deleted ? String(localized: "cron_task_delete_success") : String(localized: "cron_task_delete_not_found")
            if draft.taskID == taskID {
                draft = .create(scopeID: state.activeScopeID)
                // Every other mutator keeps `savedDraft` in step. Leaving it on
                // the deleted task latches `hasUnsavedChanges`, which pins
                // `.interactiveDismissDisabled(true)` on the sheet and makes
                // Discard restore the task that was just deleted.
                savedDraft = draft
            }
            resetRoutes()
            await loadState(lastReconciledAtMs: state.scheduling.lastReconciledAtMs)
        } catch {
            state.errorMessage = error.localizedDescription
        }
    }

    func runNow(scopeID: String, taskID: String) async {
        do {
            try Task.checkCancellation()
            state.errorMessage = nil
            let scope = try requireScope(scopeID)
            let task = try await requireTask(scopeID: scopeID, taskID: taskID)
            guard task.status == .active else {
                throw CronExecutionError(kind: .validation, message: "Resume this task before running it.", statusOverride: .failed)
            }
            try Task.checkCancellation()
            guard let claimed = try await historyStore.claim(
                scope: scope,
                taskID: taskID,
                prompt: task.prompt,
                scheduledAtMs: now(),
                manual: true,
                notificationPolicy: task.configuration.notificationPolicy
            ) else {
                state.errorMessage = String(localized: "cron_task_already_running")
                return
            }
            activeManualRetries.insert(claimed.runID)
            defer { activeManualRetries.remove(claimed.runID) }
            let outcome: CronExecutionOutcome
            do {
                // Manual runs stay in-process: the user is watching, so a
                // transient failure is reported now rather than parked.
                outcome = try await executeWithRetry(runID: claimed.runID, attemptCap: inProcessAttemptBudget) {
                    try await self.executor.runTaskNow(scope: scope, task: task, scheduledAtMs: claimed.scheduledAtMs)
                }.outcome
            } catch is CancellationError {
                outcome = CronExecutionOutcome(
                    status: .cancelled,
                    resultText: nil,
                    errorMessage: String(localized: "cron_execution_cancelled"),
                    errorKind: .cancelled
                )
            }
            if outcome.status == .queued {
                _ = try await historyStore.markRetry(runID: claimed.runID, attempt: 1, message: outcome.errorMessage ?? "Waiting for chat")
                await refresh()
                return
            }
            let record = try await historyStore.markTerminal(
                runID: claimed.runID,
                status: outcome.status,
                resultText: outcome.resultText,
                errorMessage: outcome.errorMessage,
                errorKind: outcome.errorKind,
                sessionID: outcome.sessionID, actualModel: outcome.actualModel ?? task.configuration.model
            )
            if let record {
                try await notifyTerminalRun(record, fallbackPolicy: task.configuration.notificationPolicy)
            }
            state.lastActionMessage = String(localized: "cron_run_triggered")
            await loadState(lastReconciledAtMs: state.scheduling.lastReconciledAtMs)
        } catch is CancellationError {
            state.loading = false
        } catch {
            state.errorMessage = error.localizedDescription
            await loadState(lastReconciledAtMs: state.scheduling.lastReconciledAtMs)
        }
    }

    func reconcile(reason: String) async {
        bindBackgroundTaskHandlerIfNeeded()
        state.loading = true
        let lastReconciledAtMs = now()
        do {
            try Task.checkCancellation()
            let scopes = await normalizedScopes()
            try Task.checkCancellation()
            try await reconcilePersistedRuns(scopes: scopes)
            try await recoverTerminalNotifications()
            try await retryQueuedManualRuns(scopes: scopes)
            let due = try await loadDueOccurrences(scopes: scopes, nowMs: lastReconciledAtMs)
            for item in due {
                try Task.checkCancellation()
                try await executeDueOccurrence(item.scope, task: item.task, scheduledAtMs: item.scheduledAtMs)
            }
            try Task.checkCancellation()
            state.lastActionMessage = reason == "launch" ? String(localized: "cron_reconcile_complete") : nil
            await loadState(lastReconciledAtMs: lastReconciledAtMs)
        } catch is CancellationError {
            state.loading = false
        } catch {
            state.errorMessage = error.localizedDescription
            await loadState(lastReconciledAtMs: lastReconciledAtMs)
        }
    }

    func route(forRunID runID: String) -> CronRoute {
        .run(runID: runID)
    }

    func handleNotificationUserInfo(_ userInfo: [AnyHashable: Any]) {
        if userInfo["lingxi.route"] as? String == "cron.task",
           let scope = userInfo["lingxi.cron.scope_id"] as? String,
           let task = userInfo["lingxi.cron.task_id"] as? String {
            beginEdit(scopeID: scope, taskID: task)
            return
        }
        guard
            let route = userInfo["lingxi.route"] as? String,
            route == "cron.run",
            let runID = userInfo["lingxi.cron.run_id"] as? String
        else {
            return
        }
        routePath = [.run(runID: runID)]
    }

    var selectedRun: CronRunRecord? {
        guard case .run(let runID) = routePath.last else { return nil }
        return state.history.first(where: { $0.runID == runID })
    }

    private func bindBackgroundTaskHandlerIfNeeded() {
        guard !boundBackgroundHandler else { return }
        boundBackgroundHandler = true
        backgroundTaskBridge.bind(taskIdentifier: backgroundTaskIdentifier) { [weak self] in
            await self?.handleBackgroundWake()
        }
    }

    private func loadState(lastReconciledAtMs: UInt64?) async {
        do {
            guard !Task.isCancelled else {
                state.loading = false
                return
            }
            let scopes = await normalizedScopes()
            guard !Task.isCancelled else {
                state.loading = false
                return
            }
            let history = try await historyStore.records()
            guard !Task.isCancelled else {
                state.loading = false
                return
            }
            let tasks = try await loadTasks(scopes: scopes, history: history)
            try await notifyConfigurationPauses(tasks)
            guard !Task.isCancelled else {
                state.loading = false
                return
            }
            // Only schedules the engine will actually fire may arm the OS wake;
            // `dueOccurrences` filters unsupported tasks out, so waking for them
            // would burn a background slot and run nothing.
            let nextTaskFire = tasks
                .filter(\.task.mobileSupported)
                .compactMap(\.task.nextFireMs)
                .min()
            let queuedWake = history.contains { $0.status == .queued } ? now() + 15 * 60 * 1_000 : nil
            let nextFire = [nextTaskFire, queuedWake].compactMap { $0 }.min()
            try await scheduler.schedule(taskIdentifier: backgroundTaskIdentifier, earliestAtMs: nextFire)
            guard !Task.isCancelled else {
                state.loading = false
                return
            }
            let activeScopeID = await scopeProvider.activeScopeID(for: scopes)
            guard !Task.isCancelled else {
                state.loading = false
                return
            }
            state = CronRepositoryState(
                loading: false,
                tasks: tasks,
                scopes: scopes,
                activeScopeID: scopes.contains(where: { $0.scopeID == activeScopeID })
                    ? activeScopeID
                    : (scopes.first?.scopeID ?? globalCronScopeID),
                history: history,
                generatedSessions: try updateGeneratedSessions(history: history),
                scheduling: CronSchedulingSnapshot(
                    mode: scheduler.mode,
                    nextEarliestAtMs: nextFire,
                    lastReconciledAtMs: lastReconciledAtMs,
                    note: scheduler.note,
                    backgroundTaskIdentifier: backgroundTaskIdentifier
                ),
                errorMessage: state.errorMessage,
                lastActionMessage: state.lastActionMessage
            )
        } catch is CancellationError {
            state.loading = false
        } catch {
            state.loading = false
            state.errorMessage = error.localizedDescription
        }
    }

    private func notifyConfigurationPauses(_ tasks: [CronScopedTask]) async throws {
        let file = URL(fileURLWithPath: appSandboxRoot).appendingPathComponent("cron/pause-notifications.json")
        var delivered: [String: String] = [:]
        // 🚨 `try?`, not `try`: this is a bookkeeping cache, and it is read from
        // inside `loadState`'s `do`, whose only handler abandons the whole state
        // assignment. A truncated write or a restored backup here would leave
        // tasks, history and the scheduling snapshot permanently unset — the
        // entire Cron screen empty with only an error — and it would recur on
        // every refresh forever, because nothing deletes the bad file. An
        // unreadable cache means "we have not notified anything yet", which at
        // worst re-delivers one pause notice. (The Android mirror already reads
        // its index with `runCatching { … }.getOrNull()`.)
        if FileManager.default.fileExists(atPath: file.path),
           let data = try? Data(contentsOf: file),
           let decoded = try? JSONDecoder().decode([String: String].self, from: data) {
            delivered = decoded
        }
        let previous = delivered
        var notifications: [CronNotificationPayload] = []
        for item in tasks {
            let key = item.id
            let config = item.task.configuration
            guard config.status == .paused, let reason = config.statusReason else {
                delivered.removeValue(forKey: key)
                continue
            }
            guard delivered[key] != reason else { continue }
            delivered[key] = reason
            let reportedByRun = item.lastRun?.status == .failed && item.lastRun?.errorMessage?.contains(reason) == true
            if config.shouldNotify(.failed) && !reportedByRun {
                notifications.append(CronNotificationPayload(runID: "pause-" + key, scopeID: item.scope.scopeID,
                    taskID: item.task.id, title: "Task paused", body: reason, status: .failed, taskOnly: true))
            }
        }
        if delivered != previous {
            try DefaultProjectAtomicWriter().writeData(JSONEncoder().encode(delivered), to: file) { staged in
                _ = try JSONDecoder().decode([String: String].self, from: staged)
            }
        }
        for notification in notifications { await notifier.deliver(notification) }  // receipt is the `delivered` map above
    }

    private func updateGeneratedSessions(history: [CronRunRecord]) throws -> [CronGeneratedSession] {
        let file = URL(fileURLWithPath: appSandboxRoot).appendingPathComponent("cron/sessions.json")
        var rows: [CronGeneratedSession] = []
        // `try?` for the same reason as `notifyConfigurationPauses`: this call is
        // an ARGUMENT to the `CronRepositoryState(...)` initializer, so a throw
        // here means `state` is never assigned at all and every freshly loaded
        // task, run and scheduling value computed above it is discarded. The
        // rebuild below re-derives every row that still has a run in history, so
        // an unreadable cache costs at most the titles of sessions whose runs
        // have already been pruned.
        if FileManager.default.fileExists(atPath: file.path),
           let data = try? Data(contentsOf: file),
           let decoded = try? JSONDecoder().decode([CronGeneratedSession].self, from: data) {
            rows = decoded
        }
        // `uniqueKeysWithValues:` TRAPS on a duplicate key — it is a precondition
        // failure, not a thrown error, so the surrounding `do`/`catch` cannot
        // contain it. These rows come off disk, where an interrupted rename or a
        // restored backup can leave two entries sharing an id; last writer wins.
        var byID = Dictionary(rows.map { ($0.id, $0) }, uniquingKeysWith: { $1 })
        for run in history {
            guard let id = run.sessionID else { continue }
            let timestamp = run.finishedAtMs ?? run.triggeredAtMs
            if byID[id].map({ $0.updatedAtMs >= timestamp }) == true { continue }
            byID[id] = CronGeneratedSession(id: id, projectID: run.projectID, title: String(run.prompt.prefix(200)), updatedAtMs: timestamp)
        }
        let updated = byID.values.sorted { $0.updatedAtMs > $1.updatedAtMs }
        if rows != updated {
            try DefaultProjectAtomicWriter().writeData(JSONEncoder().encode(updated), to: file) { staged in
                _ = try JSONDecoder().decode([CronGeneratedSession].self, from: staged)
            }
        }
        return updated
    }

    private func normalizedScopes() async -> [CronScope] {
        let scopes = await scopeProvider.scopes(appSandboxRoot: appSandboxRoot)
        if scopes.isEmpty {
            return [CronScope.global(appSandboxRoot: appSandboxRoot)]
        }
        var seen = Set<String>()
        return scopes.filter { seen.insert($0.scopeID).inserted }
    }

    private func loadTasks(scopes: [CronScope], history: [CronRunRecord]) async throws -> [CronScopedTask] {
        let activeByTask = Dictionary(
            uniqueKeysWithValues: history
                .filter { !$0.status.isTerminal }
                .map { (keyFor(scopeID: $0.scopeID, taskID: $0.taskID), $0) }
        )
        var latestTerminal: [String: CronRunRecord] = [:]
        for run in history where run.status.isTerminal {
            let key = keyFor(scopeID: run.scopeID, taskID: run.taskID)
            if latestTerminal[key] == nil {
                latestTerminal[key] = run
            }
        }
        var scopedTasks: [CronScopedTask] = []
        for scope in scopes {
            let store = try await storeProvider.store(for: scope, appSandboxRoot: appSandboxRoot)
            try await store.migrateLegacyTasks(defaultModel: defaultModel, reasoning: defaultReasoning)
            let tasks = try await store.list()
            scopedTasks.append(contentsOf: tasks.map { task in
                let key = keyFor(scopeID: scope.scopeID, taskID: task.id)
                return CronScopedTask(
                    scope: scope,
                    task: task,
                    activeRun: activeByTask[key],
                    lastRun: latestTerminal[key]
                )
            })
        }
        return scopedTasks.sorted { lhs, rhs in
            if lhs.scope.projectName == rhs.scope.projectName {
                switch (lhs.task.nextFireMs, rhs.task.nextFireMs) {
                case let (l?, r?):
                    if l == r { return lhs.task.id < rhs.task.id }
                    return l < r
                case (.some, .none):
                    return true
                case (.none, .some):
                    return false
                case (.none, .none):
                    return lhs.task.id < rhs.task.id
                }
            }
            return lhs.scope.projectName < rhs.scope.projectName
        }
    }

    private func notifyTerminalRun(_ run: CronRunRecord, fallbackPolicy: CronNotificationPolicy?) async throws {
        guard let policy = run.notificationPolicy ?? fallbackPolicy, policy.shouldNotify(run.status),
              let claimed = try await historyStore.claimTerminalNotification(runID: run.runID) else { return }
        // 🚨 The claim is taken BEFORE delivery so a crash between the two cannot
        // double-notify — which means a refused delivery has to give the key
        // back. `UserNotificationCronNotifier` shows nothing when the
        // scheduled-run preference is off or authorization was denied, and
        // without this release the run is marked notified forever: turning the
        // toggle on or granting permission later re-enters
        // `recoverTerminalNotifications`, which hits EEXIST and gives up.
        if await notifier.deliver(notificationPayload(for: claimed)) == false {
            try await historyStore.releaseTerminalNotification(runID: run.runID)
        }
    }

    private func recoverTerminalNotifications() async throws {
        let terminal = try await historyStore.records().filter {
            $0.status.isTerminal && $0.notificationPolicy != nil && !ownsRun($0)
        }
        // Terminal rows written before policy snapshots cannot tell us whether
        // an alert was already delivered. Do not replay those historic alerts.
        // Legacy unfinished rows capture their policy before transitioning.
        for run in terminal {
            try Task.checkCancellation()
            try await notifyTerminalRun(run, fallbackPolicy: nil)
        }
    }

    private func ownsRun(_ run: CronRunRecord) -> Bool {
        activeExecutionRunIDs.contains(run.runID) || activeManualRetries.contains(run.runID) ||
            activeDueOccurrences.contains(ActiveCronOccurrenceKey(scopeID: run.scopeID, taskID: run.taskID, scheduledAtMs: run.scheduledAtMs))
    }

    private func reconcilePersistedRuns(scopes: [CronScope]) async throws {
        let unfinished = try await historyStore.records().filter { !$0.status.isTerminal }
        for scope in scopes {
            let candidates = unfinished.filter { $0.scopeID == scope.scopeID && !ownsRun($0) }
            guard !candidates.isEmpty else { continue }
            let store = try await storeProvider.store(for: scope, appSandboxRoot: appSandboxRoot)
            let tasks = try await store.list()
            for run in candidates {
                guard !ownsRun(run),
                      let task = tasks.first(where: { $0.id == run.taskID }), task.automation != nil else { continue }
                try await historyStore.snapshotNotificationPolicy(runID: run.runID, policy: task.configuration.notificationPolicy)
                var nativeRun = task.configuration.runs.last {
                    (run.manual ? ($0.manualOccurrenceAt ?? $0.scheduledAt) : $0.scheduledAt) == run.scheduledAtMs
                }
                // Pre-identity manual runs used a later native timestamp. Recover
                // only the sole started host run with one unambiguous legacy
                // terminal result; never infer an execution for a queued claim.
                if nativeRun == nil, run.manual, run.status == .running,
                   unfinished.filter({ $0.manual && $0.scopeID == run.scopeID && $0.taskID == run.taskID }).count == 1 {
                    let earliest = max(run.triggeredAtMs, run.startedAtMs ?? run.triggeredAtMs)
                    let legacy = task.configuration.runs.filter {
                        $0.id.hasPrefix("\(task.id)-manual-") &&
                            !$0.id.hasPrefix("\(task.id)-manual-at-") &&
                            $0.manualOccurrenceAt == nil && $0.status.isTerminal && $0.scheduledAt >= earliest
                    }
                    if legacy.count == 1 { nativeRun = legacy[0] }
                }
                if let nativeRun, nativeRun.status.isTerminal {
                    if let recovered = try await historyStore.markTerminal(runID: run.runID, status: nativeRun.status,
                        resultText: nativeRun.summary, errorMessage: nativeRun.error,
                        errorKind: nativeRun.status == .succeeded ? nil : .system,
                        sessionID: nativeRun.sessionId, actualModel: nativeRun.model) {
                        try await notifyTerminalRun(recovered, fallbackPolicy: task.configuration.notificationPolicy)
                    }
                } else if run.status == .queued && task.status != .active {
                    _ = try await historyStore.cancelQueued(scopeID: scope.scopeID, taskID: task.id)
                } else if run.status == .running || nativeRun?.status == .running {
                    if let interrupted = try await historyStore.markTerminal(runID: run.runID, status: .interrupted,
                        errorMessage: "The app stopped before this run’s outcome could be confirmed. It was not replayed.", errorKind: .system) {
                        try await notifyTerminalRun(interrupted, fallbackPolicy: task.configuration.notificationPolicy)
                    }
                    if !run.manual { _ = try await store.acknowledgeOccurrence(taskID: run.taskID, scheduledAtMs: run.scheduledAtMs) }
                }
            }
        }
    }

    private func retryQueuedManualRuns(scopes: [CronScope]) async throws {
        let queued = try await historyStore.records().filter { $0.manual && $0.status == .queued }
        for run in queued {
            guard let scope = scopes.first(where: { $0.scopeID == run.scopeID }),
                  activeManualRetries.insert(run.runID).inserted else { continue }
            defer { activeManualRetries.remove(run.runID) }
            let store = try await storeProvider.store(for: scope, appSandboxRoot: appSandboxRoot)
            guard let task = try await store.list().first(where: { $0.id == run.taskID }), task.status == .active else {
                _ = try await historyStore.cancelQueued(scopeID: run.scopeID, taskID: run.taskID)
                continue
            }
            try await historyStore.snapshotNotificationPolicy(runID: run.runID, policy: task.configuration.notificationPolicy)
            let outcome = try await executeWithRetry(runID: run.runID, attemptCap: 1) {
                try await self.executor.runTaskNow(scope: scope, task: task, scheduledAtMs: run.scheduledAtMs)
            }.outcome
            if outcome.status == .queued {
                _ = try await historyStore.markRetry(runID: run.runID, attempt: run.attempt + 1, message: outcome.errorMessage ?? "Waiting for chat")
                continue
            }
            if let finished = try await historyStore.markTerminal(runID: run.runID, status: outcome.status,
                resultText: outcome.resultText, errorMessage: outcome.errorMessage, errorKind: outcome.errorKind,
                sessionID: outcome.sessionID, actualModel: outcome.actualModel ?? task.configuration.model) {
                try await notifyTerminalRun(finished, fallbackPolicy: task.configuration.notificationPolicy)
            }
        }
    }

    private func loadDueOccurrences(scopes: [CronScope], nowMs: UInt64) async throws -> [(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64)] {
        var results: [(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64)] = []
        for scope in scopes {
            try Task.checkCancellation()
            let store = try await storeProvider.store(for: scope, appSandboxRoot: appSandboxRoot)
            try Task.checkCancellation()
            try await store.migrateLegacyTasks(defaultModel: defaultModel, reasoning: defaultReasoning)
            let tasks = try await store.list().reduce(into: [String: CronTaskRecord]()) { partialResult, task in
                partialResult[task.id] = task
            }
            try Task.checkCancellation()
            let due = try await store.dueOccurrences(nowMs: nowMs)
            try Task.checkCancellation()
            results.append(contentsOf: due.compactMap { occurrence in
                tasks[occurrence.taskID].map { task in
                    (scope: scope, task: task, scheduledAtMs: occurrence.scheduledAtMs)
                }
            })
        }
        return results.sorted { lhs, rhs in
            if lhs.scheduledAtMs == rhs.scheduledAtMs {
                return lhs.task.id < rhs.task.id
            }
            return lhs.scheduledAtMs < rhs.scheduledAtMs
        }
    }

    private func executeDueOccurrence(
        _ scope: CronScope,
        task: CronTaskRecord,
        scheduledAtMs: UInt64
    ) async throws {
        guard task.status == .active else { return }
        let occurrenceKey = ActiveCronOccurrenceKey(
            scopeID: scope.scopeID,
            taskID: task.id,
            scheduledAtMs: scheduledAtMs
        )
        guard activeDueOccurrences.insert(occurrenceKey).inserted else {
            return
        }
        defer {
            activeDueOccurrences.remove(occurrenceKey)
        }

        try Task.checkCancellation()
        let store = try await storeProvider.store(for: scope, appSandboxRoot: appSandboxRoot)
        try Task.checkCancellation()
        if let existing = try await historyStore.occurrence(
            scopeID: scope.scopeID,
            taskID: task.id,
            scheduledAtMs: scheduledAtMs,
            manual: false
        ) {
            try Task.checkCancellation()
            if existing.status.isTerminal {
                try Task.checkCancellation()
                _ = try await store.acknowledgeOccurrence(taskID: task.id, scheduledAtMs: scheduledAtMs)
            } else if existing.status == .queued {
                // Queued records have not started, including a claim persisted
                // immediately before the app stopped. Revalidate through the
                // executor and keep the same occurrence and history identity.
                try await runClaimedOccurrence(
                    scope,
                    task: task,
                    scheduledAtMs: scheduledAtMs,
                    store: store,
                    runID: existing.runID,
                    startAttempt: existing.attempt + 1
                )
            } else {
                try Task.checkCancellation()
                _ = try await historyStore.markTerminal(
                    runID: existing.runID,
                    status: .interrupted,
                    resultText: nil,
                    errorMessage: "The app stopped before this run’s outcome could be confirmed. It was not replayed.",
                    errorKind: .system
                )
                try Task.checkCancellation()
                _ = try await store.acknowledgeOccurrence(taskID: task.id, scheduledAtMs: scheduledAtMs)
            }
            return
        }
        try Task.checkCancellation()
        guard let claimed = try await historyStore.claim(
            scope: scope,
            taskID: task.id,
            prompt: task.prompt,
            scheduledAtMs: scheduledAtMs,
            triggeredAtMs: now(),
            manual: false,
            notificationPolicy: task.configuration.notificationPolicy
        ) else {
            return
        }
        try await runClaimedOccurrence(
            scope,
            task: task,
            scheduledAtMs: scheduledAtMs,
            store: store,
            runID: claimed.runID,
            startAttempt: 1
        )
    }

    /// Run (or resume) one claimed occurrence.
    ///
    /// A transient failure with attempts left is PARKED rather than finished:
    /// the run row keeps its attempt count and the occurrence is deliberately
    /// left unacknowledged, so the engine still reports it due and the next wake
    /// retries it. This mirrors Android, where the worker returns
    /// `Result.retry()` and only acknowledges once the attempts are exhausted —
    /// without it a single network blip at wake time silently consumed the
    /// occurrence and the task never ran.
    private func runClaimedOccurrence(
        _ scope: CronScope,
        task: CronTaskRecord,
        scheduledAtMs: UInt64,
        store: any CronStoreClient,
        runID: String,
        startAttempt: Int
    ) async throws {
        try Task.checkCancellation()
        try await historyStore.snapshotNotificationPolicy(runID: runID, policy: task.configuration.notificationPolicy)
        // The budget applies to v2 automations too. `migrateLegacyTasks` stamps
        // an `automation` onto every task on every `refresh()`, so gating this on
        // `task.automation == nil` made the cap 1 for everything and re-created
        // the exact failure the contract above names. Android runs the same
        // 5-attempt budget for v2 rows (`CronWorkers.kt` MAX_EXECUTION_ATTEMPTS).
        let attempted = try await executeWithRetry(
            runID: runID,
            startAttempt: startAttempt,
            attemptCap: maxScheduledExecutionAttempts
        ) {
            try await self.executor.runTaskIfDue(
                scope: scope,
                task: task,
                scheduledAtMs: scheduledAtMs
            ) ?? CronExecutionOutcome(
                status: .skipped,
                resultText: nil,
                errorMessage: String(localized: "cron_trigger_already_handled"),
                errorKind: .system
            )
        }
        try Task.checkCancellation()
        if attempted.outcome.status == .queued {
            _ = try await historyStore.markRetry(runID: runID, attempt: attempted.attempt, message: attempted.outcome.errorMessage ?? "Waiting for chat")
            return
        }
        if isTransient(attempted.outcome), attempted.attempt < maxScheduledExecutionAttempts {
            _ = try await historyStore.markRetry(
                runID: runID,
                attempt: attempted.attempt,
                message: attempted.outcome.errorMessage
                    ?? String(localized: "cron_execution_failed_default")
            )
            return
        }
        let record = try await historyStore.markTerminal(
            runID: runID,
            status: attempted.outcome.status,
            resultText: attempted.outcome.resultText,
            errorMessage: attempted.outcome.errorMessage,
            errorKind: attempted.outcome.errorKind,
            sessionID: attempted.outcome.sessionID, actualModel: attempted.outcome.actualModel ?? task.configuration.model
        )
        try Task.checkCancellation()
        _ = try await store.acknowledgeOccurrence(taskID: task.id, scheduledAtMs: scheduledAtMs)
        try Task.checkCancellation()
        if let record {
            try await notifyTerminalRun(record, fallbackPolicy: task.configuration.notificationPolicy)
        }
        try Task.checkCancellation()
    }

    private func requireScope(_ scopeID: String) throws -> CronScope {
        guard let scope = state.scopes.first(where: { $0.scopeID == scopeID }) else {
            throw CronExecutionError(
                kind: .validation,
                message: String(localized: "cron_workspace_unavailable"),
                statusOverride: .failed
            )
        }
        return scope
    }

    /// Only failures the product can distinguish as transient are retried.
    private func isTransient(_ outcome: CronExecutionOutcome) -> Bool {
        outcome.errorKind == .network || outcome.errorKind == .timedOut
    }

    /// Retry a transient failure in place, and report which attempt produced the
    /// returned outcome so the caller can park the run for a later wake.
    ///
    /// `attemptCap` is the run's TOTAL attempt budget across wakes (Android's
    /// `MAX_EXECUTION_ATTEMPTS`); at most [`inProcessAttemptBudget`] of them are
    /// spent in one wake, so a backgrounded app never burns the whole budget on
    /// one outage.
    private func executeWithRetry(
        runID: String,
        startAttempt: Int = 1,
        attemptCap: Int = inProcessAttemptBudget,
        operation: @escaping () async throws -> CronExecutionOutcome
    ) async throws -> (outcome: CronExecutionOutcome, attempt: Int) {
        activeExecutionRunIDs.insert(runID)
        defer { activeExecutionRunIDs.remove(runID) }
        var last = CronExecutionOutcome(
            status: .failed,
            resultText: nil,
            errorMessage: String(localized: "cron_execution_failed_default"),
            errorKind: .unknown
        )
        var attempt = max(1, startAttempt)
        var spentHere = 0
        while true {
            try Task.checkCancellation()
            _ = try await historyStore.markRunning(runID: runID, attempt: attempt)
            try Task.checkCancellation()
            do {
                last = try await operation()
                try Task.checkCancellation()
            } catch is CancellationError {
                throw CancellationError()
            } catch {
                try Task.checkCancellation()
                let classified = classifyExecutionError(error)
                last = CronExecutionOutcome(
                    status: classified.statusOverride ?? status(for: classified.kind),
                    resultText: nil,
                    errorMessage: classified.message,
                    errorKind: classified.kind
                )
            }
            spentHere += 1
            guard isTransient(last),
                  attempt < attemptCap,
                  spentHere < inProcessAttemptBudget
            else {
                return (last, attempt)
            }
            try await Task.sleep(for: .milliseconds(250 * spentHere))
            attempt += 1
        }
    }

    private func requireTask(scopeID: String, taskID: String) async throws -> CronTaskRecord {
        guard let task = state.tasks.first(where: { $0.scope.scopeID == scopeID && $0.task.id == taskID })?.task else {
            throw CronExecutionError(
                kind: .validation,
                message: String(localized: "cron_task_not_in_project"),
                statusOverride: .failed
            )
        }
        return task
    }

    private func notificationPayload(for record: CronRunRecord) -> CronNotificationPayload {
        let title: String
        switch record.status {
        case .succeeded:
            title = String(localized: "cron_notification_completed \(record.projectName)")
        case .timedOut:
            title = String(localized: "cron_notification_timed_out \(record.projectName)")
        case .cancelled:
            title = String(localized: "cron_notification_cancelled \(record.projectName)")
        case .failed, .interrupted:
            title = String(localized: "cron_notification_failed \(record.projectName)")
        case .skipped:
            title = String(localized: "cron_notification_skipped \(record.projectName)")
        case .queued, .running:
            title = String(localized: "cron_notification_status_update \(record.projectName)")
        }
        let resultPreview = record.resultText?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let errorPreview = record.errorMessage?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let promptPreview = record.prompt
            .split(whereSeparator: \.isNewline)
            .first
            .map(String.init)
        let body = resultPreview.map { String($0.prefix(120)) }
            ?? errorPreview.map { String($0.prefix(120)) }
            ?? promptPreview
            ?? record.taskID
        return CronNotificationPayload(
            runID: record.runID,
            scopeID: record.scopeID,
            taskID: record.taskID,
            title: title,
            body: body,
            status: record.status
        )
    }

    func classifyExecutionError(_ error: Error) -> CronExecutionError {
        if let cronError = error as? CronExecutionError {
            return cronError
        }
        let message = error.localizedDescription.isEmpty ? String(describing: error) : error.localizedDescription
        let lowercased = message.lowercased()
        if lowercased.contains("credential") ||
            lowercased.contains("api key") ||
            lowercased.contains("token") ||
            lowercased.contains("oauth") ||
            message.contains("凭据") ||
            message.contains("密钥")
        {
            return CronExecutionError(kind: .missingCredentials, message: message, statusOverride: .failed)
        }
        if lowercased.contains("timed out") ||
            lowercased.contains("timeout") ||
            message.contains("超时")
        {
            return CronExecutionError(kind: .timedOut, message: message, statusOverride: .timedOut)
        }
        if lowercased.contains("cancelled") ||
            lowercased.contains("canceled") ||
            message.contains("取消")
        {
            return CronExecutionError(kind: .cancelled, message: message, statusOverride: .cancelled)
        }
        if lowercased.contains("offline") ||
            lowercased.contains("network") ||
            lowercased.contains("dns") ||
            lowercased.contains("connection") ||
            message.contains("网络")
        {
            return CronExecutionError(kind: .network, message: message, statusOverride: .failed)
        }
        if lowercased.contains("invalid") ||
            lowercased.contains("cron") ||
            message.contains("参数") ||
            message.contains("表达式")
        {
            return CronExecutionError(kind: .validation, message: message, statusOverride: .failed)
        }
        return CronExecutionError(kind: .system, message: message, statusOverride: .failed)
    }

    private func status(for kind: CronRunErrorKind) -> CronRunStatus {
        switch kind {
        case .timedOut:
            return .timedOut
        case .cancelled:
            return .cancelled
        case .missingCredentials, .network, .validation, .system, .unknown:
            return .failed
        }
    }

    private func keyFor(scopeID: String, taskID: String) -> String {
        "\(scopeID)|\(taskID)"
    }

}
