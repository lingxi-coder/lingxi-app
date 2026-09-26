import XCTest

@testable import LingxiCode

@MainActor
final class CronRepositoryTests: XCTestCase {
    func testConfiguredTaskPausesPersistsAndResumesInFuture() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(secondsFromGMT: 0)!
        let file = directory.appendingPathComponent("tasks.json")
        let store = JsonCronStoreClient(fileURL: file, now: { 1_700_000_000_000 }, makeID: { "configured" }, calendar: calendar)
        var draft = CronTaskDraft.create()
        draft.prompt = "Summarize changes"
        draft.automation.model = "provider/model"
        draft.automation.reasoning = CronReasoning(selection: "high")
        draft.automation.runMode = .taskSession
        draft.automation.notificationPolicy = .failed
        let created = try await store.saveConfigured(draft: draft)
        var config = created.configuration
        config.status = .paused
        _ = try await store.updateAutomation(taskID: created.id, automation: config)
        let reopened = JsonCronStoreClient(fileURL: file, now: { 1_700_100_000_000 }, makeID: { "other" }, calendar: calendar)
        let paused = try await reopened.list()
        XCTAssertEqual(paused.first?.configuration, config)
        let due = try await reopened.dueOccurrences(nowMs: 1_700_100_000_000)
        XCTAssertTrue(due.isEmpty)
        config.status = .active
        let resumed = try await reopened.updateAutomation(taskID: created.id, automation: config)
        XCTAssertGreaterThan(try XCTUnwrap(resumed.nextFireMs), 1_700_100_000_000)
        XCTAssertEqual(resumed.configuration.model, "provider/model")
        XCTAssertEqual(resumed.configuration.reasoning.selection, "high")
    }

    func testConfiguredOneShotRetainsCompletedRecord() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = JsonCronStoreClient(fileURL: directory.appendingPathComponent("tasks.json"), now: { 1_700_000_000_000 }, makeID: { "once" }, calendar: Calendar(identifier: .gregorian))
        var draft = CronTaskDraft.create()
        draft.prompt = "Once"
        draft.recurring = false
        draft.automation.model = "provider/model"
        let task = try await store.saveConfigured(draft: draft)
        _ = try await store.acknowledgeOccurrence(taskID: task.id, scheduledAtMs: XCTUnwrap(task.nextFireMs))
        let completed = try await store.list()
        XCTAssertEqual(completed.count, 1)
        XCTAssertEqual(completed.first?.status, .completed)
        XCTAssertNil(completed.first?.nextFireMs)
    }

    func testNotificationPolicyAndReasoningRoundTrip() throws {
        var config = CronAutomation()
        config.notificationPolicy = .failed
        config.reasoning = CronReasoning(selection: "budget:8192")
        let decoded = try JSONDecoder().decode(CronAutomation.self, from: JSONEncoder().encode(config))
        XCTAssertEqual(decoded.reasoning.selection, "budget:8192")
        XCTAssertFalse(decoded.shouldNotify(.succeeded))
        XCTAssertFalse(decoded.shouldNotify(.running))
        XCTAssertTrue(decoded.shouldNotify(.failed))
        XCTAssertTrue(decoded.shouldNotify(.interrupted))
        config.notificationPolicy = .none
        XCTAssertFalse(config.shouldNotify(.failed))
    }

    func testSaveValidationKeepsDraft() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let repository = CronRepository(appSandboxRoot: directory.path)
        await repository.refresh()
        repository.beginCreate()
        repository.draft.prompt = "Keep this prompt"
        await repository.saveDraft()
        XCTAssertEqual(repository.draft.prompt, "Keep this prompt")
        XCTAssertNotNil(repository.state.errorMessage)
        XCTAssertFalse(repository.routePath.isEmpty)
    }

    func testArchivingTargetPausesTaskAndCancelsOnlyQueuedRun() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let scope = CronScope.global(appSandboxRoot: directory.path)
        let provider = JsonCronStoreProvider()
        let store = try await provider.store(for: scope, appSandboxRoot: directory.path)
        var draft = CronTaskDraft.create()
        draft.prompt = "Follow this chat"
        draft.automation.model = "provider/model"
        draft.automation.runMode = .selectedSession
        draft.automation.targetSessionId = "selected-chat"
        let task = try await store.saveConfigured(draft: draft)
        let history = CronRunHistoryStore(fileURL: directory.appendingPathComponent("history.json"))
        let queued = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 100)
        let running = try await history.claim(scope: scope, taskID: "already-running", prompt: "Running", scheduledAtMs: 100)
        _ = try await history.markRunning(runID: XCTUnwrap(running?.runID), attempt: 1)
        let repository = CronRepository(appSandboxRoot: directory.path, storeProvider: provider, historyStore: history)
        await repository.refresh()
        try await repository.pauseTasksForArchivedSession(projectID: nil, sessionID: "selected-chat")
        let paused = try await store.list()
        let run = try await history.record(runID: XCTUnwrap(queued?.runID))
        XCTAssertEqual(paused.first?.status, .paused)
        XCTAssertTrue(paused.first?.configuration.statusReason?.contains("archived") == true)
        XCTAssertEqual(run?.status, .cancelled)
        let ongoing = try await history.record(runID: XCTUnwrap(running?.runID))
        XCTAssertEqual(ongoing?.status, .running)
    }

    func testScheduledScopeHasDedicatedPreferenceAndWorkspaceIdentity() {
        XCTAssertEqual(ConversationScope(workspaceKey: "scheduled"), .scheduled)
        XCTAssertNotEqual(ConversationScope.scheduled.preferenceScope, ConversationScope.global.preferenceScope)
        XCTAssertEqual(CronScope.global(appSandboxRoot: "/app").guestWorkspacePath, "/app/scheduled/workspace")
    }

    func testLegacyMigrationIsIdempotentAndPausesWithoutModel() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = JsonCronStoreClient(fileURL: directory.appendingPathComponent("tasks.json"), now: { 1_700_000_000_000 }, makeID: { "legacy" }, calendar: Calendar(identifier: .gregorian))
        let legacy = try await store.create(cronExpr: "0 9 * * *", prompt: "Legacy instructions", recurring: true)
        try await store.migrateLegacyTasks(defaultModel: nil, reasoning: CronReasoning())
        let first = try await store.list()
        XCTAssertEqual(first.first?.status, .paused)
        XCTAssertEqual(first.first?.id, legacy.id)
        XCTAssertEqual(first.first?.createdAtMs, legacy.createdAtMs)
        XCTAssertEqual(first.first?.prompt, legacy.prompt)
        try await store.migrateLegacyTasks(defaultModel: "provider/model", reasoning: CronReasoning(selection: "high"))
        let second = try await store.list()
        XCTAssertEqual(first, second)
    }

    func testRecoveryUsesNativeTerminalResultBeforeMarkingInterrupted() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let scope = CronScope.global(appSandboxRoot: directory.path)
        let provider = JsonCronStoreProvider()
        let store = try await provider.store(for: scope, appSandboxRoot: directory.path)
        var draft = CronTaskDraft.create()
        draft.prompt = "Completed before host stopped"
        draft.automation.model = "provider/model"
        let task = try await store.saveConfigured(draft: draft)
        var configuration = task.configuration
        configuration.status = .completed
        configuration.runs = [CronAutomationRun(id: "native-run", taskId: task.id, scheduledAt: 200,
            status: .succeeded, model: "provider/model", sessionId: "result-chat", summary: "Done")]
        _ = try await store.updateAutomation(taskID: task.id, automation: configuration)
        let history = CronRunHistoryStore(fileURL: directory.appendingPathComponent("history.json"))
        let pending = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 200)
        _ = try await history.markRunning(runID: XCTUnwrap(pending?.runID), attempt: 1)
        let notifier = FakeNotifier()
        let repository = CronRepository(appSandboxRoot: directory.path, storeProvider: provider, historyStore: history, notifier: notifier)
        await repository.reconcile(reason: "restart")
        XCTAssertEqual(repository.state.history.first?.status, .succeeded)
        XCTAssertEqual(repository.state.history.first?.sessionID, "result-chat")
        XCTAssertEqual(repository.state.generatedSessions.first?.id, "result-chat")
        await repository.reconcile(reason: "again")
        let notifications = await notifier.payloads
        XCTAssertEqual(notifications.count, 1)
    }

    #if canImport(harness_runtimeFFI)
    func testNativeConfiguredStoreRoundTripsAndHonorsPause() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let handle = try buildIosCronStore(appSandboxRoot: directory.path, projectCwd: nil)
        try await handle.setMigrationDefaults(model: "provider/model", reasoningJson: "{\"type\":\"automatic\"}")
        var configuration = CronAutomation()
        configuration.model = "provider/model"
        configuration.reasoning = CronReasoning(selection: "high")
        configuration.runMode = .taskSession
        let created = try await handle.createConfigured(cronExpr: "0 9 * * *", prompt: "Native round trip", recurring: true,
            automationJson: String(decoding: JSONEncoder().encode(configuration), as: UTF8.self))
        let decoded = try JSONDecoder().decode(CronAutomation.self, from: Data(XCTUnwrap(created.automationJson).utf8))
        XCTAssertEqual(decoded.model, configuration.model)
        XCTAssertEqual(decoded.reasoning, configuration.reasoning)
        XCTAssertEqual(decoded.runMode, .taskSession)
        configuration.status = .paused
        _ = try await handle.updateAutomation(id: created.id, automationJson: String(decoding: JSONEncoder().encode(configuration), as: UTF8.self))
        let due = await handle.dueOccurrences(nowMs: UInt64(Date().timeIntervalSince1970 * 1000) + 86_400_000)
        XCTAssertTrue(due.isEmpty)
        let listed = await handle.list()
        XCTAssertEqual(listed.count, 1)
        let deleted = await handle.delete(id: created.id)
        XCTAssertTrue(deleted)
    }
    #endif

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

    func testUnsupportedScheduleNeverArmsTheBackgroundWake() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let store = FakeCronStore(tasks: [
            CronTaskRecord(
                id: "too-frequent",
                cron: "*/5 * * * *",
                prompt: "每 5 分钟",
                createdAtMs: 1,
                lastFiredAtMs: nil,
                recurring: true,
                nextFireMs: 5_000,
                human: "每 5 分钟",
                unsupportedReason: "recurring tasks must be at least 15 minutes apart"
            ),
            CronTaskRecord(
                id: "supported",
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

        // The engine's `dueOccurrences` filters unsupported tasks out, so waking
        // the app for them would run nothing; the earliest wake must skip them.
        XCTAssertEqual(repository.state.scheduling.nextEarliestAtMs, 8_000)
        let scheduledEarliestTimes = await scheduler.scheduledEarliestTimes
        XCTAssertEqual(scheduledEarliestTimes, [8_000])
        XCTAssertEqual(repository.state.tasks.count, 2, "unsupported tasks stay listed so the user can fix them")
        XCTAssertFalse(repository.state.tasks.first { $0.task.id == "too-frequent" }!.task.mobileSupported)
    }

    func testBackgroundTransitionReArmsWakeWithoutExecuting() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let store = FakeCronStore(tasks: [
            CronTaskRecord(
                id: "due-now",
                cron: "* * * * *",
                prompt: "已到期",
                createdAtMs: 0,
                lastFiredAtMs: nil,
                recurring: true,
                nextFireMs: 1,
                human: "每分钟"
            ),
        ])
        let scheduler = FakeScheduler(mode: .bestEffortBackground, note: "系统调度不是精确闹钟")
        let executor = FakeExecutor(dueResult: .failure(SimpleError("must not execute while backgrounding")))
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            executor: executor,
            scheduler: scheduler
        )

        await repository.handleSceneDidEnterBackground()

        let scheduledEarliestTimes = await scheduler.scheduledEarliestTimes
        XCTAssertEqual(scheduledEarliestTimes, [1])
        let dueCalls = await executor.dueCalls
        XCTAssertEqual(dueCalls.count, 0, "backgrounding only hands the next fire to the OS")
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

    /// A transient failure at wake time must NOT consume the occurrence: the run
    /// is parked with its attempt count and the next wake retries it, the way
    /// Android's worker returns `Result.retry()`.
    func testTransientFailureParksTheOccurrenceAndTheNextWakeResumesIt() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-transient",
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
        let offline = CronExecutionError(kind: .network, message: "offline", statusOverride: nil)
        let executor = SequencedExecutor(
            dueResults: Array(repeating: .failure(offline), count: 3),
            fallback: .success(
                CronExecutionOutcome(status: .succeeded, resultText: "完成", errorMessage: nil, errorKind: nil)
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

        await repository.reconcile(reason: "wake-1")

        // Parked, not finished: no acknowledgement, no notification, and the run
        // keeps the attempts this wake spent.
        var acknowledged = await store.acknowledgedOccurrences.count
        var notified = await notifier.payloads.count
        var dueCalls = await executor.dueCallCount
        XCTAssertEqual(acknowledged, 0, "a retryable failure must not consume the occurrence")
        XCTAssertEqual(notified, 0)
        XCTAssertEqual(dueCalls, 3, "one wake spends its in-process attempt budget")
        XCTAssertEqual(repository.state.history.count, 1)
        XCTAssertEqual(repository.state.history.first?.status, CronRunStatus.queued)
        XCTAssertEqual(repository.state.history.first?.attempt, 3)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 1_000, "still due")

        await repository.reconcile(reason: "wake-2")

        acknowledged = await store.acknowledgedOccurrences.count
        notified = await notifier.payloads.count
        dueCalls = await executor.dueCallCount
        XCTAssertEqual(dueCalls, 4, "the next wake resumes the same run")
        XCTAssertEqual(repository.state.history.count, 1, "resumed, not re-claimed")
        XCTAssertEqual(repository.state.history.first?.status, CronRunStatus.succeeded)
        XCTAssertEqual(repository.state.history.first?.attempt, 4)
        XCTAssertEqual(acknowledged, 1)
        XCTAssertEqual(notified, 1)
    }

    /// Once the cross-wake attempt budget is spent the failure becomes terminal
    /// and the occurrence is acknowledged, so a permanently offline device does
    /// not retry the same fire forever (Android `MAX_EXECUTION_ATTEMPTS`).
    func testExhaustedRetriesAcknowledgeAndReportTheFailure() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-exhausted",
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
        let executor = SequencedExecutor(
            dueResults: [],
            fallback: .failure(CronExecutionError(kind: .network, message: "offline", statusOverride: nil))
        )
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        await repository.reconcile(reason: "wake-1")
        var acknowledged = await store.acknowledgedOccurrences.count
        XCTAssertEqual(acknowledged, 0)

        await repository.reconcile(reason: "wake-2")

        acknowledged = await store.acknowledgedOccurrences.count
        let notified = await notifier.payloads.count
        let dueCalls = await executor.dueCallCount
        XCTAssertEqual(dueCalls, 5, "five attempts across two wakes, then stop")
        XCTAssertEqual(repository.state.history.first?.status, CronRunStatus.failed)
        XCTAssertEqual(repository.state.history.first?.attempt, 5)
        XCTAssertEqual(acknowledged, 1, "the exhausted occurrence is consumed")
        XCTAssertEqual(notified, 1)

        await repository.reconcile(reason: "wake-3")
        let afterTerminal = await executor.dueCallCount
        XCTAssertEqual(afterTerminal, 5, "a terminal run is never retried")
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
        XCTAssertEqual(repository.state.history.first?.status, .interrupted)
        XCTAssertEqual(repository.state.history.first?.errorKind, .system)
        persistedHistory = try await historyStore.records()
        XCTAssertEqual(persistedHistory.first?.status, .interrupted)
        XCTAssertEqual(persistedHistory.first?.errorKind, .system)
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

    func testReconcileAckFailureRecoversMissingNotificationOnceWithoutRerun() async throws {
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
        XCTAssertEqual(recoveredNotifications, 1)
        XCTAssertEqual(acknowledgedOccurrences, 1)
        XCTAssertEqual(repository.state.history.count, 1)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 901_000)
        await repository.reconcile(reason: "again")
        let afterThirdPass = await notifier.payloads.count
        XCTAssertEqual(afterThirdPass, 1)
    }

    func testReconcileRecoversStartedOccurrenceWithoutRerunOrRenotify() async throws {
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

        _ = try await historyStore.markRunning(runID: "run-stale", attempt: 1)

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
        XCTAssertEqual(repository.state.history.first?.status, .interrupted)
        XCTAssertEqual(repository.state.history.first?.errorKind, .system)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 901_000)
    }

    func testRestartRetriesUnstartedClaimOnceForRecurringAndOneShotTasks() async throws {
        for recurring in [true, false] {
            let scope = CronScope.global(appSandboxRoot: "/tmp")
            var task = CronTaskRecord(id: "pending", cron: "*/15 * * * *", prompt: "Run once",
                createdAtMs: 0, lastFiredAtMs: nil, recurring: recurring, nextFireMs: 1_000, human: "Every 15 minutes")
            var configuration = CronAutomation()
            configuration.model = "provider/model"
            task.automation = configuration
            let store = FakeCronStore(tasks: [task])
            let executor = FakeExecutor()
            let notifier = FakeNotifier()
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: root) }
            let file = root.appendingPathComponent("history.json")
            let beforeRestart = CronRunHistoryStore(fileURL: file, now: { 1_500 }, newID: { "persisted-claim" })
            _ = try await beforeRestart.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000)
            let reopened = CronRunHistoryStore(fileURL: file, now: { 2_000 })
            let repository = makeRepository(scopes: [scope], storeProvider: FakeStoreProvider(stores: [scope.scopeID: store]),
                historyStore: reopened, executor: executor, notifier: notifier, now: { 2_000 })

            await repository.reconcile(reason: "restart")
            await repository.reconcile(reason: "again")

            let calls = await executor.dueCalls.count
            let acknowledgements = await store.acknowledgedOccurrences.count
            let notifications = await notifier.payloads.count
            XCTAssertEqual(calls, 1)
            XCTAssertEqual(acknowledgements, 1)
            XCTAssertEqual(notifications, 1)
            XCTAssertEqual(repository.state.history.count, 1)
            XCTAssertEqual(repository.state.history.first?.runID, "persisted-claim")
            XCTAssertEqual(repository.state.history.first?.status, .succeeded)
        }
    }

    func testRestartRetriesUnstartedManualClaimOnce() async throws {
        let scope = CronScope.global(appSandboxRoot: "/tmp")
        var task = CronTaskRecord(id: "manual", cron: "*/15 * * * *", prompt: "Manual",
            createdAtMs: 0, lastFiredAtMs: nil, recurring: true, nextFireMs: 900_000, human: "Every 15 minutes")
        var configuration = CronAutomation()
        configuration.model = "provider/model"
        task.automation = configuration
        let store = FakeCronStore(tasks: [task])
        let executor = FakeExecutor()
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let history = CronRunHistoryStore(fileURL: file, now: { 1_000 }, newID: { "manual-claim" })
        _ = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000, manual: true)
        let repository = makeRepository(scopes: [scope], storeProvider: FakeStoreProvider(stores: [scope.scopeID: store]),
            historyStore: CronRunHistoryStore(fileURL: file), executor: executor)
        await repository.reconcile(reason: "restart")
        await repository.reconcile(reason: "again")
        let calls = await executor.manualCalls.count
        let acknowledgements = await store.acknowledgedOccurrences.count
        XCTAssertEqual(calls, 1)
        XCTAssertEqual(acknowledgements, 0)
        XCTAssertEqual(repository.state.history.first?.runID, "manual-claim")
        XCTAssertEqual(repository.state.history.first?.status, .succeeded)
    }

    func testBusyScheduledAndManualRunsRemainQueuedThenRetryAfterRestart() async throws {
        for manual in [false, true] {
            let scope = CronScope.global(appSandboxRoot: "/tmp")
            var task = CronTaskRecord(id: "busy", cron: "*/15 * * * *", prompt: "Wait for chat",
                createdAtMs: 0, lastFiredAtMs: nil, recurring: true,
                nextFireMs: manual ? 900_000 : 1_000, human: "Every 15 minutes")
            var configuration = CronAutomation()
            configuration.model = "provider/model"
            task.automation = configuration
            let store = FakeCronStore(tasks: [task])
            let provider = FakeStoreProvider(stores: [scope.scopeID: store])
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: root) }
            let file = root.appendingPathComponent("history.json")
            let history = CronRunHistoryStore(fileURL: file, now: { 1_000 }, newID: { "busy-claim" })
            _ = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000, manual: manual)
            let busy = CronExecutionOutcome(status: .queued, resultText: nil, errorMessage: "Waiting for chat", errorKind: nil)
            let repository = makeRepository(scopes: [scope], storeProvider: provider, historyStore: history,
                executor: FakeExecutor(manualResult: .success(busy), dueResult: .success(busy)))
            await repository.reconcile(reason: "busy")
            let beforeRetry = await store.acknowledgedOccurrences.count
            XCTAssertEqual(beforeRetry, 0)
            XCTAssertEqual(repository.state.history.first?.status, .queued)
            XCTAssertEqual(repository.state.history.first?.attempt, 1)
            let executor = FakeExecutor()
            let restarted = makeRepository(scopes: [scope], storeProvider: provider,
                historyStore: CronRunHistoryStore(fileURL: file), executor: executor)
            await restarted.reconcile(reason: "restart")
            await restarted.reconcile(reason: "again")
            let manualCalls = await executor.manualCalls.count
            let timestamps = await executor.manualTimestamps
            if manual { XCTAssertEqual(timestamps, [1_000]) }
            let dueCalls = await executor.dueCalls.count
            let acknowledgements = await store.acknowledgedOccurrences.count
            XCTAssertEqual(manualCalls + dueCalls, 1)
            XCTAssertEqual(acknowledgements, manual ? 0 : 1)
            XCTAssertEqual(restarted.state.history.count, 1)
            XCTAssertEqual(restarted.state.history.first?.runID, "busy-claim")
            XCTAssertEqual(restarted.state.history.first?.status, .succeeded)
        }
    }

    func testInitialManualExecutionAndRestartRetryReceiveSameIdentity() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let scope = CronScope.global(appSandboxRoot: root.path)
        var config = CronAutomation()
        config.model = "provider/model"
        let task = CronTaskRecord(id: "manual", cron: "0 9 * * *", prompt: "Manual", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: 900_000, human: "Daily", automation: config)
        let provider = FakeStoreProvider(stores: [scope.scopeID: FakeCronStore(tasks: [task])])
        let historyFile = root.appendingPathComponent("history.json")
        let history = CronRunHistoryStore(fileURL: historyFile)
        let busy = FakeExecutor(manualResult: .success(CronExecutionOutcome(
            status: .queued, resultText: nil, errorMessage: "Waiting for chat", errorKind: nil)))
        let repository = makeRepository(scopes: [scope], storeProvider: provider, historyStore: history,
            executor: busy, now: { 1_000 })
        await repository.refresh()
        await repository.runNow(scopeID: scope.scopeID, taskID: task.id)
        let initial = await busy.manualTimestamps
        XCTAssertEqual(initial, [1_000])
        let success = FakeExecutor()
        let restarted = makeRepository(scopes: [scope], storeProvider: provider,
            historyStore: CronRunHistoryStore(fileURL: historyFile), executor: success, now: { 20_000 })
        await restarted.reconcile(reason: "restart")
        let retried = await success.manualTimestamps
        XCTAssertEqual(retried, initial)
        XCTAssertEqual(restarted.state.history.first?.status, .succeeded)
    }

    func testNotificationReservationIsExclusiveAcrossHistoryStoreInstances() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let first = CronRunHistoryStore(fileURL: file, newID: { "terminal" })
        _ = try await first.claim(scope: .global(appSandboxRoot: root.path), taskID: "task", prompt: "Done", scheduledAtMs: 1_000)
        _ = try await first.markTerminal(runID: "terminal", status: .succeeded)
        let second = CronRunHistoryStore(fileURL: file)
        async let claimA = first.claimTerminalNotification(runID: "terminal")
        async let claimB = second.claimTerminalNotification(runID: "terminal")
        let claims = try await [claimA, claimB]
        XCTAssertEqual(claims.compactMap { $0 }.count, 1)
        let reopened = CronRunHistoryStore(fileURL: file)
        let duplicate = try await reopened.claimTerminalNotification(runID: "terminal")
        XCTAssertNil(duplicate)
    }

    /// 🚨 The claim is written BEFORE delivery, so a refusal that keeps it makes
    /// the run permanently un-notifiable: turning the scheduled-run toggle on or
    /// granting permission later re-enters recovery, which hits EEXIST. Both the
    /// Android and desktop mirrors give the key back; this pins that iOS does.
    func testARefusedDeliveryReturnsTheNotificationClaimSoRecoveryCanRetry() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let store = CronRunHistoryStore(fileURL: file, newID: { "terminal" })
        _ = try await store.claim(scope: .global(appSandboxRoot: root.path), taskID: "task", prompt: "Done", scheduledAtMs: 1_000)
        _ = try await store.markTerminal(runID: "terminal", status: .succeeded)

        // Each claim is hoisted out of the assertion: XCTAssert* take a
        // non-async autoclosure, and every call here has a side effect, so the
        // order must stay exactly as written.
        let firstClaim = try await store.claimTerminalNotification(runID: "terminal")
        XCTAssertNotNil(firstClaim)
        let secondClaim = try await store.claimTerminalNotification(runID: "terminal")
        XCTAssertNil(secondClaim, "the claim is exclusive while it is held")
        try await store.releaseTerminalNotification(runID: "terminal")
        let reclaimed = try await store.claimTerminalNotification(runID: "terminal")
        XCTAssertNotNil(reclaimed, "a released claim must be reclaimable")
    }

    func testNotificationReceiptsArePrunedWithHistoryAndStaleStoresCannotReclaim() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let scope = CronScope.global(appSandboxRoot: root.path)
        let stale = CronRunHistoryStore(fileURL: file)
        var firstID = ""
        for index in 0..<25 {
            let history = CronRunHistoryStore(fileURL: file)
            let run = try await history.claim(scope: scope, taskID: "task", prompt: "Done",
                scheduledAtMs: UInt64(index), triggeredAtMs: UInt64(index))
            let id = try XCTUnwrap(run?.runID)
            if index == 0 { firstID = id }
            _ = try await history.markTerminal(runID: id, status: .succeeded)
            let receipt = try await history.claimTerminalNotification(runID: id)
            XCTAssertNotNil(receipt)
        }
        let markers = file.appendingPathExtension("notification-claims")
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: markers.path).count, maxCronRunsPerTask)
        // An instance created before pruning must neither resurrect history nor
        // reacquire the deleted receipt when recovery still holds an old ID.
        let prunedID = firstID
        async let staleWrite = stale.markTerminal(runID: prunedID, status: .succeeded)
        let reopened = CronRunHistoryStore(fileURL: file)
        async let staleClaim = reopened.claimTerminalNotification(runID: prunedID)
        let (write, claim) = try await (staleWrite, staleClaim)
        XCTAssertNil(write)
        XCTAssertNil(claim)
        let retained = try await reopened.records()
        XCTAssertEqual(retained.count, maxCronRunsPerTask)
        for record in retained {
            let duplicate = try await reopened.claimTerminalNotification(runID: record.runID)
            XCTAssertNil(duplicate)
        }
        // Legacy orphan receipts are reclaimed even without a new history write.
        let orphan = markers.appendingPathComponent(String(repeating: "0", count: 64))
        try Data().write(to: orphan)
        _ = try await reopened.claimTerminalNotification(runID: "missing")
        XCTAssertFalse(FileManager.default.fileExists(atPath: orphan.path))
    }

    func testConcurrentHistoryWritersPreserveAllNotificationReceipts() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let scope = CronScope.global(appSandboxRoot: root.path)
        try await withThrowingTaskGroup(of: Void.self) { group in
            for index in 0..<20 {
                group.addTask {
                    let store = CronRunHistoryStore(fileURL: file)
                    guard let run = try await store.claim(scope: scope, taskID: "task-\(index)",
                        prompt: "Done", scheduledAtMs: 1_000) else {
                        XCTFail("Independent task claim was lost")
                        return
                    }
                    _ = try await store.markTerminal(runID: run.runID, status: .succeeded)
                    let receipt = try await store.claimTerminalNotification(runID: run.runID)
                    XCTAssertNotNil(receipt)
                }
            }
            try await group.waitForAll()
        }
        let reopened = CronRunHistoryStore(fileURL: file)
        let retained = try await reopened.records()
        XCTAssertEqual(retained.count, 20)
        for run in retained {
            XCTAssertEqual(run.status, .succeeded)
            let duplicate = try await reopened.claimTerminalNotification(runID: run.runID)
            XCTAssertNil(duplicate)
        }
    }

    func testManualRecoveryUsesPersistedExecutionTimestamp() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let scope = CronScope.global(appSandboxRoot: root.path)
        let provider = JsonCronStoreProvider()
        let store = try await provider.store(for: scope, appSandboxRoot: root.path)
        var draft = CronTaskDraft.create()
        draft.prompt = "Manual recovery"
        draft.automation.model = "provider/model"
        let task = try await store.saveConfigured(draft: draft)
        var config = task.configuration
        config.runs = [CronAutomationRun(id: "native-manual", taskId: task.id, scheduledAt: 1_000,
            startedAt: 1_010, finishedAt: 1_020, status: .succeeded, model: "provider/model",
            sessionId: "actual-chat", summary: "Actual success")]
        _ = try await store.updateAutomation(taskID: task.id, automation: config)
        let history = CronRunHistoryStore(fileURL: root.appendingPathComponent("history.json"), newID: { "host-manual" })
        _ = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000, manual: true)
        _ = try await history.markRunning(runID: "host-manual", attempt: 1)
        let repository = CronRepository(appSandboxRoot: root.path, storeProvider: provider, historyStore: history)
        await repository.reconcile(reason: "restart")
        XCTAssertEqual(repository.state.history.first?.status, .succeeded)
        XCTAssertEqual(repository.state.history.first?.sessionID, "actual-chat")
    }

    func testLegacyAdoptedManualRecoveryUsesHostOccurrenceMarker() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let scope = CronScope.global(appSandboxRoot: root.path)
        let provider = JsonCronStoreProvider()
        let store = try await provider.store(for: scope, appSandboxRoot: root.path)
        var draft = CronTaskDraft.create()
        draft.prompt = "Manual recovery"
        draft.automation.model = "provider/model"
        let task = try await store.saveConfigured(draft: draft)
        var config = task.configuration
        config.runs = [CronAutomationRun(id: "native-manual", taskId: task.id, scheduledAt: 1_010,
            startedAt: 1_011, finishedAt: 1_020, status: .succeeded, model: "provider/model",
            sessionId: "actual-chat", summary: "Actual success", manualOccurrenceAt: 1_000)]
        _ = try await store.updateAutomation(taskID: task.id, automation: config)
        let history = CronRunHistoryStore(fileURL: root.appendingPathComponent("history.json"), newID: { "host-manual" })
        _ = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000, manual: true)
        _ = try await history.markRunning(runID: "host-manual", attempt: 1)
        let repository = CronRepository(appSandboxRoot: root.path, storeProvider: provider, historyStore: history)
        await repository.reconcile(reason: "restart")
        XCTAssertEqual(repository.state.history.first?.status, .succeeded)
        XCTAssertEqual(repository.state.history.first?.sessionID, "actual-chat")
    }

    func testPreIdentityManualRecoveryRequiresOneUnambiguousTerminal() async throws {
        for ambiguous in [false, true] {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: root) }
            let scope = CronScope.global(appSandboxRoot: root.path)
            let provider = JsonCronStoreProvider()
            let store = try await provider.store(for: scope, appSandboxRoot: root.path)
            var draft = CronTaskDraft.create()
            draft.prompt = "Legacy manual recovery"
            draft.automation.model = "provider/model"
            let task = try await store.saveConfigured(draft: draft)
            var config = task.configuration
            config.runs = [CronAutomationRun(id: "\(task.id)-manual-1010-0", taskId: task.id,
                scheduledAt: 1_010, startedAt: 1_011, finishedAt: 1_020, status: .succeeded,
                model: "provider/model", sessionId: "actual-chat", summary: "Actual success")]
            if ambiguous {
                config.runs.append(CronAutomationRun(id: "\(task.id)-manual-1020-1", taskId: task.id,
                    scheduledAt: 1_020, status: .succeeded, model: "provider/model", sessionId: "other-chat"))
            }
            _ = try await store.updateAutomation(taskID: task.id, automation: config)
            let history = CronRunHistoryStore(fileURL: root.appendingPathComponent("history.json"),
                now: { 1_000 }, newID: { "host-manual" })
            _ = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000, manual: true)
            _ = try await history.markRunning(runID: "host-manual", attempt: 1)
            let executor = FakeExecutor()
            let repository = CronRepository(appSandboxRoot: root.path, storeProvider: provider,
                historyStore: history, executor: executor)
            await repository.reconcile(reason: "upgrade")
            await repository.reconcile(reason: "again")
            XCTAssertEqual(repository.state.history.first?.status, ambiguous ? .interrupted : .succeeded)
            XCTAssertEqual(repository.state.history.first?.sessionID, ambiguous ? nil : "actual-chat")
            let calls = await executor.manualCalls.count
            XCTAssertEqual(calls, 0, "Recovery must never replay a started run")
        }
    }

    func testConcurrentRecoveryClaimsNotificationOnceAndPersistsReservation() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let scope = CronScope.global(appSandboxRoot: root.path)
        var config = CronAutomation()
        config.model = "provider/model"
        config.status = .completed
        config.runs = [CronAutomationRun(id: "native", taskId: "task", scheduledAt: 1_000,
            status: .succeeded, model: "provider/model", sessionId: "actual-chat", summary: "Done")]
        let task = CronTaskRecord(id: "task", cron: "0 9 * * *", prompt: "Recovery", createdAtMs: 0,
            lastFiredAtMs: 1_000, recurring: false, nextFireMs: nil, human: "Daily", automation: config)
        let historyFile = root.appendingPathComponent("history.json")
        let history = CronRunHistoryStore(fileURL: historyFile, newID: { "same-host-run" })
        _ = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000)
        _ = try await history.markRunning(runID: "same-host-run", attempt: 1)
        let notifier = FakeNotifier()
        let repository = makeRepository(scopes: [scope], storeProvider: FakeStoreProvider(
            stores: [scope.scopeID: FakeCronStore(tasks: [task], recoveryBarrier: true)]),
            historyStore: history, notifier: notifier)
        async let first: Void = repository.reconcile(reason: "foreground")
        async let second: Void = repository.reconcile(reason: "chat-idle")
        _ = await (first, second)
        let notifications = await notifier.payloads
        XCTAssertEqual(notifications.count, 1)
        let reopened = CronRunHistoryStore(fileURL: historyFile)
        let duplicate = try await reopened.claimTerminalNotification(runID: "same-host-run")
        XCTAssertNil(duplicate)
        let saved = try await reopened.record(runID: "same-host-run")
        XCTAssertEqual(saved?.status, .succeeded)
    }

    func testLegacyPolicyMigrationCapturesUnfinishedOnlyAndNeverOverwritesSnapshot() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let scope = CronScope.global(appSandboxRoot: root.path)
        let history = CronRunHistoryStore(fileURL: file, newID: { "legacy-pending" })
        _ = try await history.claim(scope: scope, taskID: "pending", prompt: "Pending", scheduledAtMs: 1_000)
        try await history.snapshotNotificationPolicy(runID: "legacy-pending", policy: .none)
        let reopened = CronRunHistoryStore(fileURL: file, newID: { "legacy-terminal" })
        try await reopened.snapshotNotificationPolicy(runID: "legacy-pending", policy: .all)
        _ = try await reopened.markTerminal(runID: "legacy-pending", status: .failed)
        let migrated = try await reopened.record(runID: "legacy-pending")
        XCTAssertEqual(migrated?.notificationPolicy, CronNotificationPolicy.none)
        _ = try await reopened.claim(scope: scope, taskID: "terminal", prompt: "Already notified", scheduledAtMs: 1_000)
        _ = try await reopened.markTerminal(runID: "legacy-terminal", status: .succeeded)
        try await reopened.snapshotNotificationPolicy(runID: "legacy-terminal", policy: .all)
        let historical = try await reopened.record(runID: "legacy-terminal")
        XCTAssertNil(historical?.notificationPolicy)
    }

    func testCompletedRunNotificationRecoveryUsesPersistedPolicyWithoutDueOccurrence() async throws {
        let cases: [(CronNotificationPolicy, CronRunStatus, Int)] = [
            (.all, .succeeded, 1), (.failed, .failed, 1), (.failed, .succeeded, 0), (.none, .failed, 0),
        ]
        for (savedPolicy, status, expected) in cases {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: root) }
            let scope = CronScope.global(appSandboxRoot: root.path)
            var config = CronAutomation()
            config.model = "provider/model"
            config.status = .completed
            config.notificationPolicy = savedPolicy == .all ? .none : .all
            let task = CronTaskRecord(id: "one-shot", cron: "0 9 * * *", prompt: "Completed",
                createdAtMs: 0, lastFiredAtMs: 1_000, recurring: false, nextFireMs: nil, human: "Once", automation: config)
            let file = root.appendingPathComponent("history.json")
            let oldHistory = CronRunHistoryStore(fileURL: file, newID: { "run" })
            _ = try await oldHistory.claim(scope: scope, taskID: task.id, prompt: task.prompt,
                scheduledAtMs: 1_000, notificationPolicy: savedPolicy)
            _ = try await oldHistory.markTerminal(runID: "run", status: status)
            let notifier = FakeNotifier()
            let executor = FakeExecutor()
            let repository = makeRepository(scopes: [scope], storeProvider: FakeStoreProvider(
                stores: [scope.scopeID: FakeCronStore(tasks: [task])]),
                historyStore: CronRunHistoryStore(fileURL: file), executor: executor, notifier: notifier)
            await repository.reconcile(reason: "restart")
            await repository.reconcile(reason: "again")
            let count = await notifier.payloads.count
            let executions = await executor.dueCalls.count
            XCTAssertEqual(count, expected)
            XCTAssertEqual(executions, 0)
            XCTAssertEqual(repository.state.history.first?.notificationPolicy, savedPolicy)
        }
    }

    func testCancellationAfterTerminalWriteRecoversNotificationOnRestart() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let scope = CronScope.global(appSandboxRoot: root.path)
        var config = CronAutomation()
        config.model = "provider/model"
        let task = CronTaskRecord(id: "one-shot", cron: "0 9 * * *", prompt: "Run once",
            createdAtMs: 0, lastFiredAtMs: nil, recurring: false, nextFireMs: 1_000, human: "Once", automation: config)
        let store = FakeCronStore(tasks: [task], cancelAfterAcknowledgement: true)
        let provider = FakeStoreProvider(stores: [scope.scopeID: store])
        let file = root.appendingPathComponent("history.json")
        let notifier = FakeNotifier()
        let executor = FakeExecutor()
        let repository = makeRepository(scopes: [scope], storeProvider: provider,
            historyStore: CronRunHistoryStore(fileURL: file), executor: executor, notifier: notifier, now: { 2_000 })
        let firstWake = Task { await repository.reconcile(reason: "background") }
        await firstWake.value
        let beforeRecovery = await notifier.payloads.count
        XCTAssertEqual(beforeRecovery, 0)
        let reopened = CronRunHistoryStore(fileURL: file)
        let saved = try await reopened.records()
        XCTAssertEqual(saved.first?.status, .succeeded)
        let restarted = makeRepository(scopes: [scope], storeProvider: provider, historyStore: reopened,
            executor: executor, notifier: notifier, now: { 3_000 })
        await restarted.reconcile(reason: "restart")
        await restarted.reconcile(reason: "again")
        let recovered = await notifier.payloads.count
        let executions = await executor.dueCalls.count
        XCTAssertEqual(recovered, 1)
        XCTAssertEqual(executions, 1)
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
    private let recoveryBarrier: Bool
    private let cancelAfterAcknowledgement: Bool
    private var listCalls = 0
    private var waitingList: CheckedContinuation<Void, Never>?

    init(tasks: [CronTaskRecord], ackFailureCount: Int = 0, recoveryBarrier: Bool = false, cancelAfterAcknowledgement: Bool = false) {
        self.tasks = tasks
        self.ackFailureCount = ackFailureCount
        self.recoveryBarrier = recoveryBarrier
        self.cancelAfterAcknowledgement = cancelAfterAcknowledgement
    }

    func list() async throws -> [CronTaskRecord] {
        listCalls += 1
        if recoveryBarrier && listCalls == 1 {
            await withCheckedContinuation { waitingList = $0 }
        } else if recoveryBarrier && listCalls == 2 {
            waitingList?.resume()
            waitingList = nil
        }
        return tasks.sorted { lhs, rhs in
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
        if cancelAfterAcknowledgement { withUnsafeCurrentTask { $0?.cancel() } }
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

/// An executor whose scheduled-run results are scripted per call, so a test can
/// fail transiently a few times and then succeed.
private actor SequencedExecutor: CronTaskExecuting {
    private var dueResults: [Result<CronExecutionOutcome?, Error>]
    private let fallback: Result<CronExecutionOutcome?, Error>
    private(set) var dueCallCount = 0

    init(
        dueResults: [Result<CronExecutionOutcome?, Error>],
        fallback: Result<CronExecutionOutcome?, Error>
    ) {
        self.dueResults = dueResults
        self.fallback = fallback
    }

    func runTaskNow(scope: CronScope, task: CronTaskRecord) async throws -> CronExecutionOutcome {
        throw SimpleError("manual run not scripted")
    }

    func runTaskIfDue(
        scope: CronScope,
        task: CronTaskRecord,
        scheduledAtMs: UInt64
    ) async throws -> CronExecutionOutcome? {
        dueCallCount += 1
        let next = dueResults.isEmpty ? fallback : dueResults.removeFirst()
        return try next.get()
    }
}

private actor FakeExecutor: CronTaskExecuting {
    private let manualResult: Result<CronExecutionOutcome, Error>
    private let dueResult: Result<CronExecutionOutcome?, Error>
    private(set) var dueCalls: [(String, UInt64)] = []
    private(set) var manualCalls: [String] = []
    private(set) var manualTimestamps: [UInt64] = []

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
        manualCalls.append(task.id)
        return try manualResult.get()
    }

    func runTaskNow(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64) async throws -> CronExecutionOutcome {
        manualTimestamps.append(scheduledAtMs)
        return try await runTaskNow(scope: scope, task: task)
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
    /// Set to model the real notifier's silent refusals (preference off,
    /// authorization denied), which must give the delivery claim back.
    var refuses = false

    func setRefuses(_ value: Bool) { refuses = value }

    func deliver(_ payload: CronNotificationPayload) async -> Bool {
        payloads.append(payload)
        return !refuses
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
