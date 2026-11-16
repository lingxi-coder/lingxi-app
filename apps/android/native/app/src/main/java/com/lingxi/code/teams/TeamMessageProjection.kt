package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest

/**
 * The conversation's newest message as the backend writes it onto the conversation document
 * (`TeamsPointerParser`, SPEC-mobile-team-bot-ui section 8.4). [atMs] is null while the server has not stamped it yet.
 */
internal data class TeamLastMessage(val id: String, val seq: Long, val atMs: Long?, val authorKind: String, val authorUid: String, val authorSharedBotId: String?, val preview: String, val recalled: Boolean = false)

internal data class TeamMessageConversation(val id: String, val kind: String, val title: String, val participants: List<String>, val lastSeq: Long, val lastChangeSeq: Long, val lastReadSeq: Long, val sharedBotIds: List<String> = emptyList(), val lastMessage: TeamLastMessage? = null,
    /** Who can read it, as written when it was made: its people and the maintainers its Bots had then. Null when the source does not say. */
    val readers: List<String>? = null) {
    fun merging(old: TeamMessageConversation?) = copy(lastSeq = maxOf(old?.lastSeq ?: 0, lastSeq), lastChangeSeq = maxOf(old?.lastChangeSeq ?: 0, lastChangeSeq), lastReadSeq = maxOf(old?.lastReadSeq ?: 0, lastReadSeq))
    val unread get() = (lastSeq - lastReadSeq).coerceAtLeast(0)
    /** The raw SharedBot ID carried by the Chat's explicit `shared_bot:` participant. */
    val sharedBotId: String? get() = sharedBotIds.firstOrNull()
    companion object {
        /**
         * One conversation document, read strictly: any field that breaks the stored contract is `INVALID_DATA`
         * and the caller stops the team's listeners (a half-parsed list is never shown). [millis] converts a
         * Firestore timestamp to epoch milliseconds, or returns null for anything that is not one.
         */
        fun fromDocument(id: String, data: Map<String, Any?>, lastReadSeq: Long, millis: (Any) -> Long?): TeamMessageConversation {
            val participants = (data["participants"] as? List<*>)?.map { it as? String ?: throw TeamMessageFailure("INVALID_DATA") } ?: throw TeamMessageFailure("INVALID_DATA")
            fun long(key: String) = (data[key] as? Number)?.toLong() ?: throw TeamMessageFailure("INVALID_DATA")
            return TeamMessageConversation(id, data["kind"] as? String ?: throw TeamMessageFailure("INVALID_DATA"), data["title"] as? String ?: throw TeamMessageFailure("INVALID_DATA"), participants,
                long("lastSeq"), long("lastChangeSeq"), lastReadSeq, participants.filter { it.startsWith("shared_bot:") }.map { it.removePrefix("shared_bot:") }, TeamsPointerParser.parseLastMessage(data.containsKey("lastMessage"), data["lastMessage"], millis),
                (data["memberUids"] as? List<*>)?.takeIf { uids -> uids.all { it is String } }?.map { it as String })
        }
    }
}
/** One change a Bot asks to make to its list. [item] is the item as it read when the Bot asked: null for an added item, and for one that was already gone. */
internal data class TeamListApprovalChange(val op: String, val text: String? = null, val done: Boolean? = null, val item: String? = null)
/** A Bot's request to change its list: the list and exactly the changes the approval is for. */
internal data class TeamListApproval(val title: String, val changes: List<TeamListApprovalChange>) {
    companion object {
        /** Never throws: one unreadable request must not hide the conversation it is in. It is then not shown as a request. */
        fun parse(value: Any?): TeamListApproval? {
            val request = value as? JSONObject ?: return null
            val title = request.opt("title") as? String ?: return null
            val list = request.optJSONArray("changes")?.takeIf { it.length() > 0 } ?: return null
            val changes = (0 until list.length()).map { index ->
                val change = list.optJSONObject(index) ?: return null
                val op = change.opt("op") as? String
                val text = change.opt("text").takeIf { change.has("text") && !change.isNull("text") }
                val done = change.opt("done").takeIf { change.has("done") && !change.isNull("done") }
                if ((text != null && text !is String) || (done != null && done !is Boolean)) return null
                when (op) {
                    "add" -> if (text == null) return null
                    "update" -> if (text == null && done == null) return null
                    "remove" -> Unit
                    else -> return null
                }
                TeamListApprovalChange(op!!, text as String?, done as Boolean?, if (op == "add") null else change.opt("item") as? String)
            }
            return TeamListApproval(title, changes)
        }
    }
}
internal data class TeamMessage(val id: String, val seq: Long, val authorUid: String, val authorKind: String, val text: String,
    val changeSeq: Long, val changeId: String, val recalled: Boolean, val replyTo: String?, val editedAt: String?, val kind: String = "text",
    val sharedBotId: String? = null, val taskId: String? = null, val artifactId: String? = null, val targets: List<String> = emptyList(), val createdAt: String? = null, val taskIds: List<String> = emptyList(), val authorDisplayName: String? = null,
    /** On a Bot's question and on the answer to it: the wait they belong to. */
    val waitId: String? = null,
    /** On a Bot's request to approve a list change. */
    val approval: TeamListApproval? = null,
    /** On the requester's reply to such a request: approved or declined. */
    val decision: Boolean? = null,
    /** Files the message carries, in the order they were added. */
    val attachments: List<TeamMessageAttachment> = emptyList(),
    /** On a Bot's reply: the files of this conversation its model was given. */
    val readFiles: List<TeamReadFile> = emptyList()) {
    /** The files to show: none on a recalled message. */
    val files: List<TeamMessageAttachment> get() = if (recalled) emptyList() else attachments
    /** The request to show under this message, if it is one a Bot made. */
    val approvalRequest: TeamListApproval? get() = approval.takeIf { authorKind != "user" && !recalled && waitId != null }
    val createdAtMs: Long? get() = createdAt?.let { runCatching { java.time.Instant.parse(it).toEpochMilli() }.getOrNull() }
    companion object { fun parse(v: JSONObject): TeamMessage {
        val author = v.getJSONObject("author"); val authorKind = author.getString("kind")
        if (authorKind !in setOf("user", "shared_bot")) throw TeamMessageFailure("INVALID_DATA")
        return TeamMessage(v.getString("id"), v.getLong("seq"), if (authorKind == "user") author.getString("uid") else "", authorKind,
            v.getString("text"), v.getLong("changeSeq"), v.getString("changeId"), v.optBoolean("recalled"), v.stringOrNull("replyTo"),
            v.stringOrNull("editedAt"), v.getString("kind"), if (authorKind == "shared_bot") author.getString("sharedBotId") else null,
            v.stringOrNull("taskId"), v.stringOrNull("artifactId"), v.optJSONArray("targets")?.strings() ?: emptyList(), v.stringOrNull("createdAt"), v.optJSONArray("taskIds")?.strings() ?: emptyList(), author.stringOrNull("displayName"), v.stringOrNull("waitId"),
            TeamListApproval.parse(v.opt("approval")), when (v.opt("decision")) { "approve" -> true; "deny" -> false; else -> null },
            if (v.has("attachments") && !v.isNull("attachments")) TeamMessageAttachment.list(v.optJSONArray("attachments") ?: throw TeamMessageFailure("INVALID_DATA")) else emptyList(),
            TeamReadFile.list(v.optJSONArray("readFiles")))
    } }
}
internal data class TeamMessageThread(val messages: List<TeamMessage> = emptyList())
internal data class TeamMessageCache(val uid: String, val teamId: String, val generation: Long,
    val conversations: List<TeamMessageConversation> = emptyList(), val threads: Map<String, TeamMessageThread> = emptyMap(), val outbox: List<JSONObject> = emptyList(), val creation: JSONObject? = null, val commands: List<JSONObject> = emptyList()) {
    fun json(): JSONObject {
        return JSONObject().put("version", 1).put("uid", uid).put("teamId", teamId).put("generation", generation).put("outbox", JSONArray(outbox)).put("creation", creation).put("commands", JSONArray(commands))
    }
    companion object {
        fun parse(v: JSONObject): TeamMessageCache {
            require(v.getInt("version") == 1)
            return TeamMessageCache(v.getString("uid"), v.getString("teamId"), v.getLong("generation"), outbox = v.getJSONArray("outbox").objects(), creation = v.optJSONObject("creation"), commands = v.optJSONArray("commands")?.objects() ?: emptyList())
        }
    }
}
internal class TeamMessageFailure(val code: String) : Exception(code)
internal fun JSONArray.objects() = (0 until length()).map { getJSONObject(it) }
internal fun JSONArray.strings() = (0 until length()).map { getString(it) }
internal fun JSONObject.stringOrNull(key: String) = if (isNull(key)) null else getString(key)

/** Atomic, app-private HTTP command journal. Message bodies are cached by Firestore. */
internal class TeamCommandStorage(private val directory: File) {
    private fun file(uid: String, teamId: String, generation: Long): File {
        check(directory.isDirectory || directory.mkdirs())
        val key = JSONArray(listOf(uid, teamId, generation.toString())).toString()
        val hash = MessageDigest.getInstance("SHA-256").digest(key.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }
        return File(directory, "$hash.json")
    }
    fun load(uid: String, teamId: String, generation: Long): TeamMessageCache = synchronized(lock) {
        val file = file(uid, teamId, generation)
        val source = if (file.exists()) JSONObject(file.readText()) else null
        var cleaned = false
        if (source != null) {
            for (key in listOf("answers", "settlements")) if (source.has(key)) { source.remove(key); cleaned = true }
            if ((source.optJSONObject("creation")?.optJSONArray("botKeys")?.length() ?: 0) > 0) { source.remove("creation"); cleaned = true }
            val outbox = source.getJSONArray("outbox").objects()
            val retained = outbox.filter { intent -> intent.optJSONArray("targets")?.strings()?.any { it.startsWith("bot:") } != true }
            if (retained.size != outbox.size) { source.put("outbox", JSONArray(retained)); cleaned = true }
        }
        val value = source?.let(TeamMessageCache::parse) ?: TeamMessageCache(uid, teamId, generation)
        check(value.uid == uid && value.teamId == teamId && value.generation == generation)
        if (cleaned) {
            val scratch = File.createTempFile(file.nameWithoutExtension, ".tmp", directory)
            try {
                FileOutputStream(scratch).use { stream -> stream.write(requireNotNull(source).toString().toByteArray(Charsets.UTF_8)); stream.fd.sync() }
                Files.move(scratch.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
            } finally { if (scratch.exists()) scratch.delete() }
        }
        value
    }
    fun update(uid: String, teamId: String, generation: Long, reduce: (TeamMessageCache) -> TeamMessageCache): TeamMessageCache = synchronized(lock) {
        val file = file(uid, teamId, generation)
        val old = load(uid, teamId, generation)
        val next = reduce(old)
        check(next.uid == uid && next.teamId == teamId && next.generation == generation)
        if (next === old || next == old) return@synchronized old
        write(file, next)
        next
    }
    private fun write(file: File, next: TeamMessageCache) {
        val raw = if (file.exists()) JSONObject(file.readText()) else JSONObject()
        val encoded = next.json()
        for (key in listOf("version", "uid", "teamId", "generation", "outbox", "creation", "commands")) {
            if (encoded.has(key)) raw.put(key, encoded.get(key)) else raw.remove(key)
        }
        val scratch = File.createTempFile(file.nameWithoutExtension, ".tmp", directory)
        try {
            FileOutputStream(scratch).use { stream -> stream.write(raw.toString().toByteArray(Charsets.UTF_8)); stream.fd.sync() }
            Files.move(scratch.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } finally { if (scratch.exists()) scratch.delete() }
    }
    companion object { private val lock = Any() }
}
