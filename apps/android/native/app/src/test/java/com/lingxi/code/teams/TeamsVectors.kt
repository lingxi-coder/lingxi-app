package com.lingxi.code.teams

import com.lingxi.code.tabs.SharedVectors
import com.lingxi.code.tabs.objects
import com.lingxi.code.tabs.stringOrNull
import org.json.JSONArray
import org.json.JSONObject
import java.time.Instant

/** Builds the app's own types from the neutral shape of `mobile-team-bot-vectors.json` (section 8 of the spec). */
internal object TeamsVectors {
    val root: JSONObject get() = SharedVectors.root
    const val NOW_MS = 1793613600000L
    const val UID = "u1"

    /** "N repetitions of the single code point before the multiplication sign" -> the expanded text. */
    fun expand(text: String): String = Regex("(.)×(\\d+)", RegexOption.DOT_MATCHES_ALL).replace(text) { it.groupValues[1].repeat(it.groupValues[2].toInt()) }

    fun members(): List<TeamMember> = root.getJSONArray("members").objects().map { TeamMember(it.getString("uid"), it.getString("displayName"), "member", 1) }

    fun card(json: JSONObject) = TeamBotCard(
        json.getString("key"), json.optString("ownerUid", "owner"), json.getString("name"), "", 1, "all", json.getBoolean("permitted"), json.optBoolean("online", true),
        json.optBoolean("enabled", true), 0, json.stringOrNull("botId"), json.stringOrNull("hostId"),
    )
    fun bots(): List<TeamBotCard> = root.getJSONArray("bots").objects().map(::card)

    fun strings(zh: Boolean): TeamsListStrings = root.getJSONObject("strings").getJSONObject(if (zh) "zh" else "en").let {
        TeamsListStrings(it.getString("channel"), it.getString("directMessage"), it.getString("group"), it.getString("member"), it.getString("you"), it.getString("bot"),
            it.getString("noMessages"), it.getString("recalled"), it.getString("nonText"), it.getString("listSeparator"), it.getString("authorSeparator"))
    }

    fun parseInstant(text: String?): Long? = text?.let { runCatching { Instant.parse(it).toEpochMilli() }.getOrNull() }

    /** `conversations[]` entry: `at` absent = no pointer, null = pointer not stamped yet, string = `lastMessage.at`. */
    fun conversation(json: JSONObject): TeamMessageConversation {
        val participants = json.optJSONArray("participants")?.let { array -> (0 until array.length()).map { array.getString(it) } } ?: emptyList()
        val lastSeq = json.optLong("lastSeq", 0)
        val botKey = json.stringOrNull("botKey")
        val pointer = if (json.has("at")) TeamLastMessage("m" + json.getString("id"), lastSeq, parseInstant(json.stringOrNull("at")), "user", "u2", null, "hi") else null
        return TeamMessageConversation(
            json.getString("id"), json.getString("kind"), json.getString("title"), participants, lastSeq, lastSeq, json.optLong("lastReadSeq", 0),
            if (botKey != null) listOf(botKey) else participants.filter { it.startsWith("bot:") }, pointer,
        )
    }
    fun conversations(): List<TeamMessageConversation> = root.getJSONArray("conversations").objects().map(::conversation)
    fun conversationById(id: String) = conversations().first { it.id == id }

    /** `{id}` only = a fixture conversation, otherwise a full inline one. */
    fun resolve(json: JSONObject): TeamMessageConversation = if (json.length() == 1 && json.has("id")) conversationById(json.getString("id")) else conversation(json)

    fun strings(array: JSONArray): List<String> = (0 until array.length()).map { array.getString(it) }
}
