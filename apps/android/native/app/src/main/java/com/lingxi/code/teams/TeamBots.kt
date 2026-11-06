package com.lingxi.code.teams

import org.json.JSONObject

internal data class TeamMemberRef(val uid: String, val generation: Long) {
    companion object { fun parse(v: JSONObject) = TeamMemberRef(v.getString("uid"), v.getLong("generation")) }
}
internal data class TeamBotCard(val sharedBotId: String, val name: String, val description: String, val maintainer: TeamMemberRef,
    val revision: Long, val state: String, val permitted: Boolean, val ready: Boolean, val onlineUntil: Long, val usePolicy: JSONObject,
    val quotas: JSONObject, val instanceId: String?) {
    val selectable get() = state == "published" && permitted
    val avatarId get() = sharedBotId
    fun canPause(uid: String, generation: Long) = state == "published" && maintainer == TeamMemberRef(uid, generation)
    companion object {
        fun parse(v: JSONObject) = TeamBotCard(v.getString("sharedBotId"), v.getString("name"), v.getString("description"),
            TeamMemberRef.parse(v.getJSONObject("maintainer")), v.getLong("revision"), v.getString("state"), v.getBoolean("permitted"),
            v.getBoolean("ready"), v.getLong("onlineUntil"), v.getJSONObject("usePolicy"), v.getJSONObject("quotas"), v.stringOrNull("instanceId"))
    }
}
internal data class TeamTask(val taskId: String, val conversationId: String, val sharedBotId: String, val requester: TeamMemberRef,
    val state: String, val createdAt: Long, val deadline: Long, val replyId: String?, val artifactId: String?) {
    val terminal get() = state !in setOf("queued", "running")
    companion object { fun parse(v: JSONObject) = TeamTask(v.getString("taskId"), v.getString("conversationId"), v.getString("sharedBotId"),
        TeamMemberRef.parse(v.getJSONObject("requester")), v.getString("state"), v.getLong("createdAt"), v.getLong("deadline"), v.stringOrNull("replyId"), v.stringOrNull("artifactId")) }
}
internal data class TeamArtifact(val artifactId: String, val revision: Long, val digest: String, val title: String, val body: String) {
    companion object { fun parse(v: JSONObject) = TeamArtifact(v.getString("artifactId"), v.getLong("revision"), v.getString("digest"), v.getJSONObject("document").getString("title"), v.getJSONObject("document").getString("body")) }
}
internal fun staleTeamBotSelections(selected: Collection<String>, bots: List<TeamBotCard>, participants: Collection<String>? = null): Set<String> =
    selected.filterTo(mutableSetOf()) { id -> bots.none { it.sharedBotId == id && it.selectable } || (participants != null && "shared_bot:$id" !in participants) }
internal fun requireTeamBotSelections(selected: Collection<String>, bots: List<TeamBotCard>, participants: Collection<String>? = null) {
    if (selected.size > 8 || staleTeamBotSelections(selected, bots, participants).isNotEmpty()) throw TeamMessageFailure("BOT_SELECTION_STALE")
}

/** Strict execution-2 member envelope. The transport never adapts retired commands. */
internal object TeamExecutionContract {
    fun validate(v: JSONObject) {
        fun fail(): Nothing = throw TeamMessageFailure("INVALID_INPUT")
        fun JSONObject.keysSet() = keys().asSequence().toSet()
        fun id(value: Any?) = value is String && Regex("^[A-Za-z0-9_-]{8,64}$").matches(value)
        fun integer(value: Any?, minimum: Long): Boolean = value is Number && value.toDouble().let { it.isFinite() && it == Math.rint(it) && it >= minimum && it <= 9007199254740991.0 }
        if (v.keysSet() != setOf("schemaVersion", "commandId", "teamId", "generation", "action", "payload") ||
            !integer(v.opt("schemaVersion"), 1) || (v.opt("schemaVersion") as Number).toLong() != 1L || !integer(v.opt("generation"), 1) ||
            !id(v.opt("commandId")) || !id(v.opt("teamId"))) fail()
        val p = v.optJSONObject("payload") ?: fail()
        fun shape(required: Set<String>, optional: Set<String> = emptySet()) { if (!p.keysSet().containsAll(required) || !((required + optional).containsAll(p.keysSet()))) fail() }
        fun requireId(key: String) { if (!id(p.opt(key))) fail() }
        fun ids(key: String, minimum: Int = 1) {
            val a = p.optJSONArray(key) ?: fail(); val values = (0 until a.length()).map { a.get(it) }
            if (values.size !in minimum..8 || values.distinct().size != values.size || values.any { !id(it) }) fail()
        }
        when (v.opt("action")) {
            "listSharedBots" -> shape(emptySet())
            "getTask" -> { shape(setOf("taskId")); requireId("taskId") }
            "readArtifact" -> { shape(setOf("artifactId")); requireId("artifactId") }
            "listTasks" -> { shape(emptySet(), setOf("conversationId", "after")); p.keysSet().forEach(::requireId) }
            "pauseSharedBot" -> { shape(setOf("sharedBotId", "expectedRevision")); requireId("sharedBotId"); if (!integer(p.opt("expectedRevision"), 1)) fail() }
            "setConversationPolicy" -> { shape(setOf("conversationId", "expectedRevision", "targets")); requireId("conversationId"); ids("targets", 0); if (!integer(p.opt("expectedRevision"), 0)) fail() }
            "submit" -> {
                shape(setOf("conversationId", "messageId", "changeId", "text", "targets"), setOf("replyTo"))
                listOf("conversationId", "messageId", "changeId").forEach(::requireId); ids("targets")
                if (p.has("replyTo")) requireId("replyTo")
                val text = p.opt("text") as? String ?: fail(); if (text.isEmpty() || text.toByteArray(Charsets.UTF_8).size > 8192) fail()
            }
            "createConversation" -> {
                shape(setOf("conversationId", "kind", "title", "participantUids", "sharedBotIds")); requireId("conversationId"); ids("sharedBotIds")
                if (p.opt("kind") !in setOf("bot_dm", "group", "channel")) fail()
                val title = (p.opt("title") as? String)?.trim() ?: fail(); if (title.isEmpty() || title.length > 120) fail()
                val raw = p.optJSONArray("participantUids") ?: fail(); val uids = (0 until raw.length()).map { raw.get(it) }
                if (uids.size !in 1..200 || uids.distinct().size != uids.size || uids.any { it !is String || !it.matches(Regex("^[A-Za-z0-9_-]{1,128}$")) }) fail()
            }
            else -> fail()
        }
    }
}
