import SwiftUI

// MARK: - Composer (pill text field + model chip + attach + send/mic)
struct Composer: View {
    @Environment(\.theme) private var t
    @Binding var model: ModelOption
    // SHIP-BLOCKER #2: the picker is driven by the ENGINE's real model catalog when
    // available. `availableModels` are real engine ids (empty ⇒ engine unavailable,
    // fall back to the mock catalog); `activeModelId` is whatever the engine reports;
    // `onSelectModel` submits `SetModel(id)` with a real id. The `model` binding is
    // kept only as the friendly chip label (and the mock-fallback selection).
    var availableModels: [String] = []
    var activeModelId: String = ""
    var onSelectModel: (String) -> Void = { _ in }
    /// Draft text, HOISTED so the device capabilities can inject into it:
    /// hold-to-talk STT fills it with the recognized utterance (mirrors Android
    /// `onTranscript` → draft). The caller owns it (see ChatView).
    @Binding var draft: String
    let onSend: (String) -> Void

    // PR-4 items 1 & 2: while a turn is in flight the send affordance becomes a
    // Stop button (interrupt the turn), and we never start an overlapping turn.
    var streaming: Bool = false
    var onStop: () -> Void = {}

    // Camera affordance (the + / attach button) — mirrors Android `onCameraClick`
    // + the captured-photo `attachment` chip surfaced for review before sending.
    var onCameraClick: () -> Void = {}
    var attachment: ComposerAttachment? = nil
    var onRemoveAttachment: () -> Void = {}

    // Hold-to-talk on the mic affordance — mirrors Android `onMicHoldStart` /
    // `onMicHoldRelease`. A press past the threshold enters the immersive voice
    // flow; the finger lift drives the STT transcription that fills the draft.
    var onMicHoldStart: () -> Void = {}
    var onMicHoldRelease: () -> Void = {}

    // A single TAP on the mic enters FlowMode (心流 voice orb) — mirrors the
    // prototype's `mic → openOrb`. The press-and-hold STT path is kept: a quick
    // tap fires this, a 0.6s hold fires the gesture instead.
    var onMicTap: () -> Void = {}

    @State private var modelOpen = false
    @State private var holding = false

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                // Captured-photo attachment chip (the device-vision analog of how
                // a transcript lands in the draft): a thumbnail + a remove button,
                // shown only once a camera capture has surfaced an image.
                if let attachment {
                    AttachmentThumb(attachment: attachment, onRemove: onRemoveAttachment)
                }

                TextField("", text: $draft, prompt: Text("向灵犀提问…").foregroundColor(t.text4), axis: .vertical)
                    .font(.system(size: 15.5))
                    .foregroundColor(t.text)
                    .lineLimit(1...5)
                    .padding(.horizontal, 4).padding(.vertical, 2)

                HStack(spacing: 2) {
                    // Attach / camera: drives a real on-device capture through the
                    // same CameraImpl the engine bridges onto `traits::CameraControl`;
                    // the result surfaces as the attachment chip above.
                    Button(action: onCameraClick) {
                        LXIcon(name: .plus, size: 18, color: t.text3, stroke: 1.8)
                            .frame(width: 34, height: 34)
                    }
                    .accessibilityLabel("添加附件")
                    modelChip
                    Spacer()
                    if streaming {
                        // PR-4 item 2: the Stop button replaces Send while a turn
                        // is in flight — tapping it cancels the in-flight turn.
                        Button(action: onStop) {
                            LXIcon(name: .stop, size: 14, color: .white)
                                .frame(width: 34, height: 34)
                                .background(t.danger)
                                .clipShape(RoundedRectangle(cornerRadius: 10))
                                .shadow(color: t.danger.tint(0.40), radius: 6, y: 4)
                        }
                        .accessibilityLabel("停止")
                    } else if draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        // Mic: TAP enters the FlowMode orb; press-and-hold runs STT.
                        Button(action: onMicTap) {
                            LXIcon(name: .mic, size: 18, color: holding ? t.accent : t.text2, stroke: 1.8)
                                .frame(width: 34, height: 34)
                        }
                        .simultaneousGesture(micHoldGesture)
                        .accessibilityLabel("语音心流")
                        .accessibilityHint("轻点进入语音心流；按住转写为文本")
                    } else {
                        Button(action: send) {
                            LXIcon(name: .arrowUp, size: 16, color: .white)
                                .frame(width: 34, height: 34)
                                .background(t.accent)
                                .clipShape(RoundedRectangle(cornerRadius: 10))
                                .shadow(color: t.accent.tint(0.40), radius: 6, y: 4)
                        }
                        .accessibilityLabel("发送")
                    }
                }
            }
            .padding(.horizontal, 12).padding(.top, 10).padding(.bottom, 8)
            .background(t.composerBg)
            .clipShape(RoundedRectangle(cornerRadius: 22))
            .overlay(RoundedRectangle(cornerRadius: 22).stroke(t.borderStrong, lineWidth: 0.5))
            .shadow(color: .black.opacity(0.06), radius: 8, y: 4)
        }
        .padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 4)
        .overlay(alignment: .bottomLeading) {
            if modelOpen { modelMenu.padding(.leading, 50).padding(.bottom, 50) }
        }
    }

    // Press-and-hold → release, mirroring Android `voiceHold` (and the iOS
    // hold-anywhere idiom): a 0.6s LongPress sequenced into a Drag so the same
    // touch that crosses the threshold (onMicHoldStart) is the one whose lift we
    // detect (onMicHoldRelease → STT).
    private var micHoldGesture: some Gesture {
        LongPressGesture(minimumDuration: 0.6)
            .sequenced(before: DragGesture(minimumDistance: 0))
            .onChanged { value in
                if case .second(true, _) = value, !holding {
                    holding = true
                    onMicHoldStart()
                }
            }
            .onEnded { _ in
                if holding {
                    holding = false
                    onMicHoldRelease()
                }
            }
    }

    /// One picker row, abstracting over a real engine id and a mock catalog entry
    /// so the menu renders identically in both modes (SHIP-BLOCKER #2).
    private struct ModelRow: Identifiable {
        let id: String        // the real engine id (or mock id) submitted on pick
        let name: String      // display label (friendly when known, else the id)
        let desc: String?     // optional subtitle (mock only)
        let color: Color      // dot color
    }

    /// The rows to render: the engine's real catalog when present, else the mock
    /// catalog (engine unavailable / not yet listed). `ModelDisplay` maps a raw
    /// engine id to a friendly label + a stable color (friendly display optional).
    private var modelRows: [ModelRow] {
        if !availableModels.isEmpty {
            return availableModels.map { id in
                ModelRow(id: id,
                         name: ModelDisplay.name(for: id),
                         desc: nil,
                         color: ModelDisplay.color(for: id))
            }
        }
        return MockData.models.map {
            ModelRow(id: $0.id, name: $0.name, desc: $0.desc, color: $0.color)
        }
    }

    /// The id considered "active" for the chip + highlight: the engine's reported
    /// id when driving, else the mock chip's id.
    private var selectedModelId: String {
        availableModels.isEmpty ? model.id : activeModelId
    }

    /// The chip's short label: the friendly name of the active engine id when
    /// driving, else the mock chip's short name.
    private var chipLabel: String {
        availableModels.isEmpty ? model.shortName : ModelDisplay.shortName(for: activeModelId)
    }

    /// The chip's dot color: derived from the active engine id when driving.
    private var chipColor: Color {
        availableModels.isEmpty ? model.color : ModelDisplay.color(for: activeModelId)
    }

    private var modelChip: some View {
        Button { withAnimation(.easeOut(duration: 0.15)) { modelOpen.toggle() } } label: {
            HStack(spacing: 5) {
                Circle().fill(chipColor).frame(width: 6, height: 6)
                Text(chipLabel).font(.system(size: 12, weight: .medium))
                LXIcon(name: .chevron, size: 11, color: t.text4, stroke: 2)
            }
            .foregroundColor(t.text2)
            .padding(.horizontal, 9).padding(.vertical, 5)
            .background(modelOpen ? t.surfaceHover : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 8))
        }
    }

    private var modelMenu: some View {
        VStack(spacing: 0) {
            ForEach(modelRows) { m in
                Button {
                    onSelectModel(m.id)
                    withAnimation(.easeOut(duration: 0.15)) { modelOpen = false }
                } label: {
                    HStack(spacing: 10) {
                        Circle().fill(m.color).frame(width: 8, height: 8)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(m.name).font(.system(size: 13, weight: .medium)).foregroundColor(t.text)
                            if let desc = m.desc {
                                Text(desc).font(.system(size: 11)).foregroundColor(t.text3)
                            }
                        }
                        Spacer()
                    }
                    .padding(.horizontal, 10).padding(.vertical, 8)
                    .background(m.id == selectedModelId ? t.accent.tint(0.15) : .clear)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                }
            }
        }
        .padding(5)
        .frame(width: 200)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.borderStrong, lineWidth: 0.5))
        .shadow(color: .black.opacity(0.3), radius: 20, y: 16)
        .transition(.opacity.combined(with: .scale(scale: 0.95, anchor: .bottomLeading)))
    }

    private func send() {
        let trimmed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        onSend(draft); draft = ""
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
                    .frame(width: 28, height: 28)
            }
            .accessibilityLabel("移除附件")
        }
        .padding(.horizontal, 4).padding(.vertical, 2)
    }
}
