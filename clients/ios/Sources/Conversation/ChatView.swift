import SwiftUI

// MARK: - ChatView — main conversation surface
struct ChatView: View {
    @Environment(AppState.self) private var app
    @Environment(\.theme) private var t

    let session: SessionRef
    let openDrawer: () -> Void

    /// The conversation source (mock or engine-over-UniFFI). ChatView renders its
    /// published `model` and forwards user input to it — it no longer owns the
    /// transcript or the canned reply timer.
    let source: any ConversationSource
    var onOpenShellTask: ((ConversationShellLaunchRequest) -> Void)? = nil
    @ObservedObject private var convo: ConversationModel

    @State private var dotPulse = false
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
         openDrawer: @escaping () -> Void,
         draft: Binding<String>,
         voiceInteraction: VoiceInteractionController,
         source: any ConversationSource,
         onOpenVoiceSettings: @escaping () -> Void = {},
         onOpenShellTask: ((ConversationShellLaunchRequest) -> Void)? = nil) {
        self.session = session
        self.openDrawer = openDrawer
        self._draft = draft
        self.voiceInteraction = voiceInteraction
        self.source = source
        self.onOpenVoiceSettings = onOpenVoiceSettings
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
                topBar
                // Offline banner: shown only while the device is offline. Sits
                // above the transcript so it's visible without obscuring the
                // conversation; the retry re-warms the engine source (rebuilds
                // the handle / re-lists models).
                if connectivity.isOffline {
                    OfflineBanner(onRetry: retryConnection)
                        .padding(.horizontal, 14).padding(.bottom, 8)
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
                messageList
                if voiceInteraction.isPresented {
                    InlineVoicePanel(
                        controller: voiceInteraction,
                        onConfigure: onOpenVoiceSettings
                    )
                }
                Composer(model: $convo.model,
                         // SHIP-BLOCKER #2: drive the picker off the engine's real
                         // model catalog + active id (out-of-band model state). On
                         // pick, the source submits `SetModel(id)` with a real id.
                         availableModels: convo.availableModels,
                         activeModelId: convo.activeModelId,
                         onSelectModel: { source.setModel($0) },
                         draft: $draft,
                         onSend: send,
                         streaming: convo.streaming,
                         isCancelling: convo.isCancelling,
                         sendEnabled: !convo.sessionTransitionPending && voiceInteraction.mode != .flow,
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

            // SHIP-BLOCKER #3: the engine-parked permission prompt. Sits above the
            // conversation so a tool that needs approval is answered (allow / deny)
            // instead of hanging the turn forever. No-op surface on the mock source
            // (which never parks a turn on a permission gate).
            #if canImport(engine_mobileFFI)
                PermissionPrompt(
                    pending: convo.pendingPermissions.first,
                    onApprove: { source.approvePermission($0, $1) },
                    onDeny: { source.denyPermission($0) }
                )
                    .animation(.easeOut(duration: 0.2), value: convo.pendingPermissions.first)
            #endif
        }
        .buttonStyle(.plain)
        .animation(.easeOut(duration: 0.25), value: connectivity.isOffline)
        .animation(.spring(response: 0.3, dampingFraction: 0.86), value: voiceInteraction.isPresented)
        .onAppear {
            followsLatestMessage = true
            withAnimation(.easeInOut(duration: 1.2).repeatForever()) { dotPulse = true }
            // SHIP-BLOCKER #2: build the engine eagerly so its real model catalog
            // (`ModelList`) populates the picker before the first send. No-op on the mock.
            source.warmUp()
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
    }

    // MARK: top bar
    private var topBar: some View {
        HStack(spacing: 4) {
            iconButton(.menu, color: t.text, label: String(localized: "chat_open_drawer"), action: openDrawer)
            Spacer()
            Text(convo.isNew ? String(localized: "chat_new_conversation") : session.title)
                .font(.scaledSystem(14.5, weight: .semibold, relativeTo: .subheadline))
                .foregroundColor(t.text)
                .lineLimit(1)
            Spacer()
            iconButton(app.isDark ? .sun : .moon, size: 18, color: t.text2,
                       label: app.isDark ? String(localized: "chat_theme_light") : String(localized: "chat_theme_dark")) { app.toggleTheme() }
            iconButton(.edit, size: 18, color: t.accent, label: String(localized: "chat_new_chat")) { newChat() }
        }
        .padding(.horizontal, 12).padding(.top, 6).padding(.bottom, 10)
    }

    private func iconButton(_ name: LXIconName, size: CGFloat = 20, color: Color,
                            label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            LXIcon(name: name, size: size, color: color, stroke: 1.8)
                .frame(width: 38, height: 38)
        }
        .accessibilityLabel(label)
    }

    // MARK: message list
    private var messageList: some View {
        ScrollViewReader { proxy in
            ScrollView(showsIndicators: false) {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if convo.isNew && convo.messages.isEmpty && !convo.streaming { emptyState }
                    ForEach(convo.items) { item in
                        switch item {
                        case let .message(message):
                            MessageBubble(
                                message: message,
                                detail: convo.messageDetails[message.id],
                                onShare: shareMessage
                            )
                            .equatable()
                        case let .run(run):
                            ConversationExecutionRunCard(run: run, onOpenShellTask: onOpenShellTask)
                        }
                    }
                    if convo.streaming { streamingRow }
                    // PR-4 item 3: a non-clean turn outcome (MaxTurns / Cancelled),
                    // surfaced distinctly from a normal end.
                    if let notice = convo.notice { noticeRow(notice) }
                    if let status = convo.statusLine { statusRow(status) }
                    if let cap = captureStatus { statusRow(cap) }
                    // PR-4 item 4: a persistent, dismissible, kind-aware error
                    // banner (not the old transient dim line).
                    if let err = convo.error { ErrorBanner(error: err, onDismiss: dismissError) }
                    Color.clear
                        .frame(height: 1)
                        .id("bottom")
                        .onAppear {
                            if !followsLatestMessage { followsLatestMessage = true }
                        }
                        .onDisappear {
                            if followsLatestMessage { followsLatestMessage = false }
                        }
                }
                .frame(maxWidth: 720)
                .frame(maxWidth: .infinity)
                .padding(.horizontal, 16).padding(.top, 18).padding(.bottom, 8)
            }
            .scrollDismissesKeyboard(.interactively)
            .accessibilityIdentifier("conversation.message-list")
            .onChange(of: convo.items.count) { _, _ in scrollToLatest(using: proxy) }
            .onChange(of: convo.streaming) { _, _ in scrollToLatest(using: proxy) }
            .onChange(of: convo.error) { _, _ in scrollToLatest(using: proxy) }
            .onChange(of: convo.notice) { _, _ in scrollToLatest(using: proxy) }
            .onChange(of: composerFocused) { _, focused in
                guard focused else { return }
                withAnimation(.easeOut(duration: 0.2)) {
                    proxy.scrollTo("bottom", anchor: .bottom)
                }
            }
        }
    }

    private func scrollToLatest(using proxy: ScrollViewProxy) {
        guard followsLatestMessage else { return }
        withAnimation(.easeOut(duration: 0.2)) {
            proxy.scrollTo("bottom", anchor: .bottom)
        }
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

    // A dim, single-line status row (tool activity / capture affordances).
    private func statusRow(_ status: String) -> some View {
        HStack {
            Text(status)
                .font(.system(size: 12.5))
                .foregroundColor(t.text4)
            Spacer()
        }
        .padding(.bottom, 18)
    }

    // A turn-outcome notice (MaxTurns / Cancelled) — a centered, dim chip that
    // distinguishes a non-clean end from a normal one (PR-4 item 3).
    private func noticeRow(_ notice: TurnNotice) -> some View {
        HStack {
            Spacer()
            Text(notice.text)
                .font(.system(size: 12))
                .foregroundColor(t.text3)
                .padding(.horizontal, 12).padding(.vertical, 6)
                .background(t.surface)
                .clipShape(Capsule())
                .overlay(Capsule().stroke(t.border, lineWidth: 0.5))
            Spacer()
        }
        .padding(.bottom, 18)
    }

    // MARK: actions
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

    // PR-4 item 4: dismiss the persistent error banner.
    private func dismissError() { source.dismissError() }

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
