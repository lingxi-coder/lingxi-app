import Observation

/// The Bots tab badge (spec §2.5, D5): the chats waiting for the user on the connected computer, summed
/// (`TabLogic.botsBadgeCount`). Created by the app root and injected with `.environment(_:)`.
///
/// RootView publishes the manager's live connected-host count and resets it on disconnect/sign-out.
@MainActor
@Observable
final class BotsBadge {
    var count = 0
}
