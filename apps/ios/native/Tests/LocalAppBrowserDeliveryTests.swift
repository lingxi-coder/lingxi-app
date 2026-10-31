import UIKit
import XCTest
@testable import LingxiCode

@MainActor
final class LocalAppBrowserDeliveryTests: XCTestCase {
    func testNormalPageOpenBeginsOneFiniteWindowBeforeOpening() {
        let deadlines = DeadlineRecorder()
        var order: [String] = []
        var ended: [UIBackgroundTaskIdentifier] = []
        var identifier = 40
        let controller = LocalAppBrowserDeliveryController(
            beginTask: { _ in
                order.append("begin")
                identifier += 1
                return UIBackgroundTaskIdentifier(rawValue: identifier)
            },
            endTask: { ended.append($0) },
            scheduleDeadline: { duration, handler in
                deadlines.schedule(after: duration, handler: handler)
            }
        )
        let scope = LocalAppBrowserDeliveryScope(appID: "app-a", revision: 7)

        let first = controller.openPage(scope: scope) {
            order.append("open")
        }
        let second = controller.openPage(scope: scope) {
            order.append("open-again")
        }

        XCTAssertEqual(first, .started)
        XCTAssertEqual(second, .alreadyActive)
        XCTAssertEqual(order, ["begin", "open", "open-again"])
        XCTAssertEqual(deadlines.durations, [.seconds(15)])
        XCTAssertTrue(controller.isActive(scope))

        controller.end(scope)
        XCTAssertEqual(ended.map(\.rawValue), [41])
    }

    func testInvalidUIKitIdentifierDoesNotOpenOrCallEnd() {
        let deadlines = DeadlineRecorder()
        var openCount = 0
        var endCount = 0
        let controller = LocalAppBrowserDeliveryController(
            beginTask: { _ in .invalid },
            endTask: { _ in endCount += 1 },
            scheduleDeadline: { duration, handler in
                deadlines.schedule(after: duration, handler: handler)
            }
        )

        let result = controller.openPage(
            scope: LocalAppBrowserDeliveryScope(appID: "app-a", revision: 7)
        ) {
            openCount += 1
        }

        XCTAssertEqual(result, .unavailable)
        XCTAssertEqual(openCount, 0)
        XCTAssertEqual(endCount, 0)
        XCTAssertTrue(deadlines.durations.isEmpty)
    }

    func testExpirationDeadlineAndExplicitEndRaceEndsOneTokenOnce() {
        let deadlines = DeadlineRecorder()
        var expirationHandlers: [LocalAppBrowserDeliveryController.ExpirationHandler] = []
        var nextIdentifier = 50
        var endedTasks: [UIBackgroundTaskIdentifier] = []
        let controller = LocalAppBrowserDeliveryController(
            beginTask: { handler in
                expirationHandlers.append(handler)
                nextIdentifier += 1
                return UIBackgroundTaskIdentifier(rawValue: nextIdentifier)
            },
            endTask: { endedTasks.append($0) },
            scheduleDeadline: { duration, handler in
                deadlines.schedule(after: duration, handler: handler)
            }
        )
        let expirationFirst = LocalAppBrowserDeliveryScope(appID: "app-a", revision: 7)
        let deadlineFirst = LocalAppBrowserDeliveryScope(appID: "app-b", revision: 7)

        XCTAssertEqual(controller.begin(expirationFirst), .started)
        expirationHandlers[0]()
        deadlines.fire(0)
        controller.end(expirationFirst)

        XCTAssertEqual(controller.begin(deadlineFirst), .started)
        deadlines.fire(1)
        expirationHandlers[1]()
        controller.end(deadlineFirst)

        XCTAssertEqual(endedTasks.map(\.rawValue), [51, 52])
        XCTAssertEqual(deadlines.cancelCount, 2)
        XCTAssertFalse(controller.isActive(expirationFirst))
        XCTAssertFalse(controller.isActive(deadlineFirst))
    }

    func testEndAndStaleReleaseAreIsolatedByAppAndRevision() {
        let deadlines = DeadlineRecorder()
        var nextIdentifier = 60
        var ended: [Int] = []
        let controller = LocalAppBrowserDeliveryController(
            beginTask: { _ in
                nextIdentifier += 1
                return UIBackgroundTaskIdentifier(rawValue: nextIdentifier)
            },
            endTask: { ended.append($0.rawValue) },
            scheduleDeadline: { duration, handler in
                deadlines.schedule(after: duration, handler: handler)
            }
        )
        let oldRelease = LocalAppBrowserDeliveryScope(appID: "app-a", revision: 7)
        let currentRelease = LocalAppBrowserDeliveryScope(appID: "app-a", revision: 8)
        let otherApp = LocalAppBrowserDeliveryScope(appID: "app-b", revision: 7)

        XCTAssertEqual(controller.begin(oldRelease), .started)
        XCTAssertEqual(controller.begin(currentRelease), .started)
        XCTAssertEqual(controller.begin(otherApp), .started)

        controller.endStale(appID: "app-a", keepingRevision: 8)
        XCTAssertFalse(controller.isActive(oldRelease))
        XCTAssertTrue(controller.isActive(currentRelease))
        XCTAssertTrue(controller.isActive(otherApp))

        controller.endAll(appID: "app-a")
        XCTAssertFalse(controller.isActive(currentRelease))
        XCTAssertTrue(controller.isActive(otherApp))
        XCTAssertEqual(ended, [61, 62])

        controller.end(otherApp)
        XCTAssertEqual(ended, [61, 62, 63])
    }

    func testTerminalWindowBeginsBeforeContinuedTaskCompletes() async {
        let deadlines = DeadlineRecorder()
        var order: [String] = []
        let controller = LocalAppBrowserDeliveryController(
            beginTask: { _ in
                order.append("UIKit begin")
                return UIBackgroundTaskIdentifier(rawValue: 70)
            },
            endTask: { _ in order.append("UIKit end") },
            scheduleDeadline: { duration, handler in
                deadlines.schedule(after: duration, handler: handler)
            }
        )

        let acquisition = await controller.finishBackgroundTaskAfterStartingResultDelivery(
            scope: LocalAppBrowserDeliveryScope(appID: "app-a", revision: 7)
        ) {
            order.append("BG task complete")
        }

        XCTAssertEqual(acquisition, .started)
        XCTAssertEqual(order, ["UIKit begin", "BG task complete"])
        XCTAssertEqual(deadlines.durations, [.seconds(15)])
    }

    func testTerminalDeliveryPolicyUsesDurableStateAndStandardOutcome() {
        func event(_ type: String, outcome: String? = nil) -> LocalAppJSON {
            var fields: [String: LocalAppJSON] = [
                "type": .string(type),
                "runId": .string("run-1")
            ]
            if let outcome {
                fields["outcome"] = .object(["type": .string(outcome)])
            }
            return .object(fields)
        }

        XCTAssertTrue(LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
            taskState: "succeeded",
            terminalEvent: event("RUN_FINISHED", outcome: "success")
        ))
        XCTAssertTrue(LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
            taskState: "failed",
            terminalEvent: event("RUN_ERROR")
        ))
        XCTAssertTrue(LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
            taskState: "failed",
            terminalEvent: nil
        ))
        XCTAssertTrue(LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
            taskState: "succeeded",
            terminalEvent: event("RUN_FINISHED")
        ))
        XCTAssertFalse(LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
            taskState: nil,
            terminalEvent: event("RUN_FINISHED")
        ))
        XCTAssertFalse(LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
            taskState: "cancelled",
            terminalEvent: event("RUN_FINISHED", outcome: "cancelled")
        ))
        XCTAssertFalse(LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
            taskState: "interrupted",
            terminalEvent: event("RUN_FINISHED", outcome: "interrupt")
        ))
        XCTAssertFalse(LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
            taskState: "cancelled",
            terminalEvent: event("RUN_ERROR")
        ))
        XCTAssertFalse(LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
            taskState: "running",
            terminalEvent: event("RUN_FINISHED", outcome: "cancelled")
        ))
        XCTAssertFalse(LocalAppBrowserResultDeliveryPolicy.isSuccessfulRun(
            taskState: "succeeded",
            terminalEvent: event("RUN_FINISHED", outcome: "cancelled"),
            runID: "run-1"
        ))
        XCTAssertFalse(LocalAppBrowserResultDeliveryPolicy.isSuccessfulRun(
            taskState: "cancelled",
            terminalEvent: event("RUN_FINISHED", outcome: "success"),
            runID: "run-1"
        ))
        XCTAssertTrue(LocalAppBrowserResultDeliveryPolicy.isSuccessfulRun(
            taskState: "succeeded",
            terminalEvent: event("RUN_FINISHED"),
            runID: "run-1"
        ))
    }
}

@MainActor
private final class DeadlineRecorder {
    private(set) var durations: [Duration] = []
    private(set) var cancelCount = 0
    private var handlers: [LocalAppBrowserDeliveryController.ExpirationHandler] = []

    func schedule(
        after duration: Duration,
        handler: @escaping LocalAppBrowserDeliveryController.ExpirationHandler
    ) -> LocalAppBrowserDeliveryDeadline {
        durations.append(duration)
        handlers.append(handler)
        return LocalAppBrowserDeliveryDeadline { [weak self] in
            self?.cancelCount += 1
        }
    }

    func fire(_ index: Int) {
        handlers[index]()
    }
}
