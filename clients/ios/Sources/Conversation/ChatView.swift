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

    @State private var messages = MockData.messagesDefault
    @State private var isNew = false
    @State private var streaming = false
    @State private var model = MockData.models[0]
    @State private var dotPulse = false

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
                Composer(model: $model, onSend: send)
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
        .onChange(of: session.id) { _, _ in
            isNew = false; streaming = false; messages = MockData.messagesDefault
        }
        .onAppear { withAnimation(.easeInOut(duration: 1.2).repeatForever()) { dotPulse = true } }
    }

    // MARK: top bar
    private var topBar: some View {
        HStack(spacing: 4) {
            iconButton(.menu, color: t.text, action: openDrawer)
            Spacer()
            Text(isNew ? "新对话" : session.title)
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
                    if isNew && messages.isEmpty && !streaming { emptyState }
                    ForEach(Array(messages.enumerated()), id: \.element.id) { _, m in
                        MessageBubble(message: m)
                    }
                    if streaming { streamingRow }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding(.horizontal, 16).padding(.top, 18).padding(.bottom, 8)
            }
            .onChange(of: messages.count) { _, _ in withAnimation { proxy.scrollTo("bottom", anchor: .bottom) } }
            .onChange(of: streaming) { _, _ in withAnimation { proxy.scrollTo("bottom", anchor: .bottom) } }
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

    // MARK: actions
    private func newChat() { messages = []; streaming = false; isNew = true }

    private func send(_ txt: String) {
        isNew = false
        messages.append(Message(role: .user, text: txt))
        streaming = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.1) {
            messages.append(Message(role: .ai, tag: "思考了 8 秒", text: "已记入。继续追问。"))
            streaming = false
        }
    }
}
