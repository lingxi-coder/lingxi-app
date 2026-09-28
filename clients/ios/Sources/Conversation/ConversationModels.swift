import Combine
import Foundation
import OSLog
import SwiftUI

#if canImport(harness_runtimeFFI)
import AuthenticationServices
import UIKit
import harness_runtimeFFI
#endif


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


/// A small SwiftUI projection of the engine-owned controls DTO. Keeping this
/// projection provider-neutral lets the mock source compile without UniFFI while
/// preserving budgets, disabled reasons, and dynamic permission availability.
struct ConversationReasoningOption: Identifiable, Equatable {
    let id: String
    let title: String
    let isBudget: Bool
    let persistable: Bool
}

struct ConversationPermissionOption: Identifiable, Equatable {
    let id: String
    let available: Bool
    let disabledReason: String?
}

struct ConversationControlsState: Equatable {
    let qualifiedModel: String
    let requestedPermission: String
    let effectivePermission: String
    let permissionOptions: [ConversationPermissionOption]
    let requestedReasoning: String
    let effectiveReasoning: String
    let reasoningOptions: [ConversationReasoningOption]
    let budgetRange: ClosedRange<UInt64>?
    let providerDefault: String
    let forcedReasoning: Bool
    let editable: Bool
    let disabledReason: String?
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

#if canImport(harness_runtimeFFI)
/// One engine-parked permission request the UI must answer (SHIP-BLOCKER #3).
///
/// The engine's adapter gate emits a `PermissionRequest` whenever a tool needs
/// approval (e.g. a Write/Bash invocation) and parks the turn on a oneshot until
/// the user answers. On mobile that request used to vanish into a no-op sink, so
/// the turn hung forever; now the sink forwards it here and the app root presents
/// a prompt above whichever screen is active. The user's choice resolves the
/// park by submitting
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
    /// When true, the prompt must not offer or persist an AllowAlways rule.
    let suppressAlwaysAllowRule: Bool
    /// Engine-owned optional Auto action for the primary approval button.
    let autoModePrompt: AutoModePromptDto?

    var id: UInt64 { requestId }

    init(request: PermissionRequest) {
        self.requestId = request.requestId
        self.kind = request.kind
        self.worker = request.worker
        self.suppressAlwaysAllowRule = request.suppressAlwaysAllowRule
        self.autoModePrompt = request.autoModePrompt
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
    let model: String?
    let modelProfile: String?
    var status: String
    var latestActivity: String?
    var updatedAtMs: UInt64

    init(
        id: String,
        name: String,
        agentType: String,
        model: String? = nil,
        modelProfile: String? = nil,
        status: String,
        latestActivity: String? = nil,
        updatedAtMs: UInt64 = 0
    ) {
        self.id = id
        self.name = name
        self.agentType = agentType
        self.model = model
        self.modelProfile = modelProfile
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

#if canImport(harness_runtimeFFI)
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
    private let disclosureStore: ConversationDisclosureStore

    /// The full visible transcript (user + assistant turns).
    @Published var messages: [Message] {
        didSet {
            if !suppressIndexRebuild {
                rebuildMessageIndex()
            }
        }
    }
    /// The chat surface's ordered render list: plain messages plus per-turn
    /// execution traces / shell cards. `messages` remains the compatibility
    /// transcript used by voice/setup surfaces.
    @Published var items: [ConversationRenderItem] {
        didSet {
            timelineGroupsCache = nil
            if !suppressIndexRebuild {
                rebuildItemIndex()
            }
            refreshTranscriptAgentAnchors()
        }
    }

    /// Session-scoped agent roster.  The first row is always the root agent;
    /// child rows arrive from `SessionAgentList`/`SessionAgentUpdated` or the
    /// coordinator worker fallback on older engines.
    @Published private(set) var agentSummaries: [ConversationAgentSummary]
    /// Stable transcript placement keyed by child agent id. Empty anchors mean
    /// the agent belongs at the top; other values are tool/item ids.
    @Published private(set) var transcriptAgentAnchors: [String: String] = [:]
    /// First-seen child order is independent of the engine's roster sort order.
    private var agentFirstSeenOrder: [String] = []
    /// Last visible position of each concrete anchor, used to recover from a
    /// rewind or compaction that removes the anchored row.
    private var transcriptAgentAnchorPositions: [String: Int] = [:]
    /// The currently visible agent.  Selecting a child makes its transcript
    /// read-only; selecting `main` restores the ordinary composer transcript.
    @Published var selectedAgentID: String {
        didSet { timelineGroupsCache = nil }
    }
    /// Child-agent transcript cache keyed by stable agent id.  Main-agent data
    /// intentionally remains in `messages/items` for backwards compatibility.
    @Published private(set) var agentTranscripts: [String: ConversationAgentTranscript] {
        didSet { timelineGroupsCache = nil }
    }
    @Published var isAgentTranscriptLoading = false
    @Published var agentTranscriptError: String?
    /// Identifies the one transcript request allowed to mutate loading/error
    /// state. A late task from a previous agent or session is ignored.
    private var agentTranscriptRequestKey: String?
    private var agentTranscriptGeneration: UInt64 = 0
    #if canImport(harness_runtimeFFI)
        private var pendingAgentMessages: [String: [ConversationPendingAgentMessage]] = [:]
    #endif

    /// The list renderer consumes these projections so it does not need to
    /// know whether the selected row is the root or a child agent.
    var selectedAgentMessages: [Message] {
        guard selectedAgentID != Self.mainAgentID else { return messages }
        return agentTranscripts[selectedAgentID]?.messages ?? []
    }
    var selectedAgentItems: [ConversationRenderItem] {
        guard selectedAgentID != Self.mainAgentID else {
            let hidden = Set(messages.flatMap { $0.loopFoldedItemIDs })
            return items.filter { !hidden.contains($0.id) }
        }
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
    /// Every transcript surface consumes the same model-owned roster order.
    var orderedAgentSummaries: [ConversationAgentSummary] { agentSummaries }
    /// Ordered activity projection for the currently selected session/agent.
    /// Run cards never enter this projection; reasoning, tools, notices and
    /// narrative rows retain their wire order and stable identities.
    /// Memoised because `timelineGroups` walks every transcript item and
    /// `ChatView.body` reads this more than once per evaluation, on every
    /// streamed token. The three inputs are stored `@Published` properties, so
    /// their `didSet` hooks are a single invalidation point covering every
    /// mutation site.
    var visibleTimelineGroups: [ConversationTimelineGroup] {
        if let timelineGroupsCache { return timelineGroupsCache }
        let groups = ConversationRenderLayout.timelineGroups(selectedAgentItems)
        timelineGroupsCache = groups
        timelineGroupsRebuildCount += 1
        return groups
    }

    private var timelineGroupsCache: [ConversationTimelineGroup]?
    private var messageIndexByID: [UUID: Int] = [:]
    private var itemIndexByMessageID: [UUID: Int] = [:]
    private var suppressIndexRebuild = false

    /// Subscript updates that preserve array shape and element identities do
    /// not require rebuilding the UUID maps. Streaming uses this around token
    /// replacements and run-card updates; structural append/replace paths keep
    /// the normal observer rebuild.
    func withIndexRebuildSuppressed<T>(_ body: () -> T) -> T {
        suppressIndexRebuild = true
        defer { suppressIndexRebuild = false }
        return body()
    }

    func indexOfMessage(id: UUID) -> Int? {
        if let cached = messageIndexByID[id],
           messages.indices.contains(cached),
           messages[cached].id == id {
            return cached
        }
        rebuildMessageIndex()
        return messageIndexByID[id]
    }

    func indexOfMessageItem(id: UUID) -> Int? {
        if let cached = itemIndexByMessageID[id],
           items.indices.contains(cached),
           case let .message(existing) = items[cached],
           existing.id == id {
            return cached
        }
        rebuildItemIndex()
        return itemIndexByMessageID[id]
    }

    private func rebuildMessageIndex() {
        var map = [UUID: Int](minimumCapacity: messages.count)
        for (index, message) in messages.enumerated() {
            map[message.id] = index
        }
        messageIndexByID = map
    }

    private func rebuildItemIndex() {
        var map = [UUID: Int]()
        for (index, item) in items.enumerated() {
            if case let .message(message) = item {
                map[message.id] = index
            }
        }
        itemIndexByMessageID = map
    }

    /// Number of times the projection was actually rebuilt. Test-only signal;
    /// production code must not branch on it.
    private(set) var timelineGroupsRebuildCount: Int = 0
    var visibleTimelineRows: [ConversationTimelineRow] {
        ConversationRenderLayout.timelineRows(selectedAgentItems)
    }
    /// Structured block payload for assistant bubbles keyed by `Message.id`.
    @Published var messageDetails: [UUID: ConversationMessageDetail] = [:]
    /// True while a turn is in flight (drives the streaming dots row + gates
    /// overlapping sends and the Send→Stop swap, PR-4 items 1 & 2).
    @Published var streaming: Bool = false
    /// True when the correlated durable turn is inactive in the UI (for example
    /// `WaitingForUser` after a foreground recovery). The Composer uses this
    /// only to expose an explicit Stop/Discard action; background execution and
    /// voice policy continue to derive from `streaming`.
    @Published var hasInactiveDurableRecovery: Bool = false
    /// True while the correlated turn is in a non-terminal durable recovery
    /// state, including an executor-backed live WaitingForUser question. Root
    /// navigation uses this to avoid changing session selection before New or
    /// Resume is accepted; the Composer uses the more specific inactive flag
    /// for its visible Discard affordance.
    @Published var hasUnresolvedTurnRecovery: Bool = false
    /// True after Stop is requested and until the engine confirms that the
    /// matching turn released its owner slot. The composer remains editable but
    /// cannot submit another turn during this interval.
    @Published var isCancelling: Bool = false
    /// The latest terminal turn result. Consumers must correlate its token with
    /// the token returned by `ConversationSource.send`; observing the transcript's
    /// last AI message is insufficient because session switches and late events
    /// can otherwise replay stale content.
    @Published var turnCompletion: ConversationTurnCompletion? = nil
    /// The client-owned turn currently in flight, when any. Exposed so
    /// background execution and notification surfaces can deep-link back to the
    /// exact conversation turn without peeking into source-private state.
    @Published var activeTurnToken: ConversationTurnToken? = nil
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
    /// Rich per-route facts paired with `availableModels` by the exact qualified reference.
    @Published var availableModelDetails: [String: ModelRuntimeDetails] = [:]
    /// The active model id the engine reports (`ModelList.current` / `ModelChanged.model`).
    @Published var activeModelId: String = ""
    /// Whether the current source was launched with at least one enabled LLM
    /// provider. The engine may still publish its built-in default model while
    /// keyless, so model availability alone cannot represent configuration.
    @Published var providerConfigured = false
    /// The effective permission mode reported by the engine after applying
    /// model/provider/killswitch safety gates. This may differ from the
    /// persisted user preference (for example, `auto` can downgrade to
    /// `default`).
    @Published var effectivePermissionMode: String = "auto"
    @Published var controls: ConversationControlsState?
    /// Engine-authoritative conversation controls. These remain provider
    /// neutral so the composer never guesses which effort values are valid.
    @Published var requestedPermissionMode: String = "auto"
    @Published var requestedTypescriptLspMode: String = "auto"
    @Published var effectiveTypescriptLspMode: String = "auto"
    @Published var typescriptLspAvailable: Bool = true
    @Published var reasoningSelection: String = "automatic"
    @Published var reasoningOptions: [String] = []
    @Published var reasoningOptionDetails: [ConversationReasoningOption] = []
    @Published var permissionOptions: [ConversationPermissionOption] = []
    @Published var reasoningDisabledReason: String?
    @Published var controlsPending: Bool = false
    @Published var controlsError: String?
    /// Confirmed engine speed preference; command delivery does not imply a change.
    @Published var fastMode: Bool = false
    @Published var fastModePending: Bool = false
    @Published var fastModeError: String?
    @Published var bypassPermissionsWarningSuppressed: Bool = false
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
    @Published var activeSessionId: String = "" {
        didSet { restoreTranscriptDisclosures() }
    }
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
    /// Latest manual `/compact` lifecycle; terminal state remains until the next action.
    @Published var compactionStatus: ConversationCompactionStatus? = nil
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
    /// Direct workflow resume feedback. Kept separate from task status so a
    /// rejected resume can be retried without mutating the paused row.
    @Published var workflowResumeState: WorkflowResumeState = .idle
    /// The model's own working plan (TodoWrite / Task checklist), replaced
    /// WHOLESALE on every `PlanUpdated` — the engine emits the complete ordered
    /// list and an empty list clears it. Drives `PlanTasksPanel`, pinned closest
    /// to the composer. Distinct from `backgroundTasks`, which are engine jobs.
    @Published var planTasks: [ConversationPlanTask] = []
    /// Session-owned transcript disclosures: tool bodies, tool groups, and long
    /// messages. Standalone tools use their tool-use id; other rows use namespaced
    /// stable ids so lazy recycling never resets the user's choices.
    ///
    /// This lives HERE and not in the row: every transcript list recycles its
    /// rows, so row-local `@State` is dropped on scroll and then reappears on
    /// whichever row happens to reuse the storage.
    @Published var expandedToolCalls: Set<String> = []
    #if canImport(harness_runtimeFFI)
        /// FIFO queue of engine-parked permission requests (SHIP-BLOCKER #3). The
        /// chat view renders the head (`first`) as a modal prompt; answering it pops
        /// the head and reveals the next. Engine-scoped: main-turn cancellation
        /// and transcript/session resets do not clear it; only explicit resolution
        /// or real engine/gate teardown does. Empty between requests / on the mock
        /// (which never asks for permission).
        @Published var pendingPermissions: [PendingPermission] = []
    #endif

    init(messages: [Message] = [],
         model: ModelOption = MockData.models[0],
         disclosureDefaults: UserDefaults = .standard) {
        self.disclosureStore = ConversationDisclosureStore(defaults: disclosureDefaults)
        self.messages = messages
        self.items = messages.map(ConversationRenderItem.message)
        self.model = model
        self.agentSummaries = [.main]
        self.selectedAgentID = Self.mainAgentID
        self.agentTranscripts = [:]
        self.reasoningOptionDetails = [
            ConversationReasoningOption(id: "automatic", title: "Auto", isBudget: false, persistable: true)
        ]
        self.reasoningOptions = ["automatic"]
        self.permissionOptions = [
            "default", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions"
        ].map { ConversationPermissionOption(id: $0, available: true, disabledReason: nil) }
        rebuildMessageIndex()
        rebuildItemIndex()
    }

    func toggleTranscriptDisclosure(_ id: String) {
        if expandedToolCalls.contains(id) { expandedToolCalls.remove(id) }
        else { expandedToolCalls.insert(id) }
        disclosureStore.save(expandedToolCalls, sessionID: activeSessionId)
    }

    func restoreTranscriptDisclosures() {
        expandedToolCalls = disclosureStore.load(sessionID: activeSessionId)
    }

    var hasLiveTranscriptOwner: Bool {
        if selectedAgentID != Self.mainAgentID {
            return selectedAgentSummary.map { AgentStatusPresentation(rawValue: $0.status) == .running } ?? false
        }
        return streaming || hasUnresolvedTurnRecovery
    }

    var liveTranscriptToolIDs: Set<String> {
        let visibleItems = selectedAgentID == Self.mainAgentID ? items : selectedAgentItems
        return Set(visibleItems.flatMap { item -> [String] in
            guard case let .run(run) = item,
                  (run.status == .running && hasLiveTranscriptOwner) || run.activeWorkers > 0 else { return [] }
            return run.tools.filter { $0.status == .running }.map(\.id)
        })
    }

    /// Replace the agent roster while retaining the selected row when it is
    /// still present.  A missing selection safely falls back to the main
    /// agent, which is also what session transitions use.
    func replaceAgentSummaries(_ summaries: [ConversationAgentSummary]) {
        let currentByID = Dictionary(uniqueKeysWithValues: agentSummaries.map { ($0.id, $0) })
        let incoming = summaries.map(Self.normalizedAgentSummaryForSource)
        for summary in incoming where summary.id != Self.mainAgentID {
            if !agentFirstSeenOrder.contains(summary.id) {
                agentFirstSeenOrder.append(summary.id)
            }
        }
        let incomingByID = Dictionary(uniqueKeysWithValues: incoming.map { ($0.id, $0) })
        let currentMain = agentSummaries.first(where: { $0.id == Self.mainAgentID }) ?? .main
        var merged: [ConversationAgentSummary] = [currentMain]
        merged.append(contentsOf: agentFirstSeenOrder.compactMap { id in
            guard let incomingSummary = incomingByID[id] else { return nil }
            return currentByID[id].map {
                Self.mergeAgentSummary(current: $0, incoming: incomingSummary)
            } ?? incomingSummary
        })
        if let incomingMain = incomingByID[Self.mainAgentID] {
            merged[0] = Self.mergeAgentSummary(current: currentMain, incoming: incomingMain)
        }
        agentSummaries = merged
        refreshTranscriptAgentAnchors()
        if !merged.contains(where: { $0.id == selectedAgentID }) {
            selectedAgentID = Self.mainAgentID
            isAgentTranscriptLoading = false
            agentTranscriptError = nil
            agentTranscriptRequestKey = nil
        }
    }

    private static func mergeAgentSummary(
        current: ConversationAgentSummary,
        incoming: ConversationAgentSummary
    ) -> ConversationAgentSummary {
        let incomingIsCurrent = incoming.updatedAtMs >= current.updatedAtMs
        let currentIsTerminal = ["completed", "failed", "killed", "cancelled"]
            .contains(current.status.lowercased())
        let incomingIsTerminal = ["completed", "failed", "killed", "cancelled"]
            .contains(incoming.status.lowercased())
        let name = incomingIsCurrent
            ? (incoming.name.isEmpty ? current.name : incoming.name)
            : (current.name.isEmpty ? incoming.name : current.name)
        let agentType = incomingIsCurrent
            ? (incoming.agentType.isEmpty ? current.agentType : incoming.agentType)
            : (current.agentType.isEmpty ? incoming.agentType : current.agentType)
        let model = incomingIsCurrent
            ? (incoming.model.flatMap { $0.isEmpty ? nil : $0 } ?? current.model)
            : (current.model.flatMap { $0.isEmpty ? nil : $0 } ?? incoming.model)
        let modelProfile = incomingIsCurrent
            ? (incoming.modelProfile.flatMap { $0.isEmpty ? nil : $0 } ?? current.modelProfile)
            : (current.modelProfile.flatMap { $0.isEmpty ? nil : $0 } ?? incoming.modelProfile)
        // The terminal rule is an ORDERING guard — it exists so a `running`
        // that was emitted BEFORE the end, and merely delivered after it,
        // cannot resurrect a finished agent. Applied to a NEWER update it
        // becomes an absorbing state instead, and that is wrong for every
        // agent that can come back: a parked background agent now reports
        // claude-code's `completed` (rendered `done`) rather than the port's
        // invented `idle`, so without `!incomingIsCurrent` a resumed agent
        // would sit at `done` for the rest of the session while it worked.
        // `incomingIsCurrent` is the ordering signal the guard actually wants.
        let status = if currentIsTerminal && !incomingIsTerminal && !incomingIsCurrent {
            current.status
        } else if incomingIsCurrent {
            incoming.status
        } else {
            current.status
        }
        let latestActivity = incomingIsCurrent
            ? (incoming.latestActivity.flatMap { $0.isEmpty ? nil : $0 } ?? current.latestActivity)
            : current.latestActivity

        return ConversationAgentSummary(
            id: current.id,
            name: name,
            agentType: agentType,
            model: model,
            modelProfile: modelProfile,
            status: status,
            latestActivity: latestActivity,
            updatedAtMs: max(current.updatedAtMs, incoming.updatedAtMs)
        )
    }

    static func normalizedAgentSummaryForSource(_ summary: ConversationAgentSummary) -> ConversationAgentSummary {
        guard summary.id == Self.mainAgentID else { return summary }
        return ConversationAgentSummary(
            id: Self.mainAgentID,
            name: ConversationAgentSummary.main.name,
            agentType: summary.agentType,
            model: summary.model,
            modelProfile: summary.modelProfile,
            status: summary.status,
            latestActivity: summary.latestActivity,
            updatedAtMs: summary.updatedAtMs
        )
    }

    func upsertAgentSummary(_ summary: ConversationAgentSummary) {
        var next = agentSummaries
        if let index = next.firstIndex(where: { $0.id == summary.id }) {
            next[index] = Self.mergeAgentSummary(current: next[index], incoming: summary)
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
            model: current.model,
            modelProfile: current.modelProfile,
            status: status,
            latestActivity: latestActivity,
            updatedAtMs: UInt64(Date().timeIntervalSince1970 * 1_000)
        ))
    }

    func clearAgentState() {
        agentSummaries = [.main]
        agentFirstSeenOrder = []
        transcriptAgentAnchors = [:]
        transcriptAgentAnchorPositions = [:]
        selectedAgentID = Self.mainAgentID
        agentTranscripts = [:]
        isAgentTranscriptLoading = false
        agentTranscriptError = nil
        agentTranscriptRequestKey = nil
        #if canImport(harness_runtimeFFI)
            pendingAgentMessages = [:]
        #endif
    }

    private func refreshTranscriptAgentAnchors() {
        let childIDs = agentSummaries.map(\.id).filter { $0 != Self.mainAgentID }
        let childIDSet = Set(childIDs)
        var next = transcriptAgentAnchors.filter { childIDSet.contains($0.key) }
        transcriptAgentAnchorPositions = transcriptAgentAnchorPositions.filter { childIDSet.contains($0.key) }

        // Use the same durable-to-display projection as the timeline. This
        // gives standalone tool calls the same concrete trace id as run tools
        // and excludes terminal reasoning rows hidden by the display policy.
        let visibleGroups = ConversationDesktopTimeline.groups(
            ConversationRenderLayout.timelineGroups(items)
        )
        let visibleRowIDs = visibleGroups.flatMap { group in
            group.rows.map { Self.transcriptAnchorID(for: $0) }
        }
        let visibleBoundary = visibleRowIDs.last

        for id in childIDs {
            if let trace = items.lazy.flatMap({ Self.spawnTraces(in: $0) }).first(where: {
                Self.transcriptAgentKey($0.agentID) == Self.transcriptAgentKey(id)
            }) {
                // Structured spawn metadata is authoritative and may upgrade a
                // previously assigned fallback row.
                next[id] = trace.traceID
                transcriptAgentAnchorPositions[id] = visibleRowIDs.firstIndex(of: trace.traceID)
                continue
            }

            if let existing = next[id], !existing.isEmpty {
                if let position = visibleRowIDs.firstIndex(of: existing) {
                    transcriptAgentAnchorPositions[id] = position
                } else if let previousPosition = transcriptAgentAnchorPositions[id], !visibleRowIDs.isEmpty {
                    // Rewind/compaction removed the anchor. Pick the closest
                    // surviving boundary before its former position.
                    let position = min(max(previousPosition - 1, 0), visibleRowIDs.count - 1)
                    next[id] = visibleRowIDs[position]
                    transcriptAgentAnchorPositions[id] = position
                } else {
                    // An anchor with no remembered position cannot be safely
                    // ordered relative to surviving rows, so place it at the
                    // top exactly once rather than guessing a later boundary.
                    next[id] = ""
                    transcriptAgentAnchorPositions.removeValue(forKey: id)
                }
            } else if let visibleBoundary {
                // Keep a concrete fallback stable while it survives. The empty
                // sentinel is reserved for a transcript with no visible row.
                next[id] = visibleBoundary
                transcriptAgentAnchorPositions[id] = visibleRowIDs.count - 1
            } else {
                next[id] = ""
                transcriptAgentAnchorPositions.removeValue(forKey: id)
            }
        }
        if next != transcriptAgentAnchors { transcriptAgentAnchors = next }
    }

    private static func transcriptAnchorID(for row: ConversationTimelineRow) -> String {
        if case let .tool(_, trace) = row { return trace.id }
        return row.id
    }

    private static func spawnTraces(in item: ConversationRenderItem) -> [(agentID: String, traceID: String)] {
        switch item {
        case let .run(run):
            return run.tools.compactMap { trace in
                guard let agentID = trace.spawnedAgentID else { return nil }
                return (agentID: agentID, traceID: trace.id)
            }
        case let .toolCall(trace):
            guard let agentID = trace.spawnedAgentID else { return [] }
            return [(agentID: agentID, traceID: trace.id)]
        default:
            return []
        }
    }

    private static func transcriptAgentKey(_ id: String) -> String {
        id.hasPrefix("agent:") ? String(id.dropFirst("agent:".count)) : id
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
        #if canImport(harness_runtimeFFI)
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

    #if canImport(harness_runtimeFFI)
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
            items: items,
            compactionStatus: compactionStatus
        )
    }

    var backgroundExecutionActivity: AnyPublisher<Bool, Never> {
        Publishers.CombineLatest4($streaming, $backgroundTasks, $items, $compactionStatus)
            .map(Self.requiresBackgroundExecution)
            .removeDuplicates()
            .eraseToAnyPublisher()
    }

    private static func requiresBackgroundExecution(
        streaming: Bool,
        backgroundTasks: [BackgroundTaskSnapshot],
        items: [ConversationRenderItem],
        compactionStatus: ConversationCompactionStatus?
    ) -> Bool {
        if streaming || backgroundTasks.contains(where: { $0.status.requiresExecutionLease }) {
            return true
        }
        if compactionStatus?.isActive == true {
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
