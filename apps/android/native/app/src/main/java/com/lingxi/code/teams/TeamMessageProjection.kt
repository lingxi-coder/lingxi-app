package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest

internal data class TeamMessageConversation(val id: String, val kind: String, val title: String, val participants: List<String>, val lastSeq: Long, val lastChangeSeq: Long, val lastReadSeq: Long) {
    fun merging(old: TeamMessageConversation?) = copy(lastSeq = maxOf(old?.lastSeq ?: 0, lastSeq), lastChangeSeq = maxOf(old?.lastChangeSeq ?: 0, lastChangeSeq), lastReadSeq = maxOf(old?.lastReadSeq ?: 0, lastReadSeq))
    val unread get() = (lastSeq - lastReadSeq).coerceAtLeast(0)
    fun json() = JSONObject().put("id", id).put("kind", kind).put("title", title).put("participants", JSONArray(participants)).put("lastSeq", lastSeq).put("lastChangeSeq", lastChangeSeq).put("lastReadSeq", lastReadSeq)
    companion object { fun parse(v: JSONObject) = TeamMessageConversation(v.getString("id"), v.getString("kind"), v.getString("title"), v.getJSONArray("participants").strings(), v.getLong("lastSeq"), v.getLong("lastChangeSeq"), v.getLong("lastReadSeq")) }
}
internal data class TeamMessage(val id: String, val seq: Long, val authorUid: String, val authorKind: String, val text: String, val changeSeq: Long, val changeId: String, val recalled: Boolean, val replyTo: String?, val editedAt: String?) {
    fun json() = JSONObject().put("id", id).put("seq", seq).put("author", JSONObject().put("uid", authorUid).put("kind", authorKind)).put("text", text).put("changeSeq", changeSeq).put("changeId", changeId).put("recalled", recalled).put("replyTo", replyTo).put("editedAt", editedAt)
    companion object { fun parse(v: JSONObject) = TeamMessage(v.getString("id"), v.getLong("seq"), v.getJSONObject("author").getString("uid"), v.getJSONObject("author").getString("kind"), v.getString("text"), v.getLong("changeSeq"), v.getString("changeId"), v.optBoolean("recalled"), v.stringOrNull("replyTo"), v.stringOrNull("editedAt")) }
}
internal data class TeamMessageThread(val cursor: Long = 0, val messages: List<TeamMessage> = emptyList())
internal data class TeamMessageCache(val uid: String, val teamId: String, val generation: Long,
    val conversations: List<TeamMessageConversation> = emptyList(), val threads: Map<String, TeamMessageThread> = emptyMap(), val outbox: List<JSONObject> = emptyList(), val creation: JSONObject? = null) {
    fun apply(page: JSONObject): TeamMessageCache {
        val conversation = TeamMessageConversation.parse(page.getJSONObject("conversation"))
        val before = threads[conversation.id] ?: TeamMessageThread()
        var cursor = before.cursor
        val messages = before.messages.associateBy { it.id }.toMutableMap()
        val incoming = page.getJSONArray("messages").objects().map(TeamMessage::parse).associateBy { it.id }
        for (change in page.getJSONArray("changes").objects()) {
            val seq = change.getLong("seq")
            if (seq <= cursor) continue
            val message = incoming[change.getString("messageId")]
            if (seq != cursor + 1 || message == null || message.changeSeq < seq) throw TeamMessageFailure("CHANGE_GAP")
            if ((messages[message.id]?.changeSeq ?: -1) < message.changeSeq) messages[message.id] = message
            cursor = seq
        }
        val current = conversation.merging(conversations.find { it.id == conversation.id })
        return copy(conversations = conversations.filter { it.id != conversation.id } + current, threads = threads + (conversation.id to TeamMessageThread(cursor, messages.values.sortedBy { it.seq })))
    }
    fun json(): JSONObject {
        val values = JSONObject()
        threads.forEach { (id, thread) -> values.put(id, JSONObject().put("cursor", thread.cursor).put("messages", JSONArray(thread.messages.map { it.json() }))) }
        return JSONObject().put("version", 1).put("uid", uid).put("teamId", teamId).put("generation", generation).put("conversations", JSONArray(conversations.map { it.json() })).put("threads", values).put("outbox", JSONArray(outbox)).put("creation", creation)
    }
    companion object {
        fun parse(v: JSONObject): TeamMessageCache {
            require(v.getInt("version") == 1)
            val threads = v.getJSONObject("threads")
            return TeamMessageCache(v.getString("uid"), v.getString("teamId"), v.getLong("generation"), v.getJSONArray("conversations").objects().map(TeamMessageConversation::parse), threads.keys().asSequence().associateWith { id -> val thread = threads.getJSONObject(id); TeamMessageThread(thread.getLong("cursor"), thread.getJSONArray("messages").objects().map(TeamMessage::parse)) }, v.getJSONArray("outbox").objects(), v.optJSONObject("creation"))
        }
    }
}
internal class TeamMessageFailure(val code: String) : Exception(code)
internal fun JSONArray.objects() = (0 until length()).map { getJSONObject(it) }
internal fun JSONArray.strings() = (0 until length()).map { getString(it) }
internal fun JSONObject.stringOrNull(key: String) = if (isNull(key)) null else getString(key)

/** Atomic replacement keeps the projection and cursor together. A failed write
 * never acknowledges a pending intent. App-private, excluded from Android backup. */
internal class TeamMessageStorage(private val directory: File) {
    fun update(uid: String, teamId: String, generation: Long, reduce: (TeamMessageCache) -> TeamMessageCache): TeamMessageCache = synchronized(lock) {
        check(directory.isDirectory || directory.mkdirs())
        val key = JSONArray(listOf(uid, teamId, generation.toString())).toString()
        val hash = MessageDigest.getInstance("SHA-256").digest(key.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }
        val file = File(directory, "$hash.json")
        val old = if (file.exists()) TeamMessageCache.parse(JSONObject(file.readText())) else TeamMessageCache(uid, teamId, generation)
        check(old.uid == uid && old.teamId == teamId && old.generation == generation)
        val next = reduce(old)
        val scratch = File.createTempFile(hash, ".tmp", directory)
        try {
            FileOutputStream(scratch).use { stream -> stream.write(next.json().toString().toByteArray(Charsets.UTF_8)); stream.fd.sync() }
            Files.move(scratch.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } finally { if (scratch.exists()) scratch.delete() }
        next
    }
    companion object { private val lock = Any() }
}
