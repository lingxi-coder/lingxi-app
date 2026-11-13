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
