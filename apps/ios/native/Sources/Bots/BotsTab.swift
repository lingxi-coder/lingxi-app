import SwiftUI

/// Computers, their live or last-known Bots, and remote conversations. The manager and its
/// uid-scoped snapshots belong to the app root; signed-out users can jump to Settings/Account.
struct BotsTab: View {
    @Environment(AppNavigationModel.self) private var navigation

    var body: some View {
        #if canImport(harness_runtimeFFI)
        RemoteControlView(openAccount: { navigation.showSettings(.account, origin: .bots) })
        #else
        ContentUnavailableView {
            Label("remote_bots", systemImage: "cpu")
        } description: {
            Text("chat_engine_unavailable")
        }
        #endif
    }
}
