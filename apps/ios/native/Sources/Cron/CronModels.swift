import Foundation

let globalCronScopeID = "global"
let cronBackgroundTaskIdentifier = "com.lingxi.code.cron.reconcile"
let maxCronRunsPerTask = 20
let maxCronRunsTotal = 500
let maxCronResultBytes = 256 * 1024
let maxCronOccurrencesPerPass = 64

struct CronScope: Identifiable, Hashable, Codable, Sendable {
    let scopeID: String
    let projectID: String?
    let projectName: String
    let projectCwd: String?
    let guestWorkspacePath: String?

    var id: String { scopeID }
    var displayName: String { projectID == nil ? String(localized: "common_none") : projectName }

    static func global(appSandboxRoot: String) -> CronScope {
        CronScope(
            scopeID: globalCronScopeID,
            projectID: nil,
            projectName: "None",
            projectCwd: nil,
            guestWorkspacePath: MobileUserStorage.directories().scheduledGuestCwd
        )
    }

    static func project(_ project: ProjectSnapshot) -> CronScope {
        CronScope(scopeID: project.record.id, projectID: project.record.id, projectName: project.record.name,
                  projectCwd: project.sourceWorkspace.hostURL.path,
                  guestWorkspacePath: project.sourceWorkspace.guestPath)
    }
}

enum CronTaskStatus: String, Codable, CaseIterable, Sendable {
    case active, paused, completed
    var label: String {
        switch self {
        case .active: String(localized: "cron_task_status_active")
        case .paused: String(localized: "chat_status_paused")
        case .completed: String(localized: "chat_status_completed")
        }
    }
}

enum CronRunMode: String, Codable, CaseIterable, Sendable {
    case newSession = "new_session", selectedSession = "selected_session", taskSession = "task_session"
    var label: String {
        switch self {
        case .newSession: return String(localized: "cron_run_mode_new_chat")
        case .selectedSession: return String(localized: "cron_run_mode_selected_chat")
        case .taskSession: return String(localized: "cron_run_mode_task_chat")
        }
    }
}

enum CronNotificationPolicy: String, Codable, CaseIterable, Sendable {
    case all, failed, none
    func shouldNotify(_ status: CronRunStatus) -> Bool {
        status.isTerminal && self != .none &&
            (self == .all || [.failed, .timedOut, .interrupted].contains(status))
    }
    var label: String {
        switch self {
        case .all: return String(localized: "cron_notify_all")
        case .failed: return String(localized: "cron_notify_failed")
        case .none: return String(localized: "settings_typescript_lsp_off")
        }
    }
}

struct CronReasoning: Codable, Hashable, Sendable {
    var type = "automatic"
    var id: String?
    var tokens: UInt64?
    var selection: String {
        switch type {
        case "level": return id ?? "automatic"
        case "token_budget": return "budget:\(tokens ?? 0)"
        default: return type
        }
    }
    init(selection: String = "automatic") {
        if selection.hasPrefix("budget:"), let budget = UInt64(selection.dropFirst(7)) {
            type = "token_budget"; tokens = budget
        } else if ["automatic", "enabled", "disabled"].contains(selection) {
            type = selection
        } else {
            type = "level"; id = selection
        }
    }
}

struct CronAutomationRun: Codable, Hashable, Sendable {
    var id: String
    var taskId: String
    var ownerPid: UInt32? = nil
    var scheduledAt: UInt64
    var startedAt: UInt64?
    var finishedAt: UInt64?
    var status: CronRunStatus
    var model: String
    var reasoning: CronReasoning?
    var sessionId: String?
    var sessionTranscriptPath: String? = nil
    var sessionMode: SessionMode? = nil
    var sessionScopeKey: String? = nil
    var summary: String?
    var error: String?
    var claimGeneration: UInt64?
    var manualOccurrenceAt: UInt64?
}

struct CronAutomation: Codable, Hashable, Sendable {
    var version = 2
    var runs: [CronAutomationRun] = []
    var status: CronTaskStatus = .active
    var statusReason: String?
    var name: String?
    var model: String?
    var reasoning = CronReasoning()
    var runMode: CronRunMode = .newSession
    var sessionMode: SessionMode?
    var sessionScopeKey: String?
    var targetSessionId: String?
    var targetSessionTranscriptPath: String?
    var targetSessionMode: SessionMode?
    var targetSessionScopeKey: String?
    var ownedSessionId: String?
    var ownedSessionTranscriptPath: String?
    var ownedSessionMode: SessionMode?
    var ownedSessionScopeKey: String?
    var notificationPolicy: CronNotificationPolicy = .all

    enum CodingKeys: String, CodingKey {
        case version, status, statusReason, name, model, reasoning, runMode, sessionMode, sessionScopeKey, targetSessionId, targetSessionTranscriptPath, targetSessionMode, targetSessionScopeKey, ownedSessionId, ownedSessionTranscriptPath, ownedSessionMode, ownedSessionScopeKey, notificationPolicy, runs
    }
    init() {}
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = try c.decodeIfPresent(Int.self, forKey: .version) ?? 2
        runs = try c.decodeIfPresent([CronAutomationRun].self, forKey: .runs) ?? []
        status = try c.decodeIfPresent(CronTaskStatus.self, forKey: .status) ?? .active
        statusReason = try c.decodeIfPresent(String.self, forKey: .statusReason)
        name = try c.decodeIfPresent(String.self, forKey: .name)
        model = try c.decodeIfPresent(String.self, forKey: .model)
        reasoning = try c.decodeIfPresent(CronReasoning.self, forKey: .reasoning) ?? CronReasoning()
        runMode = try c.decodeIfPresent(CronRunMode.self, forKey: .runMode) ?? .newSession
        sessionMode = try c.decodeIfPresent(SessionMode.self, forKey: .sessionMode)
        sessionScopeKey = try c.decodeIfPresent(String.self, forKey: .sessionScopeKey)
        targetSessionId = try c.decodeIfPresent(String.self, forKey: .targetSessionId)
        targetSessionTranscriptPath = try c.decodeIfPresent(String.self, forKey: .targetSessionTranscriptPath)
        targetSessionMode = try c.decodeIfPresent(SessionMode.self, forKey: .targetSessionMode)
        targetSessionScopeKey = try c.decodeIfPresent(String.self, forKey: .targetSessionScopeKey)
        ownedSessionId = try c.decodeIfPresent(String.self, forKey: .ownedSessionId)
        ownedSessionTranscriptPath = try c.decodeIfPresent(String.self, forKey: .ownedSessionTranscriptPath)
        ownedSessionMode = try c.decodeIfPresent(SessionMode.self, forKey: .ownedSessionMode)
        ownedSessionScopeKey = try c.decodeIfPresent(String.self, forKey: .ownedSessionScopeKey)
        notificationPolicy = try c.decodeIfPresent(CronNotificationPolicy.self, forKey: .notificationPolicy) ?? .all
    }

    func shouldNotify(_ status: CronRunStatus) -> Bool {
        notificationPolicy.shouldNotify(status)
    }

    mutating func clearOwnedSession() {
        ownedSessionId = nil
        ownedSessionTranscriptPath = nil
        ownedSessionMode = nil
        ownedSessionScopeKey = nil
    }
}

struct CronSessionChoice: Identifiable, Equatable {
    let id: String
    let title: String
    let scopeID: String
    let owner: ConversationScope
    let mode: SessionMode
    let transcriptPath: String
}

/// Durable native reservation; unlike run history it survives task deletion.
struct CronOwnerReservation: Codable, Equatable, Sendable {
    let sessionId: String
    let sessionTranscriptPath: String
    let sessionMode: SessionMode
    let sessionScopeKey: String
}

struct CronOwnerCandidate: Equatable, Sendable {
    let sessionId: String
    let sessionMode: SessionMode
    let sessionScopeKey: String
    let sessionTranscriptPath: String?
}

func cronCandidateMatchesTask(_ candidate: CronOwnerCandidate, task: CronTaskRecord) -> Bool {
    let id = canonicalSessionID(candidate.sessionId)
    guard UUID(uuidString: id) != nil else { return false }
    let config = task.configuration
    switch config.runMode {
    case .selectedSession:
        return canonicalSessionID(config.targetSessionId ?? "") == id &&
            config.targetSessionMode == candidate.sessionMode && config.targetSessionScopeKey == candidate.sessionScopeKey &&
            config.targetSessionTranscriptPath != nil && config.targetSessionTranscriptPath == candidate.sessionTranscriptPath
    case .newSession, .taskSession:
        guard config.sessionMode == candidate.sessionMode, config.sessionScopeKey == candidate.sessionScopeKey else { return false }
        if config.runMode == .taskSession, let owned = config.ownedSessionId, !owned.isEmpty {
            return canonicalSessionID(owned) == id && candidate.sessionTranscriptPath?.isEmpty == false &&
                candidate.sessionTranscriptPath == config.ownedSessionTranscriptPath &&
                candidate.sessionMode == config.ownedSessionMode && candidate.sessionScopeKey == config.ownedSessionScopeKey
        }
        return candidate.sessionTranscriptPath == nil
    }
}

func cronReservationMatchesTask(_ reservation: CronOwnerReservation, task: CronTaskRecord) -> Bool {
    let config = task.configuration
    switch config.runMode {
    case .selectedSession:
        return canonicalSessionID(config.targetSessionId ?? "") == canonicalSessionID(reservation.sessionId) &&
            config.targetSessionTranscriptPath == reservation.sessionTranscriptPath &&
            config.targetSessionMode == reservation.sessionMode && config.targetSessionScopeKey == reservation.sessionScopeKey
    case .newSession, .taskSession:
        guard config.sessionMode == reservation.sessionMode, config.sessionScopeKey == reservation.sessionScopeKey else { return false }
        if config.runMode == .taskSession, let owned = config.ownedSessionId, !owned.isEmpty {
            return canonicalSessionID(owned) == canonicalSessionID(reservation.sessionId) &&
                config.ownedSessionTranscriptPath == reservation.sessionTranscriptPath &&
                config.ownedSessionMode == reservation.sessionMode && config.ownedSessionScopeKey == reservation.sessionScopeKey
        }
        return true
    }
}

/// A scheduler reserves only the UUID it is about to persist. Other owners,
/// including already admitted turns for that UUID, remain independent.
@MainActor
final class CronOwnerAdmissionGate {
    private var reservations: [String: UUID] = [:]
    private var catalogUpdates = Set<UUID>()
    private var preparingSources = Set<ObjectIdentifier>()
    private var scopeMutation: UUID?
    private var pendingScopeMutations = 0
    private var scopeWaiters: [UUID: CheckedContinuation<Void, Never>] = [:]
    private var scopeRevision: UInt64 = 0
    weak var sessionHost: SessionHost?
    var ownerCatalogIsCurrent: (() -> Bool)?

    func begin(sessionID: String, token: UUID) -> Bool {
        let id = canonicalSessionID(sessionID)
        guard UUID(uuidString: id) != nil, scopeMutation == nil, pendingScopeMutations == 0,
              reservations[id] == nil else { return false }
        reservations[id] = token
        return true
    }

    func finish(_ token: UUID) {
        reservations = reservations.filter { $0.value != token }
        catalogUpdates.remove(token)
        wakeScopeWaiters()
    }

    /// Hold just the listed identities while a catalog update is validated,
    /// written and published. A scheduler reservation for the same UUID must
    /// settle first; unrelated new owners are not blocked by this lease.
    func beginCatalogUpdate(sessionIDs: [String]) async throws -> UUID {
        let identities = Set(sessionIDs.map(canonicalSessionID).filter { UUID(uuidString: $0) != nil })
        while true {
            try Task.checkCancellation()
            if !scopeMutationPending && identities.allSatisfy({ reservations[$0] == nil }) {
                let token = UUID()
                catalogUpdates.insert(token)
                for id in identities { reservations[id] = token }
                return token
            }
            try await waitForScopeChange(since: scopeRevision)
        }
    }

    func allowsNewOwner(sessionID: String) -> Bool {
        scopeMutation == nil && reservations[canonicalSessionID(sessionID)] == nil
    }

    var scopeMutationPending: Bool { scopeMutation != nil || pendingScopeMutations > 0 }

    func beginSourcePreparation(_ source: any ConversationSource) -> Bool {
        let id = ObjectIdentifier(source)
        if preparingSources.contains(id) { return true }
        guard scopeMutation == nil, pendingScopeMutations == 0 else { return false }
        preparingSources.insert(id)
        return true
    }

    func endSourcePreparation(_ source: any ConversationSource) {
        preparingSources.remove(ObjectIdentifier(source))
        wakeScopeWaiters()
    }

    func beginScopeMutation() async throws -> UUID {
        pendingScopeMutations += 1
        defer { pendingScopeMutations -= 1; wakeScopeWaiters() }
        while true {
            try Task.checkCancellation()
            if scopeMutation == nil && preparingSources.isEmpty && reservations.isEmpty && catalogUpdates.isEmpty {
                let token = UUID()
                scopeMutation = token
                return token
            }
            try await waitForScopeChange(since: scopeRevision)
        }
    }

    func endScopeMutation(_ token: UUID) {
        guard scopeMutation == token else { return }
        scopeMutation = nil
        wakeScopeWaiters()
    }

    private func wakeScopeWaiters() {
        scopeRevision &+= 1
        let waiting = Array(scopeWaiters.values)
        scopeWaiters.removeAll()
        waiting.forEach { $0.resume() }
    }

    private func waitForScopeChange(since revision: UInt64) async throws {
        try Task.checkCancellation()
        let id = UUID()
        await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                if Task.isCancelled || revision != scopeRevision { continuation.resume() }
                else { scopeWaiters[id] = continuation }
            }
        } onCancel: { [weak self] in
            Task { @MainActor in self?.scopeWaiters.removeValue(forKey: id)?.resume() }
        }
        try Task.checkCancellation()
    }

    func waitUntilAvailable(sessionID: String) async throws {
        while scopeMutationPending || !allowsNewOwner(sessionID: sessionID) {
            try await waitForScopeChange(since: scopeRevision)
        }
        try Task.checkCancellation()
    }

    func allowsReservation(sessionID: String, owner: SessionOwnerEvidence, previouslyKnown: Bool,
                           checkPreparing: Bool = true) -> Bool {
        guard let sessionHost, !checkPreparing || !sessionHost.hasPreparingClaim(sessionID: sessionID) else { return false }
        do {
            guard let entry = try sessionHost.entryForClaim(sessionID: sessionID, scope: owner.scope, mode: owner.mode) else { return true }
            if let path = entry.source.model.workspaceState?.transcriptPath, !path.isEmpty {
                return path == owner.transcriptPath
            }
            return previouslyKnown
        } catch { return false }
    }

    func allowsUnclaimedReservation(sessionID: String) -> Bool {
        guard let sessionHost, !sessionHost.hasPreparingClaim(sessionID: sessionID) else { return false }
        return !sessionHost.entries.contains { canonicalSessionID($0.sessionID) == canonicalSessionID(sessionID) }
    }
}

struct CronTaskRecord: Identifiable, Hashable, Codable, Sendable {
    var id: String
    var cron: String
    var prompt: String
    var createdAtMs: UInt64
    var lastFiredAtMs: UInt64?
    var recurring: Bool
    var nextFireMs: UInt64?
    var human: String
    /// Engine-reported reason the host cannot run this schedule (for example a
    /// recurring interval under 15 minutes). `nil` means the task is runnable.
    var unsupportedReason: String? = nil

    var automation: CronAutomation? = nil
    var configuration: CronAutomation { automation ?? CronAutomation() }
    var status: CronTaskStatus { configuration.status }
    var mobileSupported: Bool { unsupportedReason == nil && status == .active }
}

struct CronOccurrence: Hashable, Codable, Sendable {
    let taskID: String
    let scheduledAtMs: UInt64
}

enum CronRunStatus: String, Codable, CaseIterable, Sendable {
    case queued
    case running
    case succeeded
    case failed
    case timedOut
    case cancelled
    case skipped
    case interrupted

    var isTerminal: Bool {
        switch self {
        case .queued, .running:
            return false
        case .succeeded, .failed, .timedOut, .cancelled, .skipped, .interrupted:
            return true
        }
    }

    var label: String {
        switch self {
        case .queued: return String(localized: "cron_status_queued")
        case .running: return String(localized: "chat_status_running")
        case .succeeded: return String(localized: "cron_status_succeeded")
        case .failed: return String(localized: "chat_status_failed")
        case .timedOut: return String(localized: "chat_status_timed_out")
        case .cancelled: return String(localized: "chat_status_cancelled")
        case .skipped: return String(localized: "cron_status_skipped")
        case .interrupted: return String(localized: "chat_result_interrupted")
        }
    }
}

enum CronRunErrorKind: String, Codable, CaseIterable, Sendable {
    case missingCredentials
    case network
    case timedOut
    case cancelled
    case validation
    case system
    case unknown

    var label: String {
        switch self {
        case .missingCredentials: return String(localized: "cron_error_kind_missing_credentials")
        case .network: return String(localized: "cron_error_kind_network")
        case .timedOut: return String(localized: "cron_error_kind_timed_out")
        case .cancelled: return String(localized: "chat_status_cancelled")
        case .validation: return String(localized: "cron_error_kind_validation")
        case .system: return String(localized: "cron_error_kind_system")
        case .unknown: return String(localized: "common_unknown_error")
        }
    }
}

struct CronRunRecord: Identifiable, Hashable, Codable, Sendable {
    let runID: String
    let taskID: String
    let scopeID: String
    let projectID: String?
    let projectName: String
    let prompt: String
    let scheduledAtMs: UInt64
    let triggeredAtMs: UInt64
    var startedAtMs: UInt64?
    var finishedAtMs: UInt64?
    var status: CronRunStatus
    var attempt: Int
    var resultText: String?
    var errorMessage: String?
    var errorKind: CronRunErrorKind?
    let manual: Bool
    var sessionID: String? = nil
    var sessionTranscriptPath: String? = nil
    var sessionMode: SessionMode? = nil
    var sessionScopeKey: String? = nil
    var actualModel: String? = nil
    var notificationPolicy: CronNotificationPolicy? = nil

    var id: String { runID }
}

enum CronRoute: Hashable, Sendable {
    case list
    case task(scopeID: String, taskID: String?)
    case run(runID: String)
}

enum CronSchedulingMode: String, Equatable, Sendable {
    case bestEffortBackground
    case foregroundOnly
    case unavailable

    var title: String {
        switch self {
        case .bestEffortBackground: return String(localized: "cron_scheduling_mode_background")
        case .foregroundOnly: return String(localized: "cron_scheduling_mode_foreground_only")
        case .unavailable: return String(localized: "cron_scheduling_mode_unavailable")
        }
    }
}

struct CronSchedulingSnapshot: Equatable, Sendable {
    var mode: CronSchedulingMode
    var nextEarliestAtMs: UInt64?
    var lastReconciledAtMs: UInt64?
    var note: String
    var backgroundTaskIdentifier: String

    static func initial(
        mode: CronSchedulingMode = .foregroundOnly,
        note: String = String(localized: "cron_scheduling_note_default"),
        backgroundTaskIdentifier: String = cronBackgroundTaskIdentifier
    ) -> CronSchedulingSnapshot {
        CronSchedulingSnapshot(
            mode: mode,
            nextEarliestAtMs: nil,
            lastReconciledAtMs: nil,
            note: note,
            backgroundTaskIdentifier: backgroundTaskIdentifier
        )
    }
}

struct CronScopedTask: Identifiable, Hashable, Sendable {
    let scope: CronScope
    let task: CronTaskRecord
    let activeRun: CronRunRecord?
    let lastRun: CronRunRecord?

    var id: String { "\(scope.scopeID):\(task.id)" }
}

struct CronGeneratedSession: Identifiable, Codable, Equatable, Sendable {
    var id: String
    var projectID: String?
    var title: String
    var updatedAtMs: UInt64
    var sessionTranscriptPath: String? = nil
    var sessionMode: SessionMode? = nil
    var sessionScopeKey: String? = nil
}

struct CronRepositoryState: Equatable, Sendable {
    var loading = false
    var tasks: [CronScopedTask] = []
    var scopes: [CronScope] = []
    var activeScopeID = globalCronScopeID
    var history: [CronRunRecord] = []
    var generatedSessions: [CronGeneratedSession] = []
    var scheduling = CronSchedulingSnapshot.initial()
    var errorMessage: String?
    var lastActionMessage: String?
}

enum CronResultCategory: String, CaseIterable, Equatable, Sendable {
    case success
    case missingCredentials
    case network
    case timedOut
    case cancelled
    case skipped
    case validation
    case system
    case unknown

    var label: String {
        switch self {
        case .success: return String(localized: "cron_status_succeeded")
        case .missingCredentials: return String(localized: "cron_error_kind_missing_credentials")
        case .network: return String(localized: "cron_error_kind_network")
        case .timedOut: return String(localized: "chat_status_timed_out")
        case .cancelled: return String(localized: "chat_status_cancelled")
        case .skipped: return String(localized: "cron_status_skipped")
        case .validation: return String(localized: "cron_error_kind_validation")
        case .system: return String(localized: "cron_error_kind_system")
        case .unknown: return String(localized: "cron_result_unknown")
        }
    }
}

struct CronRunCategorySummary: Identifiable, Equatable, Sendable {
    let category: CronResultCategory
    let count: Int
    let latestRun: CronRunRecord

    var id: CronResultCategory { category }

    var title: String { category.label }
    var detail: String {
        let latest = latestRun.finishedAtMs ?? latestRun.triggeredAtMs
        return String(localized: "cron_category_summary_detail \(count) \(formatCronEpoch(latest))")
    }
}

struct CronDiagnosticsSnapshot: Equatable, Sendable {
    let backgroundTaskIdentifier: String
    let schedulingModeTitle: String
    let activeScopeName: String
    let activeScopeID: String
    let taskCount: Int
    let historyCount: Int
    let activeRunCount: Int
    let nextEarliestRunText: String
    let lastReconciledText: String
    let schedulingNote: String
    let resultCategories: [CronRunCategorySummary]
    let activeRunSummary: String
}

struct CronTaskDraft: Equatable, Sendable {
    var scopeID = globalCronScopeID
    var taskID: String?
    var prompt = ""
    var cron = "0 9 * * *"
    var recurring = true

    var automation = CronAutomation()

    var isEditing: Bool { taskID != nil }

    static func create(scopeID: String = globalCronScopeID) -> CronTaskDraft {
        CronTaskDraft(scopeID: scopeID, taskID: nil, prompt: "", cron: "0 9 * * *", recurring: true)
    }

    static func edit(scopeID: String, task: CronTaskRecord) -> CronTaskDraft {
        CronTaskDraft(
            scopeID: scopeID,
            taskID: task.id,
            prompt: task.prompt,
            cron: task.cron,
            recurring: task.recurring,
            automation: task.configuration
        )
    }
}

struct CronExecutionOutcome: Equatable, Sendable {
    let status: CronRunStatus
    let resultText: String?
    let errorMessage: String?
    let errorKind: CronRunErrorKind?
    var sessionID: String? = nil
    var sessionTranscriptPath: String? = nil
    var sessionMode: SessionMode? = nil
    var sessionScopeKey: String? = nil
    var actualModel: String? = nil
}

struct CronNotificationPayload: Equatable, Sendable {
    let runID: String
    let scopeID: String
    let taskID: String
    let title: String
    let body: String
    let status: CronRunStatus
    var taskOnly = false
}

protocol CronScopeProviding: Sendable {
    func scopes(appSandboxRoot: String) async -> [CronScope]
    func activeScopeID(for scopes: [CronScope]) async -> String
}

protocol CronStoreClient: Sendable {
    func list() async throws -> [CronTaskRecord]
    func listOwnerReservations() async throws -> [CronOwnerReservation]
    func saveConfigured(draft: CronTaskDraft) async throws -> CronTaskRecord
    func updateAutomation(taskID: String, automation: CronAutomation) async throws -> CronTaskRecord
    func delete(taskID: String) async throws -> Bool
    func nextFireTime() async throws -> UInt64?
    func dueOccurrences(nowMs: UInt64) async throws -> [CronOccurrence]
    func acknowledgeOccurrence(taskID: String, scheduledAtMs: UInt64) async throws -> Bool
}

protocol CronStoreProviding: Sendable {
    func store(for scope: CronScope, appSandboxRoot: String) async throws -> any CronStoreClient
    func storeForAdmission(for scope: CronScope, appSandboxRoot: String) async throws -> any CronStoreClient
}

extension CronStoreProviding {
    func storeForAdmission(for scope: CronScope, appSandboxRoot: String) async throws -> any CronStoreClient {
        try await store(for: scope, appSandboxRoot: appSandboxRoot)
    }
}

protocol CronTaskExecuting: Sendable {
    func runTaskNow(scope: CronScope, task: CronTaskRecord) async throws -> CronExecutionOutcome
    func runTaskNow(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64) async throws -> CronExecutionOutcome
    func runTaskIfDue(
        scope: CronScope,
        task: CronTaskRecord,
        scheduledAtMs: UInt64
    ) async throws -> CronExecutionOutcome?
}

extension CronTaskExecuting {
    func runTaskNow(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64) async throws -> CronExecutionOutcome {
        try await runTaskNow(scope: scope, task: task)
    }
}

protocol CronNotificationDelivering: Sendable {
    /// Returns `false` ONLY when the notification could not be shown and the
    /// caller must return whatever delivery reservation it took — see
    /// `CronHistoryStore.releaseTerminalNotification`. A stand-in that shows
    /// nothing by design (tests, the no-op default) reports `true`: it is not a
    /// refusal, and treating it as one would churn the claim on every refresh.
    @discardableResult
    func deliver(_ payload: CronNotificationPayload) async -> Bool
}

protocol CronBackgroundScheduling: Sendable {
    var mode: CronSchedulingMode { get }
    var note: String { get }
    func schedule(taskIdentifier: String, earliestAtMs: UInt64?) async throws
    func cancel(taskIdentifier: String) async
}

struct DefaultCronScopeProvider: CronScopeProviding {
    func scopes(appSandboxRoot: String) async -> [CronScope] {
        [CronScope.global(appSandboxRoot: appSandboxRoot)]
    }

    func activeScopeID(for scopes: [CronScope]) async -> String {
        scopes.first?.scopeID ?? globalCronScopeID
    }
}

struct UnavailableCronExecutor: CronTaskExecuting {
    func runTaskNow(scope: CronScope, task: CronTaskRecord) async throws -> CronExecutionOutcome {
        throw CronExecutionError(
            kind: .missingCredentials,
            message: String(localized: "cron_executor_unavailable"),
            statusOverride: .failed
        )
    }

    func runTaskIfDue(
        scope: CronScope,
        task: CronTaskRecord,
        scheduledAtMs: UInt64
    ) async throws -> CronExecutionOutcome? {
        try await runTaskNow(scope: scope, task: task)
    }
}

struct NoopCronNotifier: CronNotificationDelivering {
    func deliver(_ payload: CronNotificationPayload) async -> Bool { true }
}

struct ForegroundOnlyCronScheduler: CronBackgroundScheduling {
    let mode: CronSchedulingMode = .foregroundOnly
    let note = String(localized: "cron_scheduler_foreground_note")

    func schedule(taskIdentifier: String, earliestAtMs: UInt64?) async throws {}

    func cancel(taskIdentifier: String) async {}
}

struct CronExecutionError: LocalizedError, Sendable, Equatable {
    let kind: CronRunErrorKind
    let message: String
    let statusOverride: CronRunStatus?

    var errorDescription: String? { message }
}

protocol CronBackgroundTaskRegistrar: Sendable {
    func register(
        identifier: String,
        handler: @escaping @Sendable () async -> Void
    )
}

extension CronRunRecord {
    var resultCategory: CronResultCategory {
        switch status {
        case .succeeded:
            return .success
        case .timedOut:
            return .timedOut
        case .cancelled:
            return .cancelled
        case .skipped:
            return .skipped
        case .failed, .interrupted:
            switch errorKind {
            case .missingCredentials:
                return .missingCredentials
            case .network:
                return .network
            case .timedOut:
                return .timedOut
            case .cancelled:
                return .cancelled
            case .validation:
                return .validation
            case .system:
                return .system
            case .unknown, .none:
                return .unknown
            }
        case .queued, .running:
            switch errorKind {
            case .missingCredentials:
                return .missingCredentials
            case .network:
                return .network
            case .timedOut:
                return .timedOut
            case .cancelled:
                return .cancelled
            case .validation:
                return .validation
            case .system:
                return .system
            case .unknown, .none:
                return .unknown
            }
        }
    }
}

extension CronRepositoryState {
    var activeScope: CronScope? {
        scopes.first(where: { $0.scopeID == activeScopeID }) ?? scopes.first
    }

    var activeRuns: [CronRunRecord] {
        history.filter { !$0.status.isTerminal }
    }

    var recentTerminalRuns: [CronRunRecord] {
        history.filter(\.status.isTerminal)
    }

    var taskCount: Int { tasks.count }
    var historyCount: Int { history.count }
    var activeRunCount: Int { activeRuns.count }

    var resultCategorySummaries: [CronRunCategorySummary] {
        var grouped: [CronResultCategory: [CronRunRecord]] = [:]
        for run in recentTerminalRuns {
            grouped[run.resultCategory, default: []].append(run)
        }
        let priority = CronResultCategory.allCases
        return priority.compactMap { category in
            guard let runs = grouped[category], let latestRun = runs.max(by: { lhs, rhs in
                (lhs.finishedAtMs ?? lhs.triggeredAtMs) < (rhs.finishedAtMs ?? rhs.triggeredAtMs)
            }) else {
                return nil
            }
            return CronRunCategorySummary(
                category: category,
                count: runs.count,
                latestRun: latestRun
            )
        }
    }

    var diagnostics: CronDiagnosticsSnapshot {
        let activeScope = activeScope
        return CronDiagnosticsSnapshot(
            backgroundTaskIdentifier: scheduling.backgroundTaskIdentifier,
            schedulingModeTitle: scheduling.mode.title,
            activeScopeName: activeScope?.displayName ?? String(localized: "settings_cu_not_selected"),
            activeScopeID: activeScope?.scopeID ?? activeScopeID,
            taskCount: taskCount,
            historyCount: historyCount,
            activeRunCount: activeRunCount,
            nextEarliestRunText: scheduling.nextEarliestAtMs.map(formatCronEpoch) ?? String(localized: "cron_none_yet"),
            lastReconciledText: scheduling.lastReconciledAtMs.map(formatCronEpoch) ?? String(localized: "cron_not_checked_yet"),
            schedulingNote: scheduling.note,
            resultCategories: resultCategorySummaries,
            activeRunSummary: activeRuns.isEmpty
                ? String(localized: "cron_no_active_runs")
                : String(localized: "cron_active_runs_summary \(activeRuns.count)")
        )
    }
}

func formatCronEpoch(_ epochMs: UInt64) -> String {
    let date = Date(timeIntervalSince1970: TimeInterval(epochMs) / 1000)
    let language = Bundle._lxCurrentLanguageID.withLock { $0 }
    return date.formatted(
        .dateTime
            .locale(language.isEmpty ? .autoupdatingCurrent : Locale(identifier: language))
            .year()
            .month(.twoDigits)
            .day(.twoDigits)
            .hour(.twoDigits(amPM: .omitted))
            .minute(.twoDigits)
    )
}

extension CronStoreClient {
    func saveConfigured(draft: CronTaskDraft) async throws -> CronTaskRecord {
        throw CronExecutionError(kind: .validation, message: "This task store must be upgraded before saving task settings.", statusOverride: .failed)
    }

    func updateAutomation(taskID: String, automation: CronAutomation) async throws -> CronTaskRecord {
        throw CronExecutionError(kind: .validation, message: "This task store must be upgraded before saving task settings.", statusOverride: .failed)
    }
}
