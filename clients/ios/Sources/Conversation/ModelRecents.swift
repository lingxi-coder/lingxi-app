import Foundation

/// The models the user has most recently PICKED, newest first.
///
/// Client-local on purpose. The engine keeps its own recents list
/// (`tui-core::recent_models` → `~/.lingxi/settings.json`'s `recentModels`), but
/// nothing carries it over `ClientEvent::ModelList`, so surfacing it here would
/// mean a protocol change and a four-client rollout. Desktop and phone recents
/// are therefore independent; that is an accepted divergence, not an oversight.
///
/// Entries are the provider-QUALIFIED reference (`anthropic/claude-sonnet-5`) —
/// byte-identical to what `ModelList` carries and what `SetModel` submits — so a
/// stored entry can be matched against the live catalog by plain equality. The
/// engine's own list splits provider and model into two fields instead; the two
/// are not interchangeable, which is another reason not to pretend they are one
/// store.
struct ModelRecents {
    /// How many picks to remember. The engine's list keeps 8; 5 is enough to
    /// stay useful without letting the picker's "recent" section crowd out the
    /// provider sections below it.
    static let limit = 5

    private static let storageKey = "conversation.model.recents"

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// The remembered references, most-recently-picked first.
    func references() -> [String] {
        defaults.stringArray(forKey: Self.storageKey) ?? []
    }

    /// Record an explicit user pick: move it to the front, keep one entry per
    /// reference, and forget the oldest beyond ``limit``.
    ///
    /// Only a deliberate selection belongs here. The active model also changes
    /// on every `ModelList` / `ModelChanged` the engine emits (boot, session
    /// resume, a provider reconnect), and recording those would fill the list
    /// with models the user never chose.
    func record(_ reference: String) {
        let trimmed = reference.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        var next = references().filter { $0 != trimmed }
        next.insert(trimmed, at: 0)
        defaults.set(Array(next.prefix(Self.limit)), forKey: Self.storageKey)
    }

    /// The remembered references that the engine still offers, in recency order.
    ///
    /// A model whose provider was deleted (or disabled) is silently dropped
    /// rather than shown as a row that cannot be selected.
    func resolved(against available: [String]) -> [String] {
        let offered = Set(available)
        return references().filter { offered.contains($0) }
    }
}
