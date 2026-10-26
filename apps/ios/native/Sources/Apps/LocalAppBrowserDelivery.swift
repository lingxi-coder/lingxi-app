import Foundation
import UIKit

struct LocalAppBrowserDeliveryScope: Hashable, Sendable {
    let appID: String
    let revision: UInt64
}

@MainActor
struct LocalAppBrowserDeliveryDeadline {
    private let cancelAction: () -> Void

    init(cancel: @escaping () -> Void) {
        cancelAction = cancel
    }

    func cancel() {
        cancelAction()
    }
}

/// A short, nonrenewable UIKit assertion for opening or finishing a Local App
/// page. A completed openURL action is only a routing result, not a page ACK.
@MainActor
final class LocalAppBrowserDeliveryController {
    typealias TaskIdentifier = UIBackgroundTaskIdentifier
    typealias ExpirationHandler = @MainActor @Sendable () -> Void
    typealias BeginTask = @MainActor (@escaping ExpirationHandler) -> TaskIdentifier
    typealias EndTask = @MainActor (TaskIdentifier) -> Void
    typealias ScheduleDeadline = @MainActor (
        Duration,
        @escaping ExpirationHandler
    ) -> LocalAppBrowserDeliveryDeadline

    enum Acquisition: Equatable {
        case started
        case alreadyActive
        case unavailable
    }

    static let maximumWindow = Duration.seconds(15)

    private struct Lease {
        let identifier: TaskIdentifier
        let generation: UUID
        var deadline: LocalAppBrowserDeliveryDeadline?
    }

    private let beginTask: BeginTask
    private let endTask: EndTask
    private let scheduleDeadline: ScheduleDeadline
    private var generations: [LocalAppBrowserDeliveryScope: UUID] = [:]
    private var leases: [LocalAppBrowserDeliveryScope: Lease] = [:]

    init(
        beginTask: @escaping BeginTask,
        endTask: @escaping EndTask,
        scheduleDeadline: ScheduleDeadline? = nil
    ) {
        self.beginTask = beginTask
        self.endTask = endTask
        self.scheduleDeadline = scheduleDeadline ?? Self.scheduleSystemDeadline
    }

    static func live() -> LocalAppBrowserDeliveryController {
        let application = UIApplication.shared
        return LocalAppBrowserDeliveryController(
            beginTask: { expirationHandler in
                application.beginBackgroundTask(withName: "LocalAppBrowserDelivery") {
                    // UIKit invokes this callback on the main actor; end the token synchronously.
                    expirationHandler()
                }
            },
            endTask: { application.endBackgroundTask($0) }
        )
    }

    @discardableResult
    func begin(_ scope: LocalAppBrowserDeliveryScope) -> Acquisition {
        guard generations[scope] == nil, leases[scope] == nil else {
            return .alreadyActive
        }

        let generation = UUID()
        generations[scope] = generation
        let identifier = beginTask { [weak self] in
            self?.finish(scope, generation: generation)
        }
        guard identifier != .invalid else {
            if generations[scope] == generation {
                generations.removeValue(forKey: scope)
            }
            return .unavailable
        }
        guard generations[scope] == generation else {
            endTask(identifier)
            return .unavailable
        }

        leases[scope] = Lease(identifier: identifier, generation: generation)
        let deadline = scheduleDeadline(Self.maximumWindow) { [weak self] in
            self?.finish(scope, generation: generation)
        }
        guard var lease = leases[scope], lease.generation == generation else {
            deadline.cancel()
            return .unavailable
        }
        lease.deadline = deadline
        leases[scope] = lease
        return .started
    }

    @discardableResult
    func openPage(
        scope: LocalAppBrowserDeliveryScope,
        open: @MainActor () -> Void
    ) -> Acquisition {
        let acquisition = begin(scope)
        guard acquisition != .unavailable else { return acquisition }
        open()
        return acquisition
    }

    /// Begins a result-delivery window before completing the continued task.
    /// The bounded assertion is independent from the model task's success.
    @discardableResult
    func finishBackgroundTaskAfterStartingResultDelivery(
        scope: LocalAppBrowserDeliveryScope,
        finish: @MainActor () async -> Void
    ) async -> Acquisition {
        let acquisition = begin(scope)
        await finish()
        return acquisition
    }

    func end(_ scope: LocalAppBrowserDeliveryScope) {
        guard let generation = generations[scope] else { return }
        finish(scope, generation: generation)
    }

    func endAll(appID: String) {
        let scopes = generations.keys.filter { $0.appID == appID }
        for scope in scopes {
            if let generation = generations[scope] {
                finish(scope, generation: generation)
            }
        }
    }

    func endStale(appID: String, keepingRevision revision: UInt64) {
        let scopes = generations.keys.filter {
            $0.appID == appID && $0.revision != revision
        }
        for scope in scopes {
            if let generation = generations[scope] {
                finish(scope, generation: generation)
            }
        }
    }

    func isActive(_ scope: LocalAppBrowserDeliveryScope) -> Bool {
        leases[scope] != nil
    }

    private func finish(
        _ scope: LocalAppBrowserDeliveryScope,
        generation: UUID
    ) {
        guard generations[scope] == generation else { return }
        generations.removeValue(forKey: scope)
        guard let lease = leases.removeValue(forKey: scope),
              lease.generation == generation else { return }
        lease.deadline?.cancel()
        endTask(lease.identifier)
    }

    private static func scheduleSystemDeadline(
        _ duration: Duration,
        expirationHandler: @escaping ExpirationHandler
    ) -> LocalAppBrowserDeliveryDeadline {
        let task = Task { @MainActor in
            do {
                try await Task.sleep(for: duration)
            } catch {
                return
            }
            expirationHandler()
        }
        return LocalAppBrowserDeliveryDeadline { task.cancel() }
    }
}

enum LocalAppBrowserResultDeliveryPolicy {
    static func shouldDeliver(
        taskState: String?,
        terminalEvent: LocalAppJSON?
    ) -> Bool {
        if taskState == "cancelled" || taskState == "interrupted" {
            return false
        }

        guard let terminalEvent else {
            return taskState == "succeeded" || taskState == "failed"
        }
        switch LocalAppAgentRunEvent.type(terminalEvent) {
        case "RUN_ERROR":
            return taskState != "succeeded"
        case "RUN_FINISHED":
            guard let outcome = outcomeType(terminalEvent) else {
                return taskState == "succeeded"
            }
            guard outcome == "success" else { return false }
            return taskState != "failed"
        default:
            return false
        }
    }

    static func isSuccessfulRun(
        taskState: String?,
        terminalEvent: LocalAppJSON,
        runID: String
    ) -> Bool {
        guard LocalAppAgentRunEvent.isSuccessfulTerminal(terminalEvent, runID: runID) else {
            return false
        }
        if let taskState,
           ["succeeded", "failed", "cancelled", "interrupted"].contains(taskState) {
            return taskState == "succeeded"
        }
        return true
    }

    private static func outcomeType(_ event: LocalAppJSON) -> String? {
        guard case .object(let fields) = event,
              case .object(let outcome)? = fields["outcome"],
              case .string(let type)? = outcome["type"] else { return nil }
        return type
    }
}
