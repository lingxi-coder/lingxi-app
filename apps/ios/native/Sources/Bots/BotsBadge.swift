import Observation

/// The Bots tab badge (spec §2.5, D5): the chats waiting for the user on the connected computer, summed
/// (`TabLogic.botsBadgeCount`). Created by the app root and injected with `.environment(_:)`.
///
/// I-SHELL ships the stub (always 0). I-BOTS publishes into `count` from `RemoteControlManager`
/// whenever the connected computer's Bot list changes, and resets it to 0 on disconnect / sign-out.
@MainActor
@Observable
final class BotsBadge {
    var count = 0
}
