// ConversationSource.swift — M10 A2 / P3a.
//
// The seam between the SwiftUI conversation UI (ChatView) and whatever produces
// turns. The app talks ONLY to this protocol; two impls back it:
//
//   - MockConversationSource     — the prior canned behavior (no engine).
//   - EngineConversationSource   — the real in-process engine over UniFFI:
//       builds a `MobileEngineHandle` via `buildIosEngine(...)`, registers a
//       Swift `IosEventListener` whose `onEvent(_:)` maps each inbound
//       `ClientEvent` onto @MainActor-published state, and drives turns via
//       `handle.submit(.sendPrompt(...))`.
//
// This is the in-process analog of the Electron↔bridge real-conversation path:
// one protocol (`client-protocol`), one transport (UniFFI), one renderer
// (SwiftUI). The engine runs ON the device; no network bridge.
//
// SECRETS: the LLM API key is resolved from the iOS Keychain (`Keychain`) with a
// runtime `ANTHROPIC_API_KEY` env override for dev — see
// `EngineConfig.fromEnvironment`. It is NEVER hardcoded, logged, or persisted in
// plaintext here.

import Combine
import Foundation
import OSLog
import SwiftUI

#if canImport(engine_mobileFFI)
    import AuthenticationServices
    import UIKit
    import engine_mobileFFI

    /// Capture only stable native host facts once per engine construction.
    /// Viewport, safe-area, theme, and model/provider choices are intentionally
    /// excluded so the model-facing runtime context remains cacheable.
    @MainActor
    func makeIosHostEnvironment(
        launchMode: IosLaunchModeFfi
    ) -> IosHostEnvironmentFfi {
        let deviceClass: IosDeviceClassFfi = switch UIDevice.current.userInterfaceIdiom {
        case .phone: .phone
        case .pad: .tablet
        default: .unknown
        }
        #if targetEnvironment(simulator)
            let executionTarget = IosExecutionTargetFfi.simulator
        #else
            let executionTarget = IosExecutionTargetFfi.physicalDevice
        #endif
        return IosHostEnvironmentFfi(
            osVersion: UIDevice.current.systemVersion,
            deviceClass: deviceClass,
            executionTarget: executionTarget,
            launchMode: launchMode
        )
    }
#endif

// MARK: - Published conversation state

/// A persistent, dismissible, kind-aware error surfaced in the chat view (PR-4
/// item 4). Unlike the dim `statusLine` (tool activity), this is a banner the
/// user must acknowledge: it carries a coarse `kind` so the UI can color/label
/// it (transport vs. server vs. internal …) and survives until dismissed.
struct ConversationError: Identifiable, Equatable {
    enum Kind: Equatable {
        case transport
        case `protocol`
        case server
        case maxTurns
        case rejected
        case `internal`
        /// A failure originating in the Swift host (e.g. engine build/submit
        /// threw) rather than a lowered engine `ErrorKindDto`.
        case host

        /// A short, user-facing label (Chinese, matching the app's copy).
        var label: String {
            switch self {
            case .transport: return String(localized: "chat_error_transport")
            case .protocol: return String(localized: "chat_error_protocol")
            case .server: return String(localized: "chat_error_server")
            case .maxTurns: return String(localized: "chat_error_max_turns")
            case .rejected: return String(localized: "chat_error_rejected")
            case .internal: return String(localized: "chat_error_internal")
            case .host: return String(localized: "chat_error_host")
            }
        }
    }

    let id = UUID()
    let kind: Kind
    let message: String
}

/// How the last turn ended, surfaced distinctly to the UI (PR-4 item 3). A clean
/// `EndTurn` leaves this `nil`; `MaxTurns` / `Cancelled` set a notice the chat
/// view shows so the two non-clean outcomes aren't silently treated as a normal
/// end.
enum TurnNotice: Equatable {
    case maxTurns
    case cancelled

    var text: String {
        switch self {
        case .maxTurns: return String(localized: "chat_notice_max_turns")
        case .cancelled: return String(localized: "chat_notice_cancelled")
        }
    }
}

/// Identifies one client-owned turn independently of the engine session UUID.
/// The monotonically increasing session epoch prevents a late completion from
/// an abandoned session being mistaken for a turn in the newly-visible session.
struct ConversationTurnToken: Equatable, Hashable, Sendable {
    let clientTurnId: UInt64
    let sessionEpoch: UInt64
}

/// A terminal result published for consumers that need to react to one exact
/// turn (for example Flow voice playback). Only `.completed` is a successful
/// assistant response; all other outcomes must be treated as non-speakable.
struct ConversationTurnCompletion: Equatable, Sendable {
    enum Outcome: Equatable, Sendable {
        case completed
        case maxTurns
        case cancelled
        case failed
    }

    let token: ConversationTurnToken
    let outcome: Outcome
    /// The final assistant text for this turn, trimmed of surrounding whitespace.
    /// Empty when the turn produced no assistant message or did not complete.
    let finalAssistantText: String
}

/// One accepted assistant-text fragment for an exact client-owned turn.
/// Voice playback consumes this instead of observing rendered messages, which
/// keeps streaming speech isolated across cancellations and session switches.
struct ConversationTurnSpeechUpdate: Equatable, Sendable {
    let token: ConversationTurnToken
    let sequence: UInt64
    let delta: String
}

#if canImport(engine_mobileFFI)

    /// One engine-parked permission request the UI must answer (SHIP-BLOCKER #3).
    ///
    /// The engine's adapter gate emits a `PermissionRequest` whenever a tool needs
    /// approval (e.g. a Write/Bash invocation) and parks the turn on a oneshot until
    /// the user answers. On mobile that request used to vanish into a no-op sink, so
    /// the turn hung forever; now the sink forwards it here and the chat view renders
    /// a prompt. The user's choice resolves the park by submitting
    /// `ClientCommand.approvePermission` / `denyPermission` (correlated by
    /// `requestId`) back through the `MobileEngineHandle`.
    ///
    /// `Identifiable` on `requestId` so SwiftUI can key the modal; the head of the
    /// queue is the one rendered.
    struct PendingPermission: Identifiable, Equatable {
        /// Engine correlator echoed back in the resolving command.
        let requestId: UInt64
        /// What the user is approving (drives the prompt's title + detail).
        let kind: PermissionKindDto
        /// Sub-agent identity, when present (always `None` in the foundation).
        let worker: WorkerInfoDto?

        var id: UInt64 { requestId }

        init(request: PermissionRequest) {
            self.requestId = request.requestId
            self.kind = request.kind
            self.worker = request.worker
        }

        static func == (lhs: PendingPermission, rhs: PendingPermission) -> Bool {
            lhs.requestId == rhs.requestId
        }
    }

#endif

/// The observable conversation state ChatView renders. Both sources mutate it on
/// the main actor: the mock with canned timers, the engine from listener events.
struct SessionRestoreRecovery: Equatable, Identifiable {
    let id = UUID()
    let unavailableSessionID: String
}

struct SessionTransitionFailure: Equatable, Identifiable {
    let id = UUID()
    let requestedSessionID: String
}

/// A compact, session-scoped projection of one orchestrator agent.  The main
/// agent is represented by the stable `ConversationModel.mainAgentID` id so
/// the message list can use the same selection path for the root turn and
/// child-agent transcripts.
struct ConversationAgentSummary: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let agentType: String
    var status: String
    var latestActivity: String?
    var updatedAtMs: UInt64

    init(
        id: String,
        name: String,
        agentType: String,
        status: String,
        latestActivity: String? = nil,
        updatedAtMs: UInt64 = 0
    ) {
        self.id = id
        self.name = name
        self.agentType = agentType
        self.status = status
        self.latestActivity = latestActivity
        self.updatedAtMs = updatedAtMs
    }

    static let main = ConversationAgentSummary(
        id: "main",
        name: String(localized: "chat_agent_main"),
        agentType: "main",
        status: "idle"
    )
}

/// Render-ready transcript cached per child agent.  Child messages use the
/// same renderer as the main transcript; keeping the three projections
/// together avoids rebuilding structured tool details every time the user
/// switches the selector.
struct ConversationAgentTranscript: Equatable {
    var messages: [Message] = []
    var items: [ConversationRenderItem] = []
    var details: [UUID: ConversationMessageDetail] = [:]
    /// One canonical wire signature per inbound MessageDto. This lets a live
    /// tail event merge with a full transcript reply occurrence-by-occurrence,
    /// including repeated messages with identical text.
    var wireSignatures: [String] = []
    /// Stable visible-message indices supplied by the agent transcript stream.
    /// Legacy snapshots/events leave entries nil and use occurrence signatures.
    var wireIndices: [UInt64?] = []
    /// Exclusive watermark of the full transcript snapshot.  A live message
    /// whose index is at or above this value belongs to the pending tail.
    var nextMessageIndex: UInt64 = 0
    /// Monotonic raw transcript-record revision from the engine. This fences
    /// delayed full replies even when visible-message counts are unchanged.
    var revision: UInt64 = 0
    var loaded = false
}

#if canImport(engine_mobileFFI)
    /// A live tail received while a full transcript request is in flight.
    /// The request reply is a snapshot, so these entries must be replayed after
    /// the snapshot rather than merged by text/signature alone.
    struct ConversationPendingAgentMessage: Equatable {
        let index: UInt64?
        let message: MessageDto
    }
#endif

@MainActor
final class ConversationModel: ObservableObject {
    static let mainAgentID = "main"

    /// The full visible transcript (user + assistant turns).
    @Published var messages: [Message]
    /// The chat surface's ordered render list: plain messages plus per-turn
    /// execution traces / shell cards. `messages` remains the compatibility
    /// transcript used by voice/setup surfaces.
    @Published var items: [ConversationRenderItem]

    /// Session-scoped agent roster.  The first row is always the root agent;
    /// child rows arrive from `SessionAgentList`/`SessionAgentUpdated` or the
    /// coordinator worker fallback on older engines.
    @Published private(set) var agentSummaries: [ConversationAgentSummary]
    /// The currently visible agent.  Selecting a child makes its transcript
    /// read-only; selecting `main` restores the ordinary composer transcript.
    @Published var selectedAgentID: String
    /// Child-agent transcript cache keyed by stable agent id.  Main-agent data
    /// intentionally remains in `messages/items` for backwards compatibility.
    @Published private(set) var agentTranscripts: [String: ConversationAgentTranscript]
    @Published var isAgentTranscriptLoading = false
    @Published var agentTranscriptError: String?
    /// Identifies the one transcript request allowed to mutate loading/error
    /// state. A late task from a previous agent or session is ignored.
    private var agentTranscriptRequestKey: String?
    private var agentTranscriptGeneration: UInt64 = 0
    #if canImport(engine_mobileFFI)
        private var pendingAgentMessages: [String: [ConversationPendingAgentMessage]] = [:]
    #endif

    /// The list renderer consumes these projections so it does not need to
    /// know whether the selected row is the root or a child agent.
    var selectedAgentMessages: [Message] {
        guard selectedAgentID != Self.mainAgentID else { return messages }
        return agentTranscripts[selectedAgentID]?.messages ?? []
    }
    var selectedAgentItems: [ConversationRenderItem] {
        guard selectedAgentID != Self.mainAgentID else { return items }
        return agentTranscripts[selectedAgentID]?.items ?? []
    }
    var selectedAgentMessageDetails: [UUID: ConversationMessageDetail] {
        guard selectedAgentID != Self.mainAgentID else { return messageDetails }
        return agentTranscripts[selectedAgentID]?.details ?? [:]
    }
    var isSelectedAgentReadOnly: Bool { selectedAgentID != Self.mainAgentID }
    var selectedAgentSummary: ConversationAgentSummary? {
        agentSummaries.first { $0.id == selectedAgentID }
    }
    /// Ordered activity projection for the currently selected session/agent.
    /// Run cards never enter this projection; reasoning, tools, notices and
    /// narrative rows retain their wire order and stable identities.
    var visibleTimelineGroups: [ConversationTimelineGroup] {
        ConversationRenderLayout.timelineGroups(selectedAgentItems)
    }
    var visibleTimelineRows: [ConversationTimelineRow] {
        ConversationRenderLayout.timelineRows(selectedAgentItems)
    }
    /// Structured block payload for assistant bubbles keyed by `Message.id`.
    @Published var messageDetails: [UUID: ConversationMessageDetail] = [:]
    /// True while a turn is in flight (drives the streaming dots row + gates
    /// overlapping sends and the Send→Stop swap, PR-4 items 1 & 2).
    @Published var streaming: Bool = false
    /// True after Stop is requested and until the engine confirms that the
    /// matching turn released its owner slot. The composer remains editable but
    /// cannot submit another turn during this interval.
    @Published var isCancelling: Bool = false
    /// The latest terminal turn result. Consumers must correlate its token with
    /// the token returned by `ConversationSource.send`; observing the transcript's
    /// last AI message is insufficient because session switches and late events
    /// can otherwise replay stale content.
    @Published var turnCompletion: ConversationTurnCompletion? = nil
    /// Lossless token-scoped assistant-delta stream. A subject is used instead
    /// of an `@Published` latest-value slot because SwiftUI may coalesce several
    /// assignments in one render transaction and silently drop middle deltas.
    let turnSpeechUpdates = PassthroughSubject<ConversationTurnSpeechUpdate, Never>()
    /// True when the session is brand-new and empty (drives the empty state).
    @Published var isNew: Bool = false
    /// The currently selected model chip.
    @Published var model: ModelOption
    // ── Out-of-band model state (SHIP-BLOCKER #2) ──────────────────────────────
    // `ListModels` / `ModelChanged` are NOT part of a text turn, so they ride a
    // SEPARATE model-state path here (the @Published analog of Android's model
    // StateFlow) updated by the listener — never the per-turn delta flow. The
    // picker is driven by `availableModels` (real engine ids); `activeModelId` is
    // whatever the engine reports. Empty until the first `ModelList` lands, in
    // which case the UI falls back to the mock catalog (engine unavailable).
    /// The real model ids the engine accepts (`ModelList.models`). Empty ⇒ mock.
    @Published var availableModels: [String] = []
    /// The active model id the engine reports (`ModelList.current` / `ModelChanged.model`).
    @Published var activeModelId: String = ""
    // ── Out-of-band session state (real history) ───────────────────────────────
    // `ListSessions` / `SessionList` are NOT part of a text turn, so — exactly
    // like `ModelList` above — they ride a SEPARATE session-state path here,
    // updated by the listener when a `SessionList` event lands. The drawer
    // renders these REAL rows (engine-enumerated `~/.claude` JSONL sessions) in
    // place of the mock catalog; empty until the first `SessionList` arrives, in
    // which case the drawer falls back to MockData (engine unavailable / no
    // history). `activeSessionId` is the engine session currently driving the
    // connection (set by `SessionStarted` / `SessionResumed`), used to mark the
    // selected row.
    /// Real resumable sessions from the engine (`SessionList.sessions` lowered).
    /// Empty ⇒ the drawer falls back to the mock session lists.
    @Published var engineSessions: [EngineSession] = []
    /// Distinguishes an authoritative empty `SessionList` from the initial
    /// not-yet-loaded state. Project persistence must never treat the latter as
    /// a command to erase its cached session index.
    @Published var engineSessionsLoaded: Bool = false
    /// The engine session id currently driving the connection — set by
    /// `SessionStarted` / `SessionResumed`. Empty until the engine reports one.
    @Published var activeSessionId: String = ""
    /// A NewSession / ResumeSession command has been issued but has not yet been
    /// confirmed by SessionStarted / SessionResumed. While true, an older
    /// SessionList must not replace the persisted project index.
    @Published var sessionTransitionPending: Bool = false
    /// Emitted only when ResumeSession proves that a persisted session no longer
    /// exists. RootView consumes this before adopting the replacement NewSession,
    /// clearing the stale project/UserDefaults selection without hiding other
    /// resume failures.
    @Published var sessionRestoreRecovery: SessionRestoreRecovery? = nil
    /// Emitted when a requested ResumeSession fails without a recoverable
    /// missing-session migration. RootView uses it to roll optimistic drawer
    /// selection back to the last engine-confirmed session.
    @Published var sessionTransitionFailure: SessionTransitionFailure? = nil
    /// Monotonic signal emitted after a new session is confirmed or a real turn
    /// settles. The composition root responds with ListSessions, allowing the
    /// provisional SessionStarted row to be replaced by the durable JSONL catalog.
    @Published var sessionRefreshRevision: UInt64 = 0
    /// Real MCP servers from the engine (`McpServers` listing, lowered to the UI
    /// `MCPServer` model). `mcpServersLoaded` distinguishes an authoritative
    /// empty listing from the initial not-yet-loaded state.
    @Published var mcpServers: [MCPServer] = []
    @Published var mcpServersLoaded = false
    /// Real skills/slash commands from the engine registry. The settings page
    /// never invents rows when this catalog is empty.
    @Published var skills: [Skill] = []
    @Published var skillsLoaded = false
    /// Full command-palette projection from the live engine registry. This is
    /// separate from `skills` because input matching also needs aliases,
    /// argument hints, menu descriptions, and hidden state.
    @Published var slashCommands: [ConversationSlashCommand] = []
    @Published var slashCommandsLoaded = false
    /// A `RunSlashCommand` has been submitted but has not yet resolved into a
    /// local result or a normal streaming turn.
    @Published var slashCommandPending = false
    /// A transient, dim status line (tool activity / connection state). NOT used
    /// for errors anymore — those go to `error` (the persistent banner).
    @Published var statusLine: String? = nil
    /// A persistent, dismissible, kind-aware error banner (PR-4 item 4).
    @Published var error: ConversationError? = nil
    /// A non-clean turn outcome (MaxTurns / Cancelled) surfaced distinctly from a
    /// normal end (PR-4 item 3). Cleared when a new turn starts.
    @Published var notice: TurnNotice? = nil
    /// Pending interactive `AskUserQuestion` questionnaires, oldest first. The
    /// chat surface renders each as a card appended after the messages;
    /// answered/cancelled requests are dropped when the engine confirms with
    /// `askUserQuestionResolved`, and the queue is cleared on session end.
    @Published var pendingQuestions: [ConversationPendingQuestion] = []
    /// Background tasks announced by this scope's engine (Workflow builds,
    /// background jobs), oldest first. Engine-scoped: switching sessions
    /// within the scope keeps the panel; a scope switch builds a fresh
    /// source and starts empty. Drives the pinned tasks panel above the
    /// composer.
    @Published var backgroundTasks: [BackgroundTaskSnapshot] = []
    /// The model's own working plan (TodoWrite / Task checklist), replaced
    /// WHOLESALE on every `PlanUpdated` — the engine emits the complete ordered
    /// list and an empty list clears it. Drives `PlanTasksPanel`, pinned closest
    /// to the composer. Distinct from `backgroundTasks`, which are engine jobs.
    @Published var planTasks: [ConversationPlanTask] = []
    /// Stable keys whose tool body/diff the user expanded. Standalone rows use
    /// their tool-use id; structured message blocks use message id + tool id.
    ///
    /// This lives HERE and not in the row: every transcript list recycles its
    /// rows, so row-local `@State` is dropped on scroll and then reappears on
    /// whichever row happens to reuse the storage.
    @Published var expandedToolCalls: Set<String> = []
    #if canImport(engine_mobileFFI)
        /// FIFO queue of engine-parked permission requests (SHIP-BLOCKER #3). The
        /// chat view renders the head (`first`) as a modal prompt; answering it pops
        /// the head and reveals the next. Empty between requests / on the mock
        /// (which never asks for permission).
        @Published var pendingPermissions: [PendingPermission] = []
    #endif

    init(messages: [Message] = [],
         model: ModelOption = MockData.models[0]) {
        self.messages = messages
        self.items = messages.map(ConversationRenderItem.message)
        self.model = model
        self.agentSummaries = [.main]
        self.selectedAgentID = Self.mainAgentID
        self.agentTranscripts = [:]
    }

    /// Replace the agent roster while retaining the selected row when it is
    /// still present.  A missing selection safely falls back to the main
    /// agent, which is also what session transitions use.
    func replaceAgentSummaries(_ summaries: [ConversationAgentSummary]) {
        var merged: [ConversationAgentSummary] = [
            agentSummaries.first(where: { $0.id == Self.mainAgentID }) ?? .main
        ]
        for summary in summaries.map(Self.normalizedAgentSummaryForSource) {
            if let index = merged.firstIndex(where: { $0.id == summary.id }) {
                merged[index] = summary
            } else {
                merged.append(summary)
            }
        }
        agentSummaries = merged
        if !merged.contains(where: { $0.id == selectedAgentID }) {
            selectedAgentID = Self.mainAgentID
            isAgentTranscriptLoading = false
            agentTranscriptError = nil
            agentTranscriptRequestKey = nil
        }
    }

    static func normalizedAgentSummaryForSource(_ summary: ConversationAgentSummary) -> ConversationAgentSummary {
        guard summary.id == Self.mainAgentID else { return summary }
        return ConversationAgentSummary(
            id: Self.mainAgentID,
            name: ConversationAgentSummary.main.name,
            agentType: summary.agentType,
            status: summary.status,
            latestActivity: summary.latestActivity,
            updatedAtMs: summary.updatedAtMs
        )
    }

    func upsertAgentSummary(_ summary: ConversationAgentSummary) {
        var next = agentSummaries
        if let index = next.firstIndex(where: { $0.id == summary.id }) {
            next[index] = summary
        } else {
            next.append(summary)
        }
        replaceAgentSummaries(next)
    }

    func updateMainAgent(status: String, latestActivity: String? = nil) {
        let current = agentSummaries.first(where: { $0.id == Self.mainAgentID }) ?? .main
        upsertAgentSummary(ConversationAgentSummary(
            id: Self.mainAgentID,
            name: current.name,
            agentType: current.agentType,
            status: status,
            latestActivity: latestActivity,
            updatedAtMs: UInt64(Date().timeIntervalSince1970 * 1_000)
        ))
    }

    func clearAgentState() {
        agentSummaries = [.main]
        selectedAgentID = Self.mainAgentID
        agentTranscripts = [:]
        isAgentTranscriptLoading = false
        agentTranscriptError = nil
        agentTranscriptRequestKey = nil
        #if canImport(engine_mobileFFI)
            pendingAgentMessages = [:]
        #endif
    }

    private func agentRequestKey(sessionID: String, agentID: String) -> String {
        "\(sessionID)\u{0}\(agentID)"
    }

    private func agentRequestKeyWithGeneration(sessionID: String, agentID: String) -> String {
        "\(agentRequestKey(sessionID: sessionID, agentID: agentID))\u{0}\(agentTranscriptGeneration)"
    }

    func setAgentTranscript(
        _ id: String,
        transcript: ConversationAgentTranscript,
        sessionID: String? = nil
    ) {
        guard id != Self.mainAgentID else { return }
        let sessionID = sessionID ?? activeSessionId
        guard sessionID == activeSessionId else { return }
        agentTranscripts[id] = transcript
        if selectedAgentID == id,
           transcript.loaded,
           agentTranscriptRequestKey?.hasPrefix(agentRequestKey(sessionID: sessionID, agentID: id) + "\u{0}") == true {
            isAgentTranscriptLoading = false
            agentTranscriptError = nil
            agentTranscriptRequestKey = nil
        }
    }

    @discardableResult
    func markAgentTranscriptLoading(_ id: String, sessionID: String? = nil) -> String? {
        selectedAgentID = id
        guard id != Self.mainAgentID else {
            isAgentTranscriptLoading = false
            agentTranscriptRequestKey = nil
            return nil
        }
        let sessionID = sessionID ?? activeSessionId
        agentTranscriptGeneration &+= 1
        let requestKey = agentRequestKeyWithGeneration(sessionID: sessionID, agentID: id)
        agentTranscriptRequestKey = requestKey
        let alreadyLoaded = agentTranscripts[id]?.loaded ?? false
        isAgentTranscriptLoading = !alreadyLoaded
        agentTranscriptError = nil
        #if canImport(engine_mobileFFI)
            if !alreadyLoaded {
                let baseKey = agentRequestKey(sessionID: sessionID, agentID: id)
                if pendingAgentMessages[baseKey] == nil {
                    pendingAgentMessages[baseKey] = []
                }
            }
        #endif
        return requestKey
    }

    func failAgentTranscript(
        _ id: String,
        sessionID: String,
        message: String,
        requestKey: String? = nil
    ) {
        guard sessionID == activeSessionId,
              selectedAgentID == id,
              agentTranscriptRequestKey == (requestKey ?? agentTranscriptRequestKey),
              requestKey == nil || requestKey == agentTranscriptRequestKey
        else { return }
        isAgentTranscriptLoading = false
        agentTranscriptError = message
    }

    #if canImport(engine_mobileFFI)
        func shouldAcceptAgentMessage(
            agentID: String,
            sessionID: String,
            index: UInt64?
        ) -> Bool {
            guard sessionID == activeSessionId else { return false }
            guard let index else { return true }
            if let transcript = agentTranscripts[agentID] {
                if transcript.wireIndices.contains(where: { $0 == index }) { return false }
                if transcript.loaded, index < transcript.nextMessageIndex { return false }
            }
            if pendingAgentMessages[agentRequestKey(sessionID: sessionID, agentID: agentID)]?
                .contains(where: { $0.index == index }) == true {
                return false
            }
            return true
        }

        func enqueuePendingAgentMessage(
            _ message: MessageDto,
            index: UInt64?,
            agentID: String,
            sessionID: String
        ) -> Bool {
            guard sessionID == activeSessionId,
                  !(agentTranscripts[agentID]?.loaded ?? false)
            else { return false }
            pendingAgentMessages[agentRequestKey(sessionID: sessionID, agentID: agentID), default: []]
                .append(ConversationPendingAgentMessage(index: index, message: message))
            return true
        }

        func takePendingAgentMessages(agentID: String, sessionID: String) -> [ConversationPendingAgentMessage] {
            pendingAgentMessages.removeValue(forKey: agentRequestKey(sessionID: sessionID, agentID: agentID)) ?? []
        }
    #endif

    /// Whether user-started work still needs iOS's finite background assertion.
    /// A turn can hand work to an engine task before its text stream settles, so
    /// `streaming` alone is not a sufficient lifetime signal.
    var requiresBackgroundExecution: Bool {
        Self.requiresBackgroundExecution(
            streaming: streaming,
            backgroundTasks: backgroundTasks,
            items: items
        )
    }

    var backgroundExecutionActivity: AnyPublisher<Bool, Never> {
        Publishers.CombineLatest3($streaming, $backgroundTasks, $items)
            .map(Self.requiresBackgroundExecution)
            .removeDuplicates()
            .eraseToAnyPublisher()
    }

    private static func requiresBackgroundExecution(
        streaming: Bool,
        backgroundTasks: [BackgroundTaskSnapshot],
        items: [ConversationRenderItem]
    ) -> Bool {
        if streaming || backgroundTasks.contains(where: { !$0.status.isTerminal }) {
            return true
        }
        return items.contains { item in
            guard case let .run(run) = item else { return false }
            return run.status == .running
                || run.activeWorkers > 0
                || run.tools.contains(where: { $0.status == .running })
                || run.shellCards.contains(where: { $0.status == .running })
        }
    }
}

// MARK: - ConversationSource seam

/// What ChatView drives. A source owns the turn lifecycle and pushes results
/// into the shared `ConversationModel` (on the main actor).
@MainActor
protocol ConversationSource: AnyObject {
    /// The state ChatView observes.
    var model: ConversationModel { get }
    /// Begin a fresh, empty conversation (the "edit / new chat" affordance).
    func startNewConversation()
    /// Submit a user prompt. Appends the user message, flips `streaming`, and
    /// produces the assistant reply (canned for the mock, streamed for the engine).
    /// MUST be a no-op while a turn is in flight (`model.streaming`) so rapid taps
    /// can't start an overlapping turn (PR-4 item 1).
    @discardableResult
    func send(_ text: String) -> ConversationTurnToken?
    /// Cancel the in-flight turn (PR-4 item 2): the engine submits `.cancel(...)`
    /// and keeps ownership until the engine confirms safe completion. A no-op
    /// when nothing is streaming.
    func cancel()
    /// Cancel the in-flight turn and return only after the engine has released
    /// its single-turn slot. Engine/configuration swaps use this barrier so an
    /// old Block-behavior tool cannot overlap a replacement engine.
    func cancelAndWait() async throws
    /// Dismiss the persistent error banner (PR-4 item 4).
    func dismissError()
    /// Switch the active model (SHIP-BLOCKER #2). The engine source submits
    /// `ClientCommand.setModel(id)` with a REAL model id and reflects the
    /// confirming `ModelChanged`; the mock just swaps the chip. `id` is a real
    /// engine model id when `model.availableModels` is populated.
    func setModel(_ id: String)
    /// Switch the active conversation to `session` (the iOS analog of Android's
    /// `ChatViewModel.openSession`). MUST cancel any in-flight turn and reset the
    /// streaming bookkeeping FIRST so a turn that completes after the switch can't
    /// land its deltas / notice in the newly-shown session. A no-op when the id is
    /// already active.
    func openSession(_ session: SessionRef)
    /// Lifecycle: the app moved to the background (`scenePhase == .background`).
    /// This hook may persist state, but MUST NOT cancel an active turn or change
    /// its streaming bookkeeping. UIKit owns suspension; backgrounding is not a
    /// user Stop action.
    func handleBackground()
    /// Lifecycle: the app returned to the foreground (`scenePhase == .active`). A
    /// hook to restore/refresh state; the default is a no-op.
    func handleForeground()
    /// Optionally build the engine eagerly so the real model catalog
    /// (`ModelList`) populates before the first send (SHIP-BLOCKER #2). A no-op on
    /// the mock; idempotent on the engine.
    func warmUp()
    /// Build the backing engine and surface construction failures to callers.
    /// Project switching uses this before committing the new workspace so a
    /// failed engine rebuild can roll back atomically.
    func prepare() async throws
    /// Ask the engine for its real resumable-session catalog (submit
    /// `ClientCommand.listSessions`). The reply (`SessionList`) lands out-of-band
    /// on the listener and populates `model.engineSessions`. A no-op on the mock
    /// (which keeps the canned drawer lists). Called when the drawer opens so the
    /// history is fresh.
    func listSessions()
    /// Resume a prior session by its engine UUID (submit
    /// `ClientCommand.resumeSession`). The engine confirms with `SessionResumed`;
    /// the source resets the transcript so the resumed session's turns land
    /// clean. The UI also selects the row locally so the choice reflects
    /// immediately even if engine-side resume is still a follow-up. A no-op on the
    /// mock.
    /// `emptySessionTitle` is supplied only when the persisted project index
    /// proves this is a zero-message legacy session. The engine can then restore
    /// the empty anchor without assigning a different UUID.
    func resumeSession(_ uuid: String, emptySessionTitle: String?)
    /// Ask the engine for its real MCP server listing (submit
    /// `RefreshListings(.mcp)`). The reply (`McpServers`) lands out-of-band and
    /// populates `model.mcpServers`. A no-op on the mock (keeps the canned list).
    func refreshMcpServers()
    /// Refresh the current session's agent roster.  The command is out of band
    /// from the active turn and is safe to call whenever the agent picker opens.
    func listSessionAgents()
    /// Select one agent for the message list.  Child agents are read-only on
    /// iOS; selecting the main agent restores the composer.
    func selectAgent(_ id: String)
    /// Load a child agent's durable transcript into the source cache.
    func loadSessionAgentTranscript(_ id: String)
    #if canImport(engine_mobileFFI)
        /// Resolve a parked permission request (SHIP-BLOCKER #3): submit
        /// `ApprovePermission{requestId, response}` and pop the head of the queue.
        func approvePermission(_ requestId: UInt64, _ response: PermissionResponseDto)
        /// Resolve a parked permission request by denying it: submit
        /// `DenyPermission{requestId}` and pop the head of the queue.
        func denyPermission(_ requestId: UInt64)
        /// Submit a non-turn command such as secure Provider credential CRUD.
        func submitEngineCommand(_ command: ClientCommand) async throws
        /// Exercise the engine's token-free Provider model-list probe.
        func testProviderConnection(
            profile: ProviderLaunchProfile,
            credentialOverride: String?
        ) async throws -> ProviderConnectionTestResult
        /// Run the native browser OAuth flow. The engine owns PKCE/state and
        /// only receives the callback URL from this coordinator.
        func loginOAuth(provider: String) async throws -> ProviderOAuthState
        func logoutOAuth(provider: String) async throws
        func authState(provider: String) async throws -> ProviderOAuthState
        func testOAuthConnection(
            provider: String,
            profile: ProviderLaunchProfile
        ) async throws -> ProviderConnectionTestResult
        /// Route an app-level `lingxi://oauth/callback` into the pending web
        /// authentication session when iOS delivers it through `onOpenURL`.
        func handleOAuthCallback(_ url: URL)
        /// Observe out-of-band engine events without duplicating the listener.
        func setExternalEventHandler(_ handler: ((ClientEvent) -> Void)?)
    #endif
}

/// Default `warmUp` for sources with nothing to pre-build (the mock). The engine
/// source overrides it to eagerly build the handle + list models. Lifecycle
/// hooks are no-ops by default; the engine source may override foregrounding to
/// refresh state, but backgrounding never implies cancellation.
extension ConversationSource {
    func warmUp() {}
    func prepare() async throws {}
    func cancelAndWait() async throws { cancel() }
    func handleBackground() {}
    func handleForeground() {}
    /// Default session ops for sources with no engine catalog (the mock): no-ops,
    /// so the mock keeps its canned drawer lists and ignores resume requests.
    func listSessions() {}
    func resumeSession(_ uuid: String) {
        resumeSession(uuid, emptySessionTitle: nil)
    }
    func resumeSession(_ uuid: String, emptySessionTitle: String?) {}
    func refreshMcpServers() {}
    func listSessionAgents() {}
    func selectAgent(_ id: String) {
        guard model.agentSummaries.contains(where: { $0.id == id }) else { return }
        model.markAgentTranscriptLoading(id)
    }
    func loadSessionAgentTranscript(_ id: String) {}
}

#if canImport(engine_mobileFFI)
    /// Default permission handling for sources that never park a turn on a
    /// permission gate (the mock). The engine source overrides both.
    extension ConversationSource {
        func approvePermission(_ requestId: UInt64, _ response: PermissionResponseDto) {}
        func denyPermission(_ requestId: UInt64) {}
        func submitEngineCommand(_ command: ClientCommand) async throws {}
        func testProviderConnection(
            profile: ProviderLaunchProfile,
            credentialOverride: String?
        ) async throws -> ProviderConnectionTestResult {
            .failure(message: String(localized: "chat_provider_engine_unavailable"))
        }
        func loginOAuth(provider: String) async throws -> ProviderOAuthState {
            throw NSError(domain: "ConversationSource", code: 1, userInfo: [NSLocalizedDescriptionKey: "OAuth requires the engine"])
        }
        func logoutOAuth(provider: String) async throws {}
        func authState(provider: String) async throws -> ProviderOAuthState {
            ProviderOAuthState(provider: provider, signedIn: false, accountLabel: nil, accountID: nil, organizationID: nil, fedramp: false)
        }
        func testOAuthConnection(
            provider: String,
            profile: ProviderLaunchProfile
        ) async throws -> ProviderConnectionTestResult {
            .failure(message: String(localized: "chat_provider_engine_unavailable"))
        }
        func handleOAuthCallback(_ url: URL) {}
        func setExternalEventHandler(_ handler: ((ClientEvent) -> Void)?) {}
    }
#endif

// MARK: - Source selection

/// Chooses the conversation source at app start. Prefers the real in-process
/// engine (over UniFFI) when the bindings are linked AND the engine is opted in;
/// otherwise the canned mock. Falling back to the mock keeps the app usable in
/// preview / no-key environments.
///
/// With the FFI linked, shipped builds always use the real in-process engine —
/// even keyless — so missing provider credentials surface as real engine errors
/// rather than silently falling back to canned data. The mock remains only for
/// preview / no-FFI environments.
@MainActor
enum ConversationSourceFactory {
    struct LaunchOptions {
        var projectCwd: String? = nil
        var providerConfigured: Bool = false
        var providerProfilesJson: String? = nil
        var providerRoutingJson: String? = nil
        var defaultModelID: String? = nil
        var mobileLinux: TerminalRuntimeConfig? = nil
    }

    static func make(
        projectCwd: String? = nil,
        providerConfigured: Bool = false,
        providerProfilesJson: String? = nil,
        providerRoutingJson: String? = nil,
        defaultModelID: String? = nil,
        mobileLinux: TerminalRuntimeConfig? = nil
    ) -> any ConversationSource {
        make(options: LaunchOptions(
            projectCwd: projectCwd,
            providerConfigured: providerConfigured,
            providerProfilesJson: providerProfilesJson,
            providerRoutingJson: providerRoutingJson,
            defaultModelID: defaultModelID,
            mobileLinux: mobileLinux))
    }

    static func make(options: LaunchOptions = .init()) -> any ConversationSource {
        #if DEBUG
            if ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] == "1" {
                return MockConversationSource.uiTestFixture(
                    cancelledRun: ProcessInfo.processInfo.environment["LINGXI_UI_TEST_CANCELLED_RUN"] == "1",
                    multiAgent: ProcessInfo.processInfo.environment["LINGXI_UI_TEST_MULTI_AGENT"] == "1"
                )
            }
        #endif
        #if canImport(engine_mobileFFI)
            let env = ProcessInfo.processInfo.environment
            let isPreview = env["XCODE_RUNNING_FOR_PREVIEWS"] == "1"
            if !isPreview {
                let root = appSandboxRoot()
                // SHIP-BLOCKER #2: NEVER seed the engine with a branded mock id
                // ("lx-72b" → Anthropic 400). Use the user's last-picked real model
                // from the Keychain when set; otherwise pass "" so `buildIosEngine`
                // falls back to `MobileConfig.default_model` (a real Anthropic wire
                // id). `fromEnvironment` still lets `LINGXI_MODEL` override for dev.
                let storedModel = options.defaultModelID ?? Keychain.get(.model) ?? ""
                let config = EngineConfig.fromEnvironment(
                    appSandboxRoot: root,
                    model: storedModel,
                    projectCwd: options.projectCwd,
                    providerProfilesJson: options.providerProfilesJson,
                    providerRoutingJson: options.providerRoutingJson,
                    mobileLinux: options.mobileLinux)
                return EngineConversationSource(config: config)
            }
        #endif
        return MockConversationSource()
    }

    /// The app's writable container root the engine roots its filesystem +
    /// `~/.claude`-equivalent under. Uses Application Support (created on demand).
    ///
    /// `nonisolated` because it reads `FileManager` and nothing else: the
    /// enclosing enum is `@MainActor` for the source-construction members, and
    /// inheriting that here would force every caller onto the main actor for a
    /// pure path computation. The mobile-linux FFI bridges — Settings, the
    /// terminal, cron — must reach this from synchronous nonisolated code,
    /// because it is the single authority for the root the ios-ish runtime
    /// validates local-app mounts against, and a second copy for their benefit
    /// is exactly the divergence that broke every local-app build.
    nonisolated static func appSandboxRoot() -> String {
        let fm = FileManager.default
        let base = (try? fm.url(for: .applicationSupportDirectory,
                                in: .userDomainMask,
                                appropriateFor: nil,
                                create: true))
            ?? fm.temporaryDirectory
        let root = base.appendingPathComponent("LingxiCode", isDirectory: true)
        try? fm.createDirectory(at: root, withIntermediateDirectories: true)
        return root.path
    }
}

// MARK: - Mock source (prior behavior)

/// The pre-P3a canned behavior, lifted behind the seam verbatim: append the
/// user message, show streaming dots, then append a fixed assistant reply.
@MainActor
final class MockConversationSource: ConversationSource {
    let model = ConversationModel(messages: MockData.messagesDefault)

    /// Bumped on cancel / new-chat so an in-flight canned reply timer no-ops when
    /// it fires (the mock's analog of the engine's cancel token).
    private var turnToken = 0
    private var nextTurnId: UInt64 = 1
    private var sessionEpoch: UInt64 = 1
    private var activeTurnToken: ConversationTurnToken?
    private var turnSpeechSequence: UInt64 = 0

    #if DEBUG
        static func uiTestFixture(cancelledRun: Bool = false, multiAgent: Bool = false) -> MockConversationSource {
            let source = MockConversationSource()
            let terminalToolStatus: ConversationToolStatus = cancelledRun ? .cancelled : .completed
            let terminalShellStatus: ConversationShellStatus = cancelledRun ? .cancelled : .completed
            let terminalRunStatus: ConversationExecutionStatus = cancelledRun ? .cancelled : .completed
            let shell = ConversationShellCard(
                sessionId: "ui-session",
                turnId: 1,
                taskId: "ui-shell",
                command: "pwd",
                cwd: LXISHGuestPaths.workspace("ui-test"),
                stdout: LXISHGuestPaths.workspace("ui-test") + "\n",
                stderr: "",
                exitCode: 0,
                durationMs: 42,
                status: terminalShellStatus,
                truncated: false
            )
            let shellTrace = ConversationToolTrace(
                id: "ui-shell",
                tool: "shell",
                status: terminalToolStatus,
                inputSummary: "pwd",
                outputSummary: cancelledRun ? "Shell 已取消" : "Shell 完成",
                elapsedMs: 42
            )
            let webSearchTrace = ConversationToolTrace(
                id: "ui-web-search",
                tool: "WebSearch",
                status: .cancelled,
                inputSummary: "Wuhan weather today",
                outputSummary: nil,
                elapsedMs: 1_234
            )
            let readTrace = ConversationToolTrace(
                id: "ui-read",
                tool: "Read",
                status: .completed,
                inputSummary: "/workspace/ui-test/README.md",
                outputSummary: "Read completed",
                elapsedMs: 18
            )
            let searchTrace = ConversationToolTrace(
                id: "ui-search",
                tool: "Grep",
                status: .completed,
                inputSummary: "ConversationTimelineView",
                outputSummary: "Search completed",
                elapsedMs: 29
            )
            let toolTraces = cancelledRun ? [webSearchTrace, shellTrace] : [shellTrace, readTrace, searchTrace]
            let run = ConversationExecutionRun(
                id: "ui-run",
                sessionId: "ui-session",
                turnId: 1,
                status: terminalRunStatus,
                reasoning: "检查当前项目工作区。",
                tools: toolTraces,
                shellCards: [shell],
                usage: ConversationUsageSnapshot(
                    inputTokens: 12,
                    outputTokens: 8,
                    cacheReadTokens: 0,
                    cacheCreationTokens: 0
                ),
                activities: cancelledRun
                    ? [
                        .reasoning(id: "ui-thought", text: "检查当前项目工作区。"),
                        .tool(id: webSearchTrace.id),
                        .tool(id: shellTrace.id),
                    ]
                    : [
                        .reasoning(id: "ui-thought", text: "检查当前项目工作区。"),
                        .tool(id: shellTrace.id),
                        .textBoundary(id: "ui-text-boundary"),
                        .tool(id: readTrace.id),
                        .tool(id: searchTrace.id),
                    ]
            )
            let user = Message(role: .user, text: "Hello")
            let assistant = Message(
                role: .ai,
                text: "Hello! I'm ready to help with your software engineering tasks."
            )
            source.model.messages = [user, assistant]
            source.model.items = [.message(user), .run(run), .message(assistant)]
            source.model.messageDetails = [:]
            source.model.isNew = false
            source.model.activeSessionId = "ui-session"
            if multiAgent {
                let childMessage = Message(role: .ai, text: "Child agent completed the requested check.")
                source.model.replaceAgentSummaries([
                    .main,
                    ConversationAgentSummary(
                        id: "ui-child",
                        name: "UI Child",
                        agentType: "worker",
                        status: "completed",
                        latestActivity: "Child agent completed",
                        updatedAtMs: 2
                    ),
                ])
                source.model.setAgentTranscript(
                    "ui-child",
                    transcript: ConversationAgentTranscript(
                        messages: [childMessage],
                        items: [.message(childMessage)],
                        loaded: true
                    ),
                    sessionID: "ui-session"
                )
            }
            source.model.availableModels = uiTestModelCatalog
            source.model.activeModelId = uiTestModelCatalog[0]
            source.model.slashCommands = [
                ConversationSlashCommand(
                    name: "help",
                    description: "Show available commands",
                    aliases: ["h"],
                    argumentHint: "[topic]",
                    menuDescription: "Help",
                    source: "builtin",
                    hidden: false
                ),
                ConversationSlashCommand(
                    name: "review",
                    description: "Review the current changes",
                    aliases: ["rv"],
                    argumentHint: "[instructions]",
                    menuDescription: "Review changes",
                    source: "bundled",
                    hidden: false
                ),
            ]
            source.model.slashCommandsLoaded = true
            return source
        }

        /// A multi-provider stand-in for `ClientEvent::ModelList` so the composer's
        /// model picker is reachable in UI tests (the plain mock leaves
        /// `availableModels` empty, which disables the chip). Shaped like the
        /// engine's curated refs: provider-qualified, active model first.
        static let uiTestModelCatalog = [
            "anthropic/claude-sonnet-5",
            "anthropic/claude-opus-4-8",
            "anthropic/claude-haiku-4-5",
            "anthropic/claude-fable-5",
            "openai/gpt-5.5",
            "openai/gpt-5.4",
            "deepseek/deepseek-v4-flash",
            "deepseek/deepseek-v4-pro",
            "kimi/kimi-k3",
            "gemini/gemini-3.5-flash",
            "zai/glm-5.1",
        ]
    #endif

    func startNewConversation() {
        turnToken &+= 1
        sessionEpoch &+= 1
        activeTurnToken = nil
        model.messages = []
        model.items = []
        model.messageDetails = [:]
        model.expandedToolCalls = []
        model.streaming = false
        model.turnCompletion = nil
        model.isNew = true
        model.statusLine = nil
        model.error = nil
        model.notice = nil
        model.clearAgentState()
    }

    @discardableResult
    func send(_ text: String) -> ConversationTurnToken? {
        // PR-4 item 1: gate overlapping turns on rapid taps.
        guard !model.streaming, !model.isSelectedAgentReadOnly else { return nil }
        model.isNew = false
        model.notice = nil
        model.turnCompletion = nil
        turnSpeechSequence = 0
        let message = Message(role: .user, text: text)
        model.messages.append(message)
        model.items.append(.message(message))
        model.streaming = true
        model.updateMainAgent(status: "working", latestActivity: String(localized: "chat_working"))
        turnToken &+= 1
        let generation = turnToken
        let token = ConversationTurnToken(
            clientTurnId: nextTurnId,
            sessionEpoch: sessionEpoch
        )
        nextTurnId &+= 1
        activeTurnToken = token
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.1) { [weak self] in
            guard let self,
                  self.turnToken == generation,
                  self.activeTurnToken == token
            else { return }
            let reply = Message(role: .ai, tag: "思考了 8 秒", text: "已记入。继续追问。")
            self.model.messages.append(reply)
            self.model.items.append(.message(reply))
            self.model.streaming = false
            self.model.updateMainAgent(status: "idle", latestActivity: String(localized: "chat_completed"))
            self.turnSpeechSequence &+= 1
            self.model.turnSpeechUpdates.send(ConversationTurnSpeechUpdate(
                token: token,
                sequence: self.turnSpeechSequence,
                delta: reply.text
            ))
            self.model.turnCompletion = ConversationTurnCompletion(
                token: token,
                outcome: .completed,
                finalAssistantText: reply.text.trimmingCharacters(in: .whitespacesAndNewlines)
            )
            self.activeTurnToken = nil
        }
        return token
    }

    func cancel() {
        // PR-4 item 2: drop the in-flight canned reply and surface a Cancelled notice.
        guard model.streaming else { return }
        turnToken &+= 1
        model.streaming = false
        model.updateMainAgent(status: "cancelled", latestActivity: TurnNotice.cancelled.text)
        model.notice = .cancelled
        if let activeTurnToken {
            model.turnCompletion = ConversationTurnCompletion(
                token: activeTurnToken,
                outcome: .cancelled,
                finalAssistantText: ""
            )
        }
        activeTurnToken = nil
    }

    func dismissError() { model.error = nil }

    /// Mock model switch: no engine, so just swap the chip locally. Ordinarily
    /// `availableModels` is empty and `id` is a mock id from `MockData.models`;
    /// the UI-test fixture seeds a curated catalog, and there `id` is a real
    /// provider-qualified ref that only `activeModelId` can represent.
    func setModel(_ id: String) {
        if let opt = MockData.models.first(where: { $0.id == id }) {
            model.model = opt
        }
        if !model.availableModels.isEmpty {
            model.activeModelId = id
        }
    }

    /// Switch sessions: drop the in-flight canned reply (bump the token so its
    /// timer no-ops when it fires) and reset the conversation to the new session's
    /// default transcript. Without the token bump a reply scheduled for the OLD
    /// session would append into the NEW one (the wrong-session bug).
    func openSession(_ session: SessionRef) {
        turnToken &+= 1
        sessionEpoch &+= 1
        activeTurnToken = nil
        model.messages = MockData.messagesDefault
        model.items = MockData.messagesDefault.map(ConversationRenderItem.message)
        model.messageDetails = [:]
        model.streaming = false
        model.turnCompletion = nil
        model.isNew = false
        model.statusLine = nil
        model.error = nil
        model.notice = nil
        model.clearAgentState()
    }

}

// MARK: - Engine source (real, over UniFFI)

#if canImport(engine_mobileFFI)

    /// Runtime config for the in-process engine. The API key comes from the
    /// environment / an app setting — never hardcoded.
    struct EngineConfig {
        var apiBase: String
        var apiKey: String
        var model: String
        var appSandboxRoot: String
        var projectCwd: String?
        var providerProfilesJson: String?
        var providerRoutingJson: String?
        var mobileLinux: TerminalRuntimeConfig?

        /// Resolve the engine credentials. The API key (and optional base URL)
        /// come from the iOS Keychain FIRST (SHIP-BLOCKER #1 — a shipped app has no
        /// process env), with an environment override for development/CI so a
        /// `ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` in the env still wins for a
        /// dev run. An empty key is valid (turns 401 at run time, slash commands
        /// still work) and keeps the mock fallback in `make()`.
        static func fromEnvironment(appSandboxRoot: String,
                                    model: String,
                                    projectCwd: String? = nil,
                                    providerProfilesJson: String? = nil,
                                    providerRoutingJson: String? = nil,
                                    mobileLinux: TerminalRuntimeConfig? = nil) -> EngineConfig {
            let env = ProcessInfo.processInfo.environment
            // Key: env override (dev) > Keychain (shipped) > empty.
            let key = nonEmpty(env["ANTHROPIC_API_KEY"])
                ?? Keychain.get(.apiKey)
                ?? ""
            // Base URL: env override (dev) > Keychain (shipped) > Anthropic default.
            let base = nonEmpty(env["ANTHROPIC_BASE_URL"])
                ?? Keychain.get(.apiBase)
                ?? "https://api.anthropic.com"
            // Model: env override (dev) > caller-supplied (Keychain) > "" (engine
            // default). SHIP-BLOCKER #2: an EMPTY result is the intended "let the
            // engine pick `MobileConfig.default_model`" signal — `build_ios_engine`
            // only overrides `default_model` when the passed id is non-empty.
            return EngineConfig(
                apiBase: base,
                apiKey: key,
                model: nonEmpty(env["LINGXI_MODEL"]) ?? model,
                appSandboxRoot: appSandboxRoot,
                projectCwd: projectCwd,
                providerProfilesJson: providerProfilesJson,
                providerRoutingJson: providerRoutingJson,
                mobileLinux: mobileLinux
            )
        }

        /// `s` when it is non-nil and non-empty, else `nil` — so an unset OR blank
        /// env var falls through to the Keychain instead of masking it with "".
        private static func nonEmpty(_ s: String?) -> String? {
            guard let s, !s.isEmpty else { return nil }
            return s
        }
    }

    private final class OAuthPresentationContextProvider: NSObject, ASWebAuthenticationPresentationContextProviding {
        func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
            let windows = UIApplication.shared.connectedScenes
                .compactMap { $0 as? UIWindowScene }
                .flatMap(\.windows)
            return windows.first(where: \.isKeyWindow) ?? windows.first ?? UIWindow()
        }
    }

    /// The real conversation source: an in-process engine reached over UniFFI.
    ///
    /// Lifecycle: lazily build the `MobileEngineHandle` on first `send`; register
    /// `EngineListener` (an `IosEventListener`) whose `onEvent(_:)` maps each
    /// `ClientEvent` onto the `ConversationModel`. Turns are submitted via
    /// `handle.submit(.sendPrompt(...))`; the engine streams `TextDelta` /
    /// `ToolUse*` / `TurnEnded` back through the listener.
    @MainActor
    final class EngineConversationSource: ConversationSource {
        private static let turnLog = Logger(
            subsystem: "com.lingxi.code",
            category: "conversation-turn"
        )
        /// Project persistence replaces its cached index from SessionList, so
        /// iOS must override the host's five-row default with the full u32 range.
        private static let completeSessionListLimit = UInt32.max

        typealias HandleBuilder = @MainActor (
            _ config: IosEngineLaunchConfigFfi,
            _ listener: IosEventListener,
            _ permissions: IosPermissionSink
        ) throws -> MobileEngineHandle

        let model: ConversationModel

        private let config: EngineConfig
        private let handleBuilder: HandleBuilder
        private var handle: MobileEngineHandle?
        /// One shared bootstrap attempt for every entry point that needs the
        /// engine. Keeping the task on the main actor prevents two callers that
        /// interleave at an async submit from constructing competing handles.
        private var handleBuildTask: Task<MobileEngineHandle, Error>?
        private var handleBuildAttemptID: UInt64 = 0
        private var listener: EngineListener?
        /// The permission sink registered with the engine (SHIP-BLOCKER #3). Held so
        /// it outlives `ensureHandle`; Rust calls `onRequest` on it when a tool needs
        /// approval.
        private var permissionSink: EnginePermissionSink?
        private var externalEventHandler: ((ClientEvent) -> Void)?
        private var oauthSession: ASWebAuthenticationSession?
        private var oauthPresentationProvider: OAuthPresentationContextProvider?
        private var oauthFlowID: String?
        private var oauthCallbackHandler: ((URL) -> Void)?
        /// Index into `model.messages` of the assistant message currently being
        /// streamed (deltas append into it). `nil` between turns.
        private var streamingIndex: Int?
        /// The matching render-list slot for the in-flight assistant message.
        private var streamingItemIndex: Int?
        /// Monotonic per-turn correlator, also passed as the engine `turnId` so a
        /// `cancel` narrows to the exact in-flight turn. `nil` between turns.
        private var currentTurnId: UInt64?
        private var nextTurnId: UInt64 = 1
        /// Session-scoped event guard. Increment whenever the visible session
        /// changes so late events from an abandoned session/turn are ignored.
        private var sessionEpoch: UInt64 = 1
        private var activeTurnEpoch: UInt64?
        private var turnSpeechSequence: UInt64 = 0
        private var activeRunItemIndex: Int?
        /// Original user spelling for a slash command awaiting dispatch.
        private var pendingSlashRaw: String?
        private var testCommandSubmitter: ((ClientCommand) async throws -> Void)?
        private var testEmptySessionResumer: ((String, String) async throws -> Void)?
        private var cancellationOperation: CancellationOperation?
        private var nextCancellationOperationID: UInt64 = 1
        private var activeSessionTransitionOperationID: UInt64?
        private var nextSessionTransitionOperationID: UInt64 = 1

        private enum PendingSessionTransition: Equatable {
            case new
            case resume(String)
        }

        private enum SessionTransitionSubmission {
            case command(ClientCommand)
            case resumeEmpty(sessionID: String, title: String)
        }

        private var pendingSessionTransition: PendingSessionTransition?

        private struct TurnPrompt: Equatable {
            let text: String
            let turnId: UInt64
        }

        private struct CancellationOperation {
            let id: UInt64
            let turnId: UInt64
            let epoch: UInt64
            let pendingPermissions: [PendingPermission]
            let task: Task<Void, Error>
        }

        init(
            config: EngineConfig,
            handleBuilder: @escaping HandleBuilder = EngineConversationSource.buildDefaultHandle
        ) {
            self.config = config
            self.handleBuilder = handleBuilder
            // Seed the chip from the mock catalog only as a placeholder until the
            // engine's `ModelList` lands (SHIP-BLOCKER #2). The REAL active model is
            // `activeModelId`, set below from the (possibly empty) configured id and
            // then authoritatively replaced by `ModelList.current` / `ModelChanged`.
            self.model = ConversationModel(
                messages: [],
                model: MockData.models.first(where: { $0.id == config.model })
                    ?? MockData.models[0])
            // Out-of-band model state: the configured id (empty ⇒ engine default,
            // filled by the first `ModelList`). Never a branded mock id here.
            self.model.activeModelId = config.model
        }

        // MARK: ConversationSource

        func startNewConversation() {
            let turnIdToCancel = inFlightTurnForSessionSwitch()
            model.sessionRestoreRecovery = nil
            model.sessionTransitionFailure = nil
            let transitionOperationID = beginSessionTransition(.new)
            prepareSessionTransition(cancelling: turnIdToCancel, isNew: true)
            // Tell the engine to begin a fresh session (no cwd/model override —
            // the engine keeps its configured defaults). The new id arrives back
            // out-of-band via `SessionStarted`.
            submitSessionTransition(
                cancelling: turnIdToCancel,
                submission: .command(.newSession(cwd: nil, model: nil)),
                failurePrefix: String(localized: "chat_new_session_failed"),
                transitionOperationID: transitionOperationID,
                isNew: true
            )
        }

        /// Capture the old turn before a session transition. Its correlator stays
        /// live until Cancel succeeds so a delivery failure can restore the same
        /// retryable Stop state instead of discarding the old transcript.
        private func inFlightTurnForSessionSwitch() -> UInt64? {
            if let cancellationOperation { return cancellationOperation.turnId }
            guard model.streaming else { return nil }
            return currentTurnId
        }

        private func prepareSessionTransition(cancelling turnId: UInt64?, isNew: Bool) {
            guard let turnId else {
                resetTranscriptForSessionSwitch(isNew: isNew)
                return
            }
            _ = operationForCancelling(turnId: turnId)
            model.isCancelling = true
            model.statusLine = String(localized: "chat_stopping")
            model.pendingPermissions = []
        }

        private func submitSessionTransition(
            cancelling turnId: UInt64?,
            submission: SessionTransitionSubmission,
            failurePrefix: String,
            transitionOperationID: UInt64,
            resumeTargetID: String? = nil,
            allowsMissingSessionReplacement: Bool = false,
            isNew: Bool
        ) {
            Task { [weak self] in
                guard let self else { return }
                if let turnId {
                    do {
                        try await self.awaitCancellation(turnId)
                    } catch {
                        guard self.activeSessionTransitionOperationID == transitionOperationID else {
                            return
                        }
                        // `finishCancellation` has already restored the original
                        // visible turn and retryable Stop state. Abandon only the
                        // requested session transition; treating this as a turn-
                        // terminal host error would incorrectly clear ownership.
                        if let resumeTargetID {
                            self.model.sessionTransitionFailure = SessionTransitionFailure(
                                requestedSessionID: resumeTargetID
                            )
                        }
                        self.setPendingSessionTransition(nil)
                        if self.model.error == nil {
                            self.model.error = ConversationError(
                                kind: .host,
                                message: String(localized: "chat_cancel_old_session_failed \(error)")
                            )
                        }
                        return
                    }
                    guard self.activeSessionTransitionOperationID == transitionOperationID else {
                        return
                    }
                    self.resetTranscriptForSessionSwitch(isNew: isNew)
                }
                guard self.activeSessionTransitionOperationID == transitionOperationID else {
                    return
                }
                do {
                    try await self.submitSessionTransition(submission)
                } catch {
                    guard self.activeSessionTransitionOperationID == transitionOperationID else {
                        return
                    }
                    if let resumeTargetID {
                        // A slower failed restore must not clobber a newer drawer
                        // selection that has already replaced the pending target.
                        guard self.pendingSessionTransition == .resume(resumeTargetID) else {
                            return
                        }
                        if allowsMissingSessionReplacement,
                           Self.isMissingSessionResumeError(error) {
                            await self.replaceUnavailableSession(resumeTargetID)
                            return
                        }
                        self.model.sessionTransitionFailure = SessionTransitionFailure(
                            requestedSessionID: resumeTargetID
                        )
                    }
                    self.setPendingSessionTransition(nil)
                    self.fail(.host, "\(failurePrefix)：\(error)")
                }
            }
        }

        private func submitSessionTransition(
            _ submission: SessionTransitionSubmission
        ) async throws {
            switch submission {
            case let .command(command):
                try await submitCommand(command)
            case let .resumeEmpty(sessionID, title):
                if let testEmptySessionResumer {
                    try await testEmptySessionResumer(sessionID, title)
                    return
                }
                let handle = try await ensureHandle()
                try await handle.resumeEmptySession(sessionId: sessionID, title: title)
            }
        }

        /// A missing on-disk session is recoverable startup state, not an engine
        /// outage. Keep the transition pending while a replacement is created so
        /// an older SessionList cannot erase the cached drawer index in between.
        private func replaceUnavailableSession(_ unavailableSessionID: String) async {
            model.sessionRestoreRecovery = SessionRestoreRecovery(
                unavailableSessionID: unavailableSessionID
            )
            setPendingSessionTransition(.new)
            resetTranscriptForSessionSwitch(isNew: true)
            do {
                try await submitCommand(.newSession(cwd: nil, model: nil))
                model.statusLine = String(localized: "chat_session_replaced")
            } catch {
                setPendingSessionTransition(nil)
                fail(.host, String(localized: "chat_session_replaced_failed \(error)"))
            }
        }

        /// Branch on the generated protocol error first. Older hosts may still
        /// report a missing resume target as Rejected, whose message is matched
        /// narrowly; unrelated transport/protocol failures stay visible.
        private static func isMissingSessionResumeError(_ error: Error) -> Bool {
            guard let clientError = error as? ClientError else { return false }
            switch clientError {
            case .NotFound:
                return true
            case let .Rejected(message):
                let normalized = message.lowercased()
                guard normalized.contains("not resumable") else { return false }
                return normalized.contains("was not found")
                    || normalized.contains("session not found")
                    || normalized.contains("sessionnotfound")
            case .Transport, .Protocol, .Internal:
                return false
            @unknown default:
                return false
            }
        }

        private func setPendingSessionTransition(_ transition: PendingSessionTransition?) {
            pendingSessionTransition = transition
            model.sessionTransitionPending = transition != nil
            if transition == nil {
                activeSessionTransitionOperationID = nil
            }
        }

        private func beginSessionTransition(
            _ transition: PendingSessionTransition
        ) -> UInt64 {
            let operationID = nextSessionTransitionOperationID
            nextSessionTransitionOperationID &+= 1
            activeSessionTransitionOperationID = operationID
            setPendingSessionTransition(transition)
            return operationID
        }

        private func submitSessionCancellation(_ turnId: UInt64?, isNew: Bool) {
            guard let turnId else {
                resetTranscriptForSessionSwitch(isNew: isNew)
                return
            }
            prepareSessionTransition(cancelling: turnId, isNew: isNew)
            Task { [weak self] in
                guard let self else { return }
                do {
                    try await self.awaitCancellation(turnId)
                    self.resetTranscriptForSessionSwitch(isNew: isNew)
                } catch {
                    if self.model.error == nil {
                        self.model.error = ConversationError(
                            kind: .host,
                            message: String(localized: "chat_cancel_old_session_failed \(error)")
                        )
                    }
                }
            }
        }

        /// Shared transcript reset used by new/resume/open-session after any old
        /// turn has safely released its engine slot. Incrementing the epoch also
        /// makes late transport delivery harmless. `isNew` drives the empty-state
        /// versus a placeholder transcript.
        private func resetTranscriptForSessionSwitch(isNew: Bool) {
            invalidateTurnContext()
            model.clearAgentState()
            model.messages = []
            model.items = model.messages.map(ConversationRenderItem.message)
            model.messageDetails = [:]
            model.streaming = false
            model.isCancelling = false
            model.slashCommandPending = false
            model.turnCompletion = nil
            model.isNew = isNew
            model.statusLine = nil
            model.error = nil
            model.notice = nil
            // A pending permission belongs to the turn we're abandoning — drop it
            // so a stale prompt can't leak into the session we're switching to.
            model.pendingPermissions = []
            // Same for a pending questionnaire: its request id is scoped to
            // the abandoned session/connection. If the target session still
            // has one pending, the broker replays it after the switch.
            model.pendingQuestions = []
            // The plan and the expanded-row set belong to the transcript we're
            // dropping; the next `PlanUpdated` re-establishes a plan.
            model.planTasks = []
            model.expandedToolCalls = []
        }

        private func invalidateTurnContext() {
            sessionEpoch &+= 1
            streamingIndex = nil
            streamingItemIndex = nil
            currentTurnId = nil
            activeTurnEpoch = nil
            activeRunItemIndex = nil
            pendingSlashRaw = nil
            model.slashCommandPending = false
        }

        private func clearTurnPointers(keepEpoch: Bool = true) {
            streamingIndex = nil
            streamingItemIndex = nil
            currentTurnId = nil
            activeTurnEpoch = keepEpoch ? activeTurnEpoch : nil
            activeRunItemIndex = nil
            pendingSlashRaw = nil
            model.slashCommandPending = false
        }

        private var activeConversationTurnToken: ConversationTurnToken? {
            guard let currentTurnId,
                  let activeTurnEpoch,
                  activeTurnEpoch == sessionEpoch
            else { return nil }
            return ConversationTurnToken(
                clientTurnId: currentTurnId,
                sessionEpoch: activeTurnEpoch
            )
        }

        private func finalAssistantTextForActiveTurn() -> String {
            guard let streamingIndex,
                  model.messages.indices.contains(streamingIndex)
            else { return "" }
            let message = model.messages[streamingIndex]
            guard case .ai = message.role else { return "" }
            if let detail = model.messageDetails[message.id] {
                return detail.blocks.compactMap { block -> String? in
                    guard case let .text(text) = block else { return nil }
                    return text
                }
                .joined(separator: "\n\n")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            }
            return message.text
                .trimmingCharacters(in: .whitespacesAndNewlines)
        }

        private func publishActiveTurnCompletion(
            _ outcome: ConversationTurnCompletion.Outcome
        ) {
            guard let token = activeConversationTurnToken else { return }
            model.turnCompletion = ConversationTurnCompletion(
                token: token,
                outcome: outcome,
                finalAssistantText: outcome == .completed
                    ? finalAssistantTextForActiveTurn()
                    : ""
            )
        }

        private func appendMessage(_ message: Message, detail: ConversationMessageDetail? = nil) {
            model.messages.append(message)
            model.items.append(.message(message))
            if let detail {
                model.messageDetails[message.id] = detail
            } else {
                model.messageDetails.removeValue(forKey: message.id)
            }
        }

        private func replaceStreamingMessage(_ message: Message, detail: ConversationMessageDetail? = nil) {
            guard let streamingIndex,
                  model.messages.indices.contains(streamingIndex)
            else {
                appendMessage(message, detail: detail)
                self.streamingIndex = model.messages.count - 1
                self.streamingItemIndex = model.items.count - 1
                return
            }
            let oldMessage = model.messages[streamingIndex]
            // A streamed assistant reply is one logical list row. Keep its
            // identity stable while text/tag/detail are replaced; otherwise
            // SwiftUI treats every token as a row deletion + insertion and
            // re-lays out the transcript from scratch.
            let stableMessage = Message(
                id: oldMessage.id,
                role: message.role,
                tag: message.tag,
                text: message.text
            )
            model.messages[streamingIndex] = stableMessage
            if let itemIndex = streamingItemIndex,
               model.items.indices.contains(itemIndex) {
                model.items[itemIndex] = .message(stableMessage)
            } else if let itemIndex = model.items.firstIndex(where: { item in
                if case let .message(existing) = item {
                    return existing.id == oldMessage.id
                }
                return false
            }) {
                model.items[itemIndex] = .message(stableMessage)
                streamingItemIndex = itemIndex
            }
            model.messageDetails.removeValue(forKey: oldMessage.id)
            if let detail {
                model.messageDetails[stableMessage.id] = detail
            }
        }

        private func ensureActiveRun() -> ConversationExecutionRun {
            if let itemIndex = activeRunItemIndex,
               model.items.indices.contains(itemIndex),
               case let .run(run) = model.items[itemIndex] {
                return run
            }
            let run = ConversationExecutionRun(
                id: "session-\(sessionEpoch)-turn-\(currentTurnId ?? 0)",
                sessionId: model.activeSessionId,
                turnId: currentTurnId,
                status: .running
            )
            model.items.append(.run(run))
            activeRunItemIndex = model.items.count - 1
            return run
        }

        @discardableResult
        private func updateActiveRun(_ mutate: (inout ConversationExecutionRun) -> Void) -> ConversationExecutionRun {
            var run = ensureActiveRun()
            mutate(&run)
            if let itemIndex = activeRunItemIndex,
               model.items.indices.contains(itemIndex) {
                model.items[itemIndex] = .run(run)
            }
            return run
        }

        private func appendActivity(_ activity: ConversationExecutionActivity) {
            updateActiveRun { run in
                run.activities.append(activity)
            }
        }

        private func appendReasoningActivity(_ text: String) {
            guard !text.isEmpty else { return }
            updateActiveRun { run in
                if let index = run.activities.lastIndex(where: {
                    if case .reasoning = $0 { return true }
                    return false
                }), index == run.activities.index(before: run.activities.endIndex),
                   case let .reasoning(id, current) = run.activities[index] {
                    run.activities[index] = .reasoning(id: id, text: current + text)
                } else {
                    run.activities.append(.reasoning(id: "reasoning-\(UUID().uuidString)", text: text))
                }
                run.reasoning += text
            }
        }

        private func appendTextBoundaryActivity() {
            updateActiveRun { run in
                if case .textBoundary = run.activities.last { return }
                run.activities.append(.textBoundary(id: "boundary-\(UUID().uuidString)"))
            }
        }

        private func appendToolActivity(id: String) {
            updateActiveRun { run in
                guard !run.activities.contains(where: {
                    if case let .tool(existing) = $0 { return existing == id }
                    return false
                }) else { return }
                run.activities.append(.tool(id: id))
            }
        }

        private func appendNoticeActivity(id: String) {
            appendActivity(.notice(id: id))
        }

        private func upsertTool(
            id: String,
            tool: String,
            fallbackSummary: String?,
            mutate: (inout ConversationToolTrace) -> Void
        ) {
            updateActiveRun { run in
                if let index = run.tools.firstIndex(where: { $0.id == id }) {
                    var existing = run.tools[index]
                    mutate(&existing)
                    run.tools[index] = existing
                } else {
                    var trace = ConversationToolTrace(
                        id: id,
                        tool: tool,
                        status: .running,
                        inputSummary: fallbackSummary,
                        outputSummary: nil,
                        elapsedMs: nil
                    )
                    mutate(&trace)
                    run.tools.append(trace)
                }
            }
        }

        private func upsertShellCard(
            id: String,
            create: () -> ConversationShellCard,
            mutate: (inout ConversationShellCard) -> Void
        ) {
            updateActiveRun { run in
                if let index = run.shellCards.firstIndex(where: { $0.taskId == id }) {
                    var existing = run.shellCards[index]
                    mutate(&existing)
                    run.shellCards[index] = existing
                } else {
                    var card = create()
                    mutate(&card)
                    run.shellCards.append(card)
                }
            }
        }

        /// Close transient tool and shell rows owned by the active turn. A
        /// coordinator worker may legitimately outlive that turn, so its count
        /// remains live until the engine publishes a later idle status.
        private func finishActiveRun(_ status: ConversationExecutionStatus) {
            let toolStatus: ConversationToolStatus
            let shellStatus: ConversationShellStatus
            switch status {
            case .running:
                toolStatus = .running
                shellStatus = .running
            case .completed, .maxTurns, .restored:
                toolStatus = .completed
                shellStatus = .completed
            case .failed:
                toolStatus = .failed
                shellStatus = .failed
            case .cancelled:
                toolStatus = .cancelled
                shellStatus = .cancelled
            }
            updateActiveRun { run in
                run.status = status
                for index in run.tools.indices where run.tools[index].status == .running {
                    run.tools[index].status = toolStatus
                }
                for index in run.shellCards.indices where run.shellCards[index].status == .running {
                    run.shellCards[index].status = shellStatus
                }
            }
            // Keep the run at its original wire position. The timeline
            // projection removes the run card from durable message rows while
            // preserving the user → activity → assistant ordering.
        }

        private func updateCoordinatorStatus(activeWorkers: UInt32, team: String?) {
            if activeWorkers == 0 {
                // CoordinatorStatus is a global snapshot, not a turn-scoped
                // delta. A newer user turn may already own the active pointer,
                // so clear every older run that still carries a worker count.
                for itemIndex in model.items.indices {
                    guard case var .run(run) = model.items[itemIndex],
                          run.activeWorkers > 0
                    else { continue }
                    run.activeWorkers = 0
                    if let team { run.coordinatorTeam = team }
                    model.items[itemIndex] = .run(run)
                }
                return
            }
            if let itemIndex = activeRunItemIndex,
               model.items.indices.contains(itemIndex),
               case var .run(run) = model.items[itemIndex] {
                run.activeWorkers = activeWorkers
                run.coordinatorTeam = team
                model.items[itemIndex] = .run(run)
                return
            }

            // `TurnEnded` clears the active-turn pointers, but coordinator
            // workers are asynchronous and can report their final idle state
            // afterward. Update the latest run that still owns workers instead
            // of creating a detached synthetic run.
            guard let itemIndex = model.items.lastIndex(where: { item in
                guard case let .run(run) = item else { return false }
                return run.activeWorkers > 0
            }), case var .run(run) = model.items[itemIndex]
            else { return }
            run.activeWorkers = activeWorkers
            run.coordinatorTeam = team
            model.items[itemIndex] = .run(run)
        }

        private func acceptTurnEvent(_ event: ClientEvent) -> Bool {
            switch event {
            case .askUserQuestion, .askUserQuestionResolved, .taskStatusChanged, .taskRow,
                 .planUpdated, .coordinatorStatus:
                // Deliberately OUTSIDE the turn gate: the engine's broker
                // replays a still-pending AskUserQuestion (and resolves it)
                // after a foreground re-connect, a background task's status
                // change lands after its turn already ended, `TaskRow`
                // rows answer an out-of-band `TaskList`, and a `PlanUpdated`
                // is the authoritative full-list replace of a block that
                // outlives its turn — all of these arrive with no turn in
                // flight and must not be dropped as out-of-turn strays.
                return true
            default:
                break
            }
            guard let currentTurnId, let activeTurnEpoch, activeTurnEpoch == sessionEpoch else {
                return false
            }
            switch event {
            case let .turnStarted(turnId):
                return turnId == nil || turnId == currentTurnId
            default:
                return true
            }
        }

        private func submitCommand(_ command: ClientCommand) async throws {
            if let testCommandSubmitter {
                try await testCommandSubmitter(command)
                return
            }
            let handle = try await ensureHandle()
            try await handle.submit(command: command)
        }

        private func operationForCancelling(turnId: UInt64) -> CancellationOperation {
            if let operation = cancellationOperation, operation.turnId == turnId {
                return operation
            }
            let operationID = nextCancellationOperationID
            nextCancellationOperationID &+= 1
            let epoch = activeTurnEpoch ?? sessionEpoch
            let task = Task { @MainActor [weak self] in
                guard let self else { return }
                try await self.submitCommand(.cancel(turnId: turnId))
            }
            let operation = CancellationOperation(
                id: operationID,
                turnId: turnId,
                epoch: epoch,
                pendingPermissions: model.pendingPermissions,
                task: task
            )
            cancellationOperation = operation
            return operation
        }

        private func finishCancellation(
            _ operation: CancellationOperation,
            error: Error?
        ) {
            guard cancellationOperation?.id == operation.id else { return }
            cancellationOperation = nil

            let stillOwnsVisibleTurn = currentTurnId == operation.turnId
                && activeTurnEpoch == operation.epoch
                && operation.epoch == sessionEpoch
            model.isCancelling = false

            if let error {
                if stillOwnsVisibleTurn {
                    // Delivery failed before the engine confirmed release. Keep
                    // the original turn live so Stop can be retried safely.
                    model.streaming = true
                    model.statusLine = nil
                    if model.pendingPermissions.isEmpty {
                        model.pendingPermissions = operation.pendingPermissions
                    }
                    model.error = ConversationError(kind: .host, message: String(localized: "chat_cancel_failed \(error)"))
                } else if model.statusLine == String(localized: "chat_stopping") {
                    model.statusLine = nil
                }
                Self.turnLog.error(
                    "cancel failed turn=\(operation.turnId, privacy: .public) error=\(String(describing: error), privacy: .private(mask: .hash))"
                )
                return
            }

            if stillOwnsVisibleTurn {
                // The host only returns after all event producers have joined and
                // the single-turn slot is released. This is also a fallback for a
                // transport that failed to deliver its terminal event.
                model.streaming = false
                model.statusLine = nil
                model.notice = .cancelled
                finishActiveRun(.cancelled)
                publishActiveTurnCompletion(.cancelled)
                clearTurnPointers(keepEpoch: false)
            } else if model.statusLine == String(localized: "chat_stopping") {
                model.statusLine = nil
            }
            requestSessionCatalogRefreshAfterSettledTurn()
            Self.turnLog.debug(
                "cancel returned turn=\(operation.turnId, privacy: .public)"
            )
        }

        private func awaitCancellation(_ turnId: UInt64) async throws {
            let operation = operationForCancelling(turnId: turnId)
            do {
                try await operation.task.value
                finishCancellation(operation, error: nil)
            } catch {
                finishCancellation(operation, error: error)
                throw error
            }
        }

        private func startPrompt(_ prompt: TurnPrompt) -> ConversationTurnToken {
            model.notice = nil
            model.streaming = true
            model.isCancelling = false
            model.slashCommandPending = false
            model.turnCompletion = nil
            turnSpeechSequence = 0
            model.statusLine = nil
            streamingIndex = nil
            streamingItemIndex = nil
            currentTurnId = prompt.turnId
            activeTurnEpoch = sessionEpoch
            let token = ConversationTurnToken(
                clientTurnId: prompt.turnId,
                sessionEpoch: sessionEpoch
            )
            Self.turnLog.debug(
                "prompt submit turn=\(prompt.turnId, privacy: .public) epoch=\(self.sessionEpoch, privacy: .public)"
            )

            Task { [weak self] in
                guard let self else { return }
                do {
                    try await self.submitCommand(.sendPrompt(
                        text: prompt.text,
                        promptMode: nil,
                        images: [],
                        turnId: prompt.turnId))
                } catch {
                    guard self.activeConversationTurnToken == token else { return }
                    self.fail(.host, "\(error)")
                }
            }
            return token
        }

        private func startSlashCommand(raw: String, turnId: UInt64) -> ConversationTurnToken {
            model.notice = nil
            model.streaming = false
            model.isCancelling = false
            model.slashCommandPending = true
            model.turnCompletion = nil
            turnSpeechSequence = 0
            model.statusLine = nil
            streamingIndex = nil
            streamingItemIndex = nil
            currentTurnId = turnId
            activeTurnEpoch = sessionEpoch
            pendingSlashRaw = raw
            let token = ConversationTurnToken(clientTurnId: turnId, sessionEpoch: sessionEpoch)

            Task { [weak self] in
                guard let self else { return }
                do {
                    try await self.submitCommand(.runSlashCommand(raw: raw, turnId: turnId))
                } catch {
                    guard self.activeConversationTurnToken == token else { return }
                    self.fail(.host, "\(error)")
                }
            }
            return token
        }

        private func requestSessionCatalogRefreshAfterSettledTurn() {
            guard
                !model.streaming,
                !model.isCancelling,
                !model.isNew,
                !model.activeSessionId.isEmpty,
                !model.sessionTransitionPending
            else { return }
            model.sessionRefreshRevision &+= 1
        }

        @discardableResult
        func send(_ text: String) -> ConversationTurnToken? {
            // PR-4 item 1: a turn is already in flight — ignore the tap so we
            // never start an overlapping turn (which would corrupt appendDelta's
            // single `streamingIndex`). The Stop button is how you interrupt.
            guard
                !model.streaming,
                !model.isCancelling,
                !model.slashCommandPending,
                !model.sessionTransitionPending,
                !model.isSelectedAgentReadOnly
            else { return nil }

            model.isNew = false
            model.notice = nil
            appendMessage(Message(role: .user, text: text))

            let turnId = nextTurnId
            nextTurnId &+= 1
            if model.slashCommandsLoaded,
               SlashCommandMatcher.exactCommand(in: text, catalog: model.slashCommands) != nil {
                return startSlashCommand(raw: text, turnId: turnId)
            }
            return startPrompt(TurnPrompt(text: text, turnId: turnId))
        }

        func cancelAndWait() async throws {
            let operation: CancellationOperation
            if let currentOperation = cancellationOperation {
                operation = currentOperation
            } else {
                // PR-4 item 2: nothing in flight — no-op.
                guard model.streaming, let turnId = currentTurnId else { return }
                operation = operationForCancelling(turnId: turnId)
            }
            Self.turnLog.debug(
                "cancel requested turn=\(operation.turnId, privacy: .public) epoch=\(self.sessionEpoch, privacy: .public)"
            )
            model.isCancelling = true
            model.statusLine = String(localized: "chat_stopping")
            model.pendingPermissions = []
            do {
                try await operation.task.value
                finishCancellation(operation, error: nil)
            } catch {
                finishCancellation(operation, error: error)
                throw error
            }
        }

        func cancel() {
            Task { [weak self] in
                guard let self else { return }
                try? await self.cancelAndWait()
            }
        }

        func dismissError() { model.error = nil }

        func submitEngineCommand(_ command: ClientCommand) async throws {
            try await submitCommand(command)
        }

        func testProviderConnection(
            profile: ProviderLaunchProfile,
            credentialOverride: String?
        ) async throws -> ProviderConnectionTestResult {
            let handle = try await ensureHandle()
            let trimmed = credentialOverride?.trimmingCharacters(in: .whitespacesAndNewlines)
            let result = await handle.testProviderConnection(
                providerId: profile.id,
                providerPreset: profile.presetID,
                apiBase: profile.baseURL,
                model: profile.modelID,
                credentialOverride: trimmed.flatMap { value in
                    value.isEmpty ? nil : ProviderCredentialSecretDto(value: value)
                }
            )
            return result.connected
                ? .success(
                    message: "\(result.message) · \(result.latencyMs)ms",
                    usedStoredCredential: result.usedStoredCredential
                )
                : .failure(message: result.message)
        }

        func loginOAuth(provider: String) async throws -> ProviderOAuthState {
            let handle = try await ensureHandle()
            let session = try await handle.beginOAuth(
                provider: provider,
                redirectUri: "lingxi://oauth/callback"
            )
            guard let authorizationURL = URL(string: session.authorizationUrl) else {
                throw NSError(domain: "ConversationSource", code: 2, userInfo: [
                    NSLocalizedDescriptionKey: "OAuth 授权地址无效",
                ])
            }

            return try await withCheckedThrowingContinuation { continuation in
                oauthFlowID = session.flowId
                let flowID = session.flowId
                let callbackScheme = session.callbackUrlScheme
                let presentationProvider = OAuthPresentationContextProvider()
                oauthPresentationProvider = presentationProvider
                oauthCallbackHandler = { [weak self] url in
                    guard let self else { return }
                    guard self.oauthFlowID == flowID else { return }
                    self.oauthCallbackHandler = nil
                    self.oauthFlowID = nil
                    Task { @MainActor in
                        do {
                            let state = try await handle.completeOAuth(
                                flowId: flowID,
                                callbackUrl: url.absoluteString
                            )
                            self.oauthSession = nil
                            self.oauthPresentationProvider = nil
                            continuation.resume(returning: Self.lowerOAuthState(state))
                        } catch {
                            self.oauthSession = nil
                            self.oauthPresentationProvider = nil
                            // The callback has become terminal even when the
                            // token exchange or secure-store write fails. Rust
                            // normally consumes the flow before exchange, but
                            // cancel is an idempotent defence for malformed
                            // callbacks and older engine binaries.
                            await handle.cancelOAuth(flowId: flowID)
                            continuation.resume(throwing: error)
                        }
                    }
                }

                let webSession = ASWebAuthenticationSession(
                    url: authorizationURL,
                    callbackURLScheme: callbackScheme
                ) { [weak self] url, error in
                    guard let self else { return }
                    guard self.oauthFlowID == flowID else { return }
                    if let error {
                        self.oauthCallbackHandler = nil
                        self.oauthSession = nil
                        self.oauthPresentationProvider = nil
                        self.oauthFlowID = nil
                        Task { await handle.cancelOAuth(flowId: flowID) }
                        continuation.resume(throwing: error)
                        return
                    }
                    if let url {
                        self.handleOAuthCallback(url)
                    }
                }
                webSession.presentationContextProvider = presentationProvider
                webSession.prefersEphemeralWebBrowserSession = false
                oauthSession = webSession
                if !webSession.start() {
                    oauthCallbackHandler = nil
                    oauthSession = nil
                    oauthPresentationProvider = nil
                    oauthFlowID = nil
                    Task { await handle.cancelOAuth(flowId: flowID) }
                    continuation.resume(throwing: NSError(
                        domain: "ConversationSource",
                        code: 3,
                        userInfo: [NSLocalizedDescriptionKey: "无法启动系统浏览器 OAuth 会话"]
                    ))
                }
            }
        }

        func logoutOAuth(provider: String) async throws {
            let handle = try await ensureHandle()
            try await handle.logoutOAuth(provider: provider)
        }

        func authState(provider: String) async throws -> ProviderOAuthState {
            let handle = try await ensureHandle()
            return Self.lowerOAuthState(try await handle.authState(provider: provider))
        }

        func testOAuthConnection(
            provider: String,
            profile: ProviderLaunchProfile
        ) async throws -> ProviderConnectionTestResult {
            let handle = try await ensureHandle()
            let result = await handle.testOAuthConnection(
                provider: provider,
                apiBase: profile.baseURL,
                model: profile.modelID
            )
            return result.connected
                ? .success(
                    message: "\(result.message) · \(result.latencyMs)ms",
                    usedStoredCredential: result.usedStoredCredential
                )
                : .failure(message: result.message)
        }

        func handleOAuthCallback(_ url: URL) {
            oauthCallbackHandler?(url)
        }

        private static func lowerOAuthState(_ state: MobileOAuthStateDto) -> ProviderOAuthState {
            ProviderOAuthState(
                provider: state.provider,
                signedIn: state.signedIn,
                accountLabel: state.accountLabel,
                accountID: state.accountId,
                organizationID: state.organizationId,
                fedramp: state.fedramp
            )
        }

        func setExternalEventHandler(_ handler: ((ClientEvent) -> Void)?) {
            externalEventHandler = handler
        }

        // MARK: handle construction

        /// Build the engine handle once (lazily). Registers the listener so the
        /// adapter can stream events the moment the first turn runs.
        private func ensureHandle() async throws -> MobileEngineHandle {
            if let handle { return handle }
            if let handleBuildTask {
                return try await handleBuildTask.value
            }

            let listener = EngineListener(source: self)
            // SHIP-BLOCKER #3: register a real permission sink so a tool that needs
            // approval surfaces a prompt instead of hanging the turn forever.
            let permissionSink = EnginePermissionSink(source: self)
            let providerConfig = config.providerProfilesJson.map {
                IosProviderConfigFfi(
                    providerProfilesJson: $0,
                    routingJson: config.providerRoutingJson
                )
            }
            let launchConfig = IosEngineLaunchConfigFfi(
                apiBase: config.apiBase,
                apiKey: config.apiKey,
                model: config.model,
                appSandboxRoot: config.appSandboxRoot,
                projectCwd: config.projectCwd,
                providerConfig: providerConfig,
                mobileLinux: config.mobileLinux.map {
                    makeIosMobileLinuxConfig($0, appSandboxRoot: config.appSandboxRoot)
                },
                localAppsFullRuntime: LocalAppsRuntimeDistribution.usesFullRuntime,
                localAppsRuntimeRoot: LocalAppsRuntimeDistribution.runtimeRoot,
                physicalMemoryBytes: ProcessInfo.processInfo.physicalMemory,
                hostEnvironment: makeIosHostEnvironment(launchMode: .interactive)
            )
            let handleBuilder = self.handleBuilder
            handleBuildAttemptID &+= 1
            let attemptID = handleBuildAttemptID
            let buildTask = Task { @MainActor [handleBuilder, launchConfig, listener, permissionSink] in
                let handle = try handleBuilder(launchConfig, listener, permissionSink)
                // Bootstrap listings are part of construction: never publish a
                // handle that failed halfway through initialization.
                try await handle.submit(command: .listModels)
                try await handle.submit(
                    command: .listSessions(limit: EngineConversationSource.completeSessionListLimit)
                )
                // The composer has no static fallback: populate its command
                // palette from the same dynamic registry used for execution.
                try await handle.submit(command: .refreshListings(which: [.slashCommands]))
                // Agent roster is session-scoped and follows the same
                // out-of-band bootstrap path as models/sessions so the picker
                // is populated before the first turn.
                try? await handle.submit(command: .listSessionAgents)
                // Seed the pinned tasks panel: a re-opened scope may already
                // have a Workflow build running; its rows arrive as `TaskRow`
                // events (zero rows ⇒ zero events). Best-effort — unlike the
                // model/session listings above, the panel is not worth
                // failing the whole handle over (it self-heals on pushes).
                try? await handle.submit(command: .taskList(statusFilter: nil))
                return handle
            }
            handleBuildTask = buildTask

            do {
                let builtHandle = try await buildTask.value
                if handleBuildAttemptID == attemptID {
                    handle = builtHandle
                    self.listener = listener
                    self.permissionSink = permissionSink
                    handleBuildTask = nil
                }
                return handle ?? builtHandle
            } catch {
                if handleBuildAttemptID == attemptID {
                    handleBuildTask = nil
                }
                throw error
            }
        }

        private static func buildDefaultHandle(
            config: IosEngineLaunchConfigFfi,
            listener: IosEventListener,
            permissions: IosPermissionSink
        ) throws -> MobileEngineHandle {
            try buildIosEngineWithConfig(
                config: config,
                listener: listener,
                stt: SttImpl(),
                tts: TtsImpl(),
                camera: CameraImpl(),
                share: ShareImpl(),
                voice: VoiceImpl(),
                notifications: NotificationImpl(),
                clipboard: ClipboardImpl(),
                permissions: permissions,
                // Native Keychain secure store enables OAuth token persistence.
                secureStorage: SecureStorageImpl(),
                // One-shot location for local apps that declared it. The cron
                // bridge passes nothing here (the FFI defaults it to nil):
                // a background wake has no user present to answer an
                // authorization sheet.
                location: LocationImpl()
            )
        }

        /// Build the handle eagerly (independent of the first turn) so the model
        /// catalog populates as soon as the source is shown — the picker shouldn't
        /// have to wait for a sent message to learn the real ids. A build failure is
        /// surfaced as a host error banner; a later `send` will retry via the same
        /// `ensureHandle`.
        func warmUp() {
            guard handle == nil else { return }
            Task { [weak self] in
                guard let self else { return }
                do { _ = try await self.ensureHandle() }
                catch { self.fail(.host, "\(error)") }
            }
        }

        func prepare() async throws {
            _ = try await ensureHandle()
        }

        /// Refresh the real session catalog (drawer-open). Builds the handle if
        /// needed — `ensureHandle` already submits `listSessions` on first build,
        /// so the extra submit on a fresh build is a harmless idempotent refresh;
        /// on an existing handle it re-pulls so the drawer reflects sessions
        /// created since the handle was built. A build/submit failure surfaces as
        /// a host error banner.
        func listSessions() {
            Task { [weak self] in
                guard let self else { return }
                do {
                    let handle = try await self.ensureHandle()
                    try await handle.submit(
                        command: .listSessions(limit: Self.completeSessionListLimit)
                    )
                } catch {
                    self.fail(.host, "\(error)")
                }
            }
        }

        /// Pull the background-task rows (out-of-band, like `listSessions`).
        /// Each row lands on `apply` as a `TaskRow` event → the pinned panel.
        /// Uses the LIVE handle only — this is called from `apply`, where a
        /// handle necessarily exists in production; never trigger a build
        /// (hermetic `applyForTesting` callers must stay engine-free).
        func refreshBackgroundTasks() {
            guard let handle else { return }
            Task {
                // Best-effort: the panel self-heals on the next status push.
                try? await handle.submit(command: .taskList(statusFilter: nil))
            }
        }

        /// Upsert one background-task row. `description: nil` keeps whatever
        /// text is already known (a status push carries no description).
        private func upsertBackgroundTask(
            id: String,
            description: String?,
            status: BackgroundTaskSnapshot.Status
        ) {
            if let index = model.backgroundTasks.firstIndex(where: { $0.id == id }) {
                if !(model.backgroundTasks[index].status.isTerminal && !status.isTerminal) {
                    model.backgroundTasks[index].status = status
                }
                if let description, !description.isEmpty {
                    model.backgroundTasks[index].descriptionText = description
                }
            } else {
                model.backgroundTasks.append(BackgroundTaskSnapshot(
                    id: id,
                    descriptionText: description ?? "",
                    status: status
                ))
            }
        }

        private static func backgroundTaskStatus(
            _ status: TaskStatusDto
        ) -> BackgroundTaskSnapshot.Status? {
            switch status {
            case .pending: return .pending
            case .running: return .running
            case .completed: return .completed
            case .failed: return .failed
            case .cancelled: return .cancelled
            @unknown default: return nil
            }
        }

        /// Pull the engine's real MCP server listing (out-of-band, like
        /// `listSessions`). The `McpServers` reply lands on `apply` → `model.mcpServers`.
        func refreshMcpServers() {
            Task { [weak self] in
                guard let self else { return }
                do {
                    let handle = try await self.ensureHandle()
                    try await handle.submit(command: .refreshListings(which: [.mcp]))
                } catch {
                    self.fail(.host, "\(error)")
                }
            }
        }

        // MARK: session agents

        /// Pull the roster for the currently active session. Agent state is
        /// deliberately separate from the main turn stream so opening the
        /// picker never starts or interrupts a turn.
        func listSessionAgents() {
            guard !model.activeSessionId.isEmpty else { return }
            let requestSessionID = model.activeSessionId
            Task { [weak self] in
                guard let self else { return }
                guard self.model.activeSessionId == requestSessionID,
                      !self.model.sessionTransitionPending else { return }
                do {
                    let handle = try await self.ensureHandle()
                    guard self.model.activeSessionId == requestSessionID,
                          !self.model.sessionTransitionPending else { return }
                    try await handle.submit(command: .listSessionAgents)
                } catch {
                    // Listing failures must not overwrite a transcript load
                    // error (or a newer request). The next picker open retries.
                }
            }
        }

        /// Select a root/child agent. Child selection is read-only; the
        /// transcript is loaded lazily the first time it is requested.
        func selectAgent(_ id: String) {
            guard id == ConversationModel.mainAgentID
                || model.agentSummaries.contains(where: { $0.id == id })
            else { return }
            model.markAgentTranscriptLoading(id, sessionID: model.activeSessionId)
            guard id != ConversationModel.mainAgentID else { return }
            if model.agentTranscripts[id]?.loaded == true { return }
            loadSessionAgentTranscript(id)
        }

        func loadSessionAgentTranscript(_ id: String) {
            guard id != ConversationModel.mainAgentID, !id.isEmpty,
                  !model.activeSessionId.isEmpty
            else { return }
            let requestSessionID = model.activeSessionId
            let requestKey = model.markAgentTranscriptLoading(id, sessionID: requestSessionID)
            Task { [weak self] in
                guard let self else { return }
                guard self.model.activeSessionId == requestSessionID,
                      self.model.selectedAgentID == id else { return }
                do {
                    let handle = try await self.ensureHandle()
                    guard self.model.activeSessionId == requestSessionID,
                          self.model.selectedAgentID == id else { return }
                    try await handle.submit(command: .loadSessionAgentTranscript(agentId: id))
                } catch {
                    self.model.failAgentTranscript(
                        id,
                        sessionID: requestSessionID,
                        message: String(describing: error),
                        requestKey: requestKey
                    )
                }
            }
        }

        // MARK: inbound-event application (called on the main actor)

        private static func agentSummary(from dto: SessionAgentSummaryDto) -> ConversationAgentSummary {
            let summary = ConversationAgentSummary(
                id: dto.agentId,
                name: dto.agentId == ConversationModel.mainAgentID
                    ? ConversationAgentSummary.main.name
                    : dto.name,
                agentType: dto.agentType,
                status: dto.status,
                latestActivity: dto.agentId == ConversationModel.mainAgentID
                    ? nil
                    : dto.latestActivity,
                updatedAtMs: dto.updatedAtMs ?? 0
            )
            return ConversationModel.normalizedAgentSummaryForSource(summary)
        }

        private func applyAgentTranscript(
            sessionID: String,
            agentId: String,
            messages: [MessageDto],
            nextMessageIndex: UInt64? = nil,
            revision: UInt64 = 0
        ) {
            guard agentId != ConversationModel.mainAgentID else { return }
            guard sessionID == model.activeSessionId else { return }
            let watermark = nextMessageIndex ?? UInt64(messages.count)
            if let current = model.agentTranscripts[agentId],
               current.loaded,
               revision <= current.revision || watermark < current.nextMessageIndex {
                // A reply from an older load request arrived after a newer
                // snapshot. The engine revision counts raw transcript records,
                // including hidden/lifecycle entries, so it remains monotonic
                // even when two snapshots expose the same visible message
                // count. The visible watermark is a second independent fence:
                // live indexed rows advance it without advancing the raw
                // revision, so a delayed full reply must never move it back.
                // Equality remains allowed for a higher-revision compaction
                // update whose renderable details changed in place.
                return
            }
            let restored = Self.restoredTranscript(
                from: messages,
                sessionID: sessionID,
                identityPrefix: "\(sessionID)|agent|\(agentId)",
                occurrenceOffsets: [:],
                wireIndices: messages.indices.map { Optional(UInt64($0)) }
            )
            var transcript = ConversationAgentTranscript(
                messages: restored.messages,
                items: restored.items,
                details: restored.details,
                wireSignatures: restored.wireSignatures,
                wireIndices: restored.wireIndices,
                nextMessageIndex: watermark,
                revision: revision,
                loaded: true
            )
            let pending = model.takePendingAgentMessages(agentID: agentId, sessionID: sessionID)
                .sorted { lhs, rhs in
                    switch (lhs.index, rhs.index) {
                    case let (left?, right?): return left < right
                    case (.some, .none): return true
                    default: return false
                    }
                }
            var seenIndices = Set<UInt64>()
            for pendingMessage in pending {
                if let index = pendingMessage.index {
                    guard index >= watermark, seenIndices.insert(index).inserted else { continue }
                }
                let one = Self.restoredTranscript(
                    from: [pendingMessage.message],
                    sessionID: sessionID,
                    identityPrefix: "\(sessionID)|agent|\(agentId)",
                    occurrenceOffsets: Self.signatureCounts(transcript.wireSignatures),
                    wireIndices: [pendingMessage.index],
                    finalizeOrphans: false
                )
                transcript = appendTranscript(one, to: transcript)
                if let index = pendingMessage.index {
                    transcript.nextMessageIndex = max(transcript.nextMessageIndex, index + 1)
                }
            }
            model.setAgentTranscript(
                agentId,
                transcript: transcript,
                sessionID: sessionID
            )
        }

        /// A transcript reply and the live tail reader share one event stream,
        /// but a reply can still overtake a message that was emitted just before
        /// it. Merge the two projections instead of replacing the live cache.
        /// Message ids are deterministic hashes of the canonical wire message
        /// plus its occurrence, so repeated identical messages remain distinct;
        /// render item ids and details then retain structured tool rows without
        /// duplicating them.
        private func mergeAgentTranscript(
            _ restored: EngineConversationSource.RestoredTranscript,
            with current: ConversationAgentTranscript?,
            loaded: Bool
        ) -> ConversationAgentTranscript {
            guard let current, !current.messages.isEmpty || !current.items.isEmpty else {
                return ConversationAgentTranscript(
                    messages: restored.messages,
                    items: restored.items,
                    details: restored.details,
                    wireSignatures: restored.wireSignatures,
                    wireIndices: restored.wireIndices,
                    nextMessageIndex: restored.wireIndices.compactMap { $0 }.max().map { $0 + 1 }
                        ?? UInt64(restored.wireSignatures.count),
                    revision: 0,
                    loaded: loaded
                )
            }

            var restoredIDs = Set(restored.messages.map(\.id))
            var messages = restored.messages
            for message in current.messages {
                if restoredIDs.insert(message.id).inserted {
                    messages.append(message)
                }
            }

            var itemIDs = Set(restored.items.map(\.id))
            var messageItemIDs = Set(
                restored.items.compactMap { item -> UUID? in
                    guard case let .message(message) = item else { return nil }
                    return message.id
                }
            )
            var items = restored.items
            for item in current.items {
                if case let .message(message) = item {
                    guard messageItemIDs.insert(message.id).inserted else { continue }
                } else if case let .run(currentRun) = item,
                          let restoredIndex = items.firstIndex(where: { item in
                              guard case let .run(restoredRun) = item else { return false }
                              return restoredRun.tools.contains { restoredTool in
                                  currentRun.tools.contains { $0.id == restoredTool.id }
                              }
                          }),
                          case let .run(restoredRun) = items[restoredIndex] {
                    var merged = restoredRun
                    merged.tools = mergeToolTraces(restoredRun.tools, currentRun.tools)
                    merged.reasoning = currentRun.reasoning.isEmpty ? restoredRun.reasoning : currentRun.reasoning
                    merged.activities = mergeActivities(restoredRun.activities, currentRun.activities)
                    merged.status = merged.tools.contains(where: { $0.status == .running })
                        ? currentRun.status
                        : .restored
                    items[restoredIndex] = .run(merged)
                    continue
                } else {
                    guard itemIDs.insert(item.id).inserted else { continue }
                }
                items.append(item)
            }
            var details = restored.details
            details.merge(current.details) { _, new in new }
            return ConversationAgentTranscript(
                messages: messages,
                items: items,
                details: details,
                wireSignatures: mergeWireSignatures(
                    restored.wireSignatures,
                    current.wireSignatures
                ),
                wireIndices: restored.wireIndices + current.wireIndices,
                nextMessageIndex: max(
                    restored.wireIndices.compactMap { $0 }.max().map { $0 + 1 }
                        ?? UInt64(restored.wireSignatures.count),
                    current.nextMessageIndex
                ),
                revision: current.revision,
                loaded: loaded || current.loaded
            )
        }

        private func mergeToolTraces(
            _ base: [ConversationToolTrace],
            _ update: [ConversationToolTrace]
        ) -> [ConversationToolTrace] {
            var result = base
            for trace in update {
                if let index = result.firstIndex(where: { $0.id == trace.id }) {
                    var merged = result[index]
                    merged.tool = trace.tool.isEmpty ? merged.tool : trace.tool
                    merged.status = trace.status
                    merged.inputSummary = trace.inputSummary ?? merged.inputSummary
                    merged.outputSummary = trace.outputSummary ?? merged.outputSummary
                    merged.elapsedMs = trace.elapsedMs ?? merged.elapsedMs
                    merged.header = trace.header ?? merged.header
                    merged.display = trace.display ?? merged.display
                    result[index] = merged
                } else {
                    result.append(trace)
                }
            }
            return result
        }

        private func mergeActivities(
            _ base: [ConversationExecutionActivity],
            _ update: [ConversationExecutionActivity]
        ) -> [ConversationExecutionActivity] {
            var result = base
            var ids = Set(base.map(\.id))
            for activity in update where ids.insert(activity.id).inserted {
                result.append(activity)
            }
            return result
        }

        private func appendTranscript(
            _ incoming: EngineConversationSource.RestoredTranscript,
            to current: ConversationAgentTranscript
        ) -> ConversationAgentTranscript {
            var messages = current.messages
            var messageIDs = Set(messages.map(\.id))
            for message in incoming.messages where messageIDs.insert(message.id).inserted {
                messages.append(message)
            }

            var items = current.items
            var itemIDs = Set(items.map(\.id))
            for item in incoming.items {
                if case let .run(incomingRun) = item,
                   let index = items.firstIndex(where: { item in
                       guard case let .run(existingRun) = item else { return false }
                       return existingRun.tools.contains { existingTool in
                           incomingRun.tools.contains { $0.id == existingTool.id }
                       }
                   }),
                   case let .run(existingRun) = items[index] {
                    var merged = existingRun
                    merged.tools = mergeToolTraces(existingRun.tools, incomingRun.tools)
                    merged.reasoning = incomingRun.reasoning.isEmpty ? existingRun.reasoning : incomingRun.reasoning
                    merged.activities = mergeActivities(existingRun.activities, incomingRun.activities)
                    merged.status = incomingRun.status
                    items[index] = .run(merged)
                } else if itemIDs.insert(item.id).inserted {
                    items.append(item)
                }
            }
            var details = current.details
            details.merge(incoming.details) { _, new in new }
            return ConversationAgentTranscript(
                messages: messages,
                items: items,
                details: details,
                wireSignatures: mergeWireSignatures(
                    current.wireSignatures,
                    incoming.wireSignatures
                ),
                wireIndices: current.wireIndices + incoming.wireIndices,
                nextMessageIndex: max(
                    current.nextMessageIndex,
                    incoming.wireIndices.compactMap { $0 }.max().map { $0 + 1 }
                        ?? current.nextMessageIndex
                ),
                revision: current.revision,
                loaded: current.loaded
            )
        }

        private func mergeWireSignatures(
            _ first: [String],
            _ second: [String]
        ) -> [String] {
            let firstCounts = Self.signatureCounts(first)
            let secondCounts = Self.signatureCounts(second)
            var result = first
            var consumed = firstCounts
            for signature in second {
                let target = max(
                    firstCounts[signature, default: 0],
                    secondCounts[signature, default: 0]
                )
                guard consumed[signature, default: 0] < target else { continue }
                result.append(signature)
                consumed[signature, default: 0] += 1
            }
            return result
        }

        private func appendAgentMessage(
            agentId: String,
            message: MessageDto,
            messageIndex: UInt64? = nil
        ) {
            guard agentId != ConversationModel.mainAgentID else { return }
            let sessionID = model.activeSessionId
            guard !sessionID.isEmpty else { return }
            guard model.shouldAcceptAgentMessage(
                agentID: agentId,
                sessionID: sessionID,
                index: messageIndex
            ) else { return }
            let previous = model.agentTranscripts[agentId]
            _ = model.enqueuePendingAgentMessage(
                message,
                index: messageIndex,
                agentID: agentId,
                sessionID: sessionID
            )
            let one = Self.restoredTranscript(
                from: [message],
                sessionID: sessionID,
                identityPrefix: "\(sessionID)|agent|\(agentId)",
                occurrenceOffsets: Self.signatureCounts(previous?.wireSignatures ?? []),
                wireIndices: [messageIndex],
                finalizeOrphans: false
            )
            var current = previous ?? ConversationAgentTranscript()
            // MessageDto has no wire id. Keep every occurrence here; collapsing
            // by role/text would lose two legitimate adjacent messages that say
            // the same thing. Full transcript replies are merged by deterministic
            // occurrence IDs, while a live event is necessarily treated as one
            // new occurrence.
            current = appendTranscript(one, to: current)
            if let messageIndex {
                current.nextMessageIndex = max(current.nextMessageIndex, messageIndex + 1)
            }
            model.setAgentTranscript(agentId, transcript: current, sessionID: sessionID)
            let activity = one.messages.last?.text
                .split(whereSeparator: { $0 == "\n" || $0 == "\r" })
                .first
                .map(String.init)
            let summary = model.agentSummaries.first(where: { $0.id == agentId })
                ?? ConversationAgentSummary(
                    id: agentId,
                    name: agentId,
                    agentType: "unknown",
                    status: "working"
                )
            let terminalStatuses = Set(["completed", "failed", "cancelled", "killed"])
            let currentStatus = summary.status.lowercased()
            let status = terminalStatuses.contains(currentStatus) ? summary.status : "working"
            model.upsertAgentSummary(ConversationAgentSummary(
                id: summary.id,
                name: summary.name,
                agentType: summary.agentType,
                status: status,
                latestActivity: activity ?? summary.latestActivity,
                updatedAtMs: UInt64(Date().timeIntervalSince1970 * 1_000)
            ))
        }

        private func refreshSessionAgentsAfterTransition() {
            guard let handle else { return }
            Task { [handle] in
                try? await handle.submit(command: .listSessionAgents)
            }
        }

        /// Map one inbound `ClientEvent` onto the published state.
        fileprivate func apply(_ event: ClientEvent) {
            externalEventHandler?(event)
            switch event {
            case .turnStarted:
                guard acceptTurnEvent(event) else { return }
                model.slashCommandPending = false
                pendingSlashRaw = nil
                model.streaming = true
                model.updateMainAgent(status: "working", latestActivity: String(localized: "chat_working"))
                model.notice = nil
                streamingIndex = nil
                streamingItemIndex = nil
                updateActiveRun {
                    $0.status = .running
                    $0.retry = nil
                }

            case let .slashCommandResult(turnId, display, isError):
                guard let token = activeConversationTurnToken,
                      turnId == nil || turnId == token.clientTurnId
                else { return }
                let raw = pendingSlashRaw ?? "/"
                model.items.append(.commandOutput(ConversationCommandOutput(
                    id: "slash-\(token.sessionEpoch)-\(token.clientTurnId)",
                    command: raw,
                    text: display,
                    isError: isError
                )))
                model.streaming = false
                model.slashCommandPending = false
                model.statusLine = nil
                model.updateMainAgent(
                    status: isError ? "failed" : "idle",
                    latestActivity: isError ? display : String(localized: "chat_completed")
                )
                publishActiveTurnCompletion(isError ? .failed : .completed)
                clearTurnPointers(keepEpoch: false)
                requestSessionCatalogRefreshAfterSettledTurn()

            case let .textDelta(text):
                guard acceptTurnEvent(event) else { return }
                appendTextBoundaryActivity()
                appendDelta(text)
                let activity = text
                    .split(whereSeparator: { $0 == "\n" || $0 == "\r" })
                    .first
                    .map(String.init)
                model.updateMainAgent(status: "working", latestActivity: activity)
                publishTurnSpeechDelta(text)

            case let .thinkingDelta(thinking, signature):
                guard acceptTurnEvent(event) else { return }
                appendReasoningActivity(thinking)
                if signature != nil {
                    var addedSignature = false
                    updateActiveRun { run in
                        if run.notices.contains(where: { $0.id == "thinking-signature" }) == false {
                            run.notices.append(.init(id: "thinking-signature", kind: .info, text: String(localized: "chat_thinking_signature")))
                            addedSignature = true
                        }
                    }
                    if addedSignature { appendNoticeActivity(id: "thinking-signature") }
                }

            case let .systemNotice(message, isError):
                guard acceptTurnEvent(event) else { return }
                let notice = ConversationExecutionNotice(
                    id: "notice-\(UUID().uuidString)",
                    kind: isError ? .error : .info,
                    text: message
                )
                updateActiveRun { $0.notices.append(notice) }
                appendNoticeActivity(id: notice.id)
                model.statusLine = message

            case let .askUserQuestion(request):
                // Passes the turn gate via the allowlist in `acceptTurnEvent`:
                // a broker replay after a foreground re-connect arrives
                // OUTSIDE a turn. Dedupe by request id — a replay of a
                // still-pending request must not stack a second card.
                guard acceptTurnEvent(event) else { return }
                let question = Self.pendingQuestion(from: request)
                guard !model.pendingQuestions.contains(where: { $0.requestId == question.requestId }) else {
                    return
                }
                model.pendingQuestions.append(question)

            case let .askUserQuestionResolved(requestId):
                // Allowlisted like `.askUserQuestion` above — resolution can
                // also be replayed outside a turn. Drop the card whether the
                // request was answered here, elsewhere, or auto-continued.
                guard acceptTurnEvent(event) else { return }
                model.pendingQuestions.removeAll { $0.requestId == requestId }

            case let .taskRow(task):
                // A `TaskList` reply row (out-of-band, allowlisted). Rows
                // both seed the panel at bootstrap and backfill the
                // description for ids first seen via a status push.
                guard acceptTurnEvent(event) else { return }
                if let mapped = Self.backgroundTaskStatus(task.status) {
                    upsertBackgroundTask(
                        id: task.taskId,
                        description: task.description,
                        status: mapped
                    )
                }

            case let .planUpdated(tasks):
                // The model's working plan. FULL-LIST replace — an empty list
                // is the engine clearing it, never "no news". Allowlisted
                // through the turn gate like `taskRow` above: the plan block
                // outlives the turn that wrote it, and a dropped update would
                // strand a stale checklist above the composer.
                guard acceptTurnEvent(event) else { return }
                model.planTasks = tasks.map(Self.planTask(from:))

            case let .taskStatusChanged(taskId, status):
                // Allowlisted through the turn gate: a background task
                // normally finishes after its spawning turn already ended.
                guard acceptTurnEvent(event) else { return }
                if let mapped = Self.backgroundTaskStatus(status) {
                    let known = model.backgroundTasks.contains { $0.id == taskId }
                    upsertBackgroundTask(id: taskId, description: nil, status: mapped)
                    if !known {
                        // First sighting via a push — pull the row list so
                        // the panel can show the human description instead
                        // of the bare id.
                        refreshBackgroundTasks()
                    }
                }
                let text: String?
                switch status {
                case .completed: text = String(localized: "chat_task_completed \(taskId)")
                case .failed: text = String(localized: "chat_task_failed \(taskId)")
                case .cancelled: text = String(localized: "chat_task_cancelled \(taskId)")
                case .pending, .running: text = nil
                @unknown default: text = nil
                }
                if let text {
                    model.items.append(.notice(ConversationExecutionNotice(
                        id: "task-\(taskId)-\(UUID().uuidString)",
                        kind: status == .failed ? .error : .info,
                        text: text
                    )))
                }

            case let .toolUseStarted(id, tool, inputJson, header):
                let accepted = acceptTurnEvent(event)
                Self.turnLog.debug(
                    "tool started id=\(id, privacy: .public) name=\(tool, privacy: .public) turn=\(self.currentTurnId ?? 0, privacy: .public) accepted=\(accepted, privacy: .public)"
                )
                guard accepted else { return }
                // The engine derived the header once; we render THAT. The old
                // client-side summarizer below survives only as the fallback for
                // an engine too old to send one.
                let derivedHeader = header.map(Self.toolHeader(from:))
                appendToolActivity(id: id)
                if ConversationExecutionParsing.isShellTool(tool) {
                    let started = ConversationExecutionParsing.shellStarted(id: id, inputJson: inputJson)
                    upsertShellCard(id: id, create: {
                        ConversationShellCard(
                            sessionId: model.activeSessionId,
                            turnId: currentTurnId,
                            taskId: started.taskId,
                            command: started.command,
                            cwd: started.cwd
                        )
                    }, mutate: { card in
                        card.command = started.command
                        card.cwd = started.cwd
                        card.status = .running
                    })
                    upsertTool(id: id, tool: "Shell", fallbackSummary: started.command) { trace in
                        trace.tool = "Shell"
                        trace.status = .running
                        trace.inputSummary = started.command
                        trace.header = derivedHeader
                    }
                    model.statusLine = String(localized: "chat_shell_running")
                    model.updateMainAgent(status: "working", latestActivity: model.statusLine)
                } else {
                    let summary = ConversationExecutionParsing.summarizeToolInput(inputJson)
                    upsertTool(id: id, tool: tool, fallbackSummary: summary) { trace in
                        trace.tool = tool
                        trace.status = .running
                        trace.inputSummary = summary
                        trace.header = derivedHeader
                    }
                    model.statusLine = String(localized: "chat_tool_calling \(tool)")
                    model.updateMainAgent(status: "working", latestActivity: model.statusLine)
                }

            case let .toolHeartbeat(id, tool, elapsedMs):
                let accepted = acceptTurnEvent(event)
                Self.turnLog.debug(
                    "tool heartbeat id=\(id, privacy: .public) name=\(tool, privacy: .public) elapsed_ms=\(elapsedMs, privacy: .public) turn=\(self.currentTurnId ?? 0, privacy: .public) accepted=\(accepted, privacy: .public) cancelling=\(self.model.isCancelling, privacy: .public)"
                )
                guard accepted else { return }
                if ConversationExecutionParsing.isShellTool(tool) {
                    upsertShellCard(id: id, create: {
                        ConversationShellCard(
                            sessionId: model.activeSessionId,
                            turnId: currentTurnId,
                            taskId: id,
                            command: tool
                        )
                    }, mutate: { card in
                        card.durationMs = elapsedMs
                    })
                    upsertTool(id: id, tool: "Shell", fallbackSummary: nil) { trace in
                        trace.tool = "Shell"
                        trace.status = .running
                        trace.elapsedMs = elapsedMs
                    }
                    model.statusLine = String(localized: "chat_shell_running")
                    model.updateMainAgent(status: "working", latestActivity: model.statusLine)
                } else {
                    upsertTool(id: id, tool: tool, fallbackSummary: nil) { trace in
                        trace.tool = tool
                        trace.status = .running
                        trace.elapsedMs = elapsedMs
                    }
                    model.statusLine = String(localized: "chat_tool_running \(tool)")
                    model.updateMainAgent(status: "working", latestActivity: model.statusLine)
                }

            case let .toolUseResult(id, tool, resultJson, isError, display):
                let accepted = acceptTurnEvent(event)
                Self.turnLog.debug(
                    "tool result id=\(id, privacy: .public) name=\(tool, privacy: .public) is_error=\(isError, privacy: .public) turn=\(self.currentTurnId ?? 0, privacy: .public) accepted=\(accepted, privacy: .public)"
                )
                guard accepted else { return }
                // The pre-derived `⎿` block (headline / diff / clamped body).
                // `summarizeToolResult` below stays only as the older-engine path.
                let derivedDisplay = display.map(Self.resultDisplay(from:))
                let wasCancelled = ConversationExecutionParsing.isCancellationResult(resultJson)
                if ConversationExecutionParsing.isShellTool(tool) {
                    let finished = ConversationExecutionParsing.shellFinished(id: id, resultJson: resultJson, isError: isError)
                    let resolvedShellStatus: ConversationShellStatus = wasCancelled
                        ? .cancelled
                        : (finished?.status ?? (isError ? .failed : .completed))
                    upsertShellCard(id: id, create: {
                        ConversationShellCard(
                            sessionId: model.activeSessionId,
                            turnId: currentTurnId,
                            taskId: id,
                            command: tool
                        )
                    }, mutate: { card in
                        if let finished {
                            card.stdout = finished.stdout
                            card.stderr = finished.stderr
                            card.exitCode = finished.exitCode
                            card.durationMs = finished.durationMs ?? card.durationMs
                            card.status = resolvedShellStatus
                            card.truncated = finished.truncated
                        } else {
                            card.status = resolvedShellStatus
                        }
                    })
                    upsertTool(id: id, tool: "Shell", fallbackSummary: nil) { trace in
                        trace.tool = "Shell"
                        trace.status = resolvedShellStatus.asToolStatus
                        trace.display = derivedDisplay
                        trace.outputSummary = ConversationExecutionParsing.summarizeToolResult(resultJson, isError: isError, tool: tool)
                        trace.elapsedMs = finished?.durationMs ?? trace.elapsedMs
                    }
                    model.statusLine = ConversationExecutionParsing.shellStatusLabel(resolvedShellStatus)
                    model.updateMainAgent(status: "working", latestActivity: model.statusLine)
                } else {
                    upsertTool(id: id, tool: tool, fallbackSummary: nil) { trace in
                        trace.tool = tool
                        trace.status = wasCancelled ? .cancelled : (isError ? .failed : .completed)
                        trace.display = derivedDisplay
                        trace.outputSummary = ConversationExecutionParsing.summarizeToolResult(
                            resultJson,
                            isError: isError,
                            tool: tool
                        )
                    }
                    model.statusLine = wasCancelled
                        ? String(localized: "chat_tool_cancelled \(tool)")
                        : (isError ? String(localized: "chat_tool_failed \(tool)") : String(localized: "chat_tool_completed \(tool)"))
                    model.updateMainAgent(status: "working", latestActivity: model.statusLine)
                }

            case let .usageUpdate(inputTokens, outputTokens, cacheReadTokens, cacheCreationTokens):
                guard acceptTurnEvent(event) else { return }
                updateActiveRun {
                    $0.usage = ConversationUsageSnapshot(
                        inputTokens: inputTokens,
                        outputTokens: outputTokens,
                        cacheReadTokens: cacheReadTokens,
                        cacheCreationTokens: cacheCreationTokens
                    )
                }

            case let .apiRetry(message, attempt, maxRetries, delayMs):
                guard acceptTurnEvent(event) else { return }
                updateActiveRun {
                    $0.retry = ConversationRetrySnapshot(
                        message: message,
                        attempt: attempt,
                        maxRetries: maxRetries,
                        delayMs: delayMs
                    )
                }
                model.statusLine = String(localized: "chat_retrying \(attempt) \(maxRetries)")

            case let .costUpdate(_, _, _, _, _, formatted):
                guard acceptTurnEvent(event) else { return }
                updateActiveRun { $0.costFormatted = formatted }

            case let .compactionCompleted(messagesBefore, messagesAfter, bytesSaved):
                guard acceptTurnEvent(event) else { return }
                updateActiveRun {
                    $0.compactions.append(
                        ConversationCompactionSnapshot(
                            messagesBefore: messagesBefore,
                            messagesAfter: messagesAfter,
                            bytesSaved: bytesSaved
                        )
                    )
                }

            case let .coordinatorStatus(activeWorkers, team):
                guard acceptTurnEvent(event) else { return }
                updateCoordinatorStatus(activeWorkers: activeWorkers, team: team)

            case let .coordinatorWorker(worker):
                guard acceptTurnEvent(event) else { return }
                // Older engines expose coordinator workers before the
                // session-agent listing exists. Keep those rows selectable as
                // read-only agents so the picker degrades gracefully.
                model.upsertAgentSummary(ConversationAgentSummary(
                    id: worker.agentId,
                    name: worker.name,
                    agentType: worker.agentType,
                    status: worker.status,
                    latestActivity: model.statusLine
                ))
                updateActiveRun { run in
                    let viewModel = ConversationCoordinatorWorker(
                        id: worker.agentId,
                        name: worker.name,
                        agentType: worker.agentType,
                        status: worker.status
                    )
                    if let index = run.workers.firstIndex(where: { $0.id == viewModel.id }) {
                        run.workers[index] = viewModel
                    } else {
                        run.workers.append(viewModel)
                    }
                }

            case let .messageComplete(_, message):
                guard acceptTurnEvent(event) else { return }
                if let message {
                    appendTextBoundaryActivity()
                    let lowered = Self.message(from: message)
                    replaceStreamingMessage(lowered.message, detail: lowered.detail)
                }

            case let .turnEnded(outcome, _, _):
                let accepted = acceptTurnEvent(event)
                Self.turnLog.debug(
                    "turn ended turn=\(self.currentTurnId ?? 0, privacy: .public) outcome=\(String(describing: outcome), privacy: .public) accepted=\(accepted, privacy: .public)"
                )
                guard accepted else { return }
                // PR-4 item 3: don't treat every outcome as a clean end. A normal
                // `endTurn` just stops streaming; `maxTurns` / `cancelled` surface
                // a distinct notice so the user knows the turn was interrupted.
                model.streaming = false
                if !model.isCancelling { model.statusLine = nil }
                switch outcome {
                case .endTurn:
                    model.notice = nil
                    finishActiveRun(.completed)
                    publishActiveTurnCompletion(.completed)
                case .maxTurns:
                    model.notice = .maxTurns
                    finishActiveRun(.maxTurns)
                    publishActiveTurnCompletion(.maxTurns)
                case .cancelled:
                    model.notice = .cancelled
                    finishActiveRun(.cancelled)
                    publishActiveTurnCompletion(.cancelled)
                @unknown default:
                    // `#[non_exhaustive]` — a future outcome falls back to a clean
                    // visual end rather than crashing, but remains non-speakable
                    // until the client explicitly understands its semantics.
                    model.notice = nil
                    finishActiveRun(.completed)
                    publishActiveTurnCompletion(.failed)
                }
                model.updateMainAgent(
                    status: outcome == .cancelled ? "cancelled" : "idle",
                    latestActivity: model.notice?.text ?? String(localized: "chat_completed")
                )
                clearTurnPointers(keepEpoch: false)
                requestSessionCatalogRefreshAfterSettledTurn()

            case let .error(kind, message):
                let accepted = acceptTurnEvent(event)
                Self.turnLog.error(
                    "turn error turn=\(self.currentTurnId ?? 0, privacy: .public) kind=\(String(describing: kind), privacy: .public) accepted=\(accepted, privacy: .public) message=\(message, privacy: .private(mask: .hash))"
                )
                guard accepted else { return }
                // PR-4 item 4: a terminal error is a persistent, kind-aware banner.
                finishActiveRun(.failed)
                model.updateMainAgent(status: "failed", latestActivity: message)
                fail(Self.kind(from: kind), message)

            case let .modelList(models, current):
                // Out-of-band model catalog (SHIP-BLOCKER #2). Drive the picker off
                // these REAL engine ids and adopt the engine's reported active model
                // — not a branded mock default.
                model.availableModels = models
                applyActiveModel(current)

            case let .modelChanged(model: newModel):
                // The engine confirmed a switch (1:1 with a successful `SetModel`).
                applyActiveModel(newModel)

            case let .sessionList(sessions):
                // Out-of-band session catalog: map each lowered `SessionRowDto`
                // to the UI model (uuid/title/count + RFC 3339 → relative time).
                // Drives the drawer off REAL history; an empty list lets the
                // drawer fall back to the mock lists.
                model.engineSessions = sessions.map {
                    EngineSession(id: $0.uuid,
                                  title: $0.title,
                                  messageCount: Int($0.messageCount),
                                  relativeTime: RelativeTime.format($0.modifiedRfc3339))
                }
                // Publish the authoritative rows before flipping the loaded
                // bit. `ConversationProjectBridge` persists on the loaded
                // transition; reversing these assignments creates a crash
                // window where it can durably replace a valid index with an
                // intermediate empty value.
                model.engineSessionsLoaded = true

            case let .sessionAgentList(sessionId, agents):
                // Ignore a delayed listing from a previous session. The agent
                // picker is strictly scoped to the active session.
                guard sessionId == model.activeSessionId,
                      !model.sessionTransitionPending else { return }
                model.replaceAgentSummaries(agents.map { Self.agentSummary(from: $0) })

            case let .sessionAgentUpdated(sessionId, agent):
                guard sessionId == model.activeSessionId,
                      !model.sessionTransitionPending else { return }
                model.upsertAgentSummary(Self.agentSummary(from: agent))

            case let .sessionAgentTranscript(sessionId, agentId, messages, nextMessageIndex, revision):
                guard sessionId == model.activeSessionId,
                      !model.sessionTransitionPending else { return }
                applyAgentTranscript(
                    sessionID: sessionId,
                    agentId: agentId,
                    messages: messages,
                    nextMessageIndex: nextMessageIndex,
                    revision: revision
                )

            case let .sessionAgentMessage(sessionId, agentId, messageIndex, message):
                guard sessionId == model.activeSessionId,
                      !model.sessionTransitionPending else { return }
                appendAgentMessage(
                    agentId: agentId,
                    message: message,
                    messageIndex: messageIndex
                )

            case let .sessionStarted(sessionId):
                // A fresh session began on the connection (1:1 with a successful
                // `NewSession`). Adopt it as active. Reset the transcript ONLY
                // when this is genuinely a new id AND no turn is streaming — so an
                // unexpected `SessionStarted` (e.g. one emitted for the initial
                // session at connect time) can never wipe a live conversation.
                // `startNewConversation` already reset locally for the user-driven
                // case; this confirms + adopts the engine-assigned id.
                let isSwitch = !sessionId.isEmpty && sessionId != model.activeSessionId
                let confirmedNewSession = pendingSessionTransition == .new
                model.activeSessionId = sessionId
                model.clearAgentState()
                if case let .resume(targetID) = pendingSessionTransition,
                   targetID != sessionId {
                    // A bootstrap SessionStarted can be delivered after the host
                    // has already requested ResumeSession. It is not confirmation
                    // of that restore and must not unlock catalog replacement.
                } else {
                    setPendingSessionTransition(nil)
                }
                if isSwitch && !model.streaming {
                    resetTranscriptForSessionSwitch(isNew: true)
                    model.activeSessionId = sessionId
                }
                if confirmedNewSession {
                    model.sessionRefreshRevision &+= 1
                }
                refreshSessionAgentsAfterTransition()

            case let .sessionResumed(sessionId, messages):
                // A prior session was resumed (1:1 with a successful
                // `ResumeSession`). Adopt it as active AND surface the restored
                // transcript the engine just hot-loaded into the running
                // orchestrator, so the scrollback shows the prior conversation
                // and the user can see exactly the context the next turn will
                // continue from. `messages` is OLDEST-FIRST and always present
                // (may be empty for a zero-message session). We clear the
                // placeholder transcript `resumeSession` left in place and append
                // each restored message as a completed bubble — the out-of-band
                // session-state sibling of `SessionList` / `SessionStarted`.
                if let pendingSessionTransition {
                    guard case let .resume(targetID) = pendingSessionTransition,
                          targetID == sessionId else { return }
                }
                invalidateTurnContext()
                model.clearAgentState()
                model.activeSessionId = sessionId
                setPendingSessionTransition(nil)
                // BUG FIX: a restored message is NOT one render row. In the
                // Anthropic protocol a `tool_result` block lives in the USER
                // turn, so mapping each `MessageDto` to a single `.message`
                // rendered every restored tool result as a right-aligned user
                // bubble — and `MessageBubble`'s user branch draws only
                // `message.text`, never `detail`, so the payload was invisible
                // too. `restoredTranscript` interleaves instead: narrative
                // blocks stay a `.message`, each tool call becomes its own
                // `.toolCall` row with its result merged onto it.
                let restored = Self.restoredTranscript(
                    from: messages,
                    sessionID: sessionId
                )
                model.messages = restored.messages
                model.items = restored.items
                model.messageDetails = restored.details
                model.expandedToolCalls = []
                model.planTasks = []
                model.isNew = messages.isEmpty
                model.streaming = false
                model.turnCompletion = nil
                model.statusLine = nil
                model.notice = nil
                // A migration-safe empty resume may have just created its JSONL
                // anchor. Refresh so the full catalog and persisted project index
                // immediately include that preserved UUID.
                model.sessionRefreshRevision &+= 1
                refreshSessionAgentsAfterTransition()

            case .sessionEnded:
                // The current session ended (e.g. cleared). Drop the active id;
                // the next `SessionStarted`/`SessionResumed` re-establishes one.
                model.activeSessionId = ""
                model.clearAgentState()
                setPendingSessionTransition(nil)
                invalidateTurnContext()
                model.turnCompletion = nil
                // A pending questionnaire belongs to the ended session; its
                // request id can never be answered now.
                model.pendingQuestions = []
                // The plan belonged to that session too.
                model.planTasks = []

            case let .mcpServers(servers):
                // Out-of-band MCP listing → the UI `MCPServer` model. The wire
                // intentionally carries health/name/transport only; endpoint
                // and command details are merged from the real config file by
                // the settings host, never fabricated here. `local_apps` is an
                // engine-owned provider, not a user-editable MCP configuration.
                model.mcpServers = servers.filter { $0.name != "local_apps" }.map { dto in
                    let status: ConnStatus
                    switch dto.status {
                    case .connected: status = .connected
                    case .disconnected: status = .idle
                    case .error: status = .error
                    @unknown default: status = .idle
                    }
                    return MCPServer(id: dto.name, name: dto.name, url: nil, command: "",
                                     args: [], env: [:], headers: [:], tools: nil,
                                     status: status, enabled: status == .connected,
                                     transport: MCPServer.normalizedTransport(dto.transport))
                }
                model.mcpServersLoaded = true

            case let .slashCommandCatalog(commands), let .commandsChanged(commands):
                applySlashCommandCatalog(commands)

            default:
                // Cost / message-boundary / other listing events are not rendered
                // in this surface; ignored without breaking the stream.
                break
            }
        }

        /// Test seam (the iOS analog of Android's `reduceSessionEvent`): drive one
        /// inbound `ClientEvent` through the same `apply` reducer the live listener
        /// uses, so a unit test can assert the out-of-band session-state mapping
        /// (notably `SessionResumed` → restored transcript) without standing up the
        /// engine. `internal` so `@testable import LingxiCode` reaches it; the live
        /// path still goes through `apply` directly.
        func applyForTesting(_ event: ClientEvent) {
            apply(event)
        }

        private func applySlashCommandCatalog(_ commands: [SlashCommandDto]) {
            model.slashCommands = commands.map { command in
                ConversationSlashCommand(
                    name: command.name,
                    description: command.description,
                    aliases: command.aliases,
                    argumentHint: command.argumentHint,
                    menuDescription: command.menuDescription,
                    source: command.source,
                    hidden: command.hidden
                )
            }
            model.slashCommandsLoaded = true

            // Settings derives its skill rows from the same catalog rather
            // than maintaining another command list.
            model.skills = commands.map { command in
                let source = command.source
                let author: String
                switch source {
                case "builtin", "bundled": author = String(localized: "skills_author_official")
                case "user": author = String(localized: "skills_author_mine")
                default: author = command.source
                }
                return Skill(
                    id: "skill:\(command.name)",
                    name: command.name,
                    author: author,
                    desc: command.description,
                    triggers: ["/\(command.name)"] + command.aliases.map { "/\($0)" },
                    enabled: true,
                    builtin: source == "builtin" || source == "bundled" || source == "managed",
                    source: source
                )
            }
            model.skillsLoaded = true
        }

        func beginTurnForTesting(turnId: UInt64 = 1, sessionId: String = "test-session") {
            model.activeSessionId = sessionId
            model.streaming = true
            model.turnCompletion = nil
            turnSpeechSequence = 0
            currentTurnId = turnId
            activeTurnEpoch = sessionEpoch
            nextTurnId = max(nextTurnId, turnId &+ 1)
        }

        func cancelForTesting() {
            guard model.streaming, currentTurnId != nil else { return }
            model.isCancelling = true
            model.statusLine = String(localized: "chat_stopping")
            model.pendingPermissions = []
        }

        func setCommandSubmitterForTesting(_ submitter: ((ClientCommand) async throws -> Void)?) {
            testCommandSubmitter = submitter
        }

        func setEmptySessionResumerForTesting(
            _ resumer: ((String, String) async throws -> Void)?
        ) {
            testEmptySessionResumer = resumer
        }

        /// Append streamed text into the in-flight assistant message, creating it
        /// on the first delta of a turn.
        ///
        /// PR-4 item 1 guard: only append when a turn is actually in flight. After
        /// a cancel/turn-end we drop `streaming`/`streamingIndex`, so a late delta
        /// arriving from the engine must NOT resurrect or corrupt a message.
        private func appendDelta(_ delta: String) {
            guard model.streaming else { return }
            if let i = streamingIndex, model.messages.indices.contains(i) {
                let updated = Message(role: .ai,
                                      tag: model.messages[i].tag,
                                      text: model.messages[i].text + delta)
                replaceStreamingMessage(updated)
            } else {
                let opened = Message(role: .ai, text: delta)
                appendMessage(opened)
                streamingIndex = model.messages.count - 1
                streamingItemIndex = model.items.count - 1
            }
        }

        private func publishTurnSpeechDelta(_ delta: String) {
            guard !delta.isEmpty, let token = activeConversationTurnToken else { return }
            turnSpeechSequence &+= 1
            model.turnSpeechUpdates.send(ConversationTurnSpeechUpdate(
                token: token,
                sequence: turnSpeechSequence,
                delta: delta
            ))
        }

        // MARK: restored-transcript lowering (live ResumeSession)

        /// Map one restored `MessageDto` (the engine's lowered transcript line,
        /// `SessionResumed.messages`) onto the UI `Message` the scrollback renders.
        ///
        /// The engine roles are `"user" | "assistant" | "system"`; the iOS
        /// `Message.role` is the binary user/AI split, so non-user roles render on
        /// the AI side. `Message.text` remains a compatibility summary, while the
        /// complete typed block list is retained in `ConversationMessageDetail`
        /// so restored reasoning, tool and compaction blocks keep their identity.
        fileprivate struct RenderedMessage {
            let message: Message
            let detail: ConversationMessageDetail?
        }

        // MARK: engine-derived tool presentation (lowering)
        //
        // Straight field-for-field lowering of the additive wire DTOs onto the
        // FFI-independent mirrors the views render. NOTHING here re-parses
        // `input_json` / `result_json`: the engine already derived all of it,
        // and a second derivation on this side is exactly the drift the wire
        // fields exist to delete.

        fileprivate static func toolHeader(from dto: ToolHeaderDto) -> ConversationToolHeader {
            ConversationToolHeader(
                verb: toolVerb(from: dto.verb),
                icon: dto.icon.map(toolIcon(from:)),
                label: dto.label,
                primary: dto.primary,
                qualifier: dto.qualifier,
                count: dto.count,
                subLine: dto.subLine.map {
                    ConversationToolSubLine(prefix: $0.prefix, text: $0.text)
                },
                title: dto.title
            )
        }

        private static func toolVerb(from dto: ToolVerbDto) -> ConversationToolVerb {
            switch dto {
            case .update: return .update
            case .create: return .create
            case .read: return .read
            case .search: return .search
            case .shell: return .shell
            case .output: return .output
            case .kill: return .kill
            case .fetch: return .fetch
            case .task: return .task
            case .todo: return .todo
            case .skill: return .skill
            case .generic: return .generic
            @unknown default:
                // `#[non_exhaustive]`: a verb this build does not know still
                // renders — as the engine's English label, via `.generic`.
                return .generic
            }
        }

        private static func toolIcon(from dto: ToolIconDto) -> ConversationToolIcon {
            switch dto {
            case .read: return .read
            case .search: return .search
            case .list: return .list
            case .edit: return .edit
            case .terminal: return .terminal
            case .globe: return .globe
            case .workflow: return .workflow
            case .listChecks: return .listChecks
            case .sparkles: return .sparkles
            case .plug: return .plug
            case .output: return .output
            case .stop: return .stop
            case .wrench: return .wrench
            @unknown default: return .wrench
            }
        }

        fileprivate static func resultDisplay(
            from dto: ToolResultDisplayDto
        ) -> ConversationToolResultDisplay {
            ConversationToolResultDisplay(
                headline: dto.headline,
                headlineKind: dto.headlineKind.map(headlineKind(from:)),
                headlineArgs: dto.headlineArgs,
                diff: dto.diff.map(structuredDiff(from:)),
                body: dto.body,
                bodyLines: dto.bodyLines,
                bodyTruncated: dto.bodyTruncated,
                collapsed: dto.collapsed
            )
        }

        private static func headlineKind(
            from dto: HeadlineKindDto
        ) -> ConversationResultHeadlineKind {
            switch dto {
            case .added: return .added
            case .removed: return .removed
            case .addedRemoved: return .addedRemoved
            case .linesRead: return .linesRead
            case .linesReadPartial: return .linesReadPartial
            case .filesFound: return .filesFound
            case .filesFoundTruncated: return .filesFoundTruncated
            case .linesFound: return .linesFound
            case .matchesFound: return .matchesFound
            case .interrupted: return .interrupted
            case .noContent: return .noContent
            case .failed: return .failed
            case .plain: return .plain
            @unknown default:
                // An unknown kind falls back to the engine's own English
                // `headline` string, which `.plain` renders verbatim.
                return .plain
            }
        }

        private static func structuredDiff(
            from dto: StructuredDiffDto
        ) -> ConversationStructuredDiff {
            ConversationStructuredDiff(
                filePath: dto.filePath,
                language: dto.language,
                gutterWidth: dto.gutterWidth,
                additions: dto.additions,
                removals: dto.removals,
                truncatedRows: dto.truncatedRows,
                rows: dto.rows.map(diffRow(from:))
            )
        }

        private static func diffRow(from dto: DiffRowDto) -> ConversationDiffRow {
            ConversationDiffRow(
                kind: diffLineKind(from: dto.kind),
                lineNo: dto.lineNo,
                hunk: dto.hunk,
                wordDiffed: dto.wordDiffed,
                segments: dto.segments.map(codeSegment(from:))
            )
        }

        private static func diffLineKind(
            from dto: DiffLineKindDto
        ) -> ConversationDiffLineKind {
            switch dto {
            case .add: return .add
            case .remove: return .remove
            case .context: return .context
            @unknown default: return .context
            }
        }

        private static func codeSegment(from dto: CodeSegmentDto) -> ConversationCodeSegment {
            ConversationCodeSegment(
                text: dto.text,
                // `class` is a Swift keyword; the generated property is backticked.
                syntax: syntaxClass(from: dto.`class`),
                rgb: dto.rgb,
                bold: dto.bold,
                italic: dto.italic,
                underline: dto.underline,
                emph: dto.emph
            )
        }

        private static func syntaxClass(from dto: SyntaxClassDto) -> ConversationSyntaxClass {
            switch dto {
            case .plain: return .plain
            case .keyword: return .keyword
            case .typeName: return .typeName
            case .function: return .function
            case .stringLit: return .stringLit
            case .number: return .number
            case .comment: return .comment
            case .punctuation: return .punctuation
            // `operator` is a Swift keyword; the generated case is backticked.
            case .`operator`: return .op
            case .variable: return .variable
            case .constant: return .constant
            case .attribute: return .attribute
            @unknown default: return .plain
            }
        }

        fileprivate static func planTask(from dto: PlanTaskDto) -> ConversationPlanTask {
            ConversationPlanTask(
                taskId: dto.id,
                subject: dto.subject,
                activeForm: dto.activeForm,
                state: planTaskState(from: dto.state)
            )
        }

        private static func planTaskState(
            from dto: PlanTaskStateDto
        ) -> ConversationPlanTaskState {
            switch dto {
            case .pending: return .pending
            case .inProgress: return .inProgress
            case .completed: return .completed
            @unknown default: return .pending
            }
        }

        /// Lower one wire `AskUserQuestionRequestDto` onto the FFI-independent
        /// model the chat surface renders.
        fileprivate static func pendingQuestion(
            from dto: AskUserQuestionRequestDto
        ) -> ConversationPendingQuestion {
            ConversationPendingQuestion(
                requestId: dto.requestId,
                questions: dto.questions.map { question in
                    ConversationAskQuestion(
                        question: question.question,
                        header: question.header,
                        options: question.options.map { option in
                            ConversationAskOption(
                                label: option.label,
                                description: option.description,
                                preview: option.preview
                            )
                        },
                        multiSelect: question.multiSelect
                    )
                },
                timeoutSecs: dto.timeoutSecs
            )
        }

        fileprivate static func message(from dto: MessageDto) -> RenderedMessage {
            let role: Role = (dto.role == "user") ? .user : .ai
            let detail = detail(from: dto.blocks)
            let body = text(from: detail?.blocks ?? [])
            return RenderedMessage(message: Message(role: role, text: body), detail: detail)
        }

        private static func detail(from blocks: [MessageBlockDto]) -> ConversationMessageDetail? {
            let lowered = blocks.compactMap(messageBlock(from:))
            return lowered.isEmpty ? nil : ConversationMessageDetail(blocks: lowered)
        }

        /// Lower ONE wire block. `nil` for a block with nothing to show (blank
        /// text) or one this build does not understand.
        private static func messageBlock(
            from block: MessageBlockDto
        ) -> ConversationMessageBlock? {
            switch block {
            case let .text(text):
                return text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : .text(text)
            case let .thinking(thinking, signature):
                return thinking.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil :
                    .thinking(text: thinking, signature: signature)
            case .redactedThinking:
                return .redactedThinking
            case let .compactBoundary(messagesBefore, messagesAfter, summary):
                return .compactBoundary(
                    messagesBefore: Int(messagesBefore),
                    messagesAfter: Int(messagesAfter),
                    summary: summary
                )
            case let .toolUse(id, tool, inputJson, header):
                return .toolUse(
                    id: id,
                    tool: tool,
                    // Legacy fallback only — the header is what renders when present.
                    inputSummary: ConversationExecutionParsing.summarizeToolInput(inputJson) ?? inputJson,
                    inputJson: inputJson,
                    header: header.map(toolHeader(from:))
                )
            case let .toolResult(id, tool, resultJson, isError, oldString, newString, filePath, display):
                return .toolResult(
                    id: id,
                    tool: tool,
                    isError: isError,
                    summary: ConversationExecutionParsing.summarizeToolResult(
                        resultJson,
                        isError: isError,
                        tool: tool
                    ),
                    resultJson: resultJson,
                    oldString: oldString,
                    newString: newString,
                    filePath: filePath,
                    display: display.map(resultDisplay(from:))
                )
            @unknown default:
                return nil
            }
        }

        // MARK: restored-transcript rebuild (BUG FIX)

        /// The interleaved render list a resumed session produces.
        fileprivate struct RestoredTranscript {
            var messages: [Message] = []
            var items: [ConversationRenderItem] = []
            var details: [UUID: ConversationMessageDetail] = [:]
            var wireSignatures: [String] = []
            var wireIndices: [UInt64?] = []
        }

        /// Canonical identity for one wire message. The protocol currently has
        /// no message id, so all fields that can distinguish a replayed message
        /// are length-prefixed into a collision-resistant key.
        fileprivate static func messageSignature(_ dto: MessageDto) -> String {
            func field(_ value: String) -> String { "\(value.utf8.count):\(value)" }
            let blocks = dto.blocks.map { block -> String in
                switch block {
                case let .text(text): return "text|\(field(text))"
                case let .thinking(thinking, signature):
                    return "thinking|\(field(thinking))|\(field(signature ?? ""))"
                case let .redactedThinking(data): return "redacted|\(field(data))"
                case let .compactBoundary(before, after, summary):
                    return "compact|\(before)|\(after)|\(field(summary))"
                case let .toolUse(id, tool, inputJson, _):
                    return "tool-use|\(field(id))|\(field(tool))|\(field(inputJson))"
                case let .toolResult(id, tool, resultJson, isError, oldString, newString, filePath, _):
                    return "tool-result|\(field(id))|\(field(tool))|\(field(resultJson))|\(isError)|\(field(oldString ?? ""))|\(field(newString ?? ""))|\(field(filePath ?? ""))"
                @unknown default: return "unknown"
                }
            }.joined(separator: "|")
            return "role|\(field(dto.role))|blocks|\(field(blocks))"
        }

        fileprivate static func signatureCounts(_ signatures: [String]) -> [String: Int] {
            var counts: [String: Int] = [:]
            for signature in signatures {
                counts[signature, default: 0] += 1
            }
            return counts
        }

        /// Stable UUID for a message occurrence. FNV-1a with two independent
        /// lanes is sufficient here: the result is only a UI identity, not a
        /// security token, and remains deterministic across process launches.
        private static func stableUUID(_ key: String) -> UUID {
            var first: UInt64 = 14_695_981_039_346_656_037
            var second: UInt64 = 10_995_116_282_111
            for byte in key.utf8 {
                first ^= UInt64(byte)
                first &*= 1_099_511_628_211
                second ^= UInt64(byte) &+ 0x9d
                second &*= 1_099_511_628_211
            }
            let bytes: uuid_t = (
                UInt8(truncatingIfNeeded: first >> 56),
                UInt8(truncatingIfNeeded: first >> 48),
                UInt8(truncatingIfNeeded: first >> 40),
                UInt8(truncatingIfNeeded: first >> 32),
                UInt8(truncatingIfNeeded: first >> 24),
                UInt8(truncatingIfNeeded: first >> 16),
                UInt8(truncatingIfNeeded: first >> 8),
                UInt8(truncatingIfNeeded: first),
                UInt8(truncatingIfNeeded: second >> 56),
                UInt8(truncatingIfNeeded: second >> 48),
                UInt8(truncatingIfNeeded: second >> 40),
                UInt8(truncatingIfNeeded: second >> 32),
                UInt8(truncatingIfNeeded: second >> 24),
                UInt8(truncatingIfNeeded: second >> 16),
                UInt8(truncatingIfNeeded: second >> 8),
                UInt8(truncatingIfNeeded: second)
            )
            return UUID(uuid: bytes)
        }

        /// Rebuild the scrollback from `SessionResumed.messages`.
        ///
        /// One `MessageDto` is NOT one row. In the Anthropic protocol a
        /// `tool_result` block lives in the USER turn, so a 1:1 mapping put every
        /// restored tool result inside a right-aligned user bubble whose renderer
        /// only ever draws `message.text` — the result payload was both misplaced
        /// and invisible. Here, narrative blocks (text / thinking / compaction)
        /// accumulate into a `.message`, and a whole user-initiated turn becomes
        /// one terminal `.run` row anchored after its final assistant message.
        ///
        /// The pair spans two turns, so a `toolResult` MERGES onto the row its
        /// `toolUse` opened (matched by tool-use id) rather than adding a second
        /// run. An orphan result — the assistant turn was compacted away — still
        /// gets a terminal run of its own.
        fileprivate static func restoredTranscript(
            from dtos: [MessageDto],
            sessionID: String,
            identityPrefix: String? = nil,
            occurrenceOffsets: [String: Int] = [:],
            wireIndices: [UInt64?] = [],
            finalizeOrphans: Bool = true
        ) -> RestoredTranscript {
            var out = RestoredTranscript()
            let identityPrefix = identityPrefix ?? sessionID
            var occurrenceCounts = occurrenceOffsets
            var pendingRun: ConversationExecutionRun?
            var pendingRunAnchorMessageID: UUID?
            var restoredRunSequence = 0
            var pendingRunIDHint: String?

            func ensurePendingRun() {
                guard pendingRun == nil else { return }
                restoredRunSequence += 1
                let runIdentity = pendingRunIDHint
                    ?? "\(identityPrefix)|run|\(restoredRunSequence)"
                pendingRun = ConversationExecutionRun(
                    id: "restored-\(stableUUID(runIdentity).uuidString)",
                    sessionId: sessionID,
                    turnId: nil,
                    status: .restored
                )
            }

            func appendActivity(_ activity: ConversationExecutionActivity) {
                pendingRun?.activities.append(activity)
            }

            func finishPendingRun() {
                guard var run = pendingRun else { return }
                // A resumed session has no live turn owner. A call without a
                // persisted result is terminally incomplete, never live work in
                // this process.
                if finalizeOrphans && run.tools.contains(where: { $0.status == .running }) {
                    for index in run.tools.indices where run.tools[index].status == .running {
                        run.tools[index].status = .failed
                    }
                }
                // MessageDto does not carry the turn outcome. Do not promote a
                // failed/cancelled tool into the whole Agent result: an Agent can
                // recover and finish after either. The run is only known to be a
                // settled history row; individual tool colors retain the facts
                // that are actually present on the wire.
                run.status = finalizeOrphans
                    ? .restored
                    : (run.tools.contains(where: { $0.status == .running }) ? .running : .completed)

                if let pendingRunAnchorMessageID,
                   let anchorIndex = out.items.firstIndex(where: { item in
                       guard case let .message(message) = item else { return false }
                       return message.id == pendingRunAnchorMessageID
                   }) {
                    // The run starts when the assistant turn starts, so keep
                    // the activity before its narrative message: user → run
                    // activity → assistant. This avoids moving a completed run
                    // to the tail and preserves wire order after resume.
                    out.items.insert(.run(run), at: anchorIndex)
                } else {
                    out.items.append(.run(run))
                }
                pendingRun = nil
                pendingRunAnchorMessageID = nil
            }

            for (dtoIndex, dto) in dtos.enumerated() {
                let wireSignature = messageSignature(dto)
                let occurrence = occurrenceCounts[wireSignature, default: 0]
                occurrenceCounts[wireSignature] = occurrence + 1
                out.wireSignatures.append(wireSignature)
                let wireIndex = wireIndices.indices.contains(dtoIndex) ? wireIndices[dtoIndex] : nil
                out.wireIndices.append(wireIndex)
                let messageIdentity = wireIndex.map {
                    "\(identityPrefix)|message-index|\($0)"
                } ?? "\(identityPrefix)|message|\(wireSignature)|occurrence|\(occurrence)"
                pendingRunIDHint = messageIdentity
                let role: Role = (dto.role == "user") ? .user : .ai
                let isAssistant = dto.role == "assistant"
                let containsToolResult = dto.blocks.contains { block in
                    if case .toolResult = block { return true }
                    return false
                }
                let startsNewUserTurn = dto.role == "user"
                    && !containsToolResult
                    && dto.blocks.contains { messageBlock(from: $0) != nil }
                if startsNewUserTurn {
                    finishPendingRun()
                }
                if isAssistant {
                    ensurePendingRun()
                }
                var narrative: [ConversationMessageBlock] = []
                var narrativeSequence = 0

                func flushNarrative() {
                    guard !narrative.isEmpty else { return }
                    let detail = ConversationMessageDetail(blocks: narrative)
                    let message = Message(
                        id: stableUUID("\(messageIdentity)|narrative|\(narrativeSequence)"),
                        role: role,
                        text: text(from: narrative)
                    )
                    narrativeSequence += 1
                    out.messages.append(message)
                    out.items.append(.message(message))
                    out.details[message.id] = detail
                    if isAssistant {
                        pendingRunAnchorMessageID = message.id
                    }
                    narrative = []
                }

                for block in dto.blocks {
                    switch block {
                    case let .toolUse(id, tool, inputJson, header):
                        flushNarrative()
                        ensurePendingRun()
                        appendActivity(.tool(id: id))
                        let lowered = header.map(toolHeader(from:))
                        let trace = ConversationToolTrace(
                            id: id,
                            tool: tool,
                            // Temporarily running until the matching result is
                            // seen; finalization marks an orphan as failed.
                            status: .running,
                            inputSummary: lowered == nil
                                ? (ConversationExecutionParsing.summarizeToolInput(inputJson) ?? inputJson)
                                : nil,
                            outputSummary: nil,
                            elapsedMs: nil,
                            header: lowered,
                            display: nil
                        )
                        if let index = pendingRun?.tools.firstIndex(where: { $0.id == id }) {
                            pendingRun?.tools[index] = trace
                        } else {
                            pendingRun?.tools.append(trace)
                        }

                    case let .toolResult(id, tool, resultJson, isError, _, _, _, display):
                        flushNarrative()
                        ensurePendingRun()
                        if pendingRun?.activities.contains(where: {
                            if case let .tool(existing) = $0 { return existing == id }
                            return false
                        }) != true {
                            appendActivity(.tool(id: id))
                        }
                        let lowered = display.map(resultDisplay(from:))
                        let status: ConversationToolStatus =
                            ConversationExecutionParsing.isCancellationResult(resultJson)
                                ? .cancelled
                                : (isError ? .failed : .completed)
                        let fallback = lowered == nil
                            ? ConversationExecutionParsing.summarizeToolResult(
                                resultJson, isError: isError, tool: tool)
                            : nil
                        if let index = pendingRun?.tools.firstIndex(where: { $0.id == id }),
                           let existing = pendingRun?.tools[index] {
                            var merged = existing
                            merged.status = status
                            merged.display = lowered
                            merged.outputSummary = fallback
                            pendingRun?.tools[index] = merged
                        } else {
                            let trace = ConversationToolTrace(
                                id: id,
                                tool: tool,
                                status: status,
                                inputSummary: nil,
                                outputSummary: fallback,
                                elapsedMs: nil,
                                header: nil,
                                display: lowered
                            )
                            pendingRun?.tools.append(trace)
                        }

                    default:
                        if let lowered = messageBlock(from: block) {
                            narrative.append(lowered)
                            if isAssistant,
                               case let .thinking(text, _) = lowered {
                                if let index = pendingRun?.activities.lastIndex(where: {
                                    if case .reasoning = $0 { return true }
                                    return false
                                }),
                                   case let .reasoning(id, current) = pendingRun?.activities[index] {
                                    pendingRun?.activities[index] = .reasoning(id: id, text: current + text)
                                } else {
                                    appendActivity(.reasoning(id: "reasoning-\(messageIdentity)-\(narrative.count)", text: text))
                                }
                                pendingRun?.reasoning += text
                            } else if isAssistant, case .text = lowered {
                                appendActivity(.textBoundary(id: "boundary-\(messageIdentity)-\(narrative.count)"))
                            } else if isAssistant, case .compactBoundary = lowered {
                                appendActivity(.textBoundary(id: "boundary-\(messageIdentity)-\(narrative.count)"))
                            }
                        }
                    }
                }
                flushNarrative()
            }
            finishPendingRun()
            return out
        }

        /// Flatten structured blocks into the plain transcript text the rest of
        /// the app still consumes. The chat renderer itself uses the structured
        /// blocks for display.
        private static func text(from blocks: [ConversationMessageBlock]) -> String {
            blocks.compactMap { block -> String? in
                switch block {
                case let .text(text):
                    return text
                case let .thinking(text, _):
                    return text
                case .redactedThinking:
                    return String(localized: "chat_redacted_thinking")
                case .compactBoundary:
                    return String(localized: "chat_compacted_label")
                case let .toolUse(_, tool, _, _, header):
                    // Prefer the engine's derived title so the compatibility
                    // transcript (voice / setup surfaces) reads the same as the UI.
                    return header.map(ToolDisplayText.title)
                        ?? String(localized: "chat_tool_calling \(tool)")
                case let .toolResult(_, _, isError, summary, _, _, _, _, display):
                    let body = display.flatMap(ToolDisplayText.headline) ?? summary
                    return isError ? body : String(localized: "chat_tool_result_summary \(body)")
                }
            }
            .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            .joined(separator: "\n\n")
        }

        /// Map the lowered `ErrorKindDto` onto the UI-facing `ConversationError.Kind`.
        private static func kind(from dto: ErrorKindDto) -> ConversationError.Kind {
            switch dto {
            case .transport: return .transport
            case .protocol: return .protocol
            case .server: return .server
            case .maxTurns: return .maxTurns
            case .rejected: return .rejected
            case .internal: return .internal
            @unknown default: return .internal
            }
        }

        private func fail(_ kind: ConversationError.Kind, _ message: String) {
            let settledTurn = currentTurnId != nil
            model.error = ConversationError(kind: kind, message: message)
            model.streaming = false
            model.statusLine = nil
            publishActiveTurnCompletion(.failed)
            // A terminal error tears down the turn — its parked permission (if any)
            // can never be answered now, so drop the prompt rather than leave it
            // stranded.
            model.pendingPermissions = []
            clearTurnPointers(keepEpoch: false)
            if settledTurn {
                requestSessionCatalogRefreshAfterSettledTurn()
            }
        }

        // MARK: model selection (SHIP-BLOCKER #2)

        /// Adopt the engine's reported active model id. Updates the out-of-band
        /// `activeModelId`, persists it to the Keychain (so a relaunch resumes this
        /// real model instead of falling back to the engine default), and keeps the
        /// friendly chip in sync when the id maps to a known mock entry (a friendly
        /// label is optional — the picker itself is driven by `availableModels`).
        private func applyActiveModel(_ id: String) {
            guard !id.isEmpty else { return }
            model.activeModelId = id
            Keychain.set(.model, id)
            if let opt = MockData.models.first(where: { $0.id == id || $0.name == id }) {
                model.model = opt
            }
        }

        /// Switch the active model (SHIP-BLOCKER #2): submit `SetModel` with a REAL
        /// engine id. The engine confirms with `ModelChanged`, which `applyActiveModel`
        /// adopts + persists. Optimistically reflect the id so the chip updates even
        /// before the round-trip completes. No-op when the id is already active.
        func setModel(_ id: String) {
            guard !id.isEmpty, id != model.activeModelId else { return }
            let previous = model.activeModelId
            model.activeModelId = id
            if let opt = MockData.models.first(where: { $0.id == id || $0.name == id }) {
                model.model = opt
            }
            Task { [weak self] in
                guard let self else { return }
                do {
                    let handle = try await self.ensureHandle()
                    try await handle.submit(command: .setModel(model: id))
                } catch {
                    // The engine REJECTS a model no configured provider serves,
                    // so the optimistic update above has to be undone — leaving
                    // the chip on a model the session did not switch to would
                    // send the next turn under a label that is simply wrong.
                    self.model.activeModelId = previous
                    self.fail(.host, String(localized: "chat_switch_model_failed \(error)"))
                }
            }
        }

        // MARK: session + lifecycle

        /// Switch the active conversation to another session (iOS analog of
        /// Android `ChatViewModel.openSession`). Cancels the in-flight turn first,
        /// retaining the old transcript and correlator until the host confirms
        /// release. Only then is the transcript reset for the selected session.
        func openSession(_ session: SessionRef) {
            let turnIdToCancel = inFlightTurnForSessionSwitch()
            submitSessionCancellation(turnIdToCancel, isNew: false)
        }

        /// Resume a prior engine session by UUID (the drawer-tap path for a REAL
        /// history row). Cancels any in-flight turn first, resets the transcript
        /// only after the safe owner slot is released, then submits
        /// `ResumeSession`. The drawer owns the
        /// optimistic selection; the model changes only when the engine confirms
        /// the transition, preventing a stale SessionList from winning the race. The
        /// engine hot-restores the prior transcript into the running orchestrator
        /// and confirms with `SessionResumed{session_id, messages}`, which
        /// re-adopts the id AND replaces the placeholder with the real restored
        /// conversation (oldest-first) so the next turn continues with full prior
        /// context visible. Re-requesting the active id is intentional: process
        /// restoration still needs the engine to replay its transcript.
        func resumeSession(_ uuid: String, emptySessionTitle: String?) {
            guard !uuid.isEmpty else { return }
            let turnIdToCancel = inFlightTurnForSessionSwitch()
            model.sessionRestoreRecovery = nil
            model.sessionTransitionFailure = nil
            let transitionOperationID = beginSessionTransition(.resume(uuid))
            prepareSessionTransition(cancelling: turnIdToCancel, isNew: false)
            let submission: SessionTransitionSubmission
            if let emptySessionTitle {
                submission = .resumeEmpty(sessionID: uuid, title: emptySessionTitle)
            } else {
                submission = .command(.resumeSession(sessionId: uuid, cwd: nil))
            }
            submitSessionTransition(
                cancelling: turnIdToCancel,
                submission: submission,
                failurePrefix: String(localized: "chat_resume_session_failed"),
                transitionOperationID: transitionOperationID,
                resumeTargetID: uuid,
                allowsMissingSessionReplacement: emptySessionTitle == nil,
                isNew: false
            )
        }

        // MARK: permission gating (SHIP-BLOCKER #3)

        /// Enqueue one outbound permission request (called on the main actor by the
        /// sink). De-dupes by `requestId` so a re-delivered request can't stack two
        /// prompts. The chat view renders the head of the queue.
        fileprivate func enqueuePermission(_ request: PermissionRequest) {
            guard model.streaming,
                  !model.isCancelling,
                  currentTurnId != nil,
                  !model.pendingPermissions.contains(where: { $0.requestId == request.requestId })
            else { return }
            model.pendingPermissions.append(PendingPermission(request: request))
        }

        /// Resolve the head request by approving it (allow-once / allow-always):
        /// submit `ApprovePermission` and pop it so the next request surfaces.
        func approvePermission(_ requestId: UInt64, _ response: PermissionResponseDto) {
            resolve(requestId, command: .approvePermission(requestId: requestId, response: response))
        }

        /// Resolve the head request by denying it: submit `DenyPermission` and pop it.
        func denyPermission(_ requestId: UInt64) {
            resolve(requestId, command: .denyPermission(requestId: requestId))
        }

        /// Shared resolution path: optimistically pop the prompt (the gate's oneshot
        /// fires from the submitted command) and submit the resolving command on the
        /// engine runtime. A submit failure surfaces as a host error banner.
        private func resolve(_ requestId: UInt64, command: ClientCommand) {
            model.pendingPermissions.removeAll { $0.requestId == requestId }
            Task { [weak self] in
                guard let self else { return }
                do {
                    let handle = try await self.ensureHandle()
                    try await handle.submit(command: command)
                } catch {
                    self.fail(.host, String(localized: "chat_permission_response_failed \(error)"))
                }
            }
        }
    }

    /// Swift implementation of the `IosPermissionSink` UniFFI callback interface
    /// (SHIP-BLOCKER #3). Rust calls `onRequest(_:)` on the engine's runtime when a
    /// tool needs approval; we hop to the main actor and enqueue the request so the
    /// chat view can prompt. Returns promptly — the engine's turn parks on its own
    /// oneshot and is resolved later by `ApprovePermission` / `DenyPermission`.
    final class EnginePermissionSink: IosPermissionSink {
        private weak var source: EngineConversationSource?

        init(source: EngineConversationSource) {
            self.source = source
        }

        func onRequest(request: PermissionRequest) async {
            await MainActor.run { [weak source] in
                source?.enqueuePermission(request)
            }
        }
    }

    /// Swift implementation of the `IosEventListener` UniFFI callback interface.
    /// Rust calls `onEvent(_:)` on the engine's runtime; we hop to the main actor
    /// and forward to the source so all state mutation is main-actor-confined.
    final class EngineListener: IosEventListener {
        private weak var source: EngineConversationSource?

        init(source: EngineConversationSource) {
            self.source = source
        }

        func onEvent(event: ClientEvent) async {
            await MainActor.run { [weak source] in
                source?.apply(event)
            }
        }
    }

#endif
