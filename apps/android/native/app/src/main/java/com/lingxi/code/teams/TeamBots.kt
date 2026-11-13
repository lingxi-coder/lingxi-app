package com.lingxi.code.teams

import org.json.JSONObject

internal data class TeamMemberRef(val uid: String, val generation: Long) {
    companion object { fun parse(v: JSONObject) = TeamMemberRef(v.getString("uid"), v.getLong("generation")) }
}
internal data class TeamBotCard(val sharedBotId: String, val name: String, val description: String, val maintainer: TeamMemberRef,
    val revision: Long, val state: String, val permitted: Boolean, val ready: Boolean, val onlineUntil: Long, val usePolicy: JSONObject,
    val quotas: JSONObject, val instanceId: String?, val sharedList: TeamBotListInfo? = null) {
    val selectable get() = state == "published" && permitted
    val avatarId get() = sharedBotId
    fun canPause(uid: String, generation: Long) = state == "published" && maintainer == TeamMemberRef(uid, generation)
    companion object {
        fun parse(v: JSONObject) = TeamBotCard(v.getString("sharedBotId"), v.getString("name"), v.getString("description"),
            TeamMemberRef.parse(v.getJSONObject("maintainer")), v.getLong("revision"), v.getString("state"), v.getBoolean("permitted"),
            v.getBoolean("ready"), v.getLong("onlineUntil"), v.getJSONObject("usePolicy"), v.getJSONObject("quotas"), v.stringOrNull("instanceId"),
            v.optJSONObject("sharedList")?.let(TeamBotListInfo::parse))
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
/** The identity of one decision on a request. Made once and sent again unchanged on a retry, so a lost answer cannot post the reply twice. */
internal data class TeamDecisionIds(val commandId: String = java.util.UUID.randomUUID().toString(), val messageId: String = java.util.UUID.randomUUID().toString(),
    val changeId: String = java.util.UUID.randomUUID().toString())
/** Where a Bot's request to change its list stands for the viewer (Desktop `ApprovalCard`). */
internal sealed interface TeamApprovalState {
    /** The task waits for the viewer's decision. */
    object Decide : TeamApprovalState
    /** The task waits for this member's decision. */
    data class Waiting(val uid: String) : TeamApprovalState
    data class Approved(val uid: String) : TeamApprovalState
    data class Declined(val uid: String) : TeamApprovalState
    /** It ended without a decision: it expired, or its task was cancelled or lost its authorization. */
    object Closed : TeamApprovalState
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
        // A proposed change is approved or declined on its own card, never answered in words.
        return if (task.state == "waiting" && wait.kind == "human_input" && viewer != null && wait.recipient == viewer && wait.waitId !in pendingWaitIds) wait.waitId else null
    }
    /** Where the request [waitId] stands: open while its task waits on it, then decided by the reply that carries the decision, else closed. */
    fun approvalState(waitId: String, task: TeamTask?, viewer: TeamMemberRef?, messages: List<TeamMessage>): TeamApprovalState {
        val wait = task?.wait
        if (task?.state == "waiting" && wait != null && wait.waitId == waitId && wait.kind == "tool_approval")
            return if (viewer != null && wait.recipient == viewer) TeamApprovalState.Decide else TeamApprovalState.Waiting(wait.recipient.uid)
        val reply = messages.firstOrNull { it.waitId == waitId && it.authorKind == "user" && it.decision != null } ?: return TeamApprovalState.Closed
        return if (reply.decision == true) TeamApprovalState.Approved(reply.authorUid) else TeamApprovalState.Declined(reply.authorUid)
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
/** One revision of a shared document. [currentRevision] is the document's newest revision; a revision below it is read-only history. */
internal data class TeamArtifact(val artifactId: String, val revision: Long, val digest: String, val title: String, val body: String, val currentRevision: Long) {
    val old get() = revision < currentRevision
    companion object {
        fun parse(v: JSONObject) = TeamArtifact(v.getString("artifactId"), v.getLong("revision"), v.getString("digest"), v.getJSONObject("document").getString("title"),
            v.getJSONObject("document").getString("body"), v.getLong("currentRevision"))
    }
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
    /** The largest shared document body the service stores, in UTF-8 bytes. */
    const val DOCUMENT_BYTES = 256 * 1024
    /** The longest text of one shared-list item, in UTF-8 bytes, and the most changes one write carries. */
    const val LIST_ITEM_BYTES = 500
    const val LIST_CHANGES = 20
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
        fun ids(key: String, minimum: Int = 1, maximum: Int = 8) {
            val a = p.optJSONArray(key) ?: fail(); val values = (0 until a.length()).map { a.get(it) }
            if (values.size !in minimum..maximum || values.distinct().size != values.size || values.any { !id(it) }) fail()
        }
        val lastPart = (TeamAttachments.FILE_BYTES - 1) / TeamAttachments.PART_BYTES
        fun partIndex(value: Any?) = integer(value, 0) && (value as Number).toLong() <= lastPart
        when (v.opt("action")) {
            "listSharedBots" -> shape(emptySet())
            "getTask" -> { shape(setOf("taskId")); requireId("taskId") }
            "readArtifact" -> { shape(setOf("artifactId"), setOf("revision")); requireId("artifactId"); if (p.has("revision") && !integer(p.opt("revision"), 1)) fail() }
            "listArtifactRevisions" -> { shape(setOf("artifactId")); requireId("artifactId") }
            "reviseArtifact" -> {
                shape(setOf("artifactId", "expectedRevision", "operationId", "document")); requireId("artifactId"); requireId("operationId")
                if (!integer(p.opt("expectedRevision"), 1)) fail()
                val document = p.optJSONObject("document") ?: fail(); if (document.keysSet() != setOf("title", "body")) fail()
                val title = (document.opt("title") as? String ?: fail()).trim(); val body = document.opt("body") as? String ?: fail()
                if (title.isEmpty() || title.length > 120 || body.isEmpty() || body.toByteArray(Charsets.UTF_8).size > DOCUMENT_BYTES) fail()
            }
            "listOperations" -> { shape(setOf("taskId")); requireId("taskId") }
            "readList" -> { shape(setOf("sharedBotId")); requireId("sharedBotId") }
            "writeList" -> {
                shape(setOf("sharedBotId", "operationId", "expectedDataRevision", "changes")); requireId("sharedBotId"); requireId("operationId")
                val changes = p.optJSONArray("changes") ?: fail()
                if (!integer(p.opt("expectedDataRevision"), 0) || changes.length() !in 1..LIST_CHANGES) fail()
                fun text(value: Any?) = value is String && value.trim().isNotEmpty() && value.toByteArray(Charsets.UTF_8).size <= LIST_ITEM_BYTES && !value.contains('\u0000')
                for (index in 0 until changes.length()) {
                    val change = changes.optJSONObject(index) ?: fail(); val keys = change.keysSet()
                    val valid = when (change.opt("op")) {
                        "add" -> keys == setOf("op", "text") && text(change.opt("text"))
                        "update" -> setOf("op", "itemId", "text", "done").containsAll(keys) && id(change.opt("itemId")) && (change.has("text") || change.has("done")) &&
                            (!change.has("text") || text(change.opt("text"))) && (!change.has("done") || change.opt("done") is Boolean)
                        "remove" -> keys == setOf("op", "itemId") && id(change.opt("itemId"))
                        else -> false
                    }
                    if (!valid) fail()
                }
            }
            "resolveOperation" -> {
                shape(setOf("taskId", "operationId", "resolution")); requireId("taskId")
                val operation = p.opt("operationId"); if (operation !is String || !Regex("^[a-f0-9]{64}$").matches(operation)) fail()
                if (p.opt("resolution") !in setOf("succeeded", "failed")) fail()
            }
            "cancelTask" -> { shape(setOf("taskId")); requireId("taskId") }
            "answerWait" -> {
                shape(setOf("conversationId", "waitId", "messageId", "changeId", "text"))
                listOf("conversationId", "waitId", "messageId", "changeId").forEach(::requireId)
                val text = p.opt("text") as? String ?: fail(); if (text.isEmpty() || text.toByteArray(Charsets.UTF_8).size > 8192) fail()
            }
            "decideWait" -> {
                shape(setOf("conversationId", "waitId", "messageId", "changeId", "decision"))
                listOf("conversationId", "waitId", "messageId", "changeId").forEach(::requireId)
                if ((p.opt("decision") as? String) !in setOf("approve", "deny")) fail()
            }
            "listTasks" -> { shape(emptySet(), setOf("conversationId", "after")); p.keysSet().forEach(::requireId) }
            "pauseSharedBot" -> { shape(setOf("sharedBotId", "expectedRevision")); requireId("sharedBotId"); if (!integer(p.opt("expectedRevision"), 1)) fail() }
            "setConversationPolicy" -> { shape(setOf("conversationId", "expectedRevision", "targets")); requireId("conversationId"); ids("targets", 0); if (!integer(p.opt("expectedRevision"), 0)) fail() }
            "submit" -> {
                // A message that asks Bots, carries files, or both.
                shape(setOf("conversationId", "messageId", "changeId", "text", "targets"), setOf("replyTo", "attachments"))
                listOf("conversationId", "messageId", "changeId").forEach(::requireId)
                val files = p.has("attachments")
                ids("targets", if (files) 0 else 1)
                if (files) ids("attachments", maximum = TeamAttachments.MESSAGE_FILES)
                if (p.has("replyTo")) requireId("replyTo")
                val text = p.opt("text") as? String ?: fail(); if ((text.isEmpty() && !files) || text.toByteArray(Charsets.UTF_8).size > 8192) fail()
            }
            "createUpload" -> {
                shape(setOf("conversationId", "uploadId", "name", "mime", "byteLength", "sha256")); requireId("conversationId"); requireId("uploadId")
                val name = p.opt("name") as? String ?: fail(); val digest = p.opt("sha256") as? String ?: fail()
                if (name != TeamAttachments.name(name) || p.opt("mime") !in TeamAttachments.TYPES || !integer(p.opt("byteLength"), 1) ||
                    (p.opt("byteLength") as Number).toLong() > TeamAttachments.FILE_BYTES || !Regex("^[a-f0-9]{64}$").matches(digest)) fail()
            }
            "putUploadPart" -> {
                shape(setOf("uploadId", "index", "data")); requireId("uploadId")
                val data = p.opt("data") as? String ?: fail()
                if (!partIndex(p.opt("index")) || data.length !in 4..4 * ((TeamAttachments.PART_BYTES + 2) / 3) || !Regex("^[A-Za-z0-9+/]+={0,2}$").matches(data)) fail()
            }
            "finalizeUpload", "cancelUpload" -> { shape(setOf("uploadId")); requireId("uploadId") }
            "readAttachmentPart" -> { shape(setOf("attachmentId", "index")); requireId("attachmentId"); if (!partIndex(p.opt("index"))) fail() }
            "revokeAttachment" -> { shape(setOf("attachmentId", "changeId")); requireId("attachmentId"); requireId("changeId") }
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
