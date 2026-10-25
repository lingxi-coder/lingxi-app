import Combine
import Foundation
import OSLog
import SwiftUI

#if canImport(harness_runtimeFFI)
import AuthenticationServices
import UIKit
import harness_runtimeFFI
#endif


/// The pre-P3a canned behavior, lifted behind the seam verbatim: append the
/// user message, show streaming dots, then append a fixed assistant reply.
@MainActor
final class MockConversationSource: ConversationSource {
    let model = ConversationModel(messages: MockData.messagesDefault)
    let sessionMode: SessionMode
    private var cannedReplyDelay: TimeInterval = 1.1
    #if canImport(harness_runtimeFFI)
        private var mockProviderCatalog: [ProviderCatalogEntry] = []
    #endif

    /// Bumped on cancel / new-chat so an in-flight canned reply timer no-ops when
    /// it fires (the mock's analog of the engine's cancel token).
    private var turnToken = 0
    private var nextTurnId: UInt64 = 1
    private var sessionEpoch: UInt64 = 1
    private var activeTurnToken: ConversationTurnToken?
    private var turnSpeechSequence: UInt64 = 0

    init(sessionMode: SessionMode = .code) {
        self.sessionMode = sessionMode
    }

    #if DEBUG
        static func uiTestFixture(
            sessionMode: SessionMode = .code,
            cancelledRun: Bool = false,
            multiAgent: Bool = false,
            holdTurn: Bool = false,
            askQuestion: Bool = false
        ) -> MockConversationSource {
            let source = MockConversationSource(sessionMode: sessionMode)
            if let message = ProcessInfo.processInfo.environment["LINGXI_UI_TEST_RUNTIME_ERROR"] {
                source.model.error = ConversationError(kind: .host, message: message)
            }
            if let fixture = ProcessInfo.processInfo.environment["LINGXI_UI_TEST_FILES"],
               fixture.hasPrefix("ui-files-"), fixture.allSatisfy({ $0.isLetter || $0.isNumber || $0 == "-" }) {
                let paths = WorkspaceDirectoryPaths.current()
                let directory = paths.home.appendingPathComponent(fixture, isDirectory: true)
                do {
                    try FileManager.default.createDirectory(at: directory.appendingPathComponent("target"), withIntermediateDirectories: true)
                    try Data("Read-only file preview ✓".utf8).write(to: directory.appendingPathComponent("note2.txt"))
                    try Data("Another file".utf8).write(to: directory.appendingPathComponent("note10.txt"))
                    try Data(repeating: 65, count: 2 * 1024 * 1024 + 1).write(to: directory.appendingPathComponent("large.txt"))
                    try Data([0, 1, 2]).write(to: directory.appendingPathComponent("unknown.bin"))
                    let image = UIGraphicsImageRenderer(size: CGSize(width: 40, height: 40)).pngData { context in
                        UIColor.systemBlue.setFill(); context.fill(CGRect(x: 0, y: 0, width: 40, height: 40))
                    }
                    try image.write(to: directory.appendingPathComponent("image.png"))
                    let pdf = UIGraphicsPDFRenderer(bounds: CGRect(x: 0, y: 0, width: 200, height: 200)).pdfData { context in
                        context.beginPage(); ("PDF preview" as NSString).draw(at: CGPoint(x: 20, y: 30), withAttributes: nil)
                    }
                    try pdf.write(to: directory.appendingPathComponent("document.pdf"))
                    // Short PCM audio verifies Quick Look's native media surface without network access.
                    var wave = Data("RIFF".utf8)
                    func appendLE<T: FixedWidthInteger>(_ value: T) { var little = value.littleEndian; withUnsafeBytes(of: &little) { wave.append(contentsOf: $0) } }
                    appendLE(UInt32(8036)); wave.append(Data("WAVEfmt ".utf8)); appendLE(UInt32(16))
                    appendLE(UInt16(1)); appendLE(UInt16(1)); appendLE(UInt32(8000)); appendLE(UInt32(16000))
                    appendLE(UInt16(2)); appendLE(UInt16(16)); wave.append(Data("data".utf8)); appendLE(UInt32(8000))
                    wave.append(Data(repeating: 0, count: 8000)); try wave.write(to: directory.appendingPathComponent("sound.wav"))
                    source.model.messages = []; source.model.items = []; source.model.activeSessionId = "ui-files-session"
                    source.model.isNew = true
                    source.model.workspaceState = SessionWorkspaceState(sessionId: "ui-files-session", cwd: paths.guestHome + "/" + fixture,
                        projectRoot: paths.guestHome + "/" + fixture, additionalDirectories: [], transcriptPath: "")
                } catch { source.model.error = ConversationError(kind: .host, message: error.localizedDescription) }
                return source
            }
            if let title = ProcessInfo.processInfo.environment["LINGXI_UI_TEST_SESSION_TITLE"] {
                source.model.messages = []
                source.model.items = []
                source.model.activeSessionId = "ui-named-session"
                source.model.isNew = true
                source.model.engineSessions = [EngineSession(
                    id: "ui-named-session", title: title, mode: sessionMode,
                    messageCount: 0, modifiedAt: Date(), relativeTime: ""
                )]
                source.model.engineSessionsLoaded = true
                return source
            }
            if ProcessInfo.processInfo.environment["LINGXI_UI_TEST_COMPACT_TRANSCRIPT"] == "1" {
                var prompt = Message(
                    role: .user,
                    text: "# Inline visualization\n\n" + String(repeating: "A compact interactive animation with a penguin riding a bicycle. ", count: 16)
                )
                prompt.sentAtMs = 1_792_706_220_000
                var interrupted = Message(role: .user, text: "[Request interrupted by user]")
                interrupted.sentAtMs = 1_792_706_340_000
                source.model.messages = [prompt, interrupted]
                source.model.items = [.message(prompt), .message(interrupted)]
                source.model.isNew = false
                source.model.activeSessionId = "ui-compact-transcript"
                return source
            }
            source.cannedReplyDelay = holdTurn ? 30 : 1.1
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
                elapsedMs: 42,
                display: ConversationToolResultDisplay(
                    headline: cancelledRun ? "Shell 已取消" : "Shell 完成",
                    headlineKind: nil,
                    headlineArgs: [],
                    diff: nil,
                    body: nil,
                    bodyLines: 0,
                    bodyTruncated: false,
                    collapsed: false
                )
            )
            let webSearchTrace = ConversationToolTrace(
                id: "ui-web-search",
                tool: "WebSearch",
                status: .cancelled,
                inputSummary: "Wuhan weather today",
                elapsedMs: 1_234
            )
            let readTrace = ConversationToolTrace(
                id: "ui-read",
                tool: "Read",
                status: .completed,
                inputSummary: "/home/lingxi/lingxi/ui-test/README.md",
                elapsedMs: 18,
                display: ConversationToolResultDisplay(
                    headline: "Read completed", headlineKind: nil, headlineArgs: [], diff: nil,
                    body: "Read fixture body", bodyLines: 1, bodyTruncated: false, collapsed: false
                )
            )
            let searchTrace = ConversationToolTrace(
                id: "ui-search",
                tool: "Grep",
                status: .completed,
                inputSummary: "ConversationTimelineView",
                elapsedMs: 29,
                display: ConversationToolResultDisplay(
                    headline: "Search completed", headlineKind: nil, headlineArgs: [], diff: nil,
                    body: nil, bodyLines: 0, bodyTruncated: false, collapsed: false
                )
            )
            let toolTraces = cancelledRun ? [webSearchTrace, shellTrace] : [shellTrace, readTrace, searchTrace]
            let user = Message(role: .user, text: "Hello")
            let assistant = Message(
                role: .ai,
                text: "Hello! I'm ready to help with your software engineering tasks."
            )
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
                        .textBoundary(id: "ui-text-boundary", messageID: assistant.id),
                        .tool(id: readTrace.id),
                        .tool(id: searchTrace.id),
                    ]
            )
            source.model.messages = [user, assistant]
            source.model.items = [.message(user), .run(run), .message(assistant)]
            source.model.messageDetails = [:]
            source.model.isNew = false
            source.model.activeSessionId = "ui-session"
            // Fixture launches start closed; persistence has isolated model tests.
            source.model.expandedToolCalls = []
            if multiAgent {
                // The prompt a subagent is DISPATCHED with arrives as the first
                // user bubble of its child transcript, and real ones run to
                // thousands of characters. Seed one past
                // `AssistantMessageCollapsePolicy`'s 640-character threshold so
                // the fold affordance is actually reachable — the fixture used
                // to hold only the `.ai` reply, so no test could cross the line
                // the fold guards.
                let childPrompt = Message(
                    role: .user,
                    text: String(
                        repeating: "Inspect the workspace and report every mismatch you find. ",
                        count: 80
                    )
                )
                let childMessage = Message(role: .ai, text: "Child agent completed the requested check.")
                source.model.replaceAgentSummaries([
                    .main,
                    ConversationAgentSummary(
                        id: "ui-child",
                        name: "UI Child",
                        agentType: "worker",
                        status: "working",
                        latestActivity: "Child agent checking workspace",
                        updatedAtMs: 2
                    ),
                ])
                source.model.setAgentTranscript(
                    "ui-child",
                    transcript: ConversationAgentTranscript(
                        messages: [childPrompt, childMessage],
                        items: [.message(childPrompt), .message(childMessage)],
                        loaded: true
                    ),
                    sessionID: "ui-session"
                )
            }
            if ProcessInfo.processInfo.environment["LINGXI_UI_TEST_ANSWERED_QUESTION"] == "1" {
                let trace = ConversationToolTrace(
                    id: "ui-answered-question", tool: "AskUserQuestion", status: .completed,
                    inputSummary: nil, elapsedMs: 12,
                    questionAnswers: [.init(question: "这次统一语音服务，要不要同时纳入 provider 原生实时语音对话（双向音频、打断、会话管理）？",
                                             answer: "同时纳入原生实时对话")])
                source.model.items = [.run(ConversationExecutionRun(
                    id: "ui-ask-run", sessionId: "ui-session", turnId: 1,
                    status: .completed, tools: [trace], activities: [.tool(id: trace.id)]))]
                source.model.messages = []
            }
            if askQuestion {
                source.model.pendingQuestions = [Self.uiTestAskQuestion]
            }
            source.model.availableModels = uiTestModelCatalog
            source.model.activeModelId = uiTestModelCatalog[0]
            let fastModelReference = "anthropic/claude-opus-4-8"
            source.model.availableModelDetails[fastModelReference] = ModelRuntimeDetails(
                reference: fastModelReference,
                providerId: "anthropic",
                providerLabel: "Anthropic",
                displayName: "Claude Opus 4.8",
                modelId: "claude-opus-4-8",
                description: nil, family: nil, status: nil,
                releaseDate: nil, lastUpdated: nil, knowledgeCutoff: nil,
                inputModalities: ["text"], outputModalities: ["text"],
                contextWindowTokens: nil, maxInputTokens: nil, maxOutputTokens: nil,
                openWeights: nil, attachments: nil, temperatureControl: nil,
                pricing: nil,
                capabilities: ModelCapabilitiesDto(
                    streaming: true, tools: true, vision: false, documents: false,
                    reasoning: true, structuredOutput: false
                ),
                reasoning: ReasoningControlSpecDto(
                    options: [ReasoningOptionDto(selection: .automatic, persistable: true)]
                        + ["low", "medium", "high"].map {
                            ReasoningOptionDto(selection: .level(id: $0), persistable: true)
                        }, budgetRange: nil, providerDefault: .automatic,
                    forcedReasoning: false, editable: true, disabledReason: nil
                ),
                supportsFastMode: true
            )
            #if canImport(harness_runtimeFFI)
                var uiCatalog: [(providerID: String, modelIDs: [String])] = []
                for reference in uiTestModelCatalog {
                    let parts = reference.split(separator: "/", maxSplits: 1).map(String.init)
                    guard parts.count == 2 else { continue }
                    let providerID = parts[0] == "gemini" ? "google" : parts[0]
                    if let index = uiCatalog.firstIndex(where: { $0.providerID == providerID }) {
                        uiCatalog[index].modelIDs.append(parts[1])
                    } else {
                        uiCatalog.append((providerID, [parts[1]]))
                    }
                }
                source.mockProviderCatalog = uiCatalog.map { catalog in
                    let preset = Presets.llm.first { $0.id == catalog.providerID }
                    return
                        ProviderCatalogEntry(
                            id: catalog.providerID,
                            displayName: preset?.name ?? catalog.providerID,
                            baseURL: preset?.defaultUrl ?? "",
                            protocolName: "OpenAiChat",
                            authName: catalog.providerID == "openai-chatgpt" ? "ChatGptOAuth" : "ApiKey",
                            credentialEnv: nil,
                            models: catalog.modelIDs,
                            modelDetails: [:]
                        )
                }
            #endif
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
            if ProcessInfo.processInfo.environment["LINGXI_UI_TEST_TRANSCRIPT_SCROLL"] == "1" {
                let history = (0..<35).map { Message(role: .user, text: "Scroll history \($0)") }
                source.model.messages = history
                source.model.items = history.map(ConversationRenderItem.message)
            }
            return source
        }

        /// A worst-case `AskUserQuestion` request for the questionnaire sheet:
        /// the wire maximum of 4 questions, each carrying the maximum 4 options
        /// with the descriptions the tool schema encourages. Real requests look
        /// like this, and the sheet must show the whole first question plus its
        /// actions without the user hunting for them.
        static let uiTestAskQuestion = ConversationPendingQuestion(
            requestId: 4_242,
            questions: (1 ... 4).map { index in
                ConversationAskQuestion(
                    question: "Which approach should the migration take for step \(index)?",
                    header: "Step \(index)",
                    options: (1 ... 4).map { option in
                        ConversationAskOption(
                            label: "Option \(index).\(option)",
                            description: "Rewrite the affected call sites in place and keep the "
                                + "existing public surface stable for downstream clients.",
                            preview: nil
                        )
                    },
                    multiSelect: index == 2
                )
            },
            timeoutSecs: nil
        )

        /// A multi-provider stand-in for `ClientEvent::ModelList` so the composer's
        /// model picker is reachable in UI tests (the plain mock leaves
        /// `availableModels` empty, which disables the chip). Shaped like the
        /// engine's curated refs: provider-qualified, active model first.
        static let uiTestModelCatalog = [
            "anthropic/claude-sonnet-5",
            "anthropic/claude-opus-4-8",
            "anthropic/claude-haiku-4-5",
            "anthropic/claude-fable-5-1",
            "openai/gpt-5.5",
            "openai/gpt-5.4",
            "openai-chatgpt/gpt-5.6-sol",
            "deepseek/deepseek-flash",
            "deepseek/deepseek-v4-pro",
            "kimi/kimi-k3",
            "gemini/gemini-3.5-flash",
            "zai/glm-5.1",
        ]
    #endif

    #if canImport(harness_runtimeFFI)
        func submitEngineCommand(_ command: ClientCommand) async throws {
            switch command {
            case let .answerAskUserQuestion(requestId, answers):
                guard let question = model.pendingQuestions.first(where: { $0.requestId == requestId }) else { return }
                let trace = ConversationToolTrace(
                    id: "mock-ask:\(requestId)", tool: "AskUserQuestion", status: .completed,
                    inputSummary: nil, elapsedMs: nil,
                    questionAnswers: question.questions.map { item in
                        let questionUnits = item.sourceQuestionCodeUnits ?? Array(item.question.utf16)
                        let answer = answers.pairs.last(where: { $0.question.codeUnits == questionUnits })?.answer.displayText
                        return .init(question: item.question, answer: answer)
                    })
                model.items.append(.run(ConversationExecutionRun(
                    id: "mock-question:\(requestId)", sessionId: model.activeSessionId ?? "mock-session", turnId: nil,
                    status: .completed, tools: [trace], activities: [.tool(id: trace.id)])))
                model.pendingQuestions.removeAll { $0.requestId == requestId }
            case let .cancelAskUserQuestion(requestId):
                model.pendingQuestions.removeAll { $0.requestId == requestId }
            default: break
            }
        }

        func providerCatalog() async throws -> [ProviderCatalogEntry] {
            mockProviderCatalog
        }
    #endif

    func startNewConversation() {
        turnToken &+= 1
        sessionEpoch &+= 1
        activeTurnToken = nil
        model.promptSuggestion = nil
        model.promptSuggestionSourceCodeUnits = nil
        model.promptSuggestionUuid = nil
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

    func setWorkspaceDirectory(_ path: String, expectedSessionID: String) async throws {
        guard expectedSessionID.isEmpty || expectedSessionID == model.activeSessionId else {
            throw WorkspaceDirectoryError.sessionChanged
        }
        let path = try await WorkspaceDirectoryRepository().canonicalDirectory(path)
        model.workspaceState = SessionWorkspaceState(sessionId: model.activeSessionId, cwd: path,
            projectRoot: model.workspaceState?.projectRoot ?? LXISHGuestPaths.workspaceRoot,
            additionalDirectories: model.workspaceState?.additionalDirectories ?? [],
            transcriptPath: model.workspaceState?.transcriptPath ?? "")
    }

    @discardableResult
    func send(_ text: String) -> ConversationTurnToken? {
        send(text, images: [])
    }

    @discardableResult
    func send(_ text: String, images: [ImageRefDto]) -> ConversationTurnToken? {
        guard !model.isSelectedAgentReadOnly else { return nil }
        if model.streaming {
            guard let activeTurnToken else { return nil }
            let message = Message(role: .user, text: text, images: uiImages(from: images))
            model.messages.append(message)
            model.items.append(.message(message))
            return activeTurnToken
        }
        model.isNew = false
        model.notice = nil
        model.turnCompletion = nil
        model.activeTurnToken = nil
        turnSpeechSequence = 0
        let message = Message(role: .user, text: text, images: uiImages(from: images))
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
        model.activeTurnToken = token
        #if DEBUG
            if ProcessInfo.processInfo.environment["LINGXI_UI_TEST_TRANSCRIPT_SCROLL"] == "1",
               text == "Stream scroll probe" {
                streamScrollTestReply(generation: generation, token: token)
                return token
            }
        #endif
        DispatchQueue.main.asyncAfter(deadline: .now() + cannedReplyDelay) { [weak self] in
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
            self.model.activeTurnToken = nil
        }
        return token
    }

    #if DEBUG
        /// Drives the real ChatView/composer with same-identity text deltas,
        /// without depending on provider latency or accessing user sessions.
        private func streamScrollTestReply(generation: Int, token: ConversationTurnToken) {
            let messageID = UUID()
            var initial = Message(id: messageID, role: .ai, text: "Scroll stream tail 0")
            initial.streamed = true
            model.messages.append(initial)
            model.items.append(.message(initial))
            Task { @MainActor [weak self] in
                var paragraphs: [String] = []
                for index in 1...80 {
                    try? await Task.sleep(for: .milliseconds(300))
                    guard let self, !Task.isCancelled,
                          self.turnToken == generation, self.activeTurnToken == token else { return }
                    paragraphs.append("Scroll delta \(index). A streamed paragraph keeps growing in this same message.")
                    var reply = Message(id: messageID, role: .ai,
                        text: paragraphs.joined(separator: "\n\n") + "\n\nScroll stream tail \(index)")
                    reply.streamed = true
                    if let messageIndex = self.model.messages.firstIndex(where: { $0.id == messageID }) {
                        self.model.messages[messageIndex] = reply
                    }
                    if let itemIndex = self.model.items.firstIndex(where: {
                        if case let .message(message) = $0 { return message.id == messageID }
                        return false
                    }) {
                        self.model.items[itemIndex] = .message(reply)
                    }
                }
                guard let self, self.turnToken == generation, self.activeTurnToken == token else { return }
                self.model.streaming = false
                self.model.updateMainAgent(status: "idle", latestActivity: String(localized: "chat_completed"))
                self.activeTurnToken = nil
                self.model.activeTurnToken = nil
            }
        }
    #endif

    @discardableResult
    func send(
        _ text: String,
        sourceCodeUnits: [UInt16]?,
        images: [ImageRefDto],
        selectedSlashCommand: ConversationSlashCommand?
    ) -> ConversationTurnToken? {
        send(text, images: images)
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
        model.activeTurnToken = nil
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
            model.reasoningSelection = "automatic"
            model.reasoningOptionDetails = model.availableModelDetails[id]?.reasoning.options.map { option in
                let id: String
                switch option.selection {
                case .automatic: id = "automatic"
                case .disabled: id = "disabled"
                case .enabled: id = "enabled"
                case let .level(value): id = value
                case let .tokenBudget(tokens): id = "budget:\(tokens)"
                }
                return ConversationReasoningOption(id: id, title: id == "automatic" ? "Auto" : id.capitalized,
                                                   isBudget: id.hasPrefix("budget:"), persistable: option.persistable)
            } ?? []
            model.reasoningOptions = model.reasoningOptionDetails.map(\.id)
        }
    }

    func setPermissionMode(_ mode: String) {
        model.requestedPermissionMode = mode
        model.effectivePermissionMode = mode
    }

    func confirmAndSetBypassPermissions(suppressWarning: Bool) {
        setPermissionMode("bypassPermissions")
        if suppressWarning {
            model.bypassPermissionsWarningSuppressed = true
        }
    }

    func setReasoningSelection(_ selection: String) {
        model.reasoningSelection = selection
    }

    func setFastMode(_ enabled: Bool) {
        model.fastMode = enabled
        model.fastModePending = false
        model.fastModeError = nil
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
        model.activeTurnToken = nil
        model.isNew = false
        model.statusLine = nil
        model.error = nil
        model.notice = nil
        model.clearAgentState()
    }

}
