import Foundation

#if canImport(UserNotifications)
    import UserNotifications
#endif

#if canImport(BackgroundTasks)
    import BackgroundTasks
#endif

final class UserNotificationCronNotifier: CronNotificationDelivering, @unchecked Sendable {
    func deliver(_ payload: CronNotificationPayload) async {
        #if canImport(UserNotifications)
            guard !Task.isCancelled else { return }
            let center = UNUserNotificationCenter.current()
            let settings = await center.notificationSettings()
            guard !Task.isCancelled else { return }
            switch settings.authorizationStatus {
            case .authorized, .provisional, .ephemeral:
                break
            case .denied:
                return
            default:
                guard (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) == true else {
                    return
                }
                guard !Task.isCancelled else { return }
            }
            let content = UNMutableNotificationContent()
            content.title = payload.title
            content.body = payload.body
            content.sound = .default
            content.userInfo = [
                "lingxi.route": "cron.run",
                "lingxi.cron.run_id": payload.runID,
                "lingxi.cron.scope_id": payload.scopeID,
                "lingxi.cron.task_id": payload.taskID,
                "lingxi.cron.status": payload.status.rawValue,
            ]
            let request = UNNotificationRequest(
                identifier: "cron-run-\(payload.runID)",
                content: content,
                trigger: nil
            )
            guard !Task.isCancelled else { return }
            try? await center.add(request)
        #endif
    }
}

/// Bridges launch-time BGTask registration to the repository created by the
/// SwiftUI composition root. Safety: `lock` protects every mutable collection
/// and continuations are removed under that lock before being resumed.
final class CronBackgroundTaskBridge: @unchecked Sendable {
    static let shared = CronBackgroundTaskBridge()

    typealias CronAsyncHandler = @Sendable () async -> Void

    private let lock = NSLock()
    private var registeredIdentifiers = Set<String>()
    private var handlers: [String: CronAsyncHandler] = [:]
    private var pendingHandlerWaiters: [String: [UUID: CheckedContinuation<CronAsyncHandler?, Never>]] = [:]

    func registerAtLaunch(
        taskIdentifier: String = cronBackgroundTaskIdentifier,
        registrar: any CronBackgroundTaskRegistrar = LiveCronBackgroundTaskRegistrar()
    ) {
        lock.lock()
        let shouldRegister = registeredIdentifiers.insert(taskIdentifier).inserted
        lock.unlock()
        guard shouldRegister else { return }
        registrar.register(identifier: taskIdentifier) { [weak self] in
            await self?.runRegisteredHandler(taskIdentifier: taskIdentifier)
        }
    }

    func bind(
        taskIdentifier: String = cronBackgroundTaskIdentifier,
        handler: @escaping CronAsyncHandler
    ) {
        let waiters: [CheckedContinuation<CronAsyncHandler?, Never>]
        lock.lock()
        handlers[taskIdentifier] = handler
        waiters = pendingHandlerWaiters.removeValue(forKey: taskIdentifier).map { Array($0.values) } ?? []
        lock.unlock()
        waiters.forEach { $0.resume(returning: handler) }
    }

    func unbind(taskIdentifier: String = cronBackgroundTaskIdentifier) {
        lock.lock()
        handlers.removeValue(forKey: taskIdentifier)
        lock.unlock()
    }

    private func runRegisteredHandler(taskIdentifier: String) async {
        if let handler = boundHandler(for: taskIdentifier) {
            guard !Task.isCancelled else { return }
            await handler()
            return
        }
        let waiterID = UUID()
        let handler = await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<CronAsyncHandler?, Never>) in
                lock.lock()
                if Task.isCancelled {
                    lock.unlock()
                    continuation.resume(returning: nil)
                    return
                }
                if let handler = handlers[taskIdentifier] {
                    lock.unlock()
                    continuation.resume(returning: handler)
                    return
                }
                var waiters = pendingHandlerWaiters[taskIdentifier] ?? [:]
                waiters[waiterID] = continuation
                pendingHandlerWaiters[taskIdentifier] = waiters
                lock.unlock()
            }
        } onCancel: { [weak self] in
            self?.cancelWaiter(taskIdentifier: taskIdentifier, waiterID: waiterID)
        }
        guard let handler else { return }
        guard !Task.isCancelled else { return }
        await handler()
    }

    private func boundHandler(for taskIdentifier: String) -> CronAsyncHandler? {
        lock.lock()
        let handler = handlers[taskIdentifier]
        lock.unlock()
        return handler
    }

    private func cancelWaiter(taskIdentifier: String, waiterID: UUID) {
        let continuation: CheckedContinuation<CronAsyncHandler?, Never>?
        lock.lock()
        continuation = pendingHandlerWaiters[taskIdentifier]?[waiterID]
        pendingHandlerWaiters[taskIdentifier]?[waiterID] = nil
        if pendingHandlerWaiters[taskIdentifier]?.isEmpty == true {
            pendingHandlerWaiters[taskIdentifier] = nil
        }
        lock.unlock()
        continuation?.resume(returning: nil)
    }
}

final class BestEffortBackgroundCronScheduler: CronBackgroundScheduling, Sendable {
    let mode: CronSchedulingMode = .bestEffortBackground
    let note = String(localized: "cron_scheduler_background_note")

    func schedule(taskIdentifier: String, earliestAtMs: UInt64?) async throws {
        #if canImport(BackgroundTasks)
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: taskIdentifier)
            guard let earliestAtMs else { return }
            let request = BGProcessingTaskRequest(identifier: taskIdentifier)
            request.requiresNetworkConnectivity = true
            request.requiresExternalPower = false
            request.earliestBeginDate = Date(timeIntervalSince1970: TimeInterval(earliestAtMs) / 1000)
            try BGTaskScheduler.shared.submit(request)
        #endif
    }

    func cancel(taskIdentifier: String) async {
        #if canImport(BackgroundTasks)
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: taskIdentifier)
        #endif
    }
}

#if canImport(BackgroundTasks)
    struct LiveCronBackgroundTaskRegistrar: CronBackgroundTaskRegistrar {
        func register(
            identifier: String,
            handler: @escaping @Sendable () async -> Void
        ) {
            BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: nil) { task in
                guard let processingTask = task as? BGProcessingTask else {
                    task.setTaskCompleted(success: false)
                    return
                }
                let completion = BackgroundTaskCompletion(task: processingTask)
                let worker = Task {
                    await handler()
                    completion.finish(success: !Task.isCancelled)
                }
                processingTask.expirationHandler = {
                    worker.cancel()
                    completion.finish(success: false)
                }
            }
        }
    }

    /// Safety: completion state and the weak BGTask reference are read and
    /// mutated only while holding `lock`.
    private final class BackgroundTaskCompletion: @unchecked Sendable {
        private let lock = NSLock()
        private weak var task: BGTask?
        private var completed = false

        init(task: BGTask) {
            self.task = task
        }

        func finish(success: Bool) {
            lock.lock()
            guard !completed else {
                lock.unlock()
                return
            }
            completed = true
            let task = task
            lock.unlock()
            task?.setTaskCompleted(success: success)
        }
    }
#else
    struct LiveCronBackgroundTaskRegistrar: CronBackgroundTaskRegistrar {
        func register(
            identifier: String,
            handler: @escaping @Sendable () async -> Void
        ) {}
    }
#endif
