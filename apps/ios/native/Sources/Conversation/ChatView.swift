import SwiftUI
import OSLog
import harness_runtimeFFI

// MARK: - ChatView — main conversation surface
struct ChatView: View {
    #if canImport(harness_runtimeFFI)
        private static let questionLog = Logger(
            subsystem: "com.lingxi.code",
            category: "ask-user-question"
        )
    #endif

    @Environment(AppState.self) private var app
    @Environment(\.theme) private var t
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let session: SessionRef
    let projectName: String?
    /// The conversation's mode and workspace, for the top bar's `Mode · Workspace`
    /// line, the new-chat bar and the session panel.
    let sessionMode: SessionMode
    let workspaceName: String
    var workspaceKey: String = "global"
    var onDraftMediaChanged: (Bool) -> Void = { _ in }
    var isPinned = false
    /// An existing (not brand-new) session can be archived.
    var canArchive = false
    var onSelectNewChatMode: (SessionMode) -> Void = { _ in }
    var onPickWorkspace: () -> Void = {}
    var onContinueInOtherMode: () -> Void = {}
    var onTogglePinned: () -> Void = {}
    var onMakeScheduled: () -> Void = {}
    var onOpenTerminal: () -> Void = {}
    var onOpenFiles: () -> Void = {}
    var onArchive: () -> Void = {}
    /// Replaces the plain fresh-conversation start of the new-chat button;
    /// nil starts the new chat in the current workspace.
    var onNewChat: (() -> Void)? = nil

    /// The conversation source (mock or engine-over-UniFFI). ChatView renders its
    /// published `model` and forwards user input to it — it no longer owns the
    /// transcript or the canned reply timer.
    let source: any ConversationSource
    var onOpenSessionDetails: () -> Void = {}
    var onOpenShellTask: ((ConversationShellLaunchRequest) -> Void)? = nil
    @ObservedObject private var convo: ConversationModel

    @State private var latestMessageRequestRevision: UInt64 = 0
    @State private var pendingAgentDetailID: String?
    @State private var recoveryEvidenceReviewSnapshot: [ConversationRecoveryEvidence] = []
    @State private var showRecoveryEvidenceConfirmation = false

    // Composer draft, HOISTED up to RootView (the iOS analog of Android
    // RootScreen's `draft`) so a hold-to-talk transcription can route its
    // recognized text into the input AND so the root can persist it across the
    // app being backgrounded. ChatView binds to it; it no longer owns it.
    @Binding var draft: String
    @State private var localDraftSourceCodeUnits: [UInt16]? = nil
    private var externalDraftSourceCodeUnitsBinding: Binding<[UInt16]?>?
    private var draftSourceCodeUnitsBinding: Binding<[UInt16]?> {
        externalDraftSourceCodeUnitsBinding ?? $localDraftSourceCodeUnits
    }
    // The captured-photo attachment, hoisted like the draft: the composer's
    // camera affordance drives an on-device capture and surfaces the JPEG here.
    @State private var localAttachment: ComposerAttachment? = nil
    private var attachmentBinding: Binding<ComposerAttachment?>?
    private var attachment: ComposerAttachment? {
        get { attachmentBinding?.wrappedValue ?? localAttachment }
        nonmutating set {
            if let attachmentBinding { attachmentBinding.wrappedValue = newValue }
            else { localAttachment = newValue }
        }
    }
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
    /// The session panel, and what to do after it has closed (sheets cannot
    /// present one another, so a row picks a follow-up that runs on dismiss).
    @State private var sessionPanelOpen = false
    @State private var sessionPanelFollowUp: SessionPanelAction?
    @State private var sideChatSheetOpen = false
    @State private var modelPickerOpen = false
    @State private var permissionSheetOpen = false

    /// Root-owned state machine shared by ordinary dictation and Flow Mode.
    let voiceInteraction: VoiceInteractionController
    var onOpenVoiceSettings: () -> Void = {}
    private let cameraCapture = CameraCapture()
    @State private var composerFocused = false

    // Connectivity: an offline banner (driven by NWPathMonitor) surfaced in the
    // chat view so the user is told up front when the network is unavailable —
    // a send still needs the network to reach the LLM API even though the engine
    // runs on-device. Purely informational + a retry; it never gates sending.
    @StateObject private var connectivity = ConnectivityMonitor()

    init(session: SessionRef,
         draft: Binding<String>,
         voiceInteraction: VoiceInteractionController,
         source: any ConversationSource,
         draftSourceCodeUnits: Binding<[UInt16]?>? = nil,
         attachmentBinding: Binding<ComposerAttachment?>? = nil,
         projectName: String? = nil,
         sessionMode: SessionMode = .code,
         workspaceName: String = "",
         workspaceKey: String = "global",
         onDraftMediaChanged: @escaping (Bool) -> Void = { _ in },
         isPinned: Bool = false,
         canArchive: Bool = false,
         onSelectNewChatMode: @escaping (SessionMode) -> Void = { _ in },
         onPickWorkspace: @escaping () -> Void = {},
         onContinueInOtherMode: @escaping () -> Void = {},
         onTogglePinned: @escaping () -> Void = {},
         onMakeScheduled: @escaping () -> Void = {},
         onOpenTerminal: @escaping () -> Void = {},
         onOpenFiles: @escaping () -> Void = {},
         onArchive: @escaping () -> Void = {},
         onNewChat: (() -> Void)? = nil,
         onOpenVoiceSettings: @escaping () -> Void = {},
         onOpenSessionDetails: @escaping () -> Void = {},
         onOpenShellTask: ((ConversationShellLaunchRequest) -> Void)? = nil) {
        self.session = session
        self.projectName = projectName
        self.sessionMode = sessionMode
        self.externalDraftSourceCodeUnitsBinding = draftSourceCodeUnits
        self.workspaceName = workspaceName
        self.workspaceKey = workspaceKey
        self.onDraftMediaChanged = onDraftMediaChanged
        self.isPinned = isPinned
        self.canArchive = canArchive
        self.onSelectNewChatMode = onSelectNewChatMode
        self.onPickWorkspace = onPickWorkspace
        self.onContinueInOtherMode = onContinueInOtherMode
        self.onTogglePinned = onTogglePinned
        self.onMakeScheduled = onMakeScheduled
        self.onOpenTerminal = onOpenTerminal
        self.onOpenFiles = onOpenFiles
        self.onArchive = onArchive
        self.onNewChat = onNewChat
        self._draft = draft
        self.voiceInteraction = voiceInteraction
        self.source = source
        self.attachmentBinding = attachmentBinding
        self.onOpenVoiceSettings = onOpenVoiceSettings
        self.onOpenSessionDetails = onOpenSessionDetails
        self.onOpenShellTask = onOpenShellTask
        self.convo = source.model
    }

    private func reportSuggestionContext(focus: String? = nil) {
        var pendingPermission = !convo.pendingQuestions.isEmpty
        #if canImport(harness_runtimeFFI)
        pendingPermission = pendingPermission || !convo.pendingPermissions.isEmpty
        #endif
        source.updatePromptSuggestionContext(sessionId: convo.activeSessionId,
            focus: focus ?? (scenePhase == .active ? "focused" : "blurred"),
            inputEmpty: draft.isEmpty && attachment == nil && convo.visualizationChip == nil,
            pendingPermission: pendingPermission)
    }

    @ViewBuilder
    private var conversationContent: some View {
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
                if let guidance = AudioSetupGuidance.forPhase(voiceInteraction.phase, message: voiceInteraction.statusDetail) {
                    AudioSetupBanner(guidance: guidance, onConfigure: onOpenVoiceSettings, onDismiss: voiceInteraction.close)
                } else if voiceInteraction.isPresented {
                    InlineVoicePanel(
                        controller: voiceInteraction,
                        onConfigure: onOpenVoiceSettings
                    )
                }
                if showsNewChatBar {
                    NewChatBar(
                        mode: sessionMode,
                        workspaceName: workspaceName,
                        onSelectMode: onSelectNewChatMode,
                        onPickWorkspace: onPickWorkspace
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
                         activeModelDisplay: convo.activeModelSource,
                         contextUsage: convo.sessionTransitionPending ? nil : convo.contextUsage,
                         providerConfigured: convo.providerConfigured,
                         onSelectModel: { reference in
                             source.setModel(reference)
                             // Only an explicit pick counts. `applyActiveModel`
                             // also fires on every ModelList/ModelChanged (boot,
                             // resume, reconnect), and recording those would fill
                             // the list with models the user never chose.
                             if !source.isTemporaryConversation { modelRecents.record(reference) }
                             recentModels = modelRecents.resolved(against: visibleAvailableModels)
                         },
                         reasoningModelId: convo.controls?.qualifiedModel,
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
                         showsFastMode: !source.isTemporaryConversation,
                         controlsPending: convo.controlsPending,
                         controlsError: convo.controlsError,
                         bypassWarningSuppressed: convo.bypassPermissionsWarningSuppressed,
                         onSelectReasoning: { source.setReasoningSelection($0) },
                         onSelectPermission: { source.setPermissionMode($0) },
                         onConfirmBypassPermissions: { source.confirmAndSetBypassPermissions(suppressWarning: $0) },
                         recentModels: recentModels,
                         draft: $draft,
                         draftSourceCodeUnits: draftSourceCodeUnitsBinding,
                         onSend: { text, sourceCodeUnits, selectedSlashCommand in
                             send(text, sourceCodeUnits: sourceCodeUnits, selectedSlashCommand: selectedSlashCommand)
                         },
                         promptSuggestion: convo.promptSuggestion,
                         promptSuggestionSourceCodeUnits: convo.promptSuggestionSourceCodeUnits,
                         promptSuggestionNotice: convo.promptSuggestionNotice,
                         suggestionTurnKey: "\(convo.activeSessionId):\(convo.promptSuggestionUuid ?? "")",
                         onSuggestionFeedback: { action in
                             if let uuid = convo.promptSuggestionUuid {
                                 source.promptSuggestionFeedback(sessionId: convo.activeSessionId, uuid: uuid, action: action)
                             }
                         },
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
                         visualizationChip: convo.visualizationChip,
                         onRemoveVisualizationChip: { convo.visualizationChip = nil },
                         onMicHoldStart: startVoiceHold,
                         onMicHoldRelease: endVoiceHold,
                         onMicHoldCancel: cancelVoiceHold,
                         onMicTap: toggleVoiceCapture,
                         onFlowModeTap: enterFlowMode,
                         showsFlowMode: !source.isTemporaryConversation,
                         voiceCapturePhase: voiceInteraction.capturePhase,
                         voiceInteractionMode: voiceInteraction.mode,
                         modelOpen: $modelPickerOpen,
                         controlsOpen: $permissionSheetOpen,
                         showsPermissionChip: sessionMode == .code)
                    .preserveConversationDraft()
                }
            }

        }
        #if canImport(harness_runtimeFFI)
        .onChange(of: convo.pendingPermissions) { _, _ in reportSuggestionContext() }
        #endif
        .onChange(of: draft) { _, _ in reportSuggestionContext() }
        .onChange(of: scenePhase) { _, _ in reportSuggestionContext() }
        .onChange(of: convo.activeSessionId) { _, _ in
            convo.compactRejectionID = nil
            reportSuggestionContext()
        }
        .onChange(of: attachment) { _, value in reportSuggestionContext(); onDraftMediaChanged(value != nil || convo.visualizationChip != nil) }
        .onChange(of: convo.visualizationChip?.id) { _, _ in reportSuggestionContext(); onDraftMediaChanged(attachment != nil || convo.visualizationChip != nil) }
        .onChange(of: convo.pendingQuestions.map(\.requestId)) { _, _ in reportSuggestionContext() }
    }

    var body: some View {
        let visibleAvailableModels = providerRepository.visibleModelReferences(convo.availableModels)
        conversationContent
        .overlay(alignment: .top) {
            if let rejectionID = convo.compactRejectionID {
                CompactRejectionAlert { convo.compactRejectionID = nil }
                    .padding(.horizontal, 16)
                    .padding(.top, 8)
                    .id(rejectionID)
                    .task(id: rejectionID) {
                        guard !UIAccessibility.isVoiceOverRunning else { return }
                        do { try await Task.sleep(for: .seconds(5)) } catch { return }
                        if convo.compactRejectionID == rejectionID { convo.compactRejectionID = nil }
                    }
            }
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
            onDraftMediaChanged(attachment != nil || convo.visualizationChip != nil)
            reportSuggestionContext()
            source.listSessionAgents()
            recentModels = modelRecents.resolved(against: visibleAvailableModels)
        }
        .onChange(of: session.id) { _, _ in
            pendingAgentDetailID = nil
            source.selectAgent(ConversationModel.mainAgentID)
            summaryCategory = nil
            summaryDetent = .medium
        }
        .onChange(of: summaryCategory) { _, _ in
            #if canImport(harness_runtimeFFI)
                NotificationCenter.default.post(
                    name: .lingxiPermissionPresentationContextChanged,
                    object: nil
                )
            #endif
        }
        .onChange(of: convo.pendingQuestions.map(\.requestId)) { _, _ in
            #if canImport(harness_runtimeFFI)
                // The root permission-sheet fallback needs a presentation
                // context update if a permission arrives while this question
                // sheet is already above the chat view.
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
            reportSuggestionContext(focus: "blurred")
            voiceInteraction.handleContextChange()
        }
        .onReceive(convo.turnSpeechUpdates) { update in
            voiceInteraction.handleTurnSpeechUpdate(update)
        }
        .onChange(of: convo.visualizationFollowup) { _, followup in
            // A widget's "ask about this": seed the draft and carry its chip
            // with the next prompt (the source attaches and clears it).
            guard let followup else { return }
            draft = draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                ? followup.text : "\(draft) \(followup.text)"
            convo.visualizationChip = followup.chip
            convo.visualizationFollowup = nil
            composerFocused = true
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
        .sheet(isPresented: $sessionPanelOpen, onDismiss: runSessionPanelFollowUp) {
            SessionPanelSheet(
                title: sessionTitle,
                mode: sessionMode,
                workspaceName: workspaceName,
                messageCount: convo.messages.count,
                modelName: activeModelName,
                effortLabel: effortLabel,
                permissionLabel: sessionMode == .code ? permissionLabel : nil,
                isPinned: isPinned,
                canPin: !convo.isNew,
                canArchive: canArchive && !convo.isNew,
                onAction: { action in
                    sessionPanelFollowUp = action
                    sessionPanelOpen = false
                }
            )
            .environment(\.theme, t)
            .tint(t.accent)
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
        }
        // The chat is the detail column of RootView's split view. Its chrome is
        // the system navigation bar so that the leading item stays the system's
        // sidebar affordance — the only thing that both opens the sidebar and
        // advertises the back-swipe. Nothing here may hide that bar.
        .navigationTitle(sessionTitle)
        .navigationBarTitleDisplayMode(.inline)
        #if canImport(harness_runtimeFFI)
        .sheet(isPresented: $sideChatSheetOpen) {
            if let engine = source as? EngineConversationSource {
                SideChatSheet(collection: engine.sideChats)
            }
        }
        #endif
        .toolbar {
            if !source.isTemporaryConversation {
                ToolbarItem(placement: .principal) {
                    Button { sessionPanelOpen = true } label: {
                        HStack(spacing: 4) {
                            Text(sessionTitle)
                                .font(.headline)
                                .foregroundStyle(t.text)
                                .lineLimit(1)
                                .truncationMode(.tail)
                            Image(systemName: "chevron.down")
                                .font(.system(size: 10, weight: .semibold))
                                .foregroundStyle(t.text3)
                        }
                        .frame(minHeight: 44)
                        .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(sessionTitle)
                    .accessibilityHint(String(localized: "chat_session_panel_a11y"))
                    .accessibilityValue("\(sessionTitle), \(sessionMode.title) · \(workspaceName)")
                    .accessibilityIdentifier("conversation.project-context")
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    #if canImport(harness_runtimeFFI)
                    if let engine = source as? EngineConversationSource, !engine.isSideChat {
                        Button { sideChatSheetOpen = true } label: {
                            toolbarIcon("bubble.left.and.bubble.right", label: String(localized: "side_chats_title"))
                        }
                        .tint(t.text)
                        .accessibilityIdentifier("conversation.side-chats")
                    }
                    #endif
                    Button(action: newChat) {
                        toolbarIcon("square.and.pencil", label: String(localized: "chat_new_chat"))
                    }
                    .tint(t.text)
                    .accessibilityIdentifier("conversation.new-chat")
                    summaryMenu
                }
            }
        }
    }

    private func toolbarIcon(_ symbol: String, label: String) -> some View {
        // The native toolbar supplies its touch target and optical sizing.
        // A resizable symbol inside another 44pt frame enlarges the system
        // chrome and takes space away from the conversation name.
        Label(label, systemImage: symbol)
            .labelStyle(.iconOnly)
            .font(.system(size: 17, weight: .regular))
            .imageScale(.medium)
            .foregroundStyle(t.text)
    }

    // MARK: session panel

    private var sessionTitle: String {
        convo.sessionReference(for: session.id, cachedTitle: session.title).title
    }

    /// The new-chat strip is only for a chat that has not started yet.
    private var showsNewChatBar: Bool {
        !source.isTemporaryConversation && convo.isNew && renderItems.isEmpty && !convo.streaming
    }

    private var activeModelName: String {
        let id = convo.activeModelId
        guard !id.isEmpty else { return "—" }
        return convo.activeModelSource ?? convo.availableModelDetails[id]?.displayName ?? id
    }

    private var effortLabel: String {
        let selection = convo.reasoningSelection
        if selection.hasPrefix("budget:") { return "\(selection.dropFirst("budget:".count)) tokens" }
        return selection.isEmpty || selection == "automatic" ? "Auto" : selection.capitalized
    }

    private var permissionLabel: String {
        switch convo.effectivePermissionMode {
        case "bypassPermissions": return String(localized: "settings_permission_mode_bypass")
        case "auto": return String(localized: "settings_permission_mode_auto")
        case "default": return String(localized: "settings_permission_mode_default")
        case "acceptEdits": return String(localized: "settings_permission_mode_accept_edits")
        case "plan": return String(localized: "settings_permission_mode_plan")
        case "dontAsk": return String(localized: "settings_permission_mode_dont_ask")
        default: return convo.effectivePermissionMode
        }
    }

    private func runSessionPanelFollowUp() {
        guard let action = sessionPanelFollowUp else { return }
        sessionPanelFollowUp = nil
        switch action {
        case .model: modelPickerOpen = true
        case .permission: permissionSheetOpen = true
        case .details: onOpenSessionDetails()
        case .togglePinned: onTogglePinned()
        case .makeScheduled: onMakeScheduled()
        case .terminal: onOpenTerminal()
        case .continueInOtherMode: onContinueInOtherMode()
        case .archive: onArchive()
        }
    }

    private var summaryMenu: some View {
        Menu {
            Button(action: onOpenFiles) { Label(FilesStrings.text("files"), systemImage: "folder") }
                .accessibilityIdentifier("conversation.files")
            ForEach(summaryCategories) { category in
                Button {
                    summaryDetent = .medium
                    summaryCategory = category
                } label: {
                    Label(category.title, systemImage: category.systemImage)
                }
                .accessibilityIdentifier("conversation.summary.category.\(category.rawValue)")
            }
        } label: {
            toolbarIcon("ellipsis", label: String(localized: "chat_open_summary"))
        }
        .menuStyle(.automatic)
        .buttonStyle(.automatic)
        .tint(t.text)
        .accessibilityLabel(String(localized: "chat_open_summary"))
        .accessibilityIdentifier("conversation.summary-menu")
    }

    private var summaryCategories: [ConversationSummaryCategory] {
        [.changes, .tools, .agents, .resources, .plan]
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
        // `TranscriptScroll` owns the scroll behaviour; what stays here is this
        // screen's content and its own definition of "something new arrived".
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
            focused: showsSelectedAgent ? false : composerFocused,
            latestRequestRevision: showsSelectedAgent ? 0 : latestMessageRequestRevision,
            accessibilityIdentifier: showsSelectedAgent ? "conversation.agent-message-list" : "conversation.message-list",
            maxContentWidth: 860,
            scrollScope: "\(session.id):\(showsSelectedAgent ? convo.selectedAgentID : ConversationModel.mainAgentID)",
            alignShortContentToTop: true,
            requestedRowID: showsSelectedAgent ? nil : convo.deepLinkScrollRowID,
            requestedScrollRevision: showsSelectedAgent ? 0 : convo.deepLinkScrollRevision
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
                    transcriptAgents: convo.orderedAgentSummaries,
                    transcriptAgentAnchors: convo.transcriptAgentAnchors,
                    activeAgentID: convo.selectedAgentID,
                    onSelectAgent: { source.selectAgent($0) },
                    streaming: streaming,
                    showThinking: canShowTranscriptThinking,
                    // An agent's widgets live in its origin session's store.
                    visualizationSessionId: convo.activeSessionId,
                    highlightedRowID: convo.deepLinkScrollRowID,
                    onVisualizationFollowup: { convo.visualizationFollowup = $0 }
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
                    streaming: streaming,
                    showThinking: canShowTranscriptThinking,
                    // An agent's widgets live in its origin session's store.
                    visualizationSessionId: convo.activeSessionId,
                    highlightedRowID: convo.deepLinkScrollRowID,
                    onVisualizationFollowup: { convo.visualizationFollowup = $0 }
                )
            }
        }
        .sheet(item: $convo.deepLinkRuntimeTarget) { target in
            DeepLinkRuntimeDetail(target: target, source: source, convo: convo)
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
        #if canImport(harness_runtimeFFI)
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
    private func answerQuestion(_ requestId: UInt64, answers: AskUserQuestionAnswersDto) async -> Bool {
        #if canImport(harness_runtimeFFI)
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
        #if canImport(harness_runtimeFFI)
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
                .font(.title2.weight(.semibold))
                .foregroundStyle(t.text)
                .lineLimit(2)
            Text("chat_new_chat_hint")
                .font(.subheadline)
                .foregroundStyle(t.text3)
                .multilineTextAlignment(.center)
                .lineSpacing(3)
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

    /// Keep failures compact beside the composer; the complete diagnostic is
    /// available in a native sheet. Durable completion remains silent.
    @ViewBuilder
    private var runtimeFooter: some View {
        VStack(spacing: 0) {
            switch runtimeFooterState {
            case .hidden:
                EmptyView()
            case let .compaction(status):
                CompactionRuntimeFooter(status: status)
            case let .error(message):
                RuntimeErrorBanner(
                    title: runtimeErrorTitle(message),
                    message: message,
                    onDismiss: dismissRuntimeError
                )
                .id(message)
            }
            if !convo.recoveryEvidenceForReview.isEmpty {
                HStack(spacing: 10) {
                    Image(systemName: "exclamationmark.circle")
                    VStack(alignment: .leading, spacing: 3) {
                        Text(String(format: String(localized: "chat_recovery_warning_count %@"),
                                    convo.recoveryEvidenceForReview.count.formatted()))
                        if convo.recoveryEvidenceForReview.contains(where: { $0.projectionUncertain || $0.liveOnlyOutput }) {
                            Text("chat_recovery_projection_uncertain")
                        }
                        if convo.recoveryEvidenceForReview.contains(where: { $0.queuedCancelled == true }) {
                            Text("chat_queued_cancelled")
                        }
                    }
                    .font(.callout)
                    Spacer(minLength: 8)
                    Button("chat_recovery_review_button") {
                        recoveryEvidenceReviewSnapshot = convo.recoveryEvidenceForReview
                        showRecoveryEvidenceConfirmation = true
                    }
                    .font(.callout.weight(.semibold))
                    .accessibilityIdentifier("conversation.recovery.review")
                }
                .foregroundStyle(t.text2)
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
            }
        }
        .alert("chat_recovery_review_title", isPresented: $showRecoveryEvidenceConfirmation) {
            Button("common_cancel", role: .cancel) { recoveryEvidenceReviewSnapshot = [] }
            Button("chat_recovery_review_confirm") {
                source.acknowledgeRecoveryEvidence(recoveryEvidenceReviewSnapshot)
                recoveryEvidenceReviewSnapshot = []
            }
        } message: {
            Text(String(format: String(localized: "chat_recovery_review_message %@"),
                        recoveryEvidenceReviewSnapshot.count.formatted()))
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

    private func runtimeErrorTitle(_ message: String) -> String {
        let resumeFailure = String(localized: "chat_resume_session_failed")
        if message.hasPrefix(resumeFailure) { return resumeFailure }
        if captureStatus == nil, let error = convo.error { return error.kind.label }
        return String(localized: "chat_error_generic_headline")
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
        if let onNewChat {
            onNewChat()
        } else {
            source.startNewConversation()
        }
    }

    @discardableResult
    private func send(
        _ txt: String,
        sourceCodeUnits: [UInt16]? = nil,
        selectedSlashCommand: ConversationSlashCommand? = nil
    ) -> Bool {
        #if canImport(harness_runtimeFFI)
        if let engine = source as? EngineConversationSource,
           !engine.isSideChat,
           let input = SideChatCommand.input(txt) {
            guard attachment == nil else {
                convo.error = ConversationError(kind: .rejected, message: String(localized: "side_chat_creation_text_only"))
                return false
            }
            let submittedDraft = draft
            let submittedSourceCodeUnits = sourceCodeUnits
            let inputSourceCodeUnits = SideChatCommand.inputSourceCodeUnits(
                raw: txt,
                sourceCodeUnits: sourceCodeUnits
            )
            guard sourceCodeUnits == nil || input.isEmpty || inputSourceCodeUnits != nil else { return false }
            engine.sideChats.create(
                input: input.isEmpty ? nil : input,
                inputSourceCodeUnits: input.isEmpty ? nil : inputSourceCodeUnits
            ) { success in
                if success {
                    if Array(draft.utf16) == Array(submittedDraft.utf16),
                       draftSourceCodeUnitsBinding.wrappedValue == submittedSourceCodeUnits {
                        draft = ""
                        draftSourceCodeUnitsBinding.wrappedValue = nil
                    }
                    sideChatSheetOpen = true
                }
            }
            return false
        }
        #endif
        let images = attachment.map { [ImageRefDto(mediaType: $0.mediaType, base64: $0.base64)] } ?? []
        guard let token = source.send(
            txt,
            sourceCodeUnits: sourceCodeUnits,
            images: images,
            selectedSlashCommand: selectedSlashCommand
        ) else { return false }
        draft = ""
        draftSourceCodeUnitsBinding.wrappedValue = nil
        // The source owns the durable transcript now; remove only after it has
        // accepted the turn so a failed send leaves the attachment retryable.
        attachment = nil
        voiceInteraction.registerAutomaticPlaybackCandidate(token)
        // Sending is an explicit re-engagement: a reader who had scrolled up to
        // re-read something must not have their own prompt land off screen. The
        // re-arm happens only after the turn was accepted.
        latestMessageRequestRevision &+= 1
        return true
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
        guard !source.isTemporaryConversation else { return }
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
                    detail: percent.map { String(localized: "chat_compaction_progress \($0) \(Int(elapsed))") },
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

/// A non-blocking error summary. Technical details never compete with the
/// draft, and opening or closing the sheet does not acknowledge the error.
private struct RuntimeErrorBanner: View {
    @Environment(\.theme) private var t
    @Environment(\.colorSchemeContrast) private var contrast
    @ScaledMetric(relativeTo: .subheadline) private var symbolSize: CGFloat = 20
    let title: String
    let message: String
    let onDismiss: () -> Void
    @State private var showsDetails = false
    @State private var copied = false

    var body: some View {
        HStack(spacing: 0) {
            Button { showsDetails = true } label: {
                HStack(spacing: 10) {
                    Image(systemName: "exclamationmark.circle.fill")
                        .font(.system(size: min(symbolSize, 26)))
                        .foregroundStyle(t.danger)
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(title)
                            .font(.subheadline.weight(.semibold))
                            .foregroundStyle(t.text)
                            .fixedSize(horizontal: false, vertical: true)
                        HStack(spacing: 4) {
                            Text("chat_error_view_details")
                            Image(systemName: "chevron.right")
                                .font(.caption2.weight(.semibold))
                        }
                        .font(.caption)
                        .foregroundStyle(t.text2)
                    }
                    Spacer(minLength: 0)
                }
                .padding(.leading, 14)
                .padding(.vertical, 10)
                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("conversation.runtime-error.details")

            Button(action: onDismiss) {
                Image(systemName: "xmark")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(t.text2)
                    .frame(width: 28, height: 28)
                    .background(t.surfaceHover, in: Circle())
                    .frame(width: 44, height: 44)
                    .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("chat_dismiss_error")
            .accessibilityIdentifier("conversation.runtime-error.dismiss")
            .padding(.trailing, 4)
        }
        .background(t.surface, in: .rect(cornerRadius: 18, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 18, style: .continuous)
                .strokeBorder(t.danger.opacity(contrast == .increased ? 0.8 : 0.18),
                              lineWidth: contrast == .increased ? 1 : 0.5)
                .allowsHitTesting(false)
        }
        .padding(.horizontal, 14)
        .padding(.top, 8)
        .fixedSize(horizontal: false, vertical: true)
        .layoutPriority(1)
        .accessibilityIdentifier("conversation.llm-status")
        .sheet(isPresented: $showsDetails, onDismiss: { copied = false }) {
            NavigationStack {
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        Label(title, systemImage: "exclamationmark.circle")
                            .font(.headline)
                            .foregroundStyle(t.text)
                        Text(message)
                            .font(.callout)
                            .foregroundStyle(t.text2)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .accessibilityIdentifier("conversation.runtime-error.message")
                    }
                    .padding(20)
                }
                .background(t.windowBg)
                .navigationTitle("chat_error_details_title")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("common_close", systemImage: "xmark") { showsDetails = false }
                            .accessibilityIdentifier("conversation.runtime-error.close-details")
                    }
                    ToolbarItem(placement: .primaryAction) {
                        Button {
                            UIPasteboard.general.string = message
                            copied = true
                        } label: {
                            Label(copied ? "chat_message_copied" : "chat_message_copy",
                                  systemImage: copied ? "checkmark" : "doc.on.doc")
                        }
                        .accessibilityIdentifier("conversation.runtime-error.copy")
                    }
                }
            }
            .tint(t.accent)
            .presentationDetents([.medium, .large])
            .presentationDragIndicator(.visible)
        }
    }
}

private struct CompactRejectionAlert: View {
    @Environment(\.theme) private var t
    let onDismiss: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.circle")
                .accessibilityHidden(true)
            Text(String(localized: "chat_compact_during_turn"))
                .font(.subheadline.weight(.medium))
                .fixedSize(horizontal: false, vertical: true)
            Button(action: onDismiss) {
                Image(systemName: "xmark")
                    .font(.caption.weight(.semibold))
                    .frame(minWidth: 44, minHeight: 44)
                    .contentShape(.rect)
            }
            .accessibilityLabel(String(localized: "chat_dismiss_error"))
        }
        .foregroundStyle(t.danger)
        .padding(.leading, 14)
        .padding(.trailing, 4)
        .padding(.vertical, 4)
        .background {
            RoundedRectangle(cornerRadius: 14).fill(t.windowBg)
                .overlay { RoundedRectangle(cornerRadius: 14).fill(t.danger.opacity(0.06)) }
        }
        .overlay { RoundedRectangle(cornerRadius: 14).strokeBorder(t.danger.opacity(0.22), lineWidth: 1) }
        .shadow(color: .black.opacity(0.09), radius: 12, y: 4)
        .frame(maxWidth: 560)
        .accessibilityIdentifier("conversation.compact-rejection")
        .onAppear {
            UIAccessibility.post(notification: .announcement, argument: String(localized: "chat_compact_during_turn"))
        }
    }
}
