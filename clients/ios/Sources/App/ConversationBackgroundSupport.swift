import Foundation
import SwiftUI

#if canImport(UserNotifications)
    import UserNotifications
#endif

enum ConversationNotificationRoute {
    static let routeKey = "lingxi.route"
    static let sessionIDKey = "lingxi.conversation.session_id"
    static let turnIDKey = "lingxi.conversation.turn_id"
    static let workspaceKeyKey = "lingxi.conversation.workspace_key"
    static let sessionModeKey = "lingxi.conversation.session_mode"
    static let eventClassKey = "lingxi.conversation.event_class"
    static let routeValue = "conversation"

    static func userInfo(
        sessionID: String,
        turnID: UInt64?,
        workspaceKey: String = "global",
        sessionMode: SessionMode = .code,
        eventClass: ConversationNotificationEventClass
    ) -> [AnyHashable: Any] {
        var userInfo: [AnyHashable: Any] = [
            routeKey: routeValue,
            sessionIDKey: sessionID,
            eventClassKey: eventClass.rawValue,
            workspaceKeyKey: workspaceKey,
            sessionModeKey: sessionMode.rawValue,
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
        let workspaceKey = (userInfo[workspaceKeyKey] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let mode = (userInfo[sessionModeKey] as? String)
            .flatMap(SessionMode.init(rawValue:)) ?? .code
        return .openConversation(
            sessionID: sessionID,
            turnID: turnID,
            workspaceKey: workspaceKey?.isEmpty == false ? workspaceKey : nil,
            mode: mode
        )
    }
}

enum ConversationNotificationEventClass: String, Equatable, Sendable {
    case completed
    case failed
    case waitingForUser
    case pausedRecoverable
    /// A tool is parked on a permission decision the user has not made yet.
    case needsPermission
}

/// A parked permission request, reduced to what a notification needs.
///
/// The tool name is carried rather than re-derived: the prompt's own title has
/// already been through a localized format string, so recovering the one word
/// from it would give a different answer in each locale.
struct ConversationPendingPermission: Identifiable, Equatable, Hashable {
    let requestId: UInt64
    let toolName: String

    var id: UInt64 { requestId }
}

struct ConversationBackgroundSnapshot: Equatable {
    let sessionID: String
    let turnToken: ConversationTurnToken?
    let turnCompletion: ConversationTurnCompletion?
    let pendingQuestions: [ConversationPendingQuestion]
    let pendingPermissions: [ConversationPendingPermission]
    let backgroundTasks: [BackgroundTaskSnapshot]
    let requiresExecutionLease: Bool
    let workspaceKey: String
    let sessionMode: SessionMode

    init(
        sessionID: String,
        turnToken: ConversationTurnToken?,
        turnCompletion: ConversationTurnCompletion?,
        pendingQuestions: [ConversationPendingQuestion],
        pendingPermissions: [ConversationPendingPermission] = [],
        backgroundTasks: [BackgroundTaskSnapshot],
        requiresExecutionLease: Bool,
        workspaceKey: String = "global",
        sessionMode: SessionMode = .code
    ) {
        self.sessionID = sessionID
        self.turnToken = turnToken
        self.turnCompletion = turnCompletion
        self.pendingQuestions = pendingQuestions
        self.pendingPermissions = pendingPermissions
        self.backgroundTasks = backgroundTasks
        self.requiresExecutionLease = requiresExecutionLease
        self.workspaceKey = workspaceKey
        self.sessionMode = sessionMode
    }
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
        workspaceKey: String = "global",
        sessionMode: SessionMode = .code,
        // Distinguishes notifications that share a session, turn and class.
        // Background tasks all posted with `turnID: nil`, so every task in a
        // session collapsed onto ONE identifier and `post`'s delivered-set
        // guard silently swallowed every task after the first.
        discriminator: String? = nil,
        /// Fills the one parameterized body (`needsPermission`'s tool name).
        detail: String? = nil
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
        case .needsPermission:
            title = String(localized: "chat_background_permission_title")
            // The only parameterized body here. `detail` carries the tool name;
            // it is passed rather than parsed back out of the localized prompt
            // title, which would give a different answer in each locale.
            body = String(
                format: String(localized: "chat_background_permission_text %@"),
                detail ?? ""
            )
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
                workspaceKey: workspaceKey,
                sessionMode: sessionMode,
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

/// How the two armed delays wait. Injectable so tests need no wall clock.
typealias ConversationNotificationSleeper = @Sendable (UInt64) async -> Void

@MainActor
final class ConversationBackgroundAlertController {
    private let scheduler: ConversationNotificationScheduling
    private let activityReporter: ConversationActivityReporting
    private var scenePhase: ScenePhase = .active
    private var deliveredNotificationIDs = Set<String>()
    private var pendingQuestionIDs = Set<UInt64>()
    private var pendingPermissionIDs = Set<UInt64>()
    private var taskStatuses: [String: BackgroundTaskSnapshot.Status] = [:]

    /// Seeded from disk so a notification decided before the settings screen
    /// was ever opened still honours what the user chose in a previous launch.
    private var preferences: NotifConfig
    private let sleeper: ConversationNotificationSleeper

    /// Armed when a turn finishes, cancelled by a new turn or by the user
    /// coming back. Upstream's idle timer.
    private var idleTask: Task<Void, Never>?

    /// The completion this `idleTask` is counting down for. `sync` re-runs on
    /// every snapshot change (a background task flipping state, a question or
    /// permission arriving), and `turnCompletion` stays set until a new turn —
    /// so without this, each of those re-armed the timer for the SAME finished
    /// turn and a session whose snapshot changes faster than the threshold
    /// never got its idle notification at all.
    private var armedIdleCompletion: (turnID: UInt64?, eventClass: ConversationNotificationEventClass)?

    /// request_id -> armed 6s permission delay.
    private var permissionTasks: [UInt64: Task<Void, Never>] = [:]

    init(
        scheduler: ConversationNotificationScheduling,
        activityReporter: ConversationActivityReporting = NoopConversationActivityReporter(),
        /// Seam for the two armed delays. Injected so a test can exercise the
        /// fire-time re-checks without a real 60-second wait — the floor on
        /// `messageIdleNotifThresholdMs` is 5s, which is still far too slow for
        /// a unit test, and shortening the floor for tests would weaken the
        /// production clamp.
        sleeper: @escaping ConversationNotificationSleeper = { try? await Task.sleep(nanoseconds: $0) },
        /// Injected so a test never reads (or writes) the real user defaults.
        preferences: NotifConfig = NotificationPreferencesStore.load()
    ) {
        self.scheduler = scheduler
        self.activityReporter = activityReporter
        self.sleeper = sleeper
        self.preferences = preferences
    }

    static func live() -> ConversationBackgroundAlertController {
        ConversationBackgroundAlertController(
            scheduler: UserNotificationConversationScheduler(),
            activityReporter: liveConversationActivityReporter()
        )
    }

    func setScenePhase(_ phase: ScenePhase) {
        // Returning to the foreground is the strongest available proof the user
        // came back — upstream's `getLastInteractionTime() > lastQueryCompletionTime`
        // clause. The armed idle alert is then wrong, not merely early.
        if phase == .active { cancelIdleAlert() }
        scenePhase = phase
    }

    func setPreferences(_ config: NotifConfig) {
        preferences = config
        guard !config.enabled else { return }
        cancelIdleAlert()
        for task in permissionTasks.values { task.cancel() }
        permissionTasks.removeAll()
    }

    private func cancelIdleAlert() {
        idleTask?.cancel()
        idleTask = nil
        armedIdleCompletion = nil
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

        // A finished turn ARMS the idle alert; it does not post one. Upstream
        // has no "the turn finished" notification — it waits
        // `messageIdleNotifThresholdMs` and only then says the session is
        // waiting, and only if the user never came back. Coming back cancels
        // this two ways: `setScenePhase(.active)`, and the foreground guard
        // above on the next `sync`.
        if let completion = snapshot.turnCompletion {
            switch completion.outcome {
            case .completed:
                armIdleAlert(snapshot, turnID: completion.token.clientTurnId, eventClass: .completed)
            case .failed:
                armIdleAlert(snapshot, turnID: completion.token.clientTurnId, eventClass: .failed)
            case .maxTurns, .cancelled:
                break
            }
        }

        // New permission requests arm upstream's 6-second delay; ones that have
        // gone away settle it. Only a newly-arrived id arms — re-emitting the
        // same request must not restart the clock.
        let permissionIDs = Set(snapshot.pendingPermissions.map(\.requestId))
        for permission in snapshot.pendingPermissions where !pendingPermissionIDs.contains(permission.requestId) {
            armPermissionAlert(snapshot, permission: permission)
        }
        for settled in pendingPermissionIDs.subtracting(permissionIDs) {
            permissionTasks.removeValue(forKey: settled)?.cancel()
        }
        pendingPermissionIDs = permissionIDs

        let newQuestionIDs = Set(snapshot.pendingQuestions.map(\.requestId))
            .subtracting(previousQuestionIDs)
        if !newQuestionIDs.isEmpty || (!snapshot.pendingQuestions.isEmpty && previousQuestionIDs.isEmpty) {
            post(
                sessionID: snapshot.sessionID,
                turnID: snapshot.turnToken?.clientTurnId,
                workspaceKey: snapshot.workspaceKey,
                sessionMode: snapshot.sessionMode,
                eventClass: .waitingForUser,
                kind: .agentNeedsInput
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
                post(
                    sessionID: snapshot.sessionID,
                    turnID: turnID,
                    workspaceKey: snapshot.workspaceKey,
                    sessionMode: snapshot.sessionMode,
                    eventClass: .completed,
                    kind: .agentCompleted,
                    discriminator: task.id
                )
            case .failed:
                post(
                    sessionID: snapshot.sessionID,
                    turnID: turnID,
                    workspaceKey: snapshot.workspaceKey,
                    sessionMode: snapshot.sessionMode,
                    eventClass: .failed,
                    kind: .agentCompleted,
                    discriminator: task.id
                )
            case .paused:
                post(
                    sessionID: snapshot.sessionID,
                    turnID: turnID,
                    workspaceKey: snapshot.workspaceKey,
                    sessionMode: snapshot.sessionMode,
                    eventClass: .pausedRecoverable,
                    kind: .agentNeedsInput,
                    discriminator: task.id
                )
            case .pending, .running, .cancelled:
                break
            }
        }
    }

    func markRecoverablePause(
        sessionID: String,
        turnToken: ConversationTurnToken?,
        workspaceKey: String = "global",
        sessionMode: SessionMode = .code
    ) {
        guard scenePhase != .active else { return }
        post(
            sessionID: sessionID,
            turnID: turnToken?.clientTurnId,
            workspaceKey: workspaceKey,
            sessionMode: sessionMode,
            eventClass: .pausedRecoverable,
            // Same event class as the background-task pause above, so it must
            // answer to the same toggle: a turn the OS took away is an
            // input-needed event, not the "your turn merely finished" nag that
            // `.idlePrompt` governs. Under `.idlePrompt` a user who switched
            // idle prompts off was never told the session had stalled.
            kind: .agentNeedsInput
        )
    }

    func clear() {
        activityReporter.clear()
    }

    private func armIdleAlert(
        _ snapshot: ConversationBackgroundSnapshot,
        turnID: UInt64?,
        eventClass: ConversationNotificationEventClass
    ) {
        // Re-emitting the same completion must not restart the clock, the same
        // way `armPermissionAlert` only arms for a newly-arrived request id.
        if idleTask != nil, let armed = armedIdleCompletion,
           armed.turnID == turnID, armed.eventClass == eventClass {
            return
        }
        cancelIdleAlert()
        guard preferences.allows(.idlePrompt) else { return }
        armedIdleCompletion = (turnID, eventClass)
        let delay = UInt64(NotificationPolicy.clampIdleThreshold(preferences.messageIdleNotifThresholdMs))
        let sleeper = self.sleeper
        idleTask = Task { [weak self] in
            await sleeper(delay * 1_000_000)
            guard !Task.isCancelled, let self else { return }
            self.idleTask = nil
            // Re-checked at fire time, not trusted from arm time: the user may
            // have come back, or a new turn may have started, during the wait.
            guard self.scenePhase != .active else { return }
            self.post(
                sessionID: snapshot.sessionID,
                turnID: turnID,
                workspaceKey: snapshot.workspaceKey,
                sessionMode: snapshot.sessionMode,
                eventClass: eventClass,
                kind: .idlePrompt
            )
        }
    }

    private func armPermissionAlert(
        _ snapshot: ConversationBackgroundSnapshot,
        permission: ConversationPendingPermission
    ) {
        guard permissionTasks[permission.requestId] == nil else { return }
        guard preferences.allows(.permissionPrompt) else { return }
        let delay = UInt64(NotificationPolicy.permissionPromptNotifyDelayMs)
        let sleeper = self.sleeper
        permissionTasks[permission.requestId] = Task { [weak self] in
            await sleeper(delay * 1_000_000)
            guard !Task.isCancelled, let self else { return }
            self.permissionTasks.removeValue(forKey: permission.requestId)
            guard self.scenePhase != .active else { return }
            self.post(
                sessionID: snapshot.sessionID,
                turnID: snapshot.turnToken?.clientTurnId,
                workspaceKey: snapshot.workspaceKey,
                sessionMode: snapshot.sessionMode,
                eventClass: .needsPermission,
                kind: .permissionPrompt,
                discriminator: String(permission.requestId),
                detail: permission.toolName
            )
        }
    }

    private func post(
        sessionID: String,
        turnID: UInt64?,
        workspaceKey: String,
        sessionMode: SessionMode,
        eventClass: ConversationNotificationEventClass,
        kind: NotificationKind,
        discriminator: String? = nil,
        detail: String? = nil
    ) {
        guard preferences.allows(kind) else { return }
        let payload = ConversationNotificationPayload.make(
            sessionID: sessionID,
            turnID: turnID,
            eventClass: eventClass,
            workspaceKey: workspaceKey,
            sessionMode: sessionMode,
            discriminator: discriminator,
            detail: detail
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
                updatedAt: .now,
                workspaceKey: workspaceKey,
                sessionMode: sessionMode.rawValue
            )
        }
        if requiresExecutionLease {
            return ConversationLiveActivitySnapshot(
                sessionID: sessionID,
                turnID: turnToken?.clientTurnId,
                title: String(localized: "chat_background_service_title"),
                subtitle: String(localized: "chat_background_service_text"),
                status: .running,
                updatedAt: .now,
                workspaceKey: workspaceKey,
                sessionMode: sessionMode.rawValue
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
                    updatedAt: .now,
                    workspaceKey: workspaceKey,
                    sessionMode: sessionMode.rawValue
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
                updatedAt: .now,
                workspaceKey: workspaceKey,
                sessionMode: sessionMode.rawValue
            )
        }
        if backgroundTasks.contains(where: { $0.status == .failed }) {
            return ConversationLiveActivitySnapshot(
                sessionID: sessionID,
                turnID: nil,
                title: String(localized: "chat_background_failed_title"),
                subtitle: String(localized: "chat_background_failed_text"),
                status: .failed,
                updatedAt: .now,
                workspaceKey: workspaceKey,
                sessionMode: sessionMode.rawValue
            )
        }
        if backgroundTasks.contains(where: { $0.status == .completed }) {
            return ConversationLiveActivitySnapshot(
                sessionID: sessionID,
                turnID: nil,
                title: String(localized: "chat_background_completed_title"),
                subtitle: String(localized: "chat_background_completed_text"),
                status: .completed,
                updatedAt: .now,
                workspaceKey: workspaceKey,
                sessionMode: sessionMode.rawValue
            )
        }
        return nil
    }
}
