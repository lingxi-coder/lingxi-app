import Foundation
import OSLog
import SwiftUI

#if canImport(ActivityKit)
    import ActivityKit
#endif

#if canImport(BackgroundTasks)
    import BackgroundTasks
#endif

private let conversationContinuedProcessingIdentifier =
    "com.lingxi.code.conversation.continued"

enum ConversationBackgroundLeasePolicy {
    /// The continued-processing API is not itself a lease. A request can be
    /// queued or rejected before a `BGContinuedProcessingTask` is attached, so
    /// finite execution remains the safe fallback until attachment is real.
    static func prefersFiniteAssertion(
        continuedProcessingAvailable: Bool,
        continuedProcessingAttached: Bool
    ) -> Bool {
        !continuedProcessingAvailable || !continuedProcessingAttached
    }

    static func prefersFiniteAssertion(
        continuedProcessingAvailable: Bool,
        leaseState: ConversationContinuedProcessingLeaseState
    ) -> Bool {
        !continuedProcessingAvailable || leaseState != .attached
    }

    static func prefersFiniteAssertion(continuedProcessingAvailable: Bool) -> Bool {
        !continuedProcessingAvailable
    }

    static func usesFiniteAssertion() -> Bool {
        // iOS 26 still starts with a finite assertion. The caller releases it
        // only after the continued-processing request has attached a real task.
        true
    }

    enum ConversationContinuedProcessingLeaseState: Equatable {
        case unavailable
        case queued
        case submissionFailed
        case attached
    }
}

typealias ConversationContinuedProcessingLeaseState =
    ConversationBackgroundLeasePolicy.ConversationContinuedProcessingLeaseState

enum ConversationContinuedProcessingIdentifier {
    static let wildcard = "\(conversationContinuedProcessingIdentifier).*"

    static func make(sessionID: String, turnID: UInt64) -> String {
        "\(conversationContinuedProcessingIdentifier).\(sanitizedSessionComponent(sessionID)).\(turnID)"
    }

    static func turnID(from identifier: String) -> UInt64? {
        guard identifier.hasPrefix("\(conversationContinuedProcessingIdentifier).") else {
            return nil
        }
        return identifier.split(separator: ".").last.flatMap {
            UInt64(String($0))
        }
    }

    private static func sanitizedSessionComponent(_ sessionID: String) -> String {
        let sanitized = sessionID
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .map { character -> Character in
                switch character {
                case "a"..."z", "A"..."Z", "0"..."9", "-":
                    return character
                default:
                    return "-"
                }
            }
        let component = String(sanitized).trimmingCharacters(in: CharacterSet(charactersIn: "-"))
        return component.isEmpty ? "session" : component
    }
}

struct ConversationContinuedProcessingExpiration: Equatable {
    static let identifierKey = "identifier"
    static let turnIDKey = "turn_id"

    let identifier: String
    let turnID: UInt64?

    init(identifier: String, turnID: UInt64?) {
        self.identifier = identifier
        self.turnID = turnID
    }

    init?(userInfo: [AnyHashable: Any]?) {
        guard let identifier = userInfo?[Self.identifierKey] as? String else { return nil }
        let turnID = (userInfo?[Self.turnIDKey] as? String).flatMap(UInt64.init)
        self.init(identifier: identifier, turnID: turnID)
    }

    var userInfo: [AnyHashable: Any] {
        var payload: [AnyHashable: Any] = [Self.identifierKey: identifier]
        if let turnID {
            payload[Self.turnIDKey] = String(turnID)
        }
        return payload
    }
}

struct ConversationLiveActivityReconciliationPlan: Equatable {
    struct Entry: Equatable {
        let activityID: String
        let sessionID: String
    }

    let matchedActivityID: String?
    let staleActivityIDs: [String]
    let shouldStartNew: Bool

    static func make(
        currentActivityID: String?,
        activities: [Entry],
        targetSnapshot: ConversationLiveActivitySnapshot?
    ) -> ConversationLiveActivityReconciliationPlan {
        guard let targetSnapshot else {
            return ConversationLiveActivityReconciliationPlan(
                matchedActivityID: nil,
                staleActivityIDs: currentActivityID.map { [$0] } ?? [],
                shouldStartNew: false
            )
        }
        let matches = activities.filter { $0.sessionID == targetSnapshot.sessionID }
        let matchedActivityID = matches.first(where: { $0.activityID == currentActivityID })?.activityID
            ?? matches.first?.activityID
        var stale = matches.map(\.activityID).filter { $0 != matchedActivityID }
        if let currentActivityID, currentActivityID != matchedActivityID, !stale.contains(currentActivityID) {
            stale.append(currentActivityID)
        }
        return ConversationLiveActivityReconciliationPlan(
            matchedActivityID: matchedActivityID,
            staleActivityIDs: stale,
            shouldStartNew: matchedActivityID == nil
        )
    }
}

extension Notification.Name {
    static let lingxiConversationContinuedProcessingExpired = Notification.Name(
        "lingxi.conversation.continued-processing-expired"
    )
    static let lingxiConversationContinuedProcessingLeaseChanged = Notification.Name(
        "lingxi.conversation.continued-processing-lease-changed"
    )
}

struct ConversationSystemActivityReporter: ConversationActivityReporting {
    let ownerID: String

    @MainActor
    private static var continuedProcessing: [String: ConversationContinuedProcessingController] = [:]

    @MainActor
    private static let liveActivity = ConversationLiveActivityController()

    @MainActor
    func sync(snapshot: ConversationBackgroundSnapshot, scenePhase: ScenePhase) {
        let controller = Self.continuedProcessing[ownerID] ?? ConversationContinuedProcessingController()
        Self.continuedProcessing[ownerID] = controller
        controller.sync(snapshot: snapshot)
        Self.liveActivity.sync(ownerID: ownerID, snapshot: snapshot.preferredLiveActivitySnapshot, scenePhase: scenePhase)
    }

    @MainActor
    func clear() {
        Self.continuedProcessing.removeValue(forKey: ownerID)?.clear()
        Self.liveActivity.clear(ownerID: ownerID)
    }
}

@MainActor
final class ConversationContinuedProcessingController {
    private static let backgroundLog = Logger(
        subsystem: "com.lingxi.code",
        category: "conversation-background"
    )
    private var currentIdentifier: String?
    private var currentTurnID: UInt64?
    private var registeredIdentifiers = Set<String>()

    func sync(snapshot: ConversationBackgroundSnapshot) {
        guard #available(iOS 26.0, *) else { return }
        guard snapshot.requiresExecutionLease,
              let turnID = snapshot.turnToken?.clientTurnId
        else {
            finish(success: true)
            return
        }

        let liveSnapshot = snapshot.preferredLiveActivitySnapshot
            ?? ConversationLiveActivitySnapshot(
                sessionID: snapshot.sessionID,
                turnID: turnID,
                title: String(localized: "chat_background_service_title"),
                subtitle: String(localized: "chat_background_service_text"),
                status: .running,
                updatedAt: .now,
                workspaceKey: snapshot.workspaceKey,
                sessionMode: snapshot.sessionMode.rawValue
            )
        let identifier = ConversationContinuedProcessingIdentifier.make(
            sessionID: liveSnapshot.sessionID,
            turnID: turnID
        )
        if currentIdentifier != identifier {
            finish(success: true)
            submit(identifier: identifier, snapshot: liveSnapshot)
        }
        update(snapshot: liveSnapshot)
    }

    func clear() {
        guard #available(iOS 26.0, *) else { return }
        finish(success: false)
    }

    @available(iOS 26.0, *)
    private func registerIfNeeded(identifier: String) -> Bool {
        guard registeredIdentifiers.insert(identifier).inserted else { return true }
        let registered = BGTaskScheduler.shared.register(
            forTaskWithIdentifier: identifier,
            using: nil
        ) { task in
            guard let task = task as? BGContinuedProcessingTask else {
                task.setTaskCompleted(success: false)
                return
            }
            ConversationContinuedProcessingBridge.shared.attach(task: task)
        }
        if !registered {
            registeredIdentifiers.remove(identifier)
            Self.backgroundLog.error(
                "continued processing register failed id=\(identifier, privacy: .public)"
            )
        }
        return registered
    }

    @available(iOS 26.0, *)
    private func submit(identifier: String, snapshot: ConversationLiveActivitySnapshot) {
        guard registerIfNeeded(identifier: identifier) else { return }
        let request = BGContinuedProcessingTaskRequest(
            identifier: identifier,
            title: snapshot.title,
            subtitle: snapshot.subtitle
        )
        request.strategy = .queue
        do {
            try BGTaskScheduler.shared.submit(request)
            currentIdentifier = identifier
            currentTurnID = snapshot.turnID
        } catch {
            currentIdentifier = nil
            currentTurnID = nil
            Self.backgroundLog.error(
                "continued processing submit failed id=\(identifier, privacy: .public) turn=\(snapshot.turnID ?? 0, privacy: .public) error=\(String(describing: error), privacy: .public)"
            )
            return
        }
        ConversationContinuedProcessingBridge.shared.update(
            identifier: identifier,
            title: snapshot.title,
            subtitle: snapshot.subtitle,
            progress: 0.05
        )
    }

    @available(iOS 26.0, *)
    private func update(snapshot: ConversationLiveActivitySnapshot) {
        guard let currentIdentifier else { return }
        let progress = snapshot.status == .running ? 0.5 : 1.0
        ConversationContinuedProcessingBridge.shared.update(
            identifier: currentIdentifier,
            title: snapshot.title,
            subtitle: snapshot.subtitle,
            progress: progress
        )
    }

    @available(iOS 26.0, *)
    private func finish(success: Bool) {
        guard let identifier = currentIdentifier else { return }
        ConversationContinuedProcessingBridge.shared.finish(
            identifier: identifier,
            success: success
        )
        BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: identifier)
        self.currentIdentifier = nil
        currentTurnID = nil
    }
}

@MainActor
final class ConversationLiveActivityController {
    func sync(ownerID: String, snapshot: ConversationLiveActivitySnapshot?, scenePhase _: ScenePhase) {
        guard #available(iOS 18.0, *) else { return }
        if #available(iOS 26.0, *) {
            // iOS 26 routes the active lease through BGContinuedProcessingTask.
            // Any lingering Live Activity from a prior app version is orphaned
            // state and should be cleared rather than mirrored.
            Task { await ActivityKitConversationDriver.shared.sync(ownerID: ownerID, snapshot: nil) }
            return
        }
        Task { await ActivityKitConversationDriver.shared.sync(ownerID: ownerID, snapshot: snapshot) }
    }

    func clear(ownerID: String) {
        guard #available(iOS 18.0, *) else { return }
        Task { await ActivityKitConversationDriver.shared.sync(ownerID: ownerID, snapshot: nil) }
    }
}

#if canImport(ActivityKit)
    @available(iOS 18.0, *)
    private actor ActivityKitConversationDriver {
        static let shared = ActivityKitConversationDriver()

        private var activitiesByOwner: [String: Activity<ConversationLiveActivityAttributes>] = [:]

        func sync(ownerID: String, snapshot: ConversationLiveActivitySnapshot?) async {
            let activities = Array(Activity<ConversationLiveActivityAttributes>.activities)
            let plan = ConversationLiveActivityReconciliationPlan.make(
                currentActivityID: activitiesByOwner[ownerID]?.id,
                activities: activities.map { .init(activityID: $0.id, sessionID: $0.attributes.sessionID) },
                targetSnapshot: snapshot
            )
            for stale in activities where plan.staleActivityIDs.contains(stale.id) {
                await stale.end(nil, dismissalPolicy: .immediate)
            }
            guard let snapshot else {
                activitiesByOwner.removeValue(forKey: ownerID)
                return
            }
            let current = plan.matchedActivityID.flatMap { id in activities.first { $0.id == id } }
            guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
            if let current {
                activitiesByOwner[ownerID] = current
                if snapshot.status.isTerminal {
                    await current.end(content(for: snapshot), dismissalPolicy: snapshot.status == .completed
                        ? .after(.now.addingTimeInterval(300)) : .default)
                    activitiesByOwner.removeValue(forKey: ownerID)
                } else {
                    await current.update(content(for: snapshot))
                }
                return
            }
            do {
                let activity = try Activity<ConversationLiveActivityAttributes>.request(
                    attributes: ConversationLiveActivityAttributes(sessionID: snapshot.sessionID),
                    content: content(for: snapshot), pushType: nil
                )
                activitiesByOwner[ownerID] = activity
                if snapshot.status.isTerminal {
                    await activity.end(content(for: snapshot), dismissalPolicy: snapshot.status == .completed
                        ? .after(.now.addingTimeInterval(300)) : .default)
                    activitiesByOwner.removeValue(forKey: ownerID)
                }
            } catch {
                activitiesByOwner.removeValue(forKey: ownerID)
            }
        }

        private func content(
            for snapshot: ConversationLiveActivitySnapshot
        ) -> ActivityContent<ConversationLiveActivityAttributes.ContentState> {
            ActivityContent(
                state: ConversationLiveActivityAttributes.ContentState(
                    title: snapshot.title,
                    subtitle: snapshot.subtitle,
                    status: snapshot.status,
                    turnID: snapshot.turnID,
                    updatedAt: snapshot.updatedAt,
                    workspaceKey: snapshot.workspaceKey,
                    sessionMode: snapshot.sessionMode
                ),
                staleDate: nil
            )
        }


    }
#endif

#if canImport(BackgroundTasks)
    @available(iOS 26.0, *)
    private final class ConversationContinuedProcessingBridge: @unchecked Sendable {
        static let shared = ConversationContinuedProcessingBridge()

        private struct ContinuedProcessingLease {
            let task: BGContinuedProcessingTask
            let expiration: ConversationContinuedProcessingExpiration
        }

        private let lock = NSLock()
        private var tasks: [String: ContinuedProcessingLease] = [:]

        func attach(task: BGContinuedProcessingTask) {
            let expiration = ConversationContinuedProcessingExpiration(
                identifier: task.identifier,
                turnID: ConversationContinuedProcessingIdentifier.turnID(from: task.identifier)
            )
            lock.lock()
            tasks[task.identifier] = ContinuedProcessingLease(task: task, expiration: expiration)
            lock.unlock()
            postLeaseChange(expiration: expiration, attached: true)
            task.expirationHandler = { [weak self] in
                self?.expire(identifier: task.identifier)
            }
        }

        func update(identifier: String, title: String, subtitle: String, progress: Double) {
            lock.lock()
            let lease = tasks[identifier]
            lock.unlock()
            guard let task = lease?.task else { return }
            task.updateTitle(title, subtitle: subtitle)
            task.progress.totalUnitCount = 100
            task.progress.completedUnitCount = max(0, min(100, Int64(progress * 100)))
        }

        func finish(identifier: String, success: Bool) {
            lock.lock()
            let lease = tasks.removeValue(forKey: identifier)
            lock.unlock()
            if let expiration = lease?.expiration {
                postLeaseChange(expiration: expiration, attached: false)
            }
            lease?.task.setTaskCompleted(success: success)
        }

        private func expire(identifier: String) {
            lock.lock()
            let lease = tasks.removeValue(forKey: identifier)
            lock.unlock()
            if let expiration = lease?.expiration {
                postLeaseChange(expiration: expiration, attached: false)
                NotificationCenter.default.post(
                    name: .lingxiConversationContinuedProcessingExpired,
                    object: nil,
                    userInfo: expiration.userInfo
                )
            }
            lease?.task.setTaskCompleted(success: false)
        }

        private func postLeaseChange(
            expiration: ConversationContinuedProcessingExpiration,
            attached: Bool
        ) {
            var userInfo = expiration.userInfo
            userInfo["attached"] = attached
            NotificationCenter.default.post(
                name: .lingxiConversationContinuedProcessingLeaseChanged,
                object: nil,
                userInfo: userInfo
            )
        }
    }
#endif
