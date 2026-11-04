import SwiftUI

/// The Teams tab (spec §6). I-SHELL re-hosts today's `TeamsPage` as the tab root inside its own
/// `NavigationStack`; I-TEAMS replaces the body with the conversation list. It reads the shell state
/// from the environment: `TabCoordinator` (join-link hand-off, depth) and `AppNavigationModel` (the
/// sign-in jump to Settings/Account, which remembers this tab as the way back).
struct TeamsTab: View {
    @Environment(AppNavigationModel.self) private var navigation
    @Environment(TabCoordinator.self) private var tabs
    /// The invite code of a `team/join` link, kept until the page has put it in its join form.
    @State private var invite: String?

    var body: some View {
        NavigationStack {
            TeamsPage(initialInvite: invite, onOpenAccount: { navigation.showSettings(.account, origin: .teams) })
        }
        .onChange(of: tabs.teamsJoinCode, initial: true) { _, code in
            guard let code else { return }
            invite = code
            tabs.teamsJoinCode = nil
        }
    }
}
