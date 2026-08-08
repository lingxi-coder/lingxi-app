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
    /// Previously-picked refs, most-recent-first, pinned above the provider
    /// sections in the picker. Owned by the caller (see ChatView) because it
    /// outlives any one composer instance.
    var recentModels: [String] = []
    /// Draft text, HOISTED so the device capabilities can inject into it:
    /// hold-to-talk STT fills it with the recognized utterance (mirrors Android
    /// `onTranscript` → draft). The caller owns it (see ChatView).
    @Binding var draft: String
    let onSend: (String) -> Void

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
    @State private var holding = false
    @State private var cancellingHold = false
    @State private var suppressMicTap = false

    init(
        model: Binding<ModelOption>,
        availableModels: [String] = [],
        activeModelId: String = "",
        onSelectModel: @escaping (String) -> Void = { _ in },
        recentModels: [String] = [],
        draft: Binding<String>,
        onSend: @escaping (String) -> Void,
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
        self.recentModels = recentModels
        self._draft = draft
        self.onSend = onSend
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
            VStack(alignment: .leading, spacing: 4) {
                // Captured-photo attachment chip (the device-vision analog of how
                // a transcript lands in the draft): a thumbnail + a remove button,
                // shown only once a camera capture has surfaced an image.
                if let attachment {
                    AttachmentThumb(attachment: attachment, onRemove: onRemoveAttachment)
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
                    Spacer()
                    if isCancelling {
                        ProgressView()
                            .tint(t.text3)
                            .frame(width: 40, height: 40)
                            .accessibilityLabel("composer_stopping")
                    } else if streaming {
                        // PR-4 item 2: the Stop button replaces Send while a turn
                        // is in flight — tapping it cancels the in-flight turn.
                        Button(action: onStop) {
                            LXIcon(name: .stop, size: 14, color: .white)
                                .frame(width: 40, height: 40)
                                .background(t.danger)
                                .clipShape(.rect(cornerRadius: 12))
                                .shadow(color: t.danger.tint(0.40), radius: 6, y: 4)
                        }
                        .buttonStyle(ComposerActionButtonStyle())
                        .accessibilityLabel("composer_stop")
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
                            LXIcon(name: .arrowUp, size: 16, color: .white)
                                .frame(width: 40, height: 40)
                                .background(t.accent)
                                .clipShape(.rect(cornerRadius: 12))
                                .shadow(color: t.accent.tint(0.40), radius: 6, y: 4)
                        }
                        .buttonStyle(ComposerActionButtonStyle())
                        .accessibilityLabel("composer_send")
                        .disabled(!sendEnabled)
                        .opacity(sendEnabled ? 1 : 0.45)
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


    private func send() {
        let trimmed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !streaming, !isCancelling, sendEnabled else { return }
        onSend(draft); draft = ""
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

