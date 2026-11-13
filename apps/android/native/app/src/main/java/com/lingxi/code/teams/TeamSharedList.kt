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
