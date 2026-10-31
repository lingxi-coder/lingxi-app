package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest

internal data class TeamMessageConversation(val id: String, val kind: String, val title: String, val participants: List<String>, val lastSeq: Long, val lastChangeSeq: Long, val lastReadSeq: Long, val botKeys: List<String> = emptyList()) {
    fun merging(old: TeamMessageConversation?) = copy(lastSeq = maxOf(old?.lastSeq ?: 0, lastSeq), lastChangeSeq = maxOf(old?.lastChangeSeq ?: 0, lastChangeSeq), lastReadSeq = maxOf(old?.lastReadSeq ?: 0, lastReadSeq))
    val unread get() = (lastSeq - lastReadSeq).coerceAtLeast(0)
    companion object { fun parse(v: JSONObject) = TeamMessageConversation(v.getString("id"), v.getString("kind"), v.getString("title"), v.getJSONArray("participants").strings(), v.getLong("lastSeq"), v.getLong("lastChangeSeq"), v.getLong("lastReadSeq"), v.optJSONArray("botKeys")?.strings() ?: v.getJSONArray("participants").strings().filter { it.startsWith("bot:") }) }
}
internal data class TeamMessage(val id: String, val seq: Long, val authorUid: String, val authorKind: String, val text: String, val changeSeq: Long, val changeId: String, val recalled: Boolean, val replyTo: String?, val editedAt: String?, val kind: String = "text", val botKey: String? = null, val triggerId: String? = null, val originUid: String? = null, val originGeneration: Long? = null, val question: TeamBotQuestion? = null, val targets: List<String> = emptyList()) {
    companion object { fun parse(v: JSONObject) = TeamMessage(v.getString("id"), v.getLong("seq"), v.getJSONObject("author").getString("uid"), v.getJSONObject("author").getString("kind"), v.getString("text"), v.getLong("changeSeq"), v.getString("changeId"), v.optBoolean("recalled"), v.stringOrNull("replyTo"), v.stringOrNull("editedAt"), v.optString("kind", "text"), v.optJSONObject("author")?.stringOrNull("botKey") ?: v.stringOrNull("botKey"), v.stringOrNull("triggerId"), v.optJSONObject("origin")?.stringOrNull("uid"), v.optJSONObject("origin")?.getLong("generation"), v.optJSONObject("question")?.let(TeamBotQuestion::parse), v.optJSONArray("targets")?.strings() ?: v.optJSONArray("mentions")?.strings()?.filter { it.startsWith("bot:") } ?: emptyList()) }
}
internal data class TeamMessageThread(val messages: List<TeamMessage> = emptyList())
internal data class TeamMessageCache(val uid: String, val teamId: String, val generation: Long,
    val conversations: List<TeamMessageConversation> = emptyList(), val threads: Map<String, TeamMessageThread> = emptyMap(), val outbox: List<JSONObject> = emptyList(), val creation: JSONObject? = null, val answers: List<JSONObject> = emptyList(), val settlements: List<JSONObject> = emptyList()) {
    fun json(): JSONObject {
        return JSONObject().put("version", 1).put("uid", uid).put("teamId", teamId).put("generation", generation).put("outbox", JSONArray(outbox)).put("creation", creation).put("answers", JSONArray(answers)).put("settlements", JSONArray(settlements))
    }
    companion object {
        fun parse(v: JSONObject): TeamMessageCache {
            require(v.getInt("version") == 1)
            return TeamMessageCache(v.getString("uid"), v.getString("teamId"), v.getLong("generation"), outbox = v.getJSONArray("outbox").objects(), creation = v.optJSONObject("creation"), answers = v.optJSONArray("answers")?.objects() ?: emptyList(), settlements = v.optJSONArray("settlements")?.objects() ?: emptyList())
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
        val value = source?.let(TeamMessageCache::parse) ?: TeamMessageCache(uid, teamId, generation)
        if (source?.has("conversations") == true || source?.has("threads") == true) write(file, value)
        check(value.uid == uid && value.teamId == teamId && value.generation == generation)
        value
    }
    fun update(uid: String, teamId: String, generation: Long, reduce: (TeamMessageCache) -> TeamMessageCache): TeamMessageCache = synchronized(lock) {
        val file = file(uid, teamId, generation)
        val old = load(uid, teamId, generation)
        val next = reduce(old)
        if (next === old || next == old) return@synchronized old
        write(file, next)
        next
    }
    private fun write(file: File, next: TeamMessageCache) {
        val scratch = File.createTempFile(file.nameWithoutExtension, ".tmp", directory)
        try {
            FileOutputStream(scratch).use { stream -> stream.write(next.json().toString().toByteArray(Charsets.UTF_8)); stream.fd.sync() }
            Files.move(scratch.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } finally { if (scratch.exists()) scratch.delete() }
    }
    companion object { private val lock = Any() }
}
