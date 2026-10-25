import SwiftUI

enum VoiceHoldGesturePolicy {
    static let upwardCancelThreshold: CGFloat = -72

    static func shouldCancel(verticalTranslation: CGFloat) -> Bool {
        verticalTranslation <= upwardCancelThreshold
    }
}

// MARK: - Composer (pill text field + model chip + attach + send/mic)
struct Composer: View {
    @Environment(\.theme) private var t
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Binding var inputFocused: Bool
    @Binding var model: ModelOption
    // The picker is driven exclusively by the engine's curated, provider-qualified
    // refs. Empty means the catalog is still loading; no mock rows are invented.
    // `model` remains in the view API for compatibility with its existing owner.
    var availableModels: [String] = []
    var availableModelDetails: [String: ModelRuntimeDetails] = [:]
    var contextUsage: ConversationUsageSnapshot? = nil
    var activeModelId: String = ""
    var activeModelDisplay: String? = nil
    var providerConfigured: Bool
    var onSelectModel: (String) -> Void = { _ in }
    var reasoningModelId: String? = nil
    var reasoningSelection: String = "automatic"
    var reasoningOptions: [String] = []
    var reasoningOptionDetails: [ConversationReasoningOption] = []
    var reasoningBudgetRange: ClosedRange<UInt64>? = nil
    var reasoningDisabledReason: String? = nil
    var permissionMode: String = "auto"
    var effectivePermissionMode: String = "auto"
    var permissionOptions: [ConversationPermissionOption] = []
    var fastModeEnabled: Bool = false
    var fastModePending: Bool = false
    var fastModeError: String? = nil
    var onSetFastMode: (Bool) -> Void = { _ in }
    var showsFastMode = true
    var controlsPending: Bool = false
    var controlsError: String? = nil
    var bypassWarningSuppressed: Bool = false
    var onSelectReasoning: (String) -> Void = { _ in }
    var onSelectPermission: (String) -> Void = { _ in }
    var onConfirmBypassPermissions: (Bool) -> Void = { _ in }
    /// Previously-picked refs, most-recent-first, pinned above the provider
    /// sections in the picker. Owned by the caller (see ChatView) because it
    /// outlives any one composer instance.
    var recentModels: [String] = []
    /// Draft text, HOISTED so the device capabilities can inject into it:
    /// hold-to-talk STT fills it with the recognized utterance (mirrors Android
    /// `onTranscript` → draft). The caller owns it (see ChatView).
    @Binding var draft: String
    @Binding var draftSourceCodeUnits: [UInt16]?
    let onSend: (String, [UInt16]?, ConversationSlashCommand?) -> Bool
    @State private var selectedSlashCommand: ConversationSlashCommand? = nil
    var clearsDraftOnSend = true
    var promptSuggestion: String? = nil
    var promptSuggestionSourceCodeUnits: [UInt16]? = nil
    var promptSuggestionNotice: String? = nil
    var suggestionTurnKey: String = ""
    var onSuggestionFeedback: (String) -> Void = { _ in }
    @State private var shownSuggestionKey: String?
    @State private var rejectedSuggestionKey: String?
    @State private var draftSourceDisplayCodeUnits: [UInt16]?

    /// Engine-owned slash-command catalog. An empty loaded catalog is
    /// authoritative; `slashCommandsLoaded == false` renders a loading row and
    /// never substitutes static commands.
    var slashCommands: [ConversationSlashCommand] = []
    var slashCommandsLoaded: Bool = false
    var slashCommandPending: Bool = false

    // A running turn keeps both controls available: Stop interrupts it, while
    // Send places new text into Rust's canonical pending-message queue.
    var streaming: Bool = false
    /// True when a durable turn is inactive while waiting for explicit user
    /// resolution. This changes only the trailing affordance; the source's
    /// actual streaming state remains separate for background/voice policy.
    var showDiscardRecovery: Bool = false
    var isCancelling: Bool = false
    /// Session transitions and cancellation can temporarily make a draft
    /// non-submittable even though the text field remains editable.
    var sendEnabled: Bool = true
    /// The widget a follow-up refers to; sent with the next prompt.
    var visualizationChip: VisualizationContextChip? = nil
    var onRemoveVisualizationChip: () -> Void = {}
    var onStop: () -> Void = {}

    // Camera affordance (the + / attach button) — mirrors Android `onCameraClick`
    // + the captured-photo `attachment` chip surfaced for review before sending.
    var onCameraClick: () -> Void = {}
    var attachment: ComposerAttachment? = nil
    var onRemoveAttachment: () -> Void = {}

    // Hold-to-talk on the ordinary mic affordance — mirrors Android
    // `onMicHoldStart` / `onMicHoldRelease`. The finger lift drives the STT
    // transcription that fills the draft.
    var onMicHoldStart: () -> Void = {}
    var onMicHoldRelease: () -> Void = {}
    var onMicHoldCancel: () -> Void = {}

    // Android exposes ordinary recording and Flow Mode as separate actions. A
    // single mic tap toggles recording; the neighboring waveform opens Flow.
    var onMicTap: () -> Void = {}
    var onFlowModeTap: () -> Void = {}
    var showsFlowMode = true
    var voiceCapturePhase: VoiceCapturePhase = .idle
    var voiceInteractionMode: VoiceInteractionMode? = nil

    /// Owned by the chat so the session panel can open the same pickers.
    @Binding var modelOpen: Bool
    @Binding var controlsOpen: Bool
    /// A Chat session has no file permissions to ask about, so only Code
    /// sessions get the permission chip.
    var showsPermissionChip: Bool = true
    @State private var holding = false
    @State private var cancellingHold = false
    @State private var suppressMicTap = false
    @State private var dismissedSlashDraft: String?

    init(
        model: Binding<ModelOption>,
        availableModels: [String] = [],
        availableModelDetails: [String: ModelRuntimeDetails] = [:],
        activeModelId: String = "",
        activeModelDisplay: String? = nil,
        contextUsage: ConversationUsageSnapshot? = nil,
        providerConfigured: Bool,
        onSelectModel: @escaping (String) -> Void = { _ in },
        reasoningModelId: String? = nil,
        reasoningSelection: String = "automatic",
        reasoningOptions: [String] = [],
        reasoningOptionDetails: [ConversationReasoningOption] = [],
        reasoningBudgetRange: ClosedRange<UInt64>? = nil,
        reasoningDisabledReason: String? = nil,
        permissionMode: String = "auto",
        effectivePermissionMode: String = "auto",
        permissionOptions: [ConversationPermissionOption] = [],
        fastModeEnabled: Bool = false,
        fastModePending: Bool = false,
        fastModeError: String? = nil,
        onSetFastMode: @escaping (Bool) -> Void = { _ in },
        showsFastMode: Bool = true,
        controlsPending: Bool = false,
        controlsError: String? = nil,
        bypassWarningSuppressed: Bool = false,
        onSelectReasoning: @escaping (String) -> Void = { _ in },
        onSelectPermission: @escaping (String) -> Void = { _ in },
        onConfirmBypassPermissions: @escaping (Bool) -> Void = { _ in },
        recentModels: [String] = [],
        draft: Binding<String>,
        draftSourceCodeUnits: Binding<[UInt16]?>,
        onSend: @escaping (String, [UInt16]?, ConversationSlashCommand?) -> Bool,
        promptSuggestion: String? = nil,
        promptSuggestionSourceCodeUnits: [UInt16]? = nil,
        promptSuggestionNotice: String? = nil,
        suggestionTurnKey: String = "",
        onSuggestionFeedback: @escaping (String) -> Void = { _ in },
        slashCommands: [ConversationSlashCommand] = [],
        slashCommandsLoaded: Bool = false,
        slashCommandPending: Bool = false,
        streaming: Bool = false,
        showDiscardRecovery: Bool = false,
        isCancelling: Bool = false,
        sendEnabled: Bool = true,
        onStop: @escaping () -> Void = {},
        onCameraClick: @escaping () -> Void = {},
        inputFocused: Binding<Bool>,
        attachment: ComposerAttachment? = nil,
        onRemoveAttachment: @escaping () -> Void = {},
        visualizationChip: VisualizationContextChip? = nil,
        onRemoveVisualizationChip: @escaping () -> Void = {},
        onMicHoldStart: @escaping () -> Void = {},
        onMicHoldRelease: @escaping () -> Void = {},
        onMicHoldCancel: @escaping () -> Void = {},
        onMicTap: @escaping () -> Void = {},
        onFlowModeTap: @escaping () -> Void = {},
        showsFlowMode: Bool = true,
        voiceCapturePhase: VoiceCapturePhase = .idle,
        voiceInteractionMode: VoiceInteractionMode? = nil,
        modelOpen: Binding<Bool>,
        controlsOpen: Binding<Bool>,
        showsPermissionChip: Bool = true
    ) {
        self._model = model
        self.availableModels = availableModels
        self.availableModelDetails = availableModelDetails
        self.activeModelId = activeModelId
        self.activeModelDisplay = activeModelDisplay
        self.contextUsage = contextUsage
        self.providerConfigured = providerConfigured
        self.onSelectModel = onSelectModel
        self.reasoningModelId = reasoningModelId
        self.reasoningSelection = reasoningSelection
        self.reasoningOptions = reasoningOptions
        self.reasoningOptionDetails = reasoningOptionDetails
        self.reasoningBudgetRange = reasoningBudgetRange
        self.reasoningDisabledReason = reasoningDisabledReason
        self.permissionMode = permissionMode
        self.effectivePermissionMode = effectivePermissionMode
        self.permissionOptions = permissionOptions
        self.fastModeEnabled = fastModeEnabled
        self.fastModePending = fastModePending
        self.fastModeError = fastModeError
        self.onSetFastMode = onSetFastMode
        self.showsFastMode = showsFastMode
        self.controlsPending = controlsPending
        self.controlsError = controlsError
        self.bypassWarningSuppressed = bypassWarningSuppressed
        self.onSelectReasoning = onSelectReasoning
        self.onSelectPermission = onSelectPermission
        self.onConfirmBypassPermissions = onConfirmBypassPermissions
        self.recentModels = recentModels
        self._draft = draft
        self._draftSourceCodeUnits = draftSourceCodeUnits
        self.onSend = onSend
        self.promptSuggestion = promptSuggestion
        self.promptSuggestionSourceCodeUnits = promptSuggestionSourceCodeUnits
        self.promptSuggestionNotice = promptSuggestionNotice
        self.suggestionTurnKey = suggestionTurnKey
        self.onSuggestionFeedback = onSuggestionFeedback
        self.slashCommands = slashCommands
        self.slashCommandsLoaded = slashCommandsLoaded
        self.slashCommandPending = slashCommandPending
        self.streaming = streaming
        self.showDiscardRecovery = showDiscardRecovery
        self.isCancelling = isCancelling
        self.sendEnabled = sendEnabled
        self.onStop = onStop
        self.onCameraClick = onCameraClick
        self._inputFocused = inputFocused
        self.attachment = attachment
        self.onRemoveAttachment = onRemoveAttachment
        self.visualizationChip = visualizationChip
        self.onRemoveVisualizationChip = onRemoveVisualizationChip
        self.onMicHoldStart = onMicHoldStart
        self.onMicHoldRelease = onMicHoldRelease
        self.onMicHoldCancel = onMicHoldCancel
        self.onMicTap = onMicTap
        self.onFlowModeTap = onFlowModeTap
        self.showsFlowMode = showsFlowMode
        self.voiceCapturePhase = voiceCapturePhase
        self.voiceInteractionMode = voiceInteractionMode
        self._modelOpen = modelOpen
        self._controlsOpen = controlsOpen
        self.showsPermissionChip = showsPermissionChip
    }

    var body: some View {
        VStack(spacing: 0) {
            if shouldShowSlashPanel {
                SlashCommandSuggestionPanel(
                    suggestions: slashSuggestions,
                    isLoading: !slashCommandsLoaded,
                    selectedID: slashSuggestions.first?.id,
                    onSelect: acceptSlashCommand
                )
                .padding(.horizontal, 14)
                .layoutPriority(1)
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            VStack(alignment: .leading, spacing: 4) {
                // Captured-photo attachment chip (the device-vision analog of how
                // a transcript lands in the draft): a thumbnail + a remove button,
                // shown only once a camera capture has surfaced an image.
                if let attachment {
                    AttachmentThumb(attachment: attachment, onRemove: onRemoveAttachment)
                }

                if let visualizationChip {
                    VisualizationContextChipView(chip: visualizationChip, onDismiss: onRemoveVisualizationChip)
                        .padding(.horizontal, 4)
                }

                ComposerTextInput(
                    text: $draft,
                    isFocused: $inputFocused,
                    placeholder: visibleSuggestion == nil ? String(localized: "composer_placeholder") : "",
                    textColor: t.text,
                    hintColor: t.text4,
                    argumentHint: { text, cursor in
                        guard slashCommandsLoaded else { return nil }
                        return SlashCommandMatcher.argumentHint(for: text, catalog: slashCommands, cursor: cursor)
                    },
                    onSubmit: send,
                    onAcceptSuggestion: visibleSuggestion == nil ? nil : acceptSuggestion
                )
                    .padding(.horizontal, 4).padding(.vertical, 2)
                    .onChange(of: Array(draft.utf16)) { _, newDisplayCodeUnits in
                        updateDraftSourceCodeUnits(forDisplayCodeUnits: newDisplayCodeUnits)
                        if dismissedSlashDraft != draft {
                            dismissedSlashDraft = nil
                        }
                    }
                    .onAppear {
                        synchronizeDraftSourceCodeUnits()
                        recordSuggestionPresentation()
                    }
                    .onChange(of: suggestionTurnKey) { _, _ in
                        clearDraftSourceCodeUnits()
                        recordSuggestionPresentation()
                    }
                    .onChange(of: draftSourceCodeUnits) { _, _ in
                        synchronizeDraftSourceCodeUnits()
                    }
                    .onChange(of: promptSuggestionSourceCodeUnits) { _, _ in
                        if draft.isEmpty { clearDraftSourceCodeUnits() }
                    }
                    .overlay(alignment: .topLeading) {
                        if let visibleSuggestion {
                            Button(action: acceptSuggestion) {
                                Text(visibleSuggestion)
                                    .font(.body)
                                    .foregroundStyle(t.text4)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                            .buttonStyle(.plain)
                            .padding(.horizontal, 4).padding(.vertical, 2)
                            .accessibilityLabel(String(localized: "composer_use_suggestion \(visibleSuggestion)"))
                            .accessibilityIdentifier("composer.prompt-suggestion")
                            .onAppear {
                                if shownSuggestionKey != suggestionTurnKey {
                                    shownSuggestionKey = suggestionTurnKey
                                    onSuggestionFeedback("shown")
                                }
                            }
                            .onChange(of: suggestionTurnKey) { _, _ in
                                if shownSuggestionKey != suggestionTurnKey {
                                    shownSuggestionKey = suggestionTurnKey
                                    onSuggestionFeedback("shown")
                                }
                            }
                        }
                    }

                if let promptSuggestionNotice {
                    Text(promptSuggestionNotice)
                        .font(.caption).foregroundStyle(t.text4)
                }
                if let fastModeError {
                    Text(fastModeError)
                        .font(.caption)
                        .foregroundStyle(t.danger)
                        .accessibilityIdentifier("composer.fast-mode.error")
                }
                ViewThatFits(in: .horizontal) {
                    toolbar(showsModelText: true)
                    toolbar(showsModelText: false)
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("composer.toolbar")
            }
            .padding(.horizontal, 12).padding(.top, 14).padding(.bottom, 6)
            .background(t.composerBg)
            .clipShape(.rect(cornerRadius: 24))
            .overlay {
                RoundedRectangle(cornerRadius: 24)
                    .stroke(inputFocused ? t.accent.opacity(0.45) : t.border.opacity(0.45),
                            lineWidth: inputFocused ? 1 : 0.5)
            }
            .animation(reduceMotion ? nil : .easeOut(duration: 0.18), value: inputFocused)
        }
        .padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 4)
        // Let the transcript yield height instead of compressing controls out
        // of the card when Dynamic Type increases the editor and model row.
        .fixedSize(horizontal: false, vertical: true)
        .layoutPriority(1)
        .animation(reduceMotion ? nil : .spring(response: 0.3, dampingFraction: 1), value: shouldShowSlashPanel)
        // A sheet, not a popover anchored to the chip: the picker's search field
        // raises the keyboard over exactly the space a popover above the
        // composer would occupy, and the system sizes a sheet against the
        // keyboard for us.
        .sheet(isPresented: $modelOpen) {
            ModelPickerSheet(
                availableModels: availableModels,
                detailsByReference: availableModelDetails,
                activeModelId: activeModelId,
                recentModels: recentModels,
                onSelect: { reference in
                    onSelectModel(reference)
                },
                onDismiss: { modelOpen = false },
                reasoningModelId: reasoningModelId,
                reasoningSelection: reasoningSelection,
                reasoningOptions: reasoningOptionDetails.isEmpty ? reasoningOptions.map {
                    ConversationReasoningOption(id: $0, title: $0 == "automatic" ? "Auto" : $0.capitalized,
                                                isBudget: $0.hasPrefix("budget:"), persistable: true)
                } : reasoningOptionDetails,
                reasoningBudgetRange: reasoningBudgetRange,
                reasoningDisabledReason: reasoningDisabledReason,
                controlsPending: controlsPending,
                controlsError: controlsError,
                onSelectReasoning: onSelectReasoning,
                fastModeEnabled: fastModeEnabled,
                fastModePending: fastModePending,
                fastModeError: fastModeError,
                onSetFastMode: onSetFastMode,
                showsFastMode: showsFastMode)

        }
        .sheet(isPresented: $controlsOpen) {
            ConversationControlsSheet(
                permissionMode: permissionMode,
                effectivePermissionMode: effectivePermissionMode,
                permissionOptions: permissionOptions,
                controlsPending: controlsPending,
                controlsError: controlsError,
                bypassWarningSuppressed: bypassWarningSuppressed,
                onSelectPermission: { value in onSelectPermission(value) },
                onConfirmBypassPermissions: onConfirmBypassPermissions,
                onDismiss: { controlsOpen = false }
            )
        }
        #if canImport(harness_runtimeFFI)
            .onChange(of: modelOpen) { _, _ in
                NotificationCenter.default.post(
                    name: .lingxiPermissionPresentationContextChanged,
                    object: nil
                )
            }
            .onChange(of: controlsOpen) { _, _ in
                NotificationCenter.default.post(
                    name: .lingxiPermissionPresentationContextChanged,
                    object: nil
                )
            }
        #endif
    }

    // Press-and-hold → release, mirroring Android `voiceHold`: a 0.6s
    // LongPress sequenced into a Drag so the same
    // touch that crosses the threshold (onMicHoldStart) is the one whose lift we
    // detect (onMicHoldRelease → STT).
    private var micHoldGesture: some Gesture {
        LongPressGesture(minimumDuration: 0.6)
            .sequenced(before: DragGesture(minimumDistance: 0))
            .onChanged { value in
                if case let .second(true, drag) = value {
                    if !holding {
                        holding = true
                        cancellingHold = false
                        suppressMicTap = true
                        onMicHoldStart()
                    }
                    if let drag {
                        cancellingHold = VoiceHoldGesturePolicy.shouldCancel(
                            verticalTranslation: drag.translation.height
                        )
                    }
                }
            }
            .onEnded { value in
                if holding {
                    let shouldCancel: Bool
                    if case let .second(_, drag) = value, let drag {
                        shouldCancel = VoiceHoldGesturePolicy.shouldCancel(
                            verticalTranslation: drag.translation.height
                        )
                    } else {
                        shouldCancel = cancellingHold
                    }
                    holding = false
                    cancellingHold = false
                    if shouldCancel {
                        onMicHoldCancel()
                    } else {
                        onMicHoldRelease()
                    }
                    Task { @MainActor in
                        try? await Task.sleep(for: .milliseconds(150))
                        suppressMicTap = false
                    }
                }
            }
    }

    private func handleMicTap() {
        guard !suppressMicTap else {
            suppressMicTap = false
            return
        }
        onMicTap()
    }

    /// Desktop keeps ordinary recording available while drafting and while a
    /// turn is running. Flow Mode is the only voice affordance gated to idle.
    private var ordinaryMicButton: some View {
        Button(action: handleMicTap) {
            if voiceCapturePhase == .finishing {
                ProgressView()
                    .tint(t.text2)
                    .frame(width: 44, height: 44)
            } else {
                LXIcon(
                    name: cancellingHold || isDictationListening ? .stop : .mic,
                    size: 18,
                    color: cancellingHold ? t.danger
                        : (holding || isDictationListening ? t.accent : t.text2),
                    stroke: 1.8
                )
                .frame(width: 44, height: 44)
                .background(
                    isDictationListening || holding
                        ? t.accent.tint(0.15)
                        : .clear,
                    in: Circle()
                )
            }
        }
        .buttonStyle(ComposerActionButtonStyle())
        .simultaneousGesture(micHoldGesture)
        .accessibilityLabel(
            isDictationListening ? String(localized: "composer_stop_recording") : String(localized: "composer_record")
        )
        .accessibilityHint("composer_voice_hint")
        .accessibilityIdentifier("composer.voice")
        .disabled(
            !sendEnabled
                || voiceCapturePhase == .finishing
                || voiceInteractionMode == .flow
        )
    }

    private var flowModeButton: some View {
        Button(action: onFlowModeTap) {
            LXIcon(
                name: .audioWave,
                size: 18,
                color: voiceInteractionMode == .flow ? t.accent : t.text2,
                stroke: 1.8
            )
            .frame(width: 44, height: 44)
            .background(
                voiceInteractionMode == .flow
                    ? t.accent.tint(0.15)
                    : .clear,
                in: Circle()
            )
        }
        .buttonStyle(ComposerActionButtonStyle())
        .accessibilityLabel("composer_flow_mode")
        .accessibilityIdentifier("composer.flow")
        .disabled(!sendEnabled || voiceInteractionMode != nil)
    }

    private var isDictationListening: Bool {
        voiceInteractionMode == .dictation && voiceCapturePhase == .listening
    }

    /// The chip's short label from provider configuration plus authoritative
    /// engine state. A keyless engine can still advertise its built-in model.
    private var chipLabel: String {
        Self.modelChipLabel(
            providerConfigured: providerConfigured,
            availableModels: availableModels,
            activeModelId: activeModelId,
            activeModelDisplay: activeModelDisplay
        )
    }

    static func modelChipLabel(
        providerConfigured: Bool,
        availableModels: [String],
        activeModelId: String,
        activeModelDisplay: String? = nil
    ) -> String {
        guard providerConfigured else {
            return String(localized: "settings_provider_unconfigured")
        }
        return availableModels.isEmpty
            ? String(localized: "composer_loading_model")
            : activeModelDisplay ?? ModelDisplay.shortName(for: activeModelId)
    }

    private var attachmentButton: some View {
        Button(action: onCameraClick) {
            LXIcon(name: .plus, size: 18, color: t.text3, stroke: 1.8)
                .frame(width: 44, height: 44)
        }
        .buttonStyle(ComposerActionButtonStyle())
        .accessibilityLabel("composer_add_attachment")
        .accessibilityIdentifier("composer.attachment")
    }

    @ViewBuilder
    private var turnActions: some View {
        let hasDraft = !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        HStack(spacing: 0) {
            ordinaryMicButton
            if isCancelling || slashCommandPending {
                ProgressView()
                    .tint(t.text3)
                    .frame(width: 44, height: 44)
                    .accessibilityLabel(isCancelling ? "composer_stopping" : "slash_command_running")
            } else if hasDraft {
                // Desktop gives a drafted message precedence over Stop so
                // users can queue text while a turn is running.
                Button(action: send) {
                    ComposerTurnActionIcon(
                        systemName: "arrow.up",
                        symbolSize: 15,
                        background: t.accent
                    )
                }
                .buttonStyle(ComposerActionButtonStyle())
                .accessibilityLabel(streaming ? "composer_queue" : "composer_send")
                .accessibilityIdentifier("composer.send")
                .disabled(!canSubmitDraft)
                .opacity(canSubmitDraft ? 1 : 0.45)
            } else if streaming || showDiscardRecovery {
                Button(action: onStop) {
                    ComposerTurnActionIcon(
                        systemName: "stop.fill",
                        symbolSize: 11,
                        background: t.danger
                    )
                }
                .buttonStyle(ComposerActionButtonStyle())
                .accessibilityLabel(showDiscardRecovery ? "composer_discard_recovery" : "composer_stop")
                .accessibilityIdentifier(showDiscardRecovery ? "composer.discard-recovery" : "composer.stop")
                .disabled(showDiscardRecovery && !sendEnabled)
            } else if showsFlowMode {
                flowModeButton
            }
        }
        .fixedSize(horizontal: true, vertical: false)
        .opacity(sendEnabled ? 1 : 0.45)
    }

    private func modelChip(showsText: Bool) -> some View {
        Button {
            inputFocused = false
            modelOpen = true
        } label: {
            Group {
                if showsText {
                    HStack(spacing: 5) {
                        if fastModePending {
                            ProgressView().controlSize(.mini)
                        } else if fastModeEnabled && supportsFastMode {
                            Image(systemName: "bolt.fill")
                                .accessibilityIdentifier("composer.fast-mode")
                        }
                        Text(chipLabel)
                            .foregroundStyle(t.text)
                        Text(reasoningLabel)
                            .foregroundStyle(t.text3)
                        LXIcon(name: .chevron, size: 11, color: t.text4, stroke: 2)
                    }
                    .font(.subheadline.weight(.medium))
                    .lineLimit(1)
                    .fixedSize(horizontal: true, vertical: false)
                    .padding(.horizontal, 6)
                    .frame(minHeight: 44)
                } else {
                    Group {
                        if fastModePending {
                            ProgressView().controlSize(.small)
                        } else {
                            Image(systemName: fastModeEnabled && supportsFastMode ? "bolt.fill" : "cpu")
                                .font(.system(size: 17, weight: .medium))
                                .foregroundStyle(t.text2)
                        }
                    }
                    .frame(width: 44, height: 44)
                }
            }
        }
        .buttonStyle(ComposerActionButtonStyle())
        .accessibilityLabel(String(localized: "composer_model_and_effort"))
        .accessibilityValue("\(chipLabel), \(reasoningLabel)" + (fastModeEnabled && supportsFastMode ? ", " + String(localized: "composer_fast_mode_on") : ""))
        .accessibilityIdentifier("composer.model")
    }

    @ViewBuilder
    private var contextWindow: some View {
        if let snapshot = ComposerContextUsage(usage: contextUsage, capacity: availableModelDetails[activeModelId]?.contextBudget?.effectiveInputBudget) {
            ComposerContextWindow(snapshot: snapshot, details: availableModelDetails[activeModelId])
        }
    }

    private var permissionChip: some View {
        Button { inputFocused = false; controlsOpen = true } label: {
            Label(permissionLabel, systemImage: effectivePermissionMode == "bypassPermissions"
                  ? "exclamationmark.shield" : "checkmark.shield")
                .labelStyle(.iconOnly)
                .font(.system(size: 17, weight: .medium))
                .foregroundStyle(effectivePermissionMode == "bypassPermissions" ? .orange : t.text2)
                .frame(width: 44, height: 44)
        }
        .buttonStyle(ComposerActionButtonStyle())
        .disabled(controlsPending)
        .accessibilityLabel(String(localized: "settings_permission_mode_title"))
        .accessibilityValue(permissionLabel)
        .accessibilityIdentifier("composer.controls")
    }

    /// Measure the complete row at its natural width before falling back to
    /// icons. Truncating text would let the first candidate claim it fits.
    private func toolbar(showsModelText: Bool) -> some View {
        HStack(spacing: 0) {
            attachmentButton
            if showsPermissionChip { permissionChip }
            Spacer(minLength: 0)
            contextWindow
            modelChip(showsText: showsModelText)
            turnActions
        }
    }

    private var supportsFastMode: Bool {
        availableModelDetails[activeModelId]?.supportsFastMode == true
    }

    private var reasoningLabel: String {
        ModelDetailFormat.reasoningIdentifierLabel(reasoningSelection, fallback: reasoningSelection.capitalized)
    }

    private var permissionLabel: String {
        ModelDetailFormat.permissionModeLabel(effectivePermissionMode)
    }

    private var slashSuggestions: [SlashCommandSuggestion] {
        guard slashCommandsLoaded else { return [] }
        return SlashCommandMatcher.suggestions(for: draft, catalog: slashCommands)
    }

    private var shouldShowSlashPanel: Bool {
        guard dismissedSlashDraft != draft else { return false }
        if isWaitingForSlashCatalog { return true }
        guard draft.hasPrefix("/"), !draft.dropFirst().contains(where: \.isWhitespace) else {
            return false
        }
        return !slashSuggestions.isEmpty
    }

    private var isWaitingForSlashCatalog: Bool {
        ComposerSubmissionPolicy.isWaitingForSlashCatalog(
            draft: draft,
            slashCommandsLoaded: slashCommandsLoaded
        )
    }

    private var canSubmitDraft: Bool {
        sendEnabled
            && !isWaitingForSlashCatalog
            && !(streaming
                && SideChatCommand.input(draft) == nil
                && slashCommandsLoaded
                && SlashCommandMatcher.exactCommand(
                    in: draft.trimmingCharacters(in: .whitespacesAndNewlines),
                    catalog: slashCommands
                ) != nil)
            && !showDiscardRecovery
    }

    private var selectedSlashCommandForDraft: ConversationSlashCommand? {
        guard let selectedSlashCommand,
              let token = SlashCommandMatcher.commandToken(in: draft),
              selectedSlashCommand.matchesExactToken(token)
        else { return nil }
        return selectedSlashCommand
    }

    private func acceptSlashCommand(_ command: ConversationSlashCommand) {
        let accepted = SlashCommandMatcher.completedInput(for: command, in: draft)
        selectedSlashCommand = command
        draft = accepted
        dismissedSlashDraft = accepted
        inputFocused = true
    }

    private func recordSuggestionPresentation() {
        guard promptSuggestion != nil, shownSuggestionKey != suggestionTurnKey,
              rejectedSuggestionKey != suggestionTurnKey else { return }
        if visibleSuggestion != nil {
            shownSuggestionKey = suggestionTurnKey
            onSuggestionFeedback("shown")
        } else {
            rejectedSuggestionKey = suggestionTurnKey
            onSuggestionFeedback("dismissed")
        }
    }

    private var visibleSuggestion: String? {
        guard rejectedSuggestionKey != suggestionTurnKey, draft.isEmpty, !streaming, sendEnabled, attachment == nil,
              visualizationChip == nil else { return nil }
        return promptSuggestion
    }

    private func acceptSuggestion() {
        guard let suggestion = visibleSuggestion else { return }
        onSuggestionFeedback("accepted")
        let sourceCodeUnits = promptSuggestionSourceCodeUnits.flatMap { sourceUnits in
            PromptSuggestionDraftSource.displayCodeUnits(for: sourceUnits) == Array(suggestion.utf16)
                ? sourceUnits
                : nil
        }
        draft = suggestion
        if let sourceCodeUnits {
            draftSourceCodeUnits = sourceCodeUnits
            draftSourceDisplayCodeUnits = Array(suggestion.utf16)
        } else {
            clearDraftSourceCodeUnits()
        }
        inputFocused = true
    }

    private func updateDraftSourceCodeUnits(forDisplayCodeUnits newDisplayCodeUnits: [UInt16]) {
        guard let sourceCodeUnits = draftSourceCodeUnits,
              let previousDisplayCodeUnits = draftSourceDisplayCodeUnits else { return }
        guard let updatedSourceCodeUnits = PromptSuggestionDraftSource.applyingEdit(
            sourceCodeUnits: sourceCodeUnits,
            previousDisplayCodeUnits: previousDisplayCodeUnits,
            newDisplayCodeUnits: newDisplayCodeUnits
        ) else {
            clearDraftSourceCodeUnits()
            return
        }
        if updatedSourceCodeUnits == newDisplayCodeUnits {
            clearDraftSourceCodeUnits()
        } else {
            draftSourceCodeUnits = updatedSourceCodeUnits
            draftSourceDisplayCodeUnits = newDisplayCodeUnits
        }
    }

    private func synchronizeDraftSourceCodeUnits() {
        guard let sourceCodeUnits = draftSourceCodeUnits else {
            draftSourceDisplayCodeUnits = nil
            return
        }
        let displayCodeUnits = Array(draft.utf16)
        guard PromptSuggestionDraftSource.displayCodeUnits(for: sourceCodeUnits) == displayCodeUnits else {
            clearDraftSourceCodeUnits()
            return
        }
        draftSourceDisplayCodeUnits = displayCodeUnits
    }

    private func clearDraftSourceCodeUnits() {
        draftSourceCodeUnits = nil
        draftSourceDisplayCodeUnits = nil
    }

    private func send() {
        let trimmed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty,
              !showDiscardRecovery,
              !isCancelling,
              !slashCommandPending,
              canSubmitDraft
        else { return }
        if slashCommandsLoaded,
           SlashCommandMatcher.shouldAcceptSuggestion(for: draft, catalog: slashCommands),
           let suggestion = slashSuggestions.first {
            acceptSlashCommand(suggestion.command)
            return
        }
        let sourceCodeUnits = draftSourceCodeUnits
        guard onSend(draft, sourceCodeUnits, selectedSlashCommandForDraft) else { return }
        selectedSlashCommand = nil
        if clearsDraftOnSend {
            clearDraftSourceCodeUnits()
            draft = ""
        }
        dismissedSlashDraft = nil
    }
}

enum ComposerSubmissionPolicy {
    static func isWaitingForSlashCatalog(draft: String, slashCommandsLoaded: Bool) -> Bool {
        !slashCommandsLoaded
            && draft.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix("/")
    }
}

private struct ComposerActionButtonStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .frame(minWidth: 44, minHeight: 44)
            .contentShape(.rect)
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.96 : 1)
            .opacity(configuration.isPressed ? 0.86 : 1)
            .animation(reduceMotion ? nil : .spring(response: 0.25, dampingFraction: 1), value: configuration.isPressed)
    }
}

private struct ComposerTurnActionIcon: View {
    let systemName: String
    let symbolSize: CGFloat
    let background: Color

    var body: some View {
        Image(systemName: systemName)
            .font(.system(size: symbolSize, weight: .bold))
            .foregroundStyle(.white)
            .frame(width: 44, height: 44)
            .background(background, in: Circle())
            .overlay {
                Circle().stroke(.white.opacity(0.14), lineWidth: 0.5)
            }
            .shadow(color: .black.opacity(0.10), radius: 3, y: 1)
    }
}

// MARK: - Attachment thumbnail chip

/// A captured-photo thumbnail chip with a remove (×) affordance — mirrors
/// Android `AttachmentThumb`.
private struct AttachmentThumb: View {
    @Environment(\.theme) private var t
    let attachment: ComposerAttachment
    let onRemove: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            #if canImport(UIKit)
                ZStack(alignment: .topTrailing) {
                    Image(uiImage: attachment.image)
                        .resizable()
                        .scaledToFill()
                        .frame(width: 74, height: 62)
                        .clipShape(.rect(cornerRadius: 10))
                        .overlay {
                            RoundedRectangle(cornerRadius: 10)
                                .stroke(t.borderStrong.opacity(0.55), lineWidth: 0.5)
                        }
                    Button(action: onRemove) {
                        LXIcon(name: .x, size: 11, color: .white, stroke: 2)
                            .frame(width: 24, height: 24)
                            .background(.black.opacity(0.62), in: Circle())
                    }
                    .buttonStyle(.plain)
                    .padding(3)
                    .accessibilityLabel("composer_remove_attachment")
                }
            #endif
            VStack(alignment: .leading, spacing: 2) {
                Text(attachment.mediaType == "image/jpeg" ? "Image" : attachment.mediaType)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(t.text2)
                Text("\(attachment.width)×\(attachment.height)")
                    .font(.system(size: 11))
                    .foregroundStyle(t.text3)
            }
            Spacer()
        }
        .padding(.horizontal, 4).padding(.vertical, 2)
    }
}

/// Matches Desktop's latest input footprint, including both cache counters.
struct ComposerContextUsage: Equatable {
    let used: UInt64
    let capacity: UInt64
    var fraction: Double { min(1, Double(used) / Double(capacity)) }
    var percent: Int { Int((fraction * 100).rounded()) }

    init?(usage: ConversationUsageSnapshot?, capacity: UInt64?) {
        guard let usage, let capacity, capacity > 0 else { return nil }
        let (inputAndRead, readOverflow) = usage.inputTokens.addingReportingOverflow(usage.cacheReadTokens)
        let (used, writeOverflow) = inputAndRead.addingReportingOverflow(usage.cacheCreationTokens)
        guard !readOverflow, !writeOverflow else { return nil }
        self.used = used
        self.capacity = capacity
    }
}

private struct ComposerContextWindow: View {
    @Environment(\.theme) private var t
    let snapshot: ComposerContextUsage
    let details: ModelRuntimeDetails?
    @State private var showsDetails = false

    private var summary: String {
        String(localized: "composer_context_summary \(snapshot.percent) \(100 - snapshot.percent)")
    }
    private var tokens: String {
        String(localized: "composer_context_tokens \(ModelDetailFormat.tokenCount(snapshot.used)) \(ModelDetailFormat.tokenCount(snapshot.capacity))")
    }

    private func contextPolicyLabel(_ policy: String) -> String {
        switch policy {
        case "long": return String(localized: "composer_context_policy_long")
        case "custom": return String(localized: "composer_context_policy_custom")
        default: return String(localized: "composer_context_policy_auto")
        }
    }

    var body: some View {
        Button { showsDetails = true } label: {
            ZStack {
                Circle().stroke(t.border, lineWidth: 3)
                if snapshot.used > 0 {
                    Circle().trim(from: 0, to: snapshot.fraction)
                        .stroke(t.text3, style: StrokeStyle(lineWidth: 3, lineCap: .round))
                        .rotationEffect(.degrees(-90))
                }
            }
            .frame(width: 16, height: 16)
            .frame(width: 44, height: 44)
            .contentShape(.rect)
        }
        .buttonStyle(ComposerActionButtonStyle())
        .accessibilityLabel(String(localized: "composer_context_title"))
        .accessibilityValue("\(summary). \(tokens)")
        .accessibilityIdentifier("composer.context-window")
        .popover(isPresented: $showsDetails) {
            VStack(alignment: .leading, spacing: 6) {
                Text("composer_context_title").fontWeight(.semibold)
                Text(summary)
                Text(tokens)
                Text("composer_context_budget_note")
                if let policy = details?.contextBudget?.policy {
                    Text(String(localized: "composer_context_policy \(contextPolicyLabel(policy))"))
                }
                if let maximum = details?.contextWindowTokens {
                    Text(String(localized: "composer_context_model_max \(ModelDetailFormat.tokenCount(maximum))"))
                }
                if let boundary = details?.contextBudget?.standardPriceInputTokens {
                    Text(String(localized: "composer_context_long_price \(boundary.formatted())"))
                }
            }
            .font(.callout)
            .padding()
            .presentationCompactAdaptation(.popover)
        }
    }
}

extension Composer {
    func preserveConversationDraft() -> Composer {
        var copy = self
        copy.clearsDraftOnSend = false
        return copy
    }
}
