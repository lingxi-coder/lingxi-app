import SwiftUI

// MARK: - ChatView — main conversation surface
struct ChatView: View {
    @EnvironmentObject private var app: AppState
    @Environment(\.theme) private var t

    let session: SessionRef
    let openDrawer: () -> Void
    /// Drives the voice "flow" overlay. ChatView owns the press lifecycle:
    /// hold 0.6s to set true; release sets false.
    @Binding var voiceActive: Bool

    /// The conversation source (mock or engine-over-UniFFI). ChatView renders its
    /// published `model` and forwards user input to it — it no longer owns the
    /// transcript or the canned reply timer.
    let source: any ConversationSource
    @ObservedObject private var convo: ConversationModel

    @State private var dotPulse = false

    // Composer draft, HOISTED here (the iOS analog of Android RootScreen's
    // `draft`) so a hold-to-talk transcription can route its recognized text
    // straight into the input the user is about to send.
    @State private var draft = ""
    // The captured-photo attachment, hoisted like the draft: the composer's
    // camera affordance drives an on-device capture and surfaces the JPEG here.
    @State private var attachment: ComposerAttachment? = nil
    // A transient affordance status line (permission denied / capture failed).
    @State private var captureStatus: String? = nil

    private let voiceCapture = VoiceCapture()
    private let cameraCapture = CameraCapture()

    init(session: SessionRef,
         openDrawer: @escaping () -> Void,
         voiceActive: Binding<Bool>,
         source: any ConversationSource) {
        self.session = session
        self.openDrawer = openDrawer
        self._voiceActive = voiceActive
        self.source = source
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
                topBar
                WorkflowBar()
                messageList
                Composer(model: $convo.model,
                         draft: $draft,
                         onSend: send,
                         onCameraClick: captureFromCamera,
                         attachment: attachment,
                         onRemoveAttachment: { attachment = nil },
                         onMicHoldStart: startVoiceHold,
                         onMicHoldRelease: endVoiceHold)
            }
        }
        // Voice flow: hold anywhere 0.6s to enter immersive recording; release sends.
        // A LongPress sequenced into a Drag keeps the same touch, so the
        // .updating/onEnded of the drag fire only after the 0.6s press succeeds
        // and detect the eventual finger lift.
        .simultaneousGesture(
            LongPressGesture(minimumDuration: 0.6)
                .sequenced(before: DragGesture(minimumDistance: 0))
                .onChanged { value in
                    if case .second(true, _) = value {
                        if !voiceActive { withAnimation(.easeOut(duration: 0.25)) { voiceActive = true } }
                    }
                }
                .onEnded { _ in
                    if voiceActive { withAnimation(.easeOut(duration: 0.25)) { voiceActive = false } }
                }
        )
        .onAppear { withAnimation(.easeInOut(duration: 1.2).repeatForever()) { dotPulse = true } }
    }

    // MARK: top bar
    private var topBar: some View {
        HStack(spacing: 4) {
            iconButton(.menu, color: t.text, action: openDrawer)
            Spacer()
            Text(convo.isNew ? "新对话" : session.title)
                .font(.system(size: 14.5, weight: .semibold))
                .foregroundColor(t.text)
                .lineLimit(1)
            Spacer()
            iconButton(app.isDark ? .sun : .moon, size: 18, color: t.text2) { app.toggleTheme() }
            iconButton(.edit, size: 18, color: t.accent) { newChat() }
        }
        .padding(.horizontal, 12).padding(.top, 6).padding(.bottom, 10)
    }

    private func iconButton(_ name: LXIconName, size: CGFloat = 20, color: Color, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            LXIcon(name: name, size: size, color: color, stroke: 1.8)
                .frame(width: 38, height: 38)
        }
    }

    // MARK: message list
    private var messageList: some View {
        ScrollViewReader { proxy in
            ScrollView(showsIndicators: false) {
                VStack(spacing: 0) {
                    if convo.isNew && convo.messages.isEmpty && !convo.streaming { emptyState }
                    ForEach(Array(convo.messages.enumerated()), id: \.element.id) { _, m in
                        MessageBubble(message: m, onShare: shareMessage)
                    }
                    if convo.streaming { streamingRow }
                    if let status = convo.statusLine { statusRow(status) }
                    if let cap = captureStatus { statusRow(cap) }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding(.horizontal, 16).padding(.top, 18).padding(.bottom, 8)
            }
            .onChange(of: convo.messages.count) { _, _ in withAnimation { proxy.scrollTo("bottom", anchor: .bottom) } }
            .onChange(of: convo.streaming) { _, _ in withAnimation { proxy.scrollTo("bottom", anchor: .bottom) } }
        }
    }

    private var emptyState: some View {
        VStack(spacing: 0) {
            RoundedRectangle(cornerRadius: 15)
                .fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                .frame(width: 52, height: 52)
                .overlay(LXIcon(name: .sparkle, size: 26, color: .white, stroke: 1.8))
                .padding(.bottom, 18)
            Text("开启新对话").font(.system(size: 21, weight: .semibold)).foregroundColor(t.text)
                .padding(.bottom, 7)
            Text("随便说点什么，或按住屏幕进入语音心流模式。")
                .font(.system(size: 14)).foregroundColor(t.text4)
                .multilineTextAlignment(.center).lineSpacing(14 * 0.5)
                .frame(maxWidth: 260)
        }
        .frame(maxWidth: .infinity, minHeight: 420)
        .padding(.horizontal, 24)
    }

    private var streamingRow: some View {
        HStack(spacing: 11) {
            AssistantAvatar()
            HStack(spacing: 4) {
                ForEach(0..<3) { i in
                    Circle().fill(t.accent).frame(width: 5, height: 5)
                        .opacity(dotPulse ? 0.8 : 0.35)
                        .animation(.easeInOut(duration: 1.2).repeatForever().delay(Double(i) * 0.15), value: dotPulse)
                }
            }
            Spacer()
        }
        .padding(.bottom, 26)
    }

    // A dim, single-line status row (engine errors / tool activity).
    private func statusRow(_ status: String) -> some View {
        HStack {
            Text(status)
                .font(.system(size: 12.5))
                .foregroundColor(t.text4)
            Spacer()
        }
        .padding(.bottom, 18)
    }

    // MARK: actions
    private func newChat() { source.startNewConversation() }

    private func send(_ txt: String) { source.send(txt) }

    // MARK: capability affordances (mirror Android RootScreen)

    /// Mic press past the 0.6s threshold: enter the immersive voice flow (the
    /// same overlay the hold-anywhere gesture shows).
    private func startVoiceHold() {
        captureStatus = nil
        withAnimation(.easeOut(duration: 0.25)) { voiceActive = true }
    }

    /// Finger lift after a successful mic hold: dismiss the overlay and run a
    /// one-shot STT capture, filling the composer draft with the transcript
    /// (mirrors Android `onTranscript` → draft; gated on speech/mic auth inside
    /// `SttImpl`).
    private func endVoiceHold() {
        withAnimation(.easeOut(duration: 0.25)) { voiceActive = false }
        Task {
            switch await voiceCapture.transcribe() {
            case let .transcript(text):
                draft = draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    ? text : "\(draft) \(text)"
            case .permissionDenied:
                captureStatus = "需要麦克风与语音识别权限"
            case .empty:
                break
            case let .failed(message):
                captureStatus = "语音识别失败：\(message)"
            }
        }
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
                captureStatus = "需要相机或相册权限"
            case let .failed(message):
                captureStatus = "拍照失败：\(message)"
            }
        }
    }

    /// Share an assistant reply's text via the native share sheet.
    private func shareMessage(_ text: String) { ShareCapture.share(text: text) }
}
