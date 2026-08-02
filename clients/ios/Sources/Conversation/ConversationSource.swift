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

import Foundation
import SwiftUI

#if canImport(engine_mobileFFI)
    import engine_mobileFFI
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
            case .transport: return "网络错误"
            case .protocol: return "协议错误"
            case .server: return "服务端错误"
            case .maxTurns: return "已达最大轮数"
            case .rejected: return "操作被拒绝"
            case .internal: return "内部错误"
            case .host: return "引擎错误"
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
        case .maxTurns: return "已达最大轮数，对话已停止。"
        case .cancelled: return "已取消本轮。"
        }
    }
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
@MainActor
final class ConversationModel: ObservableObject {
    /// The full visible transcript (user + assistant turns).
    @Published var messages: [Message]
    /// The chat surface's ordered render list: plain messages plus per-turn
    /// execution traces / shell cards. `messages` remains the compatibility
    /// transcript used by voice/setup surfaces.
    @Published var items: [ConversationRenderItem]
    /// Structured block payload for assistant bubbles keyed by `Message.id`.
    @Published var messageDetails: [UUID: ConversationMessageDetail] = [:]
    /// True while a turn is in flight (drives the streaming dots row + gates
    /// overlapping sends and the Send→Stop swap, PR-4 items 1 & 2).
    @Published var streaming: Bool = false
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
    /// Real MCP servers from the engine (`McpServers` listing, lowered to the UI
    /// `MCPServer` model). Empty ⇒ the settings page keeps its mock list. Refreshed
    /// out-of-band via `refreshMcpServers()` when the MCP settings page opens.
    @Published var mcpServers: [MCPServer] = []
    /// A transient, dim status line (tool activity / connection state). NOT used
    /// for errors anymore — those go to `error` (the persistent banner).
    @Published var statusLine: String? = nil
    /// A persistent, dismissible, kind-aware error banner (PR-4 item 4).
    @Published var error: ConversationError? = nil
    /// A non-clean turn outcome (MaxTurns / Cancelled) surfaced distinctly from a
    /// normal end (PR-4 item 3). Cleared when a new turn starts.
    @Published var notice: TurnNotice? = nil
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
    func send(_ text: String)
    /// Cancel the in-flight turn (PR-4 item 2): the engine submits `.cancel(...)`
    /// and resets streaming state. A no-op when nothing is streaming.
    func cancel()
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
    /// MUST clear any stuck `streaming` flag (and the engine's per-turn
    /// bookkeeping) so a turn parked mid-stream when the user backgrounded the app
    /// is not left "streaming" forever, and persist any state worth restoring.
    /// The in-flight engine turn is cancelled so a late delta can't resurrect it.
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
    func resumeSession(_ uuid: String)
    /// Ask the engine for its real MCP server listing (submit
    /// `RefreshListings(.mcp)`). The reply (`McpServers`) lands out-of-band and
    /// populates `model.mcpServers`. A no-op on the mock (keeps the canned list).
    func refreshMcpServers()
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
        /// Observe out-of-band engine events without duplicating the listener.
        func setExternalEventHandler(_ handler: ((ClientEvent) -> Void)?)
    #endif
}

/// Default `warmUp` for sources with nothing to pre-build (the mock). The engine
/// source overrides it to eagerly build the handle + list models. `handleForeground`
/// is a no-op by default; the engine source may override to refresh state.
extension ConversationSource {
    func warmUp() {}
    func prepare() async throws {}
    func handleForeground() {}
    /// Default session ops for sources with no engine catalog (the mock): no-ops,
    /// so the mock keeps its canned drawer lists and ignores resume requests.
    func listSessions() {}
    func resumeSession(_ uuid: String) {}
    func refreshMcpServers() {}
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
            .failure(message: "当前预览环境没有可用的 Provider 引擎。")
        }
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
                return MockConversationSource.uiTestFixture()
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
    static func appSandboxRoot() -> String {
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

    #if DEBUG
        static func uiTestFixture() -> MockConversationSource {
            let source = MockConversationSource()
            let shell = ConversationShellCard(
                sessionId: "ui-session",
                turnId: 1,
                taskId: "ui-shell",
                command: "pwd",
                cwd: "/workspace/ui-test",
                stdout: "/workspace/ui-test\n",
                stderr: "",
                exitCode: 0,
                durationMs: 42,
                status: .completed,
                truncated: false
            )
            let run = ConversationExecutionRun(
                id: "ui-run",
                sessionId: "ui-session",
                turnId: 1,
                status: .completed,
                reasoning: "检查当前项目工作区。",
                tools: [
                    ConversationToolTrace(
                        id: "ui-shell",
                        tool: "shell",
                        status: .completed,
                        inputSummary: "pwd",
                        outputSummary: "Shell 完成",
                        elapsedMs: 42
                    ),
                ],
                shellCards: [shell],
                usage: ConversationUsageSnapshot(
                    inputTokens: 12,
                    outputTokens: 8,
                    cacheReadTokens: 0,
                    cacheCreationTokens: 0
                )
            )
            source.model.messages = []
            source.model.items = [.run(run)]
            source.model.messageDetails = [:]
            source.model.isNew = false
            return source
        }
    #endif

    func startNewConversation() {
        turnToken &+= 1
        model.messages = []
        model.items = []
        model.messageDetails = [:]
        model.streaming = false
        model.isNew = true
        model.statusLine = nil
        model.error = nil
        model.notice = nil
    }

    func send(_ text: String) {
        // PR-4 item 1: gate overlapping turns on rapid taps.
        guard !model.streaming else { return }
        model.isNew = false
        model.notice = nil
        let message = Message(role: .user, text: text)
        model.messages.append(message)
        model.items.append(.message(message))
        model.streaming = true
        turnToken &+= 1
        let token = turnToken
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.1) { [weak self] in
            guard let self, self.turnToken == token else { return }
            let reply = Message(role: .ai, tag: "思考了 8 秒", text: "已记入。继续追问。")
            self.model.messages.append(reply)
            self.model.items.append(.message(reply))
            self.model.streaming = false
        }
    }

    func cancel() {
        // PR-4 item 2: drop the in-flight canned reply and surface a Cancelled notice.
        guard model.streaming else { return }
        turnToken &+= 1
        model.streaming = false
        model.notice = .cancelled
    }

    func dismissError() { model.error = nil }

    /// Mock model switch: no engine, so just swap the chip from the mock catalog.
    /// The mock never populates `availableModels`, so the picker stays on
    /// `MockData.models` and this id is a mock id.
    func setModel(_ id: String) {
        if let opt = MockData.models.first(where: { $0.id == id }) {
            model.model = opt
        }
    }

    /// Switch sessions: drop the in-flight canned reply (bump the token so its
    /// timer no-ops when it fires) and reset the conversation to the new session's
    /// default transcript. Without the token bump a reply scheduled for the OLD
    /// session would append into the NEW one (the wrong-session bug).
    func openSession(_ session: SessionRef) {
        turnToken &+= 1
        model.messages = MockData.messagesDefault
        model.items = MockData.messagesDefault.map(ConversationRenderItem.message)
        model.messageDetails = [:]
        model.streaming = false
        model.isNew = false
        model.statusLine = nil
        model.error = nil
        model.notice = nil
    }

    /// Background: drop any in-flight canned reply so a turn isn't left
    /// "streaming" forever after the app is backgrounded.
    func handleBackground() {
        guard model.streaming else { return }
        turnToken &+= 1
        model.streaming = false
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

    /// The real conversation source: an in-process engine reached over UniFFI.
    ///
    /// Lifecycle: lazily build the `MobileEngineHandle` on first `send`; register
    /// `EngineListener` (an `IosEventListener`) whose `onEvent(_:)` maps each
    /// `ClientEvent` onto the `ConversationModel`. Turns are submitted via
    /// `handle.submit(.sendPrompt(...))`; the engine streams `TextDelta` /
    /// `ToolUse*` / `TurnEnded` back through the listener.
    @MainActor
    final class EngineConversationSource: ConversationSource {
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
        /// Cancelled / abandoned turns whose matching terminal event has not yet
        /// arrived. While non-empty, a newer prompt is DEFERRED instead of being
        /// submitted, so old/new event streams can never overlap on uncorrelated
        /// protocol variants.
        private var quarantinedTurnIds: [UInt64] = []
        /// A locally-rendered user prompt waiting for the quarantined turn's
        /// terminal event before it is actually submitted to the engine.
        private var pendingPrompt: PendingPrompt?
        private var activeRunItemIndex: Int?
        private var testCommandSubmitter: ((ClientCommand) async throws -> Void)?

        private struct PendingPrompt: Equatable {
            let text: String
            let turnId: UInt64
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
            // Reset the local transcript immediately so the UI reflects a fresh
            // chat without waiting for the engine round-trip. The engine confirms
            // with `SessionStarted` (which re-resets + adopts the new id); doing
            // it here too keeps the UI snappy and correct on the mock-fallback /
            // no-op host path.
            resetTranscriptForSessionSwitch(isNew: true)
            // Tell the engine to begin a fresh session (no cwd/model override —
            // the engine keeps its configured defaults). The new id arrives back
            // out-of-band via `SessionStarted`.
            submitSessionTransition(
                cancelling: turnIdToCancel,
                command: .newSession(cwd: nil, model: nil),
                failurePrefix: "新建会话失败"
            )
        }

        /// Capture the old turn before a session reset clears its correlator.
        /// The caller submits this cancellation and the transition command in
        /// one task, preserving engine-side ordering.
        private func inFlightTurnForSessionSwitch() -> UInt64? {
            guard model.streaming else { return nil }
            return currentTurnId
        }

        private func submitSessionTransition(
            cancelling turnId: UInt64?,
            command: ClientCommand,
            failurePrefix: String
        ) {
            Task { [weak self] in
                guard let self else { return }
                do {
                    if let turnId {
                        try await self.submitCommand(.cancel(turnId: turnId))
                    }
                    try await self.submitCommand(command)
                } catch {
                    await self.fail(.host, "\(failurePrefix)：\(error)")
                }
            }
        }

        private func submitSessionCancellation(_ turnId: UInt64?) {
            guard let turnId else { return }
            Task { [weak self] in
                guard let self else { return }
                do {
                    try await self.submitCommand(.cancel(turnId: turnId))
                } catch {
                    await self.fail(.host, "取消旧会话失败：\(error)")
                }
            }
        }

        /// Shared transcript reset used by new/resume/open-session: clears the
        /// in-flight turn bookkeeping and the visible transcript so a turn that
        /// completes after the switch can't bleed its deltas/notice/permission
        /// into the session we just moved to. `isNew` drives the empty-state vs.
        /// a placeholder transcript.
        private func resetTranscriptForSessionSwitch(isNew: Bool) {
            invalidateTurnContext()
            model.messages = []
            model.items = model.messages.map(ConversationRenderItem.message)
            model.messageDetails = [:]
            model.streaming = false
            model.isNew = isNew
            model.statusLine = nil
            model.error = nil
            model.notice = nil
            // A pending permission belongs to the turn we're abandoning — drop it
            // so a stale prompt can't leak into the session we're switching to.
            model.pendingPermissions = []
        }

        private func invalidateTurnContext() {
            sessionEpoch &+= 1
            streamingIndex = nil
            streamingItemIndex = nil
            currentTurnId = nil
            activeTurnEpoch = nil
            quarantinedTurnIds = []
            pendingPrompt = nil
            activeRunItemIndex = nil
        }

        private func clearTurnPointers(keepEpoch: Bool = true) {
            streamingIndex = nil
            streamingItemIndex = nil
            currentTurnId = nil
            activeTurnEpoch = keepEpoch ? activeTurnEpoch : nil
            activeRunItemIndex = nil
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
            model.messages[streamingIndex] = message
            if let itemIndex = streamingItemIndex,
               model.items.indices.contains(itemIndex) {
                model.items[itemIndex] = .message(message)
            } else if let itemIndex = model.items.firstIndex(where: { item in
                if case let .message(existing) = item {
                    return existing.id == oldMessage.id
                }
                return false
            }) {
                model.items[itemIndex] = .message(message)
                streamingItemIndex = itemIndex
            }
            model.messageDetails.removeValue(forKey: oldMessage.id)
            if let detail {
                model.messageDetails[message.id] = detail
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

        private func acceptTurnEvent(_ event: ClientEvent) -> Bool {
            guard let currentTurnId, let activeTurnEpoch, activeTurnEpoch == sessionEpoch else {
                return false
            }
            if quarantinedTurnIds.contains(currentTurnId), !model.streaming {
                switch event {
                case .turnEnded, .error:
                    return true
                default:
                    return false
                }
            }
            switch event {
            case let .turnStarted(turnId):
                return turnId == nil || turnId == currentTurnId
            default:
                return true
            }
        }

        private func quarantineCurrentTurn(_ turnId: UInt64) {
            guard !quarantinedTurnIds.contains(turnId) else { return }
            quarantinedTurnIds.append(turnId)
        }

        private func consumeQuarantinedTurn(_ turnId: UInt64) {
            quarantinedTurnIds.removeAll { $0 == turnId }
        }

        private func submitCommand(_ command: ClientCommand) async throws {
            if let testCommandSubmitter {
                try await testCommandSubmitter(command)
                return
            }
            let handle = try await ensureHandle()
            try await handle.submit(command: command)
        }

        private func pendingStatusLine() -> String {
            "正在等待上一轮取消完成…"
        }

        private func startPrompt(_ prompt: PendingPrompt) {
            pendingPrompt = nil
            model.notice = nil
            model.streaming = true
            model.statusLine = nil
            streamingIndex = nil
            streamingItemIndex = nil
            currentTurnId = prompt.turnId
            activeTurnEpoch = sessionEpoch

            Task { [weak self] in
                guard let self else { return }
                do {
                    try await self.submitCommand(.sendPrompt(
                        text: prompt.text,
                        promptMode: nil,
                        images: [],
                        turnId: prompt.turnId))
                } catch {
                    await self.fail(.host, "\(error)")
                }
            }
        }

        private func finishQuarantinedTurnAndStartPendingIfNeeded() {
            let completedTurnId = currentTurnId
            if let completedTurnId {
                consumeQuarantinedTurn(completedTurnId)
            }
            clearTurnPointers(keepEpoch: false)
            model.statusLine = nil
            guard quarantinedTurnIds.isEmpty, let pendingPrompt else { return }
            startPrompt(pendingPrompt)
        }

        func send(_ text: String) {
            // PR-4 item 1: a turn is already in flight — ignore the tap so we
            // never start an overlapping turn (which would corrupt appendDelta's
            // single `streamingIndex`). The Stop button is how you interrupt.
            guard !model.streaming, pendingPrompt == nil else { return }

            model.isNew = false
            model.notice = nil
            appendMessage(Message(role: .user, text: text))

            let turnId = nextTurnId
            nextTurnId &+= 1
            let prompt = PendingPrompt(text: text, turnId: turnId)

            if !quarantinedTurnIds.isEmpty {
                pendingPrompt = prompt
                model.statusLine = pendingStatusLine()
                return
            }
            startPrompt(prompt)
        }

        func cancel() {
            // PR-4 item 2: nothing in flight — no-op.
            guard model.streaming, let turnId = currentTurnId else { return }
            // Optimistically reset streaming state; the engine will also stream a
            // `TurnEnded(.cancelled)` which sets the notice (idempotent).
            model.streaming = false
            streamingIndex = nil
            streamingItemIndex = nil
            quarantineCurrentTurn(turnId)
            model.statusLine = pendingPrompt == nil ? nil : pendingStatusLine()
            updateActiveRun { $0.status = .cancelled }
            Task { [weak self] in
                guard let self else { return }
                do {
                    try await self.submitCommand(.cancel(turnId: turnId))
                } catch {
                    await self.fail(.host, "取消失败：\(error)")
                }
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
                ? .success(message: "\(result.message) · \(result.latencyMs)ms")
                : .failure(message: result.message)
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
                mobileLinux: config.mobileLinux.map(makeIosMobileLinuxConfig)
            )
            let handleBuilder = self.handleBuilder
            handleBuildAttemptID &+= 1
            let attemptID = handleBuildAttemptID
            let buildTask = Task { @MainActor [handleBuilder, launchConfig, listener, permissionSink] in
                let handle = try handleBuilder(launchConfig, listener, permissionSink)
                // Bootstrap listings are part of construction: never publish a
                // handle that failed halfway through initialization.
                try await handle.submit(command: .listModels)
                try await handle.submit(command: .listSessions(limit: nil))
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
                secureStorage: SecureStorageImpl()
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
                catch { await self.fail(.host, "\(error)") }
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
                    try await handle.submit(command: .listSessions(limit: nil))
                } catch {
                    await self.fail(.host, "\(error)")
                }
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
                    await self.fail(.host, "\(error)")
                }
            }
        }

        // MARK: inbound-event application (called on the main actor)

        /// Map one inbound `ClientEvent` onto the published state.
        fileprivate func apply(_ event: ClientEvent) {
            externalEventHandler?(event)
            switch event {
            case .turnStarted:
                guard acceptTurnEvent(event) else { return }
                model.streaming = true
                model.notice = nil
                streamingIndex = nil
                streamingItemIndex = nil
                updateActiveRun {
                    $0.status = .running
                    $0.retry = nil
                }

            case let .textDelta(text):
                guard acceptTurnEvent(event) else { return }
                appendDelta(text)

            case let .thinkingDelta(thinking, signature):
                guard acceptTurnEvent(event) else { return }
                updateActiveRun { run in
                    run.reasoning += thinking
                    if signature != nil && run.notices.contains(where: { $0.id == "thinking-signature" }) == false {
                        run.notices.append(.init(id: "thinking-signature", kind: .info, text: "推理签名已附加"))
                    }
                }

            case let .systemNotice(message, isError):
                guard acceptTurnEvent(event) else { return }
                let notice = ConversationExecutionNotice(
                    id: "notice-\(UUID().uuidString)",
                    kind: isError ? .error : .info,
                    text: message
                )
                updateActiveRun { $0.notices.append(notice) }
                model.statusLine = message

            case let .toolUseStarted(id, tool, inputJson):
                guard acceptTurnEvent(event) else { return }
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
                    }
                    model.statusLine = "Shell 运行中…"
                } else {
                    let summary = ConversationExecutionParsing.summarizeToolInput(inputJson)
                    upsertTool(id: id, tool: tool, fallbackSummary: summary) { trace in
                        trace.tool = tool
                        trace.status = .running
                        trace.inputSummary = summary
                    }
                    model.statusLine = "调用工具 \(tool)…"
                }

            case let .toolHeartbeat(id, tool, elapsedMs):
                guard acceptTurnEvent(event) else { return }
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
                    model.statusLine = "Shell 运行中…"
                } else {
                    upsertTool(id: id, tool: tool, fallbackSummary: nil) { trace in
                        trace.tool = tool
                        trace.status = .running
                        trace.elapsedMs = elapsedMs
                    }
                    model.statusLine = "工具 \(tool) 运行中…"
                }

            case let .toolUseResult(id, tool, resultJson, isError):
                guard acceptTurnEvent(event) else { return }
                if ConversationExecutionParsing.isShellTool(tool) {
                    let finished = ConversationExecutionParsing.shellFinished(id: id, resultJson: resultJson, isError: isError)
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
                            card.status = finished.status
                            card.truncated = finished.truncated
                        } else {
                            card.status = isError ? .failed : .completed
                        }
                    })
                    upsertTool(id: id, tool: "Shell", fallbackSummary: nil) { trace in
                        trace.tool = "Shell"
                        trace.status = finished?.status.asToolStatus ?? (isError ? .failed : .completed)
                        trace.outputSummary = ConversationExecutionParsing.summarizeToolResult(resultJson, isError: isError, tool: tool)
                        trace.elapsedMs = finished?.durationMs ?? trace.elapsedMs
                    }
                    model.statusLine = ConversationExecutionParsing.shellStatusLabel(
                        finished?.status ?? (isError ? .failed : .completed)
                    )
                } else {
                    upsertTool(id: id, tool: tool, fallbackSummary: nil) { trace in
                        trace.tool = tool
                        trace.status = isError ? .failed : .completed
                        trace.outputSummary = ConversationExecutionParsing.summarizeToolResult(
                            resultJson,
                            isError: isError,
                            tool: tool
                        )
                    }
                    model.statusLine = isError ? "工具 \(tool) 失败" : "工具 \(tool) 完成"
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
                model.statusLine = "重试中（\(attempt)/\(maxRetries)）…"

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
                updateActiveRun {
                    $0.activeWorkers = activeWorkers
                    $0.coordinatorTeam = team
                }

            case let .coordinatorWorker(worker):
                guard acceptTurnEvent(event) else { return }
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
                    let lowered = Self.message(from: message)
                    replaceStreamingMessage(lowered.message, detail: lowered.detail)
                }

            case let .turnEnded(outcome, _, _):
                guard acceptTurnEvent(event) else { return }
                let completedQuarantinedTurn = currentTurnId.map { quarantinedTurnIds.contains($0) } ?? false
                // PR-4 item 3: don't treat every outcome as a clean end. A normal
                // `endTurn` just stops streaming; `maxTurns` / `cancelled` surface
                // a distinct notice so the user knows the turn was interrupted.
                model.streaming = false
                streamingIndex = nil
                streamingItemIndex = nil
                model.statusLine = nil
                switch outcome {
                case .endTurn:
                    model.notice = nil
                    updateActiveRun { $0.status = .completed }
                case .maxTurns:
                    model.notice = .maxTurns
                    updateActiveRun { $0.status = .maxTurns }
                case .cancelled:
                    model.notice = completedQuarantinedTurn && pendingPrompt != nil ? nil : .cancelled
                    updateActiveRun { $0.status = .cancelled }
                @unknown default:
                    // `#[non_exhaustive]` — a future outcome falls back to a clean
                    // end rather than crashing.
                    model.notice = nil
                }
                if completedQuarantinedTurn {
                    finishQuarantinedTurnAndStartPendingIfNeeded()
                    return
                }
                clearTurnPointers(keepEpoch: false)

            case let .error(kind, message):
                guard acceptTurnEvent(event) else { return }
                let completedQuarantinedTurn = currentTurnId.map { quarantinedTurnIds.contains($0) } ?? false
                if completedQuarantinedTurn {
                    updateActiveRun { $0.status = .failed }
                    finishQuarantinedTurnAndStartPendingIfNeeded()
                    return
                }
                // PR-4 item 4: a terminal error is a persistent, kind-aware banner.
                updateActiveRun { $0.status = .failed }
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

            case let .sessionStarted(sessionId):
                // A fresh session began on the connection (1:1 with a successful
                // `NewSession`). Adopt it as active. Reset the transcript ONLY
                // when this is genuinely a new id AND no turn is streaming — so an
                // unexpected `SessionStarted` (e.g. one emitted for the initial
                // session at connect time) can never wipe a live conversation.
                // `startNewConversation` already reset locally for the user-driven
                // case; this confirms + adopts the engine-assigned id.
                let isSwitch = !sessionId.isEmpty && sessionId != model.activeSessionId
                model.activeSessionId = sessionId
                if isSwitch && !model.streaming {
                    resetTranscriptForSessionSwitch(isNew: true)
                    model.activeSessionId = sessionId
                }

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
                invalidateTurnContext()
                model.activeSessionId = sessionId
                let restored = messages.map(Self.message(from:))
                model.messages = restored.map(\.message)
                model.items = restored.map { .message($0.message) }
                model.messageDetails = Dictionary(
                    uniqueKeysWithValues: restored.compactMap { item in
                        item.detail.map { (item.message.id, $0) }
                    }
                )
                model.isNew = messages.isEmpty
                model.streaming = false
                model.statusLine = nil
                model.notice = nil

            case .sessionEnded:
                // The current session ended (e.g. cleared). Drop the active id;
                // the next `SessionStarted`/`SessionResumed` re-establishes one.
                model.activeSessionId = ""
                invalidateTurnContext()

            case let .mcpServers(servers):
                // Out-of-band MCP listing → the UI `MCPServer` model. The DTO is
                // thinner than the mock (no url / tool-count), so those default;
                // status maps Connected→connected, Disconnected→idle, Error→error.
                model.mcpServers = servers.map { dto in
                    let status: ConnStatus
                    switch dto.status {
                    case .connected: status = .connected
                    case .disconnected: status = .idle
                    case .error: status = .error
                    @unknown default: status = .idle
                    }
                    return MCPServer(id: dto.name, name: dto.name, url: "", tools: 0,
                                     status: status, enabled: status == .connected, transport: dto.transport)
                }

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

        func beginTurnForTesting(turnId: UInt64 = 1, sessionId: String = "test-session") {
            model.activeSessionId = sessionId
            model.streaming = true
            currentTurnId = turnId
            activeTurnEpoch = sessionEpoch
            nextTurnId = max(nextTurnId, turnId &+ 1)
        }

        func cancelForTesting() {
            guard model.streaming, let turnId = currentTurnId else { return }
            model.streaming = false
            streamingIndex = nil
            streamingItemIndex = nil
            quarantineCurrentTurn(turnId)
            model.statusLine = pendingPrompt == nil ? nil : pendingStatusLine()
            updateActiveRun { $0.status = .cancelled }
        }

        func setCommandSubmitterForTesting(_ submitter: ((ClientCommand) async throws -> Void)?) {
            testCommandSubmitter = submitter
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

        fileprivate static func message(from dto: MessageDto) -> RenderedMessage {
            let role: Role = (dto.role == "user") ? .user : .ai
            let detail = detail(from: dto.blocks)
            let body = text(from: detail?.blocks ?? [])
            return RenderedMessage(message: Message(role: role, text: body), detail: detail)
        }

        private static func detail(from blocks: [MessageBlockDto]) -> ConversationMessageDetail? {
            let lowered = blocks.compactMap { block -> ConversationMessageBlock? in
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
                case let .toolUse(id, tool, inputJson):
                    return .toolUse(
                        id: id,
                        tool: tool,
                        inputSummary: ConversationExecutionParsing.summarizeToolInput(inputJson) ?? inputJson,
                        inputJson: inputJson
                    )
                case let .toolResult(id, tool, resultJson, isError, oldString, newString, filePath):
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
                        filePath: filePath
                    )
                @unknown default:
                    return nil
                }
            }
            return lowered.isEmpty ? nil : ConversationMessageDetail(blocks: lowered)
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
                    return "[已折叠的思考]"
                case .compactBoundary:
                    return "对话已压缩"
                case let .toolUse(_, tool, _, _):
                    return "调用工具 \(tool)…"
                case let .toolResult(_, _, isError, summary, _, _, _, _):
                    return isError ? summary : "工具结果：\(summary)"
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
            model.error = ConversationError(kind: kind, message: message)
            model.streaming = false
            model.statusLine = nil
            // A terminal error tears down the turn — its parked permission (if any)
            // can never be answered now, so drop the prompt rather than leave it
            // stranded.
            model.pendingPermissions = []
            clearTurnPointers(keepEpoch: false)
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
                    await self.fail(.host, "切换模型失败：\(error)")
                }
            }
        }

        // MARK: session + lifecycle

        /// Switch the active conversation to another session (iOS analog of
        /// Android `ChatViewModel.openSession`). Cancels the in-flight turn FIRST
        /// — clearing `streaming` / `streamingIndex` / `currentTurnId` and
        /// submitting `.cancel(turnId:)` — so a turn that completes after the
        /// switch can't append its deltas (or its `TurnEnded` notice) into the new
        /// session. Then resets the transcript to the session's default.
        func openSession(_ session: SessionRef) {
            let turnIdToCancel = inFlightTurnForSessionSwitch()
            resetTranscriptForSessionSwitch(isNew: false)
            submitSessionCancellation(turnIdToCancel)
        }

        /// Resume a prior engine session by UUID (the drawer-tap path for a REAL
        /// history row). Cancels any in-flight turn FIRST and resets the
        /// transcript to a placeholder (so a late delta can't bleed into the
        /// resumed session), optimistically marks the row active so the UI
        /// reflects the choice immediately, then submits `ResumeSession`. The
        /// engine hot-restores the prior transcript into the running orchestrator
        /// and confirms with `SessionResumed{session_id, messages}`, which
        /// re-adopts the id AND replaces the placeholder with the real restored
        /// conversation (oldest-first) so the next turn continues with full prior
        /// context visible. No-op when already active.
        func resumeSession(_ uuid: String) {
            guard !uuid.isEmpty, uuid != model.activeSessionId else { return }
            let turnIdToCancel = inFlightTurnForSessionSwitch()
            resetTranscriptForSessionSwitch(isNew: false)
            // Optimistic local select (the task's "select it locally" requirement)
            // so the drawer marks the row even if engine-side resume is a
            // follow-up; `SessionResumed` confirms the same id.
            model.activeSessionId = uuid
            submitSessionTransition(
                cancelling: turnIdToCancel,
                command: .resumeSession(sessionId: uuid, cwd: nil),
                failurePrefix: "恢复会话失败"
            )
        }

        /// Background: the user left the app while a turn was streaming. Cancel the
        /// in-flight turn so it isn't left "streaming" forever and a late delta
        /// can't resurrect it; the cancel narrows to `currentTurnId`. Idempotent /
        /// no-op when nothing is in flight.
        func handleBackground() {
            guard model.streaming else { return }
            cancel()
        }

        // MARK: permission gating (SHIP-BLOCKER #3)

        /// Enqueue one outbound permission request (called on the main actor by the
        /// sink). De-dupes by `requestId` so a re-delivered request can't stack two
        /// prompts. The chat view renders the head of the queue.
        fileprivate func enqueuePermission(_ request: PermissionRequest) {
            guard !model.pendingPermissions.contains(where: { $0.requestId == request.requestId })
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
                    await self.fail(.host, "权限响应失败：\(error)")
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
