package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject

// What a shared Bot may use of its maintainer's Desktop in a Team, as ids, names and flags only: never a path, token or secret
// (mirrors `packages/teams/bot-scope.ts`; iOS has the identical contract in `TeamBotScope.swift`). Each layer only narrows the one
// before it: the Bot's own upper bound on its Desktop, what the maintainer published, and what a Chat narrows it to. A dataset also needs a
// live grant for the Chat. `TeamBotScopeRules.effective` is the middle two, which is what a member is shown as "what this Bot can use here".
// Pure: no Android types, so the JVM unit tests run it against `bot-scope-vectors.json` and `execution-vectors.json`.

/** One Local App of a published scope: its id and the name it is shown by. */
internal data class TeamBotScopeApp(val id: String, val name: String)

/** One dataset of a published scope and what the Bot may do in it. A dataset appears once in a scope, so it is named by its id alone. */
internal data class TeamBotScopeDataset(val source: String, val dataset: String, val write: Boolean, val reportReview: Boolean)

/** What the maintainer published of their Desktop for the Bot to use in the Team. A card carries it only when it is not empty. */
internal data class TeamBotScope(val localApps: List<TeamBotScopeApp> = emptyList(), val datasets: List<TeamBotScopeDataset> = emptyList()) {
    val isEmpty get() = localApps.isEmpty() && datasets.isEmpty()
    companion object {
        val EMPTY = TeamBotScope()
        /** A list that is left out is empty. One that is there and unreadable breaks the card, like any other field of it. */
        fun parse(v: JSONObject): TeamBotScope {
            fun list(key: String): JSONArray? = if (v.isNull(key)) null else v.getJSONArray(key)
            return TeamBotScope(
                list("localApps")?.objects()?.map { TeamBotScopeApp(it.getString("id"), it.getString("name")) }.orEmpty(),
                list("datasets")?.objects()?.map { TeamBotScopeDataset(it.getString("source"), it.getString("dataset"), it.getBoolean("write"), it.getBoolean("reportReview")) }.orEmpty(),
            )
        }
    }
}

/** A dataset a Chat's narrowing names: the source it is read through and its id. */
internal data class TeamBotScopeDatasetRef(val source: String, val dataset: String)

/** A Chat's narrowing of one Bot: a list that is left out (null) does not narrow that kind of thing, and an empty list allows none of it. */
internal data class TeamBotScopeNarrowing(val localApps: List<String>? = null, val datasets: List<TeamBotScopeDatasetRef>? = null) {
    /** The `scope` of `setConversationBotScope`: only the lists the narrowing names, each as the service reads it. */
    fun json(): JSONObject = JSONObject().also { value ->
        localApps?.let { value.put("localApps", JSONArray(it)) }
        datasets?.let { rows -> value.put("datasets", JSONArray().also { array -> rows.forEach { array.put(JSONObject().put("source", it.source).put("dataset", it.dataset)) } }) }
    }
}

/** The narrowing as the Chat holds it, with the revision of this Bot's entry (sent back as `expectedRevision`). A lifted narrowing keeps only its revision. */
internal data class TeamBotScopeEntry(val revision: Long, val localApps: List<String>? = null, val datasets: List<TeamBotScopeDatasetRef>? = null) {
    val narrowing get() = TeamBotScopeNarrowing(localApps, datasets)
}

/** What a person ticks in a Chat to say what a Bot may use there: Local App ids, and datasets by their id (a dataset appears once in a scope). */
internal data class TeamBotScopeChoice(val localApps: List<String> = emptyList(), val datasets: List<String> = emptyList())

/**
 * What one Bot's section of the editor starts from: the revision of the Chat's entry, what the Bot published, and what that leaves allowed in this Chat.
 * When any of them changes (a save, someone else, the maintainer), what was ticked is not about what the service holds any more, and the ticks start again.
 */
internal data class TeamBotScopeBasis(val revision: Long, val published: TeamBotScope, val allowed: TeamBotScopeChoice) {
    companion object {
        fun of(published: TeamBotScope?, entry: TeamBotScopeEntry?): TeamBotScopeBasis {
            val have = published ?: TeamBotScope.EMPTY
            return TeamBotScopeBasis(entry?.revision ?: 0, have, TeamBotScopeRules.choice(TeamBotScopeRules.effective(have, entry?.narrowing)))
        }
    }
}

internal object TeamBotScopeRules {
    /** The most Local Apps and the most datasets one scope names. */
    const val ENTRIES = 64
    // A Local App id, and a dataset id with the source it is read through, as the service accepts them.
    private val appId = Regex("[A-Za-z0-9._:-]{1,128}")
    private val dataId = Regex("[A-Za-z0-9_-]{1,64}")

    /** What the Bot may use in a Chat: what it published, cut down to what the Chat allows. A Bot published without a scope has none. */
    fun effective(published: TeamBotScope?, narrowing: TeamBotScopeNarrowing?): TeamBotScope {
        val have = published ?: TeamBotScope.EMPTY
        return TeamBotScope(
            have.localApps.filter { app -> narrowing?.localApps?.contains(app.id) ?: true },
            have.datasets.filter { row -> narrowing?.datasets?.any { it.source == row.source && it.dataset == row.dataset } ?: true },
        )
    }

    /** What a Bot may use in a Chat, given its card and the Chat (null: no Chat, so everything it published). */
    fun effective(card: TeamBotCard, conversation: TeamMessageConversation?): TeamBotScope =
        effective(card.scope, conversation?.botScopes?.get(card.sharedBotId)?.narrowing)

    /** Whether a Chat lets a Bot use less of what it published than the Bot published. */
    fun narrowed(published: TeamBotScope?, effective: TeamBotScope): Boolean =
        effective.localApps.size < (published?.localApps?.size ?: 0) || effective.datasets.size < (published?.datasets?.size ?: 0)

    /** The choice that allows exactly what is allowed now. */
    fun choice(allowed: TeamBotScope) = TeamBotScopeChoice(allowed.localApps.map { it.id }, allowed.datasets.map { it.dataset })

    /** Whether two choices allow the same, whatever their order. */
    fun same(a: TeamBotScopeChoice, b: TeamBotScopeChoice): Boolean =
        a.localApps.size == b.localApps.size && a.localApps.toSet() == b.localApps.toSet() && a.datasets.size == b.datasets.size && a.datasets.toSet() == b.datasets.toSet()

    /** [ids] with [id] in it, or without it. */
    fun toggled(ids: List<String>, id: String, on: Boolean): List<String> = if (on) ids.filter { it != id } + id else ids.filter { it != id }

    /**
     * The narrowing that says what [choice] allows of what the Bot published. A kind of which nothing is left out is not narrowed, so what the maintainer
     * publishes of it later is allowed too; null (no narrowing at all) when nothing is left out of either. Only what is published is ever named.
     */
    fun narrowing(choice: TeamBotScopeChoice, published: TeamBotScope): TeamBotScopeNarrowing? {
        val apps = published.localApps.filter { it.id in choice.localApps }.map { it.id }
        val rows = published.datasets.filter { it.dataset in choice.datasets }.map { TeamBotScopeDatasetRef(it.source, it.dataset) }
        val narrowApps = apps.size < published.localApps.size; val narrowRows = rows.size < published.datasets.size
        return if (narrowApps || narrowRows) TeamBotScopeNarrowing(if (narrowApps) apps else null, if (narrowRows) rows else null) else null
    }

    /** Whether a Chat holds a narrowing of a Bot now (an entry that was lifted keeps only its revision). */
    fun limits(entry: TeamBotScopeEntry?): Boolean = entry?.localApps != null || entry?.datasets != null

    // The Chat's side ------------------------------------------------------------------------------------------------

    /**
     * The narrowings of a Chat's Bots as its document carries them. An entry that is not well formed is left out, as the Chat then has none for that Bot;
     * a Chat with no entry has none. Never throws: an unreadable setting must not hide the conversation.
     */
    fun decode(value: Any?): Map<String, TeamBotScopeEntry>? {
        val map = value as? Map<*, *> ?: return null
        val result = LinkedHashMap<String, TeamBotScopeEntry>()
        for ((key, raw) in map) {
            val sharedBotId = key as? String ?: continue
            val entry = raw as? Map<*, *> ?: continue
            val revision = TeamsPointerParser.safeInteger(entry["revision"])?.takeIf { it >= 1 } ?: continue
            var apps: List<String>? = null; var rows: List<TeamBotScopeDatasetRef>? = null
            if (entry.containsKey("localApps")) apps = (entry["localApps"] as? List<*>)?.takeIf { list -> list.all { it is String } }?.map { it as String } ?: continue
            if (entry.containsKey("datasets")) rows = datasetRefs(entry["datasets"]) ?: continue
            result[sharedBotId] = TeamBotScopeEntry(revision, apps, rows)
        }
        return result.takeIf { it.isNotEmpty() }
    }

    /** The datasets a narrowing names, or null when any of them is not a row with its source and its id. */
    private fun datasetRefs(value: Any?): List<TeamBotScopeDatasetRef>? = (value as? List<*>)?.map { row ->
        val fields = row as? Map<*, *> ?: return null
        TeamBotScopeDatasetRef(fields["source"] as? String ?: return null, fields["dataset"] as? String ?: return null)
    }

    /** Whether this person takes part in a Chat with Bots, and so may set what its Bots do there: whether they answer on their own, and what they use. */
    fun inBotChat(conversation: TeamMessageConversation, uid: String): Boolean = conversation.kind in TeamAutoReply.kinds && "user:$uid" in conversation.participants

    /** The Bots of a Chat that share something of their maintainer's computer: these are the ones a person in the Chat can limit. */
    fun limitable(conversation: TeamMessageConversation, cards: List<TeamBotCard>, uid: String): List<TeamBotCard> =
        if (!inBotChat(conversation, uid)) emptyList() else cards.filter { "shared_bot:${it.sharedBotId}" in conversation.participants && it.scope?.isEmpty == false }

    // What is sent ---------------------------------------------------------------------------------------------------

    /**
     * A narrowing exactly as the service accepts it, in the `scope` of `setConversationBotScope`: null (lifted), or an object of two optional lists. A Local
     * App and a dataset appear once, a dataset by its id alone, and a row names its source and its id and nothing else.
     */
    fun validNarrowing(value: Any?): Boolean {
        if (value === JSONObject.NULL) return true
        val scope = value as? JSONObject ?: return false
        if (!setOf("localApps", "datasets").containsAll(scope.keys().asSequence().toSet())) return false
        if (scope.has("localApps")) {
            val list = scope.opt("localApps") as? JSONArray ?: return false
            val apps = (0 until list.length()).map { list.get(it) }
            if (apps.size > ENTRIES || apps.distinct().size != apps.size || apps.any { it !is String || !appId.matches(it) }) return false
        }
        if (scope.has("datasets")) {
            val list = scope.opt("datasets") as? JSONArray ?: return false
            if (list.length() > ENTRIES) return false
            val seen = mutableSetOf<String>()
            for (index in 0 until list.length()) {
                val row = list.opt(index) as? JSONObject ?: return false
                val source = row.opt("source"); val dataset = row.opt("dataset")
                if (row.keys().asSequence().toSet() != setOf("source", "dataset") || source !is String || dataset !is String ||
                    !dataId.matches(source) || !dataId.matches(dataset) || !seen.add(dataset)) return false
            }
        }
        return true
    }
}
