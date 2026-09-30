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
    /// The session has stopped (shell exited or a mid-session failure) and can
    /// be started again in place. The ONE decision table: the view's restart
    /// link and the model's `canRestart` both derive from this case — an
    /// earlier build kept restart in a parallel `canRestart` switch, so every
    /// new availability state had to be classified consistently in two places
    /// with nothing detecting a mismatch.
    case restartSession
    /// Nothing the user can act on from here.
    case none

    /// Headline naming the CAUSE. Two causes may share a button while still
    /// needing different words: "运行时不可用" and "工作目录不可用" are both
    /// fixed from the runtime page, but confusing them wastes the reader's time.
    ///
    /// `.restartSession` deliberately has NO headline (`nil`): its states
    /// carry their own line (the exit notice, or the failure message rendered
    /// bare) — prefixing "终端不可用" one row above a working restart link
    /// told users the terminal was beyond help and they popped the screen.
    var title: String? {
        switch self {
        case .workspaceNotMounted: String(localized: "terminal_recovery_workspace_unavailable_title")
        case .runtimeUnavailable: String(localized: "terminal_recovery_runtime_unavailable_title")
        case .repairRuntime: String(localized: "terminal_recovery_repair_title")
        case .restartSession, .none: nil
        }
    }

    static func forState(_ state: TerminalAvailabilityState) -> TerminalRecovery {
        switch state {
        case .workspaceUnavailable: .workspaceNotMounted
        case .unavailable: .runtimeUnavailable
        case .integrityFailure: .repairRuntime
        // `.unavailable`/`.integrityFailure` are NOT restartable on purpose —
        // reopening a PTY does not install a rootfs; those offer the runtime
        // page instead.
        case .closed, .failed: .restartSession
        case .invalidRequest: .none
        case .idle, .opening, .ready: .none
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
    /// Whatever the runtime said about the exit. The iSH kernel never reports a
    /// guest process exit at all — `pty_closed` is emitted only by `closePty`,
    /// i.e. by US — so a bare "exited with code 0" is indistinguishable between
    /// "your shell ended" and "something tore the session down". Carrying the
    /// detail through makes that legible on screen instead of a guess.
    var detail: String?
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
    /// Set by `close()`, cleared by `startIfNeeded()`. Guards the window where
    /// an in-flight `openPty` would otherwise hand a live PTY to a dead view.
    private var isTornDown = false
    /// Held across `startIfNeeded`'s awaits so a second caller cannot open a
    /// second PTY while the first open is still in flight.
    private var startInFlight = false
    /// The route's initial command is a property of OPENING the terminal, not
    /// of any particular shell. `restart()` funnels back through
    /// `startIfNeeded`, and without this latch every restart re-ran the
    /// command the screen was opened with — the route's `./deploy.sh` again,
    /// unasked, on every tap of 重新启动 shell.
    private var initialCommandSent = false
    /// Cancels the previous copy-toast timer. The clear belongs to the copy,
    /// not to an equality-gated `onChange` in the view: a same-count second
    /// copy never re-fired the observer, so the first copy's still-pending
    /// timer cut the second toast short.
    private var summaryClearTask: Task<Void, Never>?

    init(
        descriptor: TerminalRuntimeDescriptor,
        client: TerminalRuntimeClient
    ) {
        self.descriptor = descriptor
        self.client = client
        self.buffer = TerminalScreenBuffer(maxScrollback: descriptor.maxScrollback)
    }

    func startIfNeeded() async {
        // `startInFlight` is set synchronously, before the first `await`. The
        // other two conditions are not enough: `activeSessionID` is only
        // assigned AFTER four awaits (probe, status, listTasks, openPty), so a
        // second call — a re-fired `.task`, or `restart()` racing the initial
        // start — sails through the guard and opens a second PTY. The runtime
        // allows one, and refuses the second with "only one PTY session is
        // supported by the iSH bridge".
        guard activeSessionID == nil, !isPolling, !startInFlight else { return }
        startInFlight = true
        defer { startInFlight = false }
        isTornDown = false
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
            // The screen may have gone away while `openPty` was in flight —
            // "重新启动 shell" runs in an unstructured Task that no lifecycle
            // event cancels, and `close()` early-returns when `activeSessionID`
            // is still nil, which is exactly this window. Without this the PTY
            // opens into a dead view and never closes; the bridge allows one
            // interactive PTY per managed root, so the NEXT terminal open fails
            // with "only one interactive PTY session is supported".
            if isTornDown {
                try? await client.closePty(config: descriptor.config, sessionId: session.id)
                availability = .closed
                return
            }
            activeSessionID = session.id
            availability = .ready
            startPollingLoop()
            if let initialCommand = descriptor.initialCommand, !initialCommandSent {
                initialCommandSent = true
                appendToHistory(initialCommand)
                Task { [weak self] in
                    await self?.sendInput(initialCommand + "\n")
                }
            }
        } catch {
            // A teardown that raced the THROWING openPty must win, same as it
            // does on the success path above: the view is gone, and `.failed`
            // is a restartable state now — painting it over close()'s
            // `.closed` would advertise a restart link on a dead model.
            if isTornDown {
                availability = .closed
                return
            }
            let runtimeError = TerminalRuntimeError(error)
            availability = .failed(runtimeError.errorDescription ?? String(localized: "terminal_launch_failed"))
            lastError = runtimeError.errorDescription
        }
    }

    func close() async {
        isTornDown = true
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

    /// A bare Return is a real command: it accepts a `[Y/n]` default, pages
    /// `less`, and emits the blank line a `read` is waiting on. Refusing it was
    /// survivable while a greyed-out Send button showed the refusal; now that
    /// Return is the only submit path, an ignored newline is a shell that looks
    /// hung. Only non-empty lines join the history.
    func submitInput() async {
        let command = inputText
        if !command.isEmpty {
            appendToHistory(command)
        }
        inputText = ""
        await sendInput(command + "\n")
    }

    /// Reopen the session after the shell exits. `startIfNeeded` is driven by
    /// `.task`, which does not re-fire on a view that never went away, so
    /// without this a terminal that reached `.closed` stays closed for good.
    /// The transcript is kept — the exit line above the new prompt is the
    /// history, exactly as a desktop terminal that re-runs its shell.
    /// Reachable from `.failed` as well as `.closed`, and that is the point.
    /// An `.error` event and a polling failure both land in `.failed` WITHOUT
    /// clearing `activeSessionID`, so a guard on `activeSessionID == nil` made
    /// this a no-op in exactly the state a user needs it: the caret is dead,
    /// `TerminalRecovery.forState(.failed)` offers no repair, and the only way
    /// out was to pop the screen and lose the transcript. Close whatever handle
    /// is still open, then start again.
    func restart() async {
        guard canRestart else { return }
        // Leave the restartable state before the first await. The link stays
        // visible for the whole `closePty` round-trip otherwise, and a second
        // tap re-entered here: both continuations reached `startIfNeeded`, and
        // the loser's `catch` painted `.failed` over the live shell the winner
        // had just opened.
        availability = .opening
        pollingTask?.cancel()
        pollingTask = nil
        isPolling = false
        if let staleSessionID = activeSessionID {
            try? await client.closePty(config: descriptor.config, sessionId: staleSessionID)
            activeSessionID = nil
        }
        // The screen may have gone away while `closePty` was in flight —
        // `close()` ran, and `startIfNeeded` would clear `isTornDown` at entry
        // and open a fresh PTY into the dismissed view, occupying the
        // runtime's only interactive slot until the app is relaunched.
        if isTornDown {
            availability = .closed
            return
        }
        // A shell that died mid-escape or mid-codepoint leaves the byte-stream
        // state machines dirty, and the new session's first output would be
        // parsed as the old one's tail: U+FFFD from a stale UTF-8 prefix, or a
        // prompt swallowed while a half-open CSI hunts for its final byte.
        decoder = TerminalUTF8Decoder()
        parser = TerminalANSIParser()
        availability = .idle
        lastExitStatus = nil
        lastError = nil
        await startIfNeeded()
    }

    /// The states that have neither a message nor a caret to show for
    /// themselves. A terminal that is still opening has to say so, or it is
    /// indistinguishable from one that is broken.
    var isStarting: Bool {
        switch availability {
        case .idle, .opening: return true
        case .ready, .closed, .failed, .unavailable, .integrityFailure,
             .workspaceUnavailable, .invalidRequest:
            return false
        }
    }

    /// A session that has stopped and could be started again. DERIVED from
    /// the single [`TerminalRecovery`] decision table — never a second
    /// hand-maintained state switch.
    var canRestart: Bool {
        TerminalRecovery.forState(availability) == .restartSession
    }

    /// The line a terminal prints when its shell goes away. `.closed` carries
    /// no message of its own, so without this the screen simply stops
    /// responding with nothing on it to say why.
    var exitNotice: String? {
        guard case .closed = availability else { return nil }
        guard let status = lastExitStatus else {
            return String(localized: "terminal_exit_notice_unknown")
        }
        if status.timedOut {
            return appendingDetail(String(localized: "terminal_exit_notice_timeout"), status.detail)
        }
        guard let code = status.code else {
            // A nil code is a lost pipeline, not a clean exit — the only
            // spontaneous producer in the real stack is the runtime's
            // reader-death path. "code 0" would claim success for it.
            //
            // That path is also the ONE that carries a diagnosis: the reader
            // emits `emit_runtime_error(...)` and then a closed event with the
            // same string as its detail, and the `.exit` overwrites the
            // `.failed` the error just set — so the detail is the only surviving
            // copy of why the pipeline died. Returning the bare "[进程已退出]"
            // here threw it away and left the screen saying nothing at all.
            return appendingDetail(String(localized: "terminal_exit_notice_unknown"), status.detail)
        }
        return appendingDetail(
            String(localized: "terminal_exit_notice_code \(Int(code))"),
            status.detail
        )
    }

    private func appendingDetail(_ notice: String, _ detail: String?) -> String {
        guard let detail = detail?.trimmingCharacters(in: .whitespacesAndNewlines),
              !detail.isEmpty
        else { return notice }
        return "\(notice) \(detail)"
    }

    /// Input is gated on "a write can still reach the shell", not on `.ready`
    /// alone. `.failed` deliberately keeps `activeSessionID` (a polling error
    /// is not proof the process died), and disabling the field there took the
    /// keyboard — and with it the ^C key that rides above it — away in exactly
    /// the state where interrupting the runaway command is the way out.
    var canAcceptInput: Bool {
        if availability.canInteract { return true }
        if case .failed = availability { return activeSessionID != nil }
        return false
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
        summaryClearTask?.cancel()
        summaryClearTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled else { return }
            self?.selectionSummary = nil
        }
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
                lastExitStatus = TerminalExitStatus(
                    code: event.exitCode,
                    timedOut: event.timedOut,
                    detail: event.text
                )
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

    /// Terminal notation, so four keys plus history still fit the keyboard bar
    /// on the narrowest phone. Not localized: `^C` is the same everywhere.
    var compactLabel: String {
        switch self {
        case .interrupt: return "^C"
        case .endOfFile: return "^D"
        case .suspend: return "^Z"
        case .escape: return "esc"
        }
    }

    var byte: UInt8 {
        switch self {
        case .interrupt: return 0x03
        case .endOfFile: return 0x04
        case .suspend: return 0x1A
        case .escape: return 0x1B
        }
    }
}
