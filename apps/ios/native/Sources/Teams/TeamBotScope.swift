import Foundation

// What a shared Bot may use of its maintainer's Desktop in a Team, as ids, names and flags only: never a path, token or secret
// (mirrors `packages/teams/bot-scope.ts`; Android has the identical contract in `teams/TeamBotScope.kt`). Each layer only narrows the one
// before it: the Bot's own upper bound on its Desktop, what the maintainer published, and what a Chat narrows it to. A dataset also needs a
// live grant for the Chat. `TeamBotScopeRules.effective` is the middle two, which is what a member is shown as "what this Bot can use here".
// Pure: Foundation only, so the same file runs in the macOS XCTest harness against `bot-scope-vectors.json` and `execution-vectors.json`.

/// One Local App of a published scope: its id and the name it is shown by.
struct TeamBotScopeApp: Decodable, Equatable, Identifiable, Sendable {
    let id: String
    let name: String
}

/// One dataset of a published scope and what the Bot may do in it. A dataset appears once in a scope, so it is named by its id alone.
struct TeamBotScopeDataset: Decodable, Equatable, Identifiable, Sendable {
    let source: String
    let dataset: String
    let write: Bool
    let reportReview: Bool
    var id: String { dataset }
}

/// What the maintainer published of their Desktop for the Bot to use in the Team. A card carries it only when it is not empty.
struct TeamBotScope: Decodable, Equatable, Sendable {
    var localApps: [TeamBotScopeApp] = []
    var datasets: [TeamBotScopeDataset] = []
    static let empty = TeamBotScope()
    var isEmpty: Bool { localApps.isEmpty && datasets.isEmpty }

    init(localApps: [TeamBotScopeApp] = [], datasets: [TeamBotScopeDataset] = []) {
        self.localApps = localApps
        self.datasets = datasets
    }
    private enum CodingKeys: String, CodingKey { case localApps, datasets }
    /// A list that is left out is empty. One that is there and unreadable breaks the card, like any other field of it.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        localApps = try c.decodeIfPresent([TeamBotScopeApp].self, forKey: .localApps) ?? []
        datasets = try c.decodeIfPresent([TeamBotScopeDataset].self, forKey: .datasets) ?? []
    }
}

/// A dataset a Chat's narrowing names: the source it is read through and its id.
struct TeamBotScopeDatasetRef: Equatable, Sendable {
    let source: String
    let dataset: String
}

/// A Chat's narrowing of one Bot: a list that is left out (`nil`) does not narrow that kind of thing, and `[]` allows none of it.
struct TeamBotScopeNarrowing: Equatable, Sendable {
    var localApps: [String]? = nil
    var datasets: [TeamBotScopeDatasetRef]? = nil

    /// The `scope` of `setConversationBotScope`: only the lists the narrowing names, each as the service reads it.
    var payload: [String: Any] {
        var value: [String: Any] = [:]
        if let localApps { value["localApps"] = localApps }
        if let datasets { value["datasets"] = datasets.map { ["source": $0.source, "dataset": $0.dataset] } }
        return value
    }
}

/// The narrowing as the Chat holds it, with the revision of this Bot's entry (sent back as `expectedRevision`). A lifted narrowing keeps only its revision.
struct TeamBotScopeEntry: Equatable, Sendable {
    let revision: Int
    var localApps: [String]? = nil
    var datasets: [TeamBotScopeDatasetRef]? = nil
    var narrowing: TeamBotScopeNarrowing { TeamBotScopeNarrowing(localApps: localApps, datasets: datasets) }
}

/// What a person ticks in a Chat to say what a Bot may use there: Local App ids, and datasets by their id (a dataset appears once in a scope).
struct TeamBotScopeChoice: Equatable, Sendable {
    var localApps: [String] = []
    var datasets: [String] = []
}

/// What one Bot's section of the editor starts from: the revision of the Chat's entry, what the Bot published, and what that leaves allowed in this Chat.
/// When any of them changes (a save, someone else, the maintainer), what was ticked is not about what the service holds any more, and the ticks start again.
struct TeamBotScopeBasis: Equatable, Sendable {
    let revision: Int
    let published: TeamBotScope
    let allowed: TeamBotScopeChoice

    init(published: TeamBotScope?, entry: TeamBotScopeEntry?) {
        let have = published ?? .empty
        revision = entry?.revision ?? 0
        self.published = have
        allowed = TeamBotScopeRules.choice(of: TeamBotScopeRules.effective(have, entry?.narrowing))
    }
}

enum TeamBotScopeRules {
    /// The most Local Apps and the most datasets one scope names.
    static let entries = 64

    /// What the Bot may use in a Chat: what it published, cut down to what the Chat allows. A Bot published without a scope has none.
    static func effective(_ published: TeamBotScope?, _ narrowing: TeamBotScopeNarrowing?) -> TeamBotScope {
        let have = published ?? .empty
        return TeamBotScope(
            localApps: have.localApps.filter { app in narrowing?.localApps?.contains(app.id) ?? true },
            datasets: have.datasets.filter { row in narrowing?.datasets?.contains { $0.source == row.source && $0.dataset == row.dataset } ?? true })
    }

    /// What a Bot may use in a Chat, given its card and the Chat (`nil`: no Chat, so everything it published).
    static func effective(of card: TeamBotCard, in conversation: TeamMessageConversation?) -> TeamBotScope {
        effective(card.scope, conversation?.botScopes?[card.sharedBotId]?.narrowing)
    }

    /// Whether a Chat lets a Bot use less of what it published than the Bot published.
    static func narrowed(_ published: TeamBotScope?, to effective: TeamBotScope) -> Bool {
        effective.localApps.count < (published?.localApps.count ?? 0) || effective.datasets.count < (published?.datasets.count ?? 0)
    }

    /// The choice that allows exactly what is allowed now.
    static func choice(of allowed: TeamBotScope) -> TeamBotScopeChoice {
        TeamBotScopeChoice(localApps: allowed.localApps.map(\.id), datasets: allowed.datasets.map(\.dataset))
    }

    /// Whether two choices allow the same, whatever their order.
    static func same(_ a: TeamBotScopeChoice, _ b: TeamBotScopeChoice) -> Bool {
        Set(a.localApps) == Set(b.localApps) && a.localApps.count == b.localApps.count
            && Set(a.datasets) == Set(b.datasets) && a.datasets.count == b.datasets.count
    }

    /// `ids` with `id` in it, or without it.
    static func toggled(_ ids: [String], _ id: String, on: Bool) -> [String] {
        on ? ids.filter { $0 != id } + [id] : ids.filter { $0 != id }
    }

    /// The narrowing that says what `choice` allows of what the Bot published. A kind of which nothing is left out is not narrowed, so what the
    /// maintainer publishes of it later is allowed too; `nil` (no narrowing at all) when nothing is left out of either. Only what is published is ever named.
    static func narrowing(of choice: TeamBotScopeChoice, in published: TeamBotScope) -> TeamBotScopeNarrowing? {
        let apps = published.localApps.filter { choice.localApps.contains($0.id) }.map(\.id)
        let rows = published.datasets.filter { choice.datasets.contains($0.dataset) }.map { TeamBotScopeDatasetRef(source: $0.source, dataset: $0.dataset) }
        let narrowApps = apps.count < published.localApps.count, narrowRows = rows.count < published.datasets.count
        guard narrowApps || narrowRows else { return nil }
        return TeamBotScopeNarrowing(localApps: narrowApps ? apps : nil, datasets: narrowRows ? rows : nil)
    }

    /// Whether a Chat holds a narrowing of a Bot now (an entry that was lifted keeps only its revision).
    static func limits(_ entry: TeamBotScopeEntry?) -> Bool { entry?.localApps != nil || entry?.datasets != nil }

    // MARK: The Chat's side

    /// The narrowings of a Chat's Bots as its document carries them. An entry that is not well formed is left out, as the Chat then has none for that Bot;
    /// a Chat with no entry has none.
    static func decode(_ value: Any?) -> [String: TeamBotScopeEntry]? {
        guard let map = value as? [String: Any] else { return nil }
        var result: [String: TeamBotScopeEntry] = [:]
        for (sharedBotId, raw) in map {
            guard let entry = raw as? [String: Any], let revision = TeamsPointerParser.safeInteger(entry["revision"]), revision >= 1 else { continue }
            var apps: [String]?, rows: [TeamBotScopeDatasetRef]?
            if entry.keys.contains("localApps") {
                guard let list = entry["localApps"] as? [String] else { continue }
                apps = list
            }
            if entry.keys.contains("datasets") {
                guard let list = entry["datasets"] as? [[String: Any]] else { continue }
                let refs = list.compactMap { row -> TeamBotScopeDatasetRef? in
                    guard let source = row["source"] as? String, let dataset = row["dataset"] as? String else { return nil }
                    return TeamBotScopeDatasetRef(source: source, dataset: dataset)
                }
                guard refs.count == list.count else { continue }
                rows = refs
            }
            result[sharedBotId] = TeamBotScopeEntry(revision: revision, localApps: apps, datasets: rows)
        }
        return result.isEmpty ? nil : result
    }

    /// Whether this person takes part in a Chat with Bots, and so may set what its Bots do there: whether they answer on their own, and what they use.
    static func inBotChat(_ conversation: TeamMessageConversation, uid: String) -> Bool {
        TeamAutoReply.kinds.contains(conversation.kind) && conversation.participants.contains("user:" + uid)
    }

    /// The Bots of a Chat that share something of their maintainer's computer: these are the ones a person in the Chat can limit.
    static func limitable(_ conversation: TeamMessageConversation, cards: [TeamBotCard], uid: String) -> [TeamBotCard] {
        guard inBotChat(conversation, uid: uid) else { return [] }
        return cards.filter { conversation.participants.contains("shared_bot:" + $0.sharedBotId) && $0.scope?.isEmpty == false }
    }

    // MARK: What is sent

    private static func matches(_ value: Any?, _ pattern: String) -> Bool {
        guard let text = value as? String else { return false }
        return text.range(of: pattern, options: .regularExpression) != nil
    }
    /// A Local App id, and a dataset id with the source it is read through, as the service accepts them.
    private static let appId = "\\A[A-Za-z0-9._:-]{1,128}\\z", dataId = "\\A[A-Za-z0-9_-]{1,64}\\z"

    /// A narrowing exactly as the service accepts it, in the `scope` of `setConversationBotScope`: `null` (lifted), or an object of two optional lists. A Local
    /// App and a dataset appear once, a dataset by its id alone, and a row names its source and its id and nothing else.
    static func validNarrowing(_ value: Any?) -> Bool {
        if value is NSNull { return true }
        guard let scope = value as? [String: Any], Set(scope.keys).isSubset(of: ["localApps", "datasets"]) else { return false }
        if let apps = scope["localApps"] {
            guard let list = apps as? [Any], list.count <= entries, Set(list.compactMap { $0 as? String }).count == list.count,
                  list.allSatisfy({ matches($0, appId) }) else { return false }
        }
        if let datasets = scope["datasets"] {
            guard let list = datasets as? [[String: Any]], list.count <= entries, list.allSatisfy({ Set($0.keys) == ["source", "dataset"] && matches($0["source"], dataId) && matches($0["dataset"], dataId) }),
                  Set(list.compactMap { $0["dataset"] as? String }).count == list.count else { return false }
        }
        return true
    }
}
