import Foundation

/// What a Bot's card says about the list it shares (`sharedList` of `listSharedBots`); absent when the Bot shares none.
struct TeamBotListInfo: Decodable, Equatable, Sendable {
    let title: String
    /// `requester` when the Bot must ask before it changes the list. Members always change it directly.
    var approval: String? = nil
}
struct TeamSharedListItem: Decodable, Equatable, Identifiable, Sendable {
    let itemId: String
    let text: String
    let done: Bool
    var id: String { itemId }
}
/// A Bot's shared list as `readList` returns it. A change is saved only on top of the `dataRevision` it was made from.
struct TeamSharedList: Decodable, Equatable, Sendable {
    let sharedBotId: String
    let title: String
    let description: String
    let dataRevision: Int
    let items: [TeamSharedListItem]
}
struct TeamSharedListWritten: Decodable, Sendable { let sharedBotId: String; let dataRevision: Int }
/// Who made a revision (`listHistory`): a member (`user`), or the Bot (`shared_bot`) while it worked on `requester`'s request.
struct TeamSharedListAuthor: Decodable, Equatable, Sendable {
    let kind: String
    var uid: String? = nil
    var generation: Int? = nil
    var sharedBotId: String? = nil
    var taskId: String? = nil
    var requester: TeamMemberRef? = nil
}
struct TeamSharedListBefore: Decodable, Equatable, Sendable { let text: String; let done: Bool }
/// One change of a revision; `before` is the item as it was, for a change or a removal.
struct TeamSharedListLoggedChange: Decodable, Equatable, Sendable {
    let op: String
    let itemId: String
    var text: String? = nil
    var done: Bool? = nil
    var before: TeamSharedListBefore? = nil
}
struct TeamSharedListRevision: Decodable, Equatable, Identifiable, Sendable {
    let dataRevision: Int
    let author: TeamSharedListAuthor
    let changes: [TeamSharedListLoggedChange]
    let createdAt: Double
    var id: Int { dataRevision }
}
/// A page of a list's history, newest first. The service keeps the latest 200 revisions.
struct TeamSharedListHistory: Decodable, Equatable, Sendable {
    let sharedBotId: String
    let dataRevision: Int
    let hasMore: Bool
    let revisions: [TeamSharedListRevision]
}
/// The history as far as it was opened, and what it says in words (Desktop `SharedListDialog`).
enum TeamSharedListHistoryRules {
    enum Line: Equatable, Sendable { case added(String), removed(String), renamed(String, String), ticked(String), unticked(String), unchanged(String) }
    enum Person: Equatable, Sendable { case you, member(String), former }
    /// A member by name, or the Bot and the member whose request it was working on.
    enum Author: Equatable, Sendable { case person(Person), bot(String, for: Person) }
    /// What one change did. A change that renamed and ticked an item says both.
    static func lines(_ change: TeamSharedListLoggedChange) -> [Line] {
        if change.op == "add" { return [.added(change.text ?? "")] }
        let was = change.before?.text ?? ""
        if change.op == "remove" { return [.removed(was)] }
        var lines: [Line] = []
        if let text = change.text, text != was { lines.append(.renamed(was, text)) }
        let name = change.text ?? was
        if let done = change.done, done != change.before?.done { lines.append(done ? .ticked(name) : .unticked(name)) }
        return lines.isEmpty ? [.unchanged(name)] : lines
    }
    static func author(_ author: TeamSharedListAuthor, viewer: String, members: [TeamMember], botName: String) -> Author {
        func person(_ ref: TeamMemberRef?) -> Person {
            guard let ref else { return .former }
            if ref.uid == viewer { return .you }
            let name = members.first { $0.uid == ref.uid && $0.generation == ref.generation }?.displayName ?? ""
            return name.isEmpty ? .former : .member(name)
        }
        if author.kind == "shared_bot" { return .bot(botName, for: person(author.requester)) }
        return .person(person(author.uid.flatMap { uid in author.generation.map { TeamMemberRef(uid: uid, generation: $0) } }))
    }
    /// Adds a page to what is shown. Reading the newest page again keeps the older pages already shown, so whether there is
    /// more is still what the oldest page shown said.
    static func merge(_ current: TeamSharedListHistory?, _ page: TeamSharedListHistory, newest: Bool) -> TeamSharedListHistory {
        var known: [Int: TeamSharedListRevision] = [:]
        for revision in (current?.revisions ?? []) + page.revisions { known[revision.dataRevision] = revision }
        let hasMore = newest ? current?.hasMore ?? page.hasMore : page.hasMore
        return TeamSharedListHistory(sharedBotId: page.sharedBotId, dataRevision: max(page.dataRevision, current?.dataRevision ?? 0), hasMore: hasMore,
                                     revisions: known.values.sorted { $0.dataRevision > $1.dataRevision })
    }
}
/// One change of a `writeList` request, exactly as the service accepts it.
enum TeamSharedListChange: Equatable, Sendable {
    case add(text: String)
    case setText(itemId: String, text: String)
    case setDone(itemId: String, done: Bool)
    case remove(itemId: String)

    var payload: [String: Any] {
        switch self {
        case .add(let text): ["op": "add", "text": text]
        case .setText(let itemId, let text): ["op": "update", "itemId": itemId, "text": text]
        case .setDone(let itemId, let done): ["op": "update", "itemId": itemId, "done": done]
        case .remove(let itemId): ["op": "remove", "itemId": itemId]
        }
    }
}
/// The identity of one exact change of one revision. Saving the same change again after a lost answer sends the same id, so
/// nothing is changed twice; a different change, or the same change on a newer list, gets a new id.
struct TeamSharedListOperation: Equatable, Sendable {
    let revision: Int
    let changes: [TeamSharedListChange]
    let id: String

    static func of(_ current: TeamSharedListOperation?, revision: Int, changes: [TeamSharedListChange], newId: () -> String = { UUID().uuidString }) -> TeamSharedListOperation {
        if let current, current.revision == revision, current.changes == changes { return current }
        return TeamSharedListOperation(revision: revision, changes: changes, id: newId())
    }
}
enum TeamSharedListRules {
    static func fits(_ text: String) -> Bool { text.utf8.count <= TeamExecutionContract.listItemBytes }
    /// Text the service stores as an item: not blank, within the size limit.
    static func usable(_ text: String) -> Bool {
        !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && fits(text) && !text.contains("\0")
    }
    /// Someone else, or the Bot, saved first (or removed the item): nothing was saved and the list has to be read again.
    static func moved(_ code: String) -> Bool { code == "LIST_REVISION_CONFLICT" || code == "LIST_ITEM_REQUIRED" }
    /// The card offers the list to whoever the service lets read it: the people who may use the Bot, and its maintainer.
    static func canOpen(_ bot: TeamBotCard, viewer: TeamMemberRef) -> Bool {
        bot.sharedList != nil && bot.state == "published" && (bot.permitted || bot.maintainer == viewer)
    }
}
