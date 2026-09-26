import Foundation

#if canImport(harness_runtimeFFI)

actor FfiCronStoreProvider: CronStoreProviding {
    private var stores: [String: FfiCronStoreClient] = [:]
    private let defaults: @MainActor @Sendable () -> (String?, CronReasoning)

    init(defaults: @escaping @MainActor @Sendable () -> (String?, CronReasoning) = { (nil, CronReasoning()) }) {
        self.defaults = defaults
    }

    func store(for scope: CronScope, appSandboxRoot: String) async throws -> any CronStoreClient {
        if let cached = stores[scope.scopeID] { return cached }
        let handle = try buildIosCronStore(
            appSandboxRoot: appSandboxRoot,
            projectCwd: scope.projectCwd
        )
        let (model, reasoning) = await defaults()
        try await handle.setMigrationDefaults(model: model ?? "", reasoningJson: String(decoding: try JSONEncoder().encode(reasoning), as: UTF8.self))
        let client = FfiCronStoreClient(handle: handle)
        stores[scope.scopeID] = client
        return client
    }
}

final class FfiCronStoreClient: CronStoreClient, @unchecked Sendable {
    private let handle: MobileCronStoreHandle

    init(handle: MobileCronStoreHandle) {
        self.handle = handle
    }

    func list() async throws -> [CronTaskRecord] {
        await handle.list().map(Self.map)
    }

    func create(cronExpr: String, prompt: String, recurring: Bool) async throws -> CronTaskRecord {
        Self.map(try await handle.create(cronExpr: cronExpr, prompt: prompt, recurring: recurring))
    }

    func update(taskID: String, cronExpr: String, prompt: String, recurring: Bool) async throws -> CronTaskRecord {
        Self.map(try await handle.update(id: taskID, cronExpr: cronExpr, prompt: prompt, recurring: recurring))
    }

    func migrateLegacyTasks(defaultModel: String?, reasoning: CronReasoning) async throws {
        try await handle.setMigrationDefaults(model: defaultModel ?? "", reasoningJson: String(decoding: try JSONEncoder().encode(reasoning), as: UTF8.self))
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
        return [CronScope.global(appSandboxRoot: appSandboxRoot)] + snapshot.projects.map { project in
            CronScope(
                scopeID: project.record.id,
                projectID: project.record.id,
                projectName: project.record.name,
                projectCwd: project.workspace.hostURL.path,
                guestWorkspacePath: project.workspace.guestPath
            )
        }
    }

    func activeScopeID(for scopes: [CronScope]) async -> String {
        let snapshot = await readProjects()
        let active = snapshot.activeProjectID
        return active.flatMap { id in scopes.contains(where: { $0.scopeID == id }) ? id : nil }
            ?? globalCronScopeID
    }
}

final class FfiCronExecutor: CronTaskExecuting, @unchecked Sendable {
    private let appSandboxRoot: String
    private let launchSnapshot: @MainActor @Sendable () -> ProviderLaunchSnapshot
    private let terminalConfig: @MainActor @Sendable (CronScope) -> TerminalRuntimeConfig?

    init(
        appSandboxRoot: String,
        launchSnapshot: @escaping @MainActor @Sendable () -> ProviderLaunchSnapshot,
        terminalConfig: @escaping @MainActor @Sendable (CronScope) -> TerminalRuntimeConfig?
    ) {
        self.appSandboxRoot = appSandboxRoot
        self.launchSnapshot = launchSnapshot
        self.terminalConfig = terminalConfig
    }

    func runTaskNow(scope: CronScope, task: CronTaskRecord) async throws -> CronExecutionOutcome {
        let handle = try await makeHandle(scope: scope, task: task)
        guard let result = await handle.runCronTaskNow(taskId: task.id) else {
            return CronExecutionOutcome(
                status: .failed,
                resultText: nil,
                errorMessage: String(localized: "cron_task_unavailable"),
                errorKind: .validation
            )
        }
        return Self.map(result)
    }

    func runTaskNow(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64) async throws -> CronExecutionOutcome {
        let handle = try await makeHandle(scope: scope, task: task)
        guard let result = await handle.runCronTaskNowAt(taskId: task.id, scheduledAtMs: scheduledAtMs) else {
            return CronExecutionOutcome(status: .failed, resultText: nil,
                errorMessage: String(localized: "cron_task_unavailable"), errorKind: .validation)
        }
        return Self.map(result)
    }

    func runTaskIfDue(
        scope: CronScope,
        task: CronTaskRecord,
        scheduledAtMs: UInt64
    ) async throws -> CronExecutionOutcome? {
        let handle = try await makeHandle(scope: scope, task: task)
        return await handle.runCronTaskIfDue(taskId: task.id, scheduledAtMs: scheduledAtMs).map(Self.map)
    }

    func runLocalAppBackgroundTasks() async {
        do {
            let handle = try await makeHandle(scope: .global(appSandboxRoot: appSandboxRoot))
            let now = UInt64(Date().timeIntervalSince1970 * 1000)
            _ = await handle.runDueLocalAppBackgroundTasks(nowMs: now)
            await scheduleNextLocalAppBackgroundWake(handle: handle, nowMs: now)
        } catch {
            LocalAppBackgroundTaskBridge.shared.schedule(
                earliestAtMs: UInt64(Date().timeIntervalSince1970 * 1000) + 15 * 60 * 1_000
            )
        }
    }

    func rescheduleLocalAppBackgroundWake() async {
        do {
            let handle = try await makeHandle(scope: .global(appSandboxRoot: appSandboxRoot))
            let now = UInt64(Date().timeIntervalSince1970 * 1000)
            await scheduleNextLocalAppBackgroundWake(handle: handle, nowMs: now)
        } catch {
            LocalAppBackgroundTaskBridge.shared.schedule(
                earliestAtMs: UInt64(Date().timeIntervalSince1970 * 1000) + 15 * 60 * 1_000
            )
        }
    }

    private func scheduleNextLocalAppBackgroundWake(
        handle: MobileEngineHandle,
        nowMs: UInt64
    ) async {
        let next = await handle.nextLocalAppBackgroundWakeMs(nowMs: nowMs)
        LocalAppBackgroundTaskBridge.shared.schedule(earliestAtMs: next)
    }

    private func makeHandle(scope: CronScope, task: CronTaskRecord? = nil) async throws -> MobileEngineHandle {
        let (snapshot, runtime) = await MainActor.run {
            (launchSnapshot(), terminalConfig(scope))
        }
        var cronRuntime = runtime
        if task != nil && scope.projectID == nil {
            cronRuntime?.workspaceHostPath = URL(fileURLWithPath: appSandboxRoot).appendingPathComponent("scheduled/workspace", isDirectory: true).path
            cronRuntime?.stableWorkspaceId = "scheduled"
        }
        let listener = CronEngineListener()
        let permissionSink = CronPermissionSink()
        let provider = IosProviderConfigFfi(
            providerProfilesJson: snapshot.providerProfilesJSON,
            routingJson: snapshot.routingJSON
        )
        let config = IosEngineLaunchConfigFfi(
            apiBase: Keychain.get(.apiBase) ?? "https://api.anthropic.com",
            apiKey: Keychain.get(.apiKey) ?? "",
            model: task?.configuration.model ?? snapshot.defaultModelID ?? Keychain.get(.model) ?? "",
            sessionMode: .code,
            visionDelegationEnabled: snapshot.visionDelegationEnabled,
            appSandboxRoot: appSandboxRoot,
            projectCwd: scope.projectCwd ?? (task == nil ? nil : URL(fileURLWithPath: appSandboxRoot).appendingPathComponent("scheduled/workspace", isDirectory: true).path),
            providerConfig: provider,
            mobileLinux: cronRuntime.map {
                makeIosMobileLinuxConfig($0, appSandboxRoot: appSandboxRoot)
            },
            localAppsFullRuntime: LocalAppsRuntimeDistribution.usesFullRuntime,
            localAppsRuntimeRoot: LocalAppsRuntimeDistribution.runtimeRoot,
            physicalMemoryBytes: ProcessInfo.processInfo.physicalMemory,
            hostEnvironment: await MainActor.run {
                makeIosHostEnvironment(launchMode: .scheduledHeadless)
            }
        )
        let audio = await MainActor.run { IOSAudioServiceCallbackAdapter.shared }
        let handle = try buildIosEngineWithConfig(
            config: config,
            listener: listener,
            audio: audio,
            camera: CameraImpl(),
            share: ShareImpl(),
            notifications: NotificationImpl(),
            clipboard: ClipboardImpl(),
            permissions: permissionSink,
            secureStorage: SecureStorageImpl(),
            deviceControl: DeviceControlImpl()
        )
        permissionSink.attach(handle)
        return handle
    }

    private static func map(_ fired: FiredCronJobDto) -> CronExecutionOutcome {
        switch fired.status {
        case .ok:
            return CronExecutionOutcome(
                status: .succeeded,
                resultText: fired.resultText,
                errorMessage: nil,
                errorKind: nil,
                sessionID: fired.sessionId
            )
        case .failed(let message):
            let classification = classifyFailure(message, retryable: fired.retryable)
            return CronExecutionOutcome(
                status: classification.status,
                resultText: fired.resultText,
                errorMessage: message,
                errorKind: classification.kind,
                sessionID: fired.sessionId
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

private final class CronEngineListener: IosEventListener, @unchecked Sendable {
    func onEvent(event: ClientEvent) async {}
    func onWorkflowProgress(
        originSessionId _: String,
        taskId _: String,
        runId _: String,
        progress _: WorkflowProgressDto
    ) async {}

    /// Compatibility with pre-session-scoping generated bindings.
    func onWorkflowProgress(
        taskId _: String,
        runId _: String,
        progress _: WorkflowProgressDto
    ) async {}
}

private final class CronPermissionSink: IosPermissionSink, @unchecked Sendable {
    private var handle: MobileEngineHandle?

    func attach(_ handle: MobileEngineHandle) {
        self.handle = handle
    }

    func onRequest(request: PermissionRequest) async {
        try? await handle?.submit(command: .denyPermission(requestId: request.requestId))
    }
}

/// `appSandboxRoot` is a parameter rather than something this function looks
/// up, so that both callers pass the SAME value they hand the engine in the
/// launch config beside it. The runtime validates local-app build mounts
/// against this root; when it disagreed with the engine's by even one
/// component, every local-app build failed its mount check. Passing one
/// expression to both fields makes them agree by construction instead of by
/// two lookups that happen to match.
func makeIosMobileLinuxConfig(
    _ config: TerminalRuntimeConfig,
    appSandboxRoot: String
) -> IosMobileLinuxConfigFfi {
    IosMobileLinuxConfigFfi(
        mode: config.mode == .legacy ? .legacy : .mobileLinux,
        managedRoot: config.managedRoot,
        workspaceHostPath: config.workspaceHostPath,
        stableWorkspaceId: config.stableWorkspaceId,
        abi: config.abi,
        rootfsVersion: config.rootfsVersion,
        archiveSha256: config.archiveSha256,
        authorizationFile: config.authorizationFile,
        appSandboxRoot: appSandboxRoot
    )
}

#endif
