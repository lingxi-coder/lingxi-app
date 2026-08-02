import UIKit
import XCTest

@testable import LingxiCode

@MainActor
final class CronLifecycleTests: XCTestCase {
    func testDidFinishLaunchingRegistersCronBackgroundTaskAndDefersColdStartTriggerUntilBind() async {
        let bridge = CronBackgroundTaskBridge()
        let registrar = FakeCronBackgroundRegistrar()
        let previousBridge = AppNotificationDelegate.cronBackgroundBridge
        let previousFactory = AppNotificationDelegate.cronBackgroundRegistrarFactory
        AppNotificationDelegate.cronBackgroundBridge = bridge
        AppNotificationDelegate.cronBackgroundRegistrarFactory = { registrar }
        defer {
            AppNotificationDelegate.cronBackgroundBridge = previousBridge
            AppNotificationDelegate.cronBackgroundRegistrarFactory = previousFactory
        }

        let delegate = AppNotificationDelegate()
        _ = delegate.application(UIApplication.shared, didFinishLaunchingWithOptions: nil)

        XCTAssertEqual(registrar.registeredIdentifiers, [cronBackgroundTaskIdentifier])

        let drainExpectation = expectation(description: "pending cold-start trigger drains after bind")
        let handlerStarted = expectation(description: "handler started")
        let runCounter = LockedInt()
        let completionState = LockedBool()
        let gate = AsyncGate()

        let fireTask = Task {
            await registrar.fire(identifier: cronBackgroundTaskIdentifier)
            completionState.setTrue()
        }
        XCTAssertEqual(runCounter.value, 0)

        bridge.bind(taskIdentifier: cronBackgroundTaskIdentifier) {
            handlerStarted.fulfill()
            await gate.wait()
            runCounter.increment()
            drainExpectation.fulfill()
        }

        await fulfillment(of: [handlerStarted], timeout: 2.0)
        XCTAssertEqual(completionState.value, false)
        await gate.open()
        await fulfillment(of: [drainExpectation], timeout: 2.0)
        _ = await fireTask.value
        XCTAssertEqual(completionState.value, true)
        XCTAssertEqual(runCounter.value, 1)
    }
}

private final class FakeCronBackgroundRegistrar: CronBackgroundTaskRegistrar, @unchecked Sendable {
    private let lock = NSLock()
    private(set) var registeredIdentifiers: [String] = []
    private var handlers: [String: @Sendable () async -> Void] = [:]

    func register(
        identifier: String,
        handler: @escaping @Sendable () async -> Void
    ) {
        lock.lock()
        registeredIdentifiers.append(identifier)
        handlers[identifier] = handler
        lock.unlock()
    }

    func fire(identifier: String) async {
        await handler(for: identifier)?()
    }

    private func handler(for identifier: String) -> (@Sendable () async -> Void)? {
        lock.lock()
        let handler = handlers[identifier]
        lock.unlock()
        return handler
    }
}

private final class LockedInt: @unchecked Sendable {
    private let lock = NSLock()
    private var storage = 0

    var value: Int {
        lock.lock()
        let value = storage
        lock.unlock()
        return value
    }

    func increment() {
        lock.lock()
        storage += 1
        lock.unlock()
    }
}

private final class LockedBool: @unchecked Sendable {
    private let lock = NSLock()
    private var storage = false

    var value: Bool {
        lock.lock()
        let value = storage
        lock.unlock()
        return value
    }

    func setTrue() {
        lock.lock()
        storage = true
        lock.unlock()
    }
}

private actor AsyncGate {
    private var opened = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func wait() async {
        guard !opened else { return }
        await withCheckedContinuation { continuation in
            waiters.append(continuation)
        }
    }

    func open() {
        opened = true
        let current = waiters
        waiters.removeAll()
        current.forEach { $0.resume() }
    }
}
