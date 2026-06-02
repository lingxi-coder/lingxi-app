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
                Composer(model: $convo.model, onSend: send)
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
                        MessageBubble(message: m)
                    }
                    if convo.streaming { streamingRow }
                    if let status = convo.statusLine { statusRow(status) }
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
}
