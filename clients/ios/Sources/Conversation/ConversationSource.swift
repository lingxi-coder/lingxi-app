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
    /// The engine session id currently driving the connection — set by
    /// `SessionStarted` / `SessionResumed`. Empty until the engine reports one.
    @Published var activeSessionId: String = ""
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

    init(messages: [Message] = MockData.messagesDefault,
         model: ModelOption = MockData.models[0]) {
        self.messages = messages
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
    #if canImport(engine_mobileFFI)
        /// Resolve a parked permission request (SHIP-BLOCKER #3): submit
        /// `ApprovePermission{requestId, response}` and pop the head of the queue.
        func approvePermission(_ requestId: UInt64, _ response: PermissionResponseDto)
        /// Resolve a parked permission request by denying it: submit
        /// `DenyPermission{requestId}` and pop the head of the queue.
        func denyPermission(_ requestId: UInt64)
    #endif
}

/// Default `warmUp` for sources with nothing to pre-build (the mock). The engine
/// source overrides it to eagerly build the handle + list models. `handleForeground`
/// is a no-op by default; the engine source may override to refresh state.
extension ConversationSource {
    func warmUp() {}
    func handleForeground() {}
    /// Default session ops for sources with no engine catalog (the mock): no-ops,
    /// so the mock keeps its canned drawer lists and ignores resume requests.
    func listSessions() {}
    func resumeSession(_ uuid: String) {}
}

#if canImport(engine_mobileFFI)
    /// Default permission handling for sources that never park a turn on a
    /// permission gate (the mock). The engine source overrides both.
    extension ConversationSource {
        func approvePermission(_ requestId: UInt64, _ response: PermissionResponseDto) {}
        func denyPermission(_ requestId: UInt64) {}
    }
#endif

// MARK: - Source selection

/// Chooses the conversation source at app start. Prefers the real in-process
/// engine (over UniFFI) when the bindings are linked AND the engine is opted in;
/// otherwise the canned mock. Falling back to the mock keeps the app usable in
/// preview / no-key environments.
///
/// Opt-in (any one suffices), in priority order:
///   1. A key stored in the Keychain (SHIP-BLOCKER #1) — the shipped-app path: a
///      user pasting their key in Settings is enough, no env needed.
///   2. `LINGXI_USE_ENGINE=1` in the environment (dev/CI explicit opt-in).
///   3. `ANTHROPIC_API_KEY` present in the environment (dev convenience).
@MainActor
enum ConversationSourceFactory {
    static func make() -> any ConversationSource {
        #if canImport(engine_mobileFFI)
            let env = ProcessInfo.processInfo.environment
            let hasKeychainKey = !(Keychain.get(.apiKey) ?? "").isEmpty
            let optedIn = hasKeychainKey
                || env["LINGXI_USE_ENGINE"] == "1"
                || !(env["ANTHROPIC_API_KEY"] ?? "").isEmpty
            if optedIn {
                let root = appSandboxRoot()
                // SHIP-BLOCKER #2: NEVER seed the engine with a branded mock id
                // ("lx-72b" → Anthropic 400). Use the user's last-picked real model
                // from the Keychain when set; otherwise pass "" so `buildIosEngine`
                // falls back to `MobileConfig.default_model` (a real Anthropic wire
                // id). `fromEnvironment` still lets `LINGXI_MODEL` override for dev.
                let storedModel = Keychain.get(.model) ?? ""
                let config = EngineConfig.fromEnvironment(
                    appSandboxRoot: root, model: storedModel)
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
    let model = ConversationModel()

    /// Bumped on cancel / new-chat so an in-flight canned reply timer no-ops when
    /// it fires (the mock's analog of the engine's cancel token).
    private var turnToken = 0

    func startNewConversation() {
        turnToken &+= 1
        model.messages = []
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
        model.messages.append(Message(role: .user, text: text))
        model.streaming = true
        turnToken &+= 1
        let token = turnToken
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.1) { [weak self] in
            guard let self, self.turnToken == token else { return }
            self.model.messages.append(Message(role: .ai, tag: "思考了 8 秒", text: "已记入。继续追问。"))
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

        /// Resolve the engine credentials. The API key (and optional base URL)
        /// come from the iOS Keychain FIRST (SHIP-BLOCKER #1 — a shipped app has no
        /// process env), with an environment override for development/CI so a
        /// `ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` in the env still wins for a
        /// dev run. An empty key is valid (turns 401 at run time, slash commands
        /// still work) and keeps the mock fallback in `make()`.
        static func fromEnvironment(appSandboxRoot: String,
                                    model: String) -> EngineConfig {
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
                appSandboxRoot: appSandboxRoot
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
        let model: ConversationModel

        private let config: EngineConfig
        private var handle: MobileEngineHandle?
        private var listener: EngineListener?
        /// The permission sink registered with the engine (SHIP-BLOCKER #3). Held so
        /// it outlives `ensureHandle`; Rust calls `onRequest` on it when a tool needs
        /// approval.
        private var permissionSink: EnginePermissionSink?
        /// Index into `model.messages` of the assistant message currently being
        /// streamed (deltas append into it). `nil` between turns.
        private var streamingIndex: Int?
        /// Monotonic per-turn correlator, also passed as the engine `turnId` so a
        /// `cancel` narrows to the exact in-flight turn. `nil` between turns.
        private var currentTurnId: UInt64?
        private var nextTurnId: UInt64 = 1

        init(config: EngineConfig) {
            self.config = config
            // Seed the chip from the mock catalog only as a placeholder until the
            // engine's `ModelList` lands (SHIP-BLOCKER #2). The REAL active model is
            // `activeModelId`, set below from the (possibly empty) configured id and
            // then authoritatively replaced by `ModelList.current` / `ModelChanged`.
            self.model = ConversationModel(model: MockData.models.first(where: { $0.id == config.model })
                ?? MockData.models[0])
            // Out-of-band model state: the configured id (empty ⇒ engine default,
            // filled by the first `ModelList`). Never a branded mock id here.
            self.model.activeModelId = config.model
        }

        // MARK: ConversationSource

        func startNewConversation() {
            // Reset the local transcript immediately so the UI reflects a fresh
            // chat without waiting for the engine round-trip. The engine confirms
            // with `SessionStarted` (which re-resets + adopts the new id); doing
            // it here too keeps the UI snappy and correct on the mock-fallback /
            // no-op host path.
            resetTranscriptForSessionSwitch(isNew: true)
            // Tell the engine to begin a fresh session (no cwd/model override —
            // the engine keeps its configured defaults). The new id arrives back
            // out-of-band via `SessionStarted`.
            Task { [weak self] in
                guard let self else { return }
                do {
                    let handle = try await self.ensureHandle()
                    try await handle.submit(command: .newSession(cwd: nil, model: nil))
                } catch {
                    await self.fail(.host, "新建会话失败：\(error)")
                }
            }
        }

        /// Shared transcript reset used by new/resume/open-session: clears the
        /// in-flight turn bookkeeping and the visible transcript so a turn that
        /// completes after the switch can't bleed its deltas/notice/permission
        /// into the session we just moved to. `isNew` drives the empty-state vs.
        /// a placeholder transcript.
        private func resetTranscriptForSessionSwitch(isNew: Bool) {
            model.messages = isNew ? [] : MockData.messagesDefault
            model.streaming = false
            model.isNew = isNew
            model.statusLine = nil
            model.error = nil
            model.notice = nil
            streamingIndex = nil
            currentTurnId = nil
            // A pending permission belongs to the turn we're abandoning — drop it
            // so a stale prompt can't leak into the session we're switching to.
            model.pendingPermissions = []
        }

        func send(_ text: String) {
            // PR-4 item 1: a turn is already in flight — ignore the tap so we
            // never start an overlapping turn (which would corrupt appendDelta's
            // single `streamingIndex`). The Stop button is how you interrupt.
            guard !model.streaming else { return }

            model.isNew = false
            model.notice = nil
            model.messages.append(Message(role: .user, text: text))
            model.streaming = true
            streamingIndex = nil

            let turnId = nextTurnId
            nextTurnId &+= 1
            currentTurnId = turnId

            Task { [weak self] in
                guard let self else { return }
                do {
                    let handle = try await self.ensureHandle()
                    try await handle.submit(command: .sendPrompt(
                        text: text, promptMode: nil, images: [], turnId: turnId))
                } catch {
                    await self.fail(.host, "\(error)")
                }
            }
        }

        func cancel() {
            // PR-4 item 2: nothing in flight — no-op.
            guard model.streaming, let turnId = currentTurnId else { return }
            // Optimistically reset streaming state; the engine will also stream a
            // `TurnEnded(.cancelled)` which sets the notice (idempotent).
            model.streaming = false
            streamingIndex = nil
            Task { [weak self] in
                guard let self else { return }
                do {
                    let handle = try await self.ensureHandle()
                    try await handle.submit(command: .cancel(turnId: turnId))
                } catch {
                    await self.fail(.host, "取消失败：\(error)")
                }
            }
        }

        func dismissError() { model.error = nil }

        // MARK: handle construction

        /// Build the engine handle once (lazily). Registers the listener so the
        /// adapter can stream events the moment the first turn runs.
        private func ensureHandle() async throws -> MobileEngineHandle {
            if let handle { return handle }
            let listener = EngineListener(source: self)
            self.listener = listener
            // SHIP-BLOCKER #3: register a real permission sink so a tool that needs
            // approval surfaces a prompt instead of hanging the turn forever.
            let permissionSink = EnginePermissionSink(source: self)
            self.permissionSink = permissionSink
            let handle = try buildIosEngine(
                apiBase: config.apiBase,
                apiKey: config.apiKey,
                model: config.model,
                appSandboxRoot: config.appSandboxRoot,
                listener: listener,
                stt: SttImpl(),
                tts: TtsImpl(),
                camera: CameraImpl(),
                share: ShareImpl(),
                voice: VoiceImpl(),
                notifications: NotificationImpl(),
                clipboard: ClipboardImpl(),
                permissions: permissionSink)
            self.handle = handle
            // SHIP-BLOCKER #2: ask the engine for its real model catalog the moment
            // the handle exists. The reply (`ModelList`) arrives out-of-band on the
            // listener and populates `availableModels` / `activeModelId` — driving
            // the picker off real ids, not the branded mock catalog. This is
            // out-of-band model state, NOT part of any text turn.
            try await handle.submit(command: .listModels)
            // Same out-of-band pattern for the session catalog: ask the engine
            // for its real resumable sessions the moment the handle exists. The
            // reply (`SessionList`) arrives on the listener and populates
            // `engineSessions`, driving the drawer off real history rather than
            // the mock lists. NOT part of any text turn.
            try await handle.submit(command: .listSessions(limit: nil))
            return handle
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

        // MARK: inbound-event application (called on the main actor)

        /// Map one inbound `ClientEvent` onto the published state.
        fileprivate func apply(_ event: ClientEvent) {
            switch event {
            case .turnStarted:
                model.streaming = true
                model.notice = nil
                streamingIndex = nil

            case let .textDelta(text):
                appendDelta(text)

            case let .toolUseStarted(_, tool, _):
                // Surface tool activity as a dim status line; full tool cards are
                // later parity work (spec §5 item 3).
                model.statusLine = "调用工具 \(tool)…"

            case let .toolUseResult(_, tool, _, isError):
                model.statusLine = isError ? "工具 \(tool) 失败" : nil

            case let .turnEnded(outcome, _, _):
                // PR-4 item 3: don't treat every outcome as a clean end. A normal
                // `endTurn` just stops streaming; `maxTurns` / `cancelled` surface
                // a distinct notice so the user knows the turn was interrupted.
                model.streaming = false
                streamingIndex = nil
                currentTurnId = nil
                model.statusLine = nil
                switch outcome {
                case .endTurn:
                    model.notice = nil
                case .maxTurns:
                    model.notice = .maxTurns
                case .cancelled:
                    model.notice = .cancelled
                @unknown default:
                    // `#[non_exhaustive]` — a future outcome falls back to a clean
                    // end rather than crashing.
                    model.notice = nil
                }

            case let .error(kind, message):
                // PR-4 item 4: a terminal error is a persistent, kind-aware banner.
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
                }

            case let .sessionResumed(sessionId):
                // A prior session was resumed (1:1 with a successful
                // `ResumeSession`). Adopt it as active; the transcript was already
                // reset by `resumeSession`, so just track the id here.
                model.activeSessionId = sessionId

            case .sessionEnded:
                // The current session ended (e.g. cleared). Drop the active id;
                // the next `SessionStarted`/`SessionResumed` re-establishes one.
                model.activeSessionId = ""

            default:
                // Cost / message-boundary / listing events are not rendered in the
                // P3a conversation surface; ignored without breaking the stream.
                break
            }
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
                model.messages[i] = Message(role: .ai,
                                            tag: model.messages[i].tag,
                                            text: model.messages[i].text + delta)
            } else {
                model.messages.append(Message(role: .ai, text: delta))
                streamingIndex = model.messages.count - 1
            }
        }

        /// Map the lowered `ErrorKindDto` onto the UI-facing `ConversationError.Kind`.
        private static func kind(from dto: ErrorKindDto) -> ConversationError.Kind {
            switch dto {
            case .transport: return .transport
            case .protocol: return .protocol
            case .server: return .server
            case .maxTurns: return .maxTurns
            case .internal: return .internal
            @unknown default: return .internal
            }
        }

        private func fail(_ kind: ConversationError.Kind, _ message: String) {
            model.error = ConversationError(kind: kind, message: message)
            model.streaming = false
            streamingIndex = nil
            currentTurnId = nil
            model.statusLine = nil
            // A terminal error tears down the turn — its parked permission (if any)
            // can never be answered now, so drop the prompt rather than leave it
            // stranded.
            model.pendingPermissions = []
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
            // Cancel the in-flight turn on the engine so its late deltas/outcome
            // can't bleed into the new session. `cancel()` is a no-op if idle.
            cancel()
            model.messages = MockData.messagesDefault
            model.streaming = false
            model.isNew = false
            model.statusLine = nil
            model.error = nil
            model.notice = nil
            streamingIndex = nil
            currentTurnId = nil
            // A parked permission belongs to the turn we're leaving — drop it so a
            // stale prompt can't leak into the session we just switched to.
            model.pendingPermissions = []
        }

        /// Resume a prior engine session by UUID (the drawer-tap path for a REAL
        /// history row). Cancels any in-flight turn FIRST and resets the
        /// transcript (so a late delta can't bleed into the resumed session),
        /// optimistically marks the row active so the UI reflects the choice
        /// immediately, then submits `ResumeSession`. The engine confirms with
        /// `SessionResumed`, which re-adopts the id. No-op when already active.
        func resumeSession(_ uuid: String) {
            guard !uuid.isEmpty, uuid != model.activeSessionId else { return }
            cancel()
            resetTranscriptForSessionSwitch(isNew: false)
            // Optimistic local select (the task's "select it locally" requirement)
            // so the drawer marks the row even if engine-side resume is a
            // follow-up; `SessionResumed` confirms the same id.
            model.activeSessionId = uuid
            Task { [weak self] in
                guard let self else { return }
                do {
                    let handle = try await self.ensureHandle()
                    try await handle.submit(command: .resumeSession(sessionId: uuid, cwd: nil))
                } catch {
                    await self.fail(.host, "恢复会话失败：\(error)")
                }
            }
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
