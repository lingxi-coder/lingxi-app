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
/** What a Bot run cost, as the host reports it: `missing` carries no figures; the cost only when the host could price it (`costMicros` is millionths of `currency`). */
internal data class TeamTaskUsage(val status: String, val inputTokens: Long = 0, val outputTokens: Long = 0, val costMicros: Double? = null, val currency: String? = null) {
    companion object { fun parse(v: JSONObject) = TeamTaskUsage(v.getString("status"), v.optLong("inputTokens"), v.optLong("outputTokens"), if (v.has("costMicros") && !v.isNull("costMicros")) v.getDouble("costMicros") else null, v.stringOrNull("currency")) }
}
/** The answer a waiting task is blocked on; present only while `state` is `waiting`. `messageId` is the Bot's question. */
internal data class TeamTaskWait(val waitId: String, val kind: String, val recipient: TeamMemberRef, val expiresAt: Long, val messageId: String) {
    companion object { fun parse(v: JSONObject) = TeamTaskWait(v.getString("waitId"), v.getString("kind"), TeamMemberRef.parse(v.getJSONObject("recipient")), v.getLong("expiresAt"), v.getString("messageId")) }
}
/** `deadline` ends the current phase: the pick-up limit while queued, the running budget while running, the wait's expiry while waiting. */
internal data class TeamTask(val taskId: String, val conversationId: String, val sharedBotId: String, val requester: TeamMemberRef,
    val state: String, val createdAt: Long, val deadline: Long, val replyId: String?, val artifactId: String?, val usage: TeamTaskUsage? = null,
    val cancelRequestedAt: Long? = null, val wait: TeamTaskWait? = null) {
    val terminal get() = state !in setOf("queued", "running", "waiting")
    /** A worker has it or is about to: its state changes within seconds. A waiting task changes only with its answer or its expiry. */
    val busy get() = state == "queued" || state == "running"
    /** Cancellation was asked while the Bot was working; the task ends at the worker's next check. */
    val cancelling get() = state == "running" && cancelRequestedAt != null
    companion object { fun parse(v: JSONObject) = TeamTask(v.getString("taskId"), v.getString("conversationId"), v.getString("sharedBotId"),
        TeamMemberRef.parse(v.getJSONObject("requester")), v.getString("state"), v.getLong("createdAt"), v.getLong("deadline"), v.stringOrNull("replyId"), v.stringOrNull("artifactId"), v.optJSONObject("usage")?.let(TeamTaskUsage::parse),
        if (v.has("cancelRequestedAt") && !v.isNull("cancelRequestedAt")) v.getLong("cancelRequestedAt") else null, v.optJSONObject("wait")?.let(TeamTaskWait::parse)) }
}
/** What the viewer may do with a task (Desktop `BotRun`). */
internal object TeamTaskActions {
    enum class ExpiryUnit { MINUTES, HOURS, DAYS }
    /** The requester and the Bot's maintainer may stop a task that has not ended. */
    fun canCancel(task: TeamTask, viewer: TeamMemberRef?, maintainer: TeamMemberRef?): Boolean =
        viewer != null && !task.terminal && !task.cancelling && (task.requester == viewer || maintainer == viewer)
    /** The wait an answer from the viewer would resolve: the task waits for exactly this viewer and no answer to it is already on its way. */
    fun answerableWait(task: TeamTask?, viewer: TeamMemberRef?, pendingWaitIds: Set<String>): String? {
        val wait = task?.wait ?: return null
        return if (task.state == "waiting" && viewer != null && wait.recipient == viewer && wait.waitId !in pendingWaitIds) wait.waitId else null
    }
    /** The wait a reply to [message] would answer: [message] is a Bot's question whose task still waits for the viewer. */
    fun answerableWait(message: TeamMessage?, tasks: List<TeamTask>, viewer: TeamMemberRef?, pendingWaitIds: Set<String>): String? {
        val waitId = message?.waitId ?: return null
        if (message.authorKind == "user" || message.taskId == null) return null
        return answerableWait(tasks.firstOrNull { it.taskId == message.taskId }, viewer, pendingWaitIds).takeIf { it == waitId }
    }
    /** The waits whose answers are saved on this device and not yet confirmed. */
    fun pendingWaitIds(outbox: List<JSONObject>): Set<String> =
        outbox.mapNotNull { it.optJSONObject("payload")?.optString("waitId")?.takeIf(String::isNotEmpty) }.toSet()
    /** How long until [at] (both in milliseconds), rounded up to whole minutes, hours (under 48) or days; never less than one minute. */
    fun expiry(at: Long, now: Long): Pair<Int, ExpiryUnit> {
        val minutes = maxOf(1L, Math.ceil((at - now) / 60_000.0).toLong())
        if (minutes < 60) return minutes.toInt() to ExpiryUnit.MINUTES
        val hours = (minutes + 59) / 60
        return if (hours < 48) hours.toInt() to ExpiryUnit.HOURS else ((hours + 23) / 24).toInt() to ExpiryUnit.DAYS
    }
}
internal data class TeamArtifact(val artifactId: String, val revision: Long, val digest: String, val title: String, val body: String) {
    companion object { fun parse(v: JSONObject) = TeamArtifact(v.getString("artifactId"), v.getLong("revision"), v.getString("digest"), v.getJSONObject("document").getString("title"), v.getJSONObject("document").getString("body")) }
}
internal fun staleTeamBotSelections(selected: Collection<String>, bots: List<TeamBotCard>, participants: Collection<String>? = null): Set<String> =
    selected.filterTo(mutableSetOf()) { id -> bots.none { it.sharedBotId == id && it.selectable } || (participants != null && "shared_bot:$id" !in participants) }
/** Composer rule: a chosen Bot is stale when it is not selectable or not a participant (`shared_bot:<id>`) of the conversation. */
internal fun composerStaleTargets(targets: Collection<String>, bots: List<TeamBotCard>, conversation: TeamMessageConversation): Set<String> =
    staleTeamBotSelections(targets, bots, conversation.participants)
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
            "cancelTask" -> { shape(setOf("taskId")); requireId("taskId") }
            "answerWait" -> {
                shape(setOf("conversationId", "waitId", "messageId", "changeId", "text"))
                listOf("conversationId", "waitId", "messageId", "changeId").forEach(::requireId)
                val text = p.opt("text") as? String ?: fail(); if (text.isEmpty() || text.toByteArray(Charsets.UTF_8).size > 8192) fail()
            }
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
