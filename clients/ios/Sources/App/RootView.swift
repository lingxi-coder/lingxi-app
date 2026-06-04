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

    /// The conversation source ChatView drives — the real in-process engine when
    /// available (P3a), otherwise the canned mock. Held once for the app session.
    @State private var source: any ConversationSource = ConversationSourceFactory.make()

    private var session: SessionRef { MockData.session(activeSession) }

    var body: some View {
        ZStack {
            t.windowBg.ignoresSafeArea()

            ChatView(session: session,
                     openDrawer: { withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) { drawerOpen = true } },
                     voiceActive: $voiceActive,
                     draft: $draft,
                     source: source)

            // Drawer overlay
            if drawerOpen {
                Drawer(activeWs: $activeWs,
                       activeSession: $activeSession,
                       onClose: { withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) { drawerOpen = false } },
                       openSettings: {
                           withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) { drawerOpen = false }
                           withAnimation(.spring(response: 0.34, dampingFraction: 0.86)) { settingsOpen = true }
                       })
                .zIndex(50)
            }

            // Settings sheet
            if settingsOpen {
                SettingsHost(store: settingsStore,
                             onClose: { withAnimation(.easeOut(duration: 0.28)) { settingsOpen = false } })
                .zIndex(60)
            }

            // Voice flow overlay — ChatView's held gesture drives dismissal on release.
            if voiceActive {
                VoiceFlowView(onRelease: { withAnimation(.easeOut(duration: 0.25)) { voiceActive = false } })
                    .zIndex(70)
                    .allowsHitTesting(false)
            }
        }
        // Mid-stream session switch (iOS analog of the Android `openSession` fix):
        // the Drawer writes `activeSession`; when it actually changes, tell the
        // source to switch sessions, which cancels any in-flight turn FIRST so a
        // turn completing after the switch can't land its deltas/notice in the
        // session we just opened. The new session's title also drives ChatView's
        // top bar via the recomputed `session`.
        .onChange(of: activeSession) { _, newId in
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
