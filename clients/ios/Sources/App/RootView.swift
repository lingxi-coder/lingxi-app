import SwiftUI

// MARK: - RootView — composes the whole app (chat + drawer + settings + voice)
struct RootView: View {
    @EnvironmentObject private var app: AppState
    @Environment(\.theme) private var t

    @StateObject private var settingsStore = SettingsStore()
    @State private var activeWs = "work"
    @State private var activeSession = "s1"
    @State private var drawerOpen = false
    @State private var settingsOpen = false
    @State private var voiceActive = false

    private var session: SessionRef { MockData.session(activeSession) }

    var body: some View {
        ZStack {
            t.windowBg.ignoresSafeArea()

            ChatView(session: session,
                     openDrawer: { withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) { drawerOpen = true } },
                     voiceActive: $voiceActive)

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
    }
}
