import Foundation
import SwiftUI

#if canImport(UserNotifications)
    import UserNotifications
#endif

enum ConversationNotificationRoute {
    static let routeKey = "lingxi.route"
    static let sessionIDKey = "lingxi.conversation.session_id"
    static let turnIDKey = "lingxi.conversation.turn_id"
    static let eventClassKey = "lingxi.conversation.event_class"
    static let routeValue = "conversation"

    static func userInfo(
        sessionID: String,
        turnID: UInt64?,
        eventClass: ConversationNotificationEventClass
    ) -> [AnyHashable: Any] {
        var userInfo: [AnyHashable: Any] = [
            routeKey: routeValue,
            sessionIDKey: sessionID,
            eventClassKey: eventClass.rawValue,
        ]
        if let turnID {
            userInfo[turnIDKey] = String(turnID)
        }
        return userInfo
    }

    static func appAction(from userInfo: [AnyHashable: Any]) -> LingxiAppAction? {
        guard (userInfo[routeKey] as? String) == routeValue else { return nil }
        guard
            let sessionID = (userInfo[sessionIDKey] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines),
            !sessionID.isEmpty
        else { return nil }
        let turnID = (userInfo[turnIDKey] as? String).flatMap(UInt64.init)
        return .openConversation(sessionID: sessionID, turnID: turnID)
    }
}

enum ConversationNotificationEventClass: String, Equatable, Sendable {
    case completed
    case failed
    case waitingForUser
    case pausedRecoverable
}

struct ConversationBackgroundSnapshot: Equatable {
    let sessionID: String
    let turnToken: ConversationTurnToken?
    let turnCompletion: ConversationTurnCompletion?
    let pendingQuestions: [ConversationPendingQuestion]
    let backgroundTasks: [BackgroundTaskSnapshot]
    let requiresExecutionLease: Bool
}

struct ConversationNotificationPayload {
    let identifier: String
    let title: String
    let body: String
    let userInfo: [AnyHashable: Any]

    static func make(
        sessionID: String,
        turnID: UInt64?,
        eventClass: ConversationNotificationEventClass,
        // Distinguishes notifications that share a session, turn and class.
        // Background tasks all posted with `turnID: nil`, so every task in a
        // session collapsed onto ONE identifier and `post`'s delivered-set
        // guard silently swallowed every task after the first.
        discriminator: String? = nil
    ) -> ConversationNotificationPayload {
        let title: String
        let body: String
        switch eventClass {
        case .completed:
            title = String(localized: "chat_background_completed_title")
            body = String(localized: "chat_background_completed_text")
        case .failed:
            title = String(localized: "chat_background_failed_title")
            body = String(localized: "chat_background_failed_text")
        case .waitingForUser:
            title = String(localized: "chat_background_waiting_title")
            body = String(localized: "chat_background_waiting_text")
        case .pausedRecoverable:
            title = String(localized: "chat_background_paused_title")
            body = String(localized: "chat_background_paused_text")
        }
        let identifier = [
            "conversation",
            sessionID,
            turnID.map(String.init) ?? "none",
            eventClass.rawValue,
            discriminator ?? "-",
        ].joined(separator: ":")
        return ConversationNotificationPayload(
            identifier: identifier,
            title: title,
            body: body,
            userInfo: ConversationNotificationRoute.userInfo(
                sessionID: sessionID,
                turnID: turnID,
                eventClass: eventClass
            )
        )
    }
}

protocol ConversationNotificationScheduling: Sendable {
    func deliver(_ payload: ConversationNotificationPayload) async
}

struct UserNotificationConversationScheduler: ConversationNotificationScheduling {
    func deliver(_ payload: ConversationNotificationPayload) async {
        #if canImport(UserNotifications)
            let center = UNUserNotificationCenter.current()
            let settings = await center.notificationSettings()
            switch settings.authorizationStatus {
            case .authorized, .provisional, .ephemeral:
                break
            case .denied, .notDetermined:
                return
            default:
                return
            }
            let content = UNMutableNotificationContent()
            content.title = payload.title
            content.body = payload.body
            content.sound = .default
            content.userInfo = payload.userInfo
            let request = UNNotificationRequest(
                identifier: payload.identifier,
                content: content,
                trigger: nil
            )
            try? await center.add(request)
        #endif
    }
}

protocol ConversationActivityReporting: Sendable {
    @MainActor
    func sync(snapshot: ConversationBackgroundSnapshot, scenePhase: ScenePhase)

    @MainActor
    func clear()
}

struct NoopConversationActivityReporter: ConversationActivityReporting {
    @MainActor
    func sync(snapshot _: ConversationBackgroundSnapshot, scenePhase _: ScenePhase) {}

    @MainActor
    func clear() {}
}

@MainActor
final class ConversationBackgroundAlertController {
    private let scheduler: ConversationNotificationScheduling
    private let activityReporter: ConversationActivityReporting
    private var scenePhase: ScenePhase = .active
    private var deliveredNotificationIDs = Set<String>()
    private var pendingQuestionIDs = Set<UInt64>()
    private var taskStatuses: [String: BackgroundTaskSnapshot.Status] = [:]

    init(
        scheduler: ConversationNotificationScheduling,
        activityReporter: ConversationActivityReporting = NoopConversationActivityReporter()
    ) {
        self.scheduler = scheduler
        self.activityReporter = activityReporter
    }

    static func live() -> ConversationBackgroundAlertController {
        ConversationBackgroundAlertController(
            scheduler: UserNotificationConversationScheduler(),
            activityReporter: liveConversationActivityReporter()
        )
    }

    func setScenePhase(_ phase: ScenePhase) {
        scenePhase = phase
    }

    func sync(_ snapshot: ConversationBackgroundSnapshot) {
        activityReporter.sync(snapshot: snapshot, scenePhase: scenePhase)
        guard !snapshot.sessionID.isEmpty else { return }

        let previousTaskStatuses = taskStatuses
        let previousQuestionIDs = pendingQuestionIDs
        // `Dictionary(uniqueKeysWithValues:)` TRAPS on a repeated key, and
        // nothing upstream guarantees the engine's task list is unique — a
        // workflow re-listed after a resume while still present from the live
        // stream, or two sub-agent rows sharing a run id, would hard-crash the
        // app on the main actor. Last row wins, which matches the live stream's
        // ordering.
        taskStatuses = Dictionary(
            snapshot.backgroundTasks.map { ($0.id, $0.status) },
            uniquingKeysWith: { _, latest in latest }
        )
        pendingQuestionIDs = Set(snapshot.pendingQuestions.map(\.requestId))

        guard scenePhase != .active else { return }

        if let completion = snapshot.turnCompletion {
            switch completion.outcome {
            case .completed:
                post(
                    sessionID: snapshot.sessionID,
                    turnID: completion.token.clientTurnId,
                    eventClass: .completed
                )
            case .failed:
                post(
                    sessionID: snapshot.sessionID,
                    turnID: completion.token.clientTurnId,
                    eventClass: .failed
                )
            case .maxTurns, .cancelled:
                break
            }
        }

        let newQuestionIDs = Set(snapshot.pendingQuestions.map(\.requestId))
            .subtracting(previousQuestionIDs)
        if !newQuestionIDs.isEmpty || (!snapshot.pendingQuestions.isEmpty && previousQuestionIDs.isEmpty) {
            post(
                sessionID: snapshot.sessionID,
                turnID: snapshot.turnToken?.clientTurnId,
                eventClass: .waitingForUser
            )
        }

        for task in snapshot.backgroundTasks {
            let oldStatus = previousTaskStatuses[task.id]
            guard oldStatus != task.status else { continue }
            // `turnID: nil` also broke the tap-through: the one notification
            // that did fire routed to the session with no turn, whichever task
            // had finished.
            let turnID = snapshot.turnToken?.clientTurnId
            switch task.status {
            case .completed:
                post(sessionID: snapshot.sessionID, turnID: turnID, eventClass: .completed, discriminator: task.id)
            case .failed:
                post(sessionID: snapshot.sessionID, turnID: turnID, eventClass: .failed, discriminator: task.id)
            case .paused:
                post(sessionID: snapshot.sessionID, turnID: turnID, eventClass: .pausedRecoverable, discriminator: task.id)
            case .pending, .running, .cancelled:
                break
            }
        }
    }

    func markRecoverablePause(sessionID: String, turnToken: ConversationTurnToken?) {
        guard scenePhase != .active else { return }
        post(
            sessionID: sessionID,
            turnID: turnToken?.clientTurnId,
            eventClass: .pausedRecoverable
        )
    }

    func clear() {
        activityReporter.clear()
    }

    private func post(
        sessionID: String,
        turnID: UInt64?,
        eventClass: ConversationNotificationEventClass,
        discriminator: String? = nil
    ) {
        let payload = ConversationNotificationPayload.make(
            sessionID: sessionID,
            turnID: turnID,
            eventClass: eventClass,
            discriminator: discriminator
        )
        guard deliveredNotificationIDs.insert(payload.identifier).inserted else { return }
        Task { await scheduler.deliver(payload) }
    }
}

func liveConversationActivityReporter() -> ConversationActivityReporting {
    ConversationSystemActivityReporter()
}

extension ConversationBackgroundSnapshot {
    var preferredLiveActivitySnapshot: ConversationLiveActivitySnapshot? {
        guard !sessionID.isEmpty else { return nil }
        if !pendingQuestions.isEmpty {
            return ConversationLiveActivitySnapshot(
                sessionID: sessionID,
                turnID: turnToken?.clientTurnId,
                title: String(localized: "chat_background_waiting_title"),
                subtitle: String(localized: "chat_background_waiting_text"),
                status: .waiting,
                updatedAt: .now
            )
        }
        if requiresExecutionLease {
            return ConversationLiveActivitySnapshot(
                sessionID: sessionID,
                turnID: turnToken?.clientTurnId,
                title: String(localized: "chat_background_service_title"),
                subtitle: String(localized: "chat_background_service_text"),
                status: .running,
                updatedAt: .now
            )
        }
        if let completion = turnCompletion {
            let status: ConversationLiveActivitySnapshot.Status?
            switch completion.outcome {
            case .completed:
                status = .completed
            case .failed:
                status = .failed
            case .maxTurns, .cancelled:
                status = nil
            }
            if let status {
                return ConversationLiveActivitySnapshot(
                    sessionID: sessionID,
                    turnID: completion.token.clientTurnId,
                    title: status == .completed
                        ? String(localized: "chat_background_completed_title")
                        : String(localized: "chat_background_failed_title"),
                    subtitle: status == .completed
                        ? String(localized: "chat_background_completed_text")
                        : String(localized: "chat_background_failed_text"),
                    status: status,
                    updatedAt: .now
                )
            }
        }
        if backgroundTasks.contains(where: { $0.status == .paused }) {
            return ConversationLiveActivitySnapshot(
                sessionID: sessionID,
                turnID: nil,
                title: String(localized: "chat_background_paused_title"),
                subtitle: String(localized: "chat_background_paused_text"),
                status: .paused,
                updatedAt: .now
            )
        }
        if backgroundTasks.contains(where: { $0.status == .failed }) {
            return ConversationLiveActivitySnapshot(
                sessionID: sessionID,
                turnID: nil,
                title: String(localized: "chat_background_failed_title"),
                subtitle: String(localized: "chat_background_failed_text"),
                status: .failed,
                updatedAt: .now
            )
        }
        if backgroundTasks.contains(where: { $0.status == .completed }) {
            return ConversationLiveActivitySnapshot(
                sessionID: sessionID,
                turnID: nil,
                title: String(localized: "chat_background_completed_title"),
                subtitle: String(localized: "chat_background_completed_text"),
                status: .completed,
                updatedAt: .now
            )
        }
        return nil
    }
}
