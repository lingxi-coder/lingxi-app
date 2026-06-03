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

/// The observable conversation state ChatView renders. Both sources mutate it on
/// the main actor: the mock with canned timers, the engine from listener events.
@MainActor
final class ConversationModel: ObservableObject {
    /// The full visible transcript (user + assistant turns).
    @Published var messages: [Message]
    /// True while a turn is in flight (drives the streaming dots row).
    @Published var streaming: Bool = false
    /// True when the session is brand-new and empty (drives the empty state).
    @Published var isNew: Bool = false
    /// The currently selected model chip.
    @Published var model: ModelOption
    /// A transient, user-visible status line (engine errors, connection state).
    @Published var statusLine: String? = nil

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
    func send(_ text: String)
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

    func startNewConversation() {
        model.messages = []
        model.streaming = false
        model.isNew = true
        model.statusLine = nil
    }

    func send(_ text: String) {
        model.isNew = false
        model.messages.append(Message(role: .user, text: text))
        model.streaming = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.1) { [model] in
            model.messages.append(Message(role: .ai, tag: "思考了 8 秒", text: "已记入。继续追问。"))
            model.streaming = false
        }
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
            streamingIndex = nil
        }

        func send(_ text: String) {
            model.isNew = false
            model.messages.append(Message(role: .user, text: text))
            model.streaming = true
            streamingIndex = nil

            Task { [weak self] in
                guard let self else { return }
                do {
                    let handle = try await self.ensureHandle()
                    try await handle.submit(command: .sendPrompt(
                        text: text, promptMode: nil, images: [], turnId: nil))
                } catch {
                    await self.fail("引擎错误：\(error)")
                }
            }
        }

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
                listener: listener)
            self.handle = handle
            return handle
        }

        // MARK: inbound-event application (called on the main actor)

        /// Map one inbound `ClientEvent` onto the published state.
        fileprivate func apply(_ event: ClientEvent) {
            switch event {
            case .turnStarted:
                model.streaming = true
                streamingIndex = nil

            case let .textDelta(text):
                appendDelta(text)

            case let .toolUseStarted(_, tool, _):
                // Surface tool activity as a dim status line; full tool cards are
                // later parity work (spec §5 item 3).
                model.statusLine = "调用工具 \(tool)…"

            case let .toolUseResult(_, tool, _, isError):
                model.statusLine = isError ? "工具 \(tool) 失败" : nil

            case let .turnEnded(_, _, _):
                model.streaming = false
                streamingIndex = nil

            case let .error(_, message):
                model.statusLine = "错误：\(message)"
                model.streaming = false
                streamingIndex = nil

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
        private func appendDelta(_ delta: String) {
            if let i = streamingIndex, model.messages.indices.contains(i) {
                model.messages[i] = Message(role: .ai,
                                            tag: model.messages[i].tag,
                                            text: model.messages[i].text + delta)
            } else {
                model.messages.append(Message(role: .ai, text: delta))
                streamingIndex = model.messages.count - 1
            }
        }

        private func fail(_ message: String) {
            model.statusLine = message
            model.streaming = false
            streamingIndex = nil
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
