import Foundation

// The Host's background wake-up for the engine's due tasks. It is not Local App UI: the engine owns the schedule, and
// this only lends it iOS's BackgroundTasks entry point (the identifier is registered in Info.plist).
#if canImport(BackgroundTasks)
    import BackgroundTasks
#endif

let localAppBackgroundTaskIdentifier = "com.lingxi.code.localapps.background"

/// Process-global bridge between Apple's wake-up callback and the Host-owned
/// local-app background executor. Kept in an existing Xcode source so the
/// store/full targets share the same explicit project membership.
final class LocalAppBackgroundTaskBridge: @unchecked Sendable {
    static let shared = LocalAppBackgroundTaskBridge()

    typealias Handler = @Sendable () async -> Void

    private let lock = NSLock()
    private var registered = false
    private var handler: Handler?
    private var rescheduler: Handler?
    private var waiters: [UUID: CheckedContinuation<Handler?, Never>] = [:]

    func registerAtLaunch() {
        lock.lock()
        let shouldRegister = !registered
        registered = true
        lock.unlock()
        guard shouldRegister else { return }
        #if canImport(BackgroundTasks)
            BGTaskScheduler.shared.register(
                forTaskWithIdentifier: localAppBackgroundTaskIdentifier,
                using: nil
            ) { [weak self] task in
                guard let processing = task as? BGProcessingTask else {
                    task.setTaskCompleted(success: false)
                    return
                }
                let worker = Task {
                    await self?.runHandler()
                    processing.setTaskCompleted(success: !Task.isCancelled)
                }
                processing.expirationHandler = {
                    worker.cancel()
                    self?.schedule(
                        earliestAtMs: UInt64(Date().timeIntervalSince1970 * 1000) + 15 * 60 * 1_000
                    )
                }
            }
        #endif
        schedule(earliestAtMs: UInt64(Date().timeIntervalSince1970 * 1000) + 15 * 60 * 1_000)
    }

    func bind(_ handler: @escaping Handler, rescheduler: Handler? = nil) {
        lock.lock()
        self.handler = handler
        self.rescheduler = rescheduler
        let continuations = Array(waiters.values)
        waiters.removeAll()
        lock.unlock()
        continuations.forEach { $0.resume(returning: handler) }
    }

    func rescheduleAfterForegroundMutation() {
        lock.lock()
        let rescheduler = self.rescheduler
        lock.unlock()
        guard let rescheduler else { return }
        Task { await rescheduler() }
    }

    func schedule(earliestAtMs: UInt64?) {
        #if canImport(BackgroundTasks)
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: localAppBackgroundTaskIdentifier)
            guard let earliestAtMs else { return }
            let request = BGProcessingTaskRequest(identifier: localAppBackgroundTaskIdentifier)
            request.requiresNetworkConnectivity = false
            request.requiresExternalPower = false
            request.earliestBeginDate = Date(timeIntervalSince1970: TimeInterval(earliestAtMs) / 1000)
            try? BGTaskScheduler.shared.submit(request)
        #else
            _ = earliestAtMs
        #endif
    }

    private func runHandler() async {
        guard let handler = await resolveHandler() else { return }
        await handler()
    }

    private func resolveHandler() async -> Handler? {
        if let handler = currentHandler() {
            return handler
        }
        let id = UUID()
        return await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                if let handler = currentHandler() {
                    continuation.resume(returning: handler)
                } else if Task.isCancelled {
                    continuation.resume(returning: nil)
                } else {
                    storeWaiter(continuation, id: id)
                }
            }
        } onCancel: {
            let continuation = removeWaiter(id: id)
            continuation?.resume(returning: nil)
        }
    }

    private func currentHandler() -> Handler? {
        lock.lock()
        defer { lock.unlock() }
        return handler
    }

    private func storeWaiter(_ continuation: CheckedContinuation<Handler?, Never>, id: UUID) {
        lock.lock()
        defer { lock.unlock() }
        waiters[id] = continuation
    }

    private func removeWaiter(id: UUID) -> CheckedContinuation<Handler?, Never>? {
        lock.lock()
        defer { lock.unlock() }
        return waiters.removeValue(forKey: id)
    }
}
