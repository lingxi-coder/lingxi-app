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
// SECRETS: the LLM API key is read from the runtime environment
// (`ANTHROPIC_API_KEY`) / an app setting — see `EngineConfig.fromEnvironment`.
// It is NEVER hardcoded, logged, or persisted here.

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
    /// A transient, dim status line (tool activity / connection state). NOT used
    /// for errors anymore — those go to `error` (the persistent banner).
    @Published var statusLine: String? = nil
    /// A persistent, dismissible, kind-aware error banner (PR-4 item 4).
    @Published var error: ConversationError? = nil
    /// A non-clean turn outcome (MaxTurns / Cancelled) surfaced distinctly from a
    /// normal end (PR-4 item 3). Cleared when a new turn starts.
    @Published var notice: TurnNotice? = nil

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
}

// MARK: - Source selection

/// Chooses the conversation source at app start. Prefers the real in-process
/// engine (over UniFFI) when the bindings are linked AND the engine is opted in
/// (`LINGXI_USE_ENGINE=1`, or an `ANTHROPIC_API_KEY` present in the environment);
/// otherwise the canned mock. Falling back to the mock keeps the app usable in
/// preview / no-key environments.
@MainActor
enum ConversationSourceFactory {
    static func make() -> any ConversationSource {
        #if canImport(engine_mobileFFI)
            let env = ProcessInfo.processInfo.environment
            let optedIn = env["LINGXI_USE_ENGINE"] == "1" || !(env["ANTHROPIC_API_KEY"] ?? "").isEmpty
            if optedIn {
                let root = appSandboxRoot()
                let config = EngineConfig.fromEnvironment(
                    appSandboxRoot: root, model: MockData.models[0].id)
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

        /// Build from the process environment + the app sandbox. `ANTHROPIC_API_KEY`
        /// (optionally `ANTHROPIC_BASE_URL` / `LINGXI_MODEL`) drives the engine; an
        /// empty key is valid (turns 401 at run time, slash commands still work).
        static func fromEnvironment(appSandboxRoot: String,
                                    model: String) -> EngineConfig {
            let env = ProcessInfo.processInfo.environment
            return EngineConfig(
                apiBase: env["ANTHROPIC_BASE_URL"] ?? "https://api.anthropic.com",
                apiKey: env["ANTHROPIC_API_KEY"] ?? "",
                model: env["LINGXI_MODEL"] ?? model,
                appSandboxRoot: appSandboxRoot
            )
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
        /// Index into `model.messages` of the assistant message currently being
        /// streamed (deltas append into it). `nil` between turns.
        private var streamingIndex: Int?
        /// Monotonic per-turn correlator, also passed as the engine `turnId` so a
        /// `cancel` narrows to the exact in-flight turn. `nil` between turns.
        private var currentTurnId: UInt64?
        private var nextTurnId: UInt64 = 1

        init(config: EngineConfig) {
            self.config = config
            self.model = ConversationModel(model: MockData.models.first(where: { $0.id == config.model })
                ?? MockData.models[0])
        }

        // MARK: ConversationSource

        func startNewConversation() {
            model.messages = []
            model.streaming = false
            model.isNew = true
            model.statusLine = nil
            model.error = nil
            model.notice = nil
            streamingIndex = nil
            currentTurnId = nil
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
                clipboard: ClipboardImpl())
            self.handle = handle
            return handle
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

            case let .modelChanged(model: newModel):
                if let opt = MockData.models.first(where: { $0.id == newModel || $0.name == newModel }) {
                    model.model = opt
                }

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
