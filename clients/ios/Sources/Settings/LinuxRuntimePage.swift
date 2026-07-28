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
        let authorizationFile = Bundle.main
            .url(forResource: "AUTHORIZATION_MANIFEST", withExtension: "json")?
            .path
        return IosMobileLinuxConfigFfi(
            mode: mode == .legacy ? .legacy : .mobileLinux,
            managedRoot: managedRoot,
            workspaceHostPath: workspaceHostPath,
            stableWorkspaceId: workspaceID,
            abi: "arm64",
            rootfsVersion: "1.0.0",
            archiveSha256: nil,
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
                LinuxRuntimeCommandFailure(message: "iOS Simulator 不提供 Mobile Linux 运行时")
            )
        #else
        let cfg = config(for: mode)
        do {
            let handle = try await cache.handle(for: cfg)
            let result = try await handle.runCommand(
                request: MobileLinuxCommandRequestFfi(
                    command: "/bin/sh",
                    args: ["-lc", command],
                    cwd: nil,
                    env: [:],
                    stdin: nil,
                    timeoutMs: 30_000,
                    network: .disabled,
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
        state.summary = "iOS Simulator 不提供 Mobile Linux 运行时"
        state.detail = "完整运行时只在 arm64 真机启用；Simulator 始终返回 unavailable stub。"
        state.lastAction = action
        state.lastActionMessage = state.detail
        return state
    }
}

struct LinuxRuntimePage: View {
    @Environment(\.theme) private var t
    @ObservedObject var store: SettingsStore

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
    }

    private var blurb: some View {
        Text("iOS phase-1 只接入运行时管理面板。未取得额外书面授权时，Mobile Linux 后端不会被链接进商店构建；Simulator 继续走 unavailable stub。")
            .font(.system(size: 11.5))
            .foregroundStyle(t.text3)
            .lineSpacing(4)
            .padding(.bottom, 14)
    }

    private var modeSection: some View {
        SettingsSection(label: "后端选择") {
            RadioList(
                options: [
                    .init(value: LinuxRuntimeMode.legacy.rawValue, label: "Legacy", sub: "继续使用当前 unavailable shell stub"),
                    .init(value: LinuxRuntimeMode.mobileLinux.rawValue, label: "Mobile Linux", sub: "预留 iSH + fakefs + Alpine 运行时接缝"),
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
        SettingsSection(label: "状态") {
            SettingsRow(icon: .workflow, iconColor: Color(srgb: 0.3503,0.6649,0.9741), label: "当前后端",
                        value: store.linuxRuntime.backend, chevron: false)
            SettingsRow(label: "Rootfs", sub: store.linuxRuntime.detail,
                        value: store.linuxRuntime.rootfsState.label, chevron: false)
            SettingsRow(label: "版本", value: store.linuxRuntime.version ?? "未安装", chevron: false)
            SettingsRow(label: "体积", value: store.linuxRuntime.installedSizeBytes.map(formatBytes) ?? "—", chevron: false)
            SettingsRow(label: "托管目录", sub: store.linuxRuntime.managedRoot ?? "未创建",
                        value: store.linuxRuntime.badge, chevron: false, isLast: true)
        }
    }

    private var maintenanceSection: some View {
        SettingsSection(
            label: "维护",
            footer: "商店版不提供 Alpine / pip / npm 动态原生包安装入口。verify / repair / reset 在 phase-1 中仅返回受控状态，不会改动用户项目、会话或密钥。"
        ) {
            actionRow("刷新状态", action: .refresh, enabled: store.linuxRuntime.busyAction == nil)
            actionRow("校验 rootfs", action: .verify,
                      enabled: store.linuxRuntime.verifyAllowed && store.linuxRuntime.busyAction == nil)
            actionRow("修复 rootfs", action: .repair,
                      enabled: store.linuxRuntime.repairAllowed && store.linuxRuntime.busyAction == nil)
            actionRow("重置 rootfs", action: .reset,
                      enabled: store.linuxRuntime.resetAllowed && store.linuxRuntime.busyAction == nil,
                      isLast: true)
        }
    }

    private var workspaceSection: some View {
        SettingsSection(label: "工作区与终端") {
            SettingsRow(label: "终端入口",
                        sub: store.linuxRuntime.canOpenTerminal ? "可创建 PTY 终端会话" : "当前构建未提供可用 PTY 运行时",
                        value: store.linuxRuntime.canOpenTerminal ? "可用" : "已禁用",
                        chevron: false)
            SettingsRow(label: "外部目录挂载",
                        sub: store.linuxRuntime.writableGuestPaths.isEmpty
                            ? "当前无额外挂载；未来仅按目录授权开放"
                            : store.linuxRuntime.writableGuestPaths.prefix(2).joined(separator: " · "),
                        value: "\(store.linuxRuntime.writableGuestPaths.count) 项",
                        chevron: false)
            SettingsRow(label: "执行任务",
                        sub: store.linuxRuntime.lastActionMessage ?? "当前没有可停止的 Mobile Linux 任务",
                        value: store.linuxRuntime.lastAction?.label ?? "空闲",
                        chevron: false, isLast: true)
        }
    }

    private var terminalSection: some View {
        SettingsSection(label: "终端骨架", footer: "当前只接入宿主桥与 UI 骨架。未授权或未链接时，执行会返回明确 unavailable / blocked 错误。") {
            SettingsRow(label: "命令预览", sub: store.linuxRuntime.terminal.lastError ?? "使用 /bin/sh -lc 在 guest 中执行单条命令",
                        chevron: false) {
                EmptyView()
            }
            SettingsField(
                text: Binding(
                    get: { store.linuxRuntime.terminal.draftCommand },
                    set: { store.linuxRuntime.terminal.draftCommand = $0 }
                ),
                placeholder: "python3 --version"
            )
            .padding(.horizontal, 14)
            .padding(.bottom, 12)
            HStack(spacing: 8) {
                Button("执行") {
                    Task { await runCommandPreview() }
                }
                .disabled(store.linuxRuntime.busyAction != nil || store.linuxRuntime.terminal.draftCommand.isEmpty)
                Button("清空") {
                    store.linuxRuntime.terminal.lines.removeAll()
                    store.linuxRuntime.terminal.lastError = nil
                }
                .disabled(store.linuxRuntime.terminal.lines.isEmpty && store.linuxRuntime.terminal.lastError == nil)
            }
            .padding(.horizontal, 14)
            .padding(.bottom, 12)
            ForEach(store.linuxRuntime.terminal.lines.prefix(4)) { line in
                SettingsRow(
                    label: line.source.uppercased(),
                    sub: line.text,
                    value: line.isError ? "错误" : "输出",
                    valueColor: line.isError ? t.danger : t.text3,
                    chevron: false,
                    isLast: line.id == store.linuxRuntime.terminal.lines.prefix(4).last?.id
                )
            }
        }
    }

    private var tasksSection: some View {
        SettingsSection(label: "任务骨架") {
            if store.linuxRuntime.tasks.isEmpty {
                SettingsRow(label: "当前任务", sub: "尚无 guest 后台任务；真实 iSH 接入后这里显示 task 列表与停止入口",
                            value: "0", chevron: false, isLast: true)
            } else {
                ForEach(Array(store.linuxRuntime.tasks.enumerated()), id: \.element.id) { index, task in
                    SettingsRow(label: task.title, sub: task.detail,
                                value: task.state.label, chevron: false,
                                isLast: index == store.linuxRuntime.tasks.count - 1)
                }
            }
        }
    }

    private var mountsSection: some View {
        SettingsSection(label: "挂载骨架") {
            if store.linuxRuntime.mounts.isEmpty {
                SettingsRow(label: "挂载目录", sub: "当前无挂载；真实 runtime 接入后这里展示 workspace / memory / skills / shared",
                            value: "0", chevron: false, isLast: true)
            } else {
                ForEach(Array(store.linuxRuntime.mounts.enumerated()), id: \.element.id) { index, mount in
                    SettingsRow(label: mount.guestPath,
                                sub: mount.hostPath,
                                value: mount.readOnly ? "只读" : "可写",
                                chevron: false,
                                isLast: index == store.linuxRuntime.mounts.count - 1)
                }
            }
        }
    }

    private var safetySection: some View {
        SettingsSection(label: "安全说明") {
            SettingsRow(icon: .pin, label: "执行边界",
                        sub: "iSH / fakefs 不是安全边界；真实边界仍是 iOS App 沙箱与宿主策略",
                        chevron: false)
            SettingsRow(icon: .stop, label: "停止当前任务",
                        sub: "Phase-1 尚未接入真实后台进程句柄，因此没有可终止的 guest 任务",
                        value: "未启用", chevron: false, isLast: true)
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
            sub: store.linuxRuntime.busyAction == action ? "执行中…" : (enabled ? action.label : "当前构建未开放该操作"),
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
                Text("未启用").font(.system(size: 12, weight: .medium)).foregroundStyle(t.text4)
            }
        }
    }

    @MainActor
    private func run(_ action: LinuxRuntimeAction, mode: LinuxRuntimeMode) async {
        guard store.linuxRuntime.selectedMode == mode,
              store.linuxRuntime.busyAction == nil else { return }
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

    @MainActor
    private func runCommandPreview() async {
        let command = store.linuxRuntime.terminal.draftCommand.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !command.isEmpty else { return }
        switch await LinuxRuntimeBridge.runCommand(mode: store.linuxRuntime.selectedMode, command: command) {
        case .success(let line):
            store.linuxRuntime.terminal.lastError = nil
            store.linuxRuntime.terminal.lines.append(line)
        case .failure(let error):
            store.linuxRuntime.terminal.lastError = error.message
            store.linuxRuntime.terminal.lines.append(
                LinuxTerminalLine(
                    streamID: "run-preview",
                    source: "error",
                    text: error.message,
                    isError: true
                )
            )
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
