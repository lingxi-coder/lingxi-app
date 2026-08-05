import Foundation
import Observation
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// What the terminal offers the user when it cannot run.
///
/// Extracted from the view because it is a decision, not a layout: the states
/// `unavailable` and `workspaceUnavailable` previously shared one button, so a
/// terminal opened without a project offered "打开运行时设置" — a screen that
/// cannot create a project. Naming the cause and the remedy in one place makes
/// that class of mismatch testable.
enum TerminalRecovery: Equatable {
    /// The guest workspace/cwd is not mounted. Not a missing project — a shell
    /// no longer needs one — so the remedy is to inspect the runtime's mounts.
    case workspaceNotMounted
    /// The Linux runtime itself is not usable.
    case runtimeUnavailable
    /// The rootfs is present but damaged.
    case repairRuntime
    /// Nothing the user can act on from here.
    case none

    /// Headline naming the CAUSE. Two causes may share a button while still
    /// needing different words: "运行时不可用" and "工作目录不可用" are both
    /// fixed from the runtime page, but confusing them wastes the reader's time.
    var title: String {
        switch self {
        case .workspaceNotMounted: String(localized: "terminal_recovery_workspace_unavailable_title")
        case .runtimeUnavailable: String(localized: "terminal_recovery_runtime_unavailable_title")
        case .repairRuntime: String(localized: "terminal_recovery_repair_title")
        case .none: String(localized: "terminal_recovery_none_title")
        }
    }

    static func forState(_ state: TerminalAvailabilityState) -> TerminalRecovery {
        switch state {
        case .workspaceUnavailable: .workspaceNotMounted
        case .unavailable: .runtimeUnavailable
        case .integrityFailure: .repairRuntime
        case .invalidRequest, .failed: .none
        case .idle, .opening, .ready, .closed: .none
        }
    }
}

enum TerminalAvailabilityState: Equatable, Sendable {
    case idle
    case opening
    case ready
    case unavailable(String)
    case integrityFailure(String)
    case workspaceUnavailable(String)
    case invalidRequest(String)
    case closed
    case failed(String)

    var canInteract: Bool {
        switch self {
        case .ready:
            return true
        default:
            return false
        }
    }

    var message: String? {
        switch self {
        case let .unavailable(message),
            let .integrityFailure(message),
            let .workspaceUnavailable(message),
            let .invalidRequest(message),
            let .failed(message):
            return message
        case .idle, .opening, .ready, .closed:
            return nil
        }
    }
}

struct TerminalExitStatus: Equatable, Sendable {
    var code: Int32?
    var timedOut: Bool
}

@MainActor
@Observable
final class TerminalSessionModel {
    let descriptor: TerminalRuntimeDescriptor
    private let client: TerminalRuntimeClient

    private(set) var availability: TerminalAvailabilityState = .idle
    private(set) var capability: TerminalCapabilitySnapshot?
    private(set) var status: TerminalStatusSnapshot?
    private(set) var tasks: [TerminalTaskSnapshot] = []
    private(set) var buffer: TerminalScreenBuffer
    private(set) var activeSessionID: String?
    private(set) var lastExitStatus: TerminalExitStatus?
    private(set) var lastError: String?
    private(set) var lastSequence: UInt64?
    private(set) var isPolling = false
    var inputText = ""
    var history: [String] = []
    var historyCursor: Int?
    var selectionSummary: String?

    private var decoder = TerminalUTF8Decoder()
    private var parser = TerminalANSIParser()
    private var pollingTask: Task<Void, Never>?

    init(
        descriptor: TerminalRuntimeDescriptor,
        client: TerminalRuntimeClient
    ) {
        self.descriptor = descriptor
        self.client = client
        self.buffer = TerminalScreenBuffer(maxScrollback: descriptor.maxScrollback)
    }

    func startIfNeeded() async {
        guard activeSessionID == nil, !isPolling else { return }
        availability = .opening
        lastError = nil
        lastExitStatus = nil

        let capability = await client.probe(config: descriptor.config)
        let status = await client.status(config: descriptor.config)
        self.capability = capability
        self.status = status

        guard case .ready = validateAvailability(capability: capability, status: status) else {
            availability = validateAvailability(capability: capability, status: status)
            tasks = (try? await client.listTasks(config: descriptor.config)) ?? []
            return
        }

        do {
            tasks = try await client.listTasks(config: descriptor.config)
            let session = try await client.openPty(
                config: descriptor.config,
                request: TerminalPtyOpenRequest(
                    command: descriptor.launchCommand,
                    args: descriptor.launchArguments,
                    cwd: descriptor.launchCwd,
                    env: descriptor.environment,
                    cols: descriptor.initialCols,
                    rows: descriptor.initialRows,
                    mounts: descriptor.mounts
                )
            )
            guard session.available else {
                availability = .unavailable(session.detail ?? String(localized: "terminal_pty_session_unavailable"))
                return
            }
            activeSessionID = session.id
            availability = .ready
            startPollingLoop()
            if let initialCommand = descriptor.initialCommand {
                appendToHistory(initialCommand)
                Task { [weak self] in
                    await self?.sendInput(initialCommand + "\n")
                }
            }
        } catch {
            let runtimeError = TerminalRuntimeError(error)
            availability = .failed(runtimeError.errorDescription ?? String(localized: "terminal_launch_failed"))
            lastError = runtimeError.errorDescription
        }
    }

    func close() async {
        pollingTask?.cancel()
        pollingTask = nil
        isPolling = false
        guard let activeSessionID else {
            availability = .closed
            return
        }
        do {
            try await client.closePty(config: descriptor.config, sessionId: activeSessionID)
        } catch {
            lastError = TerminalRuntimeError(error).errorDescription
        }
        self.activeSessionID = nil
        availability = .closed
    }

    func refreshTasks() async {
        do {
            tasks = try await client.listTasks(config: descriptor.config)
        } catch {
            lastError = TerminalRuntimeError(error).errorDescription
        }
    }

    func resize(cols: UInt16, rows: UInt16) async {
        guard let activeSessionID else { return }
        do {
            try await client.resizePty(config: descriptor.config, sessionId: activeSessionID, cols: cols, rows: rows)
        } catch {
            lastError = TerminalRuntimeError(error).errorDescription
        }
    }

    func sendInput(_ input: String) async {
        guard let activeSessionID else { return }
        do {
            try await client.writePty(config: descriptor.config, sessionId: activeSessionID, data: Data(input.utf8))
        } catch {
            lastError = TerminalRuntimeError(error).errorDescription
        }
    }

    func submitInput() async {
        let command = inputText
        guard !command.isEmpty else { return }
        appendToHistory(command)
        inputText = ""
        await sendInput(command + "\n")
    }

    func sendControl(_ key: TerminalControlKey) async {
        await sendInput(String(decoding: [key.byte], as: UTF8.self))
    }

    func previousHistory() {
        guard !history.isEmpty else { return }
        let nextIndex = max((historyCursor ?? history.count) - 1, 0)
        historyCursor = nextIndex
        inputText = history[nextIndex]
    }

    func nextHistory() {
        guard !history.isEmpty else { return }
        guard let historyCursor else { return }
        let nextIndex = historyCursor + 1
        if nextIndex >= history.count {
            self.historyCursor = nil
            inputText = ""
        } else {
            self.historyCursor = nextIndex
            inputText = history[nextIndex]
        }
    }

    func copyTranscript() {
        let transcript = buffer.plainText
        guard !transcript.isEmpty else { return }
        selectionSummary = String(localized: "terminal_copied_lines \(buffer.lines.count)")
        copyToSystemPasteboard(transcript)
    }

    private func appendToHistory(_ command: String) {
        if history.last != command {
            history.append(command)
        }
        historyCursor = nil
    }

    private func startPollingLoop() {
        guard let streamID = activeSessionID else { return }
        isPolling = true
        pollingTask?.cancel()
        pollingTask = Task { [weak self] in
            guard let self else { return }
            var iteration = 0
            while !Task.isCancelled {
                do {
                    let events = try await client.readEvents(
                        config: descriptor.config,
                        afterSequence: lastSequence,
                        limit: descriptor.eventBatchLimit
                    )
                    let reachedTerminalEvent = await MainActor.run {
                        guard self.activeSessionID == streamID else { return true }
                        return self.consume(events: events, expectedStreamID: streamID)
                    }
                    if reachedTerminalEvent { break }
                    iteration += 1
                    if iteration % max(1, descriptor.taskRefreshInterval) == 0 {
                        let tasks = try await client.listTasks(config: descriptor.config)
                        await MainActor.run {
                            guard self.activeSessionID == streamID else { return }
                            self.tasks = tasks
                        }
                    }
                } catch {
                    await MainActor.run {
                        guard self.activeSessionID == streamID else { return }
                        self.lastError = TerminalRuntimeError(error).errorDescription
                        self.availability = .failed(self.lastError ?? String(localized: "terminal_polling_failed"))
                    }
                    break
                }

                try? await Task.sleep(for: descriptor.pollInterval)
            }

            await MainActor.run {
                if self.activeSessionID == streamID || self.activeSessionID == nil {
                    self.isPolling = false
                }
            }
        }
    }

    /// Consume one process-global event-journal page. The read cursor belongs to
    /// the journal, not to a single PTY, so it must advance past unrelated
    /// streams as well; otherwise an unrelated event at the head of a full page
    /// is returned forever and this terminal never reaches its own later output.
    /// Returns `true` once this PTY has emitted its terminal exit event.
    private func consume(events: [TerminalStreamEvent], expectedStreamID: String) -> Bool {
        if let newestSequence = events.map(\.sequence).max() {
            lastSequence = max(lastSequence ?? 0, newestSequence)
        }
        var reachedTerminalEvent = false
        for event in events where event.streamId == expectedStreamID {
            switch event.kind {
            case .stdoutLine, .stderrChunk:
                if let data = event.data, !data.isEmpty {
                    let text = decoder.append(data)
                    if !text.isEmpty {
                        parser.consume(text: text, buffer: &buffer)
                    }
                } else if let text = event.text, !text.isEmpty {
                    parser.consume(text: text, buffer: &buffer)
                }
            case .error:
                let message = event.text ?? String(localized: "terminal_run_failed")
                lastError = message
                availability = .failed(message)
            case .exit:
                lastExitStatus = TerminalExitStatus(code: event.exitCode, timedOut: event.timedOut)
                availability = .closed
                activeSessionID = nil
                reachedTerminalEvent = true
            }
        }
        return reachedTerminalEvent
    }

    private func validateAvailability(
        capability: TerminalCapabilitySnapshot,
        status: TerminalStatusSnapshot
    ) -> TerminalAvailabilityState {
        if let invalidRequestedCwdMessage = descriptor.invalidRequestedCwdMessage {
            return .invalidRequest(invalidRequestedCwdMessage)
        }
        // Reachable only if the descriptor was built without the runtime's own
        // home fallback; a shell no longer requires a project to have a cwd.
        if descriptor.workspace.guestPath.isEmpty {
            return .workspaceUnavailable(String(localized: "terminal_guest_workspace_unavailable"))
        }
        if !capability.available {
            return .unavailable(capability.reason ?? String(localized: "terminal_runtime_environment_unavailable"))
        }
        if !capability.pty {
            return .unavailable(String(localized: "terminal_pty_not_supported"))
        }
        if status.state == .corrupt {
            return .integrityFailure(status.lastError ?? String(localized: "terminal_integrity_check_failed"))
        }
        if status.state == .blockedByLicense {
            return .unavailable(status.lastError ?? String(localized: "terminal_authorization_failed"))
        }
        if status.state == .unsupported {
            return .unavailable(status.lastError ?? String(localized: "terminal_mobile_linux_not_connected"))
        }
        let workspacePath = descriptor.launchCwd ?? descriptor.workspace.guestPath
        let matches = status.writableGuestPaths.contains { candidate in
            workspacePath == candidate || workspacePath.hasPrefix(candidate + "/")
        }
        guard matches else {
            return .workspaceUnavailable(String(localized: "terminal_workspace_not_mounted"))
        }
        return .ready
    }
}

private func copyToSystemPasteboard(_ transcript: String) {
    #if canImport(UIKit)
        UIPasteboard.general.string = transcript
    #elseif canImport(AppKit)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(transcript, forType: .string)
    #endif
}

enum TerminalControlKey: String, CaseIterable, Identifiable, Sendable {
    case interrupt = "Ctrl-C"
    case endOfFile = "Ctrl-D"
    case suspend = "Ctrl-Z"
    case escape = "Esc"

    var id: String { rawValue }

    var byte: UInt8 {
        switch self {
        case .interrupt: return 0x03
        case .endOfFile: return 0x04
        case .suspend: return 0x1A
        case .escape: return 0x1B
        }
    }
}
