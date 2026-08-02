import XCTest

@testable import LingxiCode

@MainActor
final class CronRepositoryTests: XCTestCase {
    func testRefreshLoadsGlobalAndProjectScopes() async throws {
        let global = CronScope(
            scopeID: globalCronScopeID,
            projectID: nil,
            projectName: "全局",
            projectCwd: nil,
            guestWorkspacePath: "/workspace/global"
        )
        let project = CronScope(
            scopeID: "project-a",
            projectID: "project-a",
            projectName: "项目 A",
            projectCwd: "/tmp/project-a",
            guestWorkspacePath: "/workspace/project-a"
        )
        let globalTask = CronTaskRecord(
            id: "global-task",
            cron: "0 9 * * *",
            prompt: "全局晨报",
            createdAtMs: 1,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 9_000,
            human: "每天 09:00"
        )
        let projectTask = CronTaskRecord(
            id: "project-task",
            cron: "0 10 * * *",
            prompt: "项目日报",
            createdAtMs: 2,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 10_000,
            human: "每天 10:00"
        )

        let provider = FakeStoreProvider(
            stores: [
                global.scopeID: FakeCronStore(tasks: [globalTask]),
                project.scopeID: FakeCronStore(tasks: [projectTask]),
            ]
        )
        let repository = makeRepository(
            scopes: [global, project],
            activeScopeID: project.scopeID,
            storeProvider: provider
        )

        await repository.refresh()

        XCTAssertEqual(repository.state.scopes.map(\.scopeID), [global.scopeID, project.scopeID])
        XCTAssertEqual(repository.state.activeScopeID, project.scopeID)
        XCTAssertEqual(repository.state.tasks.count, 2)
        XCTAssertEqual(Set(repository.state.tasks.map(\.scope.projectName)), ["全局", "项目 A"])
    }

    func testSchedulingPolicyUsesBestEffortBackgroundAndEarliestTask() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let store = FakeCronStore(tasks: [
            CronTaskRecord(
                id: "late",
                cron: "0 11 * * *",
                prompt: "晚一点",
                createdAtMs: 1,
                lastFiredAtMs: nil,
                recurring: true,
                nextFireMs: 11_000,
                human: "每天 11:00"
            ),
            CronTaskRecord(
                id: "early",
                cron: "0 8 * * *",
                prompt: "早一点",
                createdAtMs: 2,
                lastFiredAtMs: nil,
                recurring: true,
                nextFireMs: 8_000,
                human: "每天 08:00"
            ),
        ])
        let scheduler = FakeScheduler(mode: .bestEffortBackground, note: "系统调度不是精确闹钟")
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            scheduler: scheduler
        )

        await repository.refresh()

        XCTAssertEqual(repository.state.scheduling.mode, CronSchedulingMode.bestEffortBackground)
        XCTAssertEqual(repository.state.scheduling.nextEarliestAtMs, 8_000)
        let scheduledEarliestTimes = await scheduler.scheduledEarliestTimes
        XCTAssertEqual(scheduledEarliestTimes, [8_000])
    }

    func testReconcileDeduplicatesRepeatedDeliveryAndRecordsHistory() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-1",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let scheduler = FakeScheduler(mode: .bestEffortBackground, note: "系统调度不是精确闹钟")
        let notifier = FakeNotifier()
        let executor = FakeExecutor(
            dueResult: .success(
                CronExecutionOutcome(
                    status: .succeeded,
                    resultText: "完成",
                    errorMessage: nil,
                    errorKind: nil
                )
            )
        )
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            executor: executor,
            notifier: notifier,
            scheduler: scheduler,
            now: { 2_000 }
        )

        await repository.reconcile(reason: "test-pass-1")
        await repository.reconcile(reason: "test-pass-2")

        XCTAssertEqual(repository.state.history.count, 1)
        XCTAssertEqual(repository.state.history.first?.status, CronRunStatus.succeeded)
        let acknowledgedCount = await store.acknowledgedOccurrences.count
        let notificationCount = await notifier.payloads.count
        let dueRunCount = await executor.dueCalls.count
        XCTAssertEqual(acknowledgedCount, 1)
        XCTAssertEqual(notificationCount, 1)
        XCTAssertEqual(dueRunCount, 1)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 901_000)
    }

    func testConcurrentReconcileDoesNotRecoverActiveOccurrenceAsTimedOut() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-concurrent",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let executor = GatedCronExecutor()
        let notifier = FakeNotifier()
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        let first = Task { await repository.reconcile(reason: "concurrent-first") }
        await executor.waitUntilDueCallStarts()
        await repository.reconcile(reason: "concurrent-second")
        await executor.releaseDueCall()
        await first.value

        XCTAssertEqual(repository.state.history.count, 1)
        XCTAssertEqual(repository.state.history.first?.status, .succeeded)
        XCTAssertNil(repository.state.history.first?.errorKind)
        let dueCallCount = await executor.dueCallCount
        let acknowledgedCount = await store.acknowledgedOccurrences.count
        let notificationCount = await notifier.payloads.count
        XCTAssertEqual(dueCallCount, 1)
        XCTAssertEqual(acknowledgedCount, 1)
        XCTAssertEqual(notificationCount, 1)
    }

    func testCancellingReconcileDoesNotCommitOrAcknowledgeOccurrence() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-cancelled",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let executor = GatedCronExecutor()
        let notifier = FakeNotifier()
        let historyRoot = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("cron-tests-cancellation-\(UUID().uuidString)", isDirectory: true)
        let historyStore = CronRunHistoryStore(
            fileURL: historyRoot.appendingPathComponent("history.json", isDirectory: false),
            now: { 2_000 },
            newID: { "run-cancelled" }
        )
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            historyStore: historyStore,
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        let reconciliation = Task { await repository.reconcile(reason: "background-task") }
        await executor.waitUntilDueCallStarts()
        reconciliation.cancel()
        await executor.releaseDueCall()
        await reconciliation.value

        var dueCallCount = await executor.dueCallCount
        var acknowledgedCount = await store.acknowledgedOccurrences.count
        var notificationCount = await notifier.payloads.count
        XCTAssertEqual(dueCallCount, 1)
        XCTAssertEqual(acknowledgedCount, 0)
        XCTAssertEqual(notificationCount, 0)
        var persistedHistory = try await historyStore.records()
        XCTAssertEqual(persistedHistory.first?.status, .running)
        XCTAssertNil(persistedHistory.first?.errorKind)

        await repository.reconcile(reason: "recover-after-expiration")

        dueCallCount = await executor.dueCallCount
        acknowledgedCount = await store.acknowledgedOccurrences.count
        notificationCount = await notifier.payloads.count
        XCTAssertEqual(dueCallCount, 1)
        XCTAssertEqual(acknowledgedCount, 1)
        XCTAssertEqual(notificationCount, 0)
        XCTAssertEqual(repository.state.history.first?.status, .timedOut)
        XCTAssertEqual(repository.state.history.first?.errorKind, .timedOut)
        persistedHistory = try await historyStore.records()
        XCTAssertEqual(persistedHistory.first?.status, .timedOut)
        XCTAssertEqual(persistedHistory.first?.errorKind, .timedOut)
    }

    func testReconcileSelfHealsTerminalOccurrenceWithoutRerunOrRenotify() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-1",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let notifier = FakeNotifier()
        let executor = FakeExecutor()
        let historyRoot = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("cron-tests-history-\(UUID().uuidString)", isDirectory: true)
        let historyStore = CronRunHistoryStore(
            fileURL: historyRoot.appendingPathComponent("history.json", isDirectory: false),
            now: { 2_000 },
            newID: { "run-terminal" }
        )
        _ = try await historyStore.claim(
            scope: global,
            taskID: task.id,
            prompt: task.prompt,
            scheduledAtMs: 1_000,
            triggeredAtMs: 1_500,
            manual: false
        )
        _ = try await historyStore.markTerminal(
            runID: "run-terminal",
            status: .succeeded,
            resultText: "done",
            errorMessage: nil,
            errorKind: nil
        )

        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            historyStore: historyStore,
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        await repository.reconcile(reason: "recover-terminal")

        let dueCalls = await executor.dueCalls.count
        let notifications = await notifier.payloads.count
        let acknowledged = await store.acknowledgedOccurrences.count
        XCTAssertEqual(dueCalls, 0)
        XCTAssertEqual(notifications, 0)
        XCTAssertEqual(acknowledged, 1)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 901_000)
    }

    func testReconcileAckFailureRecoversOnNextPassWithoutRerunOrRenotify() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-1",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task], ackFailureCount: 1)
        let notifier = FakeNotifier()
        let executor = FakeExecutor(
            dueResult: .success(
                CronExecutionOutcome(
                    status: .succeeded,
                    resultText: "完成",
                    errorMessage: nil,
                    errorKind: nil
                )
            )
        )
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        await repository.reconcile(reason: "ack-fails-once")
        let firstPassDueCalls = await executor.dueCalls.count
        let firstPassNotifications = await notifier.payloads.count
        XCTAssertEqual(firstPassDueCalls, 1)
        XCTAssertEqual(firstPassNotifications, 0)
        XCTAssertNotNil(repository.state.errorMessage)

        await repository.reconcile(reason: "ack-recovers")

        let recoveredDueCalls = await executor.dueCalls.count
        let recoveredNotifications = await notifier.payloads.count
        let acknowledgedOccurrences = await store.acknowledgedOccurrences.count
        XCTAssertEqual(recoveredDueCalls, 1)
        XCTAssertEqual(recoveredNotifications, 0)
        XCTAssertEqual(acknowledgedOccurrences, 1)
        XCTAssertEqual(repository.state.history.count, 1)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 901_000)
    }

    func testReconcileRecoversStaleNonterminalOccurrenceAsTimedOutWithoutRerunOrRenotify() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-1",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let notifier = FakeNotifier()
        let executor = FakeExecutor()
        let historyRoot = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("cron-tests-stale-\(UUID().uuidString)", isDirectory: true)
        let historyStore = CronRunHistoryStore(
            fileURL: historyRoot.appendingPathComponent("history.json", isDirectory: false),
            now: { 2_000 },
            newID: { "run-stale" }
        )
        _ = try await historyStore.claim(
            scope: global,
            taskID: task.id,
            prompt: task.prompt,
            scheduledAtMs: 1_000,
            triggeredAtMs: 1_500,
            manual: false
        )

        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            historyStore: historyStore,
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        await repository.reconcile(reason: "recover-stale")

        let dueCalls = await executor.dueCalls.count
        let notifications = await notifier.payloads.count
        let acknowledged = await store.acknowledgedOccurrences.count
        XCTAssertEqual(dueCalls, 0)
        XCTAssertEqual(notifications, 0)
        XCTAssertEqual(acknowledged, 1)
        XCTAssertEqual(repository.state.history.first?.status, .timedOut)
        XCTAssertEqual(repository.state.history.first?.errorKind, .timedOut)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 901_000)
    }

    func testDiagnosticsSnapshotSummarizesIdentifierScopeAndCounts() {
        let scope = CronScope(
            scopeID: "project-a",
            projectID: "project-a",
            projectName: "项目 A",
            projectCwd: "/tmp/project-a",
            guestWorkspacePath: "/workspace/project-a"
        )
        let run = makeRun(
            runID: "success-1",
            taskID: "task-a",
            triggeredAtMs: 10_000,
            status: .succeeded,
            errorKind: nil
        )
        let active = CronRunRecord(
            runID: "running-1",
            taskID: "task-b",
            scopeID: scope.scopeID,
            projectID: scope.projectID,
            projectName: scope.projectName,
            prompt: "正在运行",
            scheduledAtMs: 11_000,
            triggeredAtMs: 11_000,
            startedAtMs: 11_000,
            finishedAtMs: nil,
            status: .running,
            attempt: 1,
            resultText: nil,
            errorMessage: nil,
            errorKind: nil,
            manual: false
        )
        let state = CronRepositoryState(
            loading: false,
            tasks: [
                CronScopedTask(
                    scope: scope,
                    task: CronTaskRecord(
                        id: "task-a",
                        cron: "0 9 * * *",
                        prompt: "日报",
                        createdAtMs: 1,
                        lastFiredAtMs: nil,
                        recurring: true,
                        nextFireMs: 12_000,
                        human: "每天 09:00"
                    ),
                    activeRun: nil,
                    lastRun: run
                )
            ],
            scopes: [scope],
            activeScopeID: scope.scopeID,
            history: [active, run],
            scheduling: CronSchedulingSnapshot(
                mode: .bestEffortBackground,
                nextEarliestAtMs: 12_000,
                lastReconciledAtMs: 13_000,
                note: "系统尽力调度",
                backgroundTaskIdentifier: cronBackgroundTaskIdentifier
            ),
            errorMessage: nil,
            lastActionMessage: nil
        )

        let diagnostics = state.diagnostics
        XCTAssertEqual(diagnostics.backgroundTaskIdentifier, cronBackgroundTaskIdentifier)
        XCTAssertEqual(diagnostics.schedulingModeTitle, CronSchedulingMode.bestEffortBackground.title)
        XCTAssertEqual(diagnostics.activeScopeName, "项目 A")
        XCTAssertEqual(diagnostics.activeScopeID, "project-a")
        XCTAssertEqual(diagnostics.taskCount, 1)
        XCTAssertEqual(diagnostics.historyCount, 2)
        XCTAssertEqual(diagnostics.activeRunCount, 1)
        XCTAssertEqual(diagnostics.activeRunSummary, "当前 1 个活动运行")
    }

    func testResultCategorySummaryGroupsLatestRunsByType() {
        let history = [
            makeRun(
                runID: "network-new",
                taskID: "task-1",
                triggeredAtMs: 20_000,
                status: .failed,
                errorKind: .network
            ),
            makeRun(
                runID: "network-old",
                taskID: "task-2",
                triggeredAtMs: 10_000,
                status: .failed,
                errorKind: .network
            ),
            makeRun(
                runID: "success-1",
                taskID: "task-3",
                triggeredAtMs: 15_000,
                status: .succeeded,
                errorKind: nil
            ),
            makeRun(
                runID: "credential-1",
                taskID: "task-4",
                triggeredAtMs: 12_000,
                status: .failed,
                errorKind: .missingCredentials
            ),
        ]
        let state = CronRepositoryState(
            loading: false,
            tasks: [],
            scopes: [],
            activeScopeID: globalCronScopeID,
            history: history,
            scheduling: .initial(),
            errorMessage: nil,
            lastActionMessage: nil
        )

        let summaries = state.resultCategorySummaries
        XCTAssertEqual(summaries.map(\.category), [.success, .missingCredentials, .network])
        XCTAssertEqual(summaries.first(where: { $0.category == .network })?.count, 2)
        XCTAssertEqual(summaries.first(where: { $0.category == .network })?.latestRun.runID, "network-new")
        XCTAssertEqual(summaries.first(where: { $0.category == .success })?.title, "成功")
    }

    func testRunResultCategoryMapsExistingErrorTypes() {
        XCTAssertEqual(makeRun(runID: "a", taskID: "1", triggeredAtMs: 1, status: .succeeded, errorKind: nil).resultCategory, .success)
        XCTAssertEqual(makeRun(runID: "b", taskID: "1", triggeredAtMs: 1, status: .failed, errorKind: .missingCredentials).resultCategory, .missingCredentials)
        XCTAssertEqual(makeRun(runID: "c", taskID: "1", triggeredAtMs: 1, status: .failed, errorKind: .network).resultCategory, .network)
        XCTAssertEqual(makeRun(runID: "d", taskID: "1", triggeredAtMs: 1, status: .timedOut, errorKind: .timedOut).resultCategory, .timedOut)
        XCTAssertEqual(makeRun(runID: "e", taskID: "1", triggeredAtMs: 1, status: .cancelled, errorKind: .cancelled).resultCategory, .cancelled)
        XCTAssertEqual(makeRun(runID: "f", taskID: "1", triggeredAtMs: 1, status: .skipped, errorKind: .system).resultCategory, .skipped)
    }

    func testRetainCronRunHistoryHonorsPerTaskAndTotalLimits() {
        let active = CronRunRecord(
            runID: "active",
            taskID: "task-a",
            scopeID: "global",
            projectID: nil,
            projectName: "全局",
            prompt: "running",
            scheduledAtMs: 10,
            triggeredAtMs: 10,
            startedAtMs: 10,
            finishedAtMs: nil,
            status: .running,
            attempt: 1,
            resultText: nil,
            errorMessage: nil,
            errorKind: nil,
            manual: false
        )
        let terminalSameTask = makeRuns(
            prefix: "same",
            count: 30,
            taskID: "task-a",
            status: .succeeded,
            triggeredStart: 1_000,
            errorKind: nil
        )
        let manyOthers = (0..<600).map { index in
            makeRun(
                runID: "other-\(index)",
                taskID: "task-\(index)",
                triggeredAtMs: UInt64(10_000 - index),
                status: .failed,
                errorKind: .system
            )
        }

        let retained = retainCronRunHistory([active] + terminalSameTask + manyOthers)

        XCTAssertLessThanOrEqual(retained.count, maxCronRunsTotal)
        XCTAssertEqual(retained.filter { $0.taskID == "task-a" }.count, maxCronRunsPerTask)
        XCTAssertTrue(retained.contains(where: { $0.runID == "active" }))
    }

    func testErrorClassificationSeparatesCredentialNetworkAndTimeout() {
        let repository = makeRepository()

        XCTAssertEqual(
            repository.classifyExecutionError(SimpleError("missing api key")).kind,
            CronRunErrorKind.missingCredentials
        )
        XCTAssertEqual(
            repository.classifyExecutionError(SimpleError("network connection lost")).kind,
            CronRunErrorKind.network
        )
        let timeout = repository.classifyExecutionError(SimpleError("request timed out"))
        XCTAssertEqual(timeout.kind, CronRunErrorKind.timedOut)
        XCTAssertEqual(timeout.statusOverride, CronRunStatus.timedOut)
    }

    private func makeRepository(
        scopes: [CronScope] = [CronScope.global(appSandboxRoot: NSTemporaryDirectory())],
        activeScopeID: String? = nil,
        storeProvider: any CronStoreProviding = FakeStoreProvider(stores: [:]),
        historyStore: CronRunHistoryStore? = nil,
        executor: any CronTaskExecuting = UnavailableCronExecutor(),
        notifier: any CronNotificationDelivering = NoopCronNotifier(),
        scheduler: any CronBackgroundScheduling = ForegroundOnlyCronScheduler(),
        backgroundTaskBridge: CronBackgroundTaskBridge = .shared,
        now: @escaping @Sendable () -> UInt64 = { 1_000 }
    ) -> CronRepository {
        let root = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("cron-tests-\(UUID().uuidString)", isDirectory: true)
        return CronRepository(
            appSandboxRoot: root.path,
            scopeProvider: FakeScopeProvider(scopes: scopes, activeScopeID: activeScopeID ?? scopes.first?.scopeID ?? globalCronScopeID),
            storeProvider: storeProvider,
            historyStore: historyStore ?? CronRunHistoryStore(
                fileURL: root.appendingPathComponent("history.json", isDirectory: false),
                now: now,
                newID: {
                    struct Counter {
                        static var value = 0
                    }
                    Counter.value += 1
                    return "run-\(Counter.value)"
                }
            ),
            executor: executor,
            notifier: notifier,
            scheduler: scheduler,
            backgroundTaskBridge: backgroundTaskBridge,
            now: now
        )
    }
}

private func makeRuns(
    prefix: String,
    count: Int,
    taskID: String,
    status: CronRunStatus,
    triggeredStart: UInt64,
    errorKind: CronRunErrorKind?
) -> [CronRunRecord] {
    (0..<count).map { index in
        makeRun(
            runID: "\(prefix)-\(index)",
            taskID: taskID,
            triggeredAtMs: triggeredStart - UInt64(index),
            status: status,
            errorKind: errorKind
        )
    }
}

private func makeRun(
    runID: String,
    taskID: String,
    triggeredAtMs: UInt64,
    status: CronRunStatus,
    errorKind: CronRunErrorKind?
) -> CronRunRecord {
    CronRunRecord(
        runID: runID,
        taskID: taskID,
        scopeID: "global",
        projectID: nil,
        projectName: "全局",
        prompt: "sample",
        scheduledAtMs: triggeredAtMs,
        triggeredAtMs: triggeredAtMs,
        startedAtMs: nil,
        finishedAtMs: triggeredAtMs,
        status: status,
        attempt: 1,
        resultText: status == .succeeded ? "ok" : nil,
        errorMessage: status == .failed ? "nope" : nil,
        errorKind: errorKind,
        manual: false
    )
}

private struct FakeScopeProvider: CronScopeProviding {
    let scopes: [CronScope]
    let activeScopeID: String

    func scopes(appSandboxRoot: String) async -> [CronScope] {
        scopes
    }

    func activeScopeID(for scopes: [CronScope]) async -> String {
        activeScopeID
    }
}

private actor FakeCronStore: CronStoreClient {
    private(set) var tasks: [CronTaskRecord]
    private var nextCreateIndex = 100
    private(set) var acknowledgedOccurrences: [CronOccurrence] = []
    private var ackFailureCount: Int

    init(tasks: [CronTaskRecord], ackFailureCount: Int = 0) {
        self.tasks = tasks
        self.ackFailureCount = ackFailureCount
    }

    func list() async throws -> [CronTaskRecord] {
        tasks.sorted { lhs, rhs in
            (lhs.nextFireMs ?? .max) < (rhs.nextFireMs ?? .max)
        }
    }

    func create(cronExpr: String, prompt: String, recurring: Bool) async throws -> CronTaskRecord {
        nextCreateIndex += 1
        let task = CronTaskRecord(
            id: "created-\(nextCreateIndex)",
            cron: cronExpr,
            prompt: prompt,
            createdAtMs: UInt64(nextCreateIndex),
            lastFiredAtMs: nil,
            recurring: recurring,
            nextFireMs: UInt64(nextCreateIndex * 1_000),
            human: cronExpr
        )
        tasks.append(task)
        return task
    }

    func update(taskID: String, cronExpr: String, prompt: String, recurring: Bool) async throws -> CronTaskRecord {
        guard let index = tasks.firstIndex(where: { $0.id == taskID }) else {
            throw SimpleError("missing task")
        }
        tasks[index].cron = cronExpr
        tasks[index].prompt = prompt
        tasks[index].recurring = recurring
        tasks[index].human = cronExpr
        return tasks[index]
    }

    func delete(taskID: String) async throws -> Bool {
        let original = tasks.count
        tasks.removeAll { $0.id == taskID }
        return tasks.count != original
    }

    func nextFireTime() async throws -> UInt64? {
        tasks.compactMap(\.nextFireMs).min()
    }

    func dueOccurrences(nowMs: UInt64) async throws -> [CronOccurrence] {
        tasks.compactMap { task in
            guard let next = task.nextFireMs, next <= nowMs else { return nil }
            return CronOccurrence(taskID: task.id, scheduledAtMs: next)
        }
    }

    func acknowledgeOccurrence(taskID: String, scheduledAtMs: UInt64) async throws -> Bool {
        guard let index = tasks.firstIndex(where: { $0.id == taskID }) else { return false }
        if ackFailureCount > 0 {
            ackFailureCount -= 1
            throw SimpleError("ack write failed")
        }
        acknowledgedOccurrences.append(CronOccurrence(taskID: taskID, scheduledAtMs: scheduledAtMs))
        tasks[index].lastFiredAtMs = scheduledAtMs
        if tasks[index].recurring {
            tasks[index].nextFireMs = scheduledAtMs + 900_000
        } else {
            tasks[index].nextFireMs = nil
        }
        return true
    }
}

private actor FakeStoreProvider: CronStoreProviding {
    private let stores: [String: FakeCronStore]

    init(stores: [String: FakeCronStore]) {
        self.stores = stores
    }

    func store(for scope: CronScope, appSandboxRoot: String) async throws -> any CronStoreClient {
        stores[scope.scopeID] ?? FakeCronStore(tasks: [])
    }
}

private actor FakeExecutor: CronTaskExecuting {
    private let manualResult: Result<CronExecutionOutcome, Error>
    private let dueResult: Result<CronExecutionOutcome?, Error>
    private(set) var dueCalls: [(String, UInt64)] = []

    init(
        manualResult: Result<CronExecutionOutcome, Error> = .success(
            CronExecutionOutcome(status: .succeeded, resultText: "ok", errorMessage: nil, errorKind: nil)
        ),
        dueResult: Result<CronExecutionOutcome?, Error> = .success(
            CronExecutionOutcome(status: .succeeded, resultText: "ok", errorMessage: nil, errorKind: nil)
        )
    ) {
        self.manualResult = manualResult
        self.dueResult = dueResult
    }

    func runTaskNow(scope: CronScope, task: CronTaskRecord) async throws -> CronExecutionOutcome {
        try manualResult.get()
    }

    func runTaskIfDue(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64) async throws -> CronExecutionOutcome? {
        dueCalls.append((task.id, scheduledAtMs))
        return try dueResult.get()
    }
}

private actor GatedCronExecutor: CronTaskExecuting {
    private var callStarted = false
    private var startWaiters: [CheckedContinuation<Void, Never>] = []
    private var released = false
    private var releaseWaiters: [CheckedContinuation<Void, Never>] = []
    private(set) var dueCallCount = 0

    func runTaskNow(scope: CronScope, task: CronTaskRecord) async throws -> CronExecutionOutcome {
        CronExecutionOutcome(status: .succeeded, resultText: "ok", errorMessage: nil, errorKind: nil)
    }

    func runTaskIfDue(
        scope: CronScope,
        task: CronTaskRecord,
        scheduledAtMs: UInt64
    ) async throws -> CronExecutionOutcome? {
        dueCallCount += 1
        callStarted = true
        let currentStartWaiters = startWaiters
        startWaiters.removeAll()
        currentStartWaiters.forEach { $0.resume() }
        if !released {
            await withCheckedContinuation { continuation in
                releaseWaiters.append(continuation)
            }
        }
        return CronExecutionOutcome(
            status: .succeeded,
            resultText: "完成",
            errorMessage: nil,
            errorKind: nil
        )
    }

    func waitUntilDueCallStarts() async {
        guard !callStarted else { return }
        await withCheckedContinuation { continuation in
            startWaiters.append(continuation)
        }
    }

    func releaseDueCall() {
        released = true
        let currentReleaseWaiters = releaseWaiters
        releaseWaiters.removeAll()
        currentReleaseWaiters.forEach { $0.resume() }
    }
}

private actor FakeNotifier: CronNotificationDelivering {
    private(set) var payloads: [CronNotificationPayload] = []

    func deliver(_ payload: CronNotificationPayload) async {
        payloads.append(payload)
    }
}

private actor FakeScheduler: CronBackgroundScheduling {
    let mode: CronSchedulingMode
    let note: String
    private(set) var scheduledEarliestTimes: [UInt64?] = []

    init(mode: CronSchedulingMode, note: String) {
        self.mode = mode
        self.note = note
    }

    func schedule(taskIdentifier: String, earliestAtMs: UInt64?) async throws {
        scheduledEarliestTimes.append(earliestAtMs)
    }

    func cancel(taskIdentifier: String) async {}
}

private struct SimpleError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}
