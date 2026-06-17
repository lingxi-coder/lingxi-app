import SwiftUI

// MARK: - RootView — composes the whole app (chat + drawer + settings + voice)
struct RootView: View {
    @EnvironmentObject private var app: AppState
    @Environment(\.theme) private var t
    // Observe the app lifecycle so a turn left mid-stream when the user
    // backgrounds the app isn't stranded "streaming" forever, and so the active
    // session / draft are restored on return.
    @Environment(\.scenePhase) private var scenePhase

    @StateObject private var settingsStore = SettingsStore()
    @State private var activeWs = "work"
    // The active session id, DERIVED from real data (the first available
    // session) rather than the literal "s1", and PERSISTED via @AppStorage so a
    // relaunch / restore lands on the session the user last had open. @AppStorage
    // writes through immediately, so backgrounding inherently persists it.
    @AppStorage("activeSession") private var activeSession = MockData.defaultSessionId
    // The composer draft, hoisted to the root (the iOS analog of Android
    // RootScreen's `draft`) and PERSISTED so an in-progress, unsent message
    // survives the app being backgrounded and is restored on return.
    @AppStorage("composerDraft") private var draft = ""
    @State private var drawerOpen = false
    @State private var settingsOpen = false
    @State private var voiceActive = false
    /// FlowMode (心流) voice-orb overlay — opened by a TAP on the composer mic
    /// (the press-and-hold STT path still drives `voiceActive`).
    @State private var flowActive = false
    /// When the drawer itself drove the session change (engine resume / new
    /// chat), it has ALREADY called `source.resumeSession` / `startNewConversation`.
    /// This one-shot flag tells the `activeSession` `onChange` below to skip the
    /// mock `openSession` so we don't double-handle the switch. Reset after each
    /// observed change.
    @State private var suppressOpenSession = false

    /// The conversation source ChatView drives — the real in-process engine when
    /// available (P3a), otherwise the canned mock. Held once for the app session.
    @State private var source: any ConversationSource = ConversationSourceFactory.make()

    /// The session for ChatView's title bar. Prefers a REAL engine session whose
    /// UUID matches `activeSession`; falls back to the mock catalog otherwise (so
    /// the bar still resolves a title when the engine is unavailable).
    private var session: SessionRef {
        if let s = source.model.engineSessions.first(where: { $0.id == activeSession }) {
            return s.ref
        }
        return MockData.session(activeSession)
    }

    var body: some View {
        ZStack {
            t.windowBg.ignoresSafeArea()

            ChatView(session: session,
                     openDrawer: { withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) { drawerOpen = true } },
                     voiceActive: $voiceActive,
                     onEnterFlow: { withAnimation(.easeOut(duration: 0.4)) { flowActive = true } },
                     draft: $draft,
                     source: source)

            // Drawer overlay
            if drawerOpen {
                Drawer(activeWs: $activeWs,
                       activeSession: $activeSession,
                       source: source,
                       onClose: { withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) { drawerOpen = false } },
                       openSettings: {
                           withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) { drawerOpen = false }
                           withAnimation(.spring(response: 0.34, dampingFraction: 0.86)) { settingsOpen = true }
                       },
                       // The drawer already drives `source.resumeSession`; mirror
                       // the choice into `activeSession` so the title bar +
                       // persisted @AppStorage reflect the resumed session. Guard
                       // the `onChange` below from re-issuing a redundant
                       // `openSession` for this engine-driven select.
                       onSelectEngineSession: { uuid in
                           suppressOpenSession = true
                           activeSession = uuid
                       },
                       // The drawer already drove `startNewConversation`; just
                       // reflect the reset locally without re-issuing openSession.
                       onNewChat: {
                           suppressOpenSession = true
                           activeSession = ""
                       })
                .zIndex(50)
            }

            // Settings sheet
            if settingsOpen {
                SettingsHost(store: settingsStore,
                             convo: source.model,
                             onRefreshMcp: { source.refreshMcpServers() },
                             onClose: { withAnimation(.easeOut(duration: 0.28)) { settingsOpen = false } })
                .zIndex(60)
            }

            // Voice flow overlay — ChatView's held gesture drives dismissal on release.
            if voiceActive {
                VoiceFlowView(onRelease: { withAnimation(.easeOut(duration: 0.25)) { voiceActive = false } })
                    .zIndex(70)
                    .allowsHitTesting(false)
            }

            // FlowMode (心流) voice-orb overlay — a tap on the composer mic opens
            // this full-screen living-orb experience. Interactive (tap to re-listen,
            // pop-up text input); dismissed by its own close button.
            if flowActive {
                VoiceOrbView(convo: source.model,
                             onSend: { source.send($0) },
                             onCancel: { source.cancel() },
                             onClose: { withAnimation(.easeOut(duration: 0.3)) { flowActive = false } })
                    .zIndex(72)
                    .transition(.opacity)
            }

            // First-run setup wizard — shown until onboarding is complete; the
            // Settings → 关于 → 重新观看引导 row clears `setupDone` to replay it.
            if !app.setupDone {
                SetupWizardView(convo: source.model, onSetModel: { source.setModel($0) })
                    .zIndex(100)
                    .transition(.opacity)
            }
        }
        // Mid-stream session switch (iOS analog of the Android `openSession` fix):
        // the Drawer writes `activeSession`; when it actually changes, tell the
        // source to switch sessions, which cancels any in-flight turn FIRST so a
        // turn completing after the switch can't land its deltas/notice in the
        // session we just opened. The new session's title also drives ChatView's
        // top bar via the recomputed `session`.
        .onChange(of: activeSession) { _, newId in
            // The drawer's engine-session select / new-chat already drove the
            // source (`resumeSession` / `startNewConversation`); skip the mock
            // `openSession` for that one change so we don't double-switch.
            if suppressOpenSession {
                suppressOpenSession = false
                return
            }
            source.openSession(MockData.session(newId))
        }
        // Lifecycle: clear a stuck streaming flag on background (so a turn parked
        // mid-stream isn't left "streaming" forever) and restore on foreground.
        // The active session + draft are persisted via @AppStorage, so they are
        // already durable across the transition; this only manages turn state.
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .background:
                source.handleBackground()
            case .active:
                source.handleForeground()
            default:
                break
            }
        }
    }
}
