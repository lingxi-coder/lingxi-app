import Observation

/// The app-level Teams data layer (spec §6.6, D10): started when signed in, stopped on sign-out or a
/// uid change, so the Teams tab badge works without opening the tab. Created by the app root and
/// injected with `.environment(_:)`.
///
/// I-SHELL ships only the badge input (`totalUnread`, always 0). I-TEAMS replaces this body with the
/// real workspace (teams list, selected-team messaging, directory-only watchers) and keeps the
/// property name: the shell reads `totalUnread` for the Teams badge.
@MainActor
@Observable
final class TeamsWorkspace {
    /// Σ unread over every conversation of every team the user belongs to (spec §2.5).
    var totalUnread = 0
}
