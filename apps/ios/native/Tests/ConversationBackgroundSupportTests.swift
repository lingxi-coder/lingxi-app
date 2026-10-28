import SwiftUI
import XCTest

@testable import LingxiCode

@MainActor
final class ConversationBackgroundSupportTests: XCTestCase {
    func testConversationNotificationRouteReopensExactConversation() {
        let userInfo = ConversationNotificationRoute.userInfo(
            sessionID: "session-a",
            turnID: 42,
            workspaceKey: "project.weather",
            sessionMode: .chat,
            eventClass: .completed
        )

        XCTAssertEqual(
            ConversationNotificationRoute.appAction(from: userInfo),
            .openConversation(
                sessionID: "session-a",
                turnID: 42,
                workspaceKey: "project.weather",
                mode: .chat
            )
        )
    }

    func testForegroundSyncSuppressesNotifications() async {
        let scheduler = RecordingConversationScheduler()
        let controller = ConversationBackgroundAlertController(scheduler: scheduler)
        controller.setScenePhase(.active)

        controller.sync(ConversationBackgroundSnapshot(
            sessionID: "session-a",
            turnToken: ConversationTurnToken(clientTurnId: 7, sessionEpoch: 1),
            turnCompletion: ConversationTurnCompletion(
                token: ConversationTurnToken(clientTurnId: 7, sessionEpoch: 1),
                outcome: .completed,
                finalAssistantText: "done"
            ),
            pendingQuestions: [],
            backgroundTasks: [],
            requiresExecutionLease: false
        ))
        await drainAsyncNotifications()

        let payloads = await scheduler.snapshot()
        XCTAssertTrue(payloads.isEmpty)
    }

    func testBackgroundWaitingNotificationIsDeduplicated() async {
        let scheduler = RecordingConversationScheduler()
        let controller = ConversationBackgroundAlertController(scheduler: scheduler)
        controller.setScenePhase(.background)
        let question = ConversationPendingQuestion(requestId: 9, questions: [], timeoutSecs: nil)

        let snapshot = ConversationBackgroundSnapshot(
            sessionID: "session-a",
            turnToken: ConversationTurnToken(clientTurnId: 7, sessionEpoch: 1),
            turnCompletion: nil,
            pendingQuestions: [question],
            backgroundTasks: [],
            requiresExecutionLease: true
        )
        controller.sync(snapshot)
        controller.sync(snapshot)
        await drainAsyncNotifications()

        let payloads = await scheduler.snapshot()
        XCTAssertEqual(payloads.count, 1)
        XCTAssertEqual(payloads.first?.identifier, "conversation:session-a:7:waitingForUser:-")
    }

    func testBackgroundTaskTerminalTransitionPostsOnce() async {
        let scheduler = RecordingConversationScheduler()
        let controller = ConversationBackgroundAlertController(scheduler: scheduler)
        controller.setScenePhase(.background)
        let running = BackgroundTaskSnapshot(id: "task-1", descriptionText: "Build", status: .running)
        let completed = BackgroundTaskSnapshot(id: "task-1", descriptionText: "Build", status: .completed)

        controller.sync(ConversationBackgroundSnapshot(
            sessionID: "session-a",
            turnToken: nil,
            turnCompletion: nil,
            pendingQuestions: [],
            backgroundTasks: [running],
            requiresExecutionLease: true
        ))
        controller.sync(ConversationBackgroundSnapshot(
            sessionID: "session-a",
            turnToken: nil,
            turnCompletion: nil,
            pendingQuestions: [],
            backgroundTasks: [completed],
            requiresExecutionLease: false
        ))
        controller.sync(ConversationBackgroundSnapshot(
            sessionID: "session-a",
            turnToken: nil,
            turnCompletion: nil,
            pendingQuestions: [],
            backgroundTasks: [completed],
            requiresExecutionLease: false
        ))
        await drainAsyncNotifications()

        let payloads = await scheduler.snapshot()
        XCTAssertEqual(payloads.count, 1)
        XCTAssertEqual(payloads.first?.identifier, "conversation:session-a:none:completed:task-1")
    }

    func testUnknownBackgroundTaskExitDoesNotReportSuccessOrFailure() {
        let scheduler = RecordingConversationScheduler()
        var payloads: [ConversationNotificationPayload] = []
        var preferences = NotifConfig()
        preferences.enabled = true
        preferences.taskCompleteNotifEnabled = true
        let controller = ConversationBackgroundAlertController(
            scheduler: scheduler,
            preferences: preferences,
            notificationDispatcher: { payloads.append($0) }
        )
        controller.setScenePhase(.background)

        func task(_ id: String, _ status: BackgroundTaskSnapshot.Status) -> BackgroundTaskSnapshot {
            BackgroundTaskSnapshot(id: id, descriptionText: "sensitive command for \(id)", status: status)
        }
        func snapshot(
            _ tasks: [BackgroundTaskSnapshot],
            requiresExecutionLease: Bool
        ) -> ConversationBackgroundSnapshot {
            ConversationBackgroundSnapshot(
                sessionID: "session-a",
                turnToken: nil,
                turnCompletion: nil,
                pendingQuestions: [],
                backgroundTasks: tasks,
                requiresExecutionLease: requiresExecutionLease
            )
        }

        controller.sync(snapshot([
            task("unknown-transition", .running),
            task("success", .running),
            task("failure", .running),
        ], requiresExecutionLease: true))
        let unknown = task("unknown-transition", .exitedUnknown)
        let finished = snapshot([
            unknown,
            task("unknown-first-seen", .exitedUnknown),
            task("success", .completed),
            task("success", .completed),
            task("failure", .failed),
        ], requiresExecutionLease: false)
        controller.sync(finished)
        controller.sync(finished)
        let identifiers = payloads.map(\.identifier)
        XCTAssertEqual(identifiers.count, 2)
        XCTAssertEqual(identifiers.sorted(), [
            "conversation:session-a:none:completed:success",
            "conversation:session-a:none:failed:failure",
        ].sorted())
        XCTAssertEqual(
            payloads.first { $0.identifier.hasSuffix(":completed:success") }?.body,
            String(localized: "chat_status_completed")
        )
        XCTAssertEqual(
            payloads.first { $0.identifier.hasSuffix(":failed:failure") }?.body,
            String(localized: "chat_status_failed")
        )
        XCTAssertTrue(payloads.allSatisfy { $0.title == String(localized: "chat_execution_tasks") })
        XCTAssertFalse(payloads.contains { ($0.title + $0.body).contains("sensitive command") })
        XCTAssertTrue(unknown.status.isTerminal)
        XCTAssertFalse(unknown.status.requiresExecutionLease)
        XCTAssertNil(snapshot([unknown], requiresExecutionLease: false).preferredLiveActivitySnapshot)
    }

    func testUnknownTaskExitClearsMixedLiveActivityWithoutClaimingCompletion() {
        let scheduler = RecordingConversationScheduler()
        let reporter = RecordingConversationActivityReporter()
        var preferences = NotifConfig()
        preferences.enabled = false
        let controller = ConversationBackgroundAlertController(
            scheduler: scheduler,
            activityReporter: reporter,
            preferences: preferences
        )
        controller.setScenePhase(.background)

        func task(_ id: String, _ status: BackgroundTaskSnapshot.Status) -> BackgroundTaskSnapshot {
            BackgroundTaskSnapshot(id: id, descriptionText: id, status: status)
        }
        func snapshot(
            _ tasks: [BackgroundTaskSnapshot],
            requiresExecutionLease: Bool,
            turnCompletion: ConversationTurnCompletion? = nil,
            pendingQuestions: [ConversationPendingQuestion] = []
        ) -> ConversationBackgroundSnapshot {
            ConversationBackgroundSnapshot(
                sessionID: "session-a",
                turnToken: nil,
                turnCompletion: turnCompletion,
                pendingQuestions: pendingQuestions,
                backgroundTasks: tasks,
                requiresExecutionLease: requiresExecutionLease
            )
        }

        let completed = task("completed", .completed)
        let running = task("unknown", .running)
        let unknown = task("unknown", .exitedUnknown)
        let active = snapshot([completed, running], requiresExecutionLease: true)
        let settled = snapshot([completed, unknown], requiresExecutionLease: false)

        controller.sync(active)
        controller.sync(settled)
        XCTAssertEqual(reporter.snapshots.count, 2)
        XCTAssertEqual(reporter.snapshots[0]?.status, .running)
        XCTAssertNil(reporter.snapshots[1])
        XCTAssertNil(snapshot([unknown, completed], requiresExecutionLease: false).preferredLiveActivitySnapshot)
        XCTAssertEqual(snapshot([unknown, task("failed", .failed)], requiresExecutionLease: false).preferredLiveActivitySnapshot?.status, .failed)
        XCTAssertEqual(snapshot([unknown, task("paused", .paused)], requiresExecutionLease: false).preferredLiveActivitySnapshot?.status, .paused)
        XCTAssertEqual(snapshot([completed], requiresExecutionLease: false).preferredLiveActivitySnapshot?.status, .completed)
        XCTAssertEqual(snapshot([unknown, running], requiresExecutionLease: true).preferredLiveActivitySnapshot?.status, .running)
        let question = ConversationPendingQuestion(requestId: 9, questions: [], timeoutSecs: nil)
        XCTAssertEqual(
            snapshot([unknown], requiresExecutionLease: false, pendingQuestions: [question]).preferredLiveActivitySnapshot?.status,
            .waiting
        )

        let completion = ConversationTurnCompletion(
            token: ConversationTurnToken(clientTurnId: 42, sessionEpoch: 1),
            outcome: .completed,
            finalAssistantText: "done"
        )
        let turnActivity = snapshot([unknown], requiresExecutionLease: false, turnCompletion: completion).preferredLiveActivitySnapshot
        XCTAssertEqual(turnActivity?.status, .completed)
        XCTAssertEqual(turnActivity?.turnID, 42)
        XCTAssertEqual(turnActivity?.title, String(localized: "chat_agent_main"))
        XCTAssertEqual(turnActivity?.subtitle, String(localized: "chat_run_status_completed"))
        XCTAssertEqual(
            snapshot([unknown, task("failed", .failed)], requiresExecutionLease: false, turnCompletion: completion).preferredLiveActivitySnapshot?.status,
            .completed
        )
        XCTAssertEqual(
            snapshot([unknown, task("paused", .paused)], requiresExecutionLease: false, turnCompletion: completion).preferredLiveActivitySnapshot?.status,
            .completed
        )
        let failedCompletion = ConversationTurnCompletion(
            token: ConversationTurnToken(clientTurnId: 43, sessionEpoch: 1),
            outcome: .failed,
            finalAssistantText: ""
        )
        XCTAssertEqual(
            snapshot([unknown], requiresExecutionLease: false, turnCompletion: failedCompletion).preferredLiveActivitySnapshot?.status,
            .failed
        )
        let laterCompletion = ConversationTurnCompletion(
            token: ConversationTurnToken(clientTurnId: 44, sessionEpoch: 1),
            outcome: .completed,
            finalAssistantText: "done"
        )
        XCTAssertEqual(
            snapshot([unknown], requiresExecutionLease: false, turnCompletion: laterCompletion).preferredLiveActivitySnapshot?.turnID,
            44
        )
        XCTAssertEqual(
            snapshot([unknown, task("failed", .failed)], requiresExecutionLease: false, turnCompletion: laterCompletion).preferredLiveActivitySnapshot?.turnID,
            44
        )
        XCTAssertEqual(
            snapshot([unknown, task("paused", .paused)], requiresExecutionLease: false, turnCompletion: laterCompletion).preferredLiveActivitySnapshot?.turnID,
            44
        )
    }

    func testUnknownTaskDoesNotSuppressIndependentTurnIdleAlerts() async throws {
        let scheduler = RecordingConversationScheduler()
        let gate = AsyncGate()
        var payloads: [ConversationNotificationPayload] = []
        let controller = ConversationBackgroundAlertController(
            scheduler: scheduler,
            sleeper: { _ in await gate.wait() },
            preferences: NotifConfig(),
            notificationDispatcher: { payloads.append($0) }
        )
        controller.setScenePhase(.background)

        func snapshot(
            _ outcome: ConversationTurnCompletion.Outcome,
            taskStatus: BackgroundTaskSnapshot.Status,
            turnID: UInt64
        ) -> ConversationBackgroundSnapshot {
            let token = ConversationTurnToken(clientTurnId: turnID, sessionEpoch: 1)
            return ConversationBackgroundSnapshot(
                sessionID: "session-a",
                turnToken: token,
                turnCompletion: ConversationTurnCompletion(token: token, outcome: outcome, finalAssistantText: "done"),
                pendingQuestions: [],
                backgroundTasks: [BackgroundTaskSnapshot(id: "shell", descriptionText: "sensitive command", status: taskStatus)],
                requiresExecutionLease: taskStatus.requiresExecutionLease
            )
        }

        // An unknown task must not cancel a separately confirmed turn result.
        controller.sync(snapshot(.completed, taskStatus: .running, turnID: 7))
        let armed = try XCTUnwrap(controller.idleAlertTaskForTesting)
        controller.sync(snapshot(.completed, taskStatus: .exitedUnknown, turnID: 7))
        await gate.open()
        await armed.value
        XCTAssertEqual(payloads.map(\.identifier), ["conversation:session-a:7:completed:-"])

        // Historical unknown results must not suppress later completed or
        // failed turns, and each notification must describe the main run.
        controller.sync(snapshot(.completed, taskStatus: .exitedUnknown, turnID: 8))
        let completedAlert = try XCTUnwrap(controller.idleAlertTaskForTesting)
        await completedAlert.value
        controller.sync(snapshot(.failed, taskStatus: .exitedUnknown, turnID: 9))
        let failedAlert = try XCTUnwrap(controller.idleAlertTaskForTesting)
        await failedAlert.value
        XCTAssertEqual(payloads.map(\.identifier), [
            "conversation:session-a:7:completed:-",
            "conversation:session-a:8:completed:-",
            "conversation:session-a:9:failed:-",
        ])
        XCTAssertTrue(payloads.allSatisfy { $0.title == String(localized: "chat_agent_main") })
        XCTAssertEqual(payloads.map(\.body), [
            String(localized: "chat_run_status_completed"),
            String(localized: "chat_run_status_completed"),
            String(localized: "chat_run_status_failed"),
        ])
    }

    /// Every background task posted with `turnID: nil` and no discriminator, so
    /// all of them collapsed onto ONE identifier and `post`'s delivered-set
    /// guard swallowed every task after the first — permanently, since the set
    /// is never pruned. Two distinct tasks completing must notify twice.
    func testDistinctBackgroundTasksEachNotify() async throws {
        let scheduler = RecordingConversationScheduler()
        let controller = ConversationBackgroundAlertController(scheduler: scheduler)
        controller.setScenePhase(.background)
        let runningA = BackgroundTaskSnapshot(id: "task-a", descriptionText: "A", status: .running)
        let runningB = BackgroundTaskSnapshot(id: "task-b", descriptionText: "B", status: .running)
        let doneA = BackgroundTaskSnapshot(id: "task-a", descriptionText: "A", status: .completed)
        let doneB = BackgroundTaskSnapshot(id: "task-b", descriptionText: "B", status: .completed)
        func snapshot(_ tasks: [BackgroundTaskSnapshot]) -> ConversationBackgroundSnapshot {
            ConversationBackgroundSnapshot(
                sessionID: "session-a",
                turnToken: nil,
                turnCompletion: nil,
                pendingQuestions: [],
                backgroundTasks: tasks,
                requiresExecutionLease: false
            )
        }

        controller.sync(snapshot([runningA, runningB]))
        controller.sync(snapshot([doneA, runningB]))
        controller.sync(snapshot([doneA, doneB]))
        await drainAsyncNotifications()

        let identifiers = await scheduler.snapshot().map(\.identifier)
        XCTAssertEqual(
            identifiers,
            [
                "conversation:session-a:none:completed:task-a",
                "conversation:session-a:none:completed:task-b",
            ],
            "each completed task must produce its own notification"
        )
    }

    /// A repeated task id used to trap `Dictionary(uniqueKeysWithValues:)` and
    /// hard-crash the main actor.
    func testRepeatedBackgroundTaskIDDoesNotTrap() async throws {
        let scheduler = RecordingConversationScheduler()
        let controller = ConversationBackgroundAlertController(scheduler: scheduler)
        controller.setScenePhase(.background)
        let duplicate = BackgroundTaskSnapshot(id: "task-1", descriptionText: "Build", status: .running)

        controller.sync(ConversationBackgroundSnapshot(
            sessionID: "session-a",
            turnToken: nil,
            turnCompletion: nil,
            pendingQuestions: [],
            backgroundTasks: [duplicate, duplicate],
            requiresExecutionLease: false
        ))
        await drainAsyncNotifications()

        let payloadCount = await scheduler.snapshot().count
        XCTAssertEqual(payloadCount, 0, "a running duplicate notifies nothing")
    }

    func testRecoverablePauseUsesConversationRoute() async throws {
        let scheduler = RecordingConversationScheduler()
        let controller = ConversationBackgroundAlertController(scheduler: scheduler)
        controller.setScenePhase(.background)

        controller.markRecoverablePause(
            sessionID: "session-a",
            turnToken: ConversationTurnToken(clientTurnId: 5, sessionEpoch: 1)
        )
        await drainAsyncNotifications()

        let payloads = await scheduler.snapshot()
        let payload = try XCTUnwrap(payloads.first)
        XCTAssertEqual(payload.identifier, "conversation:session-a:5:pausedRecoverable:-")
        XCTAssertEqual(
            ConversationNotificationRoute.appAction(from: payload.userInfo),
            .openConversation(
                sessionID: "session-a",
                turnID: 5,
                workspaceKey: "global",
                mode: .code
            )
        )
    }

    func testContinuedProcessingIdentifierUsesWildcardSafeSuffix() {
        let identifier = ConversationContinuedProcessingIdentifier.make(
            sessionID: " session/a:b ",
            turnID: 42
        )

        XCTAssertEqual(
            identifier,
            "com.lingxi.code.conversation.continued.session-a-b.42"
        )
        XCTAssertEqual(
            ConversationContinuedProcessingIdentifier.turnID(from: identifier),
            42
        )
    }

    func testLeasePolicyDisablesFiniteAssertionWhenContinuedProcessingExists() {
        XCTAssertFalse(
            ConversationBackgroundLeasePolicy.prefersFiniteAssertion(
                continuedProcessingAvailable: true
            )
        )
        XCTAssertTrue(
            ConversationBackgroundLeasePolicy.prefersFiniteAssertion(
                continuedProcessingAvailable: false
            )
        )
    }

    func testLeasePolicyKeepsFiniteAssertionUntilAnActualTaskAttaches() {
        XCTAssertTrue(
            ConversationBackgroundLeasePolicy.prefersFiniteAssertion(
                continuedProcessingAvailable: true,
                leaseState: .queued
            )
        )
        XCTAssertTrue(
            ConversationBackgroundLeasePolicy.prefersFiniteAssertion(
                continuedProcessingAvailable: true,
                leaseState: .submissionFailed
            )
        )
        XCTAssertFalse(
            ConversationBackgroundLeasePolicy.prefersFiniteAssertion(
                continuedProcessingAvailable: true,
                leaseState: .attached
            )
        )
        XCTAssertTrue(
            ConversationBackgroundLeasePolicy.prefersFiniteAssertion(
                continuedProcessingAvailable: false,
                leaseState: .attached
            )
        )
    }

    func testFiniteAssertionControllerSkipsUIKitLeaseWhenPolicyDisablesIt() {
        var beginCount = 0
        var endCount = 0
        let controller = ConversationBackgroundExecutionController(
            beginTask: { _ in
                beginCount += 1
                return .invalid
            },
            endTask: { _ in
                endCount += 1
            },
            shouldUseFiniteAssertion: { false }
        )

        controller.setTurnActive(true)
        controller.setTurnActive(false)

        XCTAssertEqual(beginCount, 0)
        XCTAssertEqual(endCount, 0)
    }

    func testFiniteAssertionControllerKeepsFallbackForQueuedOrFailedRequestUntilAttached() {
        var beginCount = 0
        var endedTasks: [UIBackgroundTaskIdentifier] = []
        let controller = ConversationBackgroundExecutionController(
            beginTask: { _ in
                beginCount += 1
                return UIBackgroundTaskIdentifier(rawValue: 91)
            },
            endTask: { endedTasks.append($0) }
        )

        controller.setTurnActive(true, turnID: 11)
        controller.setContinuedProcessingLeaseAttached(false, turnID: 11) // queued/failure fallback
        XCTAssertEqual(beginCount, 1)
        XCTAssertTrue(endedTasks.isEmpty)

        // An unrelated task attachment cannot release this turn's finite lease.
        controller.setContinuedProcessingLeaseAttached(true, turnID: 12)
        XCTAssertTrue(endedTasks.isEmpty)

        controller.setContinuedProcessingLeaseAttached(true, turnID: 11)
        XCTAssertEqual(endedTasks.map(\.rawValue), [91])
    }

    func testLiveActivityReconciliationAdoptsMatchingSessionAndEndsStaleSessions() {
        let plan = ConversationLiveActivityReconciliationPlan.make(
            currentActivityID: "activity-b",
            activities: [
                .init(activityID: "activity-a", sessionID: "session-a"),
                .init(activityID: "activity-b", sessionID: "session-b"),
                .init(activityID: "activity-c", sessionID: "session-b"),
            ],
            targetSnapshot: ConversationLiveActivitySnapshot(
                sessionID: "session-b",
                turnID: 7,
                title: "Running",
                subtitle: "Background",
                status: .running,
                updatedAt: .now
            )
        )

        XCTAssertEqual(plan.matchedActivityID, "activity-b")
        XCTAssertEqual(plan.staleActivityIDs, ["activity-a", "activity-c"])
        XCTAssertFalse(plan.shouldStartNew)
    }

    func testContinuedProcessingExpirationParsesNotificationUserInfo() {
        let expiration = ConversationContinuedProcessingExpiration(
            identifier: "com.lingxi.code.conversation.continued.session-a.9",
            turnID: 9
        )

        XCTAssertEqual(
            ConversationContinuedProcessingExpiration(userInfo: expiration.userInfo),
            expiration
        )
    }

    // MARK: - Upstream timing policy
    //
    // The policy here is Claude Code 2.1.270's, copied rather than invented:
    // a finished turn ARMS an idle alert instead of posting one, and a
    // permission prompt answered inside 6 seconds notifies nothing at all.

    /// Immediate sleeper: exercises the fire-time re-checks without a real wait.
    /// The threshold's 5s floor is a production clamp worth keeping, and far too
    /// slow for a unit test, so the delay itself is the seam rather than the
    /// clamp.
    private static let noWait: ConversationNotificationSleeper = { _ in }

    private func completionSnapshot(
        outcome: ConversationTurnCompletion.Outcome = .completed
    ) -> ConversationBackgroundSnapshot {
        let token = ConversationTurnToken(clientTurnId: 7, sessionEpoch: 1)
        return ConversationBackgroundSnapshot(
            sessionID: "session-a",
            turnToken: token,
            turnCompletion: ConversationTurnCompletion(
                token: token,
                outcome: outcome,
                finalAssistantText: "done"
            ),
            pendingQuestions: [],
            backgroundTasks: [],
            requiresExecutionLease: false
        )
    }

    func testFinishedTurnArmsTheIdleAlertRatherThanPostingOne() async {
        let scheduler = RecordingConversationScheduler()
        // A sleeper that never returns: the alert is armed but its delay never
        // elapses, which is the state a user who comes back quickly is in.
        let controller = ConversationBackgroundAlertController(
            scheduler: scheduler,
            sleeper: { _ in try? await Task.sleep(nanoseconds: 60_000_000_000) },
            preferences: NotifConfig()
        )
        controller.setScenePhase(.background)

        controller.sync(completionSnapshot())
        await drainAsyncNotifications()

        let payloads = await scheduler.snapshot()
        XCTAssertTrue(payloads.isEmpty, "a finished turn must not notify immediately")
    }

    func testIdleAlertFiresOnceTheThresholdElapses() async throws {
        let scheduler = RecordingConversationScheduler()
        let controller = ConversationBackgroundAlertController(
            scheduler: scheduler,
            sleeper: Self.noWait,
            preferences: NotifConfig()
        )
        controller.setScenePhase(.background)

        controller.sync(completionSnapshot())
        await drainAsyncNotifications()

        let payloads = await scheduler.snapshot()
        let payload = try XCTUnwrap(payloads.first)
        XCTAssertEqual(payload.identifier, "conversation:session-a:7:completed:-")
    }

    func testComingBackBeforeTheThresholdCancelsTheIdleAlert() async {
        let scheduler = RecordingConversationScheduler()
        let gate = AsyncGate()
        let controller = ConversationBackgroundAlertController(
            scheduler: scheduler,
            sleeper: { _ in await gate.wait() },
            preferences: NotifConfig()
        )
        controller.setScenePhase(.background)
        controller.sync(completionSnapshot())
        await drainAsyncNotifications()

        // The user returns while the alert is still counting down.
        controller.setScenePhase(.active)
        await gate.open()
        await drainAsyncNotifications()

        let payloads = await scheduler.snapshot()
        XCTAssertTrue(payloads.isEmpty, "returning to the app must cancel the armed alert")
    }

    func testIdlePromptTogglesOffSuppressTheAlert() async {
        let scheduler = RecordingConversationScheduler()
        var config = NotifConfig()
        config.idlePromptNotifEnabled = false
        let controller = ConversationBackgroundAlertController(
            scheduler: scheduler,
            sleeper: Self.noWait,
            preferences: config
        )
        controller.setScenePhase(.background)

        controller.sync(completionSnapshot())
        await drainAsyncNotifications()

        let payloads = await scheduler.snapshot()
        XCTAssertTrue(payloads.isEmpty)
    }

    func testPermissionAnsweredInsideTheDelayNotifiesNothing() async {
        let scheduler = RecordingConversationScheduler()
        let gate = AsyncGate()
        let controller = ConversationBackgroundAlertController(
            scheduler: scheduler,
            sleeper: { _ in await gate.wait() },
            preferences: NotifConfig()
        )
        controller.setScenePhase(.background)

        func snapshot(_ permissions: [ConversationPendingPermission]) -> ConversationBackgroundSnapshot {
            ConversationBackgroundSnapshot(
                sessionID: "session-a",
                turnToken: ConversationTurnToken(clientTurnId: 7, sessionEpoch: 1),
                turnCompletion: nil,
                pendingQuestions: [],
                pendingPermissions: permissions,
                backgroundTasks: [],
                requiresExecutionLease: true
            )
        }

        controller.sync(snapshot([ConversationPendingPermission(requestId: 3, toolName: "Bash")]))
        await drainAsyncNotifications()
        controller.sync(snapshot([]))          // answered
        await gate.open()
        await drainAsyncNotifications()

        let payloads = await scheduler.snapshot()
        XCTAssertTrue(payloads.isEmpty)
    }

    func testPermissionLeftUnansweredNotifiesAndNamesTheTool() async throws {
        let scheduler = RecordingConversationScheduler()
        let controller = ConversationBackgroundAlertController(
            scheduler: scheduler,
            sleeper: Self.noWait,
            preferences: NotifConfig()
        )
        controller.setScenePhase(.background)

        controller.sync(ConversationBackgroundSnapshot(
            sessionID: "session-a",
            turnToken: ConversationTurnToken(clientTurnId: 7, sessionEpoch: 1),
            turnCompletion: nil,
            pendingQuestions: [],
            pendingPermissions: [ConversationPendingPermission(requestId: 3, toolName: "Bash")],
            backgroundTasks: [],
            requiresExecutionLease: true
        ))
        await drainAsyncNotifications()

        let payloads = await scheduler.snapshot()
        let payload = try XCTUnwrap(payloads.first)
        XCTAssertEqual(payload.identifier, "conversation:session-a:7:needsPermission:3")
        XCTAssertTrue(payload.body.contains("Bash"), "the body must name the tool: \(payload.body)")
    }

    func testInputNeededToggleGatesPermissionsButNotFinishedTurns() async {
        let scheduler = RecordingConversationScheduler()
        var config = NotifConfig()
        config.inputNeededNotifEnabled = false
        let controller = ConversationBackgroundAlertController(
            scheduler: scheduler,
            sleeper: Self.noWait,
            preferences: config
        )
        controller.setScenePhase(.background)

        controller.sync(ConversationBackgroundSnapshot(
            sessionID: "session-a",
            turnToken: ConversationTurnToken(clientTurnId: 7, sessionEpoch: 1),
            turnCompletion: nil,
            pendingQuestions: [],
            pendingPermissions: [ConversationPendingPermission(requestId: 3, toolName: "Bash")],
            backgroundTasks: [],
            requiresExecutionLease: true
        ))
        await drainAsyncNotifications()
        let afterPermission = await scheduler.snapshot()
        XCTAssertTrue(afterPermission.isEmpty, "input-needed is off")

        controller.sync(completionSnapshot())
        await drainAsyncNotifications()
        let afterCompletion = await scheduler.snapshot()
        XCTAssertEqual(
            afterCompletion.count, 1,
            "the idle alert has its own gate and stays on"
        )
    }

    func testMasterSwitchOffSilencesEveryKind() async {
        let scheduler = RecordingConversationScheduler()
        var config = NotifConfig()
        config.enabled = false
        let controller = ConversationBackgroundAlertController(
            scheduler: scheduler,
            sleeper: Self.noWait,
            preferences: config
        )
        controller.setScenePhase(.background)

        controller.sync(completionSnapshot())
        controller.markRecoverablePause(
            sessionID: "session-a",
            turnToken: ConversationTurnToken(clientTurnId: 5, sessionEpoch: 1)
        )
        await drainAsyncNotifications()

        let payloads = await scheduler.snapshot()
        XCTAssertTrue(payloads.isEmpty)
    }

    private func drainAsyncNotifications(_ turns: Int = 6) async {
        for _ in 0..<turns {
            await Task.yield()
        }
    }
}

/// A one-shot gate so a test can hold an armed delay open, act, then release it.
private actor AsyncGate {
    private var opened = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func wait() async {
        if opened { return }
        await withCheckedContinuation { waiters.append($0) }
    }

    func open() {
        opened = true
        let pending = waiters
        waiters.removeAll()
        for continuation in pending { continuation.resume() }
    }
}

private actor RecordingConversationScheduler: ConversationNotificationScheduling {
    private(set) var payloads: [ConversationNotificationPayload] = []

    func deliver(_ payload: ConversationNotificationPayload) async {
        payloads.append(payload)
    }

    func snapshot() -> [ConversationNotificationPayload] {
        payloads
    }
}

@MainActor
private final class RecordingConversationActivityReporter: ConversationActivityReporting {
    private(set) var snapshots: [ConversationLiveActivitySnapshot?] = []

    func sync(snapshot: ConversationBackgroundSnapshot, scenePhase _: ScenePhase) {
        snapshots.append(snapshot.preferredLiveActivitySnapshot)
    }

    func clear() {}
}
