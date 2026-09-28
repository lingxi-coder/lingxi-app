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
            "deepseek/deepseek-flash",
            "deepseek/deepseek-v4-pro",
            "kimi/kimi-k3",
            "gemini/gemini-3.5-flash",
            "zai/glm-5.1",
        ]
    #endif

    #if canImport(harness_runtimeFFI)
        func providerCatalog() async throws -> [ProviderCatalogEntry] {
            mockProviderCatalog
        }
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
