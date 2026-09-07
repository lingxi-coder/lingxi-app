import Foundation
import Observation

private struct ActiveCronOccurrenceKey: Hashable {
    let scopeID: String
    let taskID: String
    let scheduledAtMs: UInt64
}

@MainActor
@Observable
final class CronRepository {
    var state: CronRepositoryState
    var draft: CronTaskDraft
    var routePath: [CronRoute]

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
        routePath.append(.task(scopeID: target, taskID: nil))
    }

    func beginEdit(scopeID: String, taskID: String) {
        guard let scopedTask = state.tasks.first(where: { $0.scope.scopeID == scopeID && $0.task.id == taskID }) else {
            return
        }
        draft = .edit(scopeID: scopeID, task: scopedTask.task)
        routePath.append(.task(scopeID: scopeID, taskID: taskID))
    }

    func openRun(_ runID: String) {
        routePath.append(.run(runID: runID))
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
            if let taskID = draft.taskID {
                _ = try await store.update(
                    taskID: taskID,
                    cronExpr: draft.cron,
                    prompt: draft.prompt,
                    recurring: draft.recurring
                )
                state.lastActionMessage = String(localized: "cron_task_updated")
            } else {
                _ = try await store.create(
                    cronExpr: draft.cron,
                    prompt: draft.prompt,
                    recurring: draft.recurring
                )
                state.lastActionMessage = String(localized: "cron_task_created")
            }
            draft = .create(scopeID: scope.scopeID)
            resetRoutes()
            await loadState(lastReconciledAtMs: state.scheduling.lastReconciledAtMs)
        } catch {
            state.errorMessage = error.localizedDescription
        }
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
            try Task.checkCancellation()
            guard let claimed = try await historyStore.claim(
                scope: scope,
                taskID: taskID,
                prompt: task.prompt,
                scheduledAtMs: now(),
                manual: true
            ) else {
                state.errorMessage = String(localized: "cron_task_already_running")
                return
            }
            let outcome: CronExecutionOutcome
            do {
                outcome = try await executeWithRetry(runID: claimed.runID) {
                    try await self.executor.runTaskNow(scope: scope, task: task)
                }
            } catch is CancellationError {
                outcome = CronExecutionOutcome(
                    status: .cancelled,
                    resultText: nil,
                    errorMessage: String(localized: "cron_execution_cancelled"),
                    errorKind: .cancelled
                )
            }
            let record = try await historyStore.markTerminal(
                runID: claimed.runID,
                status: outcome.status,
                resultText: outcome.resultText,
                errorMessage: outcome.errorMessage,
                errorKind: outcome.errorKind
            )
            if let record {
                await notifier.deliver(notificationPayload(for: record))
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
            guard !Task.isCancelled else {
                state.loading = false
                return
            }
            // Only schedules the engine will actually fire may arm the OS wake;
            // `dueOccurrences` filters unsupported tasks out, so waking for them
            // would burn a background slot and run nothing.
            let nextFire = tasks
                .filter(\.task.mobileSupported)
                .compactMap(\.task.nextFireMs)
                .min()
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
            if !scopes.contains(where: { $0.scopeID == draft.scopeID }) {
                draft.scopeID = state.activeScopeID
            }
        } catch is CancellationError {
            state.loading = false
        } catch {
            state.loading = false
            state.errorMessage = error.localizedDescription
        }
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

    private func loadDueOccurrences(scopes: [CronScope], nowMs: UInt64) async throws -> [(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64)] {
        var results: [(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64)] = []
        for scope in scopes {
            try Task.checkCancellation()
            let store = try await storeProvider.store(for: scope, appSandboxRoot: appSandboxRoot)
            try Task.checkCancellation()
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
            } else {
                try Task.checkCancellation()
                _ = try await historyStore.markTerminal(
                    runID: existing.runID,
                    status: .timedOut,
                    resultText: nil,
                    errorMessage: String(localized: "cron_recovered_timeout_message"),
                    errorKind: .timedOut
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
            manual: false
        ) else {
            return
        }
        try Task.checkCancellation()
        let outcome = try await executeWithRetry(runID: claimed.runID) {
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
        let record = try await historyStore.markTerminal(
            runID: claimed.runID,
            status: outcome.status,
            resultText: outcome.resultText,
            errorMessage: outcome.errorMessage,
            errorKind: outcome.errorKind
        )
        try Task.checkCancellation()
        _ = try await store.acknowledgeOccurrence(taskID: task.id, scheduledAtMs: scheduledAtMs)
        try Task.checkCancellation()
        if let record {
            await notifier.deliver(notificationPayload(for: record))
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

    /// Retry only failures the product can distinguish as transient. Every
    /// attempt updates the persisted run row, while the occurrence is
    /// acknowledged only after this loop reaches a terminal outcome.
    private func executeWithRetry(
        runID: String,
        operation: @escaping () async throws -> CronExecutionOutcome
    ) async throws -> CronExecutionOutcome {
        var last = CronExecutionOutcome(
            status: .failed,
            resultText: nil,
            errorMessage: String(localized: "cron_execution_failed_default"),
            errorKind: .unknown
        )
        for attempt in 1...3 {
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
            let transient = last.errorKind == .network || last.errorKind == .timedOut
            guard transient, attempt < 3 else { return last }
            try await Task.sleep(for: .milliseconds(250 * attempt))
        }
        return last
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
        case .failed:
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
