import SwiftUI
import OSLog

// MARK: - ChatView — main conversation surface
struct ChatView: View {
    #if canImport(engine_mobileFFI)
        private static let questionLog = Logger(
            subsystem: "com.lingxi.code",
            category: "ask-user-question"
        )
    #endif

    @Environment(AppState.self) private var app
    /// Chat keeps the desktop-neutral palette scoped to this surface. Child
    /// views receive the same resolved value through the environment below.
    private var t: Palette { DesignTokens.chatPalette(dark: app.isDark) }
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let session: SessionRef
    let projectName: String?
    let projectPath: String?

    /// The conversation source (mock or engine-over-UniFFI). ChatView renders its
    /// published `model` and forwards user input to it — it no longer owns the
    /// transcript or the canned reply timer.
    let source: any ConversationSource
    var onOpenSessionDetails: () -> Void = {}
    var onOpenShellTask: ((ConversationShellLaunchRequest) -> Void)? = nil
    @ObservedObject private var convo: ConversationModel

    @State private var followsLatestMessage = true
    @State private var followsLatestAgentMessage = true
    @State private var pendingAgentDetailID: String?

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
    @State private var providerRepository = ProviderRepository.shared
    @State private var recentModels: [String] = []
    /// The selected Runtime Center category. The enum drives the one native
    /// sheet used by the summary menu and keeps the presentation dismissible
    /// when the active session changes.
    @State private var summaryCategory: ConversationSummaryCategory?
    @State private var summaryDetent: PresentationDetent = .medium

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
         projectName: String? = nil,
         projectPath: String? = nil,
         onOpenVoiceSettings: @escaping () -> Void = {},
         onOpenSessionDetails: @escaping () -> Void = {},
         onOpenShellTask: ((ConversationShellLaunchRequest) -> Void)? = nil) {
        self.session = session
        self.projectName = projectName
        self.projectPath = projectPath
        self._draft = draft
        self.voiceInteraction = voiceInteraction
        self.source = source
        self.onOpenVoiceSettings = onOpenVoiceSettings
        self.onOpenSessionDetails = onOpenSessionDetails
        self.onOpenShellTask = onOpenShellTask
        self.convo = source.model
    }

    var body: some View {
        let visibleAvailableModels = providerRepository.visibleModelReferences(convo.availableModels)
        ZStack {
            t.windowBg.ignoresSafeArea()
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
                messageList(showsSelectedAgent: false)
                runtimeFooter
                if voiceInteraction.isPresented {
                    InlineVoicePanel(
                        controller: voiceInteraction,
                        onConfigure: onOpenVoiceSettings
                    )
                }
                Group {
                    Composer(model: $convo.model,
                         // SHIP-BLOCKER #2: drive the picker off the engine's real
                         // model catalog + active id (out-of-band model state). On
                         // pick, the source submits `SetModel(id)` with a real id.
                         availableModels: convo.availableModels,
                         availableModelDetails: convo.availableModelDetails,
                         activeModelId: convo.activeModelId,
                         providerConfigured: convo.providerConfigured,
                         onSelectModel: { reference in
                             source.setModel(reference)
                             // Only an explicit pick counts. `applyActiveModel`
                             // also fires on every ModelList/ModelChanged (boot,
                             // resume, reconnect), and recording those would fill
                             // the list with models the user never chose.
                             modelRecents.record(reference)
                             recentModels = modelRecents.resolved(against: visibleAvailableModels)
                         },
                         reasoningSelection: convo.reasoningSelection,
                         reasoningOptions: convo.reasoningOptions,
                         reasoningOptionDetails: convo.reasoningOptionDetails,
                         reasoningBudgetRange: convo.controls?.budgetRange,
                         reasoningDisabledReason: convo.reasoningDisabledReason,
                         permissionMode: convo.requestedPermissionMode,
                         effectivePermissionMode: convo.effectivePermissionMode,
                         permissionOptions: convo.permissionOptions,
                         fastModeEnabled: convo.fastMode,
                         fastModePending: convo.fastModePending || convo.sessionTransitionPending,
                         fastModeError: convo.fastModeError,
                         onSetFastMode: { source.setFastMode($0) },
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
                         showDiscardRecovery: convo.hasInactiveDurableRecovery,
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
                }
            }

            #if canImport(engine_mobileFFI)
                if !convo.pendingPermissions.isEmpty {
                    EnginePermissionPromptHost(
                        model: convo,
                        placement: .inChat,
                        onApprove: { source.approvePermission($0, $1) },
                        onDeny: { source.denyPermission($0) }
                    )
                    .environment(\.theme, t)
                    .transition(.opacity)
                }
            #endif

        }
        .environment(\.theme, t)
        .tint(t.accent)
        .buttonStyle(.plain)
        .animation(.easeOut(duration: 0.25), value: connectivity.isOffline)
        .animation(.spring(response: 0.3, dampingFraction: 0.86), value: voiceInteraction.isPresented)
        .onAppear {
            // SHIP-BLOCKER #2: build the engine eagerly so its real model catalog
            // (`ModelList`) populates the picker before the first send. No-op on the mock.
            source.warmUp()
            source.listSessionAgents()
            recentModels = modelRecents.resolved(against: visibleAvailableModels)
        }
        .onChange(of: session.id) { _, _ in
            // Re-arm following only for a real session switch. A sheet or
            // navigation presentation may cause this view to appear again;
            // that must not discard the user's current scroll position.
            followsLatestMessage = true
            pendingAgentDetailID = nil
            source.selectAgent(ConversationModel.mainAgentID)
            summaryCategory = nil
            summaryDetent = .medium
        }
        .onChange(of: summaryCategory) { _, _ in
            #if canImport(engine_mobileFFI)
                NotificationCenter.default.post(
                    name: .lingxiPermissionPresentationContextChanged,
                    object: nil
                )
            #endif
        }
        .onChange(of: visibleAvailableModels) { _, models in
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
        .sheet(item: pendingQuestion) { question in
            AskUserQuestionCard(
                question: question,
                onSubmit: { answers in await answerQuestion(question.requestId, answers: answers) },
                onCancel: { await cancelQuestion(question.requestId) }
            )
            .id(question.requestId)
            .environment(\.theme, t)
            .tint(t.accent)
            .presentationDetents([.large])
            .presentationDragIndicator(.visible)
            .presentationCornerRadius(28)
            .interactiveDismissDisabled()
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("conversation.ask-user-question.sheet")
        }
        .sheet(item: $summaryCategory, onDismiss: {
            if let id = pendingAgentDetailID {
                pendingAgentDetailID = nil
                source.selectAgent(id)
            }
        }) { category in
            ConversationSummarySheet(
                category: category,
                session: session,
                source: source,
                selectedAgentID: convo.selectedAgentID,
                onSelectAgent: { id in pendingAgentDetailID = id },
                onResumeWorkflow: source.resumeWorkflow
            )
            .environment(\.theme, t)
            .presentationDetents([.medium, .large], selection: $summaryDetent)
            .presentationDragIndicator(.visible)
        }
        .sheet(item: agentDetailSelection) { agent in
            NavigationStack {
                VStack(spacing: 0) {
                    Text(AgentStatusPresentation(rawValue: convo.selectedAgentSummary?.status ?? agent.status).label)
                        .font(.subheadline)
                        .foregroundStyle(t.text3)
                        .padding(.top, 8)
                    messageList(showsSelectedAgent: true)
                    Text("chat_agent_read_only_hint")
                        .accessibilityIdentifier("conversation.agent-read-only")
                        .font(.footnote)
                        .foregroundStyle(t.text3)
                        .padding()
                }
                .background(t.windowBg.ignoresSafeArea())
                .navigationTitle(agent.name.isEmpty ? agent.agentType : agent.name)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("common_close") { agentDetailSelection.wrappedValue = nil }
                            .accessibilityIdentifier("conversation.agent-detail.close")
                    }
                }
            }
            .environment(\.theme, t)
            .tint(t.accent)
            .presentationDetents([.large])
            .presentationDragIndicator(.visible)
            .accessibilityIdentifier("conversation.agent-detail-sheet")
            .onAppear { followsLatestAgentMessage = true }
        }
        // The chat is the detail column of RootView's split view. Its chrome is
        // the system navigation bar so that the leading item stays the system's
        // sidebar affordance — the only thing that both opens the sidebar and
        // advertises the back-swipe. Nothing here may hide that bar.
        .navigationTitle(projectName ?? (convo.isNew ? String(localized: "chat_new_conversation") : session.title))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    Text(projectName ?? (convo.isNew ? String(localized: "chat_new_conversation") : session.title))
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(t.text)
                        .lineLimit(1)
                    if let path = projectPath?.trimmingCharacters(in: .whitespacesAndNewlines), !path.isEmpty {
                        Text(path)
                            .font(.system(size: 9.5, design: .monospaced))
                            .foregroundStyle(t.text4)
                            .lineLimit(1)
                    }
                }
                .frame(maxWidth: 220)
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("conversation.project-context")
            }
            ToolbarItem(placement: .topBarTrailing) {
                summaryMenu
            }
        }
    }

    private var summaryMenu: some View {
        Menu {
            ForEach(summaryCategories) { category in
                Button {
                    summaryDetent = .medium
                    summaryCategory = category
                } label: {
                    Label(category.title, systemImage: category.systemImage)
                }
                .accessibilityIdentifier("conversation.summary.category.\(category.rawValue)")
            }
            if !summaryCategories.isEmpty { Divider() }
            Button {
                onOpenSessionDetails()
            } label: {
                Label(String(localized: "session_details_button"), systemImage: "info.circle")
            }
            .accessibilityIdentifier("conversation.session-details")
            Button {
                app.toggleTheme()
            } label: {
                Label(
                    app.isDark ? String(localized: "chat_theme_light") : String(localized: "chat_theme_dark"),
                    systemImage: app.isDark ? "sun.max" : "moon"
                )
            }
            .accessibilityIdentifier("conversation.theme-toggle")
            Button {
                newChat()
            } label: {
                Label(String(localized: "chat_new_chat"), systemImage: "square.and.pencil")
            }
            .accessibilityIdentifier("conversation.new-chat")
        } label: {
            Image(systemName: "list.bullet.rectangle")
                .symbolRenderingMode(.monochrome)
                .font(.system(size: 17, weight: .medium))
                .foregroundStyle(t.accent)
                .frame(width: 38, height: 36)
                .contentShape(.rect)
        }
        .accessibilityLabel("Open conversation summary")
        .accessibilityIdentifier("conversation.summary-menu")
    }

    private var summaryCategories: [ConversationSummaryCategory] {
        [.changes, .agents, .resources, .plan]
    }

    /// Only engine resolution removes a request; an incidental presentation
    /// dismissal must never silently cancel a pending answer.
    private var pendingQuestion: Binding<ConversationPendingQuestion?> {
        Binding(
            get: {
                summaryCategory == nil
                    ? ConversationRenderLayout.sheetQuestion(convo.pendingQuestions)
                    : nil
            },
            set: { _ in }
        )
    }

    // MARK: message list
    private var agentDetailSelection: Binding<ConversationAgentSummary?> {
        Binding(
            get: { convo.selectedAgentID == ConversationModel.mainAgentID ? nil : convo.selectedAgentSummary },
            set: { if $0 == nil { source.selectAgent(ConversationModel.mainAgentID) } }
        )
    }

    private func messageList(showsSelectedAgent: Bool) -> some View {
        // The scroll container is shared with local-app generation
        // (`TranscriptScroll`); what stays here is this screen's content and
        // its own definition of "something new arrived".
        let groups = showsSelectedAgent ? visibleTimelineGroups : ConversationRenderLayout.timelineGroups(renderItems)
        let visibleRenderItems = showsSelectedAgent ? self.visibleRenderItems : renderItems
        let visibleMessageDetails = showsSelectedAgent ? self.visibleMessageDetails : convo.messageDetails
        let streaming = showsSelectedAgent
            ? convo.selectedAgentSummary.map { AgentStatusPresentation(rawValue: $0.status) == .running } ?? false
            : convo.streaming
        let hasLiveOwner = showsSelectedAgent ? streaming : convo.streaming || convo.hasUnresolvedTurnRecovery
        let liveToolIDs = Set(visibleRenderItems.flatMap { item -> [String] in
            guard case let .run(run) = item,
                  (run.status == .running && hasLiveOwner) || run.activeWorkers > 0 else { return [] }
            return run.tools.filter { $0.status == .running }.map(\.id)
        })
        return TranscriptScroll(
            follow: FollowSignal(
                itemCount: groups.reduce(0) { $0 + $1.rows.count },
                // `last(where:)` scans from the end and stops at the first
                // match. The previous `reversed().compactMap { ... }.first`
                // allocated every message's text on every body evaluation —
                // once per streamed token — to keep one of them.
                lastMessageText: visibleRenderItems.last { item in
                    if case .message = item { return true }
                    return false
                }.flatMap { item in
                    guard case let .message(message) = item else { return nil }
                    return message.text
                },
                streaming: streaming,
                error: convo.error,
                notice: convo.notice,
                agentSummarySignal: convo.orderedAgentSummaries
                    .filter { $0.id != ConversationModel.mainAgentID }
                    .map { summary in
                        "\(summary.id):\(summary.status):\(summary.latestActivity ?? "")"
                    }
                    .joined(separator: "|")
            ),
            followsLatest: showsSelectedAgent ? $followsLatestAgentMessage : $followsLatestMessage,
            focused: showsSelectedAgent ? false : composerFocused,
            accessibilityIdentifier: showsSelectedAgent ? "conversation.agent-message-list" : "conversation.message-list",
            maxContentWidth: 860,
            alignShortContentToTop: true
        ) {
            if !showsSelectedAgent {
                if visibleRenderItems.isEmpty,
                   !streaming,
                   convo.isNew,
                    !convo.orderedAgentSummaries.contains(where: { $0.id != ConversationModel.mainAgentID }) {
                    emptyState
                }
                ConversationTimelineView(
                    groups: groups,
                    liveToolIDs: liveToolIDs,
                    hasLiveOwner: hasLiveOwner,
                    messageDetails: visibleMessageDetails,
                    expandedToolCalls: convo.expandedToolCalls,
                    onToggleToolCall: toggleToolCall,
                    onShareMessage: shareMessage,
                    transcriptAgents: convo.orderedAgentSummaries,
                    transcriptAgentAnchors: convo.transcriptAgentAnchors,
                    activeAgentID: ConversationModel.mainAgentID,
                    onSelectAgent: { source.selectAgent($0) },
                    streaming: streaming,
                    showThinking: canShowTranscriptThinking
                )
            } else if convo.isAgentTranscriptLoading {
                childAgentLoadingState
            } else if let error = convo.agentTranscriptError {
                childAgentErrorState(error)
            } else if selectedAgentTranscriptLoaded, visibleRenderItems.isEmpty, !streaming {
                childAgentEmptyState
            } else if !visibleRenderItems.isEmpty {
                ConversationTimelineView(
                    groups: groups,
                    liveToolIDs: liveToolIDs,
                    hasLiveOwner: hasLiveOwner,
                    messageDetails: visibleMessageDetails,
                    expandedToolCalls: convo.expandedToolCalls,
                    onToggleToolCall: toggleToolCall,
                    onShareMessage: shareMessage,
                    streaming: streaming,
                    showThinking: canShowTranscriptThinking
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
        let agentSummarySignal: String
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

    private var visibleMessageDetails: [UUID: ConversationMessageDetail] {
        convo.selectedAgentID == ConversationModel.mainAgentID
            ? convo.messageDetails
            : convo.selectedAgentMessageDetails
    }

    private var selectedAgentTranscriptLoaded: Bool {
        guard convo.selectedAgentID != ConversationModel.mainAgentID else { return true }
        return convo.agentTranscripts[convo.selectedAgentID]?.loaded == true
    }

    private var canShowTranscriptThinking: Bool {
        guard convo.pendingQuestions.isEmpty,
              !convo.isCancelling,
              !convo.hasUnresolvedTurnRecovery,
              convo.compactionStatus?.isActive != true else { return false }
        #if canImport(engine_mobileFFI)
            guard convo.pendingPermissions.isEmpty else { return false }
        #endif
        return true
    }

    /// Durable transcript rows only. Execution summaries live in the Runtime
    /// Center menu, while pending questions remain attached to the chat.
    private var renderItems: [ConversationRenderItem] {
        ConversationRenderLayout.transcriptItems(convo.items)
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
                let errorDescription = String(describing: error)
                Self.questionLog.error(
                    "answerAskUserQuestion failed requestId=\(requestId, privacy: .public) error=\(errorDescription, privacy: .public)"
                )
                print("[LingxiCode] answerAskUserQuestion failed requestId=\(requestId) error=\(errorDescription)")
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
        VStack(spacing: 10) {
            Image(systemName: "sparkles")
                .font(.system(size: 24, weight: .medium))
                .foregroundStyle(t.accent)
            Text(projectName ?? String(localized: "chat_start_new_conversation"))
                .font(.scaledSystem(20, weight: .semibold, relativeTo: .title2))
                .foregroundStyle(t.text)
                .lineLimit(2)
            Text("Describe what you want LingXi to accomplish")
                .font(.scaledSystem(14, relativeTo: .subheadline))
                .foregroundStyle(t.text3)
                .multilineTextAlignment(.center)
                .lineSpacing(7)
                .frame(maxWidth: 300)
        }
        .frame(maxWidth: .infinity, minHeight: 360)
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
        switch runtimeFooterState {
        case .hidden:
            EmptyView()
        case let .compaction(status):
            CompactionRuntimeFooter(status: status)
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

    private var runtimeFooterState: RuntimeFooterState {
        if let captureStatus, !captureStatus.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return .error(captureStatus)
        }
        if let compaction = convo.compactionStatus {
            return .compaction(compaction)
        }
        if let error = convo.error {
            return .error(error.message)
        }
        return .hidden
    }

    private func runtimeFooterRow(
        label: String,
        detail: String?,
        color: Color,
        isAnimated: Bool,
        icon: LXIconName? = nil,
        onDismiss: (() -> Void)? = nil
    ) -> some View {
        let hasIcon = icon != nil
        return HStack(spacing: 8) {
            HStack(spacing: 8) {
                if let icon {
                    LXIcon(name: icon, size: 15, color: color, stroke: 1.7)
                        .frame(width: 18)
                } else {
                    switch RuntimeFooterMotionPolicy.indicatorPresentation(
                        isAnimated: isAnimated,
                        hasIcon: hasIcon,
                        reduceMotion: reduceMotion
                    ) {
                    case .hidden:
                        // Both sibling cases occupy 18pt, so an EmptyView here
                        // collapses the leading slot and shifts the label by
                        // that width plus the HStack's spacing — on every
                        // turn boundary and every tool start/stop. The running
                        // affordance is the text sweep, not this slot, but the
                        // slot still has to hold its place.
                        Color.clear
                            .frame(width: 18, height: 18)
                            .accessibilityHidden(true)
                    case .staticIndicator:
                        LLMActivityIndicator(isActive: false, color: color)
                            .frame(width: 18)
                    case .activeIndicator:
                        // Reduce Motion keeps the active path reachable while
                        // LLMActivityIndicator itself renders it statically.
                        LLMActivityIndicator(isActive: true, color: color)
                            .frame(width: 18)
                    }
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(label)
                        .font(.system(size: 11.5, weight: .medium))
                        .foregroundStyle(color)
                        .runtimeTextSweep(
                            isActive: RuntimeFooterMotionPolicy.textSweepIsActive(
                                isAnimated: isAnimated,
                                reduceMotion: reduceMotion
                            ),
                            highlightColor: t.text
                        )
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
        convo.toggleTranscriptDisclosure(id)
    }

    private func newChat() {
        voiceInteraction.handleContextChange()
        source.startNewConversation()
    }

    private func send(_ txt: String) {
        let images = attachment.map { [ImageRefDto(mediaType: $0.mediaType, base64: $0.base64)] } ?? []
        guard let token = source.send(txt, images: images) else { return }
        // The source owns the durable transcript now; remove only after it has
        // accepted the turn so a failed send leaves the attachment retryable.
        attachment = nil
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
    case compaction(ConversationCompactionStatus)
    case error(String)
}

private struct CompactionRuntimeFooter: View {
    @Environment(\.theme) private var t
    let status: ConversationCompactionStatus

    private static let byteFormatter: ByteCountFormatter = {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        formatter.allowedUnits = [.useKB, .useMB, .useGB]
        return formatter
    }()

    var body: some View {
        switch status {
        case .queued:
            row(
                title: String(localized: "chat_compaction_waiting"),
                detail: nil,
                icon: .book,
                color: t.accent,
                progress: nil,
                preparing: true
            )
        case let .running(phase, startedAt, phaseStartedAt, unknownPhase):
            TimelineView(.periodic(from: .now, by: 1)) { context in
                let elapsed = max(0, context.date.timeIntervalSince(startedAt))
                let percent = ConversationCompactionProgress.percent(phase: unknownPhase ? "unknown" : phase, elapsed: context.date.timeIntervalSince(phaseStartedAt))
                row(
                    title: unknownPhase ? String(localized: "chat_compacting_context") : phase == "preparing" ? String(localized: "chat_compaction_preparing")
                        : phase == "summarizing" ? String(localized: "chat_compaction_summarizing")
                        : String(localized: "chat_compaction_restoring"),
                    detail: percent.map { String(format: String(localized: "chat_compaction_progress"), $0, Int(elapsed)) },
                    icon: .book,
                    color: t.accent,
                    progress: percent,
                    preparing: unknownPhase
                )
            }
        case let .completed(messagesBefore, messagesAfter, bytesSaved):
            row(
                title: String(localized: "chat_compacted_label"),
                detail: Self.completedDetail(
                    messagesBefore: messagesBefore,
                    messagesAfter: messagesAfter,
                    bytesSaved: bytesSaved
                ).map { "100% · " + $0 } ?? "100%",
                icon: .check,
                color: t.ok,
                progress: nil
            )
        case .skipped:
            row(title: String(localized: "chat_compaction_skipped"), detail: nil, icon: .check, color: t.ok, progress: nil)
        case let .failed(detail):
            row(
                title: String(localized: "chat_compaction_failed"),
                detail: detail,
                icon: .x,
                color: t.danger,
                progress: nil
            )
        }
    }

    private static func completedDetail(
        messagesBefore: UInt32?,
        messagesAfter: UInt32?,
        bytesSaved: UInt64?
    ) -> String? {
        guard let messagesBefore, let messagesAfter, let bytesSaved else { return nil }
        let formattedBytes = byteFormatter.string(fromByteCount: Int64(clamping: bytesSaved))
        return String(
            format: String(localized: "chat_compaction_status"),
            Int(messagesBefore),
            Int(messagesAfter),
            formattedBytes
        )
    }

    private func row(
        title: String,
        detail: String?,
        icon: LXIconName,
        color: Color,
        progress: Int?,
        preparing: Bool = false
    ) -> some View {
        HStack(alignment: .top, spacing: 9) {
            LXIcon(
                name: icon,
                size: 16,
                color: color,
                stroke: 1.75
            )
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(color)
                if let detail, !detail.isEmpty {
                    Text(detail)
                        .font(.system(size: 10.5))
                        .foregroundStyle(t.text4)
                        .lineLimit(2)
                }
                if let progress {
                    ProgressView(value: Double(progress), total: 100)
                        .tint(color)
                } else if preparing {
                    ProgressView()
                        .tint(color)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("conversation.compaction-status")
    }
}

/// Selects one running affordance for the footer. Text-only active states use
/// the existing text sweep during normal motion and a static indicator under
/// Reduce Motion; non-running text-only states retain their static indicator.
enum RuntimeFooterIndicatorPresentation: Equatable {
    case hidden
    case staticIndicator
    case activeIndicator
}

enum RuntimeFooterMotionPolicy {
    static func indicatorPresentation(
        isAnimated: Bool,
        hasIcon: Bool,
        reduceMotion: Bool
    ) -> RuntimeFooterIndicatorPresentation {
        guard !hasIcon else { return .hidden }
        if isAnimated {
            return reduceMotion ? .activeIndicator : .hidden
        }
        return .staticIndicator
    }

    static func textSweepIsActive(
        isAnimated: Bool,
        reduceMotion: Bool
    ) -> Bool {
        isAnimated && !reduceMotion
    }
}

/// Matches desktop's 2.2-second left-to-right text sweep for any running text
/// while keeping the underlying content readable when Reduce Motion is enabled.
struct RuntimeTextSweep: ViewModifier {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var sweepIsVisible = false

    let isActive: Bool
    let highlightColor: Color

    func body(content: Content) -> some View {
        content
            .overlay {
                if isActive && !reduceMotion {
                    GeometryReader { geometry in
                        LinearGradient(
                            stops: [
                                .init(color: .clear, location: 0),
                                .init(color: .clear, location: 0.42),
                                .init(color: highlightColor, location: 0.5),
                                .init(color: .clear, location: 0.58),
                                .init(color: .clear, location: 1),
                            ],
                            startPoint: .leading,
                            endPoint: .trailing
                        )
                        .offset(x: sweepIsVisible ? geometry.size.width * 1.2 : -geometry.size.width * 1.2)
                        .animation(
                            .linear(duration: 2.2).repeatForever(autoreverses: false),
                            value: sweepIsVisible
                        )
                    }
                    .mask(content)
                    .allowsHitTesting(false)
                }
            }
            .onAppear { sweepIsVisible = isActive && !reduceMotion }
            .onChange(of: isActive) { _, active in
                sweepIsVisible = active && !reduceMotion
            }
            .onChange(of: reduceMotion) { _, shouldReduceMotion in
                sweepIsVisible = isActive && !shouldReduceMotion
            }
    }
}

extension View {
    func runtimeTextSweep(isActive: Bool, highlightColor: Color) -> some View {
        modifier(RuntimeTextSweep(isActive: isActive, highlightColor: highlightColor))
    }
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
