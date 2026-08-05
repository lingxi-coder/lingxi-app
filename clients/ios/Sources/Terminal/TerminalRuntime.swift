import Foundation
import Observation

enum TerminalRuntimeMode: Equatable, Sendable {
    case legacy
    case mobileLinux
}

enum TerminalMountPurpose: Equatable, Sendable {
    case workspace
    case memory
    case skills
    case shared
    case external
    case temp
}

enum TerminalRootfsState: Equatable, Sendable {
    case missing
    case installing
    case ready
    case corrupt
    case repairing
    case resetting
    case unsupported
    case blockedByLicense
}

enum TerminalTaskState: Equatable, Sendable {
    case running
    case completed
    case failed
    case cancelled
    case unavailable
}

enum TerminalStreamSource: Equatable, Sendable {
    case run
    case pty
}

enum TerminalEventKind: Equatable, Sendable {
    case stdoutLine
    case stderrChunk
    case exit
    case error
}

struct TerminalRuntimeConfig: Equatable, Sendable {
    var mode: TerminalRuntimeMode
    var managedRoot: String
    var workspaceHostPath: String
    var stableWorkspaceId: String
    var abi: String
    var rootfsVersion: String
    var archiveSha256: String?
    var authorizationFile: String?
}

struct TerminalWorkspaceDescriptor: Equatable, Sendable {
    var hostPath: String?
    var guestPath: String
    var displayName: String
}

struct TerminalMountSpec: Equatable, Sendable {
    var hostPath: String
    var guestPath: String
    var readOnly: Bool
    var purpose: TerminalMountPurpose
}

struct TerminalRuntimeDescriptor: Equatable, Sendable {
    var config: TerminalRuntimeConfig?
    var workspace: TerminalWorkspaceDescriptor
    var launchCommand: String = "/bin/sh"
    var launchArguments: [String] = ["-i"]
    var initialCommand: String? = nil
    var launchCwd: String? = nil
    var requestedCwdDisplay: String? = nil
    var invalidRequestedCwdMessage: String? = nil
    var environment: [String: String] = ["TERM": "xterm-256color"]
    var mounts: [TerminalMountSpec] = []
    var initialCols: UInt16 = 100
    var initialRows: UInt16 = 28
    var eventBatchLimit: UInt32 = 256
    var pollInterval: Duration = .milliseconds(250)
    var maxScrollback: Int = 2_000
    var taskRefreshInterval: Int = 4

    var workspaceRootGuestPath: String {
        "/workspace/\(config?.stableWorkspaceId ?? "")"
    }
}

extension TerminalRuntimeDescriptor {
    static func make(
        appSandboxRoot: String,
        project: ProjectSnapshot?,
        linuxRuntime: LinuxRuntimeState,
        initialCommand: String? = nil,
        requestedCwd: TerminalRouteCwd? = nil
    ) -> TerminalRuntimeDescriptor {
        let manifest = LXISHRuntimeBundleMetadata.current()
        // A shell does not belong to a project. With no project open the
        // terminal falls back to the runtime's own persistent workspace — the
        // same one Settings uses — instead of refusing to start. A fresh UUID
        // here would have minted a throwaway workspace on every launch.
        let projectId = project?.record.id ?? LXISHDefaultWorkspace.stableID()
        let workspaceHostPath = project?.workspace.hostURL.path
            ?? LXISHDefaultWorkspace.hostPath(id: projectId)
        let workspaceGuestPath = project?.workspace.guestPath
            ?? LXISHDefaultWorkspace.guestHome
        let managedRoot =
            linuxRuntime.managedRoot
            ?? URL(fileURLWithPath: appSandboxRoot, isDirectory: true)
            .appendingPathComponent("mobile-linux/ios-ish", isDirectory: true)
            .path

        let config = TerminalRuntimeConfig(
            mode: linuxRuntime.selectedMode == .legacy ? .legacy : .mobileLinux,
            managedRoot: managedRoot,
            workspaceHostPath: workspaceHostPath,
            stableWorkspaceId: projectId,
            abi: "arm64",
            rootfsVersion: manifest.rootfsVersion,
            archiveSha256: manifest.archiveSha256,
            authorizationFile: LXISHRuntimeBundleResources.authorizationManifestURL()?.path
        )

        let requestedResolution = resolveRequestedCwd(
            requestedCwd,
            workspaceGuestPath: workspaceGuestPath
        )

        return TerminalRuntimeDescriptor(
            config: workspaceHostPath.isEmpty ? nil : config,  // only if the container itself is unavailable
            workspace: TerminalWorkspaceDescriptor(
                hostPath: project?.workspace.hostURL.path ?? workspaceHostPath,
                guestPath: workspaceGuestPath,
                displayName: project?.record.name ?? "Linux"
            ),
            initialCommand: resolvedInitialCommand(
                explicit: initialCommand,
                fallback: linuxRuntime.terminal.draftCommand
            ),
            launchCwd: requestedResolution.cwd,
            requestedCwdDisplay: requestedCwd?.displayValue,
            invalidRequestedCwdMessage: requestedResolution.errorMessage,
            mounts: linuxRuntime.mounts.map(mapMount),
            initialCols: 100,
            initialRows: 28,
            eventBatchLimit: 256,
            pollInterval: .milliseconds(250),
            maxScrollback: 2_000,
            taskRefreshInterval: 4
        )
    }

    private static func mapMount(_ mount: LinuxRuntimeMountRow) -> TerminalMountSpec {
        TerminalMountSpec(
            hostPath: mount.hostPath,
            guestPath: mount.guestPath,
            readOnly: mount.readOnly,
            purpose: inferPurpose(for: mount.guestPath)
        )
    }

    private static func inferPurpose(for guestPath: String) -> TerminalMountPurpose {
        if guestPath.hasPrefix("/workspace/") { return .workspace }
        if guestPath.contains("/memory") { return .memory }
        if guestPath.contains("/skills") { return .skills }
        if guestPath.contains("/tmp") { return .temp }
        if guestPath.contains("/shared") { return .shared }
        return .external
    }

    private static func normalizedInitialCommand(_ value: String?) -> String? {
        guard let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines), !trimmed.isEmpty else {
            return nil
        }
        return trimmed
    }

    private static func resolvedInitialCommand(
        explicit: String?,
        fallback: String?
    ) -> String? {
        normalizedInitialCommand(explicit) ?? normalizedInitialCommand(fallback)
    }

    private static func resolveRequestedCwd(
        _ requestedCwd: TerminalRouteCwd?,
        workspaceGuestPath: String
    ) -> (cwd: String?, errorMessage: String?) {
        guard let requestedCwd else {
            return (
                workspaceGuestPath.isEmpty ? LXISHDefaultWorkspace.guestHome : workspaceGuestPath,
                nil
            )
        }
        // Only a cwd the CALLER asked for can be unsatisfiable. "No project" is
        // no longer a failure — it just means the shell opens in its own home.
        guard !workspaceGuestPath.isEmpty else {
            return (nil, String(localized: "terminal_cwd_requires_open_project \(requestedCwd.displayValue)"))
        }

        switch requestedCwd {
        case let .guestPath(path):
            let normalized = normalizeGuestPath(path)
            guard isWithinWorkspace(normalized, workspaceGuestPath: workspaceGuestPath) else {
                return (nil, String(localized: "terminal_cwd_outside_workspace \(path)"))
            }
            return (normalized, nil)
        case let .workspaceRelative(path):
            guard let normalizedRelative = normalizeRelativePath(path) else {
                return (nil, String(localized: "terminal_relative_cwd_invalid \(path)"))
            }
            let combined = normalizeGuestPath(
                workspaceGuestPath + (normalizedRelative.isEmpty ? "" : "/\(normalizedRelative)")
            )
            guard isWithinWorkspace(combined, workspaceGuestPath: workspaceGuestPath) else {
                return (nil, String(localized: "terminal_cwd_outside_workspace \(path)"))
            }
            return (combined, nil)
        }
    }

    private static func normalizeGuestPath(_ path: String) -> String {
        let segments = path.split(separator: "/", omittingEmptySubsequences: true)
        var stack: [Substring] = []
        for segment in segments {
            switch segment {
            case ".":
                continue
            case "..":
                if !stack.isEmpty {
                    stack.removeLast()
                }
            default:
                stack.append(segment)
            }
        }
        return "/" + stack.joined(separator: "/")
    }

    private static func normalizeRelativePath(_ path: String) -> String? {
        guard !path.hasPrefix("/") else { return nil }
        let segments = path.split(separator: "/", omittingEmptySubsequences: false)
        var stack: [Substring] = []
        for segment in segments {
            switch segment {
            case "", ".":
                continue
            case "..":
                guard !stack.isEmpty else { return nil }
                stack.removeLast()
            default:
                stack.append(segment)
            }
        }
        return stack.joined(separator: "/")
    }

    private static func isWithinWorkspace(
        _ candidate: String,
        workspaceGuestPath: String
    ) -> Bool {
        candidate == workspaceGuestPath || candidate.hasPrefix(workspaceGuestPath + "/")
    }
}

struct TerminalCapabilitySnapshot: Equatable, Sendable {
    var available: Bool
    var backend: String
    var mode: TerminalRuntimeMode
    var reason: String?
    var streamingOutput: Bool
    var backgroundProcesses: Bool
    var pty: Bool
    var bindMounts: Bool
    var rootfsIntegrity: Bool
}

struct TerminalStatusSnapshot: Equatable, Sendable {
    var state: TerminalRootfsState
    var backend: String
    var mode: TerminalRuntimeMode
    var platform: String
    var abi: String
    var version: String?
    var managedRoot: String?
    var activeRoot: String?
    var stagedRoot: String?
    var archiveSha256: String?
    var installedSizeBytes: UInt64?
    var writableGuestPaths: [String]
    var lastError: String?
}

struct TerminalTaskSnapshot: Identifiable, Equatable, Sendable {
    var id: String
    var title: String
    var state: TerminalTaskState
    var detail: String?
}

struct TerminalPtyOpenRequest: Equatable, Sendable {
    var command: String
    var args: [String]
    var cwd: String?
    var env: [String: String]
    var cols: UInt16
    var rows: UInt16
    var mounts: [TerminalMountSpec]
}

struct TerminalPtySession: Equatable, Sendable {
    var id: String
    var available: Bool
    var detail: String?
}

struct TerminalStreamEvent: Equatable, Sendable {
    var sequence: UInt64
    var taskId: String?
    var streamId: String
    var source: TerminalStreamSource
    var kind: TerminalEventKind
    var text: String?
    var data: Data?
    var exitCode: Int32?
    var timedOut: Bool
}

enum TerminalRuntimeError: LocalizedError, Equatable, Sendable {
    case unsupported
    case unavailable(String)
    case integrityFailure(String)
    case invalidRequest(String)
    case io(String)
    case timeout

    var errorDescription: String? {
        switch self {
        case .unsupported:
            return String(localized: "terminal_pty_not_available_build")
        case let .unavailable(message),
            let .integrityFailure(message),
            let .invalidRequest(message),
            let .io(message):
            return message
        case .timeout:
            return String(localized: "terminal_operation_timeout")
        }
    }

    init(_ error: Error) {
        if let runtimeError = error as? TerminalRuntimeError {
            self = runtimeError
        } else {
            self = .io(String(describing: error))
        }
    }
}

protocol TerminalRuntimeClient: Sendable {
    func probe(config: TerminalRuntimeConfig?) async -> TerminalCapabilitySnapshot
    func status(config: TerminalRuntimeConfig?) async -> TerminalStatusSnapshot
    func listTasks(config: TerminalRuntimeConfig?) async throws -> [TerminalTaskSnapshot]
    func openPty(config: TerminalRuntimeConfig?, request: TerminalPtyOpenRequest) async throws -> TerminalPtySession
    func readEvents(config: TerminalRuntimeConfig?, afterSequence: UInt64?, limit: UInt32?) async throws -> [TerminalStreamEvent]
    func writePty(config: TerminalRuntimeConfig?, sessionId: String, data: Data) async throws
    func resizePty(config: TerminalRuntimeConfig?, sessionId: String, cols: UInt16, rows: UInt16) async throws
    func closePty(config: TerminalRuntimeConfig?, sessionId: String) async throws
}

enum LinuxRuntimeTaskOperationKind: Equatable, Sendable {
    case refreshTasks
    case stopTask(String)
}

struct LinuxRuntimeTaskOperationResult: Equatable, Sendable {
    var tasks: [LinuxRuntimeTaskRow]
    var message: String
}

struct LinuxRuntimeTaskOperationFailure: LocalizedError, Equatable, Sendable {
    let message: String

    var errorDescription: String? { message }
}

protocol LinuxRuntimeTaskOperating: Sendable {
    func refreshTasks(mode: LinuxRuntimeMode) async throws -> LinuxRuntimeTaskOperationResult
    func stopTask(mode: LinuxRuntimeMode, taskID: String) async throws -> LinuxRuntimeTaskOperationResult
}

@MainActor
@Observable
final class LinuxRuntimeTaskOperationsModel {
    private let adapter: any LinuxRuntimeTaskOperating
    private(set) var busyOperation: LinuxRuntimeTaskOperationKind?
    private(set) var errorMessage: String?

    init(adapter: any LinuxRuntimeTaskOperating) {
        self.adapter = adapter
    }

    var isBusy: Bool { busyOperation != nil }

    func isStopping(_ taskID: String) -> Bool {
        busyOperation == .stopTask(taskID)
    }

    func refreshTasks(mode: LinuxRuntimeMode) async -> LinuxRuntimeTaskOperationResult? {
        guard busyOperation == nil else { return nil }
        busyOperation = .refreshTasks
        errorMessage = nil
        defer { busyOperation = nil }
        do {
            return try await adapter.refreshTasks(mode: mode)
        } catch {
            errorMessage = error.localizedDescription
            return nil
        }
    }

    func stopTask(mode: LinuxRuntimeMode, taskID: String) async -> LinuxRuntimeTaskOperationResult? {
        guard busyOperation == nil else { return nil }
        busyOperation = .stopTask(taskID)
        errorMessage = nil
        defer { busyOperation = nil }
        do {
            return try await adapter.stopTask(mode: mode, taskID: taskID)
        } catch {
            errorMessage = error.localizedDescription
            return nil
        }
    }
}
