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

    static func global(appSandboxRoot: String) -> CronScope {
        CronScope(
            scopeID: globalCronScopeID,
            projectID: nil,
            projectName: String(localized: "common_global"),
            projectCwd: nil,
            guestWorkspacePath: appSandboxRoot
        )
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

    var mobileSupported: Bool { unsupportedReason == nil }
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

    var isTerminal: Bool {
        switch self {
        case .queued, .running:
            return false
        case .succeeded, .failed, .timedOut, .cancelled, .skipped:
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

struct CronRepositoryState: Equatable, Sendable {
    var loading = false
    var tasks: [CronScopedTask] = []
    var scopes: [CronScope] = []
    var activeScopeID = globalCronScopeID
    var history: [CronRunRecord] = []
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
            recurring: task.recurring
        )
    }
}

struct CronExecutionOutcome: Equatable, Sendable {
    let status: CronRunStatus
    let resultText: String?
    let errorMessage: String?
    let errorKind: CronRunErrorKind?
}

struct CronNotificationPayload: Equatable, Sendable {
    let runID: String
    let scopeID: String
    let taskID: String
    let title: String
    let body: String
    let status: CronRunStatus
}

protocol CronScopeProviding: Sendable {
    func scopes(appSandboxRoot: String) async -> [CronScope]
    func activeScopeID(for scopes: [CronScope]) async -> String
}

protocol CronStoreClient: Sendable {
    func list() async throws -> [CronTaskRecord]
    func create(cronExpr: String, prompt: String, recurring: Bool) async throws -> CronTaskRecord
    func update(taskID: String, cronExpr: String, prompt: String, recurring: Bool) async throws -> CronTaskRecord
    func delete(taskID: String) async throws -> Bool
    func nextFireTime() async throws -> UInt64?
    func dueOccurrences(nowMs: UInt64) async throws -> [CronOccurrence]
    func acknowledgeOccurrence(taskID: String, scheduledAtMs: UInt64) async throws -> Bool
}

protocol CronStoreProviding: Sendable {
    func store(for scope: CronScope, appSandboxRoot: String) async throws -> any CronStoreClient
}

protocol CronTaskExecuting: Sendable {
    func runTaskNow(scope: CronScope, task: CronTaskRecord) async throws -> CronExecutionOutcome
    func runTaskIfDue(
        scope: CronScope,
        task: CronTaskRecord,
        scheduledAtMs: UInt64
    ) async throws -> CronExecutionOutcome?
}

protocol CronNotificationDelivering: Sendable {
    func deliver(_ payload: CronNotificationPayload) async
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
    func deliver(_ payload: CronNotificationPayload) async {}
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
        case .failed:
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
            activeScopeName: activeScope?.projectName ?? String(localized: "settings_cu_not_selected"),
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
    return date.formatted(
        .dateTime
            .year()
            .month(.twoDigits)
            .day(.twoDigits)
            .hour(.twoDigits(amPM: .omitted))
            .minute(.twoDigits)
    )
}
