package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject

/** What a Bot's card says about the list it shares (`sharedList` of `listSharedBots`). [approval] is `requester` when the Bot must ask before it changes the list. */
internal data class TeamBotListInfo(val title: String, val approval: String? = null) {
    companion object { fun parse(v: JSONObject) = TeamBotListInfo(v.getString("title"), v.stringOrNull("approval")) }
}
internal data class TeamSharedListItem(val itemId: String, val text: String, val done: Boolean)
/** A Bot's shared list as `readList` returns it. A change is saved only on top of the [dataRevision] it was made from. */
internal data class TeamSharedList(val sharedBotId: String, val title: String, val description: String, val dataRevision: Long, val items: List<TeamSharedListItem>) {
    companion object {
        fun parse(v: JSONObject) = TeamSharedList(v.getString("sharedBotId"), v.getString("title"), v.getString("description"), v.getLong("dataRevision"),
            v.getJSONArray("items").objects().map { TeamSharedListItem(it.getString("itemId"), it.getString("text"), it.getBoolean("done")) })
    }
}
/** One change of a `writeList` request, exactly as the service accepts it. */
internal sealed interface TeamSharedListChange {
    data class Add(val text: String) : TeamSharedListChange
    data class SetText(val itemId: String, val text: String) : TeamSharedListChange
    data class SetDone(val itemId: String, val done: Boolean) : TeamSharedListChange
    data class Remove(val itemId: String) : TeamSharedListChange

    fun json(): JSONObject = when (this) {
        is Add -> JSONObject().put("op", "add").put("text", text)
        is SetText -> JSONObject().put("op", "update").put("itemId", itemId).put("text", text)
        is SetDone -> JSONObject().put("op", "update").put("itemId", itemId).put("done", done)
        is Remove -> JSONObject().put("op", "remove").put("itemId", itemId)
    }
}
internal fun List<TeamSharedListChange>.json() = JSONArray(map { it.json() })
/**
 * The identity of one exact change of one revision. Saving the same change again after a lost answer sends the same id, so nothing is
 * changed twice; a different change, or the same change on a newer list, gets a new id.
 */
internal data class TeamSharedListOperation(val revision: Long, val changes: List<TeamSharedListChange>, val id: String) {
    companion object {
        fun of(current: TeamSharedListOperation?, revision: Long, changes: List<TeamSharedListChange>, newId: () -> String = { java.util.UUID.randomUUID().toString() }) =
            if (current != null && current.revision == revision && current.changes == changes) current else TeamSharedListOperation(revision, changes, newId())
    }
}
/** Who made a revision (`listHistory`): a member (`user`), or the Bot (`shared_bot`) while it worked on [requester]'s request. */
internal data class TeamSharedListAuthor(val kind: String, val member: TeamMemberRef?, val sharedBotId: String?, val taskId: String?, val requester: TeamMemberRef?) {
    companion object {
        private fun ref(v: JSONObject?) = v?.let { TeamMemberRef(it.getString("uid"), it.getLong("generation")) }
        fun parse(v: JSONObject) = TeamSharedListAuthor(v.getString("kind"), if (v.has("uid")) TeamMemberRef(v.getString("uid"), v.getLong("generation")) else null,
            v.stringOrNull("sharedBotId"), v.stringOrNull("taskId"), ref(v.optJSONObject("requester")))
    }
}
/** One change of a revision; [beforeText]/[beforeDone] are the item as it was, for a change or a removal. */
internal data class TeamSharedListLoggedChange(val op: String, val itemId: String, val text: String?, val done: Boolean?, val beforeText: String?, val beforeDone: Boolean?) {
    companion object {
        fun parse(v: JSONObject): TeamSharedListLoggedChange {
            val before = v.optJSONObject("before")
            return TeamSharedListLoggedChange(v.getString("op"), v.getString("itemId"), v.stringOrNull("text"), if (v.has("done")) v.getBoolean("done") else null,
                before?.getString("text"), before?.getBoolean("done"))
        }
    }
}
internal data class TeamSharedListRevision(val dataRevision: Long, val author: TeamSharedListAuthor, val changes: List<TeamSharedListLoggedChange>, val createdAt: Long)
/** A page of a list's history, newest first. The service keeps the latest 200 revisions. */
internal data class TeamSharedListHistory(val sharedBotId: String, val dataRevision: Long, val hasMore: Boolean, val revisions: List<TeamSharedListRevision>) {
    companion object {
        fun parse(v: JSONObject) = TeamSharedListHistory(v.getString("sharedBotId"), v.getLong("dataRevision"), v.getBoolean("hasMore"),
            v.getJSONArray("revisions").objects().map { r -> TeamSharedListRevision(r.getLong("dataRevision"), TeamSharedListAuthor.parse(r.getJSONObject("author")),
                r.getJSONArray("changes").objects().map(TeamSharedListLoggedChange::parse), r.getLong("createdAt")) })
    }
}
/** The history as far as it was opened, and what it says in words (Desktop `SharedListDialog`, iOS `TeamSharedListHistoryRules`). */
internal object TeamSharedListHistoryRules {
    sealed interface Line {
        data class Added(val text: String) : Line
        data class Removed(val text: String) : Line
        data class Renamed(val was: String, val text: String) : Line
        data class Ticked(val text: String) : Line
        data class Unticked(val text: String) : Line
        data class Unchanged(val text: String) : Line
    }
    sealed interface Person { data object You : Person; data class Member(val name: String) : Person; data object Former : Person }
    /** A member by name, or the Bot and the member whose request it was working on. */
    sealed interface Author { data class Of(val person: Person) : Author; data class Bot(val name: String, val requester: Person) : Author }
    /** What one change did. A change that renamed and ticked an item says both. */
    fun lines(change: TeamSharedListLoggedChange): List<Line> {
        if (change.op == "add") return listOf(Line.Added(change.text.orEmpty()))
        val was = change.beforeText.orEmpty()
        if (change.op == "remove") return listOf(Line.Removed(was))
        val name = change.text ?: was
        val lines = buildList {
            if (change.text != null && change.text != was) add(Line.Renamed(was, change.text))
            if (change.done != null && change.done != change.beforeDone) add(if (change.done) Line.Ticked(name) else Line.Unticked(name))
        }
        return lines.ifEmpty { listOf(Line.Unchanged(name)) }
    }
    fun author(author: TeamSharedListAuthor, viewer: String, members: List<TeamMember>, botName: String): Author {
        fun person(ref: TeamMemberRef?): Person = when {
            ref == null -> Person.Former
            ref.uid == viewer -> Person.You
            else -> members.firstOrNull { it.uid == ref.uid && it.generation == ref.generation }?.displayName?.takeIf { it.isNotEmpty() }?.let { Person.Member(it) } ?: Person.Former
        }
        return if (author.kind == "shared_bot") Author.Bot(botName, person(author.requester)) else Author.Of(person(author.member))
    }
    /** Adds a page to what is shown. Reading the newest page again keeps the older pages already shown, so whether there is more is still what the oldest page shown said. */
    fun merge(current: TeamSharedListHistory?, page: TeamSharedListHistory, newest: Boolean): TeamSharedListHistory {
        val known = ((current?.revisions ?: emptyList()) + page.revisions).associateBy { it.dataRevision }
        return TeamSharedListHistory(page.sharedBotId, maxOf(page.dataRevision, current?.dataRevision ?: 0), if (newest) current?.hasMore ?: page.hasMore else page.hasMore,
            known.values.sortedByDescending { it.dataRevision })
    }
}
internal object TeamSharedListRules {
    fun fits(text: String) = text.toByteArray(Charsets.UTF_8).size <= TeamExecutionContract.LIST_ITEM_BYTES
    /** Text the service stores as an item: not blank, within the size limit. */
    fun usable(text: String) = text.trim().isNotEmpty() && fits(text) && !text.contains('\u0000')
    /** Someone else, or the Bot, saved first (or removed the item): nothing was saved and the list has to be read again. */
    fun moved(code: String) = code == "LIST_REVISION_CONFLICT" || code == "LIST_ITEM_REQUIRED"
    /** The card offers the list to whoever the service lets read it: the people who may use the Bot, and its maintainer. */
    fun canOpen(bot: TeamBotCard, uid: String, generation: Long) =
        bot.sharedList != null && bot.state == "published" && (bot.permitted || bot.maintainer == TeamMemberRef(uid, generation))
}
