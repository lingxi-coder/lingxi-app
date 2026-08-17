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
    @FocusState.Binding var inputFocused: Bool
    @Binding var model: ModelOption
    // The picker is driven exclusively by the engine's curated, provider-qualified
    // refs. Empty means the catalog is still loading; no mock rows are invented.
    // `model` remains in the view API for compatibility with its existing owner.
    var availableModels: [String] = []
    var activeModelId: String = ""
    var onSelectModel: (String) -> Void = { _ in }
    var reasoningSelection: String = "automatic"
    var reasoningOptions: [String] = []
    var reasoningOptionDetails: [ConversationReasoningOption] = []
    var reasoningBudgetRange: ClosedRange<UInt64>? = nil
    var reasoningDisabledReason: String? = nil
    var permissionMode: String = "auto"
    var effectivePermissionMode: String = "auto"
    var permissionOptions: [ConversationPermissionOption] = []
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
    let onSend: (String) -> Void

    /// Engine-owned slash-command catalog. An empty loaded catalog is
    /// authoritative; `slashCommandsLoaded == false` renders a loading row and
    /// never substitutes static commands.
    var slashCommands: [ConversationSlashCommand] = []
    var slashCommandsLoaded: Bool = false
    var slashCommandPending: Bool = false

    // PR-4 items 1 & 2: while a turn is in flight the send affordance becomes a
    // Stop button (interrupt the turn), and we never start an overlapping turn.
    var streaming: Bool = false
    var isCancelling: Bool = false
    /// Session transitions and cancellation can temporarily make a draft
    /// non-submittable even though the text field remains editable.
    var sendEnabled: Bool = true
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
    var voiceCapturePhase: VoiceCapturePhase = .idle
    var voiceInteractionMode: VoiceInteractionMode? = nil

    @State private var modelOpen = false
    @State private var controlsOpen = false
    @State private var holding = false
    @State private var cancellingHold = false
    @State private var suppressMicTap = false
    @State private var dismissedSlashDraft: String?

    init(
        model: Binding<ModelOption>,
        availableModels: [String] = [],
        activeModelId: String = "",
        onSelectModel: @escaping (String) -> Void = { _ in },
        reasoningSelection: String = "automatic",
        reasoningOptions: [String] = [],
        reasoningOptionDetails: [ConversationReasoningOption] = [],
        reasoningBudgetRange: ClosedRange<UInt64>? = nil,
        reasoningDisabledReason: String? = nil,
        permissionMode: String = "auto",
        effectivePermissionMode: String = "auto",
        permissionOptions: [ConversationPermissionOption] = [],
        controlsPending: Bool = false,
        controlsError: String? = nil,
        bypassWarningSuppressed: Bool = false,
        onSelectReasoning: @escaping (String) -> Void = { _ in },
        onSelectPermission: @escaping (String) -> Void = { _ in },
        onConfirmBypassPermissions: @escaping (Bool) -> Void = { _ in },
        recentModels: [String] = [],
        draft: Binding<String>,
        onSend: @escaping (String) -> Void,
        slashCommands: [ConversationSlashCommand] = [],
        slashCommandsLoaded: Bool = false,
        slashCommandPending: Bool = false,
        streaming: Bool = false,
        isCancelling: Bool = false,
        sendEnabled: Bool = true,
        onStop: @escaping () -> Void = {},
        onCameraClick: @escaping () -> Void = {},
        inputFocused: FocusState<Bool>.Binding,
        attachment: ComposerAttachment? = nil,
        onRemoveAttachment: @escaping () -> Void = {},
        onMicHoldStart: @escaping () -> Void = {},
        onMicHoldRelease: @escaping () -> Void = {},
        onMicHoldCancel: @escaping () -> Void = {},
        onMicTap: @escaping () -> Void = {},
        onFlowModeTap: @escaping () -> Void = {},
        voiceCapturePhase: VoiceCapturePhase = .idle,
        voiceInteractionMode: VoiceInteractionMode? = nil
    ) {
        self._model = model
        self.availableModels = availableModels
        self.activeModelId = activeModelId
        self.onSelectModel = onSelectModel
        self.reasoningSelection = reasoningSelection
        self.reasoningOptions = reasoningOptions
        self.reasoningOptionDetails = reasoningOptionDetails
        self.reasoningBudgetRange = reasoningBudgetRange
        self.reasoningDisabledReason = reasoningDisabledReason
        self.permissionMode = permissionMode
        self.effectivePermissionMode = effectivePermissionMode
        self.permissionOptions = permissionOptions
        self.controlsPending = controlsPending
        self.controlsError = controlsError
        self.bypassWarningSuppressed = bypassWarningSuppressed
        self.onSelectReasoning = onSelectReasoning
        self.onSelectPermission = onSelectPermission
        self.onConfirmBypassPermissions = onConfirmBypassPermissions
        self.recentModels = recentModels
        self._draft = draft
        self.onSend = onSend
        self.slashCommands = slashCommands
        self.slashCommandsLoaded = slashCommandsLoaded
        self.slashCommandPending = slashCommandPending
        self.streaming = streaming
        self.isCancelling = isCancelling
        self.sendEnabled = sendEnabled
        self.onStop = onStop
        self.onCameraClick = onCameraClick
        self._inputFocused = inputFocused
        self.attachment = attachment
        self.onRemoveAttachment = onRemoveAttachment
        self.onMicHoldStart = onMicHoldStart
        self.onMicHoldRelease = onMicHoldRelease
        self.onMicHoldCancel = onMicHoldCancel
        self.onMicTap = onMicTap
        self.onFlowModeTap = onFlowModeTap
        self.voiceCapturePhase = voiceCapturePhase
        self.voiceInteractionMode = voiceInteractionMode
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

                if let slashArgumentHint {
                    Text(slashArgumentHint)
                        .font(.caption.monospaced())
                        .foregroundStyle(t.text3)
                        .padding(.horizontal, 4)
                        .accessibilityLabel("slash_command_argument_hint")
                        .accessibilityValue(slashArgumentHint)
                }

                TextField("", text: $draft, prompt: Text("composer_placeholder").foregroundColor(t.text4), axis: .vertical)
                    .font(.system(size: 15.5))
                    .foregroundStyle(t.text)
                    .lineLimit(1...5)
                    .focused($inputFocused)
                    .submitLabel(.send)
                    .onSubmit {
                        send()
                    }
                    .accessibilityIdentifier("composer.input")
                    .padding(.horizontal, 4).padding(.vertical, 2)
                    .onChange(of: draft) { _, newValue in
                        if dismissedSlashDraft != newValue {
                            dismissedSlashDraft = nil
                        }
                    }

                HStack(spacing: 4) {
                    // Attach / camera: drives a real on-device capture through the
                    // same CameraImpl the engine bridges onto `traits::CameraControl`;
                    // the result surfaces as the attachment chip above.
                    Button(action: onCameraClick) {
                        LXIcon(name: .plus, size: 18, color: t.text3, stroke: 1.8)
                            .frame(width: 40, height: 40)
                    }
                    .buttonStyle(ComposerActionButtonStyle())
                    .accessibilityLabel("composer_add_attachment")
                    modelChip
                    controlsChip
                    Spacer()
                    if isCancelling || slashCommandPending {
                        ProgressView()
                            .tint(t.text3)
                            .frame(width: 40, height: 40)
                            .accessibilityLabel(isCancelling ? "composer_stopping" : "slash_command_running")
                    } else if streaming {
                        // PR-4 item 2: the Stop button replaces Send while a turn
                        // is in flight — tapping it cancels the in-flight turn.
                        Button(action: onStop) {
                            ComposerTurnActionIcon(
                                systemName: "stop.fill",
                                symbolSize: 11,
                                background: t.danger
                            )
                        }
                        .buttonStyle(ComposerActionButtonStyle())
                        .accessibilityLabel("composer_stop")
                        .accessibilityIdentifier("composer.stop")
                    } else if draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        // Match Android: ordinary recording and Flow Mode are
                        // distinct controls instead of overloading one tap.
                        HStack(spacing: 6) {
                            Button(action: handleMicTap) {
                                if voiceCapturePhase == .finishing {
                                    ProgressView()
                                        .tint(t.text2)
                                        .frame(width: 40, height: 40)
                                } else {
                                    LXIcon(
                                        name: cancellingHold || isDictationListening ? .stop : .mic,
                                        size: 18,
                                        color: cancellingHold ? t.danger
                                            : (holding || isDictationListening ? t.accent : t.text2),
                                        stroke: 1.8
                                    )
                                    .frame(width: 40, height: 40)
                                    .background(
                                        isDictationListening || holding
                                            ? t.accent.tint(0.15)
                                            : t.surfaceHover.opacity(0.72),
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

                            Button(action: onFlowModeTap) {
                                LXIcon(name: .audioWave, size: 18, color: .white, stroke: 1.8)
                                    .frame(width: 40, height: 40)
                                    .background(
                                        LinearGradient(
                                            colors: [t.accent, t.accent2],
                                            startPoint: .topLeading,
                                            endPoint: .bottomTrailing
                                        )
                                    )
                                    .clipShape(Circle())
                                    .shadow(color: t.accent.tint(0.28), radius: 7, y: 4)
                            }
                            .buttonStyle(ComposerActionButtonStyle())
                            .accessibilityLabel("composer_flow_mode")
                            .accessibilityIdentifier("composer.flow")
                            .disabled(!sendEnabled || voiceInteractionMode != nil)
                        }
                        .opacity(sendEnabled ? 1 : 0.45)
                    } else {
                        Button(action: send) {
                            ComposerTurnActionIcon(
                                systemName: "arrow.up",
                                symbolSize: 15,
                                background: t.accent
                            )
                        }
                        .buttonStyle(ComposerActionButtonStyle())
                        .accessibilityLabel("composer_send")
                        .accessibilityIdentifier("composer.send")
                        .disabled(!canSubmitDraft)
                        .opacity(canSubmitDraft ? 1 : 0.45)
                    }
                }
            }
            .padding(.horizontal, 12).padding(.top, 10).padding(.bottom, 8)
            .background(t.composerBg)
            .clipShape(.rect(cornerRadius: 22))
            .overlay {
                RoundedRectangle(cornerRadius: 22)
                    .stroke(inputFocused ? t.accent.tint(0.52) : t.borderStrong,
                            lineWidth: inputFocused ? 1 : 0.5)
            }
            .shadow(
                color: inputFocused ? t.accent.tint(0.12) : .black.opacity(0.06),
                radius: inputFocused ? 12 : 8,
                y: 4
            )
            .animation(.easeOut(duration: 0.18), value: inputFocused)
        }
        .padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 4)
        .animation(.easeOut(duration: 0.16), value: shouldShowSlashPanel)
        // A sheet, not a popover anchored to the chip: the picker's search field
        // raises the keyboard over exactly the space a popover above the
        // composer would occupy, and the system sizes a sheet against the
        // keyboard for us.
        .sheet(isPresented: $modelOpen) {
            ModelPickerSheet(
                availableModels: availableModels,
                activeModelId: activeModelId,
                recentModels: recentModels,
                onSelect: { reference in
                    onSelectModel(reference)
                    modelOpen = false
                },
                onDismiss: { modelOpen = false })
        }
        .sheet(isPresented: $controlsOpen) {
            ConversationControlsSheet(
                reasoningSelection: reasoningSelection,
                reasoningOptions: reasoningOptions,
                reasoningOptionDetails: reasoningOptionDetails,
                reasoningBudgetRange: reasoningBudgetRange,
                reasoningDisabledReason: reasoningDisabledReason,
                permissionMode: permissionMode,
                effectivePermissionMode: effectivePermissionMode,
                permissionOptions: permissionOptions,
                controlsPending: controlsPending,
                controlsError: controlsError,
                bypassWarningSuppressed: bypassWarningSuppressed,
                onSelectReasoning: { value in onSelectReasoning(value) },
                onSelectPermission: { value in onSelectPermission(value) },
                onConfirmBypassPermissions: onConfirmBypassPermissions,
                onDismiss: { controlsOpen = false }
            )
        }
        .toolbar {
            ToolbarItemGroup(placement: .keyboard) {
                Spacer()
                Button("composer_done") { inputFocused = false }
                    .accessibilityIdentifier("composer.keyboard.dismiss")
            }
        }
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

    private var isDictationListening: Bool {
        voiceInteractionMode == .dictation && voiceCapturePhase == .listening
    }

    /// The chip's short label from authoritative engine state.
    private var chipLabel: String {
        availableModels.isEmpty ? String(localized: "composer_loading_model") : ModelDisplay.shortName(for: activeModelId)
    }

    /// The chip's dot color: derived from the active engine id when driving.
    private var chipColor: Color {
        availableModels.isEmpty ? t.text4 : ModelDisplay.color(for: activeModelId)
    }

    private var modelChip: some View {
        Button { modelOpen = true } label: {
            HStack(spacing: 5) {
                Circle().fill(chipColor).frame(width: 6, height: 6)
                Text(chipLabel).font(.system(size: 12, weight: .medium))
                LXIcon(name: .chevron, size: 11, color: t.text4, stroke: 2)
            }
            .foregroundColor(t.text2)
            .padding(.horizontal, 9)
            .frame(minHeight: 40)
            .background(modelOpen ? t.surfaceHover : .clear)
            .clipShape(.rect(cornerRadius: 10))
        }
        .buttonStyle(ComposerActionButtonStyle())
        .disabled(availableModels.isEmpty)
        .accessibilityIdentifier("composer.model")
    }

    private var controlsChip: some View {
        Button { controlsOpen = true } label: {
            HStack(spacing: 4) {
                Text(reasoningSelection == "automatic" ? "Auto" : reasoningSelection.capitalized)
                Text("·")
                Text(effectivePermissionMode != permissionMode ? "\(permissionMode) → \(effectivePermissionMode)" : permissionMode)
            }
            .font(.system(size: 11.5, weight: .medium))
            .foregroundStyle(t.text2)
            .padding(.horizontal, 8)
            .frame(minHeight: 40)
            .background(controlsOpen ? t.surfaceHover : .clear)
            .clipShape(.rect(cornerRadius: 10))
        }
        .buttonStyle(ComposerActionButtonStyle())
        .disabled(controlsPending)
        .accessibilityLabel("composer.controls")
        .accessibilityIdentifier("composer.controls")
    }

    private var slashSuggestions: [SlashCommandSuggestion] {
        guard slashCommandsLoaded else { return [] }
        return SlashCommandMatcher.suggestions(for: draft, catalog: slashCommands)
    }

    private var slashArgumentHint: String? {
        guard slashCommandsLoaded else { return nil }
        return SlashCommandMatcher.argumentHint(for: draft, catalog: slashCommands)
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
        sendEnabled && !isWaitingForSlashCatalog
    }

    private func acceptSlashCommand(_ command: ConversationSlashCommand) {
        let accepted = command.canonicalTrigger
        draft = accepted
        dismissedSlashDraft = accepted
        inputFocused = true
    }

    private func send() {
        let trimmed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty,
              !streaming,
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
        onSend(draft); draft = ""
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
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.96 : 1)
            .opacity(configuration.isPressed ? 0.86 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
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
            .frame(width: 40, height: 40)
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
                Image(uiImage: attachment.image)
                    .resizable()
                    .scaledToFill()
                    .frame(width: 48, height: 48)
                    .clipShape(RoundedRectangle(cornerRadius: 10))
            #endif
            Text("\(attachment.width)×\(attachment.height)")
                .font(.system(size: 12))
                .foregroundColor(t.text3)
            Spacer()
            Button(action: onRemove) {
                LXIcon(name: .x, size: 14, color: t.text3, stroke: 2)
                    .frame(width: 40, height: 40)
            }
            .buttonStyle(ComposerActionButtonStyle())
            .accessibilityLabel("composer_remove_attachment")
        }
        .padding(.horizontal, 4).padding(.vertical, 2)
    }
}
