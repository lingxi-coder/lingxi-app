import Foundation

#if canImport(harness_runtimeFFI)

actor FfiCronStoreProvider: CronStoreProviding {
    private var stores: [String: FfiCronStoreClient] = [:]

    func store(for scope: CronScope, appSandboxRoot: String) async throws -> any CronStoreClient {
        if let cached = stores[scope.scopeID] { return cached }
        let handle = try buildIosCronStore(
            appSandboxRoot: appSandboxRoot,
            managedRoot: LXISHDefaultWorkspace.managedRootPath(),
            projectCwd: scope.projectCwd
        )
        let client = FfiCronStoreClient(handle: handle)
        stores[scope.scopeID] = client
        return client
    }

    func storeForAdmission(for scope: CronScope, appSandboxRoot: String) async throws -> any CronStoreClient {
        // Strict discovery never rewrites existing task or ledger data. The
        // native constructor and advisory lock may create their directories.
        let handle = try buildIosCronStore(
            appSandboxRoot: appSandboxRoot,
            managedRoot: LXISHDefaultWorkspace.managedRootPath(),
            projectCwd: scope.projectCwd
        )
        return FfiCronStoreClient(handle: handle)
    }
}

final class FfiCronStoreClient: CronStoreClient, @unchecked Sendable {
    private let handle: MobileCronStoreHandle

    init(handle: MobileCronStoreHandle) {
        self.handle = handle
    }

    func list() async throws -> [CronTaskRecord] {
        try await handle.listStrict().map(Self.map)
    }

    func listOwnerReservations() async throws -> [CronOwnerReservation] {
        try await handle.listOwnerReservationsStrict().map { dto in
            guard let mode = SessionMode(rawValue: dto.sessionMode) else {
                throw ProjectRepositoryError.conflictingSessionMode
            }
            return CronOwnerReservation(sessionId: dto.sessionId, sessionTranscriptPath: dto.sessionTranscriptPath,
                                        sessionMode: mode, sessionScopeKey: dto.sessionScopeKey)
        }
    }

    func saveConfigured(draft: CronTaskDraft) async throws -> CronTaskRecord {
        let json = String(decoding: try JSONEncoder().encode(draft.automation), as: UTF8.self)
        if let id = draft.taskID {
            return Self.map(try await handle.updateConfigured(id: id, cronExpr: draft.cron, prompt: draft.prompt, recurring: draft.recurring, automationJson: json))
        }
        return Self.map(try await handle.createConfigured(cronExpr: draft.cron, prompt: draft.prompt, recurring: draft.recurring, automationJson: json))
    }

    func updateAutomation(taskID: String, automation: CronAutomation) async throws -> CronTaskRecord {
        let json = String(decoding: try JSONEncoder().encode(automation), as: UTF8.self)
        return Self.map(try await handle.updateAutomation(id: taskID, automationJson: json))
    }

    func delete(taskID: String) async throws -> Bool {
        await handle.delete(id: taskID)
    }

    func nextFireTime() async throws -> UInt64? {
        await handle.nextFireTime()
    }

    func dueOccurrences(nowMs: UInt64) async throws -> [CronOccurrence] {
        await handle.dueOccurrences(nowMs: nowMs).map {
            CronOccurrence(taskID: $0.taskId, scheduledAtMs: $0.scheduledAtMs)
        }
    }

    func acknowledgeOccurrence(taskID: String, scheduledAtMs: UInt64) async throws -> Bool {
        await handle.acknowledgeOccurrence(taskId: taskID, scheduledAtMs: scheduledAtMs)
    }

    private static func decodeAutomation(_ json: String?) -> CronAutomation? {
        guard let json else { return nil }
        if let configuration = try? JSONDecoder().decode(CronAutomation.self, from: Data(json.utf8)), configuration.version == 2 {
            return configuration
        }
        var unsupported = CronAutomation()
        unsupported.version = 0
        unsupported.status = .paused
        unsupported.statusReason = "This task uses settings this app cannot read. Update the app before editing it."
        return unsupported
    }

    private static func map(_ dto: CronTaskDto) -> CronTaskRecord {
        CronTaskRecord(
            id: dto.id,
            cron: dto.cron,
            prompt: dto.prompt,
            createdAtMs: dto.createdAtMs,
            lastFiredAtMs: dto.lastFiredAtMs,
            recurring: dto.recurring,
            nextFireMs: dto.nextFireMs,
            human: dto.human,
            unsupportedReason: dto.mobileSupported ? nil : dto.unsupportedReason,
            automation: decodeAutomation(dto.automationJson)
        )
    }
}

struct ProjectCronScopeProvider: CronScopeProviding, @unchecked Sendable {
    let readProjects: @MainActor @Sendable () -> (projects: [ProjectSnapshot], activeProjectID: String?)

    func scopes(appSandboxRoot: String) async -> [CronScope] {
        let snapshot = await readProjects()
        return Self.scopes(appSandboxRoot: appSandboxRoot, projects: snapshot.projects)
    }

    static func scopes(appSandboxRoot: String, projects: [ProjectSnapshot]) -> [CronScope] {
        [CronScope.global(appSandboxRoot: appSandboxRoot)] + projects.map(CronScope.project)
    }

    func activeScopeID(for scopes: [CronScope]) async -> String {
        globalCronScopeID
    }
}

// UniFFI can invoke the callback from any executor. All mutable callback
// state and every access to the UI-owned gate/closures are MainActor-isolated.
final class FfiCronOwnerAdmission: MobileCronOwnerAdmission, @unchecked Sendable {
    private let token = UUID()
    private let gate: CronOwnerAdmissionGate
    private let scope: CronScope
    private let task: CronTaskRecord
    private let prepare: @MainActor @Sendable (CronOwnerCandidate, CronScope, CronTaskRecord) -> Bool
    private let confirm: @MainActor @Sendable (CronOwnerReservation, CronScope, CronTaskRecord) async throws -> Void
    private let blockAdmission: @MainActor @Sendable (String) -> Void
    @MainActor private(set) var callbackFinished = false
    @MainActor private var began = false
    @MainActor private var confirming = false
    @MainActor private var reservedCandidate: CronOwnerCandidate?

    @MainActor
    init(gate: CronOwnerAdmissionGate, scope: CronScope, task: CronTaskRecord,
         prepare: @escaping @MainActor @Sendable (CronOwnerCandidate, CronScope, CronTaskRecord) -> Bool,
         confirm: @escaping @MainActor @Sendable (CronOwnerReservation, CronScope, CronTaskRecord) async throws -> Void,
         blockAdmission: @escaping @MainActor @Sendable (String) -> Void) {
        self.gate = gate
        self.scope = scope
        self.task = task
        self.prepare = prepare
        self.confirm = confirm
        self.blockAdmission = blockAdmission
    }

    func beginOwner(candidate: CronOwnerCandidateDto) async -> Bool {
        await MainActor.run {
            guard !began, !callbackFinished else { return false }
            began = true
            guard let mode = SessionMode(rawValue: candidate.sessionMode) else { return false }
            let owner = CronOwnerCandidate(sessionId: candidate.sessionId, sessionMode: mode,
                sessionScopeKey: candidate.sessionScopeKey, sessionTranscriptPath: candidate.sessionTranscriptPath)
            guard gate.ownerCatalogIsCurrent?() ?? true,
                  cronCandidateMatchesTask(owner, task: task), prepare(owner, scope, task),
                  gate.begin(sessionID: owner.sessionId, token: token) else { return false }
            reservedCandidate = owner
            return true
        }
    }

    func confirmOwner(reservation: CronOwnerReservationDto) async -> Bool {
        let owner: CronOwnerReservation? = await MainActor.run {
            guard !callbackFinished, !confirming else { return nil }
            guard let reservedCandidate,
                  canonicalSessionID(reservedCandidate.sessionId) == canonicalSessionID(reservation.sessionId),
                  reservedCandidate.sessionMode.rawValue == reservation.sessionMode,
                  reservedCandidate.sessionScopeKey == reservation.sessionScopeKey,
                  (reservedCandidate.sessionTranscriptPath.map { $0 == reservation.sessionTranscriptPath } ?? true) else {
                blockAdmission("Native scheduler returned a different session owner")
                callbackFinished = true
                gate.finish(token)
                return nil
            }
            guard let mode = SessionMode(rawValue: reservation.sessionMode) else {
                blockAdmission("Invalid native session mode")
                callbackFinished = true
                gate.finish(token)
                return nil
            }
            confirming = true
            return CronOwnerReservation(sessionId: reservation.sessionId,
                sessionTranscriptPath: reservation.sessionTranscriptPath,
                sessionMode: mode, sessionScopeKey: reservation.sessionScopeKey)
        }
        guard let owner else { return false }
        do {
            try await confirm(owner, scope, task)
            return await MainActor.run { callbackFinished = true; gate.finish(token); return true }
        } catch {
            return await MainActor.run {
                blockAdmission(error.localizedDescription)
                callbackFinished = true
                gate.finish(token)
                return false
            }
        }
    }

    @MainActor func finish() { gate.finish(token) }
}

// Stored configuration is immutable; the non-Sendable media composition is
// only accessed inside MainActor.run when a native handle is constructed.
final class FfiCronExecutor: CronTaskExecuting, @unchecked Sendable {
    private let appSandboxRoot: String
    private let mediaHostComposition: IOSNativeMediaHostComposition?
    private let launchSnapshot: @MainActor @Sendable () -> ProviderLaunchSnapshot
    private let terminalConfig: @MainActor @Sendable (CronScope) -> TerminalRuntimeConfig?
    private let admissionGate: CronOwnerAdmissionGate
    private let prepareOwner: @MainActor @Sendable (CronOwnerCandidate, CronScope, CronTaskRecord) -> Bool
    private let confirmOwner: @MainActor @Sendable (CronOwnerReservation, CronScope, CronTaskRecord) async throws -> Void
    private let reconcileUnconfirmed: @Sendable (CronScope) async throws -> Void
    private let blockAdmission: @MainActor @Sendable (String) -> Void

    init(
        appSandboxRoot: String,
        mediaHostComposition: IOSNativeMediaHostComposition?,
        launchSnapshot: @escaping @MainActor @Sendable () -> ProviderLaunchSnapshot,
        terminalConfig: @escaping @MainActor @Sendable (CronScope) -> TerminalRuntimeConfig?,
        admissionGate: CronOwnerAdmissionGate,
        prepareOwner: @escaping @MainActor @Sendable (CronOwnerCandidate, CronScope, CronTaskRecord) -> Bool,
        confirmOwner: @escaping @MainActor @Sendable (CronOwnerReservation, CronScope, CronTaskRecord) async throws -> Void,
        reconcileUnconfirmed: @escaping @Sendable (CronScope) async throws -> Void,
        blockAdmission: @escaping @MainActor @Sendable (String) -> Void
    ) {
        self.appSandboxRoot = appSandboxRoot
        self.mediaHostComposition = mediaHostComposition
        self.launchSnapshot = launchSnapshot
        self.terminalConfig = terminalConfig
        self.admissionGate = admissionGate
        self.prepareOwner = prepareOwner
        self.confirmOwner = confirmOwner
        self.reconcileUnconfirmed = reconcileUnconfirmed
        self.blockAdmission = blockAdmission
    }

    func runTaskNow(scope: CronScope, task: CronTaskRecord) async throws -> CronExecutionOutcome {
        let handle = try await makeHandle(scope: scope, task: task)
        let callback = await makeAdmission(scope: scope, task: task)
        let admission = MobileCronAdmission(owner: callback)
        let result = await withTaskCancellationHandler {
            await handle.runCronTaskNow(taskId: task.id, ownerAdmission: admission)
        } onCancel: {
            Task.detached { await admission.cancelAndWait() }
        }
        try await finishAdmission(callback, admission: admission, scope: scope)
        guard let result else {
            return CronExecutionOutcome(
                status: .failed,
                resultText: nil,
                errorMessage: String(localized: "cron_task_unavailable"),
                errorKind: .validation
            )
        }
        withExtendedLifetime(handle) {}
        return Self.map(result, scope: scope, task: task)
    }

    func runTaskNow(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64) async throws -> CronExecutionOutcome {
        let handle = try await makeHandle(scope: scope, task: task)
        let callback = await makeAdmission(scope: scope, task: task)
        let admission = MobileCronAdmission(owner: callback)
        let result = await withTaskCancellationHandler {
            await handle.runCronTaskNowAt(taskId: task.id, scheduledAtMs: scheduledAtMs, ownerAdmission: admission)
        } onCancel: {
            Task.detached { await admission.cancelAndWait() }
        }
        try await finishAdmission(callback, admission: admission, scope: scope)
        guard let result else {
            return CronExecutionOutcome(status: .failed, resultText: nil,
                errorMessage: String(localized: "cron_task_unavailable"), errorKind: .validation)
        }
        withExtendedLifetime(handle) {}
        return Self.map(result, scope: scope, task: task)
    }

    func runTaskIfDue(
        scope: CronScope,
        task: CronTaskRecord,
        scheduledAtMs: UInt64
    ) async throws -> CronExecutionOutcome? {
        let handle = try await makeHandle(scope: scope, task: task)
        let callback = await makeAdmission(scope: scope, task: task)
        let admission = MobileCronAdmission(owner: callback)
        let result = await withTaskCancellationHandler {
            await handle.runCronTaskIfDue(taskId: task.id, scheduledAtMs: scheduledAtMs, ownerAdmission: admission)
        } onCancel: {
            Task.detached { await admission.cancelAndWait() }
        }
        try await finishAdmission(callback, admission: admission, scope: scope)
        withExtendedLifetime(handle) {}
        return result.map { Self.map($0, scope: scope, task: task) }
    }

    private func makeAdmission(scope: CronScope, task: CronTaskRecord) async -> FfiCronOwnerAdmission {
        await MainActor.run {
            FfiCronOwnerAdmission(gate: admissionGate, scope: scope, task: task,
                                  prepare: prepareOwner,
                                  confirm: confirmOwner, blockAdmission: blockAdmission)
        }
    }

    private func finishAdmission(_ callback: FfiCronOwnerAdmission, admission: MobileCronAdmission, scope: CronScope) async throws {
        if await MainActor.run(body: { !callback.callbackFinished }) {
            // The Swift caller may be cancelled while a detached native driver
            // still has a ledger write or callback in flight. Wait for that
            // barrier independently of the caller's cancellation.
            await Task.detached { await admission.cancelAndWait() }.value
            do {
                // A cancelled caller must still mirror any durable reservation
                // before its short admission gate is released.
                try await Task.detached { try await self.reconcileUnconfirmed(scope) }.value
            }
            catch {
                await MainActor.run { blockAdmission(error.localizedDescription); callback.finish() }
                throw CronExecutionError(kind: .validation, message: error.localizedDescription, statusOverride: .failed)
            }
        }
        await MainActor.run { callback.finish() }
    }

    private func makeHandle(scope: CronScope, task: CronTaskRecord? = nil) async throws -> MobileEngineHandle {
        let (prepared, mediaHost) = try await MainActor.run {
            guard let mediaHostComposition else {
                throw NSError(
                    domain: "IOSNativeMediaHost",
                    code: 1,
                    userInfo: [NSLocalizedDescriptionKey: "The app media host was not composed before cron engine creation."]
                )
            }
            let prepared = try NativeEngineFactory.prepare(.scheduled(
                appSandboxRoot: appSandboxRoot,
                scope: scope,
                task: task,
                snapshot: launchSnapshot(),
                runtime: terminalConfig(scope)
            ))
            return (prepared, mediaHostComposition.nativeHost)
        }
        return try await NativeEngineFactory.build(prepared, mediaHost: mediaHost)
    }

    private static func map(_ fired: FiredCronJobDto, scope: CronScope, task: CronTaskRecord) -> CronExecutionOutcome {
        let mode = fired.sessionMode.flatMap(SessionMode.init(rawValue:))
        let sessionScopeKey: String?
        if task.configuration.runMode == .selectedSession {
            // SDK validates the path and mode; the opaque owner must also be
            // the one captured by the editor for this exact selected session.
            if canonicalSessionID(fired.sessionId ?? "") == canonicalSessionID(task.configuration.targetSessionId ?? ""),
               fired.sessionTranscriptPath == task.configuration.targetSessionTranscriptPath,
               mode == task.configuration.targetSessionMode,
               fired.sessionScopeKey == task.configuration.targetSessionScopeKey {
                sessionScopeKey = fired.sessionScopeKey
            } else {
                sessionScopeKey = nil
            }
        } else {
            sessionScopeKey = fired.sessionScopeKey == task.configuration.sessionScopeKey &&
                mode == task.configuration.sessionMode ? fired.sessionScopeKey : nil
        }
        switch fired.status {
        case .ok:
            return CronExecutionOutcome(
                status: .succeeded,
                resultText: fired.resultText,
                errorMessage: nil,
                errorKind: nil,
                sessionID: fired.sessionId, sessionTranscriptPath: fired.sessionTranscriptPath,
                sessionMode: mode, sessionScopeKey: sessionScopeKey
            )
        case .failed(let message):
            let classification = classifyFailure(message, retryable: fired.retryable)
            return CronExecutionOutcome(
                status: classification.status,
                resultText: fired.resultText,
                errorMessage: message,
                errorKind: classification.kind,
                sessionID: fired.sessionId, sessionTranscriptPath: fired.sessionTranscriptPath,
                sessionMode: mode, sessionScopeKey: sessionScopeKey
            )
        case .outcomeUnknown(let message):
            // The model may already have run. Keep its exact owner receipt and
            // never queue or automatically replay an indeterminate outcome.
            return CronExecutionOutcome(
                status: .interrupted,
                resultText: fired.resultText,
                errorMessage: message,
                errorKind: .system,
                sessionID: fired.sessionId, sessionTranscriptPath: fired.sessionTranscriptPath,
                sessionMode: mode, sessionScopeKey: sessionScopeKey
            )
        }
    }

    private static func classifyFailure(
        _ message: String,
        retryable: Bool
    ) -> (status: CronRunStatus, kind: CronRunErrorKind) {
        let normalized = message.lowercased()
        if normalized.hasPrefix("busy:") { return (.queued, .system) }
        if normalized.hasPrefix("paused:") { return (.failed, .validation) }
        if normalized.contains("cancel") || normalized.contains("取消") {
            return (.cancelled, .cancelled)
        }
        if normalized.contains("timeout") || normalized.contains("timed out") || normalized.contains("超时") {
            return (.timedOut, .timedOut)
        }
        if normalized.contains("credential") || normalized.contains("api key") || normalized.contains("unauthorized") {
            return (.failed, .missingCredentials)
        }
        if retryable || normalized.contains("network") || normalized.contains("connection") {
            return (.failed, .network)
        }
        return (.failed, .system)
    }
}

#endif
