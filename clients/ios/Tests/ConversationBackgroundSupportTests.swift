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

    private func drainAsyncNotifications(_ turns: Int = 6) async {
        for _ in 0..<turns {
            await Task.yield()
        }
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
