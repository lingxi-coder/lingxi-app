import CryptoKit
import SwiftUI

private struct LinuxRuntimeCommandFailure: Error {
    let message: String
}

private enum LinuxRuntimeBridge {
    private static let workspaceIDDefaultsKey = "lingxi.mobile-linux.workspace.default.id"

    actor HandleCache {
        private struct Key: Equatable {
            let mode: String
            let managedRoot: String
            let workspaceHostPath: String
            let stableWorkspaceID: String
            let abi: String
            let rootfsVersion: String
            let archiveSha256: String?
            let authorizationFile: String?
            let authorizationDigest: String?

            init(_ config: IosMobileLinuxConfigFfi) {
                mode = String(describing: config.mode)
                managedRoot = config.managedRoot
                workspaceHostPath = config.workspaceHostPath
                stableWorkspaceID = config.stableWorkspaceId
                abi = config.abi
                rootfsVersion = config.rootfsVersion
                archiveSha256 = config.archiveSha256
                authorizationFile = config.authorizationFile
                authorizationDigest = config.authorizationFile.flatMap { path in
                    guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)) else {
                        return "<missing>"
                    }
                    return SHA256.hash(data: data)
                        .map { String(format: "%02x", $0) }
                        .joined()
                }
            }
        }

        private var handle: IosMobileLinuxRuntimeHandle?
        private var key: Key?

        func handle(for config: IosMobileLinuxConfigFfi) async throws -> IosMobileLinuxRuntimeHandle {
            let nextKey = Key(config)
            if let handle, key == nextKey {
                return handle
            }
            let newHandle = try createIosMobileLinuxRuntime(config: config)
            self.handle = newHandle
            self.key = nextKey
            return newHandle
        }
    }

    private static let cache = HandleCache()

    private static func config(for mode: LinuxRuntimeMode) -> IosMobileLinuxConfigFfi {
        let manifest = LXISHRuntimeBundleMetadata.current()
        let workspaceID = stableWorkspaceID()
        let appSupportRoot = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)
            .first?
            .path ?? ""
        let workspaceHostPath = (appSupportRoot as NSString)
            .appendingPathComponent("workspaces/\(workspaceID)")
        try? FileManager.default.createDirectory(
            atPath: workspaceHostPath,
            withIntermediateDirectories: true
        )
        let managedRoot = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)
            .first?
            .appendingPathComponent("mobile-linux/ios-ish", isDirectory: true)
            .path ?? ""
        let authorizationFile = LXISHRuntimeBundleResources.authorizationManifestURL()?.path
        return IosMobileLinuxConfigFfi(
            mode: mode == .legacy ? .legacy : .mobileLinux,
            managedRoot: managedRoot,
            workspaceHostPath: workspaceHostPath,
            stableWorkspaceId: workspaceID,
            abi: "arm64",
            rootfsVersion: manifest.rootfsVersion,
            archiveSha256: manifest.archiveSha256,
            authorizationFile: authorizationFile
        )
    }

    private static func stableWorkspaceID() -> String {
        let defaults = UserDefaults.standard
        if let persisted = defaults.string(forKey: workspaceIDDefaultsKey),
           UUID(uuidString: persisted) != nil
        {
            return persisted.lowercased()
        }
        let generated = UUID().uuidString.lowercased()
        defaults.set(generated, forKey: workspaceIDDefaultsKey)
        return generated
    }

    static func load(mode: LinuxRuntimeMode) async -> LinuxRuntimeState {
        #if targetEnvironment(simulator)
            return simulatorState(mode: mode, action: .refresh)
        #else
        let cfg = config(for: mode)
        let handle = try? await cache.handle(for: cfg)
        let capability: MobileLinuxCapabilityFfi
        let status: MobileLinuxStatusFfi
        let tasks: [MobileLinuxTaskFfi]
        if let handle {
            capability = await handle.capability()
            status = (try? await handle.status()) ?? iosMobileLinuxStatus(config: cfg)
            tasks = (try? await handle.listTasks()) ?? []
        } else {
            capability = probeIosMobileLinux(config: cfg)
            status = iosMobileLinuxStatus(config: cfg)
            tasks = []
        }
        return map(mode: mode, capability: capability, status: status, action: .refresh, tasks: tasks)
        #endif
    }

    static func verify(mode: LinuxRuntimeMode) async -> LinuxRuntimeState {
        #if targetEnvironment(simulator)
            return simulatorState(mode: mode, action: .verify)
        #else
        let cfg = config(for: mode)
        let handle = try? await cache.handle(for: cfg)
        let capability: MobileLinuxCapabilityFfi
        let status: MobileLinuxStatusFfi
        let tasks: [MobileLinuxTaskFfi]
        if let handle {
            capability = await handle.capability()
            status = (try? await handle.verifyRootfs()) ?? verifyIosMobileLinux(config: cfg)
            tasks = (try? await handle.listTasks()) ?? []
        } else {
            capability = probeIosMobileLinux(config: cfg)
            status = verifyIosMobileLinux(config: cfg)
            tasks = []
        }
        return map(mode: mode, capability: capability, status: status, action: .verify, tasks: tasks)
        #endif
    }

    static func repair(mode: LinuxRuntimeMode) async -> LinuxRuntimeState {
        #if targetEnvironment(simulator)
            return simulatorState(mode: mode, action: .repair)
        #else
        let cfg = config(for: mode)
        let handle = try? await cache.handle(for: cfg)
        let capability: MobileLinuxCapabilityFfi
        let status: MobileLinuxStatusFfi
        let tasks: [MobileLinuxTaskFfi]
        if let handle {
            capability = await handle.capability()
            status = (try? await handle.repairRootfs()) ?? repairIosMobileLinux(config: cfg)
            tasks = (try? await handle.listTasks()) ?? []
        } else {
            capability = probeIosMobileLinux(config: cfg)
            status = repairIosMobileLinux(config: cfg)
            tasks = []
        }
        return map(mode: mode, capability: capability, status: status, action: .repair, tasks: tasks)
        #endif
    }

    static func reset(mode: LinuxRuntimeMode) async -> LinuxRuntimeState {
        #if targetEnvironment(simulator)
            return simulatorState(mode: mode, action: .reset)
        #else
        let cfg = config(for: mode)
        let handle = try? await cache.handle(for: cfg)
        let capability: MobileLinuxCapabilityFfi
        let status: MobileLinuxStatusFfi
        let tasks: [MobileLinuxTaskFfi]
        if let handle {
            capability = await handle.capability()
            status = (try? await handle.resetRootfs()) ?? resetIosMobileLinux(config: cfg)
            tasks = (try? await handle.listTasks()) ?? []
        } else {
            capability = probeIosMobileLinux(config: cfg)
            status = resetIosMobileLinux(config: cfg)
            tasks = []
        }
        return map(mode: mode, capability: capability, status: status, action: .reset, tasks: tasks)
        #endif
    }

    static func runCommand(
        mode: LinuxRuntimeMode,
        command: String
    ) async -> Result<LinuxTerminalLine, LinuxRuntimeCommandFailure> {
        #if targetEnvironment(simulator)
            return .failure(
                LinuxRuntimeCommandFailure(message: String(localized: "settings_linux_simulator_unavailable"))
            )
        #else
        let cfg = config(for: mode)
        do {
            let handle = try await cache.handle(for: cfg)
            let result = try await handle.runCommand(
                request: MobileLinuxCommandRequestFfi(
                    command: "/bin/sh",
                    args: ["-lc", command],
                    cwd: "/workspace/\(cfg.stableWorkspaceId)",
                    env: [:],
                    stdin: nil,
                    timeoutMs: 30_000,
                    network: .allowed,
                    mounts: []
                )
            )
            let combined = [result.stdout, result.stderr]
                .filter { !$0.isEmpty }
                .joined(separator: "\n")
            return .success(
                LinuxTerminalLine(
                    streamID: "run-preview",
                    source: "run",
                    text: combined.isEmpty ? "(无输出)" : combined,
                    isError: result.exitCode != 0 || result.timedOut || result.cancelled
                )
            )
        } catch {
            return .failure(LinuxRuntimeCommandFailure(message: String(describing: error)))
        }
        #endif
    }

    static func refreshTasks(mode: LinuxRuntimeMode) async -> Result<LinuxRuntimeTaskOperationResult, LinuxRuntimeTaskOperationFailure> {
        #if targetEnvironment(simulator)
            return .failure(LinuxRuntimeTaskOperationFailure(message: String(localized: "settings_linux_simulator_unavailable")))
        #else
        let cfg = config(for: mode)
        do {
            let handle = try await cache.handle(for: cfg)
            let tasks = try await handle.listTasks().map(mapTask)
            let message = tasks.isEmpty ? String(localized: "settings_linux_no_guest_tasks") : String(localized: "settings_linux_refresh_tasks")
            return .success(LinuxRuntimeTaskOperationResult(tasks: tasks, message: message))
        } catch {
            return .failure(LinuxRuntimeTaskOperationFailure(message: String(describing: error)))
        }
        #endif
    }

    static func stopTask(
        mode: LinuxRuntimeMode,
        taskID: String
    ) async -> Result<LinuxRuntimeTaskOperationResult, LinuxRuntimeTaskOperationFailure> {
        #if targetEnvironment(simulator)
            return .failure(LinuxRuntimeTaskOperationFailure(message: String(localized: "settings_linux_simulator_unavailable")))
        #else
        let cfg = config(for: mode)
        do {
            let handle = try await cache.handle(for: cfg)
            let stopped = try await handle.killTask(taskId: taskID)
            let tasks = try await handle.listTasks().map(mapTask)
            let message = stopped.detail ?? "已停止任务 \(stopped.title)"
            return .success(LinuxRuntimeTaskOperationResult(tasks: tasks, message: message))
        } catch {
            return .failure(LinuxRuntimeTaskOperationFailure(message: String(describing: error)))
        }
        #endif
    }

    private static func map(
        mode: LinuxRuntimeMode,
        capability: MobileLinuxCapabilityFfi,
        status: MobileLinuxStatusFfi,
        action: LinuxRuntimeAction,
        tasks: [MobileLinuxTaskFfi]
    ) -> LinuxRuntimeState {
        let detail = status.lastError ?? capability.reason ?? "未返回额外诊断信息"
        let summary: String
        if mode == .legacy {
            summary = "当前仍使用 iOS unavailable shell stub"
        } else if status.state == .blockedByLicense {
            summary = "缺少额外书面授权，iSH 后端被显式阻塞"
        } else if status.state == .unsupported {
            summary = "授权存在或模式已选中，但当前构建未链接 iSH 运行时"
        } else if capability.available {
            summary = "Mobile Linux 运行时可用"
        } else {
            summary = "Mobile Linux 运行时暂不可用"
        }
        return LinuxRuntimeState(
            selectedMode: mode,
            backend: status.backend,
            rootfsState: map(status.state),
            version: status.version,
            managedRoot: status.managedRoot,
            installedSizeBytes: status.installedSizeBytes,
            available: capability.available,
            terminalSupported: capability.pty,
            backgroundTasksSupported: capability.backgroundProcesses,
            verifyAllowed: capability.rootfsIntegrity,
            repairAllowed: capability.rootfsIntegrity,
            resetAllowed: capability.rootfsIntegrity,
            writableGuestPaths: status.writableGuestPaths,
            summary: summary,
            detail: detail,
            lastAction: action,
            lastActionMessage: detail,
            busyAction: nil,
            tasks: tasks.map(mapTask),
            mounts: status.writableGuestPaths.map {
                LinuxRuntimeMountRow(id: $0, hostPath: "App Sandbox", guestPath: $0, readOnly: !$0.starts(with: "/workspace/"))
            }
        )
    }

    private static func map(_ value: MobileLinuxRootfsStateFfi) -> LinuxRuntimeRootfsState {
        switch value {
        case .missing: return .missing
        case .installing: return .installing
        case .ready: return .ready
        case .corrupt: return .corrupt
        case .repairing: return .repairing
        case .resetting: return .resetting
        case .unsupported: return .unsupported
        case .blockedByLicense: return .blockedByLicense
        }
    }

    private static func mapTask(_ task: MobileLinuxTaskFfi) -> LinuxRuntimeTaskRow {
        LinuxRuntimeTaskRow(
            id: task.id,
            title: task.title,
            state: {
                switch task.state {
                case .running: return .running
                case .completed: return .completed
                case .failed: return .failed
                case .cancelled: return .cancelled
                case .unavailable: return .unavailable
                }
            }(),
            detail: task.detail
        )
    }

    private static func simulatorState(mode: LinuxRuntimeMode, action: LinuxRuntimeAction) -> LinuxRuntimeState {
        var state = LinuxRuntimeState(selectedMode: mode)
        state.backend = mode == .legacy ? "ios-posix" : "ios-ish"
        state.rootfsState = .unsupported
        state.summary = String(localized: "settings_linux_simulator_unavailable")
        state.detail = "完整运行时只在 arm64 真机启用；Simulator 始终返回 unavailable stub。"
        state.lastAction = action
        state.lastActionMessage = state.detail
        return state
    }
}

private struct LinuxRuntimeBridgeTaskOperator: LinuxRuntimeTaskOperating {
    func refreshTasks(mode: LinuxRuntimeMode) async throws -> LinuxRuntimeTaskOperationResult {
        switch await LinuxRuntimeBridge.refreshTasks(mode: mode) {
        case .success(let result):
            return result
        case .failure(let error):
            throw error
        }
    }

    func stopTask(mode: LinuxRuntimeMode, taskID: String) async throws -> LinuxRuntimeTaskOperationResult {
        switch await LinuxRuntimeBridge.stopTask(mode: mode, taskID: taskID) {
        case .success(let result):
            return result
        case .failure(let error):
            throw error
        }
    }
}

struct LinuxRuntimePage: View {
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    @State private var taskOperations = LinuxRuntimeTaskOperationsModel(adapter: LinuxRuntimeBridgeTaskOperator())
    var onOpenTerminal: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            blurb
            modeSection
            statusSection
            maintenanceSection
            terminalSection
            tasksSection
            mountsSection
            workspaceSection
            safetySection
        }
        .task(id: store.linuxRuntime.selectedMode) {
            await run(.refresh, mode: store.linuxRuntime.selectedMode)
        }
        .onChange(of: taskOperations.errorMessage) { _, message in
            guard let message, !message.isEmpty else { return }
            store.linuxRuntime.lastActionMessage = message
        }
    }

    private var blurb: some View {
        Text("settings_linux_blurb")
            .font(.system(size: 11.5))
            .foregroundStyle(t.text3)
            .lineSpacing(4)
            .padding(.bottom, 14)
    }

    private var modeSection: some View {
        SettingsSection(label: String(localized: "settings_linux_section_backend")) {
            RadioList(
                options: [
                    .init(value: LinuxRuntimeMode.mobileLinux.rawValue, label: "Mobile Linux",
                          sub: String(localized: "settings_linux_mobile_linux_sub")),
                    .init(value: LinuxRuntimeMode.legacy.rawValue, label: "Legacy",
                          sub: String(localized: "settings_linux_legacy_sub")),
                ],
                value: Binding(
                    get: { store.linuxRuntime.selectedMode.rawValue },
                    set: { raw in
                        guard let mode = LinuxRuntimeMode(rawValue: raw) else { return }
                        guard mode != store.linuxRuntime.selectedMode else { return }
                        store.linuxRuntime.selectedMode = mode
                        store.linuxRuntime.busyAction = nil
                    }
                )
            )
        }
    }

    private var statusSection: some View {
        SettingsSection(label: String(localized: "settings_linux_section_status")) {
            SettingsRow(icon: .workflow, iconColor: Color(srgb: 0.3503,0.6649,0.9741),
                        label: String(localized: "settings_linux_current_backend"),
                        value: store.linuxRuntime.backend, chevron: false)
            SettingsRow(label: "Rootfs", sub: store.linuxRuntime.detail,
                        value: store.linuxRuntime.rootfsState.label, chevron: false)
            SettingsRow(label: String(localized: "settings_linux_version"),
                        value: store.linuxRuntime.version ?? String(localized: "settings_linux_not_installed"),
                        chevron: false)
            SettingsRow(label: String(localized: "settings_linux_size"),
                        value: store.linuxRuntime.installedSizeBytes.map(formatBytes) ?? "—", chevron: false)
            SettingsRow(label: String(localized: "settings_linux_managed_dir"),
                        sub: store.linuxRuntime.managedRoot ?? String(localized: "settings_linux_not_installed"),
                        value: store.linuxRuntime.badge, chevron: false, isLast: true)
        }
    }

    private var maintenanceSection: some View {
        SettingsSection(
            label: String(localized: "settings_linux_section_maintenance"),
            footer: String(localized: "settings_linux_maintenance_footer")
        ) {
            actionRow(String(localized: "settings_linux_action_refresh"), action: .refresh,
                      enabled: store.linuxRuntime.busyAction == nil)
            actionRow(String(localized: "settings_linux_action_verify"), action: .verify,
                      enabled: store.linuxRuntime.verifyAllowed && store.linuxRuntime.busyAction == nil)
            actionRow(String(localized: "settings_linux_action_repair"), action: .repair,
                      enabled: store.linuxRuntime.repairAllowed && store.linuxRuntime.busyAction == nil)
            actionRow(String(localized: "settings_linux_action_reset"), action: .reset,
                      enabled: store.linuxRuntime.resetAllowed && store.linuxRuntime.busyAction == nil,
                      isLast: true)
        }
    }

    private var workspaceSection: some View {
        SettingsSection(label: String(localized: "settings_linux_section_workspace")) {
            SettingsRow(label: String(localized: "settings_linux_terminal_entry"),
                        sub: store.linuxRuntime.canOpenTerminal
                            ? String(localized: "settings_linux_pty_available")
                            : String(localized: "settings_linux_pty_unavailable"),
                        value: store.linuxRuntime.canOpenTerminal
                            ? String(localized: "settings_status_available")
                            : String(localized: "settings_status_disabled"),
                        chevron: false)
            SettingsRow(label: String(localized: "settings_linux_external_mounts"),
                        sub: store.linuxRuntime.writableGuestPaths.isEmpty
                            ? String(localized: "settings_linux_no_mounts")
                            : store.linuxRuntime.writableGuestPaths.prefix(2).joined(separator: " · "),
                        value: String(localized: "settings_count_items \(store.linuxRuntime.writableGuestPaths.count)"),
                        chevron: false)
            SettingsRow(label: String(localized: "settings_linux_task_execution"),
                        sub: store.linuxRuntime.lastActionMessage ?? String(localized: "settings_linux_no_guest_tasks"),
                        value: store.linuxRuntime.lastAction?.label ?? String(localized: "settings_linux_idle"),
                        chevron: false, isLast: true)
        }
    }

    private var terminalSection: some View {
        SettingsSection(label: String(localized: "settings_linux_section_terminal"),
                        footer: String(localized: "settings_linux_terminal_footer")) {
            FieldLabel(text: String(localized: "settings_linux_initial_command"))
            SettingsField(text: Binding(
                get: { store.linuxRuntime.terminal.draftCommand },
                set: { store.linuxRuntime.terminal.draftCommand = $0 }
            ), placeholder: String(localized: "settings_linux_command_placeholder"))
                .padding(.bottom, 14)
            SettingsRow(
                label: String(localized: "settings_linux_command_preview"),
                sub: normalizedDraftCommand ?? String(localized: "settings_linux_no_command_set"),
                value: normalizedDraftCommand == nil
                    ? String(localized: "settings_linux_interactive_shell")
                    : String(localized: "settings_linux_will_auto_run"),
                chevron: false
            )
            SettingsRow(
                label: String(localized: "settings_linux_open_fullscreen_terminal"),
                sub: store.linuxRuntime.canOpenTerminal
                    ? String(localized: "settings_linux_use_project_workspace")
                    : String(localized: "settings_linux_open_despite_unavailable"),
                value: store.linuxRuntime.canOpenTerminal
                    ? String(localized: "settings_status_available")
                    : String(localized: "settings_linux_diagnose"),
                chevron: true,
                isLast: true,
                onTap: onOpenTerminal
            )
        }
    }

    private var tasksSection: some View {
        SettingsSection(label: String(localized: "settings_linux_section_tasks")) {
            actionButtonRow(
                label: String(localized: "settings_linux_refresh_tasks"),
                buttonTitle: String(localized: "settings_linux_refresh_button"),
                enabled: store.linuxRuntime.available && !taskOperations.isBusy,
                busy: taskOperations.busyOperation == .refreshTasks,
                isLast: store.linuxRuntime.tasks.isEmpty
            ) {
                Task { await refreshTasks() }
            }
            if store.linuxRuntime.tasks.isEmpty {
                SettingsRow(label: String(localized: "settings_linux_task_current"),
                            sub: String(localized: "settings_linux_no_guest_tasks"),
                            value: "0", chevron: false, isLast: true)
            } else {
                ForEach(Array(store.linuxRuntime.tasks.enumerated()), id: \.element.id) { index, task in
                    SettingsRow(
                        label: task.title,
                        sub: task.detail,
                        value: task.state.label,
                        chevron: false,
                        isLast: index == store.linuxRuntime.tasks.count - 1
                    ) {
                        if task.state == .running && store.linuxRuntime.backgroundTasksSupported {
                            if taskOperations.isStopping(task.id) {
                                ProgressView().controlSize(.small)
                            } else {
                                Button(String(localized: "settings_linux_stop_task")) {
                                    Task { await stopTask(taskID: task.id) }
                                }
                                .font(.system(size: 12, weight: .medium))
                            }
                        } else {
                            EmptyView()
                        }
                    }
                }
            }
        }
    }

    private var mountsSection: some View {
        SettingsSection(label: String(localized: "settings_linux_section_mounts")) {
            if store.linuxRuntime.mounts.isEmpty {
                SettingsRow(label: String(localized: "settings_linux_mounts_label"),
                            sub: String(localized: "settings_linux_no_mounts_detail"),
                            value: "0", chevron: false, isLast: true)
            } else {
                ForEach(Array(store.linuxRuntime.mounts.enumerated()), id: \.element.id) { index, mount in
                    SettingsRow(label: mount.guestPath,
                                sub: mount.hostPath,
                                value: mount.readOnly
                                    ? String(localized: "settings_linux_read_only")
                                    : String(localized: "settings_linux_writable"),
                                chevron: false,
                                isLast: index == store.linuxRuntime.mounts.count - 1)
                }
            }
        }
    }

    private var safetySection: some View {
        SettingsSection(label: String(localized: "settings_linux_section_safety")) {
            SettingsRow(icon: .pin, label: String(localized: "settings_linux_execution_boundary"),
                        sub: String(localized: "settings_linux_execution_boundary_sub"),
                        chevron: false)
            SettingsRow(icon: .stop, label: String(localized: "settings_linux_stop_current_task"),
                        sub: store.linuxRuntime.backgroundTasksSupported
                            ? String(localized: "settings_linux_stop_task_sub_available")
                            : String(localized: "settings_linux_stop_task_sub_unavailable"),
                        value: store.linuxRuntime.backgroundTasksSupported
                            ? String(localized: "settings_status_available")
                            : String(localized: "settings_status_not_supported"),
                        chevron: false, isLast: true)
        }
    }

    @ViewBuilder
    private func actionButtonRow(
        label: String,
        buttonTitle: String,
        enabled: Bool,
        busy: Bool,
        isLast: Bool = false,
        action: @escaping () -> Void
    ) -> some View {
        SettingsRow(
            label: label,
            sub: busy ? String(localized: "settings_linux_running") : (enabled ? buttonTitle : String(localized: "settings_linux_unavailable")),
            chevron: false,
            isLast: isLast
        ) {
            if busy {
                ProgressView().controlSize(.small)
            } else if enabled {
                Button(buttonTitle, action: action)
                    .font(.system(size: 12, weight: .medium))
            } else {
                Text("settings_status_not_enabled")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(t.text4)
            }
        }
    }

    @ViewBuilder
    private func actionRow(
        _ title: String,
        action: LinuxRuntimeAction,
        enabled: Bool,
        isLast: Bool = false
    ) -> some View {
        SettingsRow(
            label: title,
            sub: store.linuxRuntime.busyAction == action
                ? String(localized: "settings_linux_running")
                : (enabled ? action.label : String(localized: "settings_linux_action_unavailable")),
            chevron: false,
            isLast: isLast
        ) {
            if store.linuxRuntime.busyAction == action {
                ProgressView().controlSize(.small)
            } else if enabled {
                Button(action.label) {
                    Task { await run(action, mode: store.linuxRuntime.selectedMode) }
                }
                .font(.system(size: 12, weight: .medium))
            } else {
                Text("settings_status_not_enabled").font(.system(size: 12, weight: .medium)).foregroundStyle(t.text4)
            }
        }
    }

    @MainActor
    private func run(_ action: LinuxRuntimeAction, mode: LinuxRuntimeMode) async {
        guard store.linuxRuntime.selectedMode == mode,
              store.linuxRuntime.busyAction == nil,
              !taskOperations.isBusy else { return }
        store.linuxRuntime.busyAction = action
        let next: LinuxRuntimeState
        switch action {
        case .refresh: next = await LinuxRuntimeBridge.load(mode: mode)
        case .verify: next = await LinuxRuntimeBridge.verify(mode: mode)
        case .repair: next = await LinuxRuntimeBridge.repair(mode: mode)
        case .reset: next = await LinuxRuntimeBridge.reset(mode: mode)
        }
        if Task.isCancelled {
            if store.linuxRuntime.selectedMode == mode,
               store.linuxRuntime.busyAction == action {
                store.linuxRuntime.busyAction = nil
            }
            return
        }
        guard store.linuxRuntime.selectedMode == mode,
              store.linuxRuntime.busyAction == action else { return }
        store.linuxRuntime = next
    }

    private var normalizedDraftCommand: String? {
        let trimmed = store.linuxRuntime.terminal.draftCommand.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    @MainActor
    private func refreshTasks() async {
        guard !taskOperations.isBusy else { return }
        if let result = await taskOperations.refreshTasks(mode: store.linuxRuntime.selectedMode) {
            store.linuxRuntime.tasks = result.tasks
            store.linuxRuntime.lastActionMessage = result.message
        }
    }

    @MainActor
    private func stopTask(taskID: String) async {
        guard !taskOperations.isBusy else { return }
        if let result = await taskOperations.stopTask(mode: store.linuxRuntime.selectedMode, taskID: taskID) {
            store.linuxRuntime.tasks = result.tasks
            store.linuxRuntime.lastActionMessage = result.message
        }
    }

}

private func formatBytes(_ bytes: UInt64) -> String {
    let kb: Double = 1024
    let mb = kb * 1024
    let gb = mb * 1024
    let value = Double(bytes)
    if value >= gb { return String(format: "%.1f GB", value / gb) }
    if value >= mb { return String(format: "%.1f MB", value / mb) }
    if value >= kb { return String(format: "%.1f KB", value / kb) }
    return "\(bytes) B"
}
