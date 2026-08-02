import Foundation

protocol TerminalRuntimeHandle: Sendable {
    func capability() async -> MobileLinuxCapabilityFfi
    func status() async throws -> MobileLinuxStatusFfi
    func listTasks() async throws -> [MobileLinuxTaskFfi]
    func openPty(request: MobileLinuxPtyOpenRequestFfi) async throws -> MobileLinuxPtySessionFfi
    func readEvents(afterSequence: UInt64?, limit: UInt32?) async throws -> [MobileLinuxStreamEventFfi]
    func writePty(sessionId: String, data: Data) async throws
    func resizePty(sessionId: String, cols: UInt16, rows: UInt16) async throws
    func closePty(sessionId: String) async throws
}

/// Safety invariant:
/// - the Rust handle owns an `Arc<dyn MobileLinuxRuntime + Send + Sync>`
/// - `FfiTerminalRuntimeClient` retains one wrapper for the whole PTY lifecycle
/// - `@unchecked` only bridges the generated UniFFI protocol's missing
///   `Sendable` conformance; thread safety is provided by the Rust handle
private final class LiveTerminalRuntimeHandle: @unchecked Sendable, TerminalRuntimeHandle {
    private let base: IosMobileLinuxRuntimeHandleProtocol

    init(base: IosMobileLinuxRuntimeHandleProtocol) {
        self.base = base
    }

    func capability() async -> MobileLinuxCapabilityFfi {
        await base.capability()
    }

    func status() async throws -> MobileLinuxStatusFfi {
        try await base.status()
    }

    func listTasks() async throws -> [MobileLinuxTaskFfi] {
        try await base.listTasks()
    }

    func openPty(request: MobileLinuxPtyOpenRequestFfi) async throws -> MobileLinuxPtySessionFfi {
        try await base.openPty(request: request)
    }

    func readEvents(afterSequence: UInt64?, limit: UInt32?) async throws -> [MobileLinuxStreamEventFfi] {
        try await base.readEvents(afterSequence: afterSequence, limit: limit)
    }

    func writePty(sessionId: String, data: Data) async throws {
        try await base.writePty(sessionId: sessionId, data: data)
    }

    func resizePty(sessionId: String, cols: UInt16, rows: UInt16) async throws {
        try await base.resizePty(sessionId: sessionId, cols: cols, rows: rows)
    }

    func closePty(sessionId: String) async throws {
        try await base.closePty(sessionId: sessionId)
    }
}

protocol TerminalRuntimeFFIBridge: Sendable {
    func createRuntime(config: TerminalRuntimeConfig) throws -> any TerminalRuntimeHandle
    func probe(config: TerminalRuntimeConfig?) -> MobileLinuxCapabilityFfi
    func status(config: TerminalRuntimeConfig?) -> MobileLinuxStatusFfi
}

private struct LiveTerminalRuntimeFFIBridge: TerminalRuntimeFFIBridge {
    func createRuntime(config: TerminalRuntimeConfig) throws -> any TerminalRuntimeHandle {
        LiveTerminalRuntimeHandle(base: try createIosMobileLinuxRuntime(config: map(config)))
    }

    func probe(config: TerminalRuntimeConfig?) -> MobileLinuxCapabilityFfi {
        probeIosMobileLinux(config: map(config))
    }

    func status(config: TerminalRuntimeConfig?) -> MobileLinuxStatusFfi {
        iosMobileLinuxStatus(config: map(config))
    }

    private func map(_ config: TerminalRuntimeConfig?) -> IosMobileLinuxConfigFfi? {
        config.map(map)
    }

    private func map(_ config: TerminalRuntimeConfig) -> IosMobileLinuxConfigFfi {
        IosMobileLinuxConfigFfi(
            mode: map(config.mode),
            managedRoot: config.managedRoot,
            workspaceHostPath: config.workspaceHostPath,
            stableWorkspaceId: config.stableWorkspaceId,
            abi: config.abi,
            rootfsVersion: config.rootfsVersion,
            archiveSha256: config.archiveSha256,
            authorizationFile: config.authorizationFile
        )
    }

    private func map(_ value: TerminalRuntimeMode) -> MobileLinuxRuntimeModeFfi {
        switch value {
        case .legacy: return .legacy
        case .mobileLinux: return .mobileLinux
        }
    }
}

@MainActor
final class FfiTerminalRuntimeClient: TerminalRuntimeClient {
    private let bridge: TerminalRuntimeFFIBridge
    private var cachedConfig: TerminalRuntimeConfig?
    private var cachedHandle: (any TerminalRuntimeHandle)?

    init(bridge: TerminalRuntimeFFIBridge = LiveTerminalRuntimeFFIBridge()) {
        self.bridge = bridge
    }

    func probe(config: TerminalRuntimeConfig?) async -> TerminalCapabilitySnapshot {
        if let config, config.mode == .mobileLinux {
            do {
                let handle = try runtimeHandle(for: config)
                return map(await handle.capability())
            } catch {
                return map(bridge.probe(config: config))
            }
        }
        return map(bridge.probe(config: config))
    }

    func status(config: TerminalRuntimeConfig?) async -> TerminalStatusSnapshot {
        if let config, config.mode == .mobileLinux {
            do {
                let handle = try runtimeHandle(for: config)
                return map(try await handle.status())
            } catch {
                return map(bridge.status(config: config))
            }
        }
        return map(bridge.status(config: config))
    }

    func listTasks(config: TerminalRuntimeConfig?) async throws -> [TerminalTaskSnapshot] {
        do {
            let handle = try runtimeHandle(for: config)
            return try await handle.listTasks().map(map)
        } catch {
            throw mapRuntimeError(error)
        }
    }

    func openPty(config: TerminalRuntimeConfig?, request: TerminalPtyOpenRequest) async throws -> TerminalPtySession {
        do {
            let handle = try runtimeHandle(for: config)
            return map(try await handle.openPty(request: map(request)))
        } catch {
            throw mapRuntimeError(error)
        }
    }

    func readEvents(config: TerminalRuntimeConfig?, afterSequence: UInt64?, limit: UInt32?) async throws -> [TerminalStreamEvent] {
        do {
            let handle = try runtimeHandle(for: config)
            return try await handle.readEvents(afterSequence: afterSequence, limit: limit).map(map)
        } catch {
            throw mapRuntimeError(error)
        }
    }

    func writePty(config: TerminalRuntimeConfig?, sessionId: String, data: Data) async throws {
        do {
            let handle = try runtimeHandle(for: config)
            try await handle.writePty(sessionId: sessionId, data: data)
        } catch {
            throw mapRuntimeError(error)
        }
    }

    func resizePty(config: TerminalRuntimeConfig?, sessionId: String, cols: UInt16, rows: UInt16) async throws {
        do {
            let handle = try runtimeHandle(for: config)
            try await handle.resizePty(sessionId: sessionId, cols: cols, rows: rows)
        } catch {
            throw mapRuntimeError(error)
        }
    }

    func closePty(config: TerminalRuntimeConfig?, sessionId: String) async throws {
        do {
            let handle = try runtimeHandle(for: config)
            try await handle.closePty(sessionId: sessionId)
        } catch {
            throw mapRuntimeError(error)
        }
    }

    private func runtimeHandle(for config: TerminalRuntimeConfig?) throws -> any TerminalRuntimeHandle {
        guard let config else {
            cachedConfig = nil
            cachedHandle = nil
            throw TerminalRuntimeError.unavailable("legacy unavailable backend selected")
        }
        guard config.mode == .mobileLinux else {
            cachedConfig = nil
            cachedHandle = nil
            throw TerminalRuntimeError.unavailable("legacy unavailable backend selected")
        }
        if let cachedHandle, cachedConfig == config {
            return cachedHandle
        }
        let handle = try bridge.createRuntime(config: config)
        cachedConfig = config
        cachedHandle = handle
        return handle
    }

    private func mapRuntimeError(_ error: Error) -> TerminalRuntimeError {
        if let ffiError = error as? MobileLinuxOperationFfiError {
            switch ffiError {
            case .Unsupported:
                return .unsupported
            case let .Unavailable(message), let .LicenseBlocked(message):
                return .unavailable(message)
            case let .InvalidRequest(message):
                return .invalidRequest(message)
            case let .Io(message):
                return .io(message)
            case .Timeout:
                return .timeout
            }
        }
        return TerminalRuntimeError(error)
    }

    private func map(_ request: TerminalPtyOpenRequest) -> MobileLinuxPtyOpenRequestFfi {
        MobileLinuxPtyOpenRequestFfi(
            command: request.command,
            args: request.args,
            cwd: request.cwd,
            env: request.env,
            cols: request.cols,
            rows: request.rows,
            mounts: request.mounts.map(map)
        )
    }

    private func map(_ mount: TerminalMountSpec) -> MobileLinuxMountSpecFfi {
        MobileLinuxMountSpecFfi(
            hostPath: mount.hostPath,
            guestPath: mount.guestPath,
            readOnly: mount.readOnly,
            purpose: map(mount.purpose)
        )
    }

    private func map(_ value: TerminalMountPurpose) -> MobileLinuxMountPurposeFfi {
        switch value {
        case .workspace: return .workspace
        case .memory: return .memory
        case .skills: return .skills
        case .shared: return .shared
        case .external: return .external
        case .temp: return .temp
        }
    }

    private func map(_ value: TerminalRuntimeMode) -> MobileLinuxRuntimeModeFfi {
        switch value {
        case .legacy: return .legacy
        case .mobileLinux: return .mobileLinux
        }
    }

    private func map(_ capability: MobileLinuxCapabilityFfi) -> TerminalCapabilitySnapshot {
        TerminalCapabilitySnapshot(
            available: capability.available,
            backend: capability.backend,
            mode: map(capability.mode),
            reason: capability.reason,
            streamingOutput: capability.streamingOutput,
            backgroundProcesses: capability.backgroundProcesses,
            pty: capability.pty,
            bindMounts: capability.bindMounts,
            rootfsIntegrity: capability.rootfsIntegrity
        )
    }

    private func map(_ status: MobileLinuxStatusFfi) -> TerminalStatusSnapshot {
        TerminalStatusSnapshot(
            state: map(status.state),
            backend: status.backend,
            mode: map(status.mode),
            platform: status.platform,
            abi: status.abi,
            version: status.version,
            managedRoot: status.managedRoot,
            activeRoot: status.activeRoot,
            stagedRoot: status.stagedRoot,
            archiveSha256: status.archiveSha256,
            installedSizeBytes: status.installedSizeBytes,
            writableGuestPaths: status.writableGuestPaths,
            lastError: status.lastError
        )
    }

    private func map(_ task: MobileLinuxTaskFfi) -> TerminalTaskSnapshot {
        TerminalTaskSnapshot(
            id: task.id,
            title: task.title,
            state: map(task.state),
            detail: task.detail
        )
    }

    private func map(_ session: MobileLinuxPtySessionFfi) -> TerminalPtySession {
        TerminalPtySession(id: session.id, available: session.available, detail: session.detail)
    }

    private func map(_ event: MobileLinuxStreamEventFfi) -> TerminalStreamEvent {
        TerminalStreamEvent(
            sequence: event.sequence,
            taskId: event.taskId,
            streamId: event.streamId,
            source: map(event.source),
            kind: map(event.kind),
            text: event.text,
            data: event.data,
            exitCode: event.exitCode,
            timedOut: event.timedOut
        )
    }

    private func map(_ value: MobileLinuxRuntimeModeFfi) -> TerminalRuntimeMode {
        switch value {
        case .legacy: return .legacy
        case .mobileLinux: return .mobileLinux
        }
    }

    private func map(_ value: MobileLinuxRootfsStateFfi) -> TerminalRootfsState {
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

    private func map(_ value: MobileLinuxTaskStateFfi) -> TerminalTaskState {
        switch value {
        case .running: return .running
        case .completed: return .completed
        case .failed: return .failed
        case .cancelled: return .cancelled
        case .unavailable: return .unavailable
        }
    }

    private func map(_ value: MobileLinuxStreamSourceFfi) -> TerminalStreamSource {
        switch value {
        case .run: return .run
        case .pty: return .pty
        }
    }

    private func map(_ value: MobileLinuxStreamEventKindFfi) -> TerminalEventKind {
        switch value {
        case .stdoutLine: return .stdoutLine
        case .stderrChunk: return .stderrChunk
        case .exit: return .exit
        case .error: return .error
        }
    }
}

extension TerminalSessionModel {
    convenience init(descriptor: TerminalRuntimeDescriptor) {
        self.init(descriptor: descriptor, client: FfiTerminalRuntimeClient())
    }
}

extension TerminalView {
    init(
        descriptor: TerminalRuntimeDescriptor,
        onDismiss: (() -> Void)? = nil,
        onOpenRuntimeSettings: (() -> Void)? = nil,
        onRepairRuntime: (() -> Void)? = nil
    ) {
        self.init(
            descriptor: descriptor,
            client: FfiTerminalRuntimeClient(),
            onDismiss: onDismiss,
            onOpenRuntimeSettings: onOpenRuntimeSettings,
            onRepairRuntime: onRepairRuntime
        )
    }
}
