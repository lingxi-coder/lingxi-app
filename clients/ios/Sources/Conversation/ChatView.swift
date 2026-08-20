import SwiftUI

// MARK: - ChatView — main conversation surface
struct ChatView: View {
    @Environment(AppState.self) private var app
    @Environment(\.theme) private var t

    let session: SessionRef

    /// The conversation source (mock or engine-over-UniFFI). ChatView renders its
    /// published `model` and forwards user input to it — it no longer owns the
    /// transcript or the canned reply timer.
    let source: any ConversationSource
    var onOpenSessionDetails: () -> Void = {}
    var onOpenShellTask: ((ConversationShellLaunchRequest) -> Void)? = nil
    @ObservedObject private var convo: ConversationModel

    @State private var followsLatestMessage = true

    // Composer draft, HOISTED up to RootView (the iOS analog of Android
    // RootScreen's `draft`) so a hold-to-talk transcription can route its
    // recognized text into the input AND so the root can persist it across the
    // app being backgrounded. ChatView binds to it; it no longer owns it.
    @Binding var draft: String
    // The captured-photo attachment, hoisted like the draft: the composer's
    // camera affordance drives an on-device capture and surfaces the JPEG here.
    @State private var attachment: ComposerAttachment? = nil
    // A transient affordance status line (permission denied / capture failed).
    @State private var captureStatus: String? = nil

    // The models this user has picked before, pinned to the top of the picker.
    // Held as view state (not read on every render) so the sheet re-sorts the
    // moment a pick lands instead of only after the next engine `ModelList`.
    private let modelRecents = ModelRecents()
    @State private var recentModels: [String] = []

    /// Root-owned state machine shared by ordinary dictation and Flow Mode.
    let voiceInteraction: VoiceInteractionController
    var onOpenVoiceSettings: () -> Void = {}
    private let cameraCapture = CameraCapture()
    @FocusState private var composerFocused: Bool

    // Connectivity: an offline banner (driven by NWPathMonitor) surfaced in the
    // chat view so the user is told up front when the network is unavailable —
    // a send still needs the network to reach the LLM API even though the engine
    // runs on-device. Purely informational + a retry; it never gates sending.
    @StateObject private var connectivity = ConnectivityMonitor()

    init(session: SessionRef,
         draft: Binding<String>,
         voiceInteraction: VoiceInteractionController,
         source: any ConversationSource,
         onOpenVoiceSettings: @escaping () -> Void = {},
         onOpenSessionDetails: @escaping () -> Void = {},
         onOpenShellTask: ((ConversationShellLaunchRequest) -> Void)? = nil) {
        self.session = session
        self._draft = draft
        self.voiceInteraction = voiceInteraction
        self.source = source
        self.onOpenVoiceSettings = onOpenVoiceSettings
        self.onOpenSessionDetails = onOpenSessionDetails
        self.onOpenShellTask = onOpenShellTask
        self.convo = source.model
    }

    var body: some View {
        ZStack {
            t.windowBg.ignoresSafeArea()
            // Ambient radial glow at the top.
            RadialGradient(colors: [t.ambient.top, t.ambient.bottom],
                           center: .init(x: 0.5, y: 0), startRadius: 0, endRadius: 360)
                .allowsHitTesting(false)
                .ignoresSafeArea()

            VStack(spacing: 0) {
                // Offline banner: shown only while the device is offline. Sits
                // above the transcript so it's visible without obscuring the
                // conversation; the retry re-warms the engine source (rebuilds
                // the handle / re-lists models).
                if connectivity.isOffline {
                    OfflineBanner(onRetry: retryConnection)
                        .padding(.horizontal, 14).padding(.top, 6).padding(.bottom, 8)
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
                messageList
                if !visibleExecutionGroups.isEmpty {
                    ExecutionStatusPanel(
                        agents: convo.agentSummaries,
                        selectedAgentID: selectedAgentBinding,
                        latestActivity: selectedAgentActivity,
                        isReadOnly: convo.isSelectedAgentReadOnly,
                        tasks: convo.backgroundTasks,
                        planTasks: convo.planTasks,
                        workflowResumeState: convo.workflowResumeState,
                        onSelectAgent: { id in
                            source.selectAgent(id ?? ConversationModel.mainAgentID)
                        },
                        onResumeWorkflow: source.resumeWorkflow
                    )
                    .padding(.horizontal, 14)
                    .padding(.vertical, 4)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                }
                runtimeFooter
                if !convo.isSelectedAgentReadOnly, voiceInteraction.isPresented {
                    InlineVoicePanel(
                        controller: voiceInteraction,
                        onConfigure: onOpenVoiceSettings
                    )
                }
                if !convo.isSelectedAgentReadOnly {
                    Composer(model: $convo.model,
                         // SHIP-BLOCKER #2: drive the picker off the engine's real
                         // model catalog + active id (out-of-band model state). On
                         // pick, the source submits `SetModel(id)` with a real id.
                         availableModels: convo.availableModels,
                         activeModelId: convo.activeModelId,
                         providerConfigured: convo.providerConfigured,
                         onSelectModel: { reference in
                             source.setModel(reference)
                             // Only an explicit pick counts. `applyActiveModel`
                             // also fires on every ModelList/ModelChanged (boot,
                             // resume, reconnect), and recording those would fill
                             // the list with models the user never chose.
                             modelRecents.record(reference)
                             recentModels = modelRecents.resolved(against: convo.availableModels)
                         },
                         reasoningSelection: convo.reasoningSelection,
                         reasoningOptions: convo.reasoningOptions,
                         reasoningOptionDetails: convo.reasoningOptionDetails,
                         reasoningBudgetRange: convo.controls?.budgetRange,
                         reasoningDisabledReason: convo.reasoningDisabledReason,
                         permissionMode: convo.requestedPermissionMode,
                         effectivePermissionMode: convo.effectivePermissionMode,
                         permissionOptions: convo.permissionOptions,
                         controlsPending: convo.controlsPending,
                         controlsError: convo.controlsError,
                         bypassWarningSuppressed: convo.bypassPermissionsWarningSuppressed,
                         onSelectReasoning: { source.setReasoningSelection($0) },
                         onSelectPermission: { source.setPermissionMode($0) },
                         onConfirmBypassPermissions: { source.confirmAndSetBypassPermissions(suppressWarning: $0) },
                         recentModels: recentModels,
                         draft: $draft,
                         onSend: send,
                         slashCommands: convo.slashCommands,
                         slashCommandsLoaded: convo.slashCommandsLoaded,
                         slashCommandPending: convo.slashCommandPending,
                         streaming: convo.streaming,
                         isCancelling: convo.isCancelling,
                         sendEnabled: !convo.sessionTransitionPending
                             && !convo.slashCommandPending
                             && voiceInteraction.mode != .flow,
                         onStop: stop,
                         onCameraClick: captureFromCamera,
                         inputFocused: $composerFocused,
                         attachment: attachment,
                         onRemoveAttachment: { attachment = nil },
                         onMicHoldStart: startVoiceHold,
                         onMicHoldRelease: endVoiceHold,
                         onMicHoldCancel: cancelVoiceHold,
                         onMicTap: toggleVoiceCapture,
                         onFlowModeTap: enterFlowMode,
                         voiceCapturePhase: voiceInteraction.capturePhase,
                         voiceInteractionMode: voiceInteraction.mode)
                } else {
                    HStack(spacing: 8) {
                        Image(systemName: "eye")
                            .font(.system(size: 13, weight: .medium))
                        Text(String(localized: "chat_agent_read_only_hint"))
                            .font(.system(size: 12.5))
                            .lineLimit(1)
                        Spacer(minLength: 0)
                    }
                    .foregroundStyle(t.text3)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 10)
                    .background(t.surface.opacity(0.65))
                    .clipShape(Capsule())
                    .padding(.horizontal, 14)
                    .padding(.bottom, 8)
                    .accessibilityIdentifier("conversation.agent-read-only")
                }
            }

        }
        .buttonStyle(.plain)
        .animation(.easeOut(duration: 0.25), value: connectivity.isOffline)
        .animation(.spring(response: 0.3, dampingFraction: 0.86), value: voiceInteraction.isPresented)
        .onAppear {
            // SHIP-BLOCKER #2: build the engine eagerly so its real model catalog
            // (`ModelList`) populates the picker before the first send. No-op on the mock.
            source.warmUp()
            source.listSessionAgents()
            recentModels = modelRecents.resolved(against: convo.availableModels)
        }
        .onChange(of: session.id) { _, _ in
            // Re-arm following only for a real session switch. A sheet or
            // navigation presentation may cause this view to appear again;
            // that must not discard the user's current scroll position.
            followsLatestMessage = true
        }
        .onChange(of: convo.availableModels) { _, models in
            // Re-resolve against the new catalog so a remembered model whose
            // provider was removed stops being offered.
            recentModels = modelRecents.resolved(against: models)
        }
        .onDisappear {
            voiceInteraction.handleContextChange()
        }
        .onReceive(convo.turnSpeechUpdates) { update in
            voiceInteraction.handleTurnSpeechUpdate(update)
        }
        .onChange(of: convo.turnCompletion) { _, completion in
            guard let completion else { return }
            voiceInteraction.handleTurnCompletion(completion)
        }
        .sheet(item: pendingQuestionBinding) { question in
            NavigationStack {
                ScrollView {
                    AskUserQuestionCard(
                        question: question,
                        onSubmit: { answers in await answerQuestion(question.requestId, answers: answers) },
                        onCancel: { await cancelQuestion(question.requestId) }
                    )
                    .padding(16)
                }
                .background(t.windowBg)
            }
            .presentationDetents([.medium, .large])
            .presentationDragIndicator(.visible)
            // The engine owns the pending request. Require the explicit cancel
            // action so a swipe cannot leave the turn parked with no UI.
            .interactiveDismissDisabled()
        }
        // The chat is the detail column of RootView's split view. Its chrome is
        // the system navigation bar so that the leading item stays the system's
        // sidebar affordance — the only thing that both opens the sidebar and
        // advertises the back-swipe. Nothing here may hide that bar.
        .navigationTitle(convo.isNew ? String(localized: "chat_new_conversation") : session.title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                HStack(spacing: 0) {
                    toolbarIconButton(
                        systemName: "info.circle",
                        label: String(localized: "session_details_button"),
                        accessibilityIdentifier: "conversation.session-details",
                        action: onOpenSessionDetails
                    )
                    toolbarIconButton(
                        systemName: app.isDark ? "sun.max" : "moon",
                        label: app.isDark
                            ? String(localized: "chat_theme_light")
                            : String(localized: "chat_theme_dark"),
                        accessibilityIdentifier: "conversation.theme-toggle",
                        action: app.toggleTheme
                    )
                    toolbarIconButton(
                        systemName: "square.and.pencil",
                        label: String(localized: "chat_new_chat"),
                        accessibilityIdentifier: "conversation.new-chat",
                        action: newChat
                    )
                }
            }
        }
    }

    private func toolbarIconButton(
        systemName: String,
        label: String,
        accessibilityIdentifier: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Image(systemName: systemName)
                .symbolRenderingMode(.monochrome)
                .font(.system(size: 17, weight: .medium))
                .foregroundStyle(t.accent)
                .frame(width: 32, height: 36)
                .contentShape(.rect)
        }
        .accessibilityLabel(label)
        .accessibilityIdentifier(accessibilityIdentifier)
    }

    // MARK: message list
    private var messageList: some View {
        // The scroll container is shared with local-app generation
        // (`TranscriptScroll`); what stays here is this screen's content and
        // its own definition of "something new arrived".
        TranscriptScroll(
            follow: FollowSignal(
                itemCount: visibleTimelineItemCount,
                lastMessageText: visibleRenderItems.reversed().compactMap { item in
                    guard case let .message(message) = item else { return nil }
                    return message.text
                }.first,
                streaming: convo.streaming,
                error: convo.error,
                notice: convo.notice
            ),
            followsLatest: $followsLatestMessage,
            focused: composerFocused,
            accessibilityIdentifier: "conversation.message-list"
        ) {
            if convo.selectedAgentID == ConversationModel.mainAgentID {
                if visibleRenderItems.isEmpty, !convo.streaming, convo.isNew { emptyState }
                ConversationTimelineView(
                    groups: visibleTimelineGroups,
                    messageDetails: visibleMessageDetails,
                    expandedToolCalls: convo.expandedToolCalls,
                    onToggleToolCall: toggleToolCall,
                    onShareMessage: shareMessage
                )
            } else if convo.isAgentTranscriptLoading {
                childAgentLoadingState
            } else if let error = convo.agentTranscriptError {
                childAgentErrorState(error)
            } else if selectedAgentTranscriptLoaded, visibleRenderItems.isEmpty, !convo.streaming {
                childAgentEmptyState
            } else if !visibleRenderItems.isEmpty {
                ConversationTimelineView(
                    groups: visibleTimelineGroups,
                    messageDetails: visibleMessageDetails,
                    expandedToolCalls: convo.expandedToolCalls,
                    onToggleToolCall: toggleToolCall,
                    onShareMessage: shareMessage
                )
            }
        }
    }

    /// The four signals this screen treats as "new content", collapsed into one
    /// value so `TranscriptScroll` needs a single `onChange` instead of one per
    /// signal.
    private struct FollowSignal: Equatable {
        let itemCount: Int
        /// A streamed assistant message keeps one stable row and replaces its
        /// value for every delta, so the item count alone does not change while
        /// the row's height and bottom position do. Keep the text itself so a
        /// same-length replacement still invalidates the follow signal.
        let lastMessageText: String?
        let streaming: Bool
        let error: ConversationError?
        let notice: TurnNotice?
    }

    /// Rows for the selected agent. The main agent keeps the rich execution
    /// rows produced by the reducer; child agents use the per-agent transcript
    /// cache and intentionally remain read-only.
    private var visibleRenderItems: [ConversationRenderItem] {
        guard convo.selectedAgentID != ConversationModel.mainAgentID else { return renderItems }
        return convo.selectedAgentItems
    }

    private var visibleTimelineGroups: [ConversationTimelineGroup] {
        convo.visibleTimelineGroups
    }

    private var visibleTimelineItemCount: Int {
        visibleTimelineGroups.reduce(0) { $0 + $1.rows.count }
    }

    private var visibleExecutionGroups: [ExecutionStatusPanel.Group] {
        ExecutionStatusPanel.visibleGroups(
            agents: convo.agentSummaries,
            tasks: convo.backgroundTasks,
            todos: convo.planTasks,
            workflowResumeState: convo.workflowResumeState,
            selectedAgentID: convo.selectedAgentID
        )
    }

    private var visibleMessageDetails: [UUID: ConversationMessageDetail] {
        convo.selectedAgentID == ConversationModel.mainAgentID
            ? convo.messageDetails
            : convo.selectedAgentMessageDetails
    }

    private var selectedAgentTranscriptLoaded: Bool {
        guard convo.selectedAgentID != ConversationModel.mainAgentID else { return true }
        return convo.agentTranscripts[convo.selectedAgentID]?.loaded == true
    }

    private var selectedAgentBinding: Binding<String?> {
        Binding(
            get: {
                convo.selectedAgentID == ConversationModel.mainAgentID ? nil : convo.selectedAgentID
            },
            set: { id in
                source.selectAgent(id ?? ConversationModel.mainAgentID)
            }
        )
    }

    private var selectedAgentActivity: String? {
        guard convo.selectedAgentID == ConversationModel.mainAgentID else { return nil }
        return convo.statusLine
    }

    /// Durable transcript rows only. Agent execution is pinned above the
    /// composer, while pending questions are presented by the native sheet.
    private var renderItems: [ConversationRenderItem] {
        ConversationRenderLayout.transcriptItems(convo.items)
    }

    private var pendingQuestionBinding: Binding<ConversationPendingQuestion?> {
        Binding(
            get: { ConversationRenderLayout.sheetQuestion(convo.pendingQuestions) },
            set: { _ in }
        )
    }

    /// 提交 for a pending questionnaire. The card stays disabled until the
    /// engine confirms with `askUserQuestionResolved` (which removes it);
    /// returning `false` re-enables the card for a retry.
    private func answerQuestion(_ requestId: UInt64, answers: [String: String]) async -> Bool {
        #if canImport(engine_mobileFFI)
            do {
                try await source.submitEngineCommand(
                    .answerAskUserQuestion(requestId: requestId, answers: answers)
                )
                return true
            } catch {
                return false
            }
        #else
            return false
        #endif
    }

    /// 取消 for a pending questionnaire.
    private func cancelQuestion(_ requestId: UInt64) async -> Bool {
        #if canImport(engine_mobileFFI)
            do {
                try await source.submitEngineCommand(.cancelAskUserQuestion(requestId: requestId))
                return true
            } catch {
                return false
            }
        #else
            return false
        #endif
    }

    private var emptyState: some View {
        VStack(spacing: 0) {
            RoundedRectangle(cornerRadius: 15)
                .fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                .frame(width: 52, height: 52)
                .overlay(LXIcon(name: .sparkle, size: 26, color: .white, stroke: 1.8))
                .padding(.bottom, 18)
            Text("chat_start_new_conversation")
                .font(.scaledSystem(21, weight: .semibold, relativeTo: .title2))
                .foregroundColor(t.text)
                .padding(.bottom, 7)
            Text("chat_empty_state_hint")
                .font(.scaledSystem(14, relativeTo: .subheadline)).foregroundColor(t.text4)
                .multilineTextAlignment(.center).lineSpacing(14 * 0.5)
                .frame(maxWidth: 260)
        }
        .frame(maxWidth: .infinity, minHeight: 420)
        .padding(.horizontal, 24)
    }

    private var childAgentEmptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: "person.crop.circle.badge.questionmark")
                .font(.system(size: 26, weight: .medium))
                .foregroundStyle(t.text3)
            Text(String(localized: "chat_agent_empty_transcript"))
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(t.text2)
            Text(String(localized: "chat_agent_empty_transcript_hint"))
                .font(.system(size: 12.5))
                .foregroundStyle(t.text4)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity, minHeight: 260)
        .padding(.horizontal, 24)
    }

    private var childAgentLoadingState: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(String(localized: "chat_agent_loading"))
                .font(.system(size: 12.5))
                .foregroundStyle(t.text3)
        }
        .frame(maxWidth: .infinity, minHeight: 260)
        .padding(.horizontal, 24)
    }

    private func childAgentErrorState(_ message: String) -> some View {
        VStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 24, weight: .medium))
                .foregroundStyle(t.danger)
            Text(String(localized: "chat_agent_load_failed"))
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(t.text2)
            Text(message)
                .font(.system(size: 12.5))
                .foregroundStyle(t.text4)
                .multilineTextAlignment(.center)
                .lineLimit(3)
        }
        .frame(maxWidth: .infinity, minHeight: 260)
        .padding(.horizontal, 24)
    }

    /// A single-line runtime footer for the main agent. Durable completion is
    /// intentionally silent; only work that still needs user attention remains
    /// visible below the transcript.
    @ViewBuilder
    private var runtimeFooter: some View {
        if convo.selectedAgentID == ConversationModel.mainAgentID {
            switch runtimeFooterState {
            case .hidden:
                EmptyView()
            case let .activeTool(tool):
                runtimeFooterRow(
                    label: tool.header.map { ToolDisplayText.title($0) } ?? tool.tool,
                    detail: tool.elapsedMs.flatMap(ConversationExecutionParsing.formatDuration),
                    color: ToolDisplayText.iconColor(header: tool.header, tool: tool.tool, palette: t),
                    isAnimated: true,
                    icon: ToolDisplayText.icon(header: tool.header, tool: tool.tool)
                )
            case let .thinking(status):
                runtimeFooterRow(
                    label: String(localized: "chat_thinking"),
                    detail: status,
                    color: t.accent,
                    isAnimated: true
                )
            case let .activeLabel(activity):
                runtimeFooterRow(
                    label: activity,
                    detail: nil,
                    color: t.accent,
                    isAnimated: true
                )
            case .stopping:
                runtimeFooterRow(
                    label: String(localized: "chat_stopping"),
                    detail: nil,
                    color: t.text3,
                    isAnimated: false
                )
            case .waiting:
                runtimeFooterRow(
                    label: String(localized: "chat_runtime_waiting"),
                    detail: nil,
                    color: t.text3,
                    isAnimated: false
                )
            case let .error(message):
                runtimeFooterRow(
                    label: String(localized: "chat_error_generic_headline"),
                    detail: message,
                    color: t.danger,
                    isAnimated: false,
                    onDismiss: dismissRuntimeError
                )
            }
        }
    }

    private var runtimeFooterState: RuntimeFooterState {
        if let captureStatus, !captureStatus.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return .error(captureStatus)
        }
        if let error = convo.error {
            return .error(error.message)
        }
        #if canImport(engine_mobileFFI)
            if !convo.pendingPermissions.isEmpty {
                return .waiting
            }
        #endif
        if !convo.pendingQuestions.isEmpty {
            return .waiting
        }
        if convo.isCancelling {
            return .stopping
        }
        if let tool = currentRunningTool {
            return .activeTool(tool)
        }
        if convo.streaming {
            return .thinking(convo.statusLine)
        }
        if let summary = convo.selectedAgentSummary,
           AgentStatusPresentation(rawValue: summary.status) == .running,
           let activity = summary.latestActivity?.trimmingCharacters(in: .whitespacesAndNewlines),
           !activity.isEmpty {
            return .activeLabel(activity)
        }
        return .hidden
    }

    private var currentRunningTool: ConversationToolTrace? {
        for item in convo.selectedAgentItems.reversed() {
            let tools: [ConversationToolTrace]
            switch item {
            case let .run(run): tools = run.tools
            case let .toolCall(trace): tools = [trace]
            default: continue
            }
            if let trace = tools.reversed().first(where: { $0.status == .running }) {
                return trace
            }
        }
        return nil
    }

    private func runtimeFooterRow(
        label: String,
        detail: String?,
        color: Color,
        isAnimated: Bool,
        icon: LXIconName? = nil,
        onDismiss: (() -> Void)? = nil
    ) -> some View {
        HStack(spacing: 8) {
            HStack(spacing: 8) {
                if let icon {
                    LXIcon(name: icon, size: 15, color: color, stroke: 1.7)
                        .frame(width: 18)
                } else {
                    LLMActivityIndicator(isActive: isAnimated, color: color)
                        .frame(width: 18)
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(label)
                        .font(.system(size: 11.5, weight: .medium))
                        .foregroundStyle(color)
                    if let detail, !detail.isEmpty {
                        Text(detail)
                            .font(.system(size: 10.5))
                            .foregroundStyle(t.text4)
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 0)
            }
            .accessibilityElement(children: .combine)

            if let onDismiss {
                Button(action: onDismiss) {
                    LXIcon(name: .x, size: 13, color: t.text3, stroke: 2)
                        .frame(width: 28, height: 28)
                        .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("chat_dismiss_error")
                .accessibilityIdentifier("conversation.runtime-error.dismiss")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 5)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(t.border.opacity(0.35))
                .frame(height: 0.5)
        }
        .accessibilityIdentifier("conversation.llm-status")
    }

    private func dismissRuntimeError() {
        if let status = captureStatus,
           !status.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            captureStatus = nil
        } else {
            source.dismissError()
        }
    }

    // MARK: actions

    /// Flip a tool row's expanded state. The set lives in the MODEL, not in the
    /// row: this list recycles its rows, so row-local state is lost on scroll
    /// and would then reappear on whichever row reused the storage.
    private func toggleToolCall(_ id: String) {
        if convo.expandedToolCalls.contains(id) {
            convo.expandedToolCalls.remove(id)
        } else {
            convo.expandedToolCalls.insert(id)
        }
    }

    private func newChat() {
        voiceInteraction.handleContextChange()
        source.startNewConversation()
    }

    private func send(_ txt: String) {
        guard let token = source.send(txt) else { return }
        voiceInteraction.registerAutomaticPlaybackCandidate(token)
    }

    // PR-4 item 2: interrupt the in-flight turn.
    private func stop() { source.cancel() }

    // MARK: capability affordances (mirror Android RootScreen)

    /// Mic press past the 0.6s threshold opens the microphone immediately and
    /// presents the compact capture overlay. Duplicate delivery is ignored.
    private func startVoiceHold() {
        captureStatus = nil
        composerFocused = false
        voiceInteraction.startDictation(onTranscript: appendVoiceTranscript)
    }

    /// Finger lift ends the request that has been recording since press-down;
    /// Speech then emits its final transcript through the shared state machine.
    private func endVoiceHold() {
        voiceInteraction.finishListening()
    }

    /// Android exposes ordinary recording and Flow Mode as separate controls.
    /// A tap on the microphone toggles the same capture used by press-and-hold.
    private func toggleVoiceCapture() {
        switch voiceInteraction.capturePhase {
        case .idle:
            startVoiceHold()
        case .listening:
            endVoiceHold()
        case .finishing:
            break
        }
    }

    private func enterFlowMode() {
        guard !convo.sessionTransitionPending else { return }
        cancelVoiceHold()
        composerFocused = false
        voiceInteraction.startFlow(source: source)
    }

    private func cancelVoiceHold() {
        voiceInteraction.cancelDictation()
    }

    private func appendVoiceTranscript(_ text: String) {
        draft = draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            ? text : "\(draft) \(text)"
    }

    /// Attach button: drive an on-device camera capture (falling back to the
    /// photo library when the camera is unavailable, e.g. the simulator) and
    /// surface the photo as a composer attachment chip.
    private func captureFromCamera() {
        captureStatus = nil
        Task {
            var result = await cameraCapture.capture(fromLibrary: false)
            // Simulators have no camera — fall back to the library so the
            // affordance is still exercisable.
            if case .failed = result { result = await cameraCapture.capture(fromLibrary: true) }
            switch result {
            case let .captured(att):
                attachment = att
            case .cancelled:
                break
            case .permissionDenied:
                captureStatus = String(localized: "chat_capture_permission_denied")
            case let .failed(message):
                captureStatus = String(localized: "chat_capture_failed \(message)")
            }
        }
    }

    /// Share an assistant reply's text via the native share sheet.
    private func shareMessage(_ text: String) { ShareCapture.share(text: text) }

    /// Retry from the offline banner: re-warm the engine source so a recovered
    /// connection rebuilds the handle / re-lists models. `NWPathMonitor` clears
    /// the banner on its own once the path is satisfied again; this gives the
    /// user an explicit nudge instead of waiting for the next send to fail.
    private func retryConnection() { source.warmUp() }

}

private enum RuntimeFooterState: Equatable {
    case hidden
    case activeTool(ConversationToolTrace)
    case thinking(String?)
    case activeLabel(String)
    case stopping
    case waiting
    case error(String)
}

private struct LLMActivityIndicator: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let isActive: Bool
    let color: Color
    @State private var phase = false

    var body: some View {
        HStack(alignment: .center, spacing: 3) {
            ForEach(0..<3) { index in
                Capsule()
                    .fill(color)
                    .frame(width: 3, height: 14)
                    .scaleEffect(y: isActive && !reduceMotion && phase ? 1 : 0.52)
                    .animation(
                        reduceMotion
                            ? nil
                            : isActive
                                ? .easeInOut(duration: 0.7).repeatForever(autoreverses: true).delay(Double(index) * 0.12)
                                : .easeOut(duration: 0.16),
                        value: phase
                    )
            }
        }
        .frame(width: 18, height: 18)
        .onAppear {
            if isActive && !reduceMotion { phase = true }
        }
        .onChange(of: isActive) { _, active in
            phase = active && !reduceMotion
        }
        .onChange(of: reduceMotion) { _, shouldReduceMotion in
            phase = isActive && !shouldReduceMotion
        }
    }
}

// MARK: - Error banner (PR-4 item 4)

/// A persistent, dismissible, kind-aware error surface. Unlike the dim
/// `statusLine`, it stays until the user taps × — and its label/tint are driven
/// by the error `kind` (transport vs. server vs. internal …) so the user can
/// tell a network hiccup from a protocol fault.
private struct ErrorBanner: View {
    @Environment(\.theme) private var t
    let error: ConversationError
    let onDismiss: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            LXIcon(name: .warning, size: 16, color: t.danger, stroke: 1.8)
                .frame(width: 20, height: 20)
            VStack(alignment: .leading, spacing: 3) {
                Text(error.kind.label)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(t.text)
                Text(error.message)
                    .font(.system(size: 12.5))
                    .foregroundColor(t.text2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            Button(action: onDismiss) {
                LXIcon(name: .x, size: 14, color: t.text3, stroke: 2)
                    .frame(width: 28, height: 28)
                    .contentShape(RoundedRectangle(cornerRadius: 8))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("chat_dismiss_error")
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(t.danger.opacity(0.10))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.danger.opacity(0.40), lineWidth: 0.5))
        .padding(.bottom, 18)
    }
}
