import SwiftUI

/// The Bots tab (spec §4). I-SHELL re-hosts today's Remote Control screens (computers list, a computer,
/// its Bots and chats) as the tab root; I-BOTS replaces the body with the Bots tree. It reads the app-level
/// `RemoteControlManager` and `TabCoordinator` from the environment, and sends the signed-out user to
/// Settings/Account with this tab as the way back.
///
/// The engine-bound screens exist only when the runtime bindings do; without them the tab shows the
/// existing "engine unavailable" state instead of disappearing (tabs are never hidden).
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
